// Opt-in end-to-end engine check against real model files and WAV clips.
//
//   FING_SMOKE_MODELS=<models dir> FING_SMOKE_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts engine_smoke::engine_smoke -- --ignored --nocapture --test-threads=1
//
// Every `*.wav` in the clips dir (16 kHz mono 16-bit) runs through every model
// in the models dir that is in the registry. A clip named `de_*` is
// transcribed with languages [de], [en] and [en, de], anything else with [en].
// FING_SMOKE_TERMS is an optional comma-separated dictionary. Output is what
// the hotkey path would type; it prints transcripts, so only use your own audio.

use crate::audio::{duration_secs, Recording, WHISPER_SAMPLE_RATE};
use crate::engine::{AbortFlag, TranscribeError};
use crate::model::{self, ModelDefinition, ModelEngine};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn read_wav_16k_mono(path: &Path) -> Vec<f32> {
    use rodio::Source;
    let decoder = rodio::Decoder::try_from(std::fs::File::open(path).expect("open wav"))
        .expect("decodable wav");
    assert_eq!(decoder.channels().get(), 1, "mono: {}", path.display());
    assert_eq!(
        decoder.sample_rate().get(),
        WHISPER_SAMPLE_RATE,
        "16 kHz: {}",
        path.display()
    );
    decoder.collect()
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// The models and clips dirs, or None (with a hint) when they are not set.
fn smoke_dirs() -> Option<(PathBuf, PathBuf)> {
    let dirs = env_dir("FING_SMOKE_MODELS").zip(env_dir("FING_SMOKE_CLIPS"));
    if dirs.is_none() {
        eprintln!("set FING_SMOKE_MODELS and FING_SMOKE_CLIPS");
    }
    dirs
}

/// Replace the loaded engine with `def`; prints and returns false on failure.
fn reload(def: &ModelDefinition, model_path: &Path) -> bool {
    crate::transcribe::unload_transcriber();
    if let Err(err) = crate::transcribe::init_transcriber(def.variant, model_path) {
        println!("[{}] LOAD FAILED: {err}", def.filename);
        return false;
    }
    true
}

/// The language a single-language run uses for a clip: [de] for `de_*`, else [en].
fn clip_language(clip_name: &str) -> Vec<String> {
    let language = if clip_name.starts_with("de_") {
        "de"
    } else {
        "en"
    };
    vec![language.to_string()]
}

fn sorted_files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = std::fs::read_dir(dir)
        .expect("readable dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(extension))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// Every clip in `dir` as (file name, audio).
fn load_clips(dir: &Path) -> Vec<(String, Vec<f32>)> {
    sorted_files(dir, "wav")
        .iter()
        .map(|clip| {
            let name = clip.file_name().unwrap().to_string_lossy().into_owned();
            (name, read_wav_16k_mono(clip))
        })
        .collect()
}

fn rss_mb() -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        / 1024
}

#[test]
#[ignore = "needs real model files and clips"]
fn engine_smoke() {
    let Some((models_dir, clips_dir)) = smoke_dirs() else {
        return;
    };

    let terms = crate::dictionary::sanitize_terms(
        &std::env::var("FING_SMOKE_TERMS")
            .map(|t| {
                t.split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    let prompt = crate::dictionary::build_prompt(&terms);
    let clips = load_clips(&clips_dir);

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        crate::transcribe::unload_transcriber();
        println!(
            "== {name} ({:?}), rss before load {} MB",
            def.engine,
            rss_mb()
        );

        let started = Instant::now();
        if !reload(def, &model_path) {
            continue;
        }
        println!(
            "[{name}] load {} ms, rss after load {} MB",
            started.elapsed().as_millis(),
            rss_mb()
        );

        for (clip_name, audio) in &clips {
            let language_sets: Vec<Vec<String>> = if clip_name.starts_with("de_") {
                vec![
                    vec!["de".into()],
                    vec!["en".into()],
                    vec!["en".into(), "de".into()],
                ]
            } else {
                vec![vec!["en".into()]]
            };
            for languages in language_sets {
                let started = Instant::now();
                let result = crate::transcribe::transcribe_audio(
                    audio,
                    &languages,
                    prompt.as_deref(),
                    &AbortFlag::default(),
                );
                let ms = started.elapsed().as_millis();
                match result {
                    Ok(text) => {
                        let corrected =
                            crate::dictionary::apply_dictionary_corrections(text.trim(), &terms);
                        println!(
                            "[{name}] {clip_name} {:.1}s {languages:?} {ms} ms: {corrected}",
                            duration_secs(audio.len(), WHISPER_SAMPLE_RATE)
                        );
                    }
                    Err(err) => println!("[{name}] {clip_name} {languages:?} FAILED: {err}"),
                }
            }
        }
        println!("[{name}] rss after runs {} MB", rss_mb());
        if def.engine == ModelEngine::Parakeet {
            println!("[{name}] (language settings ignored by Parakeet)");
        }
    }
    crate::transcribe::unload_transcriber();
}

/// Word-level edit distance, to compare chunked and full-pass text.
fn word_edits(left: &str, right: &str) -> usize {
    let left: Vec<&str> = left.split_whitespace().collect();
    let right: Vec<&str> = right.split_whitespace().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, l) in left.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(l != r))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[right.len()]
}

// Pre-transcription as the hotkey path runs it, on a simulated clock: audio
// "arrives" in real time, the worker ticks every 500 ms and is busy while it
// transcribes, and on release the tail waits for any chunk still in flight.
// Clips shorter than 12 s are skipped (they never produce a chunk).
//
//   FING_SMOKE_MODELS=<models dir> FING_SMOKE_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts pretranscribe_smoke -- --ignored --nocapture --test-threads=1
//
// FING_SMOKE_CPU=1 loads the models without the GPU (macOS).
#[test]
#[ignore = "needs real model files and clips"]
fn pretranscribe_smoke() {
    use crate::pretranscribe::{commit_due_chunks, transcribe_recording, Committed, Flow};

    let Some((models_dir, clips_dir)) = smoke_dirs() else {
        return;
    };
    const RATE: u32 = WHISPER_SAMPLE_RATE;
    const TICK_S: f64 = 0.5;
    let clips: Vec<(String, Vec<f32>)> = load_clips(&clips_dir)
        .into_iter()
        .filter(|(_, audio)| audio.len() >= 12 * RATE as usize)
        .collect();

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        if !reload(def, &model_path) {
            continue;
        }
        // Warm-up, so the first measured call does not pay for it.
        if let Some((_, audio)) = clips.first() {
            let _ = crate::transcribe::transcribe_audio(
                &audio[..RATE as usize * 5],
                &[],
                None,
                &AbortFlag::default(),
            );
        }

        for (clip_name, audio) in &clips {
            let language_sets: Vec<Vec<String>> = if clip_name.starts_with("de_") {
                vec![vec!["de".into()], vec!["en".into(), "de".into()]]
            } else {
                vec![vec!["en".into()], vec!["en".into(), "de".into()]]
            };
            let end_s = audio.len() as f64 / f64::from(RATE);
            for languages in language_sets {
                let started = Instant::now();
                let full = crate::transcribe::transcribe_audio(
                    audio,
                    &languages,
                    None,
                    &AbortFlag::default(),
                )
                .expect("full pass");
                let full_ms = started.elapsed().as_secs_f64() * 1000.0;

                let abort = AbortFlag::default();
                let mut committed = Committed::new(RATE, languages.clone(), None);
                let mut cut = 0;
                let mut pending = Vec::new();
                let mut clock = TICK_S;
                let mut busy_until: f64 = 0.0;
                while clock < end_s {
                    let read = ((clock * f64::from(RATE)) as usize).min(audio.len());
                    pending.extend_from_slice(&audio[cut + pending.len()..read]);
                    let before = pending.len();
                    let tick = Instant::now();
                    let flow = commit_due_chunks(
                        &mut committed,
                        &mut pending,
                        || clock + tick.elapsed().as_secs_f64() >= end_s,
                        &abort,
                    );
                    cut += before - pending.len();
                    clock += tick.elapsed().as_secs_f64();
                    busy_until = clock;
                    if flow == Flow::Stop {
                        break;
                    }
                    clock += TICK_S;
                }
                // Release at end_s: wait for a chunk that was still running.
                let wait_ms = (busy_until - end_s).max(0.0) * 1000.0;
                let chunks = committed.chunks();
                let started = Instant::now();
                let recording = Recording {
                    samples: audio.clone(),
                    sample_rate: RATE,
                };
                let chunked = transcribe_recording(
                    Some(committed),
                    recording,
                    &languages,
                    None,
                    Instant::now(),
                    &abort,
                )
                .expect("chunked pass");
                let tail_ms = started.elapsed().as_secs_f64() * 1000.0;

                println!(
                    "[{name}] {clip_name} {end_s:.1}s {languages:?}: chunks {chunks}, \
                     tail {:.1}s, release {:.0} ms (wait {wait_ms:.0} + tail {tail_ms:.0}) \
                     vs full {full_ms:.0} ms, word edits {}",
                    end_s - cut as f64 / f64::from(RATE),
                    wait_ms + tail_ms,
                    word_edits(&full, &chunked),
                );
                println!("  full:    {full}");
                println!("  chunked: {chunked}");
            }
        }
    }
    crate::transcribe::unload_transcriber();
}

// Noise and silence handling: per clip, the text and whether the hotkey path
// drops it as blank (`noise_*` clips should). Clips named `de_*` use [de],
// others [en]. Per-stage timing logs need RUST_LOG.
//
//   RUST_LOG=fing_lib::transcribe=info FING_SMOKE_MODELS=<models dir> FING_SMOKE_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts silence_smoke -- --ignored --nocapture --test-threads=1
#[test]
#[ignore = "needs real model files and clips"]
fn silence_smoke() {
    let Some((models_dir, clips_dir)) = smoke_dirs() else {
        return;
    };
    let _ = tracing_subscriber::fmt::try_init();
    let clips = load_clips(&clips_dir);

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        if !reload(def, &model_path) {
            continue;
        }
        for (clip_name, audio) in &clips {
            let languages = clip_language(clip_name);
            let started = Instant::now();
            let text =
                crate::transcribe::transcribe_audio(audio, &languages, None, &AbortFlag::default())
                    .unwrap_or_else(|err| format!("FAILED: {err}"));
            println!(
                "[{name}] {clip_name} {:.1}s: {} ms, blank {}: {text}",
                duration_secs(audio.len(), WHISPER_SAMPLE_RATE),
                started.elapsed().as_millis(),
                crate::transcribe::is_blank_audio(&text),
            );
        }
    }
    crate::transcribe::unload_transcriber();
}

// Aborting a running transcription (Escape while Processing): per model, on
// the longest clip, how long a call takes to return once the flag is set
// mid-call, and that the engine still gives the same text afterwards (the
// Parakeet state is reused across calls). Clips named `de_*` use [de],
// others [en]. Prints transcripts.
//
//   FING_SMOKE_MODELS=<models dir> FING_SMOKE_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts abort_smoke -- --ignored --nocapture --test-threads=1
#[test]
#[ignore = "needs real model files and clips"]
fn abort_smoke() {
    let Some((models_dir, clips_dir)) = smoke_dirs() else {
        return;
    };
    let Some((clip_name, audio)) = load_clips(&clips_dir)
        .into_iter()
        .max_by_key(|(_, audio)| audio.len())
    else {
        eprintln!("no clips");
        return;
    };
    let languages = clip_language(&clip_name);
    let transcribe =
        |abort: &AbortFlag| crate::transcribe::transcribe_audio(&audio, &languages, None, abort);

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        if !reload(def, &model_path) {
            continue;
        }

        let started = Instant::now();
        let baseline = transcribe(&AbortFlag::default()).expect("baseline");
        let full_ms = started.elapsed().as_millis();

        let set = AbortFlag::default();
        set.abort();
        let started = Instant::now();
        assert!(matches!(transcribe(&set), Err(TranscribeError::Aborted)));
        let preset_ms = started.elapsed().as_millis();

        // Abort at a quarter and at half of the full pass.
        for fraction in [4, 2] {
            let abort = AbortFlag::default();
            let delay = std::time::Duration::from_millis((full_ms / fraction) as u64);
            let setter = {
                let abort = abort.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    abort.abort();
                    Instant::now()
                })
            };
            let result = transcribe(&abort);
            let returned = Instant::now();
            let aborted_at = setter.join().unwrap();
            println!(
                "[{name}] {clip_name}: abort at {} ms of {full_ms} ms returned after {} ms ({})",
                delay.as_millis(),
                returned.saturating_duration_since(aborted_at).as_millis(),
                match &result {
                    Ok(_) => "finished first".to_string(),
                    Err(err) => err.to_string(),
                },
            );
        }

        let after = transcribe(&AbortFlag::default()).expect("after abort");
        println!(
            "[{name}] {clip_name} {:.1}s: full {full_ms} ms, preset abort {preset_ms} ms, \
             same text after aborts: {}",
            duration_secs(audio.len(), WHISPER_SAMPLE_RATE),
            after == baseline
        );
        assert_eq!(after, baseline, "engine still works after an abort");
    }
    crate::transcribe::unload_transcriber();
}
