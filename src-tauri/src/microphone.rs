//! Which microphone records.
//!
//! The dropdown choice lives in `selectedMicrophone*` (no ID = follow the
//! system default) and the hearted mic in `preferredMicrophone*`. A manual pick
//! wins until the preferred mic is unplugged and plugged back in, or until the
//! picked mic itself disappears; then the preferred mic becomes the selection
//! again. Without a connected preferred mic, a missing (or unopenable) mic
//! falls back to the system default without touching settings.

use crate::audio::{AudioCapture, AudioDevice, AudioError};
use crate::settings::Settings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// Emitted with the full settings after the backend changed microphone fields.
pub const SETTINGS_CHANGED_EVENT: &str = "microphone-settings-changed";

/// Set once the preferred mic has been seen unplugged; the next time it shows
/// up it becomes the selection again.
static PREFERRED_MISSING: AtomicBool = AtomicBool::new(false);

/// Device ID of the previous recording, so a mic change is announced once.
static LAST_RECORDING_MIC: Mutex<Option<String>> = Mutex::new(None);

/// A remembered microphone. The name finds it again when its ID changes, e.g.
/// a USB mic without a serial number moved to another port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicRef {
    pub id: String,
    pub name: Option<String>,
}

impl MicRef {
    pub fn new(id: Option<String>, name: Option<String>) -> Option<Self> {
        let id = id.filter(|id| !id.trim().is_empty())?;
        Some(Self {
            id,
            name: name.filter(|name| !name.trim().is_empty()),
        })
    }

    fn of(device: &AudioDevice) -> Self {
        Self {
            id: device.id.clone(),
            name: Some(device.name.clone()),
        }
    }
}

/// The microphone settings that decide which device records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicChoice {
    pub selected: Option<MicRef>,
    pub preferred: Option<MicRef>,
}

impl MicChoice {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            selected: MicRef::new(
                settings.selected_microphone_id.clone(),
                settings.selected_microphone_name.clone(),
            ),
            preferred: MicRef::new(
                settings.preferred_microphone_id.clone(),
                settings.preferred_microphone_name.clone(),
            ),
        }
    }

    fn apply_to(&self, settings: &mut Settings) {
        settings.selected_microphone_id = self.selected.as_ref().map(|mic| mic.id.clone());
        settings.selected_microphone_name = self.selected.as_ref().and_then(|mic| mic.name.clone());
        settings.preferred_microphone_id = self.preferred.as_ref().map(|mic| mic.id.clone());
        settings.preferred_microphone_name =
            self.preferred.as_ref().and_then(|mic| mic.name.clone());
    }
}

/// Index of the device `mic` refers to: exact ID first, then the remembered
/// name, then IDs saved by older versions (which stored the device name).
pub fn find_device(devices: &[AudioDevice], mic: &MicRef) -> Option<usize> {
    if let Some(index) = devices.iter().position(|device| device.id == mic.id) {
        return Some(index);
    }
    if let Some(name) = &mic.name {
        if let Some(index) = devices.iter().position(|device| device.name == *name) {
            return Some(index);
        }
    }
    let legacy_id = mic.id.trim().to_lowercase();
    devices.iter().position(|device| {
        device.legacy_id.trim().to_lowercase() == legacy_id
            || device.name.trim().to_lowercase() == legacy_id
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Resolution {
    /// Device to record with; `None` means the system default.
    pub device: Option<usize>,
    /// A specific mic was wanted but is not connected.
    pub fell_back: bool,
    /// Whether the preferred mic is connected (`None` without one).
    pub preferred_connected: Option<bool>,
    /// The preferred mic is back after being unplugged: make it the selection.
    pub switch_back: bool,
}

pub fn resolve(
    devices: &[AudioDevice],
    choice: &MicChoice,
    preferred_was_missing: bool,
) -> Resolution {
    let selected = choice
        .selected
        .as_ref()
        .map(|mic| find_device(devices, mic));
    let preferred = choice
        .preferred
        .as_ref()
        .map(|mic| find_device(devices, mic));

    if let Some(Some(preferred_index)) = preferred {
        let is_selected = selected == Some(Some(preferred_index));
        let selected_missing = selected == Some(None);
        if is_selected || preferred_was_missing || selected_missing {
            return Resolution {
                device: Some(preferred_index),
                fell_back: false,
                preferred_connected: Some(true),
                switch_back: !is_selected,
            };
        }
    }

    Resolution {
        device: selected.flatten(),
        fell_back: matches!(selected, Some(None)),
        preferred_connected: preferred.map(|index| index.is_some()),
        switch_back: false,
    }
}

/// Resolves against the current devices and records whether the preferred mic
/// was seen. Choices without a preferred mic (e.g. a mic test) leave it alone,
/// and so does an empty list: that is a failed enumeration (or no permission
/// yet), not proof the preferred mic was unplugged.
fn resolve_and_observe(devices: &[AudioDevice], choice: &MicChoice) -> Resolution {
    let resolution = resolve(devices, choice, PREFERRED_MISSING.load(Ordering::SeqCst));
    if let (Some(connected), false) = (resolution.preferred_connected, devices.is_empty()) {
        PREFERRED_MISSING.store(!connected, Ordering::SeqCst);
    }
    resolution
}

/// Settings after `resolution`: the switch-back, plus current IDs and names for
/// mics that were found by name or legacy ID. `None` if nothing changes.
fn updated_choice(
    devices: &[AudioDevice],
    choice: &MicChoice,
    resolution: &Resolution,
) -> Option<MicChoice> {
    let refreshed = |mic: &Option<MicRef>| {
        mic.as_ref().map(|mic| {
            find_device(devices, mic)
                .map(|index| MicRef::of(&devices[index]))
                .unwrap_or_else(|| mic.clone())
        })
    };
    let preferred = refreshed(&choice.preferred);
    let selected = if resolution.switch_back {
        preferred.clone()
    } else {
        refreshed(&choice.selected)
    };
    let next = MicChoice {
        selected,
        preferred,
    };
    (next != *choice).then_some(next)
}

/// The microphone a recording or mic test opened.
#[derive(Debug)]
pub struct OpenedMic {
    pub id: String,
    pub name: String,
    /// A specific mic was wanted but another one is recording.
    pub fell_back: bool,
    /// Microphone settings to save in place of the choice the mic was opened with.
    pub settings_update: Option<MicChoice>,
}

/// Opens the microphone `choice` asks for, falling back to the system default
/// when it is missing or fails to open.
pub fn open(capture: &mut AudioCapture, choice: &MicChoice) -> Result<OpenedMic, AudioError> {
    // Following the system default needs no device list, which keeps the
    // hotkey fast.
    let inputs = if *choice == MicChoice::default() {
        Vec::new()
    } else {
        AudioCapture::input_devices()
    };
    let devices: Vec<AudioDevice> = inputs.iter().map(|input| input.info.clone()).collect();
    let resolution = resolve_and_observe(&devices, choice);
    let settings_update = updated_choice(&devices, choice, &resolution);

    let wanted = resolution.device.map(|index| &inputs[index]);
    let mut failed = None;
    if let Some(input) = wanted {
        match capture.init_capture(input) {
            Ok(()) => {
                return Ok(OpenedMic {
                    id: input.info.id.clone(),
                    name: input.info.name.clone(),
                    fell_back: false,
                    settings_update,
                })
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to open '{}', using the system default: {}",
                    input.info.name,
                    error
                );
                failed = Some((input, error));
            }
        }
    }

    // Reuse the default from the list when there is one instead of asking the
    // OS again.
    let looked_up;
    let fallback = match inputs.iter().find(|input| input.info.is_default) {
        Some(input) => input,
        None => {
            looked_up = AudioCapture::default_input_device().ok_or(AudioError::NoDevicesFound)?;
            &looked_up
        }
    };
    if let Some((input, error)) = failed {
        if fallback.info.id == input.info.id {
            return Err(error);
        }
    }

    capture.init_capture(fallback)?;
    Ok(OpenedMic {
        id: fallback.info.id.clone(),
        name: fallback.info.name.clone(),
        fell_back: resolution.fell_back || wanted.is_some(),
        settings_update,
    })
}

/// Whether a recording on `opened` should be announced: when the mic differs
/// from the previous recording's, or on the first recording if it fell back.
fn should_announce(previous: Option<&str>, opened: &OpenedMic) -> bool {
    match previous {
        Some(previous) => previous != opened.id,
        None => opened.fell_back,
    }
}

/// Records the mic a recording started with; true if it should be announced.
pub fn note_recording_mic(opened: &OpenedMic) -> bool {
    let mut last = match LAST_RECORDING_MIC.lock() {
        Ok(last) => last,
        Err(poisoned) => poisoned.into_inner(),
    };
    let announce = should_announce(last.as_deref(), opened);
    *last = Some(opened.id.clone());
    announce
}

/// Saves `next` only if the microphone settings still equal `expected`, so an
/// automatic update never overrides a choice made in the meantime.
pub async fn persist_update(app: &AppHandle, expected: MicChoice, next: MicChoice) {
    let result = crate::settings::update_settings_atomic(|settings| {
        if MicChoice::from_settings(settings) == expected {
            next.apply_to(settings);
        }
    })
    .await;

    match result {
        Ok(settings) if MicChoice::from_settings(&settings) == next => {
            tracing::info!("Updated microphone settings automatically");
            emit_settings_changed(app, &settings);
        }
        Ok(_) => tracing::debug!("Skipped microphone update; selection changed meanwhile"),
        Err(error) => tracing::error!("Failed to save microphone settings: {}", error),
    }
}

fn emit_settings_changed(app: &AppHandle, settings: &Settings) {
    if let Err(error) = app.emit(SETTINGS_CHANGED_EVENT, settings) {
        tracing::warn!("Failed to emit microphone settings change: {}", error);
    }
}

async fn enumerate() -> Vec<AudioDevice> {
    tauri::async_runtime::spawn_blocking(AudioCapture::list_devices)
        .await
        .unwrap_or_default()
}

async fn saved_choice() -> MicChoice {
    MicChoice::from_settings(&crate::settings::load_settings().await)
}

/// Lists input devices for the UI. Also notices the preferred mic coming back,
/// so the dropdown already shows it before the next recording.
pub async fn list_devices(app: &AppHandle) -> Vec<AudioDevice> {
    let devices = enumerate().await;
    let choice = saved_choice().await;
    let resolution = resolve_and_observe(&devices, &choice);
    if let Some(next) = updated_choice(&devices, &choice, &resolution) {
        persist_update(app, choice, next).await;
    }
    devices
}

/// Saves the dropdown choice (`None` = system default).
pub async fn set_selected(selected: Option<MicRef>) -> Result<Settings, String> {
    // Record whether the preferred mic is connected right now, so a pick made
    // while it is plugged in wins until it is unplugged and plugged back in.
    let choice = saved_choice().await;
    if choice.preferred.is_some() {
        resolve_and_observe(&enumerate().await, &choice);
    }

    crate::settings::update_settings_atomic(|settings| {
        let preferred = MicChoice::from_settings(settings).preferred;
        MicChoice {
            selected,
            preferred,
        }
        .apply_to(settings);
    })
    .await
}

/// Hearts `preferred` (which also becomes the selection) or clears the heart.
pub async fn set_preferred(preferred: Option<MicRef>) -> Result<Settings, String> {
    PREFERRED_MISSING.store(false, Ordering::SeqCst);
    crate::settings::update_settings_atomic(|settings| {
        let selected = match &preferred {
            Some(mic) => Some(mic.clone()),
            None => MicChoice::from_settings(settings).selected,
        };
        MicChoice {
            selected,
            preferred,
        }
        .apply_to(settings);
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str) -> AudioDevice {
        AudioDevice {
            id: id.to_string(),
            name: name.to_string(),
            is_default: false,
            legacy_id: name.to_string(),
        }
    }

    fn mic(id: &str, name: &str) -> Option<MicRef> {
        MicRef::new(Some(id.to_string()), Some(name.to_string()))
    }

    fn devices() -> Vec<AudioDevice> {
        vec![
            device("builtin", "MacBook Pro Microphone"),
            device("hyperx", "HyperX QuadCast"),
        ]
    }

    #[test]
    fn finds_by_id_then_name_then_legacy_id() {
        let devices = devices();
        assert_eq!(
            find_device(&devices, &mic("hyperx", "Old").unwrap()),
            Some(1)
        );
        assert_eq!(
            find_device(
                &devices,
                &mic("hyperx-other-port", "HyperX QuadCast").unwrap()
            ),
            Some(1)
        );
        let legacy = MicRef::new(Some("hyperx quadcast".to_string()), None).unwrap();
        assert_eq!(find_device(&devices, &legacy), Some(1));
    }

    #[test]
    fn never_matches_on_partial_names() {
        let devices = vec![device("usb-2", "USB Audio Device")];
        assert_eq!(
            find_device(&devices, &mic("usb-1", "USB Audio").unwrap()),
            None
        );
    }

    #[test]
    fn follows_system_default_without_a_choice() {
        let resolution = resolve(&devices(), &MicChoice::default(), false);
        assert_eq!(resolution.device, None);
        assert!(!resolution.fell_back);
        assert_eq!(resolution.preferred_connected, None);
    }

    #[test]
    fn missing_selection_falls_back_to_system_default() {
        let choice = MicChoice {
            selected: mic("jabra", "Jabra Evolve2"),
            preferred: None,
        };
        let resolution = resolve(&devices(), &choice, false);
        assert_eq!(resolution.device, None);
        assert!(resolution.fell_back);
    }

    #[test]
    fn preferred_records_when_selected() {
        let choice = MicChoice {
            selected: mic("hyperx", "HyperX QuadCast"),
            preferred: mic("hyperx", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices(), &choice, false);
        assert_eq!(resolution.device, Some(1));
        assert!(!resolution.switch_back);
        assert_eq!(resolution.preferred_connected, Some(true));
    }

    #[test]
    fn manual_pick_wins_while_preferred_stays_connected() {
        let choice = MicChoice {
            selected: mic("builtin", "MacBook Pro Microphone"),
            preferred: mic("hyperx", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices(), &choice, false);
        assert_eq!(resolution.device, Some(0));
        assert!(!resolution.switch_back);
    }

    #[test]
    fn preferred_takes_over_again_after_replug() {
        let choice = MicChoice {
            selected: None,
            preferred: mic("hyperx", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices(), &choice, true);
        assert_eq!(resolution.device, Some(1));
        assert!(resolution.switch_back);

        let next = updated_choice(&devices(), &choice, &resolution).unwrap();
        assert_eq!(next.selected, choice.preferred);
    }

    #[test]
    fn preferred_takes_over_when_the_picked_mic_disappears() {
        let choice = MicChoice {
            selected: mic("usb", "USB Microphone"),
            preferred: mic("hyperx", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices(), &choice, false);
        assert_eq!(resolution.device, Some(1));
        assert!(!resolution.fell_back);
        assert!(resolution.switch_back);
    }

    #[test]
    fn unplugged_preferred_falls_back_and_is_reported_missing() {
        let choice = MicChoice {
            selected: mic("hyperx", "HyperX QuadCast"),
            preferred: mic("hyperx", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices()[..1], &choice, false);
        assert_eq!(resolution.device, None);
        assert!(resolution.fell_back);
        assert_eq!(resolution.preferred_connected, Some(false));
        assert_eq!(updated_choice(&devices()[..1], &choice, &resolution), None);
    }

    #[test]
    fn refreshes_ids_and_names_of_mics_found_by_name() {
        let choice = MicChoice {
            selected: MicRef::new(Some("MacBook Pro Microphone".to_string()), None),
            preferred: mic("hyperx-other-port", "HyperX QuadCast"),
        };
        let resolution = resolve(&devices(), &choice, false);
        let next = updated_choice(&devices(), &choice, &resolution).unwrap();
        assert_eq!(next.selected, mic("builtin", "MacBook Pro Microphone"));
        assert_eq!(next.preferred, mic("hyperx", "HyperX QuadCast"));
    }

    fn opened(id: &str, fell_back: bool) -> OpenedMic {
        OpenedMic {
            id: id.to_string(),
            name: String::new(),
            fell_back,
            settings_update: None,
        }
    }

    #[test]
    fn announces_mic_changes_and_first_fallback_only() {
        assert!(!should_announce(None, &opened("builtin", false)));
        assert!(should_announce(None, &opened("builtin", true)));
        assert!(!should_announce(Some("builtin"), &opened("builtin", true)));
        assert!(should_announce(Some("builtin"), &opened("hyperx", false)));
    }
}
