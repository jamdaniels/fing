# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Fing is a speech-to-text desktop app that runs in the system tray. Users hold F8, speak, release, and transcribed text is pasted into the active application. All processing happens locally using Whisper (whisper.cpp via whisper-rs).

**Tech Stack:**
- Frontend: Vanilla TypeScript + HTML/CSS + Lucide icons
- Backend: Tauri v2 + Rust
- Inference: whisper-rs with Metal (macOS) / Vulkan (Windows)
- Audio: cpal capture + rubato resampling to 16kHz
- Storage: rusqlite (SQLite with FTS5)

## Commands

```bash
# Development (runs Tauri + Vite)
bun run dev

# Production build
bun run build

# Frontend type checking
bun run typecheck

# Lint (ultracite/biome)
bun run lint
bun run lint:fix

# Download model for local dev
mkdir -p .models && curl -L "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin" -o .models/ggml-tiny.en.bin
```

## Architecture

### Application States
```
NeedsSetup → Initializing → Ready ⇄ Recording → Processing → Ready
```
Hotkey is only registered when state is `Ready`. Model must be downloaded and verified before transitioning out of `NeedsSetup`.

### Core Pipeline (hotkey.rs)
1. F8 down → init mic, show indicator, start capture
2. F8 up → stop capture, close mic, resample to 16kHz, transcribe
3. Transcription complete → clipboard + paste, save to DB, hide indicator

### Backend Modules (src-tauri/src/)
| Module | Purpose |
|--------|---------|
| lib.rs | Tauri setup, tray menu, IPC command handlers |
| state.rs | AppState enum and transitions |
| hotkey.rs | F8 hold detection, full recording pipeline |
| audio.rs | cpal capture, rubato resampling |
| transcribe.rs | whisper-rs inference wrapper |
| engine.rs | Transcription engine trait |
| model.rs | Model download, SHA256 verification |
| paste.rs | Clipboard + Cmd/Ctrl+V injection |
| platform/macos.rs | CGEventTap for global hotkey, accessibility |
| platform/windows.rs | Stubs (Windows LL keyboard hook TODO) |
| db.rs | SQLite with FTS5 for transcript history |
| stats.rs | Usage statistics computation |
| settings.rs | Settings persistence (JSON) |
| indicator.rs | Floating overlay window control |
| sounds.rs | rodio playback for start/stop sounds |
| notifications.rs | Native OS notifications (paste fallback) |
| updates.rs | GitHub release update checker |
| app_info.rs | Build info and version metadata |

### Frontend (src/)
| File | Purpose |
|------|---------|
| main.ts | Entry point, sidebar nav, view switching |
| indicator.ts | Recording/processing indicator animations |
| components/onboarding.ts | First-run setup wizard |
| lib/ipc.ts | Typed Tauri invoke wrappers |
| lib/types.ts | Shared TypeScript types |
| lib/icons.ts | Lucide icon rendering helper |

### IPC Pattern
Frontend calls Rust via `invoke()`. Types in `lib/types.ts` mirror Rust structs. Events emitted from Rust via `app.emit()` (e.g., `app-state-changed`, `transcript-added`).

## Key Constraints

- **Privacy-first**: Mic only active while F8 held, no cloud, no telemetry
- **Model not bundled**: User downloads ~75MB ggml-tiny.en.bin (SHA256 verified)
- **macOS permissions**: Requires Accessibility (for hotkey + paste) and Microphone
- **GPU acceleration**: Metal on macOS, Vulkan on Windows, CPU fallback
- **Hold-only mode**: V1 only supports hold-to-record (no toggle mode)

## File Locations

**Model:** `~/Library/Application Support/com.jamdaniels.fing/models/ggml-tiny.en.bin` (macOS)
**Database:** `~/Library/Application Support/com.jamdaniels.fing/transcripts.db`
**Settings:** `~/Library/Application Support/com.jamdaniels.fing/settings.json`
