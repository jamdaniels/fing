# AGENTS.md
Compact guidance for agents working in Fing.

## Mission & Context
- Fing is a privacy-first speech-to-text tray app: hold hotkey → speak → release → text pasted. All local via whisper.cpp.
- State machine: `NeedsSetup → Ready ⇄ Recording → Processing → Ready`. Hotkey only active in `Ready`.
- Core flow (hotkey.rs): hotkey down starts mic + indicator, hotkey up stops capture → resample 16kHz → transcribe → clipboard + DB → hide indicator.
- Stack: Vanilla TypeScript + HTML/CSS + Lucide icons; Tauri v2 + Rust; whisper-rs; cpal + rubato; rusqlite (FTS5).
- Cursor/Copilot rules: none found in `.cursor/rules/`, `.cursorrules`, or `.github/copilot-instructions.md`.

## Guardrails
- **Never touch `.env` or `.env.local`** (read/write forbidden). `.env.example` is ok.
- Prefer Bun; do not mix package managers. App may already be running; do not restart unless needed.
- ASCII-only edits unless file already contains non-ASCII.
- Avoid new abstractions; prefer clear names over comments; add comments only for non-obvious logic.
- No destructive git commands; never revert unrelated user changes.
- Use `gh` for GitHub; new repos must be private unless told otherwise.

## Commands
```bash
bun run dev        # Tauri + Vite dev
bun run build      # Production build
bun run lint       # Lint (ultracite/biome)
bun run lint:fix   # Autofix lint
bun run typecheck  # Frontend tsc

cd src-tauri && cargo fmt
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo test
cd src-tauri && cargo test hotkey::tests::register_hotkey -- --nocapture
```

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

## Key Files
- `src/main.ts` (frontend entry), `src/components/onboarding.ts`
- `src/lib/ipc.ts`, `src/lib/types.ts`
- `src-tauri/src/lib.rs`, `src-tauri/src/state.rs`, `src-tauri/src/hotkey.rs`
