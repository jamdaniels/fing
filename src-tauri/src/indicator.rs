use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const HIDE_ANIMATION_MS: u64 = 200;
/// How long a notice stays visible (sent to the webview in the payload).
pub const NOTICE_DURATION_MS: u64 = 3000;
pub const INDICATOR_LABEL: &str = "indicator";
/// Logical px from the screen bottom to the pill's vertical center. Matches the
/// original 70x30 indicator window, whose bottom edge sat 100px above the bottom.
const PILL_CENTER_FROM_BOTTOM: f64 = 115.0;

#[derive(Clone, serde::Serialize)]
pub struct IndicatorStatePayload {
    pub state: String,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeKind {
    Info,
    Error,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NoticePayload<'a> {
    kind: NoticeKind,
    message: &'a str,
    duration_ms: u64,
}

/// Native window bookkeeping. `generation` changes whenever the window is shown
/// or a hide is scheduled, so a stale delayed hide never hides newer content.
#[derive(Clone, Copy)]
struct Visibility {
    generation: u64,
    /// Whether the base state (recording/processing) wants the window shown.
    base_visible: bool,
    notice_until: Option<Instant>,
}

static VISIBILITY: Mutex<Visibility> = Mutex::new(Visibility {
    generation: 0,
    base_visible: false,
    notice_until: None,
});
static CURSOR_PASSTHROUGH_SET: AtomicBool = AtomicBool::new(false);

fn lock_visibility() -> MutexGuard<'static, Visibility> {
    match VISIBILITY.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Indicator visibility mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Position and show the window, invalidating any pending delayed hide.
fn show_window(
    app: &AppHandle,
    update: impl FnOnce(&mut Visibility),
) -> Result<Visibility, String> {
    let window = get_or_create_indicator(app)?;

    // The window is larger than the pill; let clicks pass through it.
    if !CURSOR_PASSTHROUGH_SET.load(Ordering::Relaxed) {
        match window.set_ignore_cursor_events(true) {
            Ok(()) => CURSOR_PASSTHROUGH_SET.store(true, Ordering::Relaxed),
            Err(e) => tracing::warn!("Failed to make indicator click-through: {}", e),
        }
    }
    if let Err(e) = position_indicator(&window) {
        tracing::warn!("Failed to position indicator: {}", e);
    }

    let mut visibility = lock_visibility();
    visibility.generation = visibility.generation.wrapping_add(1);
    update(&mut visibility);
    window.show().map_err(|e| e.to_string())?;
    Ok(*visibility)
}

/// Hide the native window after `delay`, unless it was shown again meanwhile.
fn schedule_window_hide(app: &AppHandle, generation: u64, delay: Duration) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let visibility = lock_visibility();
        if visibility.generation != generation {
            return;
        }
        if let Some(window) = app_handle.get_webview_window(INDICATOR_LABEL) {
            let _ = window.hide();
        }
    });
}

/// Show indicator in recording state
pub fn show_recording(app: &AppHandle) -> Result<(), String> {
    show_window(app, |visibility| {
        visibility.base_visible = true;
        // The webview drops any notice when recording starts.
        visibility.notice_until = None;
    })?;

    app.emit(
        "indicator-state-changed",
        IndicatorStatePayload {
            state: "recording".to_string(),
        },
    )
    .map_err(|e| e.to_string())?;

    tracing::info!("Indicator showing: recording");
    Ok(())
}

/// Show indicator in processing state
pub fn show_processing(app: &AppHandle) -> Result<(), String> {
    app.emit(
        "indicator-state-changed",
        IndicatorStatePayload {
            state: "processing".to_string(),
        },
    )
    .map_err(|e| e.to_string())?;

    tracing::info!("Indicator showing: processing");
    Ok(())
}

/// Hide the indicator window. The webview shrinks out (after any active notice
/// expires); the native window is hidden once that animation has finished.
pub fn hide(app: &AppHandle) -> Result<(), String> {
    app.emit(
        "indicator-state-changed",
        IndicatorStatePayload {
            state: "hidden".to_string(),
        },
    )
    .map_err(|e| e.to_string())?;

    let (generation, delay) = {
        let mut visibility = lock_visibility();
        visibility.generation = visibility.generation.wrapping_add(1);
        visibility.base_visible = false;
        let notice_left = visibility
            .notice_until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        (
            visibility.generation,
            notice_left + Duration::from_millis(HIDE_ANIMATION_MS),
        )
    };
    schedule_window_hide(app, generation, delay);

    tracing::info!("Indicator hiding");
    Ok(())
}

/// Show a short, already localized notice in the indicator pill for
/// [`NOTICE_DURATION_MS`], on top of whatever state it is in.
pub fn notify(app: &AppHandle, kind: NoticeKind, message: &str) -> Result<(), String> {
    let duration = Duration::from_millis(NOTICE_DURATION_MS);
    let notice_until = Instant::now() + duration;
    let visibility = show_window(app, |visibility| {
        visibility.notice_until = Some(notice_until);
    })?;

    app.emit_to(
        INDICATOR_LABEL,
        "indicator-notice",
        NoticePayload {
            kind,
            message,
            duration_ms: NOTICE_DURATION_MS,
        },
    )
    .map_err(|e| e.to_string())?;

    // Nothing else will hide the window if no recording/processing is shown.
    if !visibility.base_visible {
        schedule_window_hide(
            app,
            visibility.generation,
            duration + Duration::from_millis(HIDE_ANIMATION_MS),
        );
    }

    tracing::info!("Indicator notice shown ({:?})", kind);
    Ok(())
}

/// Get existing indicator window or create it
fn get_or_create_indicator(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(INDICATOR_LABEL) {
        return Ok(window);
    }

    // Window should already exist from tauri.conf.json, but if not, error
    Err("Indicator window not found".to_string())
}

/// Position indicator at bottom center of primary screen. The pill is centered
/// inside the (larger, transparent) window, so center the window on the pill spot.
pub fn position_indicator(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("No primary monitor found")?;

    let screen_size = monitor.size();
    let screen_position = monitor.position();
    let scale_factor = monitor.scale_factor();

    // Window dimensions from config (logical; the window lives on this monitor)
    let window_size = window
        .outer_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale_factor);

    // Calculate position (convert physical pixels to logical)
    let screen_width = screen_size.width as f64 / scale_factor;
    let screen_height = screen_size.height as f64 / scale_factor;

    let x = screen_position.x as f64 / scale_factor + (screen_width - window_size.width) / 2.0;
    let y = screen_position.y as f64 / scale_factor + screen_height
        - PILL_CENTER_FROM_BOTTOM
        - window_size.height / 2.0;

    window
        .set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;

    tracing::debug!("Indicator positioned at ({}, {})", x, y);
    Ok(())
}

// Tauri commands for testing/manual control
#[tauri::command]
pub fn indicator_show_recording(app: AppHandle) -> Result<(), String> {
    show_recording(&app)
}

#[tauri::command]
pub fn indicator_show_processing(app: AppHandle) -> Result<(), String> {
    show_processing(&app)
}

#[tauri::command]
pub fn indicator_hide(app: AppHandle) -> Result<(), String> {
    hide(&app)
}
