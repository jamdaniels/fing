// Transcription engines (Whisper and Parakeet on whisper.cpp) and the global
// loaded-engine slot.

use crate::audio::WHISPER_SAMPLE_RATE;
use crate::engine::{AbortFlag, TranscribeError, TranscriptionEngine};
use crate::inference::InferenceDevicePreference;
use crate::inference::{self, InferenceDevice};
use crate::model::{self, ModelDefinition, ModelEngine, ModelVariant};
use crate::whisper_cpp::{self, lang_id, FfiError, FullParams, ParakeetContext, WhisperContext};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
        .filter(|language| lang_id(language).is_some())
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
            let language_id = usize::try_from(lang_id(language)?).ok()?;
            let probability = *probabilities.get(language_id)?;
            probability.is_finite().then_some((language, probability))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(language, _)| language)
        .unwrap_or(DEFAULT_LANGUAGE)
}

/// The error for a failed whisper.cpp inference call.
fn inference_error(error: FfiError) -> TranscribeError {
    match error {
        FfiError::Aborted => TranscribeError::Aborted,
        FfiError::Failed(message) => TranscribeError::InferenceFailed(message),
    }
}

fn aborted(abort: &AbortFlag) -> Result<(), TranscribeError> {
    if abort.is_aborted() {
        return Err(TranscribeError::Aborted);
    }
    Ok(())
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
    fn new(
        model_path: &str,
        variant: ModelVariant,
        preference: InferenceDevicePreference,
    ) -> Result<Self, TranscribeError> {
        let prepared = inference::prepare_context(model_path, variant, preference)?;

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
        abort: &AbortFlag,
    ) -> Result<(String, String), TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let ctx = self.ctx.lock().map_err(|_| {
            TranscribeError::InferenceFailed("Transcriber lock poisoned".to_string())
        })?;
        // The lock may have waited for another call of the same recording.
        aborted(abort)?;
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

        // Language detection can't be aborted; check before the main pass.
        aborted(abort)?;
        let stages = state
            .full(&params, audio, abort.as_atomic())
            .map_err(inference_error)?;

        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            text.push_str(&state.segment_text_lossy(i));
        }
        tracing::info!(
            detect_ms,
            mel_ms = stages.mel_ms,
            encode_ms = stages.encode_ms,
            decode_ms = stages.decode_ms,
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
        language: Option<&str>,
        abort: &AbortFlag,
    ) -> Result<(String, Option<String>), TranscribeError> {
        self.run(audio, languages, dictionary_prompt, language, abort)
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
    fn new(
        model_path: &str,
        variant: ModelVariant,
        preference: InferenceDevicePreference,
    ) -> Result<Self, TranscribeError> {
        let prepared = inference::prepare_parakeet(model_path, variant, preference)?;

        Ok(Self {
            ctx: Mutex::new(prepared.context),
            device: prepared.device,
        })
    }
}

impl TranscriptionEngine for ParakeetEngine {
    fn transcribe(
        &self,
        audio: &[f32],
        _languages: &[String],
        _dictionary_prompt: Option<&str>,
        _language: Option<&str>,
        abort: &AbortFlag,
    ) -> Result<(String, Option<String>), TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let mut ctx = self.ctx.lock().map_err(|_| {
            TranscribeError::InferenceFailed("Transcriber lock poisoned".to_string())
        })?;
        aborted(abort)?;
        let (text, stages) = ctx
            .transcribe(audio, detection_threads(), abort.as_atomic())
            .map_err(inference_error)?;
        inference::mark_executed(&self.device);
        tracing::info!(
            mel_ms = stages.mel_ms,
            encode_ms = stages.encode_ms,
            decode_ms = stages.decode_ms,
            "Parakeet stages"
        );

        Ok((text.trim().to_string(), None))
    }
}

// Peak amplitude at or below which a recording counts as digital silence.
// Whisper hallucinates text on such input, so no engine gets to see it.
const DIGITAL_SILENCE_PEAK: f32 = 0.001;

pub fn is_digital_silence(samples: &[f32]) -> bool {
    samples
        .iter()
        .all(|sample| sample.abs() <= DIGITAL_SILENCE_PEAK)
}

/// Check if a transcription has no actual speech: empty, or only markers like
/// "[BLANK_AUDIO]" or sound annotations like "[swoosh]", "(clicking)" or "♪",
/// which Whisper writes on room noise.
pub fn is_blank_audio(text: &str) -> bool {
    if text.trim().is_empty() {
        return true;
    }
    let mut depth = 0_usize;
    let mut annotated = false;
    for c in text.chars() {
        match c {
            '[' | '(' => {
                depth += 1;
                annotated = true;
            }
            ']' | ')' => match depth.checked_sub(1) {
                Some(outer) => depth = outer,
                None => return false,
            },
            '♪' | '♫' | '♬' | '♩' => annotated = true,
            _ if depth > 0 => {}
            c if c.is_whitespace() || c.is_ascii_punctuation() => {}
            _ => return false,
        }
    }
    annotated && depth == 0
}

/// The engine currently in memory plus what the timing logs need.
struct LoadedEngine {
    engine: Arc<dyn TranscriptionEngine>,
    model: &'static ModelDefinition,
    load_ms: u128,
    /// Distinct per load, so parts of one recording can tell whether they all
    /// ran on the same engine.
    generation: u64,
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn load_engine(variant: ModelVariant, model_path: &Path) -> Result<LoadedEngine, TranscribeError> {
    if !model_path.exists() {
        return Err(TranscribeError::ModelNotFound);
    }
    let model = model::get_definition(variant);
    let path = model_path.to_string_lossy();
    let preference = crate::settings::load_settings_sync().inference_device;
    let started = Instant::now();
    let engine: Arc<dyn TranscriptionEngine> = match model.engine {
        ModelEngine::Whisper => Arc::new(WhisperEngine::new(&path, variant, preference)?),
        ModelEngine::Parakeet => Arc::new(ParakeetEngine::new(&path, variant, preference)?),
    };
    let loaded = LoadedEngine {
        engine,
        model,
        load_ms: started.elapsed().as_millis(),
        generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
    };
    tracing::info!(
        engine = ?model.engine,
        model = model.filename,
        load_ms = loaded.load_ms as u64,
        whisper_cpp = whisper_cpp::WHISPER_CPP_VERSION,
        "Transcription engine loaded"
    );
    Ok(loaded)
}

// Global engine instance (can be loaded/unloaded/swapped at runtime). The lock
// is only held to read or swap the slot, never while a model loads, so a
// hotkey press during a load is not blocked. `model_loader` serializes loads.
static TRANSCRIBER: Lazy<Mutex<Option<Arc<LoadedEngine>>>> = Lazy::new(|| Mutex::new(None));

/// Load `variant` from `model_path` as the global engine (Whisper or Parakeet,
/// per its registry entry). A no-op when that model is already loaded; a
/// different loaded model is freed first, so two never share memory.
pub fn init_transcriber(variant: ModelVariant, model_path: &Path) -> Result<(), TranscribeError> {
    if loaded_variant() == Some(variant) {
        return Ok(());
    }
    if is_transcriber_loaded() {
        tracing::info!("Replacing loaded transcription engine");
        unload_transcriber();
    }

    tracing::info!("Initializing transcriber");
    let loaded = Arc::new(load_engine(variant, model_path)?);
    let mut guard = TRANSCRIBER
        .lock()
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber lock poisoned".to_string()))?;
    *guard = Some(loaded);
    Ok(())
}

fn get_loaded() -> Option<Arc<LoadedEngine>> {
    match TRANSCRIBER.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }
}

/// The registry model of the loaded engine, if one is loaded.
pub fn loaded_variant() -> Option<ModelVariant> {
    get_loaded().map(|loaded| loaded.model.variant)
}

/// Whether the global transcriber is currently loaded.
pub fn is_transcriber_loaded() -> bool {
    loaded_variant().is_some()
}

/// Unload the global transcriber.
pub fn unload_transcriber() {
    if let Ok(mut guard) = TRANSCRIBER.lock() {
        *guard = None;
    }
    inference::mark_unloaded();
}

/// Transcribe audio using the global engine and log its timing (never the text).
/// Setting `abort` stops it with [`TranscribeError::Aborted`].
pub fn transcribe_audio(
    audio: &[f32],
    languages: &[String],
    dictionary_prompt: Option<&str>,
    abort: &AbortFlag,
) -> Result<String, TranscribeError> {
    transcribe_audio_part(audio, languages, dictionary_prompt, None, abort).map(|part| part.text)
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
    abort: &AbortFlag,
) -> Result<PartTranscript, TranscribeError> {
    let Some(loaded) = get_loaded() else {
        return Err(TranscribeError::ModelNotFound);
    };

    let started = Instant::now();
    let result = loaded
        .engine
        .transcribe(audio, languages, dictionary_prompt, language, abort);
    tracing::info!(
        engine = ?loaded.model.engine,
        model = loaded.model.filename,
        audio_s = format!("{:.2}", audio.len() as f32 / WHISPER_SAMPLE_RATE as f32),
        load_ms = loaded.load_ms as u64,
        transcribe_ms = started.elapsed().as_millis() as u64,
        ok = result.is_ok(),
        aborted = matches!(result, Err(TranscribeError::Aborted)),
        "Transcription timing"
    );
    result.map(|(text, language)| PartTranscript {
        text,
        language,
        engine: loaded.generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_markers_and_sound_annotations_are_blank() {
        for text in [
            "",
            "  ",
            "[BLANK_AUDIO]",
            "(blank audio)",
            "[inaudible]",
            "[Music]",
            "[swoosh]",
            "(clicking)",
            "[waves crashing]",
            "(clicking) [Music].",
            "♪ ♪",
        ] {
            assert!(is_blank_audio(text), "{text}");
        }
    }

    #[test]
    fn text_with_words_is_not_blank() {
        for text in [
            ".",
            "Thank you.",
            "Call me (after five).",
            "[laughs] That works.",
            "(clicking",
            "clicking)",
        ] {
            assert!(!is_blank_audio(text), "{text}");
        }
    }

    #[test]
    fn digital_silence_is_detected_by_peak() {
        assert!(is_digital_silence(&[0.0; 48_000]));
        assert!(is_digital_silence(&[0.001, -0.001, 0.0005]));
        assert!(!is_digital_silence(&[0.0, 0.0, -0.002, 0.0]));
    }

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
            .filter_map(|(language, _)| lang_id(language))
            .max()
            .unwrap_or_default();
        let mut probabilities = vec![0.0; usize::try_from(max_language_id + 1).unwrap_or(0)];

        for (language, probability) in values {
            if let Some(language_id) = lang_id(language) {
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
            transcribe_audio(&[0.25], &[], None, &AbortFlag::default()),
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
            init_transcriber(ModelVariant::Small, Path::new(&missing_path)),
            Err(TranscribeError::ModelNotFound)
        ));
        assert!(!is_transcriber_loaded());
    }
}
