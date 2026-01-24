// Updates module - check for new releases via GitHub API

use serde::{Deserialize, Serialize};

// Placeholder: update these when publishing to GitHub
const GITHUB_OWNER: &str = "OWNER";
const GITHUB_REPO: &str = "fing";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() >= 3 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;
        Some((major, minor, patch))
    } else {
        None
    }
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some((l_maj, l_min, l_patch)), Some((c_maj, c_min, c_patch))) => {
            (l_maj, l_min, l_patch) > (c_maj, c_min, c_patch)
        }
        _ => false,
    }
}

fn get_platform_asset_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "fing.dmg"
    }
    #[cfg(target_os = "windows")]
    {
        "fing.msi"
    }
    #[cfg(target_os = "linux")]
    {
        "fing.AppImage"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        ""
    }
}

pub async fn check_for_updates() -> Result<UpdateInfo, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        GITHUB_OWNER, GITHUB_REPO
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", format!("fing/{}", CURRENT_VERSION))
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch releases: {}", e))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // No releases yet or repo not found
        return Ok(UpdateInfo {
            available: false,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: CURRENT_VERSION.to_string(),
            download_url: None,
            release_notes: None,
        });
    }

    if !response.status().is_success() {
        return Err(format!("GitHub API error: {}", response.status()));
    }

    let release: GitHubRelease = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse release: {}", e))?;

    let latest_version = release.tag_name.trim_start_matches('v').to_string();
    let available = is_newer(&latest_version, CURRENT_VERSION);

    // Find platform-specific download URL
    let asset_name = get_platform_asset_name();
    let download_url = release
        .assets
        .iter()
        .find(|a| a.name.contains(asset_name))
        .map(|a| a.browser_download_url.clone())
        .or_else(|| Some(release.html_url.clone()));

    Ok(UpdateInfo {
        available,
        current_version: CURRENT_VERSION.to_string(),
        latest_version,
        download_url,
        release_notes: release.body,
    })
}

#[tauri::command]
pub async fn check_for_updates_cmd() -> Result<UpdateInfo, String> {
    check_for_updates().await
}
