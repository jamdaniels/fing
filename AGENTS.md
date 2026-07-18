## Mission & Context
- Fing is a privacy-first speech-to-text tray app: hold hotkey -> speak -> release -> text pasted. Its main goal is to be lightweight and fast.
- Transcription is done locally via whisper.cpp/whisper-rs and downloaded models.
- Core app states: `NeedsSetup -> Ready -> Recording -> Processing -> Ready`; recording failures return directly to `Ready`.
- Stack: Tauri v2 + Rust, Vanilla TypeScript + HTML/CSS, Lucide icons, cpal/rubato audio, rusqlite/FTS5.
- macOS and Windows behavior can differ; check the relevant platform code before changing hotkey, focus, tray, window, or input behavior.

## Commands
- `bun run dev`, `bun run build`, or `bun run frontend:build`.
- Checks: `bun run verify`, or individually `bun run typecheck`, `bun run lint` (`lint:fix`), and `bun run test` (`test:ts` / `test:rust`).

## Guardrails
- Never read, write, or modify `.env` or `.env.local` directly.
- The app may already be running; do not start it unless needed.
- No destructive git commands. Never revert unrelated user changes.
- Do not log raw audio, transcripts, secrets, or full user-specific filesystem paths.
- Settings loading/saving and startup IPC ordering are regression-critical (Windows repeatedly re-showed onboarding after restart when weakened). Before touching them, read the `REGRESSION GUARD` comments in `src-tauri/src/paths.rs` (`wait_until_initialized`), `src-tauri/src/settings.rs` (load/parse/cache/save), and `src-tauri/src/lib.rs` (`load_bootstrap_context`). Never simplify those invariants away; the tests in `settings.rs` pin them.

## Engineering Guidance
- Keep changes minimal and local. Read the owning code before changing behavior.
- TypeScript is strict; do not use `any`.
- Keep frontend IPC wrappers and shared types in `src/lib/ipc.ts` and `src/lib/types.ts`; register Rust commands in `src-tauri/src/lib.rs`.
- Keep Rust IPC commands thin: validate input and delegate to domain logic.
- State transitions should go through helpers in `state.rs`.
- Preserve the existing visual language; avoid generic purple-on-white defaults.

## Privacy & Data
- Mic access should only happen during recording or mic test flows.
- Models must be downloaded and verified before setup is complete.
- Optional transcript history is local and settings-controlled.
- Resolve app data through `src-tauri/src/paths.rs`; do not hard-code platform paths.
