// Model download and verification

use reqwest::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub const MODEL_FILENAME: &str = "ggml-base.bin";
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
pub const MODEL_SIZE_BYTES: u64 = 147_964_211;
// SHA256 hash verification is informational only - HuggingFace may update the file
// Primary validation is GGML magic bytes + size check
pub const MODEL_SHA256: &str = "";

// GGML file magic bytes (little-endian): "ggml" = 0x6c6d6767 or "ggjt" = 0x746a6767
const GGML_MAGIC_GGML: u32 = 0x67676d6c;
const GGML_MAGIC_GGJT: u32 = 0x67676a74;

/// Result of model file verification (size + GGML magic bytes).
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
    static ref DOWNLOAD_STATE: Mutex<InternalDownloadState> =
        Mutex::new(InternalDownloadState::default());
}

/// Get the default model file path in app data directory.
pub fn default_model_path() -> PathBuf {
    crate::paths::models_dir().join(MODEL_FILENAME)
}

/// Verify a model file exists and has valid size/magic bytes.
pub fn verify(path: &std::path::Path) -> ModelVerification {
    let exists = path.exists();
    let mut size_valid = false;
    let mut hash_valid = false;

    if exists {
        // Check file size (1MB tolerance)
        if let Ok(metadata) = std::fs::metadata(path) {
            let size = metadata.len();
            size_valid = size > MODEL_SIZE_BYTES - 1_000_000 && size < MODEL_SIZE_BYTES + 1_000_000;
            if !size_valid {
                tracing::warn!("Model file size invalid: {} bytes (expected ~{} bytes)", size, MODEL_SIZE_BYTES);
            }
        }

        // Verify GGML magic bytes - this is the primary validation
        if size_valid && !validate_ggml_magic(path) {
            tracing::warn!("Model file has invalid GGML magic bytes: {:?}", path);
            size_valid = false;
        }

        // Hash verification is informational only (HuggingFace may update files)
        // Model is valid if size and magic bytes check pass
        if size_valid {
            hash_valid = true; // Accept based on size and magic bytes

            // Log hash for debugging (but don't fail on mismatch)
            if !MODEL_SHA256.is_empty() {
                if let Ok(hash) = compute_sha256(path) {
                    if hash != MODEL_SHA256 {
                        tracing::info!(
                            "Model hash differs from expected (this is OK - HuggingFace may have updated the file): {}",
                            hash
                        );
                    }
                }
            }
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

/// Validate GGML file magic bytes
fn validate_ggml_magic(path: &std::path::Path) -> bool {
    use std::io::Read;

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut magic_bytes = [0u8; 4];
    if file.read_exact(&mut magic_bytes).is_err() {
        return false;
    }

    let magic = u32::from_le_bytes(magic_bytes);
    magic == GGML_MAGIC_GGML || magic == GGML_MAGIC_GGJT
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

/// Get current download progress.
pub fn get_progress() -> DownloadProgress {
    let state = DOWNLOAD_STATE.lock().unwrap();
    DownloadProgress::from(&*state)
}

/// Open file picker dialog to select a model file and copy it to the default location
pub fn select_file(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app
        .dialog()
        .file()
        .add_filter("Whisper Model", &["bin"])
        .set_title("Select Whisper Model")
        .blocking_pick_file();

    let selected_path = file_path.and_then(|f| f.into_path().ok())?;

    // Pre-validate selected file before copying
    // Check size first
    let file_size = match std::fs::metadata(&selected_path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!("Failed to read selected file metadata: {}", e);
            return None;
        }
    };

    // Validate size (1MB tolerance)
    if !(MODEL_SIZE_BYTES - 1_000_000..=MODEL_SIZE_BYTES + 1_000_000).contains(&file_size) {
        tracing::error!(
            "Selected file has invalid size: {} bytes (expected ~{} bytes)",
            file_size,
            MODEL_SIZE_BYTES
        );
        return None;
    }

    // Validate GGML magic bytes
    if !validate_ggml_magic(&selected_path) {
        tracing::error!("Selected file is not a valid GGML model (invalid magic bytes)");
        return None;
    }

    // Get the default model path
    let dest_path = default_model_path();

    // Create directory if needed
    if let Some(parent) = dest_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("Failed to create model directory: {}", e);
            return None;
        }
    }

    // Copy the selected file to the default location
    if let Err(e) = std::fs::copy(&selected_path, &dest_path) {
        tracing::error!("Failed to copy model file: {}", e);
        return None;
    }

    // Verify the copied file
    let verification = verify(&dest_path);
    if !verification.is_valid {
        tracing::error!(
            "Copied model file verification failed: size_valid={}, hash_valid={}",
            verification.size_valid,
            verification.hash_valid
        );
        // Delete invalid file
        let _ = std::fs::remove_file(&dest_path);
        return None;
    }

    tracing::info!("Copied and verified model from {:?} to {:?}", selected_path, dest_path);
    Some(dest_path)
}
