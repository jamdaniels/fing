// Audio capture with cpal

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

static OVERFLOW_LOGGED: AtomicBool = AtomicBool::new(false);

/// Maximum recording duration in seconds (2 minutes).
pub const MAX_RECORDING_DURATION_SECS: u32 = 120;
/// Whisper model input sample rate.
pub const WHISPER_SAMPLE_RATE: u32 = 16000;
/// Initial audio buffer capacity before the input device sample rate is known.
const INITIAL_BUFFER_CAPACITY: usize = (MAX_RECORDING_DURATION_SECS * WHISPER_SAMPLE_RATE) as usize;

fn max_buffer_size_for_sample_rate(sample_rate: u32) -> usize {
    MAX_RECORDING_DURATION_SECS as usize * sample_rate as usize
}

/// Audio input device info for frontend display.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Result of a microphone test (audio level check).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneTest {
    pub device_name: String,
    pub peak_level: f32,
    pub is_receiving_audio: bool,
}

/// Result of device lookup (whether requested device was found).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMatchResult {
    pub requested: Option<String>,
    pub actual: String,
    pub matched: bool,
}

/// Errors that can occur during audio capture.
#[derive(Debug, Clone, Serialize)]
pub enum AudioError {
    /// No audio input devices available.
    NoDevicesFound,
    /// Failed to initialize the selected device.
    DeviceInitFailed(String),
    /// Error during audio stream operation.
    StreamError(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoDevicesFound => write!(f, "No audio input devices found"),
            AudioError::DeviceInitFailed(msg) => write!(f, "Failed to initialize device: {msg}"),
            AudioError::StreamError(msg) => write!(f, "Audio stream error: {msg}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Manages microphone capture, buffering, and resampling to 16kHz.
pub struct AudioCapture {
    selected_device_id: Option<String>,
    stream: Option<Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    native_sample_rate: u32,
    is_recording: bool,
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            selected_device_id: None,
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(INITIAL_BUFFER_CAPACITY))),
            native_sample_rate: WHISPER_SAMPLE_RATE,
            is_recording: false,
        }
    }

    pub fn list_devices() -> Vec<AudioDevice> {
        let host = cpal::default_host();
        let default_device = host.default_input_device();
        let default_name = default_device.as_ref().and_then(|d| d.name().ok());

        let mut devices = Vec::new();

        if let Ok(input_devices) = host.input_devices() {
            for device in input_devices {
                if let Ok(name) = device.name() {
                    let is_default = default_name.as_ref() == Some(&name);
                    devices.push(AudioDevice {
                        id: name.clone(),
                        name,
                        is_default,
                    });
                }
            }
        }

        devices
    }

    pub fn set_device(&mut self, device_id: Option<String>) {
        self.selected_device_id = device_id;
    }

    fn get_device(&self) -> Result<(Device, DeviceMatchResult), AudioError> {
        let host = cpal::default_host();

        match &self.selected_device_id {
            Some(id) => {
                let devices: Vec<_> = host
                    .input_devices()
                    .map_err(|_| AudioError::NoDevicesFound)?
                    .filter_map(|d| d.name().ok().map(|n| (d, n)))
                    .collect();

                // Log available devices for debugging
                let device_names: Vec<_> = devices.iter().map(|(_, n)| n.as_str()).collect();
                tracing::info!("Available input devices: {:?}", device_names);
                tracing::info!("Looking for device: '{}'", id);

                // Try exact match first
                if let Some(idx) = devices.iter().position(|(_, name)| name == id) {
                    let actual_name = devices[idx].1.clone();
                    tracing::info!("Exact match found for device '{}'", id);
                    let (device, _) = devices.into_iter().nth(idx).unwrap();
                    return Ok((
                        device,
                        DeviceMatchResult {
                            requested: Some(id.clone()),
                            actual: actual_name,
                            matched: true,
                        },
                    ));
                }

                // Try fuzzy match: trim whitespace and case-insensitive
                let id_normalized = id.trim().to_lowercase();
                if let Some(idx) = devices
                    .iter()
                    .position(|(_, name)| name.trim().to_lowercase() == id_normalized)
                {
                    let actual_name = devices[idx].1.clone();
                    tracing::info!(
                        "Fuzzy match found: requested='{}', actual='{}'",
                        id,
                        actual_name
                    );
                    let (device, _) = devices.into_iter().nth(idx).unwrap();
                    return Ok((
                        device,
                        DeviceMatchResult {
                            requested: Some(id.clone()),
                            actual: actual_name,
                            matched: true,
                        },
                    ));
                }

                // Try contains match (for Bluetooth devices that may have varying names)
                if let Some(idx) = devices.iter().position(|(_, name)| {
                    let name_normalized = name.trim().to_lowercase();
                    name_normalized.contains(&id_normalized)
                        || id_normalized.contains(&name_normalized)
                }) {
                    let actual_name = devices[idx].1.clone();
                    tracing::info!(
                        "Partial match found: requested='{}', actual='{}'",
                        id,
                        actual_name
                    );
                    let (device, _) = devices.into_iter().nth(idx).unwrap();
                    return Ok((
                        device,
                        DeviceMatchResult {
                            requested: Some(id.clone()),
                            actual: actual_name,
                            matched: true,
                        },
                    ));
                }

                // Fall back to default device
                tracing::warn!(
                    "Device '{}' not found among {:?}, falling back to default",
                    id,
                    device_names
                );
                let default_device = host
                    .default_input_device()
                    .ok_or(AudioError::NoDevicesFound)?;
                let default_name = default_device
                    .name()
                    .unwrap_or_else(|_| "Unknown".to_string());
                Ok((
                    default_device,
                    DeviceMatchResult {
                        requested: Some(id.clone()),
                        actual: default_name,
                        matched: false,
                    },
                ))
            }
            None => {
                let device = host
                    .default_input_device()
                    .ok_or(AudioError::NoDevicesFound)?;
                let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                Ok((
                    device,
                    DeviceMatchResult {
                        requested: None,
                        actual: name,
                        matched: true,
                    },
                ))
            }
        }
    }

    pub fn init_capture(&mut self) -> Result<DeviceMatchResult, AudioError> {
        let (device, match_result) = self.get_device()?;
        let device_name = match_result.actual.clone();

        let config = device
            .default_input_config()
            .map_err(|e| AudioError::DeviceInitFailed(e.to_string()))?;

        self.native_sample_rate = config.sample_rate().0;
        let max_buffer_size = max_buffer_size_for_sample_rate(self.native_sample_rate);

        tracing::info!(
            "Initializing audio capture: device='{}', format={:?}, channels={}, sample_rate={}",
            device_name,
            config.sample_format(),
            config.channels(),
            config.sample_rate().0
        );

        if let Ok(mut buf) = self.buffer.lock() {
            let capacity = buf.capacity();
            if capacity < max_buffer_size {
                buf.reserve(max_buffer_size - capacity);
            }
        }

        let buffer = Arc::clone(&self.buffer);
        let channels = config.channels() as usize;

        let stream_config = StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        let err_fn = |err| {
            tracing::error!("Audio stream error: {}", err);
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut buf = match buffer.lock() {
                        Ok(buf) => buf,
                        Err(poisoned) => {
                            tracing::warn!(
                                "Audio buffer mutex poisoned in f32 callback, recovering"
                            );
                            poisoned.into_inner()
                        }
                    };
                    // Convert to mono by averaging channels
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                        if buf.len() < max_buffer_size {
                            buf.push(mono);
                        } else {
                            OVERFLOW_LOGGED.store(true, Ordering::Relaxed);
                        }
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let buffer = Arc::clone(&self.buffer);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mut buf = match buffer.lock() {
                            Ok(buf) => buf,
                            Err(poisoned) => {
                                tracing::warn!(
                                    "Audio buffer mutex poisoned in i16 callback, recovering"
                                );
                                poisoned.into_inner()
                            }
                        };
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                                / channels as f32;
                            if buf.len() < max_buffer_size {
                                buf.push(mono);
                            } else {
                                OVERFLOW_LOGGED.store(true, Ordering::Relaxed);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let buffer = Arc::clone(&self.buffer);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let mut buf = match buffer.lock() {
                            Ok(buf) => buf,
                            Err(poisoned) => {
                                tracing::warn!(
                                    "Audio buffer mutex poisoned in u16 callback, recovering"
                                );
                                poisoned.into_inner()
                            }
                        };
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk
                                .iter()
                                .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                .sum::<f32>()
                                / channels as f32;
                            if buf.len() < max_buffer_size {
                                buf.push(mono);
                            } else {
                                OVERFLOW_LOGGED.store(true, Ordering::Relaxed);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => {
                return Err(AudioError::DeviceInitFailed(
                    "Unsupported sample format".to_string(),
                ))
            }
        }
        .map_err(|e| AudioError::StreamError(e.to_string()))?;

        self.stream = Some(stream);
        Ok(match_result)
    }

    pub fn begin_recording(&mut self) {
        // Reset overflow flag for new session
        OVERFLOW_LOGGED.store(false, Ordering::Relaxed);

        // Clear buffer
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        // Start stream
        if let Some(ref stream) = self.stream {
            let _ = stream.play();
        }
        self.is_recording = true;
    }

    pub fn end_recording(&mut self) -> Vec<f32> {
        // Pause stream
        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }
        self.is_recording = false;
        if OVERFLOW_LOGGED.swap(false, Ordering::Relaxed) {
            tracing::warn!("Audio buffer full (120s max), samples dropped");
        }

        // Extract buffer
        let mut buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(poisoned) => {
                tracing::warn!("Audio buffer mutex poisoned in end_recording, recovering");
                poisoned.into_inner()
            }
        };
        std::mem::take(&mut *buf)
    }

    pub fn close_capture(&mut self) {
        // Pause stream before dropping to ensure audio callbacks stop
        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }
        self.stream = None;
        self.is_recording = false;
        tracing::debug!("Audio capture closed");
    }

    pub fn resample_to_16k(&self, buffer: Vec<f32>) -> Vec<f32> {
        if self.native_sample_rate == WHISPER_SAMPLE_RATE {
            return buffer;
        }

        if buffer.is_empty() {
            return buffer;
        }

        // Use rubato for high-quality resampling
        let chunk_size = 1024;
        let mut resampler = match FftFixedIn::<f32>::new(
            self.native_sample_rate as usize,
            WHISPER_SAMPLE_RATE as usize,
            chunk_size,
            2,
            1,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to create resampler: {}", e);
                // Fallback: simple linear interpolation
                return Self::simple_resample(
                    &buffer,
                    self.native_sample_rate,
                    WHISPER_SAMPLE_RATE,
                );
            }
        };

        let mut output = Vec::new();
        let mut pos = 0;

        while pos < buffer.len() {
            let end = (pos + chunk_size).min(buffer.len());
            let mut chunk = buffer[pos..end].to_vec();

            // Pad last chunk if needed
            if chunk.len() < chunk_size {
                chunk.resize(chunk_size, 0.0);
            }

            match resampler.process(&[chunk], None) {
                Ok(resampled) => {
                    if !resampled.is_empty() {
                        output.extend_from_slice(&resampled[0]);
                    }
                }
                Err(e) => {
                    tracing::error!("Resampling error: {}", e);
                    break;
                }
            }

            pos += chunk_size;
        }

        output
    }

    fn simple_resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
        let ratio = from_rate as f64 / to_rate as f64;
        let output_len = (input.len() as f64 / ratio) as usize;
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let src_idx = i as f64 * ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;

            let sample = if idx + 1 < input.len() {
                input[idx] * (1.0 - frac as f32) + input[idx + 1] * frac as f32
            } else if idx < input.len() {
                input[idx]
            } else {
                0.0
            };

            output.push(sample);
        }

        output
    }

    pub fn test_microphone(&mut self) -> Result<MicrophoneTest, AudioError> {
        let (_, match_result) = self.get_device()?;
        let device_name = match_result.actual;

        // Initialize if not already
        let was_init = self.stream.is_some();
        if !was_init {
            let _ = self.init_capture()?;
        }

        // Clear buffer and record briefly
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        if let Some(ref stream) = self.stream {
            let _ = stream.play();
        }

        // Wait a bit for samples (keep short to avoid blocking IPC)
        std::thread::sleep(std::time::Duration::from_millis(50));

        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }

        // Analyze buffer
        let buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(poisoned) => {
                tracing::warn!("Audio buffer mutex poisoned in test_microphone, recovering");
                poisoned.into_inner()
            }
        };
        let peak_level = buf.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
        let is_receiving_audio = !buf.is_empty() && peak_level > 0.001;

        // Clean up if we initialized
        if !was_init {
            drop(buf);
            self.close_capture();
        }

        Ok(MicrophoneTest {
            device_name,
            peak_level,
            is_receiving_audio,
        })
    }

    /// Start continuous mic test - keeps stream open
    pub fn start_mic_test(&mut self) -> Result<DeviceMatchResult, AudioError> {
        let match_result = self.init_capture()?;

        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        if let Some(ref stream) = self.stream {
            let _ = stream.play();
        }

        self.is_recording = true;
        Ok(match_result)
    }

    /// Get current audio level during mic test and clear old samples
    pub fn get_mic_level(&mut self) -> MicrophoneTest {
        let mut buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(poisoned) => {
                tracing::warn!("Audio buffer mutex poisoned in get_mic_level, recovering");
                poisoned.into_inner()
            }
        };
        let buf_len = buf.len();

        // Calculate peak from all samples
        let peak_level = buf.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
        let is_receiving_audio = buf_len > 0 && peak_level > 0.001;

        // Clear buffer for next reading
        buf.clear();

        tracing::trace!("Mic level: {} (samples: {})", peak_level, buf_len);

        MicrophoneTest {
            device_name: String::new(),
            peak_level,
            is_receiving_audio,
        }
    }

    /// Stop continuous mic test
    pub fn stop_mic_test(&mut self) {
        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }
        self.close_capture();
        self.is_recording = false;
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        // Ensure stream is properly stopped when AudioCapture is dropped
        if self.stream.is_some() {
            tracing::debug!("AudioCapture dropped with active stream, closing");
            self.close_capture();
        }
    }
}
