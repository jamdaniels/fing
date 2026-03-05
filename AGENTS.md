## Mission & Context
- Fing is a privacy-first speech-to-text tray app: hold hotkey → speak → release → text pasted. All local via whisper.cpp.
- State machine: `NeedsSetup → Ready ⇄ Recording → Processing → Ready`. Hotkey capture path is active in `Ready` (onboarding test mode temporarily allows it in `NeedsSetup`).
- Core flow (`hotkey_listener.rs` + `hotkey.rs`): hotkey down starts mic + indicator, hotkey up stops capture → resample 16kHz → transcribe → optional direct text input + optional DB save (per settings) → hide indicator.
- Windows note: when the main window is focused, frontend key handlers forward hotkey press/release to Rust (`hotkey_press` / `hotkey_release`) as a WebView2 workaround.
- Stack: Vanilla TypeScript + HTML/CSS + Lucide icons; Tauri v2 + Rust; whisper-rs; cpal + rubato; rusqlite (FTS5).

## Guardrails
- **Never touch `.env` or `.env.local`** (read/write forbidden). `.env.example` is ok.
- Prefer Bun; do not mix package managers. App may already be running; do not restart unless needed.
- No destructive git commands; never revert unrelated user changes.
- Use `gh` CLI for GitHub; new repos must be private unless told otherwise.

## Code Style
### General
- Keep changes minimal and local; colocate code that changes together.
- Use explicit, descriptive names; avoid helper functions for simple inline logic.
- Only add comments for complex logic; prefer making code self-explanatory.

### TypeScript
- `strict` mode; never use `any`.
- Exports: add explicit return types when not obvious.
- Imports: third-party first, then local absolute/aliased (none today), then relative.
- DOM: null-check queries, clean up listeners for mounted/unmounted nodes.
- IPC: add commands in `src/lib/ipc.ts` and mirror types in `src/lib/types.ts`.

### Rust (Tauri)
- Imports: std → crates → local modules.
- IPC commands should be thin: validate input, call domain logic, return `Result<T, String>`.
- Convert errors with `map_err(|e| e.to_string())`, log via `tracing`.
- Use `tauri::async_runtime::{spawn, block_on}`; no ad-hoc runtimes.
- State transitions only via helpers in `state.rs`.

### UI/CSS
- Preserve existing visual language; avoid purple-on-white defaults and `transition-colors`.
- Use semantic HTML and responsive layouts; prefer CSS animations tied to state classes.
- Indicator window: keep show/hide states mutually exclusive.

## Privacy & Data
- Mic only while hotkey held; no cloud/telemetry.
- Model must be downloaded + verified before leaving `NeedsSetup`.
- Do not log raw audio or transcripts; avoid full filesystem paths with usernames.
- App data paths (macOS):
  - Models: `~/Library/Application Support/com.jamdaniels.fing/models/`
  - DB: `~/Library/Application Support/com.jamdaniels.fing/transcripts.db`
  - Settings: `~/Library/Application Support/com.jamdaniels.fing/settings.json`
