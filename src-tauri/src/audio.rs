// Audio capture with cpal

use audioadapter_buffers::direct::InterleavedSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use rubato::{Fft, FixedSync, Resampler};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    pub legacy_id: String,
}

/// An input device together with the handle needed to open it.
pub struct InputDevice {
    pub info: AudioDevice,
    device: Device,
}

/// Result of a microphone test (audio level check).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneTest {
    pub device_name: String,
    pub peak_level: f32,
    pub is_receiving_audio: bool,
}

fn display_name(description: &cpal::DeviceDescription) -> String {
    #[cfg(target_os = "windows")]
    if let Some(name) = description
        .extended()
        .first()
        .filter(|name| !name.trim().is_empty())
    {
        return name.trim().to_string();
    }

    description.name().to_string()
}

fn input_device(device: Device, default_id: Option<&str>) -> InputDevice {
    let description = device.description().ok();
    let legacy_id = description
        .as_ref()
        .map(|description| description.name().to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let name = description
        .as_ref()
        .map(display_name)
        .unwrap_or_else(|| legacy_id.clone());
    let id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| legacy_id.clone());

    InputDevice {
        info: AudioDevice {
            is_default: default_id == Some(id.as_str()),
            id,
            name,
            legacy_id,
        },
        device,
    }
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
    stream: Option<Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    native_sample_rate: u32,
    is_recording: bool,
    /// Current recording session for [`AudioTap::is_live`]; 0 while not recording.
    live_session: Arc<AtomicU64>,
    last_session: u64,
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(INITIAL_BUFFER_CAPACITY))),
            native_sample_rate: WHISPER_SAMPLE_RATE,
            is_recording: false,
            live_session: Arc::new(AtomicU64::new(0)),
            last_session: 0,
        }
    }

    /// All input devices, with the system default flagged. Enumerating does
    /// not open any device.
    pub fn input_devices() -> Vec<InputDevice> {
        let host = cpal::default_host();
        let default_id = host
            .default_input_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());

        match host.input_devices() {
            Ok(devices) => devices
                .map(|device| input_device(device, default_id.as_deref()))
                .collect(),
            Err(error) => {
                tracing::warn!("Failed to enumerate input devices: {}", error);
                Vec::new()
            }
        }
    }

    pub fn list_devices() -> Vec<AudioDevice> {
        Self::input_devices()
            .into_iter()
            .map(|device| device.info)
            .collect()
    }

    /// The input device the OS currently uses by default.
    pub fn default_input_device() -> Option<InputDevice> {
        let device = cpal::default_host().default_input_device()?;
        let mut input = input_device(device, None);
        input.info.is_default = true;
        Some(input)
    }

    pub fn init_capture(&mut self, input: &InputDevice) -> Result<(), AudioError> {
        let device = &input.device;
        let device_name = &input.info.name;

        let config = device
            .default_input_config()
            .map_err(|e| AudioError::DeviceInitFailed(e.to_string()))?;

        self.native_sample_rate = config.sample_rate();
        let max_buffer_size = max_buffer_size_for_sample_rate(self.native_sample_rate);

        tracing::info!(
            "Initializing audio capture: device='{}', format={:?}, channels={}, sample_rate={}",
            device_name,
            config.sample_format(),
            config.channels(),
            config.sample_rate()
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
        Ok(())
    }

    /// Cheap handle for reading recent samples while recording (level meter).
    /// It goes stale once the current recording ends.
    pub fn tap(&self) -> AudioTap {
        AudioTap {
            buffer: Arc::clone(&self.buffer),
            sample_rate: self.native_sample_rate,
            live_session: Arc::clone(&self.live_session),
            session: self.live_session.load(Ordering::SeqCst),
        }
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
        self.last_session = self.last_session.wrapping_add(1).max(1);
        self.live_session.store(self.last_session, Ordering::SeqCst);
    }

    pub fn end_recording(&mut self) -> Vec<f32> {
        // Pause stream
        if let Some(ref stream) = self.stream {
            let _ = stream.pause();
        }
        self.is_recording = false;
        self.live_session.store(0, Ordering::SeqCst);
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
        self.live_session.store(0, Ordering::SeqCst);
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
        let mut resampler = match Fft::<f32>::new(
            self.native_sample_rate as usize,
            WHISPER_SAMPLE_RATE as usize,
            chunk_size,
            2,
            1,
            FixedSync::Input,
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

        let input_frames = buffer.len();
        let input_adapter = match InterleavedSlice::new(&buffer, 1, input_frames) {
            Ok(adapter) => adapter,
            Err(e) => {
                tracing::error!("Failed to create input adapter: {}", e);
                return Self::simple_resample(
                    &buffer,
                    self.native_sample_rate,
                    WHISPER_SAMPLE_RATE,
                );
            }
        };

        let mut output = vec![0.0f32; resampler.process_all_needed_output_len(input_frames)];
        let output_capacity = output.len();
        let process_result = {
            let mut output_adapter =
                match InterleavedSlice::new_mut(&mut output, 1, output_capacity) {
                    Ok(adapter) => adapter,
                    Err(e) => {
                        tracing::error!("Failed to create output adapter: {}", e);
                        return Self::simple_resample(
                            &buffer,
                            self.native_sample_rate,
                            WHISPER_SAMPLE_RATE,
                        );
                    }
                };

            resampler.process_all_into_buffer(
                &input_adapter,
                &mut output_adapter,
                input_frames,
                None,
            )
        };

        match process_result {
            Ok((_, output_frames)) => {
                output.truncate(output_frames);
                output
            }
            Err(e) => {
                tracing::error!("Resampling error: {}", e);
                Self::simple_resample(&buffer, self.native_sample_rate, WHISPER_SAMPLE_RATE)
            }
        }
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
}

/// Read-only view of the capture buffer (mono samples at the native rate).
/// Holds no reference to the input stream, so it never keeps the mic alive.
#[derive(Clone)]
pub struct AudioTap {
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    live_session: Arc<AtomicU64>,
    session: u64,
}

impl AudioTap {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Whether the recording this tap was taken for is still running.
    pub fn is_live(&self) -> bool {
        self.session != 0 && self.live_session.load(Ordering::SeqCst) == self.session
    }

    /// Replace `out` with (at most) the `count` most recent samples. The lock
    /// is held only for the copy.
    pub fn copy_tail(&self, count: usize, out: &mut Vec<f32>) {
        let buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(poisoned) => poisoned.into_inner(),
        };
        out.clear();
        out.extend_from_slice(&buf[buf.len().saturating_sub(count)..]);
    }
}

/// Number of frequency bands reported to the recording indicator.
pub const LEVEL_BAND_COUNT: usize = 3;
/// Band edges in Hz: voice fundamentals, formants, upper formants/consonants.
const LEVEL_BANDS_HZ: [(f32, f32); LEVEL_BAND_COUNT] =
    [(85.0, 300.0), (300.0, 1500.0), (1500.0, 4500.0)];
/// Per-band gain compensating for speech's spectral tilt (~6 dB/octave), so
/// the mid/high bands move visibly for normal speech.
const LEVEL_TILT_DB: [f32; LEVEL_BAND_COUNT] = [0.0, 4.0, 10.0];
/// Tilt-compensated band power (dBFS) mapped to 0; room noise stays below it.
const LEVEL_NOISE_FLOOR_DB: f32 = -58.0;
/// dB above the noise floor that maps to a full level of 1. Wide enough that
/// loud speech keeps headroom; the indicator rescales to recent peaks itself.
const LEVEL_RANGE_DB: f32 = 48.0;
/// Analysis window length; the FFT size is the next power of two.
const LEVEL_WINDOW_SECS: f32 = 0.040;

/// Computes perceptual 0..1 voice levels for [`LEVEL_BAND_COUNT`] bands.
/// Owns the FFT plan and buffers so per-frame analysis does not allocate.
pub struct LevelAnalyzer {
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    band_bins: [(usize, usize); LEVEL_BAND_COUNT],
}

/// Callers guarantee `len >= 2`.
fn hann(index: usize, len: usize) -> f32 {
    0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / (len - 1) as f32).cos()
}

impl LevelAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        let fft_len = ((sample_rate as f32 * LEVEL_WINDOW_SECS) as usize)
            .max(2)
            .next_power_of_two();
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(fft_len);
        let bin_hz = sample_rate as f32 / fft_len as f32;
        let bin_count = fft_len / 2 + 1;
        let band_bins = LEVEL_BANDS_HZ.map(|(low, high)| {
            let start = ((low / bin_hz).ceil() as usize).min(bin_count);
            let end = ((high / bin_hz).ceil() as usize).min(bin_count);
            (start, end)
        });

        Self {
            window: (0..fft_len).map(|i| hann(i, fft_len)).collect(),
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            band_bins,
        }
    }

    /// Number of most recent samples [`Self::analyze`] looks at.
    pub fn window_len(&self) -> usize {
        self.window.len()
    }

    /// Band levels in 0..1 for the most recent samples (silence -> 0).
    pub fn analyze(&mut self, samples: &[f32]) -> [f32; LEVEL_BAND_COUNT] {
        let fft_len = self.window.len();
        let samples = &samples[samples.len().saturating_sub(fft_len)..];
        let used = samples.len();
        if used < 2 {
            return [0.0; LEVEL_BAND_COUNT];
        }

        // Short input (recording just started): Hann over what we have, zero-padded.
        let mut window_energy = 0.0f32;
        for (i, slot) in self.input.iter_mut().enumerate() {
            *slot = match samples.get(i) {
                Some(&sample) if sample.is_finite() => {
                    let weight = if used == fft_len {
                        self.window[i]
                    } else {
                        hann(i, used)
                    };
                    window_energy += weight * weight;
                    sample * weight
                }
                _ => 0.0,
            };
        }
        if window_energy <= f32::EPSILON
            || self
                .fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
                .is_err()
        {
            return [0.0; LEVEL_BAND_COUNT];
        }

        // One-sided power normalized to the signal's mean square (sine amplitude A -> A^2/2).
        let scale = 2.0 / (fft_len as f32 * window_energy);
        let mut levels = [0.0; LEVEL_BAND_COUNT];
        for (band, level) in levels.iter_mut().enumerate() {
            let (start, end) = self.band_bins[band];
            let power: f32 = self.spectrum[start..end]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum::<f32>()
                * scale;
            let db = 10.0 * (power + 1e-12).log10() + LEVEL_TILT_DB[band];
            let value = (db - LEVEL_NOISE_FLOOR_DB) / LEVEL_RANGE_DB;
            *level = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        levels
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_wave(sample_rate: u32, sample_count: usize, frequency: f32) -> Vec<f32> {
        (0..sample_count)
            .map(|index| {
                let time = index as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * frequency * time).sin()
            })
            .collect()
    }

    fn assert_length_near(actual: usize, expected: usize) {
        let tolerance = (expected as f64 * 0.02).ceil() as usize;
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "expected output length near {expected}, got {actual}"
        );
    }

    fn scaled_sine(sample_rate: u32, frequency: f32, amplitude: f32) -> Vec<f32> {
        sine_wave(sample_rate, sample_rate as usize / 10, frequency)
            .into_iter()
            .map(|sample| sample * amplitude)
            .collect()
    }

    fn assert_levels_valid(levels: &[f32; LEVEL_BAND_COUNT]) {
        assert!(
            levels
                .iter()
                .all(|level| level.is_finite() && (0.0..=1.0).contains(level)),
            "levels out of range: {levels:?}"
        );
    }

    const LEVEL_TEST_RATES: [u32; 3] = [16_000, 44_100, 48_000];

    #[test]
    fn levels_gate_quiet_room_noise() {
        // Deterministic white noise at roughly -66 dBFS RMS.
        let mut seed = 0x1234_5678u32;
        let noise: Vec<f32> = (0..48_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.0017
            })
            .collect();
        for rate in LEVEL_TEST_RATES {
            let levels = LevelAnalyzer::new(rate).analyze(&noise);
            assert!(
                levels.iter().all(|level| *level < 0.05),
                "noise should be gated at {rate} Hz: {levels:?}"
            );
        }
    }

    #[test]
    fn sine_lands_in_matching_band() {
        for rate in LEVEL_TEST_RATES {
            let mut analyzer = LevelAnalyzer::new(rate);
            for (frequency, band) in [(150.0, 0), (800.0, 1), (2_500.0, 2)] {
                let levels = analyzer.analyze(&scaled_sine(rate, frequency, 0.05));
                assert_levels_valid(&levels);
                assert!(
                    levels[band] > 0.5,
                    "{frequency} Hz at {rate} Hz should drive band {band}: {levels:?}"
                );
                for (other, level) in levels.iter().enumerate() {
                    if other != band {
                        assert!(
                            *level < 0.1,
                            "{frequency} Hz at {rate} Hz leaked into band {other}: {levels:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn levels_handle_silent_empty_loud_and_invalid_input() {
        for rate in LEVEL_TEST_RATES {
            let mut analyzer = LevelAnalyzer::new(rate);
            assert_eq!(analyzer.analyze(&[]), [0.0; LEVEL_BAND_COUNT]);
            assert_eq!(analyzer.analyze(&[0.5]), [0.0; LEVEL_BAND_COUNT]);
            assert_eq!(
                analyzer.analyze(&vec![0.0; rate as usize / 10]),
                [0.0; LEVEL_BAND_COUNT]
            );

            let loud = scaled_sine(rate, 300.0, 1.0);
            assert_levels_valid(&analyzer.analyze(&loud));
            assert_levels_valid(&analyzer.analyze(&loud[..64]));
            assert_levels_valid(&analyzer.analyze(&[f32::NAN, f32::INFINITY, 1.0, -1.0]));
        }
    }

    #[test]
    fn short_input_still_reports_level() {
        let rate = 48_000;
        let sine = scaled_sine(rate, 800.0, 0.05);
        let levels = LevelAnalyzer::new(rate).analyze(&sine[..rate as usize / 100]);
        assert!(levels[1] > 0.5, "10 ms of 800 Hz: {levels:?}");
    }

    #[test]
    fn tap_is_live_only_for_its_own_recording() {
        let mut capture = AudioCapture::new();
        assert!(!capture.tap().is_live());

        capture.begin_recording();
        let first = capture.tap();
        assert!(first.is_live());
        drop(capture.end_recording());
        assert!(!first.is_live());

        capture.begin_recording();
        assert!(!first.is_live());
        assert!(capture.tap().is_live());
        capture.close_capture();
        assert!(!capture.tap().is_live());
    }

    #[test]
    fn simple_resample_fallback_downsamples_and_handles_short_input() {
        let output = AudioCapture::simple_resample(&[0.5; 4_800], 48_000, 16_000);
        assert_eq!(output.len(), 1_600);
        assert!(output.iter().all(|sample| (*sample - 0.5).abs() < 1e-6));

        assert!(AudioCapture::simple_resample(&[], 48_000, 16_000).is_empty());
        assert!(AudioCapture::simple_resample(&[0.5], 48_000, 16_000).is_empty());
    }

    #[test]
    fn resample_to_16k_passes_native_rate_and_empty_input_through() {
        let mut capture = AudioCapture::new();
        let input = vec![-0.25, 0.0, 0.5, 1.0];
        let input_ptr = input.as_ptr();
        let output = capture.resample_to_16k(input);
        assert_eq!(output, vec![-0.25, 0.0, 0.5, 1.0]);
        assert_eq!(output.as_ptr(), input_ptr);

        capture.native_sample_rate = 48_000;
        assert!(capture.resample_to_16k(Vec::new()).is_empty());
    }

    #[test]
    fn resample_to_16k_downsamples_common_rates() {
        for rate in [48_000, 44_100] {
            let mut capture = AudioCapture::new();
            capture.native_sample_rate = rate;
            let output = capture.resample_to_16k(sine_wave(rate, rate as usize, 440.0));

            assert_length_near(output.len(), 16_000);
            assert!(
                output
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.1),
                "resampled output out of range at {rate} Hz"
            );
        }
    }
}
