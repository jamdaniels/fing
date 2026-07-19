use crate::engine::TranscribeError;
use crate::model::{get_definition, ModelVariant};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use whisper_rs::{WhisperContext, WhisperContextParameters, WhisperState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum InferenceDevicePreference {
    #[default]
    Auto,
    Cpu,
    Vulkan {
        #[serde(rename = "deviceId")]
        device_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InferenceBackend {
    Cpu,
    Metal,
    Vulkan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub enum InferenceDeviceKind {
    Cpu,
    IntegratedGpu,
    DiscreteGpu,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceDevice {
    pub id: String,
    pub name: String,
    pub backend: InferenceBackend,
    pub device_kind: InferenceDeviceKind,
    pub memory_free_mb: Option<u64>,
    pub memory_total_mb: Option<u64>,
    pub gpu_device_index: Option<i32>,
    #[serde(skip)]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    registry_index: Option<usize>,
    #[serde(skip)]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    backend_name: Option<String>,
}

impl InferenceDevice {
    fn cpu() -> Self {
        Self {
            id: "cpu".to_string(),
            name: "CPU".to_string(),
            backend: InferenceBackend::Cpu,
            device_kind: InferenceDeviceKind::Cpu,
            memory_free_mb: None,
            memory_total_mb: None,
            gpu_device_index: None,
            registry_index: None,
            backend_name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceRuntimeInfo {
    pub preference: InferenceDevicePreference,
    pub devices: Vec<InferenceDevice>,
    pub recommended_device_id: String,
    pub resolved_device_id: String,
    pub resolved_device_name: String,
    pub resolved_backend: InferenceBackend,
    pub selection_verified: bool,
    pub last_execution_backend: Option<InferenceBackend>,
    pub last_execution_device_name: Option<String>,
    pub last_execution_verified: bool,
    pub fallback_reason: Option<String>,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Default)]
struct LoadedRuntime {
    preference: Option<InferenceDevicePreference>,
    resolved_device: Option<InferenceDevice>,
    selection_verified: bool,
    last_execution_backend: Option<InferenceBackend>,
    last_execution_device_name: Option<String>,
    last_execution_verified: bool,
    fallback_reason: Option<String>,
}

static LOADED_RUNTIME: Lazy<Mutex<LoadedRuntime>> =
    Lazy::new(|| Mutex::new(LoadedRuntime::default()));
static DEVICE_CATALOG: Lazy<Mutex<Option<Vec<InferenceDevice>>>> = Lazy::new(|| Mutex::new(None));

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const USING_BACKEND_MARKER: &str = "using {device} backend";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const FAILED_BACKEND_MARKER: &str = "failed to initialize";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const NO_GPU_MARKER: &str = "no GPU found";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum BackendVerification {
    Verified,
    GpuFailed,
    Unavailable,
}

#[derive(Default)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct BackendObservation {
    expected_backend_name: String,
    saw_expected_backend: bool,
    saw_failure: bool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl BackendObservation {
    fn observe(&mut self, message: &str) {
        let expected_marker = USING_BACKEND_MARKER.replace("{device}", &self.expected_backend_name);
        if message.contains(&expected_marker) {
            self.saw_expected_backend = true;
        }
        if (message.contains(FAILED_BACKEND_MARKER)
            && message.contains(&self.expected_backend_name))
            || message.contains(NO_GPU_MARKER)
        {
            self.saw_failure = true;
        }
    }

    fn verification(&self) -> BackendVerification {
        if self.saw_failure {
            BackendVerification::GpuFailed
        } else if self.saw_expected_backend {
            BackendVerification::Verified
        } else {
            BackendVerification::Unavailable
        }
    }
}

pub struct PreparedContext {
    pub context: WhisperContext,
    pub device: InferenceDevice,
    selection_verified: bool,
    fallback_reason: Option<String>,
}

pub fn runtime_info(
    variant: ModelVariant,
    preference: InferenceDevicePreference,
    refresh_devices: bool,
) -> InferenceRuntimeInfo {
    let devices = device_catalog(refresh_devices);
    let required_mb = required_device_memory_mb(variant);
    let (recommended, _) =
        resolve_preference(&devices, &InferenceDevicePreference::Auto, required_mb);
    let (predicted, predicted_reason) = resolve_preference(&devices, &preference, required_mb);
    #[cfg(target_os = "windows")]
    let prediction_reason = predicted_reason;
    #[cfg(not(target_os = "windows"))]
    let prediction_reason = {
        let _ = predicted_reason;
        None
    };
    let loaded = lock_loaded_runtime().clone();
    let restart_required = loaded
        .preference
        .as_ref()
        .is_some_and(|loaded_preference| loaded_preference != &preference);
    let resolved = loaded.resolved_device.as_ref().unwrap_or(&predicted);

    InferenceRuntimeInfo {
        preference,
        devices,
        recommended_device_id: recommended.id,
        resolved_device_id: resolved.id.clone(),
        resolved_device_name: resolved.name.clone(),
        resolved_backend: resolved.backend,
        selection_verified: loaded.selection_verified,
        last_execution_backend: loaded.last_execution_backend,
        last_execution_device_name: loaded.last_execution_device_name,
        last_execution_verified: loaded.last_execution_verified,
        fallback_reason: loaded.fallback_reason.or(prediction_reason),
        restart_required,
    }
}

pub fn prepare_context(
    model_path: &str,
    variant: ModelVariant,
    preference: InferenceDevicePreference,
) -> Result<PreparedContext, TranscribeError> {
    invalidate_device_catalog();
    #[cfg(not(target_os = "windows"))]
    let _ = variant;
    #[cfg(target_os = "windows")]
    let prepared = prepare_windows_context(model_path, variant, &preference)?;

    #[cfg(not(target_os = "windows"))]
    let prepared = prepare_default_context(model_path)?;

    let mut loaded = lock_loaded_runtime();
    loaded.preference = Some(preference);
    loaded.resolved_device = Some(prepared.device.clone());
    loaded.selection_verified = prepared.selection_verified;
    loaded.fallback_reason = prepared.fallback_reason.clone();
    drop(loaded);

    Ok(prepared)
}

#[cfg(not(target_os = "windows"))]
fn prepare_default_context(model_path: &str) -> Result<PreparedContext, TranscribeError> {
    let context = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
        .map_err(|error| TranscribeError::ModelLoadFailed(error.to_string()))?;
    let device = if cfg!(target_os = "macos") {
        InferenceDevice {
            id: "metal:auto".to_string(),
            name: "Apple GPU".to_string(),
            backend: InferenceBackend::Metal,
            device_kind: InferenceDeviceKind::IntegratedGpu,
            memory_free_mb: None,
            memory_total_mb: None,
            gpu_device_index: Some(0),
            registry_index: None,
            backend_name: None,
        }
    } else {
        InferenceDevice::cpu()
    };
    Ok(PreparedContext {
        context,
        device,
        selection_verified: true,
        fallback_reason: None,
    })
}

pub fn create_state(
    context: &WhisperContext,
    device: &InferenceDevice,
) -> Result<WhisperState, TranscribeError> {
    #[cfg(target_os = "windows")]
    let (state, verification) = create_state_observed(context, device)?;

    #[cfg(not(target_os = "windows"))]
    let (state, verification) = (
        context
            .create_state()
            .map_err(|error| TranscribeError::InferenceFailed(error.to_string()))?,
        BackendVerification::Verified,
    );

    let mut loaded = lock_loaded_runtime();
    apply_execution_status(&mut loaded, device, verification);
    drop(loaded);

    Ok(state)
}

pub fn mark_unloaded() {
    let mut loaded = lock_loaded_runtime();
    reset_loaded_runtime(&mut loaded);
}

fn reset_loaded_runtime(loaded: &mut LoadedRuntime) {
    *loaded = LoadedRuntime::default();
}

fn apply_execution_status(
    loaded: &mut LoadedRuntime,
    device: &InferenceDevice,
    verification: BackendVerification,
) {
    match verification {
        BackendVerification::Verified => {
            loaded.last_execution_backend = Some(device.backend);
            loaded.last_execution_device_name = Some(device.name.clone());
            loaded.last_execution_verified = true;
            loaded.fallback_reason = None;
        }
        BackendVerification::Unavailable => {
            loaded.last_execution_backend = Some(device.backend);
            loaded.last_execution_device_name = Some(device.name.clone());
            loaded.last_execution_verified = false;
            loaded.fallback_reason = None;
        }
        BackendVerification::GpuFailed => {
            loaded.last_execution_backend = Some(InferenceBackend::Cpu);
            loaded.last_execution_device_name = Some("CPU".to_string());
            loaded.last_execution_verified = true;
            loaded.fallback_reason = Some("execution_fell_back_to_cpu".to_string());
        }
    }
}

fn invalidate_device_catalog() {
    let mut catalog = match DEVICE_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    *catalog = None;
}

fn device_catalog(refresh: bool) -> Vec<InferenceDevice> {
    let mut catalog = match DEVICE_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    if refresh || catalog.is_none() {
        *catalog = Some(discover_devices());
    }
    catalog
        .clone()
        .unwrap_or_else(|| vec![InferenceDevice::cpu()])
}

fn lock_loaded_runtime() -> std::sync::MutexGuard<'static, LoadedRuntime> {
    match LOADED_RUNTIME.lock() {
        Ok(loaded) => loaded,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn required_device_memory_mb(variant: ModelVariant) -> u64 {
    let estimate = u64::from(get_definition(variant).memory_estimate_mb);
    (estimate * 5 / 4).max(estimate + 256)
}

fn resolve_preference(
    devices: &[InferenceDevice],
    preference: &InferenceDevicePreference,
    required_mb: u64,
) -> (InferenceDevice, Option<String>) {
    let cpu = devices
        .iter()
        .find(|device| device.backend == InferenceBackend::Cpu)
        .cloned()
        .unwrap_or_else(InferenceDevice::cpu);

    if matches!(preference, InferenceDevicePreference::Cpu) {
        return (cpu, None);
    }

    if let InferenceDevicePreference::Vulkan { device_id } = preference {
        if let Some(device) = devices.iter().find(|device| &device.id == device_id) {
            return (device.clone(), None);
        }
    }

    if let Some(device) = ranked_vulkan_candidates(devices, required_mb)
        .into_iter()
        .next()
    {
        let reason = matches!(preference, InferenceDevicePreference::Vulkan { .. })
            .then(|| "preferred_device_not_found".to_string());
        return (device, reason);
    }

    if matches!(preference, InferenceDevicePreference::Auto) {
        if let Some(device) = devices
            .iter()
            .find(|device| device.backend == InferenceBackend::Metal)
        {
            return (device.clone(), None);
        }
    }

    (cpu, fallback_reason_for(devices, preference))
}

fn ranked_vulkan_candidates(devices: &[InferenceDevice], required_mb: u64) -> Vec<InferenceDevice> {
    let mut candidates = devices
        .iter()
        .filter(|device| device.backend == InferenceBackend::Vulkan)
        .filter(|device| device.memory_free_mb.is_none_or(|free| free >= required_mb))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        device_kind_score(right.device_kind)
            .cmp(&device_kind_score(left.device_kind))
            .then_with(|| right.memory_free_mb.cmp(&left.memory_free_mb))
            .then_with(|| right.memory_total_mb.cmp(&left.memory_total_mb))
            .then_with(|| left.gpu_device_index.cmp(&right.gpu_device_index))
    });
    candidates
}

fn fallback_reason_for(
    devices: &[InferenceDevice],
    preference: &InferenceDevicePreference,
) -> Option<String> {
    if matches!(preference, InferenceDevicePreference::Cpu) {
        return None;
    }
    if preferred_device_missing(devices, preference) {
        return Some("preferred_device_not_found".to_string());
    }
    if devices
        .iter()
        .any(|device| device.backend == InferenceBackend::Vulkan)
    {
        Some("insufficient_gpu_memory".to_string())
    } else {
        Some("no_vulkan_device".to_string())
    }
}

fn preferred_device_missing(
    devices: &[InferenceDevice],
    preference: &InferenceDevicePreference,
) -> bool {
    match preference {
        InferenceDevicePreference::Vulkan { device_id } => {
            !devices.iter().any(|device| &device.id == device_id)
        }
        InferenceDevicePreference::Auto | InferenceDevicePreference::Cpu => false,
    }
}

fn device_kind_score(kind: InferenceDeviceKind) -> u8 {
    match kind {
        InferenceDeviceKind::DiscreteGpu => 2,
        InferenceDeviceKind::IntegratedGpu => 1,
        InferenceDeviceKind::Cpu => 0,
    }
}

#[cfg(target_os = "windows")]
fn prepare_windows_context(
    model_path: &str,
    variant: ModelVariant,
    preference: &InferenceDevicePreference,
) -> Result<PreparedContext, TranscribeError> {
    let devices = device_catalog(false);
    let required_mb = required_device_memory_mb(variant);
    let mut candidates = candidate_order(&devices, preference, required_mb);
    let mut fallback_reason = preferred_device_missing(&devices, preference)
        .then(|| "preferred_device_not_found".to_string());

    tracing::info!(
        candidate_count = candidates.len(),
        required_memory_mb = required_mb,
        "Selecting Windows inference device"
    );

    for device in candidates.drain(..) {
        tracing::info!(
            device = %device.name,
            ordinal = device.gpu_device_index.unwrap_or_default(),
            kind = ?device.device_kind,
            free_memory_mb = ?device.memory_free_mb,
            "Checking Vulkan inference candidate"
        );
        if !probe_device(&device) {
            tracing::warn!(
                device = %device.name,
                ordinal = device.gpu_device_index.unwrap_or_default(),
                "Vulkan device initialization probe failed"
            );
            fallback_reason = Some("device_initialization_failed".to_string());
            continue;
        }

        let mut params = WhisperContextParameters::default();
        params.gpu_device(device.gpu_device_index.unwrap_or_default());
        let context = match WhisperContext::new_with_params(model_path, params) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(
                    device = %device.name,
                    ordinal = device.gpu_device_index.unwrap_or_default(),
                    error = %error,
                    "Failed to create Whisper context for Vulkan device"
                );
                fallback_reason = Some("device_initialization_failed".to_string());
                continue;
            }
        };

        match create_state_observed(&context, &device) {
            Ok((state, BackendVerification::Verified)) => {
                drop(state);
                tracing::info!(
                    device = %device.name,
                    ordinal = device.gpu_device_index.unwrap_or_default(),
                    "Selected and verified Vulkan inference device"
                );
                return Ok(PreparedContext {
                    context,
                    device,
                    selection_verified: true,
                    fallback_reason,
                });
            }
            Ok((state, BackendVerification::Unavailable)) => {
                drop(state);
                tracing::warn!(
                    device = %device.name,
                    ordinal = device.gpu_device_index.unwrap_or_default(),
                    whisper_cpp_version = whisper_rs::WHISPER_CPP_VERSION,
                    "Vulkan state initialized but backend verification markers were not observed; accepting device as unverified"
                );
                return Ok(PreparedContext {
                    context,
                    device,
                    selection_verified: false,
                    fallback_reason,
                });
            }
            Ok((state, BackendVerification::GpuFailed)) => {
                drop(state);
                tracing::warn!(
                    device = %device.name,
                    ordinal = device.gpu_device_index.unwrap_or_default(),
                    "Whisper reported Vulkan initialization failure"
                );
                fallback_reason = Some("device_initialization_failed".to_string());
            }
            Err(error) => {
                tracing::warn!(
                    device = %device.name,
                    ordinal = device.gpu_device_index.unwrap_or_default(),
                    error = %error,
                    "Failed to create Whisper state for Vulkan device"
                );
                fallback_reason = Some("device_initialization_failed".to_string());
            }
        }
    }

    let mut params = WhisperContextParameters::default();
    params.use_gpu(false);
    let context = match WhisperContext::new_with_params(model_path, params) {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(error = %error, "Failed to create CPU Whisper context");
            return Err(TranscribeError::ModelLoadFailed(error.to_string()));
        }
    };
    let fallback_reason = fallback_reason.or_else(|| fallback_reason_for(&devices, preference));
    tracing::warn!(
        reason = fallback_reason.as_deref().unwrap_or("user_selected_cpu"),
        "Using CPU inference backend"
    );

    Ok(PreparedContext {
        context,
        device: InferenceDevice::cpu(),
        selection_verified: true,
        fallback_reason,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn candidate_order(
    devices: &[InferenceDevice],
    preference: &InferenceDevicePreference,
    required_mb: u64,
) -> Vec<InferenceDevice> {
    if matches!(preference, InferenceDevicePreference::Cpu) {
        return Vec::new();
    }

    if let InferenceDevicePreference::Vulkan { device_id } = preference {
        if let Some(device) = devices.iter().find(|device| &device.id == device_id) {
            return vec![device.clone()];
        }
    }

    ranked_vulkan_candidates(devices, required_mb)
}

fn discover_devices() -> Vec<InferenceDevice> {
    #[cfg(target_os = "windows")]
    {
        discover_windows_devices()
    }

    #[cfg(target_os = "macos")]
    {
        vec![InferenceDevice {
            id: "metal:auto".to_string(),
            name: "Apple GPU".to_string(),
            backend: InferenceBackend::Metal,
            device_kind: InferenceDeviceKind::IntegratedGpu,
            memory_free_mb: None,
            memory_total_mb: None,
            gpu_device_index: Some(0),
            registry_index: None,
            backend_name: None,
        }]
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        vec![InferenceDevice::cpu()]
    }
}

#[cfg(target_os = "windows")]
fn discover_windows_devices() -> Vec<InferenceDevice> {
    use std::ffi::CStr;
    use whisper_rs::whisper_rs_sys as sys;

    let mut devices = Vec::new();
    let mut gpu_device_index = 0_i32;
    // ggml_backend_dev_count() initializes the built-in backend registry,
    // including Vulkan. Calling whisper_rs::vulkan::list_devices() first would
    // enumerate every physical device a second time.
    let count = unsafe { sys::ggml_backend_dev_count() };

    for registry_index in 0..count {
        let device = unsafe { sys::ggml_backend_dev_get(registry_index) };
        if device.is_null() {
            continue;
        }
        let raw_kind = unsafe { sys::ggml_backend_dev_type(device) };
        let kind = match raw_kind {
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU => {
                InferenceDeviceKind::DiscreteGpu
            }
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU => {
                InferenceDeviceKind::IntegratedGpu
            }
            _ => continue,
        };

        let backend_name = c_string(unsafe { sys::ggml_backend_dev_name(device) })
            .unwrap_or_else(|| format!("Vulkan{gpu_device_index}"));
        let description = c_string(unsafe { sys::ggml_backend_dev_description(device) })
            .unwrap_or_else(|| backend_name.clone());
        let mut free = 0_usize;
        let mut total = 0_usize;
        unsafe { sys::ggml_backend_dev_memory(device, &mut free, &mut total) };
        let mut props: sys::ggml_backend_dev_props = unsafe { std::mem::zeroed() };
        unsafe { sys::ggml_backend_dev_get_props(device, &mut props) };
        let stable_hardware_id = if props.device_id.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(props.device_id) }
                .to_str()
                .ok()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let id = stable_hardware_id
            .map(|id| format!("vulkan:{id}"))
            .unwrap_or_else(|| format!("vulkan:{description}:{total}"));

        let discovered = InferenceDevice {
            id,
            name: description,
            backend: InferenceBackend::Vulkan,
            device_kind: kind,
            memory_free_mb: Some((free / 1024 / 1024) as u64),
            memory_total_mb: Some((total / 1024 / 1024) as u64),
            gpu_device_index: Some(gpu_device_index),
            registry_index: Some(registry_index),
            backend_name: Some(backend_name),
        };
        tracing::info!(
            device = %discovered.name,
            ordinal = gpu_device_index,
            kind = ?discovered.device_kind,
            free_memory_mb = ?discovered.memory_free_mb,
            total_memory_mb = ?discovered.memory_total_mb,
            "Discovered Vulkan inference device"
        );
        devices.push(discovered);
        gpu_device_index += 1;
    }

    if gpu_device_index == 0 {
        tracing::warn!("No Vulkan inference devices were discovered");
    }
    devices.push(InferenceDevice::cpu());
    devices
}

#[cfg(target_os = "windows")]
fn c_string(pointer: *const std::os::raw::c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(pointer) }
        .to_str()
        .ok()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(target_os = "windows")]
fn probe_device(device: &InferenceDevice) -> bool {
    use whisper_rs::whisper_rs_sys as sys;

    unsafe extern "C" {
        fn fing_probe_ggml_backend(device: *mut std::ffi::c_void) -> i32;
    }

    let Some(registry_index) = device.registry_index else {
        return false;
    };
    let raw_device = unsafe { sys::ggml_backend_dev_get(registry_index) };
    if raw_device.is_null() {
        return false;
    }
    unsafe { fing_probe_ggml_backend(raw_device.cast()) == 1 }
}

#[cfg(target_os = "windows")]
static BACKEND_OBSERVATION: Lazy<Mutex<Option<BackendObservation>>> =
    Lazy::new(|| Mutex::new(None));
#[cfg(target_os = "windows")]
static OBSERVATION_SERIAL: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
#[cfg(target_os = "windows")]
static INSTALL_LOG_OBSERVER: std::sync::Once = std::sync::Once::new();

#[cfg(target_os = "windows")]
unsafe extern "C" fn whisper_log_observer(
    level: whisper_rs::whisper_rs_sys::ggml_log_level,
    text: *const std::os::raw::c_char,
    _user_data: *mut std::ffi::c_void,
) {
    use whisper_rs::whisper_rs_sys as sys;

    if text.is_null() {
        return;
    }
    let message = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
    let observation_active = {
        let mut observation = match BACKEND_OBSERVATION.lock() {
            Ok(observation) => observation,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(observation) = observation.as_mut() {
            observation.observe(&message);
            true
        } else {
            false
        }
    };

    let is_error = level == sys::ggml_log_level_GGML_LOG_LEVEL_ERROR;
    if !(observation_active || is_error) {
        return;
    }
    let Some(message) = safe_whisper_diagnostic(&message) else {
        return;
    };

    if level == sys::ggml_log_level_GGML_LOG_LEVEL_ERROR {
        tracing::error!(target: "whisper_cpp", message = %message);
    } else if level == sys::ggml_log_level_GGML_LOG_LEVEL_WARN {
        tracing::warn!(target: "whisper_cpp", message = %message);
    } else if level == sys::ggml_log_level_GGML_LOG_LEVEL_INFO {
        tracing::info!(target: "whisper_cpp", message = %message);
    } else {
        tracing::debug!(target: "whisper_cpp", message = %message);
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn safe_whisper_diagnostic(message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let has_windows_absolute_path = message.as_bytes().windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'\\' | b'/')
    });
    let has_unix_absolute_path = message.split_whitespace().any(|part| {
        part.trim_start_matches(['\'', '"', '(', '[', '{'])
            .starts_with('/')
    }) || message.contains("=/");
    let may_contain_model_path = lower.contains("loading model from")
        || lower.contains("model path")
        || lower.contains("whisper_init_from_file")
        || lower.contains(".bin")
        || lower.contains("/users/")
        || lower.contains("/home/")
        || lower.contains("\\users\\")
        || has_windows_absolute_path
        || has_unix_absolute_path;
    if may_contain_model_path {
        return None;
    }

    const MAX_DIAGNOSTIC_CHARS: usize = 512;
    let mut truncated = message
        .chars()
        .take(MAX_DIAGNOSTIC_CHARS)
        .collect::<String>();
    if message.chars().count() > MAX_DIAGNOSTIC_CHARS {
        truncated.push('…');
    }
    Some(truncated)
}

#[cfg(target_os = "windows")]
fn create_state_observed(
    context: &WhisperContext,
    device: &InferenceDevice,
) -> Result<(WhisperState, BackendVerification), TranscribeError> {
    if device.backend == InferenceBackend::Cpu {
        return context
            .create_state()
            .map(|state| (state, BackendVerification::Verified))
            .map_err(|error| TranscribeError::InferenceFailed(error.to_string()));
    }

    INSTALL_LOG_OBSERVER.call_once(|| unsafe {
        whisper_rs::set_log_callback(Some(whisper_log_observer), std::ptr::null_mut());
    });
    let _serial = match OBSERVATION_SERIAL.lock() {
        Ok(serial) => serial,
        Err(poisoned) => poisoned.into_inner(),
    };
    let expected_backend_name = device
        .backend_name
        .clone()
        .unwrap_or_else(|| device.name.clone());
    {
        let mut observation = match BACKEND_OBSERVATION.lock() {
            Ok(observation) => observation,
            Err(poisoned) => poisoned.into_inner(),
        };
        *observation = Some(BackendObservation {
            expected_backend_name,
            ..BackendObservation::default()
        });
    }

    let state_result = context
        .create_state()
        .map_err(|error| TranscribeError::InferenceFailed(error.to_string()));
    let observation = match BACKEND_OBSERVATION.lock() {
        Ok(mut observation) => observation.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    let verification = observation
        .as_ref()
        .map(BackendObservation::verification)
        .unwrap_or(BackendVerification::Unavailable);
    if verification == BackendVerification::Unavailable {
        tracing::warn!(
            device = %device.name,
            whisper_cpp_version = whisper_rs::WHISPER_CPP_VERSION,
            "Whisper backend verification contract was not observed"
        );
    }
    Ok((state_result?, verification))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(id: &str, kind: InferenceDeviceKind, free_mb: u64) -> InferenceDevice {
        InferenceDevice {
            id: id.to_string(),
            name: id.to_string(),
            backend: InferenceBackend::Vulkan,
            device_kind: kind,
            memory_free_mb: Some(free_mb),
            memory_total_mb: Some(free_mb),
            gpu_device_index: Some(0),
            registry_index: None,
            backend_name: None,
        }
    }

    fn metal() -> InferenceDevice {
        InferenceDevice {
            id: "metal:auto".to_string(),
            name: "Apple GPU".to_string(),
            backend: InferenceBackend::Metal,
            device_kind: InferenceDeviceKind::IntegratedGpu,
            memory_free_mb: None,
            memory_total_mb: None,
            gpu_device_index: Some(0),
            registry_index: None,
            backend_name: None,
        }
    }

    #[test]
    fn auto_prefers_a_fitting_discrete_gpu() {
        let devices = vec![
            gpu("integrated", InferenceDeviceKind::IntegratedGpu, 4096),
            gpu("discrete", InferenceDeviceKind::DiscreteGpu, 2048),
            InferenceDevice::cpu(),
        ];
        let (resolved, reason) =
            resolve_preference(&devices, &InferenceDevicePreference::Auto, 1024);
        assert_eq!(resolved.id, "discrete");
        assert_eq!(reason, None);
    }

    #[test]
    fn auto_uses_cpu_when_no_gpu_fits() {
        let devices = vec![
            gpu("small", InferenceDeviceKind::DiscreteGpu, 512),
            InferenceDevice::cpu(),
        ];
        let (resolved, reason) =
            resolve_preference(&devices, &InferenceDevicePreference::Auto, 1024);
        assert_eq!(resolved.id, "cpu");
        assert_eq!(reason.as_deref(), Some("insufficient_gpu_memory"));
    }

    #[test]
    fn explicit_device_is_respected_even_when_memory_is_low() {
        let devices = vec![
            gpu("chosen", InferenceDeviceKind::IntegratedGpu, 256),
            InferenceDevice::cpu(),
        ];
        let preference = InferenceDevicePreference::Vulkan {
            device_id: "chosen".to_string(),
        };
        let (resolved, reason) = resolve_preference(&devices, &preference, 2048);
        assert_eq!(resolved.id, "chosen");
        assert_eq!(reason, None);
    }

    #[test]
    fn auto_uses_metal_when_vulkan_is_not_available() {
        let devices = vec![metal(), InferenceDevice::cpu()];

        let (resolved, reason) =
            resolve_preference(&devices, &InferenceDevicePreference::Auto, 1024);

        assert_eq!(resolved.backend, InferenceBackend::Metal);
        assert_eq!(reason, None);
    }

    #[test]
    fn prediction_and_load_paths_share_vulkan_ranking() {
        let devices = vec![
            gpu("integrated", InferenceDeviceKind::IntegratedGpu, 4096),
            gpu("too-small", InferenceDeviceKind::DiscreteGpu, 512),
            gpu("discrete", InferenceDeviceKind::DiscreteGpu, 2048),
            InferenceDevice::cpu(),
        ];
        let ranked = ranked_vulkan_candidates(&devices, 1024);
        let load_order = candidate_order(&devices, &InferenceDevicePreference::Auto, 1024);

        assert_eq!(
            ranked.iter().map(|device| &device.id).collect::<Vec<_>>(),
            load_order
                .iter()
                .map(|device| &device.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(ranked[0].id, "discrete");
        assert_eq!(ranked[1].id, "integrated");
    }

    #[test]
    fn missing_manual_device_has_consistent_fallback_reason() {
        let devices = vec![
            gpu("available", InferenceDeviceKind::DiscreteGpu, 2048),
            InferenceDevice::cpu(),
        ];
        let preference = InferenceDevicePreference::Vulkan {
            device_id: "missing".to_string(),
        };

        let (_, reason) = resolve_preference(&devices, &preference, 1024);

        assert_eq!(reason.as_deref(), Some("preferred_device_not_found"));
        assert_eq!(
            fallback_reason_for(&devices, &preference).as_deref(),
            Some("preferred_device_not_found")
        );
    }

    #[test]
    fn observer_contract_matches_pinned_whisper_cpp_markers() {
        assert_eq!(
            whisper_rs::WHISPER_CPP_VERSION,
            "1.8.3",
            "review the backend log markers before upgrading whisper.cpp"
        );
        let mut verified = BackendObservation {
            expected_backend_name: "Vulkan1".to_string(),
            ..BackendObservation::default()
        };
        verified.observe("whisper_backend_init_gpu: using Vulkan1 backend\n");
        assert_eq!(verified.verification(), BackendVerification::Verified);

        let mut failed = BackendObservation {
            expected_backend_name: "Vulkan1".to_string(),
            ..BackendObservation::default()
        };
        failed.observe("whisper_backend_init_gpu: using Vulkan1 backend\n");
        failed.observe("whisper_backend_init_gpu: failed to initialize Vulkan1 backend\n");
        assert_eq!(failed.verification(), BackendVerification::GpuFailed);

        let unavailable = BackendObservation {
            expected_backend_name: "Vulkan1".to_string(),
            ..BackendObservation::default()
        };
        assert_eq!(unavailable.verification(), BackendVerification::Unavailable);
    }

    #[test]
    fn successful_execution_clears_transient_fallback_status() {
        let device = gpu("discrete", InferenceDeviceKind::DiscreteGpu, 2048);
        let mut loaded = LoadedRuntime::default();
        apply_execution_status(&mut loaded, &device, BackendVerification::GpuFailed);
        assert_eq!(
            loaded.fallback_reason.as_deref(),
            Some("execution_fell_back_to_cpu")
        );

        apply_execution_status(&mut loaded, &device, BackendVerification::Verified);

        assert_eq!(
            loaded.last_execution_backend,
            Some(InferenceBackend::Vulkan)
        );
        assert!(loaded.last_execution_verified);
        assert_eq!(loaded.fallback_reason, None);
    }

    #[test]
    fn unloading_clears_all_runtime_status() {
        let device = gpu("discrete", InferenceDeviceKind::DiscreteGpu, 2048);
        let mut loaded = LoadedRuntime {
            preference: Some(InferenceDevicePreference::Auto),
            resolved_device: Some(device),
            selection_verified: true,
            last_execution_backend: Some(InferenceBackend::Cpu),
            last_execution_device_name: Some("CPU".to_string()),
            last_execution_verified: true,
            fallback_reason: Some("execution_fell_back_to_cpu".to_string()),
        };

        reset_loaded_runtime(&mut loaded);

        assert_eq!(loaded.preference, None);
        assert!(loaded.resolved_device.is_none());
        assert!(!loaded.selection_verified);
        assert_eq!(loaded.last_execution_backend, None);
        assert_eq!(loaded.last_execution_device_name, None);
        assert!(!loaded.last_execution_verified);
        assert_eq!(loaded.fallback_reason, None);
    }

    #[test]
    fn diagnostic_filter_drops_model_paths_and_truncates_messages() {
        assert!(safe_whisper_diagnostic(
            "whisper_model_load: loading model from 'C:\\Users\\person\\model.bin'"
        )
        .is_none());
        assert!(safe_whisper_diagnostic("backend error at /Users/person/private/file").is_none());
        assert_eq!(
            safe_whisper_diagnostic("whisper_backend_init_gpu: using Vulkan0 backend\n").as_deref(),
            Some("whisper_backend_init_gpu: using Vulkan0 backend")
        );
        assert!(safe_whisper_diagnostic(&"x".repeat(600))
            .is_some_and(|message| message.chars().count() == 513));
    }
}
