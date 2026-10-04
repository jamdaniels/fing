// Live voice levels for the recording indicator.

use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::audio::{AudioTap, LevelAnalyzer, LEVEL_BAND_COUNT};
use crate::indicator::INDICATOR_LABEL;

/// ~30 Hz update rate.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Clone, serde::Serialize)]
struct LevelsPayload {
    levels: [f32; LEVEL_BAND_COUNT],
}

/// Emit `indicator-levels` until the recording behind `tap` ends; the thread
/// exits within one frame of the capture stopping or being discarded.
pub fn start(app: &AppHandle, tap: AudioTap) {
    let app_handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("level-meter".to_string())
        .spawn(move || run(&app_handle, &tap));
    if let Err(e) = spawned {
        tracing::warn!("Failed to start level meter: {}", e);
    }
}

fn run(app: &AppHandle, tap: &AudioTap) {
    let mut analyzer = LevelAnalyzer::new(tap.sample_rate());
    let mut samples = Vec::with_capacity(analyzer.window_len());
    let mut next_frame = Instant::now();

    while tap.is_live() {
        tap.copy_tail(analyzer.window_len(), &mut samples);
        let levels = analyzer.analyze(&samples);
        if !tap.is_live() {
            break;
        }
        if let Err(e) = app.emit_to(
            INDICATOR_LABEL,
            "indicator-levels",
            LevelsPayload { levels },
        ) {
            tracing::debug!("Failed to emit indicator levels: {}", e);
        }

        next_frame += FRAME_INTERVAL;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            next_frame = now;
        }
    }

    tracing::debug!("Level meter stopped");
}
