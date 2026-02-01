use serde::{Deserialize, Serialize};

/// Application metadata for the about/settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub commit: String,
    pub build_date: String,
    pub repository: String,
    pub inference_backend: String,
}

/// Build application info from compile-time environment variables.
pub fn build_app_info() -> AppInfo {
    let inference_backend = if cfg!(target_os = "macos") {
        "Metal"
    } else if cfg!(target_os = "windows") {
        "Vulkan"
    } else {
        "CPU"
    };

    AppInfo {
        name: "Fing".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("GIT_COMMIT").unwrap_or("dev").to_string(),
        build_date: option_env!("BUILD_DATE").unwrap_or("unknown").to_string(),
        repository: "https://github.com/your-username/fing".to_string(),
        inference_backend: inference_backend.to_string(),
    }
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    build_app_info()
}
