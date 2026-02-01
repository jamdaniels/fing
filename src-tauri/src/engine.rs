// Transcription engine trait

use serde::Serialize;

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
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscribeError::ModelNotFound => write!(f, "Model file not found"),
            TranscribeError::ModelLoadFailed(msg) => write!(f, "Failed to load model: {msg}"),
            TranscribeError::InferenceFailed(msg) => write!(f, "Inference failed: {msg}"),
            TranscribeError::EmptyAudio => write!(f, "Audio buffer is empty"),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// Trait for speech-to-text engines (currently whisper-rs).
pub trait TranscriptionEngine: Send + Sync {
    /// Transcribe 16kHz mono f32 audio samples to text.
    fn transcribe(&self, audio: &[f32], language: Option<&str>) -> Result<String, TranscribeError>;
}
