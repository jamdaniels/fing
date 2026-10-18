// Opt-in end-to-end engine check against real model files and WAV clips.
//
//   FING_DIRECT_MODELS=<models dir> FING_DIRECT_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts engine_smoke::engine_smoke -- --ignored --nocapture --test-threads=1
//
// Every `*.wav` in the clips dir (16 kHz mono 16-bit) runs through every model
// in the models dir that is in the registry. A clip named `de_*` is
// transcribed with languages [de], [en] and [en, de], anything else with [en].
// FING_DIRECT_TERMS is an optional comma-separated dictionary. Output is what
// the hotkey path would type; it prints transcripts, so only use your own audio.

use crate::model::{self, ModelEngine};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn read_wav_16k_mono(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    assert_eq!(&bytes[0..4], b"RIFF");
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        if id == b"data" {
            return bytes[body..body + len]
                .chunks_exact(2)
                .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
                .collect();
        }
        offset = body + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
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
    let (Some(models_dir), Some(clips_dir)) =
        (env_dir("FING_DIRECT_MODELS"), env_dir("FING_DIRECT_CLIPS"))
    else {
        eprintln!("set FING_DIRECT_MODELS and FING_DIRECT_CLIPS");
        return;
    };

    let terms = crate::dictionary::sanitize_terms(
        &std::env::var("FING_DIRECT_TERMS")
            .map(|t| {
                t.split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    let prompt = crate::dictionary::build_prompt(&terms);
    let clips = sorted_files(&clips_dir, "wav");

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
        if let Err(err) = crate::transcribe::init_transcriber(model_path.to_str().unwrap()) {
            println!("[{name}] LOAD FAILED: {err}");
            continue;
        }
        println!(
            "[{name}] load {} ms, rss after load {} MB",
            started.elapsed().as_millis(),
            rss_mb()
        );

        for clip in &clips {
            let clip_name = clip.file_name().unwrap().to_string_lossy();
            let audio = read_wav_16k_mono(clip);
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
                let result =
                    crate::transcribe::transcribe_audio(&audio, &languages, prompt.as_deref());
                let ms = started.elapsed().as_millis();
                match result {
                    Ok(text) => {
                        let corrected =
                            crate::dictionary::apply_dictionary_corrections(text.trim(), &terms);
                        println!(
                            "[{name}] {clip_name} {:.1}s {languages:?} {ms} ms: {corrected}",
                            audio.len() as f32 / 16_000.0
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
//   FING_DIRECT_MODELS=<models dir> FING_DIRECT_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts pretranscribe_smoke -- --ignored --nocapture --test-threads=1
//
// FING_DIRECT_CPU=1 loads the models without the GPU (macOS).
#[test]
#[ignore = "needs real model files and clips"]
fn pretranscribe_smoke() {
    use crate::pretranscribe::{commit_due_chunks, transcribe_recording, Committed, Flow};

    let (Some(models_dir), Some(clips_dir)) =
        (env_dir("FING_DIRECT_MODELS"), env_dir("FING_DIRECT_CLIPS"))
    else {
        eprintln!("set FING_DIRECT_MODELS and FING_DIRECT_CLIPS");
        return;
    };
    const RATE: u32 = 16_000;
    const TICK_S: f64 = 0.5;
    let clips: Vec<(String, Vec<f32>)> = sorted_files(&clips_dir, "wav")
        .iter()
        .map(|clip| {
            let name = clip.file_name().unwrap().to_string_lossy().into_owned();
            (name, read_wav_16k_mono(clip))
        })
        .filter(|(_, audio)| audio.len() >= 12 * RATE as usize)
        .collect();

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        crate::transcribe::unload_transcriber();
        if let Err(err) = crate::transcribe::init_transcriber(model_path.to_str().unwrap()) {
            println!("[{name}] LOAD FAILED: {err}");
            continue;
        }
        // Warm-up, so the first measured call does not pay for it.
        if let Some((_, audio)) = clips.first() {
            let _ = crate::transcribe::transcribe_audio(&audio[..RATE as usize * 5], &[], None);
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
                let full = crate::transcribe::transcribe_audio(audio, &languages, None)
                    .expect("full pass");
                let full_ms = started.elapsed().as_secs_f64() * 1000.0;

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
                    let flow = commit_due_chunks(&mut committed, &mut pending, || {
                        clock + tick.elapsed().as_secs_f64() >= end_s
                    });
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
                let chunked =
                    transcribe_recording(Some(committed), audio, &languages, None, Instant::now())
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
//   RUST_LOG=fing_lib::transcribe=info FING_DIRECT_MODELS=<models dir> FING_DIRECT_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts silence_smoke -- --ignored --nocapture --test-threads=1
#[test]
#[ignore = "needs real model files and clips"]
fn silence_smoke() {
    let (Some(models_dir), Some(clips_dir)) =
        (env_dir("FING_DIRECT_MODELS"), env_dir("FING_DIRECT_CLIPS"))
    else {
        eprintln!("set FING_DIRECT_MODELS and FING_DIRECT_CLIPS");
        return;
    };
    let _ = tracing_subscriber::fmt::try_init();
    let clips = sorted_files(&clips_dir, "wav");

    for model_path in sorted_files(&models_dir, "bin") {
        let Some(def) = model::definition_for_path(&model_path) else {
            continue;
        };
        let name = def.filename;
        crate::transcribe::unload_transcriber();
        if let Err(err) = crate::transcribe::init_transcriber(model_path.to_str().unwrap()) {
            println!("[{name}] LOAD FAILED: {err}");
            continue;
        }
        for clip in &clips {
            let clip_name = clip.file_name().unwrap().to_string_lossy();
            let audio = read_wav_16k_mono(clip);
            let languages: Vec<String> = if clip_name.starts_with("de_") {
                vec!["de".into()]
            } else {
                vec!["en".into()]
            };
            let started = Instant::now();
            let text = crate::transcribe::transcribe_audio(&audio, &languages, None)
                .unwrap_or_else(|err| format!("FAILED: {err}"));
            println!(
                "[{name}] {clip_name} {:.1}s: {} ms, blank {}: {text}",
                audio.len() as f32 / 16_000.0,
                started.elapsed().as_millis(),
                crate::hotkey::is_blank_audio(&text),
            );
        }
    }
    crate::transcribe::unload_transcriber();
}
