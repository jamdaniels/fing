// Transcription engine trait

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub enum TranscribeError {
    ModelNotFound,
    ModelInvalid,
    ModelLoadFailed(String),
    InferenceFailed(String),
    EmptyAudio,
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscribeError::ModelNotFound => write!(f, "Model file not found"),
            TranscribeError::ModelInvalid => write!(f, "Model file is invalid"),
            TranscribeError::ModelLoadFailed(msg) => write!(f, "Failed to load model: {}", msg),
            TranscribeError::InferenceFailed(msg) => write!(f, "Inference failed: {}", msg),
            TranscribeError::EmptyAudio => write!(f, "Audio buffer is empty"),
        }
    }
}

impl std::error::Error for TranscribeError {}

pub trait TranscriptionEngine: Send + Sync {
    fn transcribe(&self, audio: &[f32]) -> Result<String, TranscribeError>;
    fn backend_name(&self) -> &str;
}
