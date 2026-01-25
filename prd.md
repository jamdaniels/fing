# Fing - Product Requirements Document

> **Purpose:** This document contains all specifications needed to build the Fing speech-to-text application. It is intended for use by coding agents/developers.

## Project Overview

A lightweight, fast, cross-platform speech-to-text desktop application that lives in the system tray/menu bar. Users hold a hotkey, speak, release the hotkey, and their transcribed text is instantly pasted into the active application.

### Core Principles

- **Speed is paramount**: Transcription should feel instant (~200-400ms)
- **Clean, focused UI**: Intuitive interface with sidebar navigation, no clutter
- **Privacy-first**: All processing happens locally, no cloud services, mic only active during recording
- **Cross-platform**: macOS + Windows in V1 (Linux planned)

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Frontend | Bun + TypeScript + HTML/CSS (vanilla) | Clean UI, no framework |
| Icons | Lucide | Consistent iconography |
| App Framework | Tauri v2 | Native wrapper, Rust backend |
| Inference | whisper-rs (whisper.cpp bindings) | Speech-to-text |
| Model | ggml-tiny.en.bin (~75MB) | Fast English transcription (downloaded by user; not bundled) |
| Audio | cpal | Audio capture (activated on demand) |
| Resampling | rubato | High-quality async resampling to 16kHz |
| Storage | rusqlite (SQLite) | Transcript history |
| Clipboard | arboard | Clipboard set (paste uses OS APIs) |
| Hotkeys | Native OS hooks | Hold-to-record key down/up (macOS event tap; Windows LL hook) |
| Async Runtime | tokio | Non-blocking operations |

---

## V1 Scope

**Supported platforms:** macOS, Windows

**Explicitly out of scope for V1:**
- Linux (Wayland/X11)
- Mobile
- Toggle activation mode (hold-only in V1)
- Silence trimming / VAD

**V1 operating assumptions:**
- The app is tray-first (main window is optional for daily usage)
- Primary workflow is global hotkey (hold) → dictate → release → paste
- Model is not bundled; user must download or choose a local file
- GPU acceleration via Metal (macOS) or Vulkan (Windows)
- Microphone is only active while recording (privacy-first)

---

## Application States

The application has distinct global states that govern what functionality is available:

```rust
pub enum AppState {
    /// No valid model file found. App opens to onboarding/setup.
    /// Hotkey is NOT registered. Tray shows "Setup required".
    NeedsSetup,
    
    /// Model is being loaded/verified at startup.
    /// Hotkey is NOT registered. Brief loading state.
    Initializing,
    
    /// App is ready. Model loaded, hotkey registered.
    /// Normal tray menu available.
    Ready,
    
    /// User is holding hotkey, audio is being captured.
    Recording,
    
    /// Audio captured, transcription in progress.
    Processing,
}
```

**State Transitions:**
```
App Launch
    │
    ▼
[Check Model] ──(missing/invalid)──▶ NeedsSetup ──(model downloaded)──┐
    │                                                                  │
    │ (valid)                                                          │
    ▼                                                                  │
Initializing ◀─────────────────────────────────────────────────────────┘
    │
    │ (model loaded, hotkey registered)
    ▼
  Ready ◀──────────────────────────────────────────────────────────────┐
    │                                                                   │
    │ (hotkey down)                                                     │
    ▼                                                                   │
Recording                                                               │
    │                                                                   │
    │ (hotkey up)                                                       │
    ▼                                                                   │
Processing ─────(complete)──────────────────────────────────────────────┘
```

**Critical Rule:** Hotkey is only registered when state is `Ready`. This prevents accidental recordings during setup or while processing.

---

## Architecture Diagram

**Backend runtime model (V1):**
- Microphone activated on-demand when hotkey is pressed (privacy-first: no always-on mic)
- One transcription worker thread with the Whisper model loaded once at startup
- One DB writer task/thread to keep disk I/O off the hot path
- OS-specific global key down/up hooks (hold) and OS-specific paste injection

```
┌───────────────────────────────────────────────────────────────────────────────┐
│                                 FING APP                                      │
├───────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────────────────┐  │
│  │   MAIN WINDOW (800x600) - WebView                                       │  │
│  │  ┌────────────┬────────────────────────────────────────────────────────┐│  │
│  │  │            │                                                        ││  │
│  │  │  SIDEBAR   │              CONTENT AREA                              ││  │
│  │  │            │                                                        ││  │
│  │  │  ┌──────┐  │  ┌──────────────────────────────────────────────────┐  ││  │
│  │  │  │ Home │  │  │                                                  │  ││  │
│  │  │  └──────┘  │  │   Content changes based on sidebar selection     │  ││  │
│  │  │  ┌──────┐  │  │                                                  │  ││  │
│  │  │  │Histo-│  │  │   - Onboarding: First-run setup wizard           │  ││  │
│  │  │  │ry    │  │  │   - Home: Stats (words transcribed, etc.)        │  ││  │
│  │  │  └──────┘  │  │   - History: Searchable transcript list          │  ││  │
│  │  │  ┌──────┐  │  │   - Settings: Hotkey, mic, model, autostart      │  ││  │
│  │  │  │Setti-│  │  │   - About: Version, commit, links                │  ││  │
│  │  │  │ngs   │  │  │                                                  │  ││  │
│  │  │  └──────┘  │  │                                                  │  ││  │
│  │  │  ┌──────┐  │  │                                                  │  ││  │
│  │  │  │About │  │  └──────────────────────────────────────────────────┘  ││  │
│  │  │  └──────┘  │                                                        ││  │
│  │  │  ┌──────┐  │                                                        ││  │
│  │  │  │ Quit │  │                                                        ││  │
│  │  │  └──────┘  │                                                        ││  │
│  │  └────────────┴────────────────────────────────────────────────────────┘│  │
│  └─────────────────────────────────────────────────────────────────────────┘  │
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────────────────┐  │
│  │   FLOATING INDICATOR (~70x30) - Overlay Window                          │  │
│  │   Bottom center of screen, always on top, shows during recording only   │  │
│  │                          ┌─────────────┐                                │  │
│  │                          │ ● Rec       │                                │  │
│  │                          └─────────────┘                                │  │
│  └─────────────────────────────────────────────────────────────────────────┘  │
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────────────────┐  │
│  │   SYSTEM TRAY                                                           │  │
│  │   Menu: Open App | Select Mic ▶ | Check for Updates | About | Quit     │  │
│  └─────────────────────────────────────────────────────────────────────────┘  │
│                                     │                                         │
│                                     │ IPC (invoke)                            │
│                                     ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────────┐  │
│  │   RUST BACKEND                                                          │  │
│  │                                                                         │  │
│  │   ┌──────────────────────────────────────────────────────────────────┐  │  │
│  │   │                    Core Pipeline                                 │  │  │
│  │   │                                                                  │  │  │
│  │   │   [Hotkey Down]                                                  │  │  │
│  │   │        │                                                         │  │  │
│  │   │        ▼                                                         │  │  │
│  │   │   [Init Mic + Start Capture] ──▶ [Buffer PCM samples]            │  │  │
│  │   │        │                              │                          │  │  │
│  │   │        ▼                              │                          │  │  │
│  │   │   [Show Indicator]                    │                          │  │  │
│  │   │                                       │                          │  │  │
│  │   │   [Hotkey Up]                         │                          │  │  │
│  │   │        │                              │                          │  │  │
│  │   │        ▼                              ▼                          │  │  │
│  │   │   [Stop Capture + Close Mic] ◀── [Flush Buffer]                  │  │  │
│  │   │        │                                                         │  │  │
│  │   │        ▼                                                         │  │  │
│  │   │   [Resample to 16kHz mono f32]                                   │  │  │
│  │   │        │                                                         │  │  │
│  │   │        ▼                                                         │  │  │
│  │   │   [Whisper Inference] ──▶ [Text]                                 │  │  │
│  │   │        │                     │                                   │  │  │
│  │   │        │                     ├──▶ [Clipboard + Paste]            │  │  │
│  │   │        │                     │         │                         │  │  │
│  │   │        │                     │         ▼                         │  │  │
│  │   │        │                     │    [Notify if paste failed]       │  │  │
│  │   │        │                     │                                   │  │  │
│  │   │        │                     └──▶ [Save to SQLite] (async)       │  │  │
│  │   │        │                                                         │  │  │
│  │   │        ▼                                                         │  │  │
│  │   │   [Hide Indicator]                                               │  │  │
│  │   │                                                                  │  │  │
│  │   └──────────────────────────────────────────────────────────────────┘  │  │
│  │                                                                         │  │
│  │   ┌──────────────────────────────────────────────────────────────────┐  │  │
│  │   │   SQLite (dedicated writer; never blocks paste)                  │  │  │
│  │   │   transcripts.db                                                 │  │  │
│  │   └──────────────────────────────────────────────────────────────────┘  │  │
│  └─────────────────────────────────────────────────────────────────────────┘  │
│                                                                               │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

## Data Flow

```
Time ──────────────────────────────────────────────────────────────────▶

User:     [PRESS]  [====== HOLDING HOTKEY ======]  [RELEASE]
              │                                         │
              ▼                                         ▼
Audio:    [init mic]──▶[capture][capture][capture]──▶[stop + close mic]
              │            │       │       │              │
              ▼            ▼       ▼       ▼              ▼
Indicator: [SHOW: 🎤 Recording]                    [SHOW: ◐ Processing]
                                                        │
Buffer:   [====PCM SAMPLES ACCUMULATING====]            │
                                                        ▼
Resample:                                         [Convert to 16kHz]
                                                        │
                                                        ▼
Whisper:                                          [TRANSCRIBE]
                                                  (~150-250ms GPU)
                                                  (~400-800ms CPU)
                                                        │
                                                        ▼
Output:                                           [TEXT READY]
                                                        │
                       ┌────────────────────────────────┼────────────────────┐
                       ▼                                ▼                    ▼
                [SET CLIPBOARD]                 [SAVE TO SQLITE]     [HIDE INDICATOR]
                       │                            (async, if
                       ▼                          history enabled)
                [PASTE (best-effort)]                        
                       │                         
                       ▼                         
                (if failed)                    
                       │                       
                       ▼                       
                [NOTIFY: "Text copied         
                 to clipboard"]               

Note: Microphone is ONLY active while hotkey is held. No always-on mic.
Paste is best-effort: clipboard is always set; if paste injection fails, 
user sees notification and can paste manually.
```

---

## File Structure

```
fing/
├── src/                          # Frontend
│   ├── index.html                # Main window HTML shell
│   ├── indicator.html            # Floating recording indicator
│   ├── main.ts                   # Main window entry point
│   ├── indicator.ts              # Indicator window logic
│   ├── styles.css                # Main window styles
│   ├── indicator.css             # Indicator styles (animations)
│   ├── components/
│   │   ├── sidebar.ts            # Sidebar navigation
│   │   ├── onboarding.ts         # First-run setup wizard
│   │   ├── home.ts               # Home/Dashboard view
│   │   ├── history.ts            # History list view
│   │   ├── settings.ts           # Settings view
│   │   └── about.ts              # About view
│   └── lib/
│       ├── ipc.ts                # Typed Tauri invoke wrappers
│       └── types.ts              # Shared TypeScript types
│
├── src-tauri/                    # Backend
│   ├── Cargo.toml                # Rust dependencies
│   ├── tauri.conf.json           # Tauri configuration
│   ├── build.rs                  # Build script (whisper-rs + vergen)
│   ├── icons/                    # App and tray icons
│   │   ├── tray-16.png
│   │   ├── tray-16@2x.png
│   │   ├── tray-32.png
│   │   ├── tray.ico              # Windows
│   │   └── tray.png              # Fallback
│   ├── sounds/
│   │   ├── recording-start.wav   # Played when recording begins
│   │   └── recording-done.wav    # Played when transcription completes
│   └── src/
│       ├── main.rs               # Tauri setup, tray, window management
│       ├── state.rs              # AppState management
│       ├── hotkey.rs             # Hold state machine + OS backends
│       ├── platform/             # OS-specific hotkey/paste
│       │   ├── mod.rs
│       │   ├── macos.rs
│       │   └── windows.rs
│       ├── audio.rs              # cpal on-demand capture + resample
│       ├── transcribe.rs         # whisper-rs inference wrapper
│       ├── paste.rs              # Clipboard set + best-effort paste
│       ├── db.rs                 # SQLite operations
│       ├── engine.rs             # Trait for swappable transcription engines
│       ├── model.rs              # Model download, verification, management
│       ├── updates.rs            # GitHub release update checker
│       ├── app_info.rs           # Version, commit, build info
│       ├── indicator.rs          # Floating indicator window control
│       ├── notifications.rs      # System notifications (paste fallback, errors)
│       └── stats.rs              # Transcript statistics for Home dashboard
│
├── package.json                  # Bun/npm dependencies
├── bunfig.toml                   # Bun configuration
├── tsconfig.json                 # TypeScript configuration
└── README.md
```

---

## Platform-Specific GPU Acceleration

```
┌─────────────────────────────────────────────────────────────────┐
│                    whisper-rs (whisper.cpp)                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   macOS      ──▶  Metal acceleration                            │
│                   Feature flag: "metal"                         │
│                   Works on: Apple Silicon, Intel Macs with      │
│                             Metal-capable GPU                   │
│                   ~150-200ms inference                          │
│                                                                 │
│   Windows    ──▶  Vulkan acceleration                           │
│                   Feature flag: "vulkan"                        │
│                   Works on: NVIDIA, AMD, Intel GPUs             │
│                   (including integrated graphics)               │
│                   ~150-250ms inference                          │
│                                                                 │
│                   Fallback: CPU                                 │
│                   ~400-800ms inference                          │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Why Vulkan for Windows:**
- **Universal GPU support**: Works with NVIDIA, AMD, and Intel GPUs
- **Integrated graphics**: Laptops with Intel/AMD integrated GPUs benefit (~12x speedup over CPU)
- **No user requirements**: Vulkan drivers come pre-installed with standard GPU drivers
- **Single build**: One Windows binary works for all users

---

## Rust Dependencies (Cargo.toml)

```toml
[package]
name = "fing"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
whisper-rs = "0.15"
cpal = "0.15"
rubato = "0.14"                    # High-quality resampling
rusqlite = { version = "0.31", features = ["bundled"] }
arboard = "3"
tokio = { version = "1", features = ["rt-multi-thread", "sync", "fs"] }
once_cell = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json", "stream"] }  # Updates + model download
sha2 = "0.10"                      # Model verification
dirs = "5"                         # OS app data directories

[build-dependencies]
vergen = { version = "8", features = ["git", "gitcl"] }

# macOS: Metal for Apple GPUs
[target.'cfg(target_os = "macos")'.dependencies]
whisper-rs = { version = "0.15", features = ["metal"] }

# Windows: Vulkan for cross-vendor GPU support (NVIDIA, AMD, Intel)
[target.'cfg(target_os = "windows")'.dependencies]
whisper-rs = { version = "0.15", features = ["vulkan"] }
```

---

## Frontend Dependencies (package.json)

```json
{
  "name": "fing",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "bunx tauri dev",
    "build": "bunx tauri build",
    "frontend:dev": "bunx --bun vite",
    "frontend:build": "bunx --bun vite build",
    "typecheck": "tsc --noEmit"
  },
  "dependencies": {
    "lucide": "^0.460"
  },
  "devDependencies": {
    "@tauri-apps/api": "^2",
    "@tauri-apps/cli": "^2",
    "typescript": "^5"
  }
}
```

---

## Backend Module Specifications

### 1. main.rs - Application Entry Point

**Responsibilities:**
- Initialize Tauri application
- Check for valid model on startup → set initial AppState
- Set up system tray with icon and menu
- Register IPC commands
- Manage main window visibility (show/hide via tray)
- Manage indicator window (show/hide during recording/processing)
- Initialize transcriber when model is valid
- Handle navigation to specific sidebar tabs from tray

**Key IPC Commands to expose:**
- `get_app_state() -> AppState`
- `get_recent_transcripts(limit: u32) -> Vec<Transcript>`
- `delete_transcript(id: i64) -> Result<()>`
- `search_transcripts(query: String) -> Vec<Transcript>`
- `get_stats() -> Stats`
- `get_settings() -> Settings`
- `update_settings(settings: Settings) -> Result<()>`
- `get_audio_devices() -> Vec<AudioDevice>`
- `set_audio_device(device_id: String) -> Result<()>`
- `check_for_updates() -> UpdateInfo`
- `get_app_info() -> AppInfo`
- `open_main_window(tab: Option<String>) -> Result<()>`
- `quit_app() -> Result<()>`
- `start_model_download() -> Result<()>`
- `get_download_progress() -> DownloadProgress`
- `select_model_file() -> Result<String>`
- `verify_model(path: String) -> Result<ModelVerification>`
- `complete_setup() -> Result<()>`
- `test_microphone() -> Result<MicrophoneTest>`
- `request_permissions() -> PermissionStatus`

**Tray Menu Actions:**
- "Open App" → Show main window, navigate to Home (or Onboarding if NeedsSetup)
- "Select Mic" → Submenu with device list, checkmark on selected
- "Check for Updates" → Call update checker, show notification if available
- "About" → Show main window, navigate to About tab
- "Quit" → Clean shutdown

**Tray Menu in NeedsSetup State:**
```
┌─────────────────────────────┐
│  Complete Setup...          │  ← Opens onboarding
├─────────────────────────────┤
│  Quit                       │
└─────────────────────────────┘
```

---

### 2. state.rs - Application State Management

**Responsibilities:**
- Maintain global AppState
- Provide thread-safe state transitions
- Emit state change events to frontend

**Key Structs:**
```rust
use std::sync::RwLock;
use once_cell::sync::Lazy;

pub static APP_STATE: Lazy<RwLock<AppState>> = Lazy::new(|| RwLock::new(AppState::NeedsSetup));

pub enum AppState {
    NeedsSetup,
    Initializing,
    Ready,
    Recording,
    Processing,
}

impl AppState {
    pub fn can_record(&self) -> bool {
        matches!(self, AppState::Ready)
    }
    
    pub fn is_busy(&self) -> bool {
        matches!(self, AppState::Recording | AppState::Processing)
    }
}

pub fn transition_to(new_state: AppState) -> Result<(), StateError> {
    // Validate transition is allowed
    // Update state
    // Emit event to frontend
}
```

---

### 3. hotkey.rs - Global Shortcut Handling

**Responsibilities:**
- Implement the recording state machine: `Ready → Recording → Processing → Ready`
- Support hold activation mode only (V1)
- Receive global key events from OS-specific backends
- Drive the pipeline: mic init → indicator → audio capture → transcribe → paste
- Ignore new triggers while `Processing` (speed + predictability)
- Handle hotkey registration failures gracefully

**Default Hotkey:**
- macOS: `F8`
- Windows: `F8`

**Key Behavior (Hold Mode):**
```
State: Ready | Recording | Processing

on_key_down:
  if state != Ready: return
  
  // Initialize microphone (cold start)
  audio::init_capture()
  
  // Show indicator
  indicator::show_recording()
  
  // Start capturing audio
  audio::begin_recording()
  
  state = Recording

on_key_up:
  if state != Recording: return
  
  // Show processing state
  indicator::show_processing()
  
  // Stop capture and get audio buffer
  audio_buffer = audio::end_recording()
  
  // Close microphone (privacy: mic indicator goes away)
  audio::close_capture()
  
  state = Processing
  
  // Resample to 16kHz mono
  audio_16k = audio::resample_to_16k(audio_buffer)
  
  // Transcribe
  text = transcribe::run(audio_16k)
  
  // Output
  paste_result = paste::set_clipboard_and_paste(text)
  if paste_result.should_notify():
      notifications::show("Text copied to clipboard")
  
  // Save to history (async, if enabled)
  if settings.history_enabled:
      db::save_transcript(text)  // don't wait
  
  // Done
  indicator::hide()
  state = Ready
```

**Hotkey Registration:**
```rust
pub enum HotkeyRegistrationResult {
    Success,
    ConflictWithSystem(String),      // e.g., "F8 is used by Spotlight"
    ConflictWithOtherApp(String),    // e.g., "F8 is registered by AppName"
    PermissionDenied,                // macOS Accessibility
    Unknown(String),
}

pub fn register_hotkey(key: &str) -> HotkeyRegistrationResult;
pub fn unregister_hotkey() -> Result<()>;
```

**macOS note:** Hold-to-record and programmatic paste require Accessibility permission.

---

### 4. audio.rs - Audio Capture (On-Demand)

**Responsibilities:**
- Enumerate available input devices (microphones)
- Allow user to select preferred microphone
- Initialize microphone on-demand when recording starts (cold start)
- Close microphone when recording ends (privacy-first)
- Capture at device-native format/rate
- Convert and resample to Whisper input format (16kHz mono f32)
- Accumulate samples in a bounded buffer while recording
- Return audio buffer on recording end

**Privacy Model:**
- Microphone is ONLY initialized when hotkey is pressed
- Microphone is closed immediately when hotkey is released
- OS microphone indicator only shows during active recording
- No background mic access

**Recording Limits:**
```rust
/// Maximum recording duration in seconds
pub const MAX_RECORDING_DURATION_SECS: u32 = 120;  // 2 minutes

/// Sample rate for Whisper input
pub const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Maximum buffer size (2 minutes of 16kHz mono f32 samples ≈ 7.7MB)
pub const MAX_BUFFER_SIZE: usize = (MAX_RECORDING_DURATION_SECS as usize) * (WHISPER_SAMPLE_RATE as usize);
```

**Whisper Input Requirements:**
- Sample rate: 16000 Hz
- Channels: 1 (mono)
- Format: f32 PCM samples

**Key Structs:**
```rust
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub struct AudioCapture {
    selected_device_id: Option<String>,
    stream: Option<cpal::Stream>,  // Only Some while recording
    buffer: Vec<f32>,
    native_sample_rate: u32,
}

impl AudioCapture {
    /// List available input devices
    pub fn list_devices() -> Vec<AudioDevice>;
    
    /// Set preferred device (None = system default)
    pub fn set_device(&mut self, device_id: Option<String>);
    
    /// Initialize microphone and prepare for capture (cold start)
    /// Called on hotkey down
    pub fn init_capture(&mut self) -> Result<(), AudioError>;
    
    /// Start recording audio into buffer
    pub fn begin_recording(&mut self);
    
    /// Stop recording and return raw audio buffer
    pub fn end_recording(&mut self) -> Vec<f32>;
    
    /// Close microphone (releases device, mic indicator goes away)
    /// Called on hotkey up after getting buffer
    pub fn close_capture(&mut self);
    
    /// Get current recording duration
    pub fn get_recording_duration(&self) -> Duration;
    
    /// Resample buffer from native rate to 16kHz mono
    pub fn resample_to_16k(&self, buffer: Vec<f32>) -> Vec<f32>;
    
    /// Test microphone (for onboarding)
    pub fn test_microphone(&mut self) -> Result<MicrophoneTest, AudioError>;
}

pub struct MicrophoneTest {
    pub device_name: String,
    pub peak_level: f32,       // 0.0 - 1.0
    pub is_receiving_audio: bool,
}

pub enum AudioError {
    NoDevicesFound,
    DeviceNotFound(String),
    DeviceInitFailed(String),
    PermissionDenied,
    StreamError(String),
}
```

**Timing Expectations:**
- `init_capture()`: ~50-150ms (device initialization)
- `begin_recording()`: ~1ms
- `end_recording()`: ~1ms
- `close_capture()`: ~5ms
- `resample_to_16k()`: ~5-15ms (depending on duration)

---

### 5. transcribe.rs - Whisper Inference

**Responsibilities:**
- Load Whisper model once at startup
- Run inference on audio buffer
- Return transcribed text
- Handle errors gracefully
- Report active backend (Metal/Vulkan/CPU)

**Key Configuration:**
- Model: ggml-tiny.en.bin (English-only, fastest)
- Sampling strategy: Greedy with best_of = 1
- Disable all printing/logging for speed
- Language: English (hardcoded for tiny.en model)

**Key Structs:**
```rust
pub enum InferenceBackend {
    Metal,      // macOS
    Vulkan,     // Windows (NVIDIA, AMD, Intel)
    Cpu,        // Fallback
}

pub struct Transcriber {
    ctx: WhisperContext,
    backend: InferenceBackend,
}

impl Transcriber {
    pub fn new(model_path: &str) -> Result<Self, TranscribeError>;
    pub fn transcribe(&self, audio: &[f32]) -> Result<String, TranscribeError>;
    pub fn backend(&self) -> InferenceBackend;
}

pub enum TranscribeError {
    ModelNotFound,
    ModelInvalid,
    ModelLoadFailed(String),
    InferenceFailed(String),
    EmptyAudio,
}
```

---

### 6. paste.rs - Paste to Active Window

**Responsibilities:**
- Set clipboard content to transcribed text
- Attempt to paste into the active application (macOS: Cmd+V, Windows: Ctrl+V)
- Return result indicating if paste succeeded or only clipboard was set
- Best-effort approach: always set clipboard, paste may fail silently

**Key Structs:**
```rust
pub enum PasteResult {
    /// Paste command was sent successfully
    Success,
    
    /// Clipboard set, but paste failed - app is in secure input mode
    /// (e.g., password fields, some Terminal apps)
    ClipboardOnlySecureInput,
    
    /// Clipboard set, but paste failed - target window is elevated (Windows UAC)
    ClipboardOnlyElevated,
    
    /// Clipboard set, paste was sent but no confirmation it worked
    /// (some apps ignore synthetic input)
    ClipboardOnlySent,
}

impl PasteResult {
    pub fn should_notify(&self) -> bool {
        !matches!(self, PasteResult::Success)
    }
    
    pub fn notification_message(&self) -> &'static str {
        "Text copied to clipboard"
    }
}

pub fn set_clipboard_and_paste(text: &str) -> PasteResult;
```

**Platform Notes:**

*macOS:*
- Some apps (Terminal, certain Electron apps) don't respond to synthetic Cmd+V
- Secure input fields block event injection entirely
- Requires Accessibility permission

*Windows:*
- UAC-elevated windows reject SendInput from non-elevated processes
- Some apps use raw input and ignore SendInput
- No special permissions needed for basic operation

---

### 7. db.rs - SQLite Storage

**Responsibilities:**
- Initialize database and create tables on first run
- Save transcripts asynchronously (don't block paste)
- Respect `history_enabled` setting (don't save if disabled)
- Query transcripts for history display
- Full-text search support with automatic FTS sync
- Delete transcripts
- Prune old transcripts based on history limit setting

**Schema:**
```sql
CREATE TABLE IF NOT EXISTS transcripts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    text         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,  -- Unix timestamp (milliseconds)
    duration_ms  INTEGER,           -- Recording duration
    app_context  TEXT,              -- Active app when dictated (optional)
    word_count   INTEGER
);

CREATE INDEX IF NOT EXISTS idx_created ON transcripts(created_at DESC);

-- Full-text search
CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
    text,
    content='transcripts',
    content_rowid='id'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER IF NOT EXISTS transcripts_ai AFTER INSERT ON transcripts BEGIN
    INSERT INTO transcripts_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS transcripts_ad AFTER DELETE ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, text) VALUES('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS transcripts_au AFTER UPDATE ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, text) VALUES('delete', old.id, old.text);
    INSERT INTO transcripts_fts(rowid, text) VALUES (new.id, new.text);
END;
```

**Key Functions:**
```rust
pub async fn init_db() -> Result<Connection>;

/// Save transcript. Respects history_enabled setting.
pub async fn save_transcript(text: &str, duration_ms: Option<i64>, settings: &Settings) -> Result<Option<i64>> {
    if !settings.history_enabled {
        return Ok(None);  // Don't save
    }
    // ... save and return id
}

pub async fn get_recent(limit: u32) -> Result<Vec<Transcript>>;
pub async fn search(query: &str) -> Result<Vec<Transcript>>;
pub async fn delete(id: i64) -> Result<()>;
pub async fn delete_all() -> Result<u64>;  // Clear all history
pub async fn get_stats() -> Result<Stats>;

/// Prune transcripts exceeding the limit. Returns number deleted.
/// If limit is -1 (unlimited), does nothing.
pub async fn prune_history(limit: i64) -> Result<u64>;
```

**Pruning Strategy:**
- Run `prune_history()` after each successful transcript save
- Also run on app startup to catch any missed cleanup

---

### 8. model.rs - Model Management

**Responsibilities:**
- Check if model exists and is valid
- Download model from Hugging Face with progress reporting
- Verify model integrity via SHA256 hash (REQUIRED)
- Allow user to select local model file
- Manage model path in settings

**Key Constants:**
```rust
pub const MODEL_FILENAME: &str = "ggml-tiny.en.bin";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin";
pub const MODEL_SIZE_BYTES: u64 = 77_691_713;  // ~75MB
pub const MODEL_SHA256: &str = "...";  // Actual hash to be verified
```

**Key Structs:**
```rust
pub struct ModelVerification {
    pub path: String,
    pub exists: bool,
    pub size_valid: bool,
    pub hash_valid: bool,
    pub is_valid: bool,  // All checks passed
}

pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    pub status: DownloadStatus,
}

pub enum DownloadStatus {
    NotStarted,
    Downloading,
    Verifying,
    Complete,
    Failed(String),
}

impl ModelManager {
    /// Get the default model path in app data directory
    pub fn default_model_path() -> PathBuf;
    
    /// Verify a model file (REQUIRED before loading)
    pub async fn verify(path: &Path) -> Result<ModelVerification>;
    
    /// Download model to default location with progress callback
    pub async fn download(progress_callback: impl Fn(DownloadProgress)) -> Result<PathBuf>;
    
    /// Open file picker and verify selected file
    pub async fn select_file() -> Result<Option<PathBuf>>;
}
```

**Model Location:**
- Default path: `{app_data_dir}/models/ggml-tiny.en.bin`
  - macOS: `~/Library/Application Support/com.jamdaniels.fing/models/`
  - Windows: `%APPDATA%\Fing\models\`
- User can override via Settings or by selecting a file during onboarding

**Verification (REQUIRED):**
1. File exists
2. File size matches expected (~75MB ± 1MB tolerance)
3. SHA256 hash matches known good value

If any verification step fails, refuse to load model and show clear error.

---

### 9. engine.rs - Swappable Transcription Engine (Future-Proofing)

**Purpose:** Allow switching between Whisper and Moonshine (or other engines) later.

**Trait Definition:**
```rust
pub trait TranscriptionEngine: Send + Sync {
    fn transcribe(&self, audio: &[f32]) -> Result<String>;
    fn backend_name(&self) -> &str;
}

pub struct WhisperEngine { /* ... */ }
impl TranscriptionEngine for WhisperEngine { /* ... */ }

// Future: Add MoonshineEngine
// pub struct MoonshineEngine { /* ... */ }
// impl TranscriptionEngine for MoonshineEngine { /* ... */ }
```

---

### 10. updates.rs - Update Checker

**Responsibilities:**
- Check GitHub releases API for newer versions
- Compare current version with latest release
- Return update availability and download URL
- Handle network errors gracefully (don't block app)

**Key Structs:**
```rust
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
}

impl UpdateChecker {
    pub async fn check() -> Result<UpdateInfo>;
}
```

**Note:** Use GitHub releases API endpoint: `https://api.github.com/repos/{owner}/{repo}/releases/latest`

---

### 11. app_info.rs - Build Information

**Build-time information to embed:**
- Version from Cargo.toml
- Git commit hash (short)
- Build timestamp

**Key Struct:**
```rust
pub struct AppInfo {
    pub name: String,           // "Fing"
    pub version: String,        // "0.1.0"
    pub commit: String,         // "a1b2c3d"
    pub build_date: String,     // "2024-01-15"
    pub repository: String,     // GitHub URL
    pub inference_backend: String,  // "Metal" / "Vulkan" / "CPU"
}
```

**Implementation:** Use `vergen` crate to embed git info at compile time via build.rs.

---

### 12. indicator.rs - Floating Recording Indicator

**Responsibilities:**
- Control the floating indicator window visibility
- Position window at bottom center of primary screen
- Manage indicator state transitions with animations
- Handle multi-monitor setups (use primary display)
- Send state updates to frontend via events

**Key Functions:**
```rust
pub fn show_recording() -> Result<()>;     // Show pill with red dot + "Rec"
pub fn show_processing() -> Result<()>;    // Switch to spinner state
pub fn hide() -> Result<()>;               // Shrink and hide indicator
pub fn position_indicator() -> Result<()>; // Position at bottom center
```

**State Machine:**
```
Hidden → Recording → Processing → Hidden
         (show)      (release)    (complete)
```

**Window Positioning Logic:**
```rust
// Get primary monitor dimensions
// Calculate center bottom position
// x = (screen_width - 70) / 2   // 70px indicator width
// y = screen_height - 30 - 50   // 30px height + 50px margin from bottom
```

**Events emitted to frontend:**
- `indicator-state-changed`: { state: "recording" | "processing" | "hidden" }

---

### 13. notifications.rs - System Notifications

**Responsibilities:**
- Show system notifications for important events
- Handle "Text copied to clipboard" fallback notification
- Show error notifications for critical failures

**Key Functions:**
```rust
/// Show notification when paste fails (clipboard still set)
pub fn show_clipboard_fallback() {
    // Show brief, non-intrusive notification
    // "Text copied to clipboard"
}

pub fn show_error(title: &str, message: &str);

pub fn show_update_available(version: &str, download_url: &str);
```

---

### 14. stats.rs - Transcript Statistics

**Responsibilities:**
- Calculate statistics from transcript database
- Provide data for Home dashboard
- Cache stats with TTL to avoid repeated DB queries
- Return zeros if history is disabled

**Key Struct:**
```rust
pub struct Stats {
    pub total_transcriptions: u64,
    pub total_words: u64,
    pub transcriptions_today: u64,
    pub words_today: u64,
    pub average_words_per_transcription: f64,
}

impl StatsService {
    pub fn get_stats() -> Result<Stats>;
    pub fn refresh() -> Result<Stats>;  // Force refresh cache
}
```

---

## Frontend Specifications

### Onboarding View (components/onboarding.ts)

First-run setup wizard that guides users through initial configuration.

**When to show:** When `AppState == NeedsSetup`

**Steps:**

```
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│  Step 1 of 4                                           [Skip Setup]     │
│                                                                         │
│                        Welcome to Fing                                  │
│                                                                         │
│              Fast, private, local speech-to-text                        │
│                                                                         │
│      Hold a key, speak, release — your words appear instantly.          │
│      All processing happens on your device. No cloud. No data sent.     │
│      Your microphone is only active while you hold the key.             │
│                                                                         │
│                                                                         │
│                         [ Get Started ]                                 │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│  Step 2 of 4                                                            │
│                                                                         │
│                      Download Speech Model                              │
│                                                                         │
│      Fing needs a speech recognition model (~75 MB, one-time download)  │
│                                                                         │
│      ┌─────────────────────────────────────────────────────────────┐    │
│      │  ████████████████████████░░░░░░░░░░  67%                    │    │
│      │  50.3 MB / 75 MB                                            │    │
│      └─────────────────────────────────────────────────────────────┘    │
│                                                                         │
│      [ Cancel ]                                                         │
│                                                                         │
│      ─────────────────────── OR ───────────────────────                 │
│                                                                         │
│      Already have the model file?  [ Choose File... ]                   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│  Step 3 of 4                                                            │
│                                                                         │
│                      Permissions (macOS only)                           │
│                                                                         │
│      Fing needs two permissions to work:                                │
│                                                                         │
│      ┌──────────────────────────────────────────────────────────────┐   │
│      │  🎤  Microphone Access                              [Granted]│   │
│      │      Required to capture your voice                          │   │
│      └──────────────────────────────────────────────────────────────┘   │
│                                                                         │
│      ┌──────────────────────────────────────────────────────────────┐   │
│      │  ⌨️  Accessibility Access                           [Grant] │   │
│      │      Required for global hotkey and auto-paste               │   │
│      │      Opens System Preferences...                             │   │
│      └──────────────────────────────────────────────────────────────┘   │
│                                                                         │
│                         [ Continue ]                                    │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│  Step 4 of 4                                                            │
│                                                                         │
│                      Test Your Microphone                               │
│                                                                         │
│      Selected: MacBook Pro Microphone                    [ Change ▼]    │
│                                                                         │
│      ┌──────────────────────────────────────────────────────────────┐   │
│      │                                                              │   │
│      │      ████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░            │   │
│      │                                                              │   │
│      │      Say something to test your microphone...                │   │
│      │                                                              │   │
│      └──────────────────────────────────────────────────────────────┘   │
│                                                                         │
│      ✓ Audio detected! Your microphone is working.                      │
│                                                                         │
│                         [ Finish Setup ]                                │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│                           You're all set!                               │
│                                                                         │
│                              ✓                                          │
│                                                                         │
│      Press and hold [ F8 ] to start recording                           │
│                                                                         │
│      Release to transcribe and paste                                    │
│                                                                         │
│      Fing will run in your menu bar. Click the icon anytime.            │
│                                                                         │
│                    [ Start Using Fing ]                                 │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

**Onboarding State:**
```typescript
interface OnboardingState {
  currentStep: 1 | 2 | 3 | 4 | 'complete';
  modelDownloadProgress: DownloadProgress | null;
  permissionStatus: {
    microphone: 'unknown' | 'granted' | 'denied';
    accessibility: 'unknown' | 'granted' | 'denied';  // macOS only
  };
  microphoneTestResult: MicrophoneTest | null;
}
```

**Skip Behavior:**
- User can skip onboarding at any step
- If model is missing, app will prompt again on next launch
- User can complete setup later via Settings

---

### Main Window (index.html)

**Size:** 800x600 pixels  
**Layout:** Sidebar (left) + Content area (right)

```
┌─────────────────────────────────────────────────────────────────────────┐
│  ┌──────────┐  ┌─────────────────────────────────────────────────────┐  │
│  │          │  │                                                     │  │
│  │   Home   │  │                  CONTENT AREA                       │  │
│  │          │  │                                                     │  │
│  │  History │  │   Renders based on selected sidebar item            │  │
│  │          │  │                                                     │  │
│  │ Settings │  │                                                     │  │
│  │          │  │                                                     │  │
│  │  About   │  │                                                     │  │
│  │          │  │                                                     │  │
│  │   Quit   │  │                                                     │  │
│  │          │  │                                                     │  │
│  └──────────┘  └─────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### Sidebar (components/sidebar.ts)

- Fixed width (~180px)
- Navigation items with Lucide icons:
  - Home (Lucide: `Home`)
  - History (Lucide: `History`)
  - Settings (Lucide: `Settings`)
  - About (Lucide: `Info`)
  - Quit (Lucide: `Power`) - at bottom, visually separated
- Active item highlighted with accent color and left border

### Home View (components/home.ts)

Dashboard with usage statistics:
- Total transcriptions (all time)
- Total words transcribed
- Transcriptions today
- Words today
- Average transcription length

**Note:** If `history_enabled` is false, show a message: "History is disabled. Enable it in Settings to see your stats."

```
┌─────────────────────────────────────────────────────────────────┐
│  Dashboard                                                      │
│                                                                 │
│  ┌─────────────────┐  ┌─────────────────┐                       │
│  │  Total          │  │  Today          │                       │
│  │  Transcriptions │  │  Transcriptions │                       │
│  │       1,234     │  │        12       │                       │
│  └─────────────────┘  └─────────────────┘                       │
│                                                                 │
│  ┌─────────────────┐  ┌─────────────────┐                       │
│  │  Total Words    │  │  Words Today    │                       │
│  │      45,678     │  │       523       │                       │
│  └─────────────────┘  └─────────────────┘                       │
│                                                                 │
│  ┌─────────────────────────────────────┐                        │
│  │  Average words per transcription    │                        │
│  │              37 words               │                        │
│  └─────────────────────────────────────┘                        │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### History View (components/history.ts)

Searchable list of past transcriptions with date grouping and pagination.

**Note:** If `history_enabled` is false, show a message: "History is disabled. Enable it in Settings to start saving transcripts."

- Search bar at top (with Lucide `Search` icon)
- Date group headers (Today, Yesterday, This Week, Earlier)
- List of transcripts with: timestamp, preview text, word count
- Click to expand full text
- Quick action icons: Copy (Lucide `Copy` icon), Delete (Lucide `Trash` icon)
- Pagination at bottom

```
┌─────────────────────────────────────────────────────────────────┐
│  History                                                        │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  🔍  Search transcripts...                              │    │
│  └─────────────────────────────────────────────────────────┘    │
│                                                                 │
│  TODAY                                                          │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  2:34 PM                                    12 words    │    │
│  │  "Hey Claude, can you help me with..."                  │    │
│  │                                         [Copy] [Trash]  │    │
│  └─────────────────────────────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  1:15 PM                                    28 words    │    │
│  │  "I need to write an email to my boss..."               │    │
│  │                                         [Copy] [Trash]  │    │
│  └─────────────────────────────────────────────────────────┘    │
│                                                                 │
│  YESTERDAY                                                      │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  5:42 PM                                     8 words    │    │
│  │  "What's the weather like tomorrow"                     │    │
│  │                                         [Copy] [Trash]  │    │
│  └─────────────────────────────────────────────────────────┘    │
│                                                                 │
│           [◀]  Page 1 of 12  [▶]                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Date Grouping Logic:**
```typescript
function getDateGroup(timestamp: number): string {
  const now = new Date();
  const date = new Date(timestamp);
  const diffDays = Math.floor((now.getTime() - date.getTime()) / (1000 * 60 * 60 * 24));
  
  if (isToday(date)) return 'Today';
  if (isYesterday(date)) return 'Yesterday';
  if (diffDays < 7) return 'This Week';
  if (diffDays < 30) return 'This Month';
  return 'Earlier';
}
```

**Lucide Icons Used:**
- `Search` (search bar)
- `Copy` (copy to clipboard action)
- `Trash` (delete action)
- `ChevronLeft` / `ChevronRight` (pagination)

**Pagination:**
- Show 20 items per page
- Display current page and total pages
- Previous/Next buttons with chevron icons

### Settings View (components/settings.ts)

Configuration options with description on left, control on right:

```
┌─────────────────────────────────────────────────────────────────┐
│  Settings                                                       │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                                                             ││
│  │  RECORDING                                                  ││
│  │                                                             ││
│  │  Hotkey                                                     ││
│  │  Press and hold to record              [ F8 ] [ Change ]    ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  AUDIO                                                      ││
│  │                                                             ││
│  │  Microphone                                                 ││
│  │  Select audio input device             [ System Default ▼]  ││
│  │                                                             ││
│  │  Sound feedback                                             ││
│  │  Play sounds for recording start/stop             [====●]   ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  OUTPUT                                                     ││
│  │                                                             ││
│  │  Auto-paste                                                 ││
│  │  Automatically paste after transcription          [====●]   ││
│  │  If disabled, text is only copied to clipboard              ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  MODEL                                                      ││
│  │                                                             ││
│  │  Speech model                                               ││
│  │  ggml-tiny.en.bin (English only)                            ││
│  │  ~/.../models/ggml-tiny.en.bin           [ Change... ]      ││
│  │                                                             ││
│  │  Inference backend                                          ││
│  │  Vulkan (GPU accelerated)                   ✓ Optimal       ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  PRIVACY                                                    ││
│  │                                                             ││
│  │  Save transcript history                                    ││
│  │  Store transcripts locally for search              [====●]  ││
│  │                                                             ││
│  │  History limit                                              ││
│  │  Maximum transcripts to keep           [ 1,000        ▼]    ││
│  │                                                             ││
│  │  Clear history                                              ││
│  │  Delete all saved transcripts           [ Clear All... ]    ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  SYSTEM                                                     ││
│  │                                                             ││
│  │  Launch at startup                                          ││
│  │  Start Fing when you log in                       [====●]   ││
│  │                                                             ││
│  │  ─────────────────────────────────────────────────────────  ││
│  │                                                             ││
│  │  PERMISSIONS (macOS)                                        ││
│  │                                                             ││
│  │  Microphone                                     ✓ Granted   ││
│  │  Accessibility                                  ✓ Granted   ││
│  │                                    [ Open System Settings ] ││
│  │                                                             ││
│  └─────────────────────────────────────────────────────────────┘│
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**History Limit Dropdown Options (only shown when history is enabled):**
| Value | Display |
|-------|---------|
| 100 | "100" |
| 500 | "500" |
| 1000 | "1,000" |
| 5000 | "5,000" |
| -1 | "Unlimited" |

**Clear All Confirmation:**
When user clicks "Clear All...", show confirmation dialog:
```
┌─────────────────────────────────────────┐
│  Clear all history?                     │
│                                         │
│  This will permanently delete all       │
│  saved transcripts. This cannot be      │
│  undone.                                │
│                                         │
│         [ Cancel ]  [ Clear All ]       │
└─────────────────────────────────────────┘
```

**Hotkey Conflict Handling:**
- When user clicks "Change", enter key capture mode
- If captured key conflicts with system shortcut, show warning:
  ```
  ⚠️ F5 is reserved by the system. Please choose another key.
  ```
- If captured key conflicts with another app (if detectable), show warning:
  ```
  ⚠️ F6 may conflict with another application. Try it, or choose another key.
  ```

**Toggle Switch:** Standard on/off switch (not checkbox). Shows filled/colored when on, empty/gray when off.

### About View (components/about.ts)

App information and links:

```
┌─────────────────────────────────────────────────────────────────┐
│  About                                                          │
│                                                                 │
│                     ┌─────────────────┐                         │
│                     │   ≋≋≋           │  ← AudioLines icon      │
│                     │     Fing        │                         │
│                     └─────────────────┘                         │
│                                                                 │
│                    Version 0.1.0                                │
│                    Commit: a1b2c3d                              │
│                    Built: 2024-01-15                            │
│                                                                 │
│          Fast, private, local speech-to-text                    │
│                                                                 │
│                    Backend: Vulkan (GPU)                        │
│                                                                 │
│                  [ View on GitHub ]                             │
│                                                                 │
│            © 2024 Your Name. MIT License.                       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Floating Indicator (indicator.html + indicator.ts)

Separate small pill-shaped window for recording feedback.

**Size:** 70x30 pixels  
**Position:** Bottom center of primary screen, ~50px from edge  
**Behavior:** Transparent, no decorations, always on top, click-through

```html
<!-- indicator.html -->
<link rel="stylesheet" href="indicator.css">
<div class="indicator" id="indicator">
  <!-- Recording state: red dot + "Rec" text -->
  <div class="recording" id="recording">
    <span class="red-dot"></span>
    <span class="rec-text">Rec</span>
  </div>
  <!-- Processing state: LoaderCircle spinning -->
  <div class="processing hidden" id="processing">
    <!-- Lucide LoaderCircle icon (inline SVG) -->
    <svg class="spinner" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M21 12a9 9 0 1 1-6.219-8.56"/>
    </svg>
  </div>
</div>
<script src="indicator.ts" type="module"></script>
```

```css
/* indicator.css */
.indicator {
  width: 70px;
  height: 30px;
  background: rgba(0, 0, 0, 0.8);
  border-radius: 15px;  /* Full pill shape */
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  color: white;
  font-family: -apple-system, BlinkMacSystemFont, sans-serif;
  font-size: 12px;
  font-weight: 500;
  transform: scale(0);
  animation: scaleIn 0.1s ease-out forwards;
}

.indicator.shrinking {
  animation: scaleOut 0.1s ease-in forwards;
}

@keyframes scaleIn {
  from { transform: scale(0); }
  to { transform: scale(1); }
}

@keyframes scaleOut {
  from { transform: scale(1); }
  to { transform: scale(0); }
}

.red-dot {
  width: 8px;
  height: 8px;
  background: #ef4444;
  border-radius: 50%;
}

.spinner {
  animation: spin 1s linear infinite;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.hidden { display: none; }
```

**Animation Sequence:**
1. **Appear** - Starts as tiny dot, quickly expands to full pill size (~100ms)
2. **Recording** - Shows red dot (●) on left + "Rec" text on right
3. **Processing** - Red dot and "Rec" fade out, LoaderCircle icon fades in and spins
4. **Disappear** - Pill shrinks quickly back to dot and fades out (~100ms)

**Window Properties:**
- Size: 70x30px (pill shape)
- Position: Bottom center of primary display, ~50px from bottom edge
- Style: Rounded corners (full pill radius), semi-transparent dark background, no title bar
- Behavior: Always on top, click-through (doesn't steal focus), no taskbar entry

---

### main.ts

**Responsibilities:**
- Check AppState on load → show Onboarding or normal UI
- Initialize sidebar navigation
- Handle view switching
- Listen for Tauri events (new transcript, status changes)
- Call Tauri IPC commands

**Key Event Listeners:**
- `listen('app-state-changed', callback)` - Handle state transitions
- `listen('transcript-added', callback)` - Update history UI, refresh stats
- `listen('recording-started', callback)` - Update any UI state if needed
- `listen('recording-stopped', callback)` - Update any UI state if needed
- `listen('download-progress', callback)` - Update onboarding progress

**Key IPC Calls:**
- `invoke('get_app_state')` - Check if onboarding needed
- `invoke('get_recent_transcripts', { limit: 50 })`
- `invoke('delete_transcript', { id })`
- `invoke('delete_all_transcripts')` - Clear all history
- `invoke('search_transcripts', { query })`
- `invoke('get_stats')` - For Home dashboard
- `invoke('get_settings')`
- `invoke('update_settings', { settings })`
- `invoke('get_audio_devices')` - List available microphones
- `invoke('set_audio_device', { deviceId })` - Select microphone
- `invoke('check_for_updates')` - Check GitHub for updates
- `invoke('get_app_info')` - Get version, commit, build date, backend
- `invoke('start_model_download')` - Begin model download
- `invoke('select_model_file')` - Open file picker
- `invoke('test_microphone')` - Test mic input level
- `invoke('request_permissions')` - Request OS permissions (macOS)
- `invoke('complete_setup')` - Finish onboarding, transition to Ready

---

## TypeScript Types (lib/types.ts)

```typescript
type AppState = 'needs-setup' | 'initializing' | 'ready' | 'recording' | 'processing';

interface Transcript {
  id: number;
  text: string;
  createdAt: number;
  durationMs?: number;
  appContext?: string;
  wordCount: number;
}

interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

// History limit options: 100, 500, 1000, 5000, or -1 for unlimited
type HistoryLimit = 100 | 500 | 1000 | 5000 | -1;

interface Settings {
  hotkey: string;              // Default: F8
  modelPath: string;           // Path to .bin model file
  selectedMicrophoneId: string | null;  // Selected mic device ID, null = system default
  autoStart: boolean;          // Launch on system startup
  soundEnabled: boolean;       // Play sounds for recording start/stop
  pasteEnabled: boolean;       // Attempt to paste after setting clipboard
  historyEnabled: boolean;     // Save transcripts to database
  historyLimit: HistoryLimit;  // Max transcripts to keep, -1 = unlimited
}

interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  downloadUrl?: string;
  releaseNotes?: string;
}

interface AppInfo {
  name: string;
  version: string;
  commit: string;
  buildDate: string;
  repository: string;
  inferenceBackend: 'Metal' | 'Vulkan' | 'CPU';
}

interface Stats {
  totalTranscriptions: number;
  totalWords: number;
  transcriptionsToday: number;
  wordsToday: number;
  averageWordsPerTranscription: number;
}

type SidebarItem = 'home' | 'history' | 'settings' | 'about';

interface DownloadProgress {
  bytesDownloaded: number;
  totalBytes: number;
  percentage: number;
  status: 'not-started' | 'downloading' | 'verifying' | 'complete' | 'failed';
  errorMessage?: string;
}

interface ModelVerification {
  path: string;
  exists: boolean;
  sizeValid: boolean;
  hashValid: boolean;
  isValid: boolean;
}

interface MicrophoneTest {
  deviceName: string;
  peakLevel: number;  // 0.0 - 1.0
  isReceivingAudio: boolean;
}

interface PermissionStatus {
  microphone: 'unknown' | 'granted' | 'denied';
  accessibility: 'unknown' | 'granted' | 'denied' | 'not-applicable';  // macOS only
}

interface HotkeyRegistrationResult {
  success: boolean;
  error?: 'conflict-system' | 'conflict-app' | 'permission-denied' | 'unknown';
  message?: string;
}
```

---

## Settings Structure

```typescript
// History limit options: 100, 500, 1000, 5000, or -1 for unlimited
type HistoryLimit = 100 | 500 | 1000 | 5000 | -1;

interface Settings {
  hotkey: string;              // Default: F8
  modelPath: string;           // Path to .bin model file
  selectedMicrophoneId: string | null;  // Selected mic device ID, null = system default
  autoStart: boolean;          // Launch on system startup
  soundEnabled: boolean;       // Play sounds for recording start/stop
  pasteEnabled: boolean;       // Attempt to paste (Cmd/Ctrl+V) after setting clipboard
  historyEnabled: boolean;     // Save transcripts to local database
  historyLimit: HistoryLimit;  // Max transcripts to keep, -1 = unlimited
}
```

**Default settings:**
- **Hotkey:** `F8`
- **Model path:** `{app_data_dir}/models/ggml-tiny.en.bin`
- **Selected microphone:** null (use system default)
- **Auto-start:** true
- **Sound feedback:** true
- **Paste enabled:** true (best-effort; clipboard is always set)
- **History enabled:** true
- **History limit:** 1000

**Note:** On macOS, hold hotkey detection and pasting require Accessibility permission.

---

## Tauri Configuration (tauri.conf.json)

```json
{
  "build": {
    "beforeDevCommand": "bun run frontend:dev",
    "beforeBuildCommand": "bun run frontend:build",
    "devUrl": "http://localhost:5173",
    "frontendDist": "../dist"
  },
  "app": {
    "trayIcon": {
      "iconPath": "icons/tray.png",
      "iconAsTemplate": true
    },
    "windows": [
      {
        "label": "main",
        "title": "Fing",
        "width": 800,
        "height": 600,
        "visible": false,
        "resizable": true,
        "minWidth": 600,
        "minHeight": 400,
        "center": true
      },
      {
        "label": "indicator",
        "title": "",
        "url": "indicator.html",
        "width": 70,
        "height": 30,
        "visible": false,
        "resizable": false,
        "decorations": false,
        "transparent": true,
        "alwaysOnTop": true,
        "skipTaskbar": true,
        "center": false
      }
    ]
  },
  "bundle": {
    "resources": [
      "sounds/recording-start.wav",
      "sounds/recording-done.wav"
    ]
  }
}
```

**Note:** The indicator window position (bottom center) must be set programmatically in Rust based on the primary monitor dimensions.

---

## Tray Menu Structure

**Normal State (Ready):**
```
┌─────────────────────────────┐
│  Open App                   │  ← Opens main window (Home tab)
├─────────────────────────────┤
│  ▶ Select Mic               │  ← Submenu
│    ├─ ✓ System Default      │    ← Default option
│    ├─   MacBook Pro Mic     │
│    ├─   External USB Mic    │
│    └─   AirPods Pro         │
├─────────────────────────────┤
│  Check for Updates          │  ← Checks GitHub releases
│  About                      │  ← Opens main window (About tab)
├─────────────────────────────┤
│  Quit                       │
└─────────────────────────────┘
```

**Setup Required State (NeedsSetup):**
```
┌─────────────────────────────┐
│  Complete Setup...          │  ← Opens onboarding
├─────────────────────────────┤
│  Quit                       │
└─────────────────────────────┘
```

---

## CSS Style Guidelines

### General Principles
- Clean, simple design
- **No gradients** - use solid colors only
- **No zoom/scale hover effects** on elements in main window
- Respect system color scheme (`prefers-color-scheme: dark/light`)
- Consistent spacing and typography

### Color Palette

```css
/* Light mode */
--bg-primary: #ffffff;
--bg-secondary: #f5f5f5;
--bg-sidebar: #1a1a1a;
--text-primary: #1a1a1a;
--text-secondary: #666666;
--text-sidebar: #ffffff;
--accent: #3b82f6;        /* Blue for primary actions */
--danger: #ef4444;        /* Red for destructive actions */
--success: #22c55e;       /* Green for success states */
--warning: #f59e0b;       /* Amber for warnings */
--border: #e5e5e5;

/* Dark mode */
--bg-primary: #1a1a1a;
--bg-secondary: #262626;
--bg-sidebar: #0d0d0d;
--text-primary: #ffffff;
--text-secondary: #a3a3a3;
--text-sidebar: #ffffff;
--accent: #60a5fa;
--danger: #f87171;
--success: #4ade80;
--warning: #fbbf24;
--border: #333333;
```

### Button Styles

Three button variants:

**Primary** - Main actions (solid background)
```css
.btn-primary {
  background-color: var(--accent);
  color: white;
  border: none;
  padding: 8px 16px;
  border-radius: 6px;
}
.btn-primary:hover {
  opacity: 0.9;  /* Subtle hover, no scale */
}
```

**Secondary** - Secondary actions (muted background)
```css
.btn-secondary {
  background-color: var(--bg-secondary);
  color: var(--text-primary);
  border: none;
  padding: 8px 16px;
  border-radius: 6px;
}
.btn-secondary:hover {
  background-color: var(--border);
}
```

**Outline** - Tertiary actions (border only)
```css
.btn-outline {
  background-color: transparent;
  color: var(--text-primary);
  border: 1px solid var(--border);
  padding: 8px 16px;
  border-radius: 6px;
}
.btn-outline:hover {
  background-color: var(--bg-secondary);
}
```

**Danger** - Destructive actions
```css
.btn-danger {
  background-color: var(--danger);
  color: white;
  border: none;
  padding: 8px 16px;
  border-radius: 6px;
}
.btn-danger:hover {
  opacity: 0.9;
}
```

### Toggle Switch

```css
.toggle {
  width: 44px;
  height: 24px;
  background-color: var(--bg-secondary);
  border-radius: 12px;
  position: relative;
  cursor: pointer;
  transition: background-color 0.2s;
}
.toggle.active {
  background-color: var(--accent);
}
.toggle::after {
  content: '';
  width: 20px;
  height: 20px;
  background: white;
  border-radius: 50%;
  position: absolute;
  top: 2px;
  left: 2px;
  transition: transform 0.2s;
}
.toggle.active::after {
  transform: translateX(20px);
}
```

### Sidebar
```css
.sidebar {
  width: 180px;
  background-color: var(--bg-sidebar);
  color: var(--text-sidebar);
  padding: 16px 0;
}
.sidebar-item {
  padding: 12px 20px;
  cursor: pointer;
}
.sidebar-item:hover {
  background-color: rgba(255, 255, 255, 0.1);
}
.sidebar-item.active {
  background-color: rgba(255, 255, 255, 0.15);
  border-left: 3px solid var(--accent);
}
```

### Cards (for Home dashboard stats)
```css
.card {
  background-color: var(--bg-secondary);
  border-radius: 8px;
  padding: 16px;
  /* No shadows, no gradients */
}
```

### List Items (for History)
```css
.list-item {
  padding: 12px 16px;
  border-bottom: 1px solid var(--border);
}
.list-item:hover {
  background-color: var(--bg-secondary);
  /* No scale transform */
}
```

### Date Group Headers (for History)
```css
.date-group-header {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  padding: 8px 16px;
  margin-top: 16px;
}
.date-group-header:first-child {
  margin-top: 0;
}
```

### Settings Sections
```css
.settings-section {
  margin-bottom: 24px;
}
.settings-section-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 12px;
}
.settings-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 0;
}
.settings-row + .settings-row {
  border-top: 1px solid var(--border);
}
```

### Progress Bar (for Onboarding download)
```css
.progress-bar {
  width: 100%;
  height: 8px;
  background-color: var(--bg-secondary);
  border-radius: 4px;
  overflow: hidden;
}
.progress-bar-fill {
  height: 100%;
  background-color: var(--accent);
  transition: width 0.2s ease;
}
```

### Permission Status Indicators
```css
.permission-granted {
  color: var(--success);
}
.permission-denied {
  color: var(--danger);
}
.permission-unknown {
  color: var(--text-secondary);
}
```

### Empty State (for History/Home when disabled)
```css
.empty-state {
  text-align: center;
  padding: 48px 24px;
  color: var(--text-secondary);
}
.empty-state-icon {
  font-size: 48px;
  margin-bottom: 16px;
  opacity: 0.5;
}
.empty-state-title {
  font-size: 16px;
  font-weight: 500;
  margin-bottom: 8px;
  color: var(--text-primary);
}
.empty-state-description {
  font-size: 14px;
}
```

---

## Error Handling

**Critical errors (show to user):**
- Model file not found
- Model file invalid (verification failed)
- No microphone access / permission denied
- Audio device initialization failed
- Hotkey registration failed (conflict or permission)

**Recoverable errors (log, continue):**
- SQLite write failed (don't block paste)
- Transcription returned empty (might be silence)
- Paste injection failed (clipboard is still set, show "Text copied to clipboard" notification)

**User feedback:**
- Floating indicator appears during recording (bottom center of screen)
- Indicator shows processing spinner while transcribing
- Optional sound on transcription complete
- System notification "Text copied to clipboard" if paste fails
- Error notification if critical failure

**macOS permissions (V1):**
- Microphone permission is required for audio capture
- Accessibility permission is required for: hold hotkey detection (global key up/down) and programmatic paste
- If permission missing: show clear status in Settings and Onboarding with button to open System Settings
