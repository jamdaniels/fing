// Opt-in end-to-end engine check against real model files and WAV clips.
//
//   FING_DIRECT_MODELS=<models dir> FING_DIRECT_CLIPS=<clips dir> \
//     bun run scripts/cargo-test.ts -- engine_smoke --ignored --nocapture --test-threads=1
//
// Every `*.wav` in the clips dir (16 kHz mono 16-bit) runs through every model
// in the models dir that is in the registry. A clip named `de_*` is
// transcribed with languages [de] and [de, en], anything else with [en].
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
                vec![vec!["de".into()], vec!["de".into(), "en".into()]]
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
