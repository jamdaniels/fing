// Whisper transcription wrapper

use crate::engine::{TranscribeError, TranscriptionEngine};
use std::path::Path;
use std::sync::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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
    fn transcribe(&self, audio: &[f32]) -> Result<String, TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let ctx = self.ctx.lock().unwrap();
        let mut state = ctx
            .create_state()
            .map_err(|e| TranscribeError::InferenceFailed(e.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        params.set_no_context(true);

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

// Global transcriber instance (loaded once on startup)
static TRANSCRIBER: once_cell::sync::OnceCell<Transcriber> = once_cell::sync::OnceCell::new();

pub fn init_transcriber(model_path: &str) -> Result<(), TranscribeError> {
    let transcriber = Transcriber::new(model_path)?;
    TRANSCRIBER
        .set(transcriber)
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber already initialized".to_string()))
}

pub fn get_transcriber() -> Option<&'static Transcriber> {
    TRANSCRIBER.get()
}

pub fn transcribe_audio(audio: &[f32]) -> Result<String, TranscribeError> {
    match get_transcriber() {
        Some(t) => t.transcribe(audio),
        None => Err(TranscribeError::ModelNotFound),
    }
}
