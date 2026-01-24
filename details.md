# Fing - Supplementary Information

> **Purpose:** Reference documentation for assets, model details, security, future plans, and development setup.

---

## Tray Icon Assets

The tray icon uses the **AudioLines** icon from Lucide. Export the icon as PNG files in the following sizes and place them in `src-tauri/icons/`:

| Filename | Size | Purpose |
|----------|------|---------|
| `tray-16.png` | 16x16 | macOS standard |
| `tray-16@2x.png` | 32x32 | macOS Retina (named 16@2x) |
| `tray-32.png` | 32x32 | Windows standard |
| `tray-48.png` | 48x48 | Windows high DPI |
| `tray.ico` | Multi-size | Windows (contains 16, 32, 48) |
| `tray.png` | 32x32 | Fallback |

**Icon Guidelines:**
- Use a single color (white or black depending on OS theme)
- macOS: Use template image style (white icon, system applies color)
- Windows: May need both light and dark variants
- Export from Lucide with stroke width 2, no fill

**Tauri Config:**
```json
"trayIcon": {
  "iconPath": "icons/tray.png",
  "iconAsTemplate": true
}
```

---

## Sound Files

Two sound files are required for audio feedback. Place them in `src-tauri/sounds/`:

| Filename | Purpose | Trigger |
|----------|---------|---------|
| `recording-start.wav` | Short blip/beep | When hotkey is pressed and recording begins |
| `recording-done.wav` | Confirmation tone | When transcription is complete and text is pasted |

**Format Requirements:**
- Format: WAV (PCM, 16-bit)
- Sample rate: 44100 Hz
- Channels: Mono or Stereo
- Duration: < 0.5 seconds (short, non-intrusive)

**Note:** Sound playback is controlled by the "Sound feedback" setting. When disabled, no sounds are played.

---

## Model Information

### Whisper tiny.en Model

| Property | Value |
|----------|-------|
| Filename | `ggml-tiny.en.bin` |
| Size | ~75 MB (77,691,713 bytes) |
| Language | English only |
| Download URL | `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin` |
| SHA256 | *(to be verified and pinned in code)* |

### Model Download

The Whisper tiny.en model must be provided by the user (downloaded or selected locally).

**V1 goal:** Keep the app installer small by not bundling any model weights.

**Model location:**
- Default path: `{app_data_dir}/models/ggml-tiny.en.bin`
  - macOS: `~/Library/Application Support/com.fing.app/models/`
  - Windows: `%APPDATA%\Fing\models\`
- User can override via Settings or by selecting a file during onboarding

**Download command (manual):**
```bash
curl -L "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin" \
  -o <model-path>
```

### Verification (REQUIRED)

All three checks must pass before loading the model:

1. **File exists** - Path is valid and file is accessible
2. **Size valid** - File size matches expected (~75MB ± 1MB tolerance)
3. **Hash valid** - SHA256 hash matches known good value

If any verification step fails, refuse to load model and show clear error.

---

## App Data Locations

### macOS

| Type | Path |
|------|------|
| App Support | `~/Library/Application Support/com.fing.app/` |
| Model | `~/Library/Application Support/com.fing.app/models/ggml-tiny.en.bin` |
| Database | `~/Library/Application Support/com.fing.app/transcripts.db` |
| Settings | `~/Library/Application Support/com.fing.app/settings.json` |

### Windows

| Type | Path |
|------|------|
| App Data | `%APPDATA%\Fing\` |
| Model | `%APPDATA%\Fing\models\ggml-tiny.en.bin` |
| Database | `%APPDATA%\Fing\transcripts.db` |
| Settings | `%APPDATA%\Fing\settings.json` |

---

## Security Considerations

### Model Integrity
- SHA256 verification is REQUIRED
- Refuse to load unverified models
- Pin known-good hash in code
- Re-verify on each app launch

### Local Processing
- All audio processing happens on-device
- No audio or transcripts are sent to any server
- Model download is only network operation (user-initiated)
- Microphone only active while hotkey held
- No wake word / always-listening mode

### Privacy Controls
- History can be completely disabled
- History can be cleared at any time
- No telemetry or analytics
- No crash reporting (V1)

### Settings Storage
- Settings stored in OS app data directory
- No sensitive data in settings
- Consider secure storage for future sensitive settings (API keys, etc.)

### Network Requests (V1)
Only two types of network requests:
1. **Model download** - User-initiated, one-time
2. **Update check** - GitHub releases API (can be disabled)

---

## Linux Support (Future - Out of Scope for V1)

Linux support is planned but faces fundamental challenges, particularly on Wayland.

### X11 Support (Planned for V2)

| Feature | Implementation |
|---------|----------------|
| Global hotkeys | `XGrabKey` API |
| Paste injection | `XTest` extension |
| Tray icon | X11 system tray protocol |
| Overlay window | Standard X11 window with override-redirect |

### Wayland Challenges (TBD)

| Feature | Challenge |
|---------|-----------|
| Global hotkeys | No standard protocol |
| Paste injection | Fundamentally restricted by Wayland security model |
| Always-on-top overlay | Requires layer-shell protocol (compositor-dependent) |
| Tray icon | StatusNotifierItem protocol (not universally supported) |

**Recommendation:** Linux V2 should target X11 first. Wayland support depends on ecosystem maturation.

---

## Future Enhancements (Out of Scope for V1)

1. **Toggle activation mode** - Press to start, press again to stop (V1 is hold-only)
2. **Moonshine support** - Swap engine via settings for 2-3x speed boost
3. **Multiple languages** - Use larger multilingual models
4. **Custom vocabulary** - Fine-tune for domain-specific terms
5. **Voice commands** - "Delete that", "New paragraph", etc.
6. **App integrations** - Context-aware transcription per app
7. **Audio storage** - Optional, for debugging/reprocessing
8. **Cloud sync** - Sync transcripts across devices
9. **Linux support** - X11 first, then Wayland where possible
10. **Silence trimming / VAD** - Trim silence for faster inference (needs careful tuning)
11. **Alternative models** - Support for base, small, medium models
12. **Custom hotkeys** - Modifier key combinations
13. **Text corrections** - Post-processing rules for common errors
14. **Export transcripts** - CSV, JSON, plain text export

---

## Development Setup

### Prerequisites

- **Rust** (latest stable)
- **Bun** (or Node.js with npm)
- **Tauri CLI** v2

### Platform-specific

**macOS:**
- Xcode Command Line Tools
- No additional GPU requirements (Metal is standard)

**Windows:**
- Visual Studio Build Tools (C++ workload)
- Vulkan SDK (for development)
- Vulkan drivers (users have these pre-installed with GPU drivers)

### Commands

```bash
# Install dependencies
bun install

# Download model (optional for local dev)
mkdir -p "$(pwd)/.models"
curl -L "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin" \
  -o "$(pwd)/.models/ggml-tiny.en.bin"

# Run in development
bun run dev

# Build for production
bun run build

# Type check frontend
bun run typecheck
```

**Note:** 
- `bun run dev` runs Tauri which runs `frontend:dev` via `beforeDevCommand`
- `bun run build` runs Tauri build which runs `frontend:build` via `beforeBuildCommand`

---

## Build Configuration

### Cargo Features

```toml
# macOS: Metal (always enabled)
[target.'cfg(target_os = "macos")'.dependencies]
whisper-rs = { version = "0.15", features = ["metal"] }

# Windows: Vulkan (cross-vendor GPU support)
[target.'cfg(target_os = "windows")'.dependencies]
whisper-rs = { version = "0.15", features = ["vulkan"] }
```

### Build Outputs

| Platform | Output |
|----------|--------|
| macOS | `target/release/bundle/macos/Fing.app` |
| Windows | `target/release/bundle/msi/Fing_x.x.x_x64_en-US.msi` |

---

## Update Mechanism

### Version Checking

The app checks for updates via the GitHub releases API:
```
GET https://api.github.com/repos/{owner}/{repo}/releases/latest
```

### Update Flow

1. User clicks "Check for Updates" or app checks on startup (if enabled)
2. Fetch latest release from GitHub API
3. Compare version strings (semver)
4. If newer version available:
   - Show notification with version and changelog
   - Provide download link to GitHub releases page
5. User manually downloads and installs

**Note:** V1 does not include auto-update. Users download new versions manually.

---

## Logging

### Log Levels

| Level | Usage |
|-------|-------|
| ERROR | Critical failures (model load, permissions) |
| WARN | Recoverable issues (paste failed, empty transcription) |
| INFO | Normal operations (recording started, transcription complete) |
| DEBUG | Detailed diagnostics (timing, audio levels) |

### Log Location

- Development: Console output
- Production: 
  - macOS: `~/Library/Logs/Fing/fing.log`
  - Windows: `%APPDATA%\Fing\logs\fing.log`

### What NOT to log

- Audio data (raw samples)
- Transcription text (privacy)
- Full model paths (may contain username)

---

## Keyboard Shortcuts

### Default Hotkey

| Platform | Key | Notes |
|----------|-----|-------|
| macOS | F8 | Requires Accessibility permission |
| Windows | F8 | No special permissions |

### Supported Keys (V1)

- Function keys: F1-F12
- Standalone keys without modifiers

### Reserved Keys (cannot use)

- Escape
- Tab
- Caps Lock
- Space (too easy to trigger accidentally)
- Enter/Return
- System reserved (Print Screen, etc.)

### Modifier Support (V2)

Future versions may support:
- Ctrl/Cmd + Key
- Alt/Option + Key
- Shift + Key
- Multiple modifier combinations

---

## Glossary

| Term | Definition |
|------|------------|
| **Cold start** | Initializing microphone on-demand (not pre-warmed) |
| **FTS** | Full-Text Search (SQLite FTS5) |
| **Hold mode** | Press and hold hotkey to record, release to transcribe |
| **Inference** | Running the Whisper model on audio data |
| **Metal** | Apple's GPU acceleration framework |
| **Tray-first** | App primarily lives in system tray, window is optional |
| **Vulkan** | Cross-platform GPU acceleration API |
| **whisper.cpp** | C++ port of OpenAI's Whisper model |
| **whisper-rs** | Rust bindings for whisper.cpp |
