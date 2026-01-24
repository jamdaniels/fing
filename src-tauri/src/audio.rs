// Audio capture with cpal

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use serde::Serialize;
use std::sync::{Arc, Mutex};

pub const MAX_RECORDING_DURATION_SECS: u32 = 120;
pub const WHISPER_SAMPLE_RATE: u32 = 16000;
pub const MAX_BUFFER_SIZE: usize = (MAX_RECORDING_DURATION_SECS * WHISPER_SAMPLE_RATE) as usize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneTest {
    pub device_name: String,
    pub peak_level: f32,
    pub is_receiving_audio: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum AudioError {
    NoDevicesFound,
    DeviceNotFound,
    DeviceInitFailed(String),
    PermissionDenied,
    StreamError(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoDevicesFound => write!(f, "No audio input devices found"),
            AudioError::DeviceNotFound => write!(f, "Specified audio device not found"),
            AudioError::DeviceInitFailed(msg) => write!(f, "Failed to initialize device: {}", msg),
            AudioError::PermissionDenied => write!(f, "Microphone permission denied"),
            AudioError::StreamError(msg) => write!(f, "Audio stream error: {}", msg),
        }
    }
}

impl std::error::Error for AudioError {}

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
            buffer: Arc::new(Mutex::new(Vec::with_capacity(MAX_BUFFER_SIZE))),
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
                    let is_default = default_name.as_ref().map_or(false, |dn| dn == &name);
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

    fn get_device(&self) -> Result<Device, AudioError> {
        let host = cpal::default_host();

        match &self.selected_device_id {
            Some(id) => {
                let devices = host
                    .input_devices()
                    .map_err(|_| AudioError::NoDevicesFound)?;
                for device in devices {
                    if let Ok(name) = device.name() {
                        if &name == id {
                            return Ok(device);
                        }
                    }
                }
                Err(AudioError::DeviceNotFound)
            }
            None => host.default_input_device().ok_or(AudioError::NoDevicesFound),
        }
    }

    pub fn init_capture(&mut self) -> Result<(), AudioError> {
        let device = self.get_device()?;
        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        let config = device
            .default_input_config()
            .map_err(|e| AudioError::DeviceInitFailed(e.to_string()))?;

        self.native_sample_rate = config.sample_rate().0;

        tracing::info!(
            "Initializing audio capture: device='{}', format={:?}, channels={}, sample_rate={}",
            device_name,
            config.sample_format(),
            config.channels(),
            config.sample_rate().0
        );

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

        // Counter for logging
        use std::sync::atomic::{AtomicU64, Ordering};
        static CALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let count = CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                    if count % 100 == 0 {
                        let peak: f32 = data.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                        tracing::info!("Audio callback #{}: {} samples, peak={:.4}", count, data.len(), peak);
                    }
                    let mut buf = buffer.lock().unwrap();
                    // Convert to mono by averaging channels
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                        if buf.len() < MAX_BUFFER_SIZE {
                            buf.push(mono);
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
                        let count = CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        if count % 100 == 0 {
                            let peak: f32 = data.iter().map(|&s| (s as f32 / 32768.0).abs()).fold(0.0f32, f32::max);
                            tracing::info!("Audio callback #{}: {} samples, peak={:.4}", count, data.len(), peak);
                        }
                        let mut buf = buffer.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                                / channels as f32;
                            if buf.len() < MAX_BUFFER_SIZE {
                                buf.push(mono);
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
                        let mut buf = buffer.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk
                                .iter()
                                .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                .sum::<f32>()
                                / channels as f32;
                            if buf.len() < MAX_BUFFER_SIZE {
                                buf.push(mono);
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
        Ok(())
    }

    pub fn begin_recording(&mut self) {
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

        // Extract buffer
        let mut buf = self.buffer.lock().unwrap();
        std::mem::take(&mut *buf)
    }

    pub fn close_capture(&mut self) {
        self.stream = None;
        self.is_recording = false;
    }

    pub fn native_sample_rate(&self) -> u32 {
        self.native_sample_rate
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
                return Self::simple_resample(&buffer, self.native_sample_rate, WHISPER_SAMPLE_RATE);
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
        let device = self.get_device()?;
        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        // Initialize if not already
        let was_init = self.stream.is_some();
        if !was_init {
            self.init_capture()?;
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
        let buf = self.buffer.lock().unwrap();
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
    pub fn start_mic_test(&mut self) -> Result<String, AudioError> {
        let device = self.get_device()?;
        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        self.init_capture()?;

        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        if let Some(ref stream) = self.stream {
            let _ = stream.play();
        }

        self.is_recording = true;
        Ok(device_name)
    }

    /// Get current audio level during mic test and clear old samples
    pub fn get_mic_level(&mut self) -> MicrophoneTest {
        let mut buf = self.buffer.lock().unwrap();
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
