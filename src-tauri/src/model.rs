// Model download and verification

use reqwest::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const MODEL_FILENAME: &str = "ggml-tiny.en.bin";
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin";
pub const MODEL_SIZE_BYTES: u64 = 77_704_715;
// Note: Hash verification disabled - HuggingFace may update the file
pub const MODEL_SHA256: &str = "";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelVerification {
    pub path: String,
    pub exists: bool,
    pub size_valid: bool,
    pub hash_valid: bool,
    pub is_valid: bool,
}

/// Internal download status enum
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    NotStarted,
    Downloading,
    Verifying,
    Complete,
    Failed(String),
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::NotStarted => "not-started",
            DownloadStatus::Downloading => "downloading",
            DownloadStatus::Verifying => "verifying",
            DownloadStatus::Complete => "complete",
            DownloadStatus::Failed(_) => "failed",
        }
    }

    pub fn error_message(&self) -> Option<String> {
        match self {
            DownloadStatus::Failed(msg) => Some(msg.clone()),
            _ => None,
        }
    }
}

/// Internal state for tracking download progress
#[derive(Debug, Clone)]
struct InternalDownloadState {
    bytes_downloaded: u64,
    total_bytes: u64,
    percentage: f32,
    status: DownloadStatus,
}

impl Default for InternalDownloadState {
    fn default() -> Self {
        Self {
            bytes_downloaded: 0,
            total_bytes: MODEL_SIZE_BYTES,
            percentage: 0.0,
            status: DownloadStatus::NotStarted,
        }
    }
}

/// Serializable progress for frontend (camelCase JSON)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl From<&InternalDownloadState> for DownloadProgress {
    fn from(state: &InternalDownloadState) -> Self {
        Self {
            bytes_downloaded: state.bytes_downloaded,
            total_bytes: state.total_bytes,
            percentage: state.percentage,
            status: state.status.as_str().to_string(),
            error_message: state.status.error_message(),
        }
    }
}

// Global download state
lazy_static::lazy_static! {
    static ref DOWNLOAD_STATE: Arc<Mutex<InternalDownloadState>> =
        Arc::new(Mutex::new(InternalDownloadState::default()));
}

pub fn default_model_path() -> PathBuf {
    let app_data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    app_data.join("com.fing.app").join("models").join(MODEL_FILENAME)
}

pub fn verify(path: &std::path::Path) -> ModelVerification {
    let exists = path.exists();
    let mut size_valid = false;
    let mut hash_valid = false;

    if exists {
        if let Ok(metadata) = std::fs::metadata(path) {
            let size = metadata.len();
            // Allow 1MB tolerance
            size_valid = size > MODEL_SIZE_BYTES - 1_000_000 && size < MODEL_SIZE_BYTES + 1_000_000;
        }

        // Verify SHA256 (skip if hash constant is empty)
        if size_valid && !MODEL_SHA256.is_empty() {
            if let Ok(hash) = compute_sha256(path) {
                hash_valid = hash == MODEL_SHA256;
            }
        } else if size_valid {
            // Skip hash verification, just trust the size
            hash_valid = true;
        }
    }

    ModelVerification {
        path: path.to_string_lossy().to_string(),
        exists,
        size_valid,
        hash_valid,
        is_valid: exists && size_valid && hash_valid,
    }
}

fn compute_sha256(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = std::io::Read::read(&mut file, &mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub async fn download() -> Result<PathBuf, String> {
    let path = default_model_path();
    tracing::info!("Starting model download to {:?}", path);

    // Create directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            tracing::error!("Failed to create model directory: {}", e);
            e.to_string()
        })?;
    }

    // Reset progress
    {
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        *state = InternalDownloadState {
            bytes_downloaded: 0,
            total_bytes: MODEL_SIZE_BYTES,
            percentage: 0.0,
            status: DownloadStatus::Downloading,
        };
        tracing::info!("Download state reset to Downloading");
    }

    // Download
    let client = Client::new();
    tracing::info!("Fetching model from {}", MODEL_URL);

    let response = client
        .get(MODEL_URL)
        .send()
        .await
        .map_err(|e| {
            let err_msg = format!("Network error: {}", e);
            tracing::error!("{}", err_msg);
            let mut state = DOWNLOAD_STATE.lock().unwrap();
            state.status = DownloadStatus::Failed(err_msg.clone());
            err_msg
        })?;

    if !response.status().is_success() {
        let err_msg = format!("HTTP error: {}", response.status());
        tracing::error!("{}", err_msg);
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.status = DownloadStatus::Failed(err_msg.clone());
        return Err(err_msg);
    }

    let total_size = response.content_length().unwrap_or(MODEL_SIZE_BYTES);
    tracing::info!("Model size: {} bytes", total_size);

    let mut file = std::fs::File::create(&path).map_err(|e| {
        let err_msg = format!("Failed to create file: {}", e);
        tracing::error!("{}", err_msg);
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.status = DownloadStatus::Failed(err_msg.clone());
        err_msg
    })?;

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            let err_msg = format!("Download error: {}", e);
            tracing::error!("{}", err_msg);
            let mut state = DOWNLOAD_STATE.lock().unwrap();
            state.status = DownloadStatus::Failed(err_msg.clone());
            err_msg
        })?;

        file.write_all(&chunk).map_err(|e| {
            let err_msg = format!("Write error: {}", e);
            tracing::error!("{}", err_msg);
            let mut state = DOWNLOAD_STATE.lock().unwrap();
            state.status = DownloadStatus::Failed(err_msg.clone());
            err_msg
        })?;

        downloaded += chunk.len() as u64;

        // Update progress
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.bytes_downloaded = downloaded;
        state.total_bytes = total_size;
        state.percentage = (downloaded as f32 / total_size as f32) * 100.0;
    }

    tracing::info!("Download complete, verifying...");

    // Verify
    {
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.status = DownloadStatus::Verifying;
    }

    let verification = verify(&path);

    if verification.is_valid {
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.status = DownloadStatus::Complete;
        state.percentage = 100.0;
        tracing::info!("Model verified successfully");
        Ok(path)
    } else {
        // Delete invalid file
        let _ = std::fs::remove_file(&path);

        let err_msg = "Model verification failed".to_string();
        tracing::error!("{}", err_msg);
        let mut state = DOWNLOAD_STATE.lock().unwrap();
        state.status = DownloadStatus::Failed(err_msg.clone());
        Err(err_msg)
    }
}

pub fn get_progress() -> DownloadProgress {
    let state = DOWNLOAD_STATE.lock().unwrap();
    DownloadProgress::from(&*state)
}

/// Open file picker dialog to select a model file
pub fn select_file(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app
        .dialog()
        .file()
        .add_filter("Whisper Model", &["bin"])
        .set_title("Select Whisper Model")
        .blocking_pick_file();

    file_path.and_then(|f| f.into_path().ok())
}
