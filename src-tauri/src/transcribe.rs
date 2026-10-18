// Transcription engines (Whisper and Parakeet on whisper.cpp) and the global
// loaded-engine slot.

use crate::engine::{TranscribeError, TranscriptionEngine};
use crate::inference::{self, InferenceDevice};
use crate::model::{self, ModelEngine};
use crate::whisper_cpp::{self, FullParams, ParakeetContext, WhisperContext};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const DEFAULT_LANGUAGE: &str = "en";
const MAX_PROMPT_TOKENS: usize = 256;
const UNSUPPORTED_LANGUAGE: &str = "yue";
const SAMPLE_RATE: f32 = 16_000.0;

fn get_lang_id(language: &str) -> Option<i32> {
    whisper_cpp::lang_id(language)
}
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

/// Whisper engine: forced or detected language, dictionary prompt.
pub struct WhisperEngine {
    ctx: Mutex<WhisperContext>,
    device: InferenceDevice,
}

impl WhisperEngine {
    pub fn new(model_path: &str) -> Result<Self, TranscribeError> {
        let path = Path::new(model_path);
        if !path.exists() {
            return Err(TranscribeError::ModelNotFound);
        }

        let settings = crate::settings::load_settings_sync();
        let prepared = inference::prepare_context(
            model_path,
            settings.active_model_variant,
            settings.inference_device,
        )?;

        Ok(Self {
            ctx: Mutex::new(prepared.context),
            device: prepared.device,
        })
    }
}

/// A language pinned by an earlier part of the recording, if it is still one
/// of the detection candidates.
fn pinned_candidate<'a>(candidates: &[&'a str], pinned: Option<&str>) -> Option<&'a str> {
    let pinned = pinned?;
    candidates
        .iter()
        .copied()
        .find(|candidate| *candidate == pinned)
}

impl WhisperEngine {
    /// One `whisper_full` pass. `pinned` replaces detection with the language
    /// an earlier part of the same recording used. Returns the text and the
    /// language used.
    fn run(
        &self,
        audio: &[f32],
        languages: &[String],
        dictionary_prompt: Option<&str>,
        pinned: Option<&str>,
    ) -> Result<(String, String), TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let ctx = self.ctx.lock().map_err(|_| {
            TranscribeError::InferenceFailed("Transcriber lock poisoned".to_string())
        })?;
        let mut state = inference::create_state(&ctx, &self.device)?;

        let mut detecting = false;
        let mut pinning = false;
        let detect_started = Instant::now();
        let language = match language_plan(languages) {
            LanguagePlan::Explicit(language) => language,
            LanguagePlan::Detect(candidates) => match pinned_candidate(&candidates, pinned) {
                Some(language) => {
                    pinning = true;
                    language
                }
                None => {
                    detecting = true;
                    let threads = detection_threads();
                    state
                        .pcm_to_mel(audio, threads)
                        .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;
                    let (_, probabilities) = state
                        .lang_detect(0, threads)
                        .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;
                    select_detected_language(&candidates, &probabilities)
                }
            },
        };
        let detect_ms = detect_started.elapsed().as_millis() as u64;
        tracing::info!(
            configured = ?languages,
            language,
            detected = detecting,
            pinned = pinning,
            "Whisper language"
        );

        let mut params = FullParams::greedy();
        params.set_language(language);
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

        let stages = state
            .full(&params, audio)
            .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;

        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            text.push_str(&state.segment_text_lossy(i));
        }
        tracing::info!(
            detect_ms,
            mel_ms = stages.mel_ms(),
            encode_ms = stages.encode_ms(),
            decode_ms = stages.decode_ms(),
            "Whisper stages"
        );

        Ok((text.trim().to_string(), language.to_string()))
    }
}

impl TranscriptionEngine for WhisperEngine {
    fn transcribe(
        &self,
        audio: &[f32],
        languages: &[String],
        dictionary_prompt: Option<&str>,
    ) -> Result<String, TranscribeError> {
        self.run(audio, languages, dictionary_prompt, None)
            .map(|(text, _)| text)
    }

    fn transcribe_part(
        &self,
        audio: &[f32],
        languages: &[String],
        dictionary_prompt: Option<&str>,
        language: Option<&str>,
    ) -> Result<(String, Option<String>), TranscribeError> {
        self.run(audio, languages, dictionary_prompt, language)
            .map(|(text, language)| (text, Some(language)))
    }
}

/// Parakeet TDT engine. Ignores the language settings (the model auto-detects
/// among its 25 European languages) and the dictionary prompt (it has none).
pub struct ParakeetEngine {
    ctx: Mutex<ParakeetContext>,
    device: InferenceDevice,
}

impl ParakeetEngine {
    pub fn new(model_path: &str) -> Result<Self, TranscribeError> {
        if !Path::new(model_path).exists() {
            return Err(TranscribeError::ModelNotFound);
        }

        let settings = crate::settings::load_settings_sync();
        let (ctx, device) = inference::prepare_parakeet(model_path, settings.inference_device)?;

        Ok(Self {
            ctx: Mutex::new(ctx),
            device,
        })
    }
}

impl TranscriptionEngine for ParakeetEngine {
    fn transcribe(
        &self,
        audio: &[f32],
        _languages: &[String],
        _dictionary_prompt: Option<&str>,
    ) -> Result<String, TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let mut ctx = self.ctx.lock().map_err(|_| {
            TranscribeError::InferenceFailed("Transcriber lock poisoned".to_string())
        })?;
        let (text, stages) = ctx
            .transcribe(audio, detection_threads())
            .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;
        inference::mark_executed(&self.device);
        tracing::info!(
            mel_ms = stages.mel_ms(),
            encode_ms = stages.encode_ms(),
            decode_ms = stages.decode_ms(),
            "Parakeet stages"
        );

        Ok(text.trim().to_string())
    }
}

/// The engine currently in memory plus what the timing logs need.
struct LoadedEngine {
    engine: Arc<dyn TranscriptionEngine>,
    kind: ModelEngine,
    model_path: PathBuf,
    load_ms: u128,
    /// Distinct per load, so parts of one recording can tell whether they all
    /// ran on the same engine.
    generation: u64,
}

impl LoadedEngine {
    fn file_name(&self) -> &str {
        self.model_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
    }
}

fn engine_kind_for(path: &Path) -> ModelEngine {
    model::definition_for_path(path)
        .map(|def| def.engine)
        .unwrap_or(ModelEngine::Whisper)
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn load_engine(model_path: &str) -> Result<LoadedEngine, TranscribeError> {
    let path = PathBuf::from(model_path);
    let kind = engine_kind_for(&path);
    let started = Instant::now();
    let engine: Arc<dyn TranscriptionEngine> = match kind {
        ModelEngine::Whisper => Arc::new(WhisperEngine::new(model_path)?),
        ModelEngine::Parakeet => Arc::new(ParakeetEngine::new(model_path)?),
    };
    let loaded = LoadedEngine {
        engine,
        kind,
        model_path: path,
        load_ms: started.elapsed().as_millis(),
        generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
    };
    tracing::info!(
        engine = ?loaded.kind,
        model = loaded.file_name(),
        load_ms = loaded.load_ms as u64,
        whisper_cpp = whisper_cpp::WHISPER_CPP_VERSION,
        "Transcription engine loaded"
    );
    Ok(loaded)
}

// Global engine instance (can be loaded/unloaded/swapped at runtime).
static TRANSCRIBER: Lazy<Mutex<Option<Arc<LoadedEngine>>>> = Lazy::new(|| Mutex::new(None));

/// Initialize the global engine from `model_path`; the engine (Whisper or
/// Parakeet) follows the model registry entry for that file name. A no-op when
/// that model is already loaded; replaces a different loaded model.
pub fn init_transcriber(model_path: &str) -> Result<(), TranscribeError> {
    let mut guard = TRANSCRIBER
        .lock()
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber lock poisoned".to_string()))?;
    if let Some(loaded) = guard.as_ref() {
        if loaded.model_path == Path::new(model_path) {
            return Ok(());
        }
        tracing::info!("Replacing loaded transcription engine");
        *guard = None;
        inference::mark_unloaded();
    }

    tracing::info!("Initializing transcriber");
    *guard = Some(Arc::new(load_engine(model_path)?));

    Ok(())
}

fn get_loaded() -> Option<Arc<LoadedEngine>> {
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
    inference::mark_unloaded();
}

/// Run `transcribe` on the global engine and log its timing (never the text).
fn timed<T>(
    audio: &[f32],
    transcribe: impl FnOnce(&LoadedEngine) -> Result<T, TranscribeError>,
) -> Result<(T, u64), TranscribeError> {
    let Some(loaded) = get_loaded() else {
        return Err(TranscribeError::ModelNotFound);
    };

    let started = Instant::now();
    let result = transcribe(&loaded);
    tracing::info!(
        engine = ?loaded.kind,
        model = loaded.file_name(),
        audio_s = format!("{:.2}", audio.len() as f32 / SAMPLE_RATE),
        load_ms = loaded.load_ms as u64,
        transcribe_ms = started.elapsed().as_millis() as u64,
        ok = result.is_ok(),
        "Transcription timing"
    );
    result.map(|value| (value, loaded.generation))
}

/// Transcribe audio using the global engine and log its timing (never the text).
pub fn transcribe_audio(
    audio: &[f32],
    languages: &[String],
    dictionary_prompt: Option<&str>,
) -> Result<String, TranscribeError> {
    timed(audio, |loaded| {
        loaded
            .engine
            .transcribe(audio, languages, dictionary_prompt)
    })
    .map(|(text, _)| text)
}

/// One part of a recording transcribed by [`transcribe_audio_part`].
#[derive(Debug, Clone)]
pub struct PartTranscript {
    pub text: String,
    /// Language the part used; pass it to the next part of the recording.
    pub language: Option<String>,
    /// Identifies the loaded engine; parts are only combined when it matches.
    pub engine: u64,
}

/// Like [`transcribe_audio`], for one part of a longer recording: `language`
/// pins the language an earlier part used.
pub fn transcribe_audio_part(
    audio: &[f32],
    languages: &[String],
    dictionary_prompt: Option<&str>,
    language: Option<&str>,
) -> Result<PartTranscript, TranscribeError> {
    timed(audio, |loaded| {
        loaded
            .engine
            .transcribe_part(audio, languages, dictionary_prompt, language)
    })
    .map(|((text, language), engine)| PartTranscript {
        text,
        language,
        engine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
