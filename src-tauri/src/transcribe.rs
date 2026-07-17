// Whisper transcription wrapper

use crate::engine::{TranscribeError, TranscriptionEngine};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::{Arc, Mutex};
use whisper_rs::{
    get_lang_id, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

const DEFAULT_LANGUAGE: &str = "en";
const MAX_PROMPT_TOKENS: usize = 256;
const UNSUPPORTED_LANGUAGE: &str = "yue";

#[derive(Debug, PartialEq)]
enum LanguagePlan<'a> {
    Explicit(&'a str),
    Detect(Vec<&'a str>),
}

fn language_plan(languages: &[String]) -> LanguagePlan<'_> {
    let candidates = languages
        .iter()
        .map(String::as_str)
        .filter(|language| !language.eq_ignore_ascii_case(UNSUPPORTED_LANGUAGE))
        .filter(|language| get_lang_id(language).is_some())
        .collect::<Vec<_>>();

    match candidates.as_slice() {
        [] => LanguagePlan::Explicit(DEFAULT_LANGUAGE),
        [language] => LanguagePlan::Explicit(language),
        _ => LanguagePlan::Detect(candidates),
    }
}

fn select_detected_language<'a>(candidates: &[&'a str], probabilities: &[f32]) -> &'a str {
    candidates
        .iter()
        .copied()
        .filter(|language| !language.eq_ignore_ascii_case(UNSUPPORTED_LANGUAGE))
        .filter_map(|language| {
            let language_id = usize::try_from(get_lang_id(language)?).ok()?;
            let probability = *probabilities.get(language_id)?;
            probability.is_finite().then_some((language, probability))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(language, _)| language)
        .unwrap_or(DEFAULT_LANGUAGE)
}

fn detection_threads() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(4)
}

/// Whisper-based transcription engine using whisper-rs.
pub struct Transcriber {
    ctx: Mutex<WhisperContext>,
}

impl Transcriber {
    pub fn new(model_path: &str) -> Result<Self, TranscribeError> {
        let path = Path::new(model_path);
        if !path.exists() {
            return Err(TranscribeError::ModelNotFound);
        }

        #[cfg(target_os = "windows")]
        let mut ctx_params = WhisperContextParameters::default();
        #[cfg(not(target_os = "windows"))]
        let ctx_params = WhisperContextParameters::default();
        #[cfg(target_os = "windows")]
        let prefer_gpu = configure_windows_backend(&mut ctx_params);
        #[cfg(not(target_os = "windows"))]
        let prefer_gpu = false;

        let ctx = match WhisperContext::new_with_params(model_path, ctx_params) {
            Ok(ctx) => ctx,
            Err(err) if prefer_gpu => {
                tracing::warn!(
                    "Failed to initialize Vulkan transcriber: {}. Falling back to CPU.",
                    err
                );
                let mut cpu_params = WhisperContextParameters::default();
                cpu_params.use_gpu(false);
                WhisperContext::new_with_params(model_path, cpu_params)
                    .map_err(|e| TranscribeError::ModelLoadFailed(e.to_string()))?
            }
            Err(err) => return Err(TranscribeError::ModelLoadFailed(err.to_string())),
        };

        Ok(Self {
            ctx: Mutex::new(ctx),
        })
    }
}

#[cfg(target_os = "windows")]
fn configure_windows_backend(ctx_params: &mut WhisperContextParameters<'_>) -> bool {
    let devices = whisper_rs::vulkan::list_devices();
    if let Some(device) = devices.first() {
        tracing::info!(
            "Using Vulkan device {}: {} ({} MiB free / {} MiB total)",
            device.id,
            device.name,
            device.vram.free / 1024 / 1024,
            device.vram.total / 1024 / 1024
        );
        true
    } else {
        tracing::warn!("No Vulkan devices detected. Falling back to CPU.");
        ctx_params.use_gpu(false);
        false
    }
}

impl TranscriptionEngine for Transcriber {
    fn transcribe(
        &self,
        audio: &[f32],
        languages: &[String],
        dictionary_prompt: Option<&str>,
    ) -> Result<String, TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let ctx = self.ctx.lock().map_err(|_| {
            TranscribeError::InferenceFailed("Transcriber lock poisoned".to_string())
        })?;
        let mut state = ctx
            .create_state()
            .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;

        let language = match language_plan(languages) {
            LanguagePlan::Explicit(language) => language,
            LanguagePlan::Detect(candidates) => {
                let threads = detection_threads();
                state
                    .pcm_to_mel(audio, threads)
                    .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;
                let (_, probabilities) = state
                    .lang_detect(0, threads)
                    .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;
                select_detected_language(&candidates, &probabilities)
            }
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(false);
        params.set_no_context(false);

        let prompt_tokens = dictionary_prompt
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .map(|prompt| {
                ctx.tokenize(prompt, MAX_PROMPT_TOKENS)
                    .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))
            })
            .transpose()?;

        if let Some(tokens) = prompt_tokens.as_ref() {
            if !tokens.is_empty() {
                params.set_tokens(tokens);
            }
        }

        state
            .full(params, audio)
            .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;

        let num_segments = state.full_n_segments();
        let mut text = String::new();

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(segment_text) = segment.to_str_lossy() {
                    text.push_str(segment_text.as_ref());
                }
            }
        }

        Ok(text.trim().to_string())
    }
}

// Global transcriber instance (can be loaded/unloaded at runtime).
static TRANSCRIBER: Lazy<Mutex<Option<Arc<Transcriber>>>> = Lazy::new(|| Mutex::new(None));

/// Initialize the global transcriber with the given model file.
/// Safe to call multiple times.
pub fn init_transcriber(model_path: &str) -> Result<(), TranscribeError> {
    let mut guard = TRANSCRIBER
        .lock()
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber lock poisoned".to_string()))?;
    if guard.is_some() {
        return Ok(());
    }

    tracing::info!("Initializing transcriber");
    *guard = Some(Arc::new(Transcriber::new(model_path)?));

    Ok(())
}

/// Get the global transcriber instance (None if not initialized).
pub fn get_transcriber() -> Option<Arc<Transcriber>> {
    match TRANSCRIBER.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }
}

/// Whether the global transcriber is currently loaded.
pub fn is_transcriber_loaded() -> bool {
    match TRANSCRIBER.lock() {
        Ok(guard) => guard.is_some(),
        Err(_) => false,
    }
}

/// Unload the global transcriber.
pub fn unload_transcriber() {
    if let Ok(mut guard) = TRANSCRIBER.lock() {
        *guard = None;
    }
}

/// Transcribe audio using the global transcriber.
pub fn transcribe_audio(
    audio: &[f32],
    languages: &[String],
    dictionary_prompt: Option<&str>,
) -> Result<String, TranscribeError> {
    match get_transcriber() {
        Some(t) => t.transcribe(audio, languages, dictionary_prompt),
        None => Err(TranscribeError::ModelNotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TRANSCRIBE_TEST_MUTEX: Mutex<()> = Mutex::new(());

    struct TranscriberReset;

    impl Drop for TranscriberReset {
        fn drop(&mut self) {
            unload_transcriber();
        }
    }

    fn missing_model_path() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        std::env::temp_dir()
            .join(format!("fing-missing-model-{nanos}.bin"))
            .to_string_lossy()
            .into_owned()
    }

    fn probabilities_with(values: &[(&str, f32)]) -> Vec<f32> {
        let max_language_id = values
            .iter()
            .filter_map(|(language, _)| get_lang_id(language))
            .max()
            .unwrap_or_default();
        let mut probabilities = vec![0.0; usize::try_from(max_language_id + 1).unwrap_or(0)];

        for (language, probability) in values {
            if let Some(language_id) = get_lang_id(language) {
                probabilities[usize::try_from(language_id).expect("language id should be valid")] =
                    *probability;
            }
        }

        probabilities
    }

    #[test]
    fn detection_selects_only_from_safe_candidates() {
        let probabilities =
            probabilities_with(&[("en", 0.95), ("yue", 0.99), ("de", 0.40), ("fr", 0.70)]);

        assert_eq!(
            select_detected_language(&["de", "fr"], &probabilities),
            "fr"
        );
    }

    #[test]
    fn detection_ignores_cantonese_and_invalid_codes() {
        let probabilities = probabilities_with(&[("yue", 0.99), ("de", 0.40)]);

        assert_eq!(
            select_detected_language(&["yue", "invalid", "de"], &probabilities),
            "de"
        );
    }

    #[test]
    fn detection_without_safe_candidates_falls_back_to_english() {
        let probabilities = probabilities_with(&[("yue", 0.99)]);

        assert_eq!(
            select_detected_language(&["yue", "invalid"], &probabilities),
            "en"
        );
    }

    #[test]
    fn a_single_safe_candidate_bypasses_detection() {
        let languages = vec!["yue".to_string(), "invalid".to_string(), "de".to_string()];

        assert_eq!(language_plan(&languages), LanguagePlan::Explicit("de"));
    }

    #[test]
    fn transcribe_audio_requires_loaded_transcriber() {
        let _guard = TRANSCRIBE_TEST_MUTEX
            .lock()
            .expect("transcribe test mutex should lock");
        let _reset = TranscriberReset;

        unload_transcriber();

        assert!(matches!(
            transcribe_audio(&[0.25], &[], None),
            Err(TranscribeError::ModelNotFound)
        ));
        assert!(!is_transcriber_loaded());
    }

    #[test]
    fn init_transcriber_returns_model_not_found_for_missing_file() {
        let _guard = TRANSCRIBE_TEST_MUTEX
            .lock()
            .expect("transcribe test mutex should lock");
        let _reset = TranscriberReset;

        unload_transcriber();
        let missing_path = missing_model_path();

        assert!(matches!(
            init_transcriber(&missing_path),
            Err(TranscribeError::ModelNotFound)
        ));
        assert!(!is_transcriber_loaded());
    }

    #[test]
    fn unload_transcriber_keeps_global_state_unloaded() {
        let _guard = TRANSCRIBE_TEST_MUTEX
            .lock()
            .expect("transcribe test mutex should lock");
        let _reset = TranscriberReset;

        unload_transcriber();
        unload_transcriber();

        assert!(get_transcriber().is_none());
        assert!(!is_transcriber_loaded());
    }
}
