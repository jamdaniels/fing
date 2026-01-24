# Fing - Build Progress Tracker

> Track implementation progress. Check off items as completed.

---

## Phase 0: Project Scaffolding - COMPLETE

### Root Config
- [x] package.json
- [x] bunfig.toml
- [x] tsconfig.json
- [x] vite.config.ts
- [x] .gitignore

### Backend Setup (src-tauri/)
- [x] Cargo.toml
- [x] tauri.conf.json
- [x] build.rs
- [x] src/main.rs
- [x] src/lib.rs

### Frontend Setup (src/)
- [x] index.html
- [x] indicator.html
- [x] main.ts
- [x] indicator.ts
- [x] styles.css
- [x] indicator.css
- [x] lib/types.ts
- [x] lib/ipc.ts
- [x] lib/icons.ts

### Assets
- [x] src-tauri/icons/*
- [x] src-tauri/sounds/recording-start.wav
- [x] src-tauri/sounds/recording-done.wav

### Verification
- [x] `bun install` succeeds
- [x] `bun run dev` launches Tauri window
- [x] `bun run build` creates .app and .dmg

---

## Phase 1: State Machine + Settings + Database - COMPLETE

- [x] src/state.rs
- [x] src/settings.rs
- [x] src/db.rs
- [x] src/app_info.rs
- [x] src/stats.rs
- [x] All IPC commands

---

## Phase 2: Model Management - COMPLETE

- [x] src/model.rs
- [x] src/engine.rs
- [x] src/transcribe.rs
- [x] All IPC commands

---

## Phase 3: Audio Capture + Core Pipeline - COMPLETE

### Backend Modules
- [x] src/audio.rs (cpal capture, rubato resample)
- [x] src/paste.rs (arboard clipboard, paste injection)
- [x] src/platform/mod.rs
- [x] src/platform/macos.rs (CGEventTap F8 detection)
- [x] src/platform/windows.rs (stubs)
- [x] src/hotkey.rs (full pipeline with audio thread)

### Pipeline Integration
- [x] F8 hotkey registration (CGEventTap on macOS)
- [x] Audio capture via channel-based thread
- [x] Transcription with lazy model loading
- [x] Clipboard set and paste
- [x] Database save (if history_enabled)

### IPC Commands
- [x] get_audio_devices()
- [x] set_audio_device()
- [x] test_microphone()

---

## Phase 4: Indicator Window + Notifications - COMPLETE

- [x] src/indicator.rs
- [x] src/notifications.rs
- [x] src/indicator.ts
- [x] src/indicator.css

---

## Phase 5: Tray Menu + Main Window UI - COMPLETE

### Backend
- [x] src/updates.rs (GitHub releases API)
- [x] Tray menu implementation
- [x] open_main_window()
- [x] quit_app()
- [x] check_for_updates()
- [x] complete_setup()

### Frontend
- [x] Sidebar navigation
- [x] Home view with stats
- [x] History view (search, date grouping, delete)
- [x] Settings view (functional toggles, mic dropdown)
- [x] About view

---

## Phase 6: Onboarding Flow - COMPLETE

- [x] src/components/onboarding.ts
  - [x] Welcome step
  - [x] Model download/selection
  - [x] Permissions (macOS)
  - [x] Mic test
  - [x] Completion screen

---

## Phase 7: Polish + Edge Cases - IN PROGRESS

### Sound Feedback
- [ ] Play recording-start.wav on F8 down
- [ ] Play recording-done.wav on transcription complete
- [ ] Respect soundEnabled setting

### Auto-Start
- [ ] macOS: LaunchAgent or Login Items
- [ ] Windows: Registry or Startup folder

### Logging
- [x] tracing crate integration
- [ ] Log files in OS-appropriate locations

### Edge Cases
- [ ] Rapid hotkey press/release guard
- [ ] 2-minute auto-stop
- [ ] Mic disconnected mid-recording
- [ ] Model deleted while running
- [ ] Empty transcription handling

### Performance
- [ ] No memory leaks (100 transcription test)
- [ ] <400ms GPU latency

---

## Final Checklist

### Core Functionality
- [x] F8 hotkey works on macOS (CGEventTap)
- [ ] F8 hotkey works on Windows (LL keyboard hook)
- [ ] Transcription produces correct text (needs model download)
- [ ] Paste works in active app (needs testing)

### Cross-Platform
- [x] macOS: Metal acceleration configured
- [ ] macOS: Accessibility permission tested
- [ ] Windows: Vulkan acceleration tested
- [ ] Windows: CPU fallback tested
