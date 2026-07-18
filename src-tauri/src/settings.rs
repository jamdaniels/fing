use crate::model::ModelVariant;
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, RwLock};
use tokio::fs;
use tokio::sync::Mutex;

/// User-selected theme preference.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// User-selected interface language.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    #[default]
    En,
    De,
}

impl UiLanguage {
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::De => "de",
        }
    }
}

/// History retention mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub enum HistoryMode {
    #[serde(rename = "off")]
    Off,
    #[default]
    #[serde(rename = "30d")]
    ThirtyDays,
}

/// Cached settings to reduce disk I/O
static SETTINGS_CACHE: RwLock<Option<Settings>> = RwLock::new(None);
static SETTINGS_UPDATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// Windows reads of settings.json can transiently fail (AV/indexer holding the
// file, sharing violation). Retry before treating the file as unreadable.
const ASYNC_RETRY_DELAYS_MS: [u64; 4] = [100, 200, 400, 600];

/// Result of attempting to load settings from disk.
#[derive(Debug, Clone)]
pub enum SettingsLoadOutcome {
    /// settings.json existed and parsed (possibly recovered from settings.json.bak).
    Loaded(Settings),
    /// settings.json genuinely absent: first-run defaults.
    FirstRun(Settings),
    /// Read failed after retries, or JSON unparseable with no usable backup.
    /// Never cached and never persisted, so later loads retry the disk.
    Failed(String),
}

fn settings_sibling(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    std::path::PathBuf::from(os)
}

fn settings_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    settings_sibling(path, ".bak")
}

fn settings_tmp_path(path: &std::path::Path) -> std::path::PathBuf {
    settings_sibling(path, ".tmp")
}

fn recover_from_backup(backup_path: &std::path::Path, reason: &str) -> Option<Settings> {
    let contents = std::fs::read_to_string(backup_path).ok()?;
    let settings = parse_settings_json(&contents).ok()?;
    tracing::warn!("Recovered settings from backup ({})", reason);
    Some(settings)
}

fn interpret_settings_contents(
    contents: &str,
    backup_path: &std::path::Path,
) -> SettingsLoadOutcome {
    match parse_settings_json(contents) {
        Ok(settings) => SettingsLoadOutcome::Loaded(settings),
        Err(parse_error) => match recover_from_backup(backup_path, &parse_error) {
            Some(settings) => SettingsLoadOutcome::Loaded(settings),
            None => SettingsLoadOutcome::Failed(format!("settings JSON unparseable: {parse_error}")),
        },
    }
}

// REGRESSION GUARD: only genuinely loaded (or first-run-created) settings may
// enter the cache. Caching defaults produced by a failed read (uninitialized
// paths, locked or corrupt file) poisons every later load_settings() call for
// the whole process — this cache poisoning was part of the Windows bug where
// onboarding re-appeared after every restart. `Failed` must never touch the
// cache so the next read retries the disk.
fn cache_outcome(outcome: &SettingsLoadOutcome) {
    if let SettingsLoadOutcome::Loaded(settings) | SettingsLoadOutcome::FirstRun(settings) = outcome
    {
        if let Ok(mut cache) = SETTINGS_CACHE.write() {
            *cache = Some(settings.clone());
        }
    }
}

/// User-configurable application settings (persisted to JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub hotkey: String,
    pub model_path: String,
    pub selected_microphone_id: Option<String>,
    pub auto_start: bool,
    pub sound_enabled: bool,
    pub paste_enabled: bool,
    #[serde(default)]
    pub history_mode: HistoryMode,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    #[serde(default)]
    pub active_model_variant: ModelVariant,
    pub theme: Theme,
    #[serde(default)]
    pub lazy_model_loading: bool,
    #[serde(default)]
    pub dictionary_terms: Vec<String>,
    #[serde(default)]
    pub ui_language: UiLanguage,
}

fn default_languages() -> Vec<String> {
    vec!["en".to_string()]
}

fn merge_settings_update(latest: Settings, incoming: Settings) -> Settings {
    let incoming_completed = incoming.onboarding_completed;
    let mut merged = sanitize_settings(incoming);
    merged.onboarding_completed = latest.onboarding_completed || incoming_completed;
    merged
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "F9".to_string(),
            model_path: String::new(),
            selected_microphone_id: None,
            auto_start: false,
            sound_enabled: true,
            paste_enabled: true,
            history_mode: HistoryMode::default(),
            onboarding_completed: false,
            languages: default_languages(),
            active_model_variant: ModelVariant::default(),
            theme: Theme::default(),
            lazy_model_loading: false,
            dictionary_terms: Vec::new(),
            ui_language: UiLanguage::default(),
        }
    }
}

fn resolve_ui_language<I, S>(locales: I) -> UiLanguage
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for locale in locales {
        let normalized = locale.as_ref().trim().to_ascii_lowercase();
        let base = normalized
            .split(['-', '_'])
            .next()
            .unwrap_or(normalized.as_str());
        match base {
            "de" => return UiLanguage::De,
            "en" => return UiLanguage::En,
            _ => {}
        }
    }

    UiLanguage::En
}

fn first_run_settings() -> Settings {
    first_run_settings_for_locales(sys_locale::get_locales())
}

fn first_run_settings_for_locales<I, S>(locales: I) -> Settings
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    Settings {
        ui_language: resolve_ui_language(locales),
        ..Settings::default()
    }
}

/// Load settings from cache or disk (async).
pub async fn load_settings() -> Settings {
    // Check cache first
    if let Ok(cache) = SETTINGS_CACHE.read() {
        if let Some(ref settings) = *cache {
            return settings.clone();
        }
    }

    match load_settings_outcome_uncached().await {
        SettingsLoadOutcome::Loaded(settings) | SettingsLoadOutcome::FirstRun(settings) => settings,
        SettingsLoadOutcome::Failed(error) => {
            tracing::error!("Settings unavailable, using in-memory defaults: {}", error);
            Settings::default()
        }
    }
}

/// Load settings from disk, bypassing the cache. Retries transient read errors
/// and reports failure explicitly instead of silently returning defaults.
pub async fn load_settings_outcome_uncached() -> SettingsLoadOutcome {
    // REGRESSION GUARD: the frontend's first IPC calls can arrive before
    // setup() has initialized paths (Windows webview wins that race in
    // installed builds). Wait instead of answering from an uninitialized
    // state, which used to return defaults and re-show onboarding.
    crate::paths::wait_until_initialized().await;
    let Some(path) = crate::paths::settings_path() else {
        tracing::warn!("Settings path unavailable after initialization");
        return SettingsLoadOutcome::Failed("app paths not initialized".to_string());
    };
    let backup_path = settings_backup_path(&path);

    let mut attempt = 0usize;
    let outcome = loop {
        match fs::read_to_string(&path).await {
            Ok(contents) => break interpret_settings_contents(&contents, &backup_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("Settings file not found, using first-run defaults");
                let settings = first_run_settings();
                if let Err(save_error) = save_settings_unlocked(&settings).await {
                    tracing::warn!("Failed to persist first-run settings: {}", save_error);
                }
                break SettingsLoadOutcome::FirstRun(settings);
            }
            Err(error) if attempt < ASYNC_RETRY_DELAYS_MS.len() => {
                tracing::warn!(
                    "Settings read failed (attempt {}), retrying: {}",
                    attempt + 1,
                    error
                );
                tokio::time::sleep(std::time::Duration::from_millis(
                    ASYNC_RETRY_DELAYS_MS[attempt],
                ))
                .await;
                attempt += 1;
            }
            Err(error) => {
                break match recover_from_backup(&backup_path, &error.to_string()) {
                    Some(settings) => SettingsLoadOutcome::Loaded(settings),
                    None => SettingsLoadOutcome::Failed(format!(
                        "settings file unreadable after retries: {error}"
                    )),
                };
            }
        }
    };

    cache_outcome(&outcome);
    outcome
}

/// Sync version of load_settings for use in menu building. The cache is warm
/// once startup finishes (bootstrap loads settings), so the disk path here is
/// cold; it makes a single attempt and never retries.
pub fn load_settings_sync() -> Settings {
    // Check cache first
    if let Ok(cache) = SETTINGS_CACHE.read() {
        if let Some(ref settings) = *cache {
            return settings.clone();
        }
    }

    // Load from disk
    let Some(path) = crate::paths::settings_path() else {
        tracing::warn!("Settings path requested before app paths were initialized");
        return Settings::default();
    };
    let backup_path = settings_backup_path(&path);

    let outcome = match std::fs::read_to_string(&path) {
        Ok(contents) => interpret_settings_contents(&contents, &backup_path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("Settings file not found, using first-run defaults");
            SettingsLoadOutcome::FirstRun(first_run_settings())
        }
        Err(error) => match recover_from_backup(&backup_path, &error.to_string()) {
            Some(settings) => SettingsLoadOutcome::Loaded(settings),
            None => SettingsLoadOutcome::Failed(format!("settings file unreadable: {error}")),
        },
    };

    cache_outcome(&outcome);
    match outcome {
        SettingsLoadOutcome::Loaded(settings) | SettingsLoadOutcome::FirstRun(settings) => settings,
        SettingsLoadOutcome::Failed(error) => {
            tracing::error!("Settings unavailable, using in-memory defaults: {}", error);
            Settings::default()
        }
    }
}

pub async fn save_settings(settings: &Settings) -> Result<(), String> {
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    save_settings_unlocked(settings).await
}

async fn save_settings_unlocked(settings: &Settings) -> Result<(), String> {
    let sanitized = sanitize_settings(settings.clone());
    let path =
        crate::paths::settings_path().ok_or_else(|| "App paths not initialized".to_string())?;

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create settings directory: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&sanitized)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;

    // REGRESSION GUARD (do not simplify back to a plain `fs::write`): settings
    // are written atomically via tmp-file + rename so a concurrent reader or a
    // crash mid-write can never observe a truncated/partial settings.json. A
    // non-atomic write here caused Windows users to lose onboarding state.
    // The pre-rename copy keeps settings.json.bak as a last-known-good file
    // that the load path uses to recover from a corrupted settings.json.
    let tmp_path = settings_tmp_path(&path);
    let backup_path = settings_backup_path(&path);

    fs::write(&tmp_path, json)
        .await
        .map_err(|e| format!("Failed to write settings: {e}"))?;

    match fs::copy(&path, &backup_path).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("Failed to back up settings before save: {}", error),
    }

    if let Err(error) = fs::rename(&tmp_path, &path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(format!("Failed to write settings: {error}"));
    }

    // Update cache with new settings
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(sanitized);
    }

    Ok(())
}

// REGRESSION GUARD: read-modify-write flows must abort when the settings file
// is unreadable. Falling back to `Settings::default()` here and then saving
// would OVERWRITE the user's real settings.json with defaults (data loss).
async fn latest_settings_for_update() -> Result<Settings, String> {
    match load_settings_outcome_uncached().await {
        SettingsLoadOutcome::Loaded(settings) | SettingsLoadOutcome::FirstRun(settings) => {
            Ok(settings)
        }
        SettingsLoadOutcome::Failed(error) => Err(format!(
            "Settings file is currently unreadable; not saving to avoid data loss: {error}"
        )),
    }
}

pub async fn update_settings_atomic<F>(mutator: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    let mut settings = latest_settings_for_update().await?;
    mutator(&mut settings);
    let sanitized = sanitize_settings(settings);
    save_settings_unlocked(&sanitized).await?;
    Ok(sanitized)
}

/// Invalidate the settings cache (call after external changes)
#[allow(dead_code)]
pub fn invalidate_settings_cache() {
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = None;
    }
}

#[tauri::command]
pub async fn get_settings() -> Result<Settings, String> {
    Ok(load_settings().await)
}

pub async fn update_settings(settings: Settings) -> Result<Settings, String> {
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    let latest = latest_settings_for_update().await?;
    let merged = merge_settings_update(latest, settings);
    save_settings_unlocked(&merged).await?;
    Ok(merged)
}

fn sanitize_settings(mut settings: Settings) -> Settings {
    settings
        .languages
        .retain(|language| !language.trim().eq_ignore_ascii_case("yue"));
    if settings.languages.is_empty() {
        settings.languages = default_languages();
    }
    settings.dictionary_terms = crate::dictionary::sanitize_terms(&settings.dictionary_terms);
    settings
}

// REGRESSION GUARD: this must return Err on unparseable JSON, never a silent
// `Settings::default()`. A silent default flips onboardingCompleted to false
// and resets uiLanguage, which re-showed onboarding on Windows. Callers decide
// how to recover (backup file, explicit Failed outcome).
fn parse_settings_json(contents: &str) -> Result<Settings, String> {
    // Windows tools (e.g. PowerShell 5 Set-Content) may prepend a UTF-8 BOM,
    // which serde_json rejects. Strip it before parsing.
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let mut value: serde_json::Value =
        serde_json::from_str(contents).map_err(|error| error.to_string())?;

    let raw_onboarding_completed = value
        .get("onboardingCompleted")
        .and_then(serde_json::Value::as_bool);
    let raw_dictionary_terms = value
        .get("dictionaryTerms")
        .and_then(serde_json::Value::as_array)
        .map(|terms| {
            terms
                .iter()
                .filter_map(|term| term.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });

    // Existing installations predate uiLanguage. Keep them in English, and
    // sanitize unknown values without discarding the rest of their settings.
    if let Some(object) = value.as_object_mut() {
        let ui_language = object.get("uiLanguage").and_then(serde_json::Value::as_str);
        if !matches!(ui_language, Some("en" | "de")) {
            object.insert(
                "uiLanguage".to_string(),
                serde_json::Value::String("en".to_string()),
            );
        }
    }

    let mut settings = match serde_json::from_value::<Settings>(value) {
        Ok(settings) => sanitize_settings(settings),
        Err(error) => {
            tracing::warn!(
                "Failed to deserialize settings JSON, using defaults: {}",
                error
            );
            Settings::default()
        }
    };

    if raw_onboarding_completed == Some(true) && !settings.onboarding_completed {
        tracing::warn!("Recovering onboardingCompleted=true from raw settings JSON");
        settings.onboarding_completed = true;
    }

    if settings.dictionary_terms.is_empty() {
        if let Some(raw_terms) = raw_dictionary_terms {
            let sanitized_terms = crate::dictionary::sanitize_terms(&raw_terms);
            if !sanitized_terms.is_empty() {
                tracing::warn!(
                    "Recovering {} dictionary terms from raw settings JSON",
                    sanitized_terms.len()
                );
                settings.dictionary_terms = sanitized_terms;
            }
        }
    }

    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::runtime::Builder;

    // tokio Mutex so tests can hold the guard across await points without
    // tripping clippy::await_holding_lock.
    static SETTINGS_TEST_MUTEX: Mutex<()> = Mutex::const_new(());

    fn init_test_paths() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("fing-settings-tests");
        crate::paths::init_test_app_data_dir(path.clone());
        path
    }

    async fn reset_test_settings() {
        let app_data_dir = init_test_paths();
        let settings_path = crate::paths::settings_path().expect("settings path should exist");

        invalidate_settings_cache();
        let _ = fs::remove_file(&settings_path).await;
        let _ = fs::remove_file(settings_backup_path(&settings_path)).await;
        let _ = fs::remove_file(settings_tmp_path(&settings_path)).await;
        fs::create_dir_all(&app_data_dir)
            .await
            .expect("test app data dir should be created");
    }

    async fn load_from_disk_for_test() -> Settings {
        match load_settings_outcome_uncached().await {
            SettingsLoadOutcome::Loaded(settings) | SettingsLoadOutcome::FirstRun(settings) => {
                settings
            }
            SettingsLoadOutcome::Failed(error) => panic!("settings should load: {error}"),
        }
    }

    fn run_async_test<F>(test: F)
    where
        F: std::future::Future<Output = ()>,
    {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(test);
    }

    fn unique_suffix() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string())
    }

    #[test]
    fn resolves_supported_system_locales_in_preference_order() {
        assert_eq!(resolve_ui_language(["de-DE"]), UiLanguage::De);
        assert_eq!(resolve_ui_language(["de_CH"]), UiLanguage::De);
        assert_eq!(resolve_ui_language(["en-GB"]), UiLanguage::En);
        assert_eq!(
            resolve_ui_language(["fr-CH", "de-CH", "en-US"]),
            UiLanguage::De
        );
        assert_eq!(resolve_ui_language(["fr-FR"]), UiLanguage::En);
        assert_eq!(resolve_ui_language(Vec::<String>::new()), UiLanguage::En);
    }

    #[test]
    fn first_run_settings_use_detected_language() {
        assert_eq!(
            first_run_settings_for_locales(["de-DE"]).ui_language,
            UiLanguage::De
        );
        assert_eq!(
            first_run_settings_for_locales(["fr-FR"]).ui_language,
            UiLanguage::En
        );
    }

    #[test]
    fn existing_settings_without_ui_language_stay_english() {
        let raw = serde_json::to_string(&Settings {
            ui_language: UiLanguage::De,
            ..Settings::default()
        })
        .expect("settings should serialize");
        let value: serde_json::Value =
            serde_json::from_str(&raw).expect("settings JSON should parse");
        let mut object = value
            .as_object()
            .expect("settings should be an object")
            .clone();
        object.remove("uiLanguage");

        let parsed = parse_settings_json(
            &serde_json::to_string(&object).expect("legacy settings should serialize"),
        )
        .expect("legacy settings should parse");

        assert_eq!(parsed.ui_language, UiLanguage::En);
    }

    #[test]
    fn invalid_ui_language_does_not_discard_other_settings() {
        let mut value = serde_json::to_value(Settings {
            theme: Theme::Dark,
            ..Settings::default()
        })
        .expect("settings should serialize");
        value["uiLanguage"] = serde_json::Value::String("xx".to_string());

        let parsed =
            parse_settings_json(&serde_json::to_string(&value).expect("settings should serialize"))
                .expect("settings should parse");

        assert_eq!(parsed.ui_language, UiLanguage::En);
        assert_eq!(parsed.theme, Theme::Dark);
    }

    #[test]
    fn explicit_ui_language_round_trips() {
        let settings = Settings {
            ui_language: UiLanguage::De,
            ..Settings::default()
        };
        let raw = serde_json::to_string(&settings).expect("settings should serialize");
        assert_eq!(
            parse_settings_json(&raw)
                .expect("settings should parse")
                .ui_language,
            UiLanguage::De
        );
    }

    #[test]
    fn sanitizing_languages_removes_cantonese_and_preserves_order() {
        let settings = sanitize_settings(Settings {
            languages: vec!["de".to_string(), "yue".to_string()],
            ..Settings::default()
        });

        assert_eq!(settings.languages, vec!["de"]);
    }

    #[test]
    fn sanitizing_only_cantonese_falls_back_to_english() {
        let settings = sanitize_settings(Settings {
            languages: vec!["yue".to_string()],
            ..Settings::default()
        });

        assert_eq!(settings.languages, vec!["en"]);
    }

    #[test]
    fn legacy_settings_with_cantonese_are_sanitized_when_loaded() {
        let raw = serde_json::to_string(&Settings {
            languages: vec!["de".to_string(), "yue".to_string()],
            ..Settings::default()
        })
        .expect("legacy settings should serialize");

        assert_eq!(
            parse_settings_json(&raw)
                .expect("legacy settings should parse")
                .languages,
            vec!["de"]
        );
    }

    #[test]
    fn update_settings_preserves_onboarding_completed_when_incoming_snapshot_is_stale() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let completed = Settings {
                onboarding_completed: true,
                ..Settings::default()
            };
            save_settings(&completed)
                .await
                .expect("completed settings should save");

            let stale = Settings {
                onboarding_completed: false,
                selected_microphone_id: Some(format!("mic-{}", unique_suffix())),
                ..Settings::default()
            };

            let updated = update_settings(stale)
                .await
                .expect("stale settings update should succeed");

            assert!(updated.onboarding_completed);
            assert!(updated.selected_microphone_id.is_some());

            let reloaded = load_from_disk_for_test().await;
            assert!(reloaded.onboarding_completed);
            assert_eq!(
                reloaded.selected_microphone_id,
                updated.selected_microphone_id
            );
        });
    }

    #[test]
    fn update_settings_keeps_onboarding_completed_sticky() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let persisted = Settings {
                onboarding_completed: true,
                ..Settings::default()
            };
            save_settings(&persisted)
                .await
                .expect("completed settings should save");

            let incoming = Settings {
                onboarding_completed: false,
                theme: Theme::Dark,
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(updated.onboarding_completed);
            assert_eq!(updated.theme, Theme::Dark);
        });
    }

    #[test]
    fn update_settings_keeps_incoming_completed_state_when_disk_read_is_incomplete() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            save_settings(&Settings::default())
                .await
                .expect("default settings should save");

            let incoming = Settings {
                onboarding_completed: true,
                dictionary_terms: vec!["Tauri".to_string()],
                theme: Theme::Light,
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(updated.onboarding_completed);
            assert_eq!(updated.theme, Theme::Light);
            assert_eq!(updated.dictionary_terms, vec!["Tauri".to_string()]);
        });
    }

    #[test]
    fn update_settings_still_allows_incomplete_onboarding_settings_updates() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            save_settings(&Settings::default())
                .await
                .expect("default settings should save");

            let incoming = Settings {
                selected_microphone_id: Some(format!("mic-{}", unique_suffix())),
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(!updated.onboarding_completed);
            assert!(updated.selected_microphone_id.is_some());

            let reloaded = load_from_disk_for_test().await;
            assert!(!reloaded.onboarding_completed);
            assert_eq!(
                reloaded.selected_microphone_id,
                updated.selected_microphone_id
            );
        });
    }

    // The tests below guard the Windows fix for onboarding re-showing after
    // restart (silent default fallback on failed settings reads). Do not
    // remove or weaken them; each one pins a piece of the fix.

    #[test]
    fn parse_settings_json_strips_utf8_bom() {
        let raw = serde_json::to_string(&Settings {
            ui_language: UiLanguage::De,
            onboarding_completed: true,
            ..Settings::default()
        })
        .expect("settings should serialize");
        let with_bom = format!("\u{feff}{raw}");

        let parsed = parse_settings_json(&with_bom).expect("BOM-prefixed settings should parse");

        assert!(parsed.onboarding_completed);
        assert_eq!(parsed.ui_language, UiLanguage::De);
    }

    #[test]
    fn corrupt_settings_fall_back_to_backup() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            let backup = serde_json::to_string(&Settings {
                ui_language: UiLanguage::De,
                onboarding_completed: true,
                ..Settings::default()
            })
            .expect("backup settings should serialize");
            fs::write(&settings_path, "{ not json")
                .await
                .expect("corrupt settings should write");
            fs::write(settings_backup_path(&settings_path), backup)
                .await
                .expect("backup settings should write");

            match load_settings_outcome_uncached().await {
                SettingsLoadOutcome::Loaded(settings) => {
                    assert!(settings.onboarding_completed);
                    assert_eq!(settings.ui_language, UiLanguage::De);
                }
                other => panic!("expected Loaded from backup, got {other:?}"),
            }
        });
    }

    #[test]
    fn failed_load_is_not_cached() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            fs::write(&settings_path, "{ not json")
                .await
                .expect("corrupt settings should write");

            let defaulted = load_settings().await;
            assert!(!defaulted.onboarding_completed);

            // A failed read must not poison the cache: once the file is
            // readable again, the very next load must return the real values.
            let valid = serde_json::to_string(&Settings {
                onboarding_completed: true,
                ui_language: UiLanguage::De,
                ..Settings::default()
            })
            .expect("valid settings should serialize");
            fs::write(&settings_path, valid)
                .await
                .expect("valid settings should write");

            let reloaded = load_settings().await;
            assert!(reloaded.onboarding_completed);
            assert_eq!(reloaded.ui_language, UiLanguage::De);
        });
    }

    #[test]
    fn update_settings_refuses_to_overwrite_unreadable_file() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            let corrupt = "{ not json";
            fs::write(&settings_path, corrupt)
                .await
                .expect("corrupt settings should write");

            let update_result = update_settings(Settings::default()).await;
            assert!(update_result.is_err(), "update must not clobber");

            let atomic_result = update_settings_atomic(|settings| {
                settings.theme = Theme::Dark;
            })
            .await;
            assert!(atomic_result.is_err(), "atomic update must not clobber");

            let raw = fs::read_to_string(&settings_path)
                .await
                .expect("settings file should still exist");
            assert_eq!(raw, corrupt, "settings file must be untouched");
        });
    }

    #[test]
    fn save_settings_writes_atomically_with_backup() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            let first = Settings {
                onboarding_completed: true,
                theme: Theme::Light,
                ..Settings::default()
            };
            let second = Settings {
                onboarding_completed: true,
                theme: Theme::Dark,
                ..Settings::default()
            };
            save_settings(&first).await.expect("first save should work");
            save_settings(&second)
                .await
                .expect("second save should work");

            let current = fs::read_to_string(&settings_path)
                .await
                .expect("settings should read");
            assert_eq!(
                parse_settings_json(&current)
                    .expect("saved settings should parse")
                    .theme,
                Theme::Dark
            );

            let backup = fs::read_to_string(settings_backup_path(&settings_path))
                .await
                .expect("backup should exist after second save");
            assert_eq!(
                parse_settings_json(&backup)
                    .expect("backup settings should parse")
                    .theme,
                Theme::Light
            );

            assert!(
                fs::metadata(settings_tmp_path(&settings_path)).await.is_err(),
                "no tmp file should remain after save"
            );
        });
    }

    #[test]
    fn missing_file_yields_first_run_defaults_and_persists() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            match load_settings_outcome_uncached().await {
                SettingsLoadOutcome::FirstRun(settings) => {
                    assert!(!settings.onboarding_completed);
                }
                other => panic!("expected FirstRun, got {other:?}"),
            }
            assert!(
                fs::metadata(&settings_path).await.is_ok(),
                "first-run defaults should be persisted"
            );
        });
    }

    // Directly reproduces the original Windows bug: another process holds
    // settings.json without sharing, the read fails transiently, and the
    // retry loop must recover instead of falling back to defaults.
    #[cfg(windows)]
    #[test]
    fn transient_sharing_violation_is_retried() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX.lock().await;
            reset_test_settings().await;

            let settings_path = crate::paths::settings_path().expect("settings path should exist");
            let valid = serde_json::to_string(&Settings {
                onboarding_completed: true,
                ui_language: UiLanguage::De,
                ..Settings::default()
            })
            .expect("valid settings should serialize");
            fs::write(&settings_path, valid)
                .await
                .expect("valid settings should write");

            use std::os::windows::fs::OpenOptionsExt;
            let exclusive = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&settings_path)
                .expect("exclusive handle should open");

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                drop(exclusive);
            });

            match load_settings_outcome_uncached().await {
                SettingsLoadOutcome::Loaded(settings) => {
                    assert!(settings.onboarding_completed);
                    assert_eq!(settings.ui_language, UiLanguage::De);
                }
                other => panic!("expected Loaded after lock release, got {other:?}"),
            }
        });
    }
}
