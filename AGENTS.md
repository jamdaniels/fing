# AGENTS.md
Guidance for autonomous coding agents working inside Fing.

## Mission & Context
- Fing is a privacy-first speech-to-text tray app: hold F8, speak, text is pasted locally.
- All inference runs via whisper-rs/whisper.cpp with Metal (macOS) or Vulkan (Windows); no cloud calls.
- App state machine: NeedsSetup → Initializing → Ready ⇄ Recording → Processing → Ready; only register the hotkey in `Ready`.
- Respect onboarding: model must be downloaded and verified before exiting `NeedsSetup`.
- Refer to `CLAUDE.md` for architecture, modules, and storage locations before making structural changes.
- `details.md` documents assets (tray icons, sounds), model integrity rules, logging expectations, and platform-specific setups.
- No Cursor or Copilot rule files exist; this handbook supersedes external defaults.

## Repository Guardrails
- **Do not touch `.env` or `.env.local`** under any circumstance; only read/write `.env.example` if needed.
- Prefer Bun for tooling (`bun install`, `bun run`) and avoid mixing package managers.
- Never revert user changes; operate within the existing diff without destructive git commands.
- Keep edits ASCII unless the target file already relies on other glyphs.
- Add comments only when logic would otherwise be unclear; prefer self-explanatory names.
- Avoid new abstractions unless an obvious reuse or simplification emerges; inline simple helpers instead of bloating the API surface.
- The desktop app might already be running via `bun run dev`; do not restart unless debugging requires it.
- Prefer running linters/formatters (Biome/ultracite when present) before heavy typecheck loops.
- Git commits happen **only** when the user explicitly asks; never amend or force-push without instruction.
- Use `gh` CLI for GitHub interactions, and keep new repos private unless told otherwise.

## Commands & Tooling
- **Install deps**: `bun install` (installs frontend + Tauri hooks) and `cd src-tauri && cargo fetch` for Rust crates if cold cache.
- **Model download (optional for dev)**: `mkdir -p .models && curl -L "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin" -o .models/ggml-tiny.en.bin`.
- **Development app**: `bun run dev` launches Vite + Tauri (watches Rust + frontend). App window auto-spawns but UI mainly lives in tray.
- **Frontend-only dev**: `bun run frontend:dev` to iterate on HTML/CSS/TS without the Rust host.
- **Production build**: `bun run build` (runs Vite build then `tauri build`, producing bundles under `src-tauri/target/release/bundle`).
- **Frontend typecheck**: `bun run typecheck` (tsc `--noEmit`, uses `tsconfig.json`).
- **Lint/format (ultracite/biome)**: `bun run lint` for checks, `bun run lint:fix` for autofixes; run these before heavier loops.
- **Rust lint**: `cd src-tauri && cargo clippy --all-targets -- -D warnings` (keep warnings fatal; align with `tracing` usage).
- **Rust fmt**: `cd src-tauri && cargo fmt` (run before/after invasive backend edits).
- **Run all backend tests**: `cd src-tauri && cargo test` (even though few tests exist, keep harness healthy).
- **Single Rust test**: `cd src-tauri && cargo test hotkey::tests::register_hotkey -- --nocapture` (replace path with the module/test you need).
- **Bundle sanity**: `cd src-tauri && cargo tauri build` if you need direct cargo invocation.
- **Database**: SQLite lives under system app data; avoid touching production DB during dev—use the local environment’s default path.

## Frontend Workflow
- Stack: Vanilla TypeScript, HTML templates, CSS modules, Lucide icons; no framework.
- Avoid generic UI; typography should be deliberate (custom font stacks or purposeful fallbacks) and backgrounds should include gradients/patterns when reworking layouts.
- Never rely on purple-on-white defaults or dark-mode bias; craft color systems intentionally and avoid `transition-colors` (per design guidance).
- Use semantic HTML, keep layout responsive (sidebar + main content) and ensure indicator overlays animate smoothly.
- Global state is local to modules (see `src/main.ts`), so prefer module-level variables over sprawling singletons.
- For icons, import from `lucide` and generate inline SVG via `createIcon` helper in `src/lib/icons.ts`.
- Onboarding UI renders into `#onboarding-container`; keep cleanup hooks symmetrical via `cleanupOnboarding`.

## TypeScript & CSS Style
- Modules live in `src/`; keep relative imports short and grouped: third-party first, then absolute/aliased (none today), then relative `/lib`, then component-level modules.
- Enable `strict` TypeScript (already enforced); avoid `any`, stick to discriminated unions defined in `src/lib/types.ts`.
- Use explicit return types on exported functions; internal helpers can rely on inference when obvious.
- Keep DOM querying defensive: check for null, exit early, and ensure event listeners are cleaned if you mount/unmount dynamic nodes.
- Favor functional helpers (e.g., `groupTranscriptsByDate`) over stateful classes; maintain pure functions when possible.
- Debounce expensive DOM updates manually (see search input), reuse that pattern across new filters/searches.
- CSS lives in `index.html`/`indicator.html`; respect existing naming conventions (`sidebar-item`, `stat-card`) and avoid Tailwind.
- When adding animations, prefer keyframes or CSS transitions tied to specific state classes rather than JS timers.
- Clipboard writes should swallow rejections silently to preserve UX (see `copyToClipboard`).

## Rust Backend Style
- Modules reside under `src-tauri/src/`; follow existing layout (`audio.rs`, `hotkey.rs`, `state.rs`, etc.).
- Imports: standard library first, then crates, then local modules; keep braces multi-line when grouping multiple items from the same crate.
- Error types mostly use `Result<T, String>` for IPC commands; convert errors with `map_err(|e| e.to_string())` and log using `tracing` (`info`, `debug`, `error`).
- Use `lazy_static`/`once_cell` for globals that must cross threads (e.g., mic test device); prefer atomics for flags shared across threads.
- Command handlers exposed via `#[tauri::command]` should remain thin: validate input, call domain module, propagate errors to the frontend.
- When spawning threads, immediately log start/stop, gate loops with atomics, and clean up resources (stop capture, close handles) before exit.
- Keep state transitions centralized in `state.rs`; never mutate `APP_STATE` outside helper functions.
- Database layer uses rusqlite with bundled FTS5; do not bypass helpers in `db.rs` without ensuring migrations stay in sync.

## IPC & State Synchronization
- Frontend obtains data exclusively through `src/lib/ipc.ts` wrappers around `invoke`; when adding commands, extend TypeScript types in lockstep with Rust structs (`serde::Serialize`/`Deserialize`).
- Events emitted via `app.emit()` (e.g., `app-state-changed`, `transcript-added`, `navigate-to-tab`) must have listeners registered in `main.ts`; debounce UI refreshes if the event can fan out quickly.
- Keep state modules side-effect-free beyond intended actions; for example, `hotkey.rs` manages capture lifecycle, while `indicator.rs` handles overlay windows.
- Background tasks (model download, mic test) should report progress via dedicated structs so the frontend can show deterministic UI.

## Error Handling & Logging
- **Frontend**: Wrap `invoke` calls in `try/catch` only when you can act on failure; otherwise, let the UX degrade gracefully (e.g., set stats to `null`).
- **Rust**: Use `tracing` macros at the appropriate level; avoid `unwrap` outside initialization unless failure is unrecoverable.
- When user data might be missing (no transcripts, no mic devices), prefer empty arrays and show UI placeholders (`empty-state`).
- Never log raw audio, transcripts, or full filesystem paths that expose usernames; redaction is mandatory per `details.md`.
- Before transitioning app state, ensure prerequisites hold (model verified, transcriber initialized) and bubble human-readable errors to the UI.

## Data & Storage Notes
- Default model path: `~/Library/Application Support/com.fing.app/models/ggml-tiny.en.bin` on macOS; `%APPDATA%\Fing\models\...` on Windows.
- Transcripts DB: SQLite with FTS5 under each platform’s app data directory; keep schema migrations idempotent.
- Settings stored in JSON under app data; updates must go through Rust `settings` module to keep watchers consistent.
- History can be disabled; honor `historyEnabled` before writing transcripts.

## Assets & Indicator
- Tray icons: Lucide AudioLines derivative; see `details.md` for required PNG sizes and `.ico` bundle.
- Sounds: `recording-start.wav` and `recording-done.wav` in `src-tauri/sounds/`; keep <0.5s WAV/PCM 16-bit 44.1kHz.
- Indicator window states (`indicator_show_recording`, `indicator_show_processing`, `indicator_hide`) must remain mutually exclusive; update CSS animations instead of proliferating new windows.

## Testing Expectations
- Rust: Keep `cargo test` passing even if test coverage is light; add unit tests near logic such as transcription parsing or state transitions.
- When adding tokio async code, mark tests with `#[tokio::test]` and ensure runtime features (`rt-multi-thread`) stay enabled.
- Frontend: No automated tests yet; validate flows manually by running `bun run dev` and exercising onboarding, history search, and settings toggles.
- For quick manual checks, leverage `tests.md` for scenario lists; expand that document instead of adding ad-hoc markdown files.

## Naming & Formatting Conventions
- TypeScript variables use `camelCase`; React-style hooks are absent, so prefer descriptive verbs (`renderSidebar`, `loadTranscripts`).
- Rust structs/enums stay in `PascalCase`; module-level statics are `SCREAMING_SNAKE_CASE` when immutable or `Atomic` handles.
- Keep file names kebab or snake case matching language norms (`main.ts`, `hotkey.rs`).
- Sorting: when updating lists (audio devices, sidebar items), maintain deterministic ordering to prevent flicker.
- Formatting: run `cargo fmt` and rely on your editor’s Prettier-equivalent for TS (Vite/Bun uses standard 2 spaces, semicolons on).

## Hotkey & Audio Pipeline Reminders
- Capture begins on F8 down via `hotkey.rs`; ensure microphone initialization, indicator, and clipboard/paste happen in the documented order.
- Resample audio to 16kHz using rubato before whisper inference; do not bypass resampling for speed shortcuts.
- Keep microphone access time-bounded: start capture only on hold and shut down immediately afterward to satisfy privacy promises.
- When editing `hotkey.rs`, ensure indicator and sounds stay synchronized with the state machine to avoid user confusion.

## Security & Privacy
- Re-verify Whisper model integrity (exists, size, SHA256) before allowing transcription; block setup completion until valid.
- Accessibility and microphone permissions are mandatory on macOS; expose status via `request_permissions` IPC so onboarding can surface accurate guidance.
- Never introduce telemetry, crash reporting, or background network calls without explicit product direction.
- Clipboard fallbacks should notify users via `notifications::notify_clipboard_fallback` when paste injection fails.

## When Expanding Functionality
- Update both TypeScript and Rust types simultaneously; keep serialized shapes aligned to avoid runtime panics.
- Extend `src/lib/ipc.ts` for new commands and adjust `src/lib/types.ts` to mirror Rust structs/enums.
- Document any new workflows briefly in `README.md` or existing docs (e.g., `details.md`) instead of spawning new markdown sprawl.
- Preserve accessibility (keyboard-only nav, ARIA where relevant) when altering UI components.

## Quick Reference Paths
- Frontend entry: `src/main.ts`.
- Onboarding component: `src/components/onboarding.ts`.
- IPC helpers: `src/lib/ipc.ts` and shared types `src/lib/types.ts`.
- Indicator logic: `indicator.html`, `src/indicator.ts`, and Rust `indicator.rs`.
- Backend entry: `src-tauri/src/lib.rs`; supporting modules share the same directory.
- Config: `package.json`, `tsconfig.json`, `src-tauri/tauri.conf.json` (not shown above but present), `bunfig.toml`.

## Troubleshooting & Tips
- `bun run dev` spawns both frontend and backend; if UI fails to load, inspect the Tauri console in the terminal first.
- Whisper model verification failures usually mean missing file, wrong size, or hash mismatch—use `model::verify` IPC to gather details.
- When hotkey registration fails on macOS, confirm Accessibility permission and check `platform/macos.rs` logs before retrying.
- If audio capture stays active after recording, ensure `capture.close_capture()` runs on all exit paths in `hotkey.rs`.
- Vite asset caching can stale indicator styles; delete `dist/` and restart dev server when CSS looks wrong.
- Clipboard injection issues often stem from missing `notifications::notify_clipboard_fallback` wiring; verify listeners in `main.ts`.
- Cargo build times drop when `whisper-rs` features align with the platform; avoid toggling Metal/Vulkan flags mid-session.
- Database schema problems? Run `db::init_db()` manually in a small test to emit migration logs instead of editing the file blindly.
- To test onboarding repeatedly, delete the local settings/model files and restart, but never commit those deletions.
- When adjusting tray menus, keep `build_tray_menu` and `handle_menu_event` changes in sync; mismatches cause silent clicks.
- Indicator window ordering issues typically resolve by reusing the existing CSS classes rather than spawning extra DOM.
- Keep `tests.md` updated whenever you manually verify a new workflow; future agents rely on that playbook.
- Prefer `console.error` for actionable frontend failures so logs surface in Tauri devtools without spamming info output.
- Always summarize user-visible impact in PR descriptions even for refactors; reviewers prioritize UX implications.

## Final Notes
- Keep this file updated when commands change or new tooling is introduced; target length ≈150 lines to stay digestible.
- Treat privacy constraints as product requirements, not preferences; any change risking leakage needs explicit approval.
- Provide concise status summaries in PRs/commits, focusing on why the change exists.
- Prefer deterministic hacks over cleverness: readability beats tricks in this mission-critical tray app.
