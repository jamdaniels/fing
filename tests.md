# Fing - Testing Checklist

> **Purpose:** Comprehensive testing checklist for the Fing speech-to-text application.

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Cold start to ready state | < 3 seconds |
| Model load time | < 2 seconds |
| Mic initialization | < 200ms |
| GPU inference (Metal/Vulkan) | < 300ms |
| CPU inference (fallback) | < 1000ms |
| Total latency (GPU) | ~220-430ms |
| Total latency (CPU) | ~470-980ms |
| Memory usage after 100 transcriptions | Stable (no leaks) |

---

## Expected Timing Breakdown

| Action | Time |
|--------|------|
| Hotkey down detected | ~1ms |
| Microphone initialization | ~50-150ms |
| Audio capture (while holding) | N/A (real-time) |
| Hotkey up detected | ~1ms |
| Audio buffer + close mic | ~5ms |
| Resample to 16kHz | ~5-15ms |
| Whisper inference (Metal/Vulkan) | ~150-250ms |
| Whisper inference (CPU fallback) | ~400-800ms |
| Clipboard set + paste attempt | ~5-15ms |
| SQLite write (async) | ~2ms (doesn't block) |

---

## Core Functionality

- [ ] Hotkey registers and responds on all platforms (F8)
- [ ] Hotkey conflict detection works
- [ ] Microphone initializes on hotkey press
- [ ] Microphone closes on hotkey release (OS indicator goes away)
- [ ] Audio capture works with default microphone
- [ ] Transcription produces correct text
- [ ] Text pastes into active application
- [ ] "Text copied to clipboard" notification shows when paste fails
- [ ] No memory leaks on repeated transcriptions
- [ ] Graceful handling when no microphone available

---

## Application States

- [ ] App launches to Onboarding when model missing
- [ ] App launches to Ready when model valid
- [ ] Hotkey is disabled during NeedsSetup/Initializing
- [ ] State transitions are correct and emit events
- [ ] Recording blocked while already Recording or Processing

---

## Onboarding

- [ ] Welcome screen displays correctly
- [ ] Model download works with progress reporting
- [ ] Model download can be cancelled
- [ ] "Choose file" opens file picker
- [ ] Model verification runs after download/selection
- [ ] Invalid model shows clear error
- [ ] macOS permission requests work
- [ ] Microphone test shows audio level
- [ ] Setup completion transitions to Ready state
- [ ] Skip button works at each step

---

## Audio

- [ ] Recording with no microphone connected shows error
- [ ] Very short recording (<0.5s) produces reasonable result or empty
- [ ] Very long recording (>2min) stops at limit
- [ ] Recording only silence produces empty/minimal result
- [ ] Microphone selection persists across restarts
- [ ] Device list updates when devices are added/removed

---

## Privacy

- [ ] Microphone only active while hotkey held
- [ ] OS mic indicator only shows during recording
- [ ] No audio data persisted (only transcripts, if enabled)
- [ ] History can be disabled entirely
- [ ] History can be cleared
- [ ] History respects configured limit

---

## Floating Indicator

- [ ] Indicator appears at bottom center when recording starts
- [ ] Indicator shows red dot + "Rec" during recording
- [ ] Indicator shows spinner during processing
- [ ] Indicator disappears after text is pasted
- [ ] Indicator stays on top of all windows
- [ ] Indicator doesn't steal focus from active app
- [ ] Indicator positions correctly on primary monitor
- [ ] Scale-in animation works (~100ms)
- [ ] Scale-out animation works (~100ms)

---

## Tray Menu

- [ ] Tray icon appears in system tray/menu bar
- [ ] Tray menu shows "Complete Setup" when NeedsSetup
- [ ] "Open App" opens main window to Home tab
- [ ] "Select Mic" submenu shows all available devices
- [ ] Selected mic has checkmark in submenu
- [ ] Switching mic from tray works
- [ ] "Check for Updates" fetches from GitHub
- [ ] "About" opens main window to About tab
- [ ] "Quit" closes app cleanly

---

## Main Window

- [ ] Window opens at 800x600 pixels
- [ ] Window is hidden on startup (tray-first)
- [ ] Sidebar navigation works (Home, History, Settings, About)
- [ ] Quit button in sidebar closes app
- [ ] Minimum size enforced (600x400)
- [ ] Window state remembered between opens

---

## Home Dashboard

- [ ] Total transcriptions count displays correctly
- [ ] Total words count displays correctly
- [ ] Today's transcriptions count updates
- [ ] Today's words count updates
- [ ] Average words per transcription calculates correctly
- [ ] Stats refresh after new transcription
- [ ] Shows empty state when history disabled

---

## History View

- [ ] History list loads and displays transcripts
- [ ] Date grouping headers display correctly (Today, Yesterday, etc.)
- [ ] Search filters transcripts correctly
- [ ] FTS search returns relevant results
- [ ] Copy button copies text to clipboard
- [ ] Delete button removes transcript
- [ ] Timestamps display correctly
- [ ] Word count shows for each transcript
- [ ] Pagination works for large lists (20 per page)
- [ ] History pruning respects limit setting
- [ ] Shows empty state when history disabled

---

## Settings View

- [ ] All settings sections display correctly
- [ ] Current hotkey displays correctly
- [ ] Hotkey can be changed (key capture)
- [ ] Hotkey conflict shows warning
- [ ] Microphone dropdown shows all devices
- [ ] Microphone selection syncs with tray menu
- [ ] Sound feedback toggle works
- [ ] Auto-paste toggle works
- [ ] Model path displays correctly
- [ ] Model can be changed via "Change" button
- [ ] Inference backend displays correctly (Metal/Vulkan/CPU)
- [ ] History enabled toggle works
- [ ] History limit dropdown shows when history enabled
- [ ] Clear All History works with confirmation
- [ ] Auto-start toggle works
- [ ] macOS: Permission status displays correctly
- [ ] All settings persist across restarts

---

## About View

- [ ] Version number displays correctly
- [ ] Git commit hash displays correctly
- [ ] Build date displays correctly
- [ ] Inference backend displays correctly
- [ ] GitHub link works (opens browser)

---

## Permissions (macOS)

- [ ] App prompts for microphone permission on first use
- [ ] App prompts for accessibility permission when needed
- [ ] App functions (clipboard-only mode) when accessibility denied
- [ ] Permission status in Settings updates correctly
- [ ] "Open System Settings" button works
- [ ] App works after granting permissions (no restart required)

---

## Cross-Platform: macOS

- [ ] F8 works as default hotkey
- [ ] Metal acceleration enabled and detected
- [ ] Accessibility permission flow works
- [ ] Microphone permission flow works
- [ ] Tray icon uses template image (adapts to light/dark)
- [ ] System notifications appear correctly
- [ ] Auto-start (Launch at Login) works
- [ ] App data stored in ~/Library/Application Support/

---

## Cross-Platform: Windows

- [ ] F8 works as default hotkey
- [ ] App runs without admin privileges
- [ ] Vulkan acceleration works (NVIDIA GPUs)
- [ ] Vulkan acceleration works (AMD GPUs)
- [ ] Vulkan acceleration works (Intel GPUs)
- [ ] CPU fallback works when no Vulkan GPU
- [ ] Tray icon appears in system tray
- [ ] System notifications appear correctly (Windows Toast)
- [ ] Auto-start (Registry/Startup folder) works
- [ ] App data stored in %APPDATA%\Fing\

---

## Model Management

- [ ] Default model path is correct per platform
- [ ] Model download URL works (Hugging Face)
- [ ] Download progress reports accurately
- [ ] Download can be cancelled
- [ ] SHA256 verification catches corrupt files
- [ ] Size verification catches partial downloads
- [ ] Invalid model prevents app from entering Ready state
- [ ] Model can be changed after initial setup
- [ ] Custom model path persists in settings

---

## Update Checker

- [ ] Update check succeeds when GitHub is reachable
- [ ] Update check handles network errors gracefully
- [ ] New version detected correctly
- [ ] Download URL points to correct release
- [ ] Release notes displayed if available
- [ ] No crash if GitHub API rate limited

---

## Stress Testing

- [ ] 100 consecutive transcriptions work without error
- [ ] Memory usage stable over extended use
- [ ] No file handle leaks
- [ ] Database doesn't grow unbounded (pruning works)
- [ ] App recovers from transient errors

---

## Edge Cases

- [ ] Empty transcription (silence only) handled gracefully
- [ ] Very long text (1000+ words) transcribed correctly
- [ ] Non-ASCII characters in transcription displayed correctly
- [ ] Rapid hotkey press/release handled (no crash)
- [ ] Hotkey held for 2+ minutes triggers auto-stop
- [ ] App launched while another instance running
- [ ] Database file locked by another process
- [ ] Model file deleted while app running
- [ ] Microphone disconnected mid-recording

---

## Accessibility

- [ ] Keyboard navigation works in main window
- [ ] Screen reader compatible (proper ARIA labels)
- [ ] Sufficient color contrast
- [ ] Focus indicators visible
- [ ] No reliance on color alone for status

---

## Security

- [ ] Model SHA256 verification cannot be bypassed
- [ ] Settings stored securely in app data directory
- [ ] No sensitive data in logs
- [ ] No network requests except: model download, update check
- [ ] Clipboard cleared option (future consideration)
