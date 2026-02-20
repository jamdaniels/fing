# CLAUDE.md

Fing is a speech-to-text desktop app (system tray). Hold keyboard shortcut → speak → release → transcribed text pasted. All local via whisper.cpp.

## Tech Stack
- **Frontend:** Vanilla TypeScript + HTML/CSS + Lucide icons
- **Backend:** Tauri v2 + Rust
- **Inference:** whisper-rs (Metal on macOS, Vulkan on Windows, CPU fallback)
- **Audio:** cpal capture + rubato resampling to 16kHz
- **Storage:** rusqlite (SQLite with FTS5)

## Commands
```bash
bun run dev        # Development (Tauri + Vite)
bun run build      # Production build
bun run typecheck  # Frontend type checking
bun run lint       # Lint (ultracite/biome)
bun run lint:fix   # Auto-fix lints (ultracite/biome)
```

## State Machine (state.rs)
```
NeedsSetup → Ready ⇄ Recording → Processing → Ready
```
Hotkey only active when `Ready`. Model must be downloaded before transitioning out of `NeedsSetup`.

## Core Pipeline (hotkey.rs)
1. F9 down → init mic, show indicator, start capture
2. F9 up → stop capture, resample 16kHz, transcribe via whisper-rs
3. Done → direct text input (no clipboard), optional DB save, hide indicator

## Project Structure
```
src-tauri/src/
├── lib.rs              # Tauri setup, tray, IPC commands
├── state.rs            # AppState enum + transitions
├── hotkey.rs           # Recording pipeline (core flow)
├── hotkey_listener.rs  # Key event detection
├── hotkey_config.rs    # Hotkey parsing/validation
├── audio.rs            # cpal capture + rubato resampling
├── transcribe.rs       # whisper-rs wrapper
├── model.rs            # Model download + SHA256 verify
├── settings.rs         # Settings struct + persistence
├── db.rs               # SQLite FTS5 history
├── platform/macos.rs   # macOS permissions + direct text input/focus helpers
└── indicator.rs        # Recording overlay window

src/
├── main.ts               # Entry, sidebar nav, view switching
├── components/onboarding.ts  # Setup wizard
├── lib/types.ts          # TypeScript types (mirrors Rust)
└── lib/ipc.ts            # Typed invoke wrappers
```

## IPC Pattern
- Frontend calls Rust via `invoke()`, types in `lib/types.ts` mirror Rust structs
- Events from Rust via `app.emit()`: `app-state-changed`, `transcript-added`
- Model download progress is retrieved via IPC polling (`get_download_progress`)
- Root listeners are attached once on `#sidebar` and `#content`; `#content` routes handling by current view to avoid listener leaks

## Async (Rust)
Use `tauri::async_runtime::spawn` for async, `tauri::async_runtime::block_on` for sync contexts. No ad-hoc tokio runtimes.

## Key Constraints
- **Privacy:** Mic only while hotkey held, no cloud, no telemetry
- **Models:** Small Q5 (~190MB), Small (~488MB), Large Turbo Q5 (~574MB)
- **macOS:** Requires Accessibility + Microphone permissions
- **Hold-only:** No toggle mode, hold-to-record only

## Data Paths (macOS)
- **Models:** `~/Library/Application Support/com.jamdaniels.fing/models/`
- **Database:** `~/Library/Application Support/com.jamdaniels.fing/transcripts.db`
- **Settings:** `~/Library/Application Support/com.jamdaniels.fing/settings.json`
