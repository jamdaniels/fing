// Background pre-transcription: while a recording runs, finished parts of the
// audio are transcribed so that on release only the tail is left. Invisible to
// the user; recordings shorter than MIN_CHUNK_MS never produce a chunk and take
// the single full pass unchanged.
//
// The worker never loads or unloads a model (model_loader owns that). It skips
// a tick while no engine is loaded, e.g. while a lazy preload is in flight.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{resample_to_16k, AudioTap, WHISPER_SAMPLE_RATE};
use crate::engine::TranscribeError;
use crate::hotkey::{is_blank_audio, is_digital_silence};
use crate::transcribe::{
    is_transcriber_loaded, transcribe_audio, transcribe_audio_part, PartTranscript,
};

/// Kill switch. Deliberately not a persisted setting: settings load/save is
/// regression-critical.
const PRETRANSCRIBE_ENABLED: bool = true;

/// How often the worker looks at the live buffer.
const TICK: Duration = Duration::from_millis(500);

// Chunk planner: energy VAD on short frames with an adaptive noise floor.
/// Analysis frame length.
const FRAME_MS: usize = 30;
/// A cut needs a pause at least this long...
const MIN_PAUSE_MS: usize = 600;
/// ...that has ended: this much activity must follow it. An open pause at the
/// end of the buffer is where the user may release; cutting there would leave
/// a tail of pure room noise, which Whisper tends to hallucinate on.
const PAUSE_CLOSE_MS: usize = 90;
/// No cut before this much uncommitted audio, so short dictations take the
/// unchanged single pass.
const MIN_CHUNK_MS: usize = 12_000;
/// Hard cap on a chunk (Whisper's window is 30 s).
const MAX_CHUNK_MS: usize = 25_000;
/// At the cap, a cut goes into the longest shorter pause; with no gap at all,
/// at the quietest frame this close to the cap.
const FORCED_CUT_WINDOW_MS: usize = 3_000;
/// A chunk needs at least this much activity; silence alone is never cut.
const MIN_ACTIVE_MS: usize = 500;
/// Frame energy percentile taken as the noise floor.
const NOISE_FLOOR_PERCENTILE: f32 = 0.10;
/// A frame is active this far above the noise floor...
const ACTIVE_MARGIN_DB: f32 = 10.0;
/// ...or above this level, so a loud, steady signal is never one long pause.
const ACTIVE_THRESHOLD_MAX_DBFS: f32 = -35.0;
/// Level of a digitally silent frame.
const MIN_FRAME_DBFS: f32 = -100.0;
/// With this much uncommitted audio and still no cut (no activity in the first
/// MAX_CHUNK_MS, or a very long open pause), leave the rest of the recording
/// to the tail pass and stop copying it.
const GIVE_UP_MS: usize = 60_000;

fn frame_len(sample_rate: u32) -> usize {
    (sample_rate as usize * FRAME_MS / 1000).max(1)
}

fn ms_to_frames(ms: usize) -> usize {
    ms / FRAME_MS
}

fn frame_dbfs(frame: &[f32]) -> f32 {
    let mean_square =
        frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len().max(1) as f32;
    let db = 10.0 * mean_square.log10();
    if db.is_finite() {
        db.max(MIN_FRAME_DBFS)
    } else {
        MIN_FRAME_DBFS
    }
}

fn active_threshold(frame_db: &[f32]) -> f32 {
    let mut sorted = frame_db.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = ((sorted.len() as f32 * NOISE_FLOOR_PERCENTILE) as usize).min(sorted.len() - 1);
    (sorted[index] + ACTIVE_MARGIN_DB).min(ACTIVE_THRESHOLD_MAX_DBFS)
}

/// Index of the quietest frame in `range` (the first one on a tie).
fn quietest(frame_db: &[f32], range: std::ops::Range<usize>) -> usize {
    range
        .min_by(|left, right| frame_db[*left].total_cmp(&frame_db[*right]))
        .unwrap_or(0)
}

/// A run of inactive frames, `start..end`.
struct Pause {
    start: usize,
    end: usize,
    /// Followed by PAUSE_CLOSE_MS of activity.
    closed: bool,
    /// Active frames before it.
    active_before: usize,
}

/// Where the next chunk of `samples` (the uncommitted audio, mono at
/// `sample_rate`) should end, as an offset into it; `None` while no cut is due.
fn plan_cut(samples: &[f32], sample_rate: u32) -> Option<usize> {
    let frame_len = frame_len(sample_rate);
    let min_chunk = ms_to_frames(MIN_CHUNK_MS);
    let max_chunk = ms_to_frames(MAX_CHUNK_MS);
    if samples.len() / frame_len <= min_chunk {
        return None;
    }
    let frame_db: Vec<f32> = samples.chunks_exact(frame_len).map(frame_dbfs).collect();

    let threshold = active_threshold(&frame_db);
    let active: Vec<bool> = frame_db.iter().map(|db| *db > threshold).collect();
    let mut pauses = Vec::new();
    let mut active_before = 0;
    let mut frame = 0;
    while frame < frame_db.len().min(max_chunk) {
        if active[frame] {
            active_before += 1;
            frame += 1;
            continue;
        }
        // Activity shorter than PAUSE_CLOSE_MS (a click) stays part of the pause.
        let start = frame;
        let closed = loop {
            while frame < frame_db.len() && !active[frame] {
                frame += 1;
            }
            let burst = active[frame..]
                .iter()
                .take(ms_to_frames(PAUSE_CLOSE_MS))
                .take_while(|a| **a)
                .count();
            if burst == ms_to_frames(PAUSE_CLOSE_MS) {
                break true;
            }
            if frame + burst >= frame_db.len() {
                break false;
            }
            frame += burst;
        };
        pauses.push(Pause {
            start,
            end: frame,
            closed,
            active_before,
        });
    }

    // Where a closed pause after enough activity overlaps min_chunk..max_chunk.
    let cut_range = |pause: &Pause| {
        let (low, high) = (pause.start.max(min_chunk), pause.end.min(max_chunk));
        (pause.closed && pause.active_before >= ms_to_frames(MIN_ACTIVE_MS) && low < high)
            .then_some(low..high)
    };
    let cut_at = |range: std::ops::Range<usize>| {
        let frame = quietest(&frame_db, range);
        frame * frame_len + frame_len / 2
    };

    // The first long pause.
    if let Some(range) = pauses
        .iter()
        .filter(|pause| pause.end - pause.start >= ms_to_frames(MIN_PAUSE_MS))
        .find_map(cut_range)
    {
        return Some(cut_at(range));
    }

    // Hard cap. Never inside a pause that is still open (the user may be
    // about to release), and never without activity.
    if frame_db.len() < max_chunk
        || pauses.iter().any(|pause| !pause.closed)
        || active[..max_chunk].iter().filter(|a| **a).count() < ms_to_frames(MIN_ACTIVE_MS)
    {
        return None;
    }
    // The longest shorter pause (usually a sentence end, so no sentence is
    // split), else the quietest frame just before the cap.
    let range = pauses
        .iter()
        .filter_map(|pause| cut_range(pause).map(|range| (pause.end - pause.start, range)))
        .max_by_key(|(len, _)| *len)
        .map(|(_, range)| range)
        .unwrap_or(max_chunk - ms_to_frames(FORCED_CUT_WINDOW_MS)..max_chunk);
    Some(cut_at(range))
}

/// A native-rate sample index mapped onto the 16 kHz buffer.
fn to_16k_index(native_index: usize, native_rate: u32) -> usize {
    (native_index as u64 * u64::from(WHISPER_SAMPLE_RATE) / u64::from(native_rate.max(1))) as usize
}

/// What a recording's worker committed: the audio before `cut` is
/// transcribed into `texts`.
#[derive(Debug, Default)]
pub struct Committed {
    /// End of the committed audio, in native-rate samples.
    cut: usize,
    native_rate: u32,
    chunks: usize,
    /// Non-blank chunk texts, in order.
    texts: Vec<String>,
    /// Language the chunks used (Whisper with several candidate languages).
    language: Option<String>,
    /// Engine the chunks ran on.
    engine: Option<u64>,
    languages: Vec<String>,
    dictionary_prompt: Option<String>,
}

impl Committed {
    pub(crate) fn new(
        native_rate: u32,
        languages: Vec<String>,
        dictionary_prompt: Option<String>,
    ) -> Self {
        Self {
            native_rate,
            languages,
            dictionary_prompt,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn chunks(&self) -> usize {
        self.chunks
    }

    fn commit(&mut self, offset: usize, part: Option<PartTranscript>) {
        self.cut += offset;
        self.chunks += 1;
        let Some(part) = part else {
            return;
        };
        self.engine = Some(part.engine);
        if self.language.is_none() {
            self.language = part.language;
        }
        self.push_text(&part.text);
    }

    fn push_text(&mut self, text: &str) {
        let text = text.trim();
        if !is_blank_audio(text) {
            self.texts.push(text.to_string());
        }
    }
}

struct Worker {
    session_id: u64,
    /// Dropping it wakes the worker and stops it.
    stop_tx: Sender<()>,
    handle: JoinHandle<Committed>,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

fn lock_worker() -> std::sync::MutexGuard<'static, Option<Worker>> {
    match WORKER.lock() {
        Ok(worker) => worker,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn take_worker() -> Option<Worker> {
    lock_worker().take()
}

/// Start pre-transcribing the recording behind `tap` (recording `session_id`).
/// The worker exits once the tap goes stale.
pub fn start(
    tap: AudioTap,
    session_id: u64,
    languages: Vec<String>,
    dictionary_prompt: Option<String>,
) {
    cancel();
    if !PRETRANSCRIBE_ENABLED {
        return;
    }

    let (stop_tx, stop_rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("pretranscribe".to_string())
        .spawn(move || run(&tap, &stop_rx, languages, dictionary_prompt));
    match spawned {
        Ok(handle) => {
            *lock_worker() = Some(Worker {
                session_id,
                stop_tx,
                handle,
            });
        }
        Err(e) => tracing::warn!("Failed to start pre-transcription: {}", e),
    }
}

/// Stop the worker and drop whatever it transcribed (Escape). A chunk in
/// flight finishes in the background and its result is discarded.
pub fn cancel() {
    drop(take_worker());
}

/// A stopped worker whose committed chunks can still be collected.
pub struct Stopped {
    handle: JoinHandle<Committed>,
}

impl Stopped {
    /// Wait for a chunk in flight, then return what was committed. Blocking.
    pub fn wait(self) -> Option<Committed> {
        match self.handle.join() {
            Ok(committed) => Some(committed),
            Err(_) => {
                tracing::warn!("Pre-transcription worker panicked");
                None
            }
        }
    }
}

/// Stop the worker for `session_id` without waiting for it. A worker from
/// another session is cancelled instead, so its text can never be used.
pub fn stop(session_id: u64) -> Option<Stopped> {
    let worker = take_worker()?;
    if worker.session_id != session_id {
        return None;
    }
    drop(worker.stop_tx);
    Some(Stopped {
        handle: worker.handle,
    })
}

/// Whether the worker goes on after [`commit_due_chunks`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Stop,
}

/// Transcribe and commit every chunk the planner cuts from `pending` (the
/// native-rate audio after `committed.cut`). `stop_requested` is checked
/// before each chunk.
pub(crate) fn commit_due_chunks(
    committed: &mut Committed,
    pending: &mut Vec<f32>,
    stop_requested: impl Fn() -> bool,
) -> Flow {
    let rate = committed.native_rate;
    while let Some(offset) = plan_cut(pending, rate) {
        if stop_requested() {
            return Flow::Stop;
        }
        let started = Instant::now();
        let audio = resample_to_16k(pending[..offset].to_vec(), rate);
        let part = if is_digital_silence(&audio) {
            None
        } else {
            match transcribe_audio_part(
                &audio,
                &committed.languages,
                committed.dictionary_prompt.as_deref(),
                committed.language.as_deref(),
            ) {
                Ok(part) if committed.engine.is_some_and(|engine| engine != part.engine) => {
                    tracing::warn!("Engine changed during pre-transcription; dropping chunks");
                    *committed = Committed::default();
                    return Flow::Stop;
                }
                Ok(part) => Some(part),
                // Unloaded meanwhile; nothing committed, retry next tick.
                Err(TranscribeError::ModelNotFound) => return Flow::Continue,
                Err(e) => {
                    tracing::warn!("Pre-transcription chunk failed: {}", e);
                    return Flow::Stop;
                }
            }
        };
        tracing::info!(
            chunk = committed.chunks,
            audio_s = format!("{:.2}", audio.len() as f32 / WHISPER_SAMPLE_RATE as f32),
            transcribe_ms = started.elapsed().as_millis() as u64,
            silent = part.is_none(),
            "Pre-transcribed chunk"
        );
        committed.commit(offset, part);
        pending.drain(..offset);
    }

    if pending.len() > GIVE_UP_MS * rate as usize / 1000 {
        tracing::info!("No chunk cut point; leaving the rest to the release pass");
        return Flow::Stop;
    }
    Flow::Continue
}

fn run(
    tap: &AudioTap,
    stop_rx: &Receiver<()>,
    languages: Vec<String>,
    dictionary_prompt: Option<String>,
) -> Committed {
    let mut committed = Committed::new(tap.sample_rate(), languages, dictionary_prompt);
    let mut pending = Vec::new();

    while matches!(stop_rx.recv_timeout(TICK), Err(RecvTimeoutError::Timeout)) {
        // Without an engine nothing is copied; the copy catches up once one
        // is loaded.
        if !is_transcriber_loaded() {
            if !tap.is_live() {
                break;
            }
            continue;
        }
        tap.append_from(committed.cut + pending.len(), &mut pending);
        // Checked after the copy: if the recording ended meanwhile, the copy
        // may hold another recording's audio.
        if !tap.is_live() {
            break;
        }
        let stop_requested = || !matches!(stop_rx.try_recv(), Err(TryRecvError::Empty));
        if commit_due_chunks(&mut committed, &mut pending, stop_requested) == Flow::Stop {
            break;
        }
    }
    committed
}

/// Transcribe a finished recording. `audio` is the whole recording at 16 kHz.
/// With committed chunks, only the tail after them is transcribed and the
/// texts are joined; otherwise (or when the chunks no longer match the
/// settings or the engine) this is the same single pass as without
/// pre-transcription.
pub fn transcribe_recording(
    committed: Option<Committed>,
    audio: &[f32],
    languages: &[String],
    dictionary_prompt: Option<&str>,
    released: Instant,
) -> Result<String, TranscribeError> {
    let committed = committed.filter(|committed| {
        let usable = committed.chunks > 0
            && committed.languages == languages
            && committed.dictionary_prompt.as_deref() == dictionary_prompt;
        if committed.chunks > 0 && !usable {
            tracing::info!("Settings changed during the recording; dropping chunks");
        }
        usable
    });
    let Some(mut committed) = committed else {
        return transcribe_audio(audio, languages, dictionary_prompt);
    };

    let tail_start = to_16k_index(committed.cut, committed.native_rate).min(audio.len());
    let tail = &audio[tail_start..];
    let started = Instant::now();
    if !tail.is_empty() && !is_digital_silence(tail) {
        let part = transcribe_audio_part(
            tail,
            languages,
            dictionary_prompt,
            committed.language.as_deref(),
        )?;
        if committed.engine.is_some_and(|engine| engine != part.engine) {
            tracing::warn!("Engine changed since pre-transcription; transcribing in full");
            return transcribe_audio(audio, languages, dictionary_prompt);
        }
        committed.push_text(&part.text);
    }
    tracing::info!(
        chunks = committed.chunks,
        committed_s = format!("{:.2}", tail_start as f32 / WHISPER_SAMPLE_RATE as f32),
        tail_s = format!("{:.2}", tail.len() as f32 / WHISPER_SAMPLE_RATE as f32),
        tail_ms = started.elapsed().as_millis() as u64,
        release_ms = released.elapsed().as_millis() as u64,
        "Pre-transcription"
    );
    Ok(committed.texts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn samples(rate: u32, ms: usize) -> usize {
        rate as usize * ms / 1000
    }

    /// Low deterministic noise (about -66 dBFS RMS).
    fn noise(rate: u32, ms: usize, seed: &mut u32) -> Vec<f32> {
        (0..samples(rate, ms))
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.0017
            })
            .collect()
    }

    fn tone(rate: u32, ms: usize, amplitude: f32) -> Vec<f32> {
        (0..samples(rate, ms))
            .map(|i| {
                amplitude * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / rate as f32).sin()
            })
            .collect()
    }

    /// Speech-like audio: 200 ms syllables with 60 ms gaps over room noise.
    fn speech(rate: u32, ms: usize, seed: &mut u32) -> Vec<f32> {
        let mut out = Vec::with_capacity(samples(rate, ms));
        while out.len() < samples(rate, ms) {
            out.extend(tone(rate, 200, 0.2));
            out.extend(noise(rate, 60, seed));
        }
        out.truncate(samples(rate, ms));
        out
    }

    fn secs(index: usize, rate: u32) -> f32 {
        index as f32 / rate as f32
    }

    #[test]
    fn short_clip_has_no_cut() {
        let mut seed = 1;
        for ms in [3_000, 8_000, 11_900] {
            let mut audio = speech(RATE, ms / 2, &mut seed);
            audio.extend(noise(RATE, 1_000, &mut seed));
            audio.extend(speech(RATE, ms / 2 - 1_000, &mut seed));
            assert_eq!(plan_cut(&audio, RATE), None, "{ms} ms");
        }
    }

    #[test]
    fn pause_after_min_chunk_is_cut_at_its_quietest_frame() {
        let mut seed = 2;
        let mut audio = speech(RATE, 13_000, &mut seed);
        let mut pause = noise(RATE, 1_000, &mut seed);
        // The quietest spot of the pause: 60 ms of digital silence at +500 ms.
        let quiet = samples(RATE, 500)..samples(RATE, 560);
        pause[quiet].iter_mut().for_each(|sample| *sample = 0.0);
        audio.extend(pause);
        audio.extend(speech(RATE, 2_000, &mut seed));

        let cut = plan_cut(&audio, RATE).expect("cut inside the pause");
        assert!(
            (13.49..=13.57).contains(&secs(cut, RATE)),
            "cut at {} s",
            secs(cut, RATE)
        );
    }

    #[test]
    fn pause_before_min_chunk_is_not_used() {
        let mut seed = 3;
        let mut audio = speech(RATE, 5_000, &mut seed);
        audio.extend(noise(RATE, 1_000, &mut seed));
        audio.extend(speech(RATE, 10_000, &mut seed));
        assert_eq!(plan_cut(&audio, RATE), None);
    }

    #[test]
    fn short_pause_is_not_a_cut_point() {
        let mut seed = 4;
        let mut audio = speech(RATE, 13_000, &mut seed);
        audio.extend(noise(RATE, 400, &mut seed));
        audio.extend(speech(RATE, 3_000, &mut seed));
        assert_eq!(plan_cut(&audio, RATE), None);
    }

    #[test]
    fn open_pause_at_the_end_waits_for_speech() {
        let mut seed = 5;
        let mut audio = speech(RATE, 13_000, &mut seed);
        audio.extend(noise(RATE, 1_500, &mut seed));
        assert_eq!(plan_cut(&audio, RATE), None, "pause still open");

        audio.extend(speech(RATE, 300, &mut seed));
        let cut = plan_cut(&audio, RATE).expect("closed pause");
        assert!((12.9..14.5).contains(&secs(cut, RATE)));
    }

    #[test]
    fn continuous_tone_past_the_cap_is_force_cut() {
        let audio = tone(RATE, 24_000, 0.3);
        assert_eq!(plan_cut(&audio, RATE), None, "below the cap");

        let audio = tone(RATE, 26_000, 0.3);
        let cut = plan_cut(&audio, RATE).expect("forced cut");
        let at = secs(cut, RATE);
        assert!((22.0..25.0).contains(&at), "forced cut at {at} s");
    }

    #[test]
    fn forced_cut_uses_a_short_gap_near_the_cap() {
        let mut seed = 6;
        // Continuous speech, with one short (non-pause) dip at 23.5 s.
        let mut audio = speech(RATE, 23_500, &mut seed);
        audio.extend(vec![0.0; samples(RATE, 120)]);
        audio.extend(tone(RATE, 3_000, 0.2));
        let cut = plan_cut(&audio, RATE).expect("forced cut");
        assert!(
            (23.5..23.62).contains(&secs(cut, RATE)),
            "cut at {}",
            secs(cut, RATE)
        );
    }

    #[test]
    fn forced_cut_prefers_the_longest_pause() {
        let mut seed = 9;
        let mut audio = speech(RATE, 15_000, &mut seed);
        audio.extend(noise(RATE, 400, &mut seed));
        audio.extend(speech(RATE, 8_000, &mut seed));
        audio.extend(noise(RATE, 200, &mut seed));
        audio.extend(speech(RATE, 3_000, &mut seed));
        assert!(audio.len() > samples(RATE, MAX_CHUNK_MS));
        let at = secs(plan_cut(&audio, RATE).expect("forced cut"), RATE);
        assert!((14.9..15.45).contains(&at), "cut at {at} s");
    }

    #[test]
    fn forced_cut_waits_for_an_open_pause_at_the_cap() {
        let mut seed = 10;
        let mut audio = speech(RATE, 24_700, &mut seed);
        audio.extend(noise(RATE, 1_300, &mut seed));
        assert_eq!(plan_cut(&audio, RATE), None, "pause still open");

        audio.extend(speech(RATE, 300, &mut seed));
        let at = secs(plan_cut(&audio, RATE).expect("closed pause"), RATE);
        assert!((24.6..25.0).contains(&at), "cut at {at} s");
    }

    #[test]
    fn a_click_does_not_split_a_pause() {
        let mut seed = 11;
        let mut audio = speech(RATE, 13_000, &mut seed);
        audio.extend(noise(RATE, 400, &mut seed));
        audio.extend(tone(RATE, 30, 0.3));
        audio.extend(noise(RATE, 400, &mut seed));
        audio.extend(speech(RATE, 2_000, &mut seed));
        let at = secs(plan_cut(&audio, RATE).expect("one long pause"), RATE);
        assert!((12.9..13.9).contains(&at), "cut at {at} s");
    }

    #[test]
    fn silence_only_has_no_cut() {
        let mut seed = 7;
        assert_eq!(plan_cut(&vec![0.0; samples(RATE, 60_000)], RATE), None);
        assert_eq!(plan_cut(&noise(RATE, 60_000, &mut seed), RATE), None);
        assert_eq!(plan_cut(&[], RATE), None);
    }

    #[test]
    fn frame_boundaries_and_rates() {
        let mut seed = 8;
        for rate in [16_000, 44_100, 48_000] {
            let frame = frame_len(rate);
            // Exactly MIN_CHUNK_MS of frames, and one partial frame more.
            let mut audio = speech(rate, MIN_CHUNK_MS, &mut seed);
            audio.truncate(ms_to_frames(MIN_CHUNK_MS) * frame);
            assert_eq!(plan_cut(&audio, rate), None, "{rate} Hz at the minimum");
            audio.extend(vec![0.0; frame - 1]);
            assert_eq!(plan_cut(&audio, rate), None, "{rate} Hz partial frame");

            // A pause straddling the minimum: the cut lands at or after it,
            // inside the buffer, mid-frame.
            let mut audio = speech(rate, MIN_CHUNK_MS - 300, &mut seed);
            audio.extend(noise(rate, 900, &mut seed));
            audio.extend(speech(rate, 500, &mut seed));
            audio.extend(vec![0.1; 7]);
            let cut = plan_cut(&audio, rate).expect("pause straddling the minimum");
            assert!(cut >= ms_to_frames(MIN_CHUNK_MS) * frame, "{rate} Hz");
            assert!(cut < samples(rate, MIN_CHUNK_MS + 600), "{rate} Hz");
            assert_eq!(cut % frame, frame / 2, "{rate} Hz");
        }
    }

    #[test]
    fn native_cut_maps_onto_the_16k_buffer() {
        assert_eq!(to_16k_index(480_000, 48_000), 160_000);
        assert_eq!(to_16k_index(441_000, 44_100), 160_000);
        assert_eq!(to_16k_index(160_000, 16_000), 160_000);
        assert_eq!(to_16k_index(2, 48_000), 0);
        assert_eq!(to_16k_index(0, 48_000), 0);
        // 300 s at 96 kHz does not overflow.
        assert_eq!(to_16k_index(28_800_000, 96_000), 4_800_000);
    }

    fn part(text: &str, language: Option<&str>) -> Option<PartTranscript> {
        Some(PartTranscript {
            text: text.to_string(),
            language: language.map(String::from),
            engine: 1,
        })
    }

    #[test]
    fn commit_keeps_the_first_language_and_drops_blank_texts() {
        let mut committed = Committed::default();
        committed.commit(100, part(" Hello there. ", Some("de")));
        committed.commit(50, None);
        committed.commit(70, part("[BLANK_AUDIO]", Some("en")));
        committed.commit(30, part("", Some("en")));
        committed.commit(20, part("Again.", Some("en")));

        assert_eq!(committed.cut, 270);
        assert_eq!(committed.chunks, 5);
        assert_eq!(committed.texts, vec!["Hello there.", "Again."]);
        assert_eq!(committed.language.as_deref(), Some("de"));
    }

    #[test]
    fn stop_never_returns_another_sessions_worker() {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = stop_rx.recv();
            Committed {
                chunks: 1,
                ..Committed::default()
            }
        });
        *WORKER.lock().unwrap() = Some(Worker {
            session_id: 7,
            stop_tx,
            handle,
        });
        assert!(stop(8).is_none(), "other session");
        assert!(take_worker().is_none(), "mismatched worker is dropped");
    }
}
