// Model download and verification

use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

/// Model variant representing different quality/size tradeoffs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelVariant {
    SmallQ5,
    #[default]
    Small,
    LargeTurboQ5,
}

/// Definition of a model variant with all metadata
pub struct ModelDefinition {
    pub variant: ModelVariant,
    pub filename: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    pub display_name: &'static str,
    pub description: &'static str,
    pub memory_estimate_mb: u32,
}

/// Registry of all available models
pub const MODEL_REGISTRY: &[ModelDefinition] = &[
    ModelDefinition {
        variant: ModelVariant::SmallQ5,
        filename: "ggml-small-q5_1.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        size_bytes: 190_000_000, // ~190 MB
        display_name: "Small Q5",
        description: "Good",
        memory_estimate_mb: 300,
    },
    ModelDefinition {
        variant: ModelVariant::Small,
        filename: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        size_bytes: 488_000_000, // ~488 MB
        display_name: "Small",
        description: "Better",
        memory_estimate_mb: 600,
    },
    ModelDefinition {
        variant: ModelVariant::LargeTurboQ5,
        filename: "ggml-large-v3-turbo-q5_0.bin",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        size_bytes: 574_000_000, // ~574 MB
        display_name: "Large Turbo Q5",
        description: "Best",
        memory_estimate_mb: 750,
    },
];

/// Get the model definition for a variant
pub fn get_definition(variant: ModelVariant) -> &'static ModelDefinition {
    MODEL_REGISTRY
        .iter()
        .find(|m| m.variant == variant)
        .expect("All variants should have definitions")
}

/// Get the file path for a model variant
pub fn model_path_for_variant(variant: ModelVariant) -> PathBuf {
    let def = get_definition(variant);
    crate::paths::models_dir()
        .map(|p| p.join(def.filename))
        .unwrap_or_else(|| PathBuf::from(def.filename))
}

/// Check if a model variant is downloaded and valid
pub fn is_variant_downloaded(variant: ModelVariant) -> bool {
    let path = model_path_for_variant(variant);
    verify_for_variant(&path, variant).is_valid
}

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
    pub format_valid: bool,
    pub hash_valid: bool,
    pub is_valid: bool,
}

/// Runtime info about a model (for frontend display)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub variant: ModelVariant,
    pub filename: String,
    pub display_name: String,
    pub description: String,
    pub size_bytes: u64,
    pub memory_estimate_mb: u32,
    pub is_downloaded: bool,
    pub is_active: bool,
}

/// Get info about all models with current status
pub fn get_all_models(active_variant: ModelVariant) -> Vec<ModelInfo> {
    MODEL_REGISTRY
        .iter()
        .map(|def| ModelInfo {
            variant: def.variant,
            filename: def.filename.to_string(),
            display_name: def.display_name.to_string(),
            description: def.description.to_string(),
            size_bytes: def.size_bytes,
            memory_estimate_mb: def.memory_estimate_mb,
            is_downloaded: is_variant_downloaded(def.variant),
            is_active: def.variant == active_variant,
        })
        .collect()
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
    variant: Option<ModelVariant>,
    bytes_downloaded: u64,
    total_bytes: u64,
    percentage: f32,
    status: DownloadStatus,
}

impl Default for InternalDownloadState {
    fn default() -> Self {
        Self {
            variant: None,
            bytes_downloaded: 0,
            total_bytes: 0,
            percentage: 0.0,
            status: DownloadStatus::NotStarted,
        }
    }
}

/// Serializable progress for frontend (camelCase JSON)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub variant: Option<ModelVariant>,
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
            variant: state.variant,
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
    static ref HASH_CACHE: Mutex<HashMap<PathBuf, HashCacheEntry>> = Mutex::new(HashMap::new());
}

const HASH_CACHE_MAX_ENTRIES: usize = 64;

fn lock_download_state() -> std::sync::MutexGuard<'static, InternalDownloadState> {
    match DOWNLOAD_STATE.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Download state mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

#[derive(Debug, Clone)]
struct HashCacheEntry {
    size: u64,
    modified: Option<SystemTime>,
    sha256: String,
}

/// Verify a model file exists and has valid magic bytes.
/// Uses a general size check (minimum 50MB for any whisper model).
pub fn verify(path: &std::path::Path) -> ModelVerification {
    verify_with_expected_size(path, None, None)
}

/// Verify a model file for a specific variant.
pub fn verify_for_variant(path: &std::path::Path, variant: ModelVariant) -> ModelVerification {
    let def = get_definition(variant);
    verify_with_expected_size(path, Some(def.size_bytes), Some(def.sha256))
}

/// Internal verify function with optional expected size.
fn verify_with_expected_size(
    path: &std::path::Path,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
) -> ModelVerification {
    let exists = path.exists();
    let mut size_valid = false;
    let mut format_valid = false;
    let mut hash_valid = expected_sha256.is_none();

    if exists {
        // Check file size
        if let Ok(metadata) = std::fs::metadata(path) {
            let size = metadata.len();
            if let Some(expected) = expected_size {
                // Tier-specific: 20% tolerance
                let tolerance = expected / 5;
                size_valid =
                    size > expected.saturating_sub(tolerance) && size < expected + tolerance;
                if !size_valid {
                    tracing::warn!(
                        "Model file size invalid: {} bytes (expected ~{} bytes)",
                        size,
                        expected
                    );
                }
            } else {
                // General check: any whisper model should be at least 50MB
                size_valid = size > 50_000_000;
                if !size_valid {
                    tracing::warn!("Model file too small: {} bytes", size);
                }
            }
        }

        // Verify GGML magic bytes - this is the primary validation
        if size_valid && !validate_ggml_magic(path) {
            tracing::warn!("Model file has invalid GGML magic bytes: {:?}", path);
            size_valid = false;
        }

        if size_valid {
            if let Some(expected_hash) = expected_sha256 {
                hash_valid = verify_sha256_with_cache(path, expected_hash);
                if !hash_valid {
                    tracing::warn!("Model file SHA256 mismatch: {:?}", path);
                }
            } else {
                hash_valid = true;
            }
        }

        // Model format is valid if size, magic bytes, and hash checks pass.
        if size_valid {
            format_valid = hash_valid;
        }
    }

    ModelVerification {
        path: path.to_string_lossy().to_string(),
        exists,
        size_valid,
        format_valid,
        hash_valid,
        is_valid: exists && size_valid && format_valid && hash_valid,
    }
}

fn metadata_signature(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // Keep the I/O buffer on the heap to avoid a large stack frame on UI-thread calls.
    let mut buffer = vec![0_u8; 1024 * 1024];

    loop {
        let bytes_read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_sha256_with_cache(path: &Path, expected_sha256: &str) -> bool {
    let normalized_expected = expected_sha256.trim().to_ascii_lowercase();
    if normalized_expected.len() != 64
        || !normalized_expected.chars().all(|c| c.is_ascii_hexdigit())
    {
        tracing::error!("Invalid expected SHA256 for {:?}", path);
        return false;
    }

    let Some((size, modified)) = metadata_signature(path) else {
        return false;
    };

    if let Ok(cache) = HASH_CACHE.lock() {
        if let Some(entry) = cache.get(path) {
            if entry.size == size && entry.modified == modified {
                return entry.sha256 == normalized_expected;
            }
        }
    }

    let computed = match compute_file_sha256(path) {
        Ok(hash) => hash,
        Err(e) => {
            tracing::error!("Failed to compute SHA256 for {:?}: {}", path, e);
            return false;
        }
    };

    if let Ok(mut cache) = HASH_CACHE.lock() {
        if cache.len() >= HASH_CACHE_MAX_ENTRIES && !cache.contains_key(path) {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            HashCacheEntry {
                size,
                modified,
                sha256: computed.clone(),
            },
        );
    }

    computed == normalized_expected
}

fn move_hash_cache_entry(from: &Path, to: &Path) {
    // Downloads verify the `.part` file first; preserve the computed hash cache after rename.
    let Some((size, modified)) = metadata_signature(to) else {
        return;
    };

    if let Ok(mut cache) = HASH_CACHE.lock() {
        let Some(entry) = cache.remove(from) else {
            return;
        };

        if cache.len() >= HASH_CACHE_MAX_ENTRIES && !cache.contains_key(to) {
            cache.clear();
        }

        cache.insert(
            to.to_path_buf(),
            HashCacheEntry {
                size,
                modified,
                sha256: entry.sha256,
            },
        );
    }
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

/// Download the default model variant (for backwards compatibility)
pub async fn download() -> Result<PathBuf, String> {
    download_variant(ModelVariant::default()).await
}

/// Download a specific model variant
pub async fn download_variant(variant: ModelVariant) -> Result<PathBuf, String> {
    let def = get_definition(variant);
    let path = model_path_for_variant(variant);
    let part_path = path.with_extension("part");
    tracing::info!("Starting {} model download to {:?}", def.display_name, path);

    // Create directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            tracing::error!("Failed to create model directory: {}", e);
            e.to_string()
        })?;
    }
    if part_path.exists() {
        let _ = std::fs::remove_file(&part_path);
    }

    // Reset progress
    {
        let mut state = lock_download_state();
        *state = InternalDownloadState {
            variant: Some(variant),
            bytes_downloaded: 0,
            total_bytes: def.size_bytes,
            percentage: 0.0,
            status: DownloadStatus::Downloading,
        };
        tracing::info!("Download state reset to Downloading for {:?}", variant);
    }

    // Download
    let client = Client::new();
    tracing::info!("Fetching model from {}", def.url);

    let response = client.get(def.url).send().await.map_err(|e| {
        let err_msg = format!("Network error: {e}");
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        err_msg
    })?;

    if !response.status().is_success() {
        let err_msg = format!("HTTP error: {}", response.status());
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        return Err(err_msg);
    }

    let total_size = response.content_length().unwrap_or(def.size_bytes);
    tracing::info!("Model size: {} bytes", total_size);

    let mut file = std::fs::File::create(&part_path).map_err(|e| {
        let err_msg = format!("Failed to create file: {e}");
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        err_msg
    })?;

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            let err_msg = format!("Download error: {e}");
            tracing::error!("{}", err_msg);
            let mut state = lock_download_state();
            state.status = DownloadStatus::Failed(err_msg.clone());
            let _ = std::fs::remove_file(&part_path);
            err_msg
        })?;

        file.write_all(&chunk).map_err(|e| {
            let err_msg = format!("Write error: {e}");
            tracing::error!("{}", err_msg);
            let mut state = lock_download_state();
            state.status = DownloadStatus::Failed(err_msg.clone());
            let _ = std::fs::remove_file(&part_path);
            err_msg
        })?;

        downloaded += chunk.len() as u64;

        // Update progress
        let mut state = lock_download_state();
        state.bytes_downloaded = downloaded;
        state.total_bytes = total_size;
        state.percentage = (downloaded as f32 / total_size as f32) * 100.0;
    }

    if let Err(e) = file.sync_all() {
        let err_msg = format!("Failed to sync download to disk: {e}");
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        let _ = std::fs::remove_file(&part_path);
        return Err(err_msg);
    }
    drop(file);

    tracing::info!("Download complete, verifying...");

    // Verify
    {
        let mut state = lock_download_state();
        state.status = DownloadStatus::Verifying;
    }

    let verification = verify_for_variant(&part_path, variant);

    if verification.is_valid {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                let err_msg = format!("Failed to replace existing model file: {e}");
                tracing::error!("{}", err_msg);
                let mut state = lock_download_state();
                state.status = DownloadStatus::Failed(err_msg.clone());
                err_msg
            })?;
        }

        std::fs::rename(&part_path, &path).map_err(|e| {
            let err_msg = format!("Failed to finalize model file: {e}");
            tracing::error!("{}", err_msg);
            let mut state = lock_download_state();
            state.status = DownloadStatus::Failed(err_msg.clone());
            let _ = std::fs::remove_file(&part_path);
            err_msg
        })?;
        move_hash_cache_entry(&part_path, &path);

        let mut state = lock_download_state();
        state.status = DownloadStatus::Complete;
        state.percentage = 100.0;
        tracing::info!("{} model verified successfully", def.display_name);
        Ok(path)
    } else {
        // Delete invalid file
        let _ = std::fs::remove_file(&part_path);

        let err_msg = "Model verification failed".to_string();
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        Err(err_msg)
    }
}

/// Delete a downloaded model
pub fn delete_model(variant: ModelVariant) -> Result<(), String> {
    let path = model_path_for_variant(variant);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| {
            tracing::error!("Failed to delete model: {}", e);
            e.to_string()
        })?;
        if let Ok(mut cache) = HASH_CACHE.lock() {
            cache.remove(&path);
        }
        tracing::info!(
            "Deleted {} model at {:?}",
            get_definition(variant).display_name,
            path
        );
    }
    Ok(())
}

/// Get current download progress.
pub fn get_progress() -> DownloadProgress {
    let state = lock_download_state();
    DownloadProgress::from(&*state)
}
