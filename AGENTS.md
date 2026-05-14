## Mission & Context
- Fing is a privacy-first speech-to-text tray app: hold hotkey -> speak -> release -> text pasted.
- Transcription is local via whisper.cpp/whisper-rs. No cloud transcription or telemetry.
- Core app states: `NeedsSetup -> Ready <-> Recording -> Processing -> Ready`.
- Stack: Tauri v2 + Rust, Vanilla TypeScript + HTML/CSS, Lucide icons, cpal/rubato audio, rusqlite/FTS5.
- macOS and Windows behavior can differ; check the relevant platform code before changing hotkey, focus, tray, window, or input behavior.

## Commands
- Prefer Bun; do not mix package managers.
- Dev: `bun run dev`
- Build: `bun run build`
- Frontend build: `bun run frontend:build`
- Typecheck: `bun run typecheck`
- Lint: `bun run lint`
- Autofix lint: `bun run lint:fix`
- Tests: `bun run test`, or `bun run test:ts` / `bun run test:rust`

## Guardrails
- Never read, write, or modify `.env` or `.env.local`. `.env.example` is okay.
- The app may already be running; do not restart it unless needed.
- No destructive git commands. Never revert unrelated user changes.
- Use `gh` CLI for GitHub. New repos must be private unless told otherwise.
- Do not log raw audio, transcripts, secrets, or full user-specific filesystem paths.

## Engineering Guidance
- Keep changes minimal and local. Read the owning code before changing behavior.
- Follow existing patterns for TypeScript, Rust, IPC, styling, and tests.
- TypeScript is strict; do not use `any`.
- Add or update IPC commands in `src/lib/ipc.ts` and mirror types in `src/lib/types.ts`.
- Rust IPC commands should stay thin: validate input, call domain logic, and return `Result<T, String>`.
- State transitions should go through helpers in `state.rs`.
- Preserve the existing visual language; avoid generic purple-on-white defaults.

## Privacy & Data
- Mic access should only happen during recording or mic test flows.
- Models must be downloaded and verified before setup is complete.
- Optional transcript history is local and settings-controlled.
- App data paths:
  - macOS: `~/Library/Application Support/com.jamdaniels.fing/`
  - Windows: `C:\Users\<User>\AppData\Roaming\com.jamdaniels.fing\`
