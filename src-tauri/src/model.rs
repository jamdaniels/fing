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
pub const MODEL_SIZE_BYTES: u64 = 77_691_713;
pub const MODEL_SHA256: &str = "bd577a113a864445d4c299e3b7c6c66bab8b55adfb2e5354dd3f64bfc2d2d36b";

#[derive(Debug, Clone, Serialize)]
pub struct ModelVerification {
    pub path: String,
    pub exists: bool,
    pub size_valid: bool,
    pub hash_valid: bool,
    pub is_valid: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DownloadStatus {
    NotStarted,
    Downloading,
    Verifying,
    Complete,
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub status: DownloadStatus,
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            bytes_downloaded: 0,
            total_bytes: MODEL_SIZE_BYTES,
            percentage: 0.0,
            status: DownloadStatus::NotStarted,
        }
    }
}

// Global download progress
lazy_static::lazy_static! {
    pub static ref DOWNLOAD_PROGRESS: Arc<Mutex<DownloadProgress>> =
        Arc::new(Mutex::new(DownloadProgress::default()));
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

        // Verify SHA256
        if size_valid {
            if let Ok(hash) = compute_sha256(path) {
                hash_valid = hash == MODEL_SHA256;
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

pub async fn download() -> Result<PathBuf, String> {
    let path = default_model_path();

    // Create directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Reset progress
    {
        let mut progress = DOWNLOAD_PROGRESS.lock().unwrap();
        *progress = DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: MODEL_SIZE_BYTES,
            percentage: 0.0,
            status: DownloadStatus::Downloading,
        };
    }

    // Download
    let client = Client::new();
    let response = client
        .get(MODEL_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let total_size = response.content_length().unwrap_or(MODEL_SIZE_BYTES);

    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;

    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;

        downloaded += chunk.len() as u64;

        // Update progress
        let mut progress = DOWNLOAD_PROGRESS.lock().unwrap();
        progress.bytes_downloaded = downloaded;
        progress.total_bytes = total_size;
        progress.percentage = (downloaded as f32 / total_size as f32) * 100.0;
    }

    // Verify
    {
        let mut progress = DOWNLOAD_PROGRESS.lock().unwrap();
        progress.status = DownloadStatus::Verifying;
    }

    let verification = verify(&path);

    if verification.is_valid {
        let mut progress = DOWNLOAD_PROGRESS.lock().unwrap();
        progress.status = DownloadStatus::Complete;
        progress.percentage = 100.0;
        Ok(path)
    } else {
        // Delete invalid file
        let _ = std::fs::remove_file(&path);

        let mut progress = DOWNLOAD_PROGRESS.lock().unwrap();
        progress.status = DownloadStatus::Failed("Model verification failed".to_string());
        Err("Downloaded model failed verification".to_string())
    }
}

pub fn get_progress() -> DownloadProgress {
    DOWNLOAD_PROGRESS.lock().unwrap().clone()
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
