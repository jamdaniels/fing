# Plan 003: Small hardening — path-aware transcriber init, panic-free get_app_state, single-open mic test

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 0cdc229..HEAD -- src-tauri/src/transcribe.rs src-tauri/src/lib.rs src-tauri/src/audio.rs`
> If any of these changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. NOTE: at planning time, `lib.rs`
> carried uncommitted maintainer changes around lines ~562 (update_hotkey),
> ~947 (keyboard device observer), ~1015 (run-event match), and the test
> module — those regions are expected to differ from `0cdc229` and are NOT
> drift for this plan; the regions this plan edits (`get_app_state`,
> `start_mic_test`) were untouched by them.

## Status

- **Priority**: P2
- **Effort**: S (three independent one-file fixes)
- **Risk**: LOW
- **Depends on**: none (touches `transcribe.rs` and `lib.rs`, which plans
  001/002 deliberately avoid)
- **Category**: bug / tech-debt
- **Planned at**: commit `0cdc229`, 2026-06-10

## Why this matters

Three small latent defects in Fing (a Tauri v2 local speech-to-text tray app):

1. **`init_transcriber` ignores its argument when a model is already loaded.**
   Today a restart-required flow masks this, but any future caller passing a
   *different* model path gets `Ok(())` with the old model silently still in
   memory — a wrong-model transcription bug waiting to happen.
2. **`get_app_state` is the only lock access in the codebase that `.unwrap()`s.**
   One panic in any thread holding the state write lock would make this IPC
   command panic on every subsequent call, while every other access (including
   `state.rs` itself) recovers from poisoning.
3. **`start_mic_test` opens the microphone device twice** — once synchronously
   just to learn which device matched, then again in the worker thread that
   actually streams levels. On macOS this double-activates the mic indicator
   and adds latency to the settings/onboarding mic test.

## Current state

### Fix A — `src-tauri/src/transcribe.rs` (global transcriber)

`src-tauri/src/transcribe.rs:134-151`:

```rust
// Global transcriber instance (can be loaded/unloaded at runtime).
static TRANSCRIBER: Lazy<Mutex<Option<Arc<Transcriber>>>> = Lazy::new(|| Mutex::new(None));

/// Initialize the global transcriber with the given model file.
/// Safe to call multiple times.
pub fn init_transcriber(model_path: &str) -> Result<(), TranscribeError> {
    let mut guard = TRANSCRIBER
        .lock()
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber lock poisoned".to_string()))?;
    if guard.is_some() {
        return Ok(());
    }

    tracing::info!("Initializing transcriber from {}", model_path);
    *guard = Some(Arc::new(Transcriber::new(model_path)?));

    Ok(())
}
```

Callers of `init_transcriber` (do not change them): `src-tauri/src/lib.rs`
(`update_settings`, `complete_setup`, app setup) and `src-tauri/src/hotkey.rs`
(`init_transcriber_async`). All pass the path of the *active* model variant.
`TranscribeError` is defined in `src-tauri/src/engine.rs`.

### Fix B — `src-tauri/src/lib.rs` (`get_app_state`)

`src-tauri/src/lib.rs:92-96`:

```rust
#[tauri::command]
fn get_app_state() -> String {
    let state = state::APP_STATE.read().unwrap();
    state.as_str().to_string()
}
```

The poison-safe accessor already exists — `src-tauri/src/state.rs:74-82`
(`pub fn get_state() -> AppState`) recovers from poisoning and is used
everywhere else (e.g. `lib.rs:381`, `lib.rs:665`).

### Fix C — `src-tauri/src/lib.rs` (`start_mic_test`) + `src-tauri/src/audio.rs`

`src-tauri/src/lib.rs:210-219` — the probe that opens the device a first time:

```rust
    // Get device match info
    let mut capture = AudioCapture::new();
    if let Some(ref id) = device_id {
        capture.set_device(Some(id.clone()));
    }

    let match_result = match capture.start_mic_test() {
        Ok(result) => result,
        Err(e) => return Err(e.to_string()),
    };
    capture.stop_mic_test();
```

The worker thread spawned right after (`lib.rs:250-308`) creates its own
`AudioCapture` and calls `init_capture()` — that is the second (legitimate)
device open.

In `src-tauri/src/audio.rs`, the private method `fn get_device(&self) ->
Result<(Device, DeviceMatchResult), AudioError>` (`audio.rs:132`) already
resolves the device and computes the `DeviceMatchResult` (exact / fuzzy /
contains / default-fallback matching) **without opening a stream**. It is
private; Fix C exposes a thin public wrapper.

Repo conventions: IPC commands stay thin and return `Result<T, String>`
(`CLAUDE.md`); doc comments on public functions; existing tests live in
`#[cfg(test)] mod tests` at the bottom of each file.

## Commands you will need

| Purpose        | Command                                                  | Expected on success |
|----------------|----------------------------------------------------------|---------------------|
| Rust compile   | `cargo check --manifest-path src-tauri/Cargo.toml`       | exit 0              |
| Rust tests     | `cargo test --manifest-path src-tauri/Cargo.toml`        | all pass, exit 0    |

## Scope

**In scope** (the only files you should modify):
- `src-tauri/src/transcribe.rs`
- `src-tauri/src/lib.rs` (ONLY `get_app_state` and `start_mic_test`)
- `src-tauri/src/audio.rs` (ONLY adding the public device-resolution wrapper)

**Out of scope** (do NOT touch):
- Any other region of `lib.rs` — `update_hotkey`, the `run()` event loop, and
  the keyboard-device-observer setup have in-flight maintainer changes.
- `src-tauri/src/hotkey.rs` — plan 001's territory.
- `src-tauri/src/state.rs` — already correct; Fix B reuses it.
- The device-matching heuristics inside `audio.rs::get_device` — behavior
  must not change, only visibility via a wrapper.
- `src-tauri/src/hotkey_listener.rs`, `src-tauri/src/platform/macos.rs`,
  `src-tauri/tauri.conf.json` — uncommitted maintainer changes.

## Git workflow

- Branch: `advisor/003-small-hardening`
- One commit per fix (A, B, C), short imperative subjects, e.g.
  "Reload transcriber when the model path changes".
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1 (Fix A): Make the global transcriber path-aware

In `src-tauri/src/transcribe.rs`:

1. Change the global to store the path alongside the instance:

```rust
// Global transcriber instance and the model path it was loaded from
// (can be loaded/unloaded at runtime).
static TRANSCRIBER: Lazy<Mutex<Option<(String, Arc<Transcriber>)>>> =
    Lazy::new(|| Mutex::new(None));
```

2. Rewrite `init_transcriber` so a same-path call is still a no-op, but a
   different-path call replaces the loaded model:

```rust
/// Initialize the global transcriber with the given model file.
/// Safe to call multiple times; reloads if `model_path` differs from the
/// currently loaded model.
pub fn init_transcriber(model_path: &str) -> Result<(), TranscribeError> {
    let mut guard = TRANSCRIBER
        .lock()
        .map_err(|_| TranscribeError::ModelLoadFailed("Transcriber lock poisoned".to_string()))?;

    if let Some((loaded_path, _)) = guard.as_ref() {
        if loaded_path == model_path {
            return Ok(());
        }
        tracing::info!("Model path changed, reloading transcriber");
    }

    tracing::info!("Initializing transcriber from {}", model_path);
    *guard = Some((model_path.to_string(), Arc::new(Transcriber::new(model_path)?)));

    Ok(())
}
```

   Note on failure semantics: in the shape above, `Transcriber::new(...)?`
   fires *before* the assignment to `*guard`, so a failed reload leaves the
   previously loaded model in place and usable. That is the desired
   behavior — keep this ordering.

3. Update the accessors for the new tuple type:
   - `get_transcriber()` → `guard.as_ref().map(|(_, t)| Arc::clone(t))`
     (adjust for the `match` shape already there).
   - `is_transcriber_loaded()` and `unload_transcriber()` need no logic
     change, only whatever the compiler demands.

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0.

### Step 2 (Fix A): Add a reload test

In the existing `mod tests` of `transcribe.rs` (model the locking/reset
pattern on `transcribe_audio_requires_loaded_transcriber` at
`transcribe.rs:216-230`), add:

```rust
#[test]
fn init_transcriber_attempts_reload_when_path_changes() {
    let _guard = TRANSCRIBE_TEST_MUTEX
        .lock()
        .expect("transcribe test mutex should lock");
    let _reset = TranscriberReset;

    unload_transcriber();

    // Both paths are missing files, so both calls fail with ModelNotFound —
    // proving the second call did NOT short-circuit to Ok(()) the way a
    // path-blind guard would once something was loaded. The stronger
    // assertion (old model kept on failed reload) needs a real model file
    // and is intentionally not tested here.
    assert!(matches!(
        init_transcriber(&missing_model_path()),
        Err(TranscribeError::ModelNotFound)
    ));
    assert!(matches!(
        init_transcriber(&missing_model_path()),
        Err(TranscribeError::ModelNotFound)
    ));
    assert!(!is_transcriber_loaded());
}
```

**Verify**: `cargo test --manifest-path src-tauri/Cargo.toml transcribe` →
all `transcribe` tests pass, including the new one.

### Step 3 (Fix B): Route `get_app_state` through the poison-safe accessor

Replace the body of `get_app_state` in `src-tauri/src/lib.rs:92-96` with:

```rust
#[tauri::command]
fn get_app_state() -> String {
    state::get_state().as_str().to_string()
}
```

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0, and
`grep -n "APP_STATE.read().unwrap()" src-tauri/src/lib.rs` → no matches.

### Step 4 (Fix C): Expose stream-free device resolution in `audio.rs`

In `src-tauri/src/audio.rs`, directly below `set_device` (`audio.rs:128-130`),
add a public wrapper (keep `get_device` itself private):

```rust
/// Resolve the configured device without opening an audio stream.
/// Returns the same match result `init_capture` would produce.
pub fn resolve_device(&self) -> Result<DeviceMatchResult, AudioError> {
    self.get_device().map(|(_, match_result)| match_result)
}
```

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0.

### Step 5 (Fix C): Use it in `start_mic_test` instead of the open/close probe

In `src-tauri/src/lib.rs`, replace the probe excerpt shown in "Current state"
(lines 210-219) with:

```rust
    // Get device match info without opening the device; the worker thread
    // below performs the only real capture.
    let mut capture = AudioCapture::new();
    if let Some(ref id) = device_id {
        capture.set_device(Some(id.clone()));
    }

    let match_result = match capture.resolve_device() {
        Ok(result) => result,
        Err(e) => return Err(e.to_string()),
    };
```

Everything after (building `MicTestStartResult`, logging, generation bump,
worker-thread spawn) stays exactly as is.

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0, and
`grep -n "start_mic_test()" src-tauri/src/lib.rs` → no matches (the only
remaining callers of `AudioCapture::start_mic_test` are inside `audio.rs`
itself, if any).

### Step 6: Full test suite

**Verify**: `cargo test --manifest-path src-tauri/Cargo.toml` → all pass.

## Test plan

- New test (step 2): `init_transcriber_attempts_reload_when_path_changes` in
  `transcribe.rs`, modeled on the existing serialized-mutex test pattern there.
- Fixes B and C are covered structurally (greps in done criteria) and by the
  existing suite; they have no unit-testable logic without real devices.
- Verification: `cargo test --manifest-path src-tauri/Cargo.toml` → all pass,
  including 1 new test.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo check --manifest-path src-tauri/Cargo.toml` exits 0
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` exits 0 with the new
      `init_transcriber_attempts_reload_when_path_changes` test passing
- [ ] `grep -n ".unwrap()" src-tauri/src/lib.rs` shows no match on an
      `APP_STATE` lock
- [ ] `grep -rn "fn resolve_device" src-tauri/src/audio.rs` → one match
- [ ] In `start_mic_test`, no call pair `start_mic_test()` + `stop_mic_test()`
      remains before the worker-thread spawn
- [ ] Only `transcribe.rs`, `lib.rs`, `audio.rs` modified on the plan branch
      beyond the maintainer's pre-existing uncommitted changes
      (`git status --short`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any "Current state" excerpt no longer matches the live code — especially in
  `lib.rs`, where maintainer changes are in flight; if the `start_mic_test`
  or `get_app_state` regions themselves have changed, STOP.
- A caller of `init_transcriber` outside `lib.rs`/`hotkey.rs` appears, or any
  caller depends on the old "already loaded ⇒ always Ok" behavior in a way
  the type change breaks beyond mechanical fixes.
- Changing `start_mic_test` appears to require touching the worker-thread
  body or `MIC_TEST_STATE` handling.
- `cargo test` fails twice after a reasonable fix attempt.

## Maintenance notes

- Fix A makes hot model switching possible without an app restart; the
  restart-required flow in `set_active_model` (`lib.rs:366-390`) could be
  simplified later to call `init_transcriber` with the new path directly.
  Deferred: that is a product/UX change, not hardening.
- Reviewer should scrutinize: failed-reload semantics in Fix A (a failed
  reload must keep the previously loaded model usable — the `?` placement
  guarantees it), and that Fix C didn't alter device-matching behavior.
- If the maintainer's in-flight `lib.rs`/`hotkey_listener.rs` changes land
  first, only the drift check is affected; the edited regions are disjoint.
