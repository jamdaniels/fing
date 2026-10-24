// Transcription engine trait

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Errors that can occur during transcription.
#[derive(Debug, Clone, Serialize)]
pub enum TranscribeError {
    /// Whisper model file not found at expected path.
    ModelNotFound,
    /// Failed to load or initialize the model.
    ModelLoadFailed(String),
    /// Whisper inference failed during processing.
    InferenceFailed(String),
    /// No audio samples provided.
    EmptyAudio,
    /// Stopped through its [`AbortFlag`] (Escape, or a cancelled recording).
    Aborted,
    /// A later part of a recording found a different engine loaded than the
    /// earlier parts ran on, so the parts can't be combined.
    EngineChanged,
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscribeError::ModelNotFound => write!(f, "Model file not found"),
            TranscribeError::ModelLoadFailed(msg) => write!(f, "Failed to load model: {msg}"),
            TranscribeError::InferenceFailed(msg) => write!(f, "Inference failed: {msg}"),
            TranscribeError::EmptyAudio => write!(f, "Audio buffer is empty"),
            TranscribeError::Aborted => write!(f, "Transcription aborted"),
            TranscribeError::EngineChanged => write!(f, "Transcription engine changed"),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// Cancels the transcriptions of one recording. whisper.cpp polls it after
/// every encoder run and decoder step, so a running call returns
/// [`TranscribeError::Aborted`] within one of those. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct AbortFlag(Arc<AtomicBool>);

impl AbortFlag {
    pub fn abort(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub(crate) fn as_atomic(&self) -> &AtomicBool {
        &self.0
    }

    /// Whether `other` is a clone of this flag.
    pub fn same(&self, other: &AbortFlag) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Trait for speech-to-text engines (Whisper and Parakeet on whisper.cpp).
pub trait TranscriptionEngine: Send + Sync {
    /// Transcribe 16kHz mono f32 audio samples to text. `language` pins the
    /// language an earlier part of the same recording used, so a short part
    /// is not re-detected. Returns the text and the language used (`None` for
    /// engines without a language setting).
    fn transcribe(
        &self,
        audio: &[f32],
        languages: &[String],
        dictionary_prompt: Option<&str>,
        language: Option<&str>,
        abort: &AbortFlag,
    ) -> Result<(String, Option<String>), TranscribeError>;
}
