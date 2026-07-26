# Plan 001: Run whisper inference on a blocking thread instead of the async runtime

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 0cdc229..HEAD -- src-tauri/src/hotkey.rs`
> If `src-tauri/src/hotkey.rs` changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `0cdc229`, 2026-06-10

## Why this matters

Fing is a Tauri v2 tray app that transcribes speech locally with whisper.cpp.
When the user releases the hotkey, `on_key_up` in `src-tauri/src/hotkey.rs`
spawns an async task that calls `transcribe_audio(...)` **directly on a tokio
runtime worker thread**. Whisper inference is CPU-bound and can take seconds to
tens of seconds (large model, up to 5 minutes of audio). While it runs, it pins
one tokio worker, which can stall other async tasks in the app: the auto-stop
recording timer, the lazy-model-unload timer, and the main-window presentation
fallback in `lib.rs`. The same file already does this correctly for model
*initialization* (`init_transcriber_async` uses `spawn_blocking`); this plan
makes inference follow the same pattern.

## Current state

- `src-tauri/src/hotkey.rs` — owns the record → transcribe → paste pipeline.
  The bug is inside the `tauri::async_runtime::spawn(async move { ... })` block
  in `on_key_up`.

The blocking call as it exists today (`src-tauri/src/hotkey.rs:679-693`):

```rust
        // Transcribe
        let text =
            match transcribe_audio(&audio_buffer, lang.as_deref(), dictionary_prompt.as_deref()) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("Transcription failed: {}", e);
                    crate::notifications::show_error(
                        &app_handle,
                        "Transcription Error",
                        &format!("{e}"),
                    );
                    finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                    return;
                }
            };
```

The repo's existing pattern for offloading blocking work, in the same file
(`src-tauri/src/hotkey.rs:326-331`) — match it:

```rust
async fn init_transcriber_async(model_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || init_transcriber(&model_path))
        .await
        .map_err(|e| format!("Transcriber initialization task failed: {e}"))?
        .map_err(|e| e.to_string())
}
```

Context for ownership: at the point of the call, `audio_buffer: Vec<f32>`,
`lang: Option<String>`, and `dictionary_prompt: Option<String>` are all owned
locals of the async block. `audio_buffer` is not used after the
`transcribe_audio` call (verify with grep in step 1). `dictionary_terms` IS
used after the call (line ~696, `apply_dictionary_corrections`), so do not
move it into the closure.

Repo conventions (from `CLAUDE.md` and the surrounding code): Rust errors are
surfaced as `Result<T, String>` at IPC boundaries; never log raw audio or
transcripts; keep changes minimal and local.

## Commands you will need

| Purpose        | Command                                                  | Expected on success |
|----------------|----------------------------------------------------------|---------------------|
| Rust compile   | `cargo check --manifest-path src-tauri/Cargo.toml`       | exit 0              |
| Rust tests     | `cargo test --manifest-path src-tauri/Cargo.toml`        | all pass, exit 0    |

(First compile of `whisper-rs` can take several minutes; that is normal.)

## Scope

**In scope** (the only file you should modify):
- `src-tauri/src/hotkey.rs`

**Out of scope** (do NOT touch, even though they look related):
- `src-tauri/src/transcribe.rs` — the inference implementation itself is fine;
  only the call site's threading is wrong. (Plan 003 touches this file; keep
  the plans independent.)
- `src-tauri/src/hotkey_listener.rs` and `src-tauri/src/lib.rs` — both have
  uncommitted local changes from the maintainer; do not modify them.
- The speech-activity detection and dictionary logic in `hotkey.rs` — unrelated.

## Git workflow

- Branch: `advisor/001-spawn-blocking-inference`
- Single commit; message style matches repo history (short imperative subject,
  e.g. "Run whisper inference off the async runtime" — compare `git log --oneline -5`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Confirm `audio_buffer` is not used after the transcribe call

Run: `grep -n "audio_buffer" src-tauri/src/hotkey.rs`

Expected: all uses are at or before the `transcribe_audio(` call line
(declaration, emptiness check, sample-count log, `detect_speech_activity`,
and the call itself). If `audio_buffer` is referenced on any line *after* the
`transcribe_audio` call, STOP and report.

### Step 2: Wrap the inference call in `spawn_blocking`

In `src-tauri/src/hotkey.rs`, replace the excerpt shown in "Current state"
with a version that moves the owned inputs into a blocking task. Target shape:

```rust
        // Transcribe on a blocking thread; whisper inference is CPU-bound and
        // must not pin an async runtime worker.
        let lang_for_inference = lang.clone();
        let prompt_for_inference = dictionary_prompt.clone();
        let transcribe_result = tauri::async_runtime::spawn_blocking(move || {
            transcribe_audio(
                &audio_buffer,
                lang_for_inference.as_deref(),
                prompt_for_inference.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Transcription task failed: {e}"))
        .and_then(|result| result.map_err(|e| e.to_string()));

        let text = match transcribe_result {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Transcription failed: {}", e);
                crate::notifications::show_error(
                    &app_handle,
                    "Transcription Error",
                    &e,
                );
                finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                return;
            }
        };
```

Notes:
- `audio_buffer` moves into the closure (allowed per step 1).
- The error path must behave exactly as before: log, user-facing
  notification, `finish_transcription(...)`, `return`. A `JoinError` from the
  blocking task takes the same path.
- If `lang` / `dictionary_prompt` are not used after this point either, you
  may move them instead of cloning — verify with grep first
  (`dictionary_terms` is used afterward; `dictionary_prompt` likely is not).

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0.

### Step 3: Run the Rust test suite

**Verify**: `cargo test --manifest-path src-tauri/Cargo.toml` → all tests pass
(the suite at `0cdc229` passes; same count or more expected).

## Test plan

No new unit test: exercising real whisper inference requires a downloaded
model and is not feasible in CI here. The protection is:

- Existing tests in `hotkey.rs` (`speech_activity_*`,
  `failed_start_does_not_leak_into_next_stop_response`) still pass.
- The structural done-criterion below proves the call site changed.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo check --manifest-path src-tauri/Cargo.toml` exits 0
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` exits 0
- [ ] `grep -n "spawn_blocking" src-tauri/src/hotkey.rs` shows a new match in
      `on_key_up`'s async block wrapping `transcribe_audio`
- [ ] `git status --short` shows only `src-tauri/src/hotkey.rs` modified on
      the plan branch (note: `hotkey_listener.rs`, `lib.rs`,
      `platform/macos.rs`, `tauri.conf.json` carry pre-existing uncommitted
      changes from the maintainer — those must remain untouched and uncommitted)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The "Current state" excerpt no longer matches `hotkey.rs` (drift).
- `audio_buffer`, `lang`, or `dictionary_prompt` is used after the transcribe
  call in a way that prevents moving/cloning cleanly.
- The fix appears to require changing `transcribe.rs` signatures (e.g.
  `transcribe_audio` taking ownership) — that is plan-003 territory.
- `cargo test` fails twice after a reasonable fix attempt.

## Maintenance notes

- If streaming/partial transcription is ever added, the blocking-task boundary
  established here is where chunked inference should hook in.
- Reviewer should scrutinize: error-path parity (notification + state reset via
  `finish_transcription` must fire on both `JoinError` and inference error),
  and that no transcript text is logged.
- Deferred: `paste_text` (called a few lines later) also does synchronous
  platform work on the runtime; it is fast in practice and intentionally left
  out of this plan.
