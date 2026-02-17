// Whisper transcription wrapper

use crate::engine::{TranscribeError, TranscriptionEngine};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::{Arc, Mutex};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MAX_PROMPT_TOKENS: usize = 256;

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

        let ctx_params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(model_path, ctx_params)
            .map_err(|e| TranscribeError::ModelLoadFailed(e.to_string()))?;

        Ok(Self {
            ctx: Mutex::new(ctx),
        })
    }
}

impl TranscriptionEngine for Transcriber {
    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
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

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        params.set_no_context(true);

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

        let num_segments = state.full_n_segments().unwrap_or(0);
        let mut text = String::new();

        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
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

    tracing::info!("Initializing transcriber from {}", model_path);
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
    language: Option<&str>,
    dictionary_prompt: Option<&str>,
) -> Result<String, TranscribeError> {
    match get_transcriber() {
        Some(t) => t.transcribe(audio, language, dictionary_prompt),
        None => Err(TranscribeError::ModelNotFound),
    }
}
