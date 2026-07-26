# Plan 002: Add network timeouts to model downloads so they fail instead of hanging forever

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 0cdc229..HEAD -- src-tauri/src/model.rs src-tauri/Cargo.toml`
> If either file changed since this plan was written, compare the
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

Fing downloads whisper model files (190–574 MB) from Hugging Face during
onboarding and model switching. The download in `src-tauri/src/model.rs` uses
`reqwest::Client::new()`, which by default has **no connect timeout and no
read timeout**. If the connection stalls mid-download (flaky Wi-Fi, captive
portal, CDN hiccup), the byte-stream loop simply never yields again: the UI
stays on "downloading" forever, the progress bar freezes, and the only
recovery is restarting the app. With timeouts, a stalled download fails within
seconds through the *existing* error path, which already sets
`DownloadStatus::Failed`, cleans up the `.part` file, and lets the user retry
from the UI.

## Current state

- `src-tauri/src/model.rs` — model registry, download, and SHA-256
  verification. The client is built in `download_variant`.
- `src-tauri/Cargo.toml:34` — `reqwest = { version = "0.13", features = ["json", "stream"] }`
  (lockfile has 0.13.3, which supports `ClientBuilder::connect_timeout` and
  `ClientBuilder::read_timeout`).

The client construction as it exists today (`src-tauri/src/model.rs:498-508`):

```rust
    // Download
    let client = Client::new();
    tracing::info!("Fetching model from {}", def.url);

    let response = client.get(def.url).send().await.map_err(|e| {
        let err_msg = format!("Network error: {e}");
        tracing::error!("{}", err_msg);
        let mut state = lock_download_state();
        state.status = DownloadStatus::Failed(err_msg.clone());
        err_msg
    })?;
```

Every failure path in `download_variant` already sets
`DownloadStatus::Failed(msg)` and removes the `.part` file where applicable —
a timeout error from reqwest will surface through the existing
`chunk.map_err(...)` / `send().await.map_err(...)` handlers with no extra
plumbing.

Important: do NOT use `ClientBuilder::timeout(...)` (the *total request
deadline*). These downloads legitimately take many minutes on slow links; a
total deadline would break working downloads. Use `connect_timeout` plus
`read_timeout` (idle time between reads) only.

Repo conventions: errors as `Result<T, String>`; user-visible failure text
stays short ("Network error: ..."); constants in SCREAMING_SNAKE_CASE near the
top of the module (see `HASH_CACHE_MAX_ENTRIES` at `model.rs:223`).

## Commands you will need

| Purpose        | Command                                                  | Expected on success |
|----------------|----------------------------------------------------------|---------------------|
| Rust compile   | `cargo check --manifest-path src-tauri/Cargo.toml`       | exit 0              |
| Rust tests     | `cargo test --manifest-path src-tauri/Cargo.toml`        | all pass, exit 0    |

## Scope

**In scope** (the only file you should modify):
- `src-tauri/src/model.rs`

**Out of scope** (do NOT touch):
- `src-tauri/Cargo.toml` / `Cargo.lock` — no new dependencies or feature flags
  are needed; `connect_timeout`/`read_timeout` are core reqwest API.
- `src-tauri/src/update.rs` — the app updater uses the tauri updater plugin's
  own HTTP stack; not this code path.
- The download progress/state machinery and verification logic in `model.rs` —
  unchanged.
- `src-tauri/src/lib.rs`, `src-tauri/src/hotkey_listener.rs`,
  `src-tauri/src/platform/macos.rs`, `src-tauri/tauri.conf.json` — carry
  uncommitted maintainer changes; do not modify.

## Git workflow

- Branch: `advisor/002-model-download-timeouts`
- Single commit, short imperative subject (e.g. "Add timeouts to model
  downloads").
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add timeout constants

Near the other module constants (just below `HASH_CACHE_MAX_ENTRIES` at
`src-tauri/src/model.rs:223`), add:

```rust
/// Abort if a connection can't be established within this window.
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Abort if the byte stream goes idle for this long mid-download.
const DOWNLOAD_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
```

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0
(unused-const warnings are fine at this intermediate step).

### Step 2: Build the client with timeouts

In `download_variant` (`src-tauri/src/model.rs:499`), replace:

```rust
    let client = Client::new();
```

with:

```rust
    let client = Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .build()
        .map_err(|e| {
            let err_msg = format!("Failed to create download client: {e}");
            tracing::error!("{}", err_msg);
            let mut state = lock_download_state();
            state.status = DownloadStatus::Failed(err_msg.clone());
            err_msg
        })?;
```

(The error-handling shape mirrors every other failure path in this function:
log, set `DownloadStatus::Failed`, return the message.)

**Verify**: `cargo check --manifest-path src-tauri/Cargo.toml` → exit 0, no
warnings about the new constants.

### Step 3: Run the Rust test suite

**Verify**: `cargo test --manifest-path src-tauri/Cargo.toml` → all pass.

## Test plan

No new unit test: reqwest builder configuration isn't meaningfully unit-testable
without a network harness, and the repo has no HTTP-mocking infrastructure —
do not add one for this. Existing `model.rs` tests (verification, hashing,
download-status strings) must keep passing. The structural done-criterion
below proves the configuration is in place.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo check --manifest-path src-tauri/Cargo.toml` exits 0
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` exits 0
- [ ] `grep -n "Client::new()" src-tauri/src/model.rs` returns no matches
- [ ] `grep -n "connect_timeout\|read_timeout" src-tauri/src/model.rs` shows
      both configured on the builder
- [ ] `grep -n "\.timeout(" src-tauri/src/model.rs` returns no matches (no
      total-request deadline was added)
- [ ] On the plan branch, only `src-tauri/src/model.rs` is modified beyond the
      maintainer's pre-existing uncommitted changes (`git status --short`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `read_timeout` does not exist on `ClientBuilder` in the resolved reqwest
  version (compile error in step 2). Report the version and error; do not
  substitute `.timeout(...)`.
- `download_variant` no longer matches the "Current state" excerpt (drift).
- The change appears to require touching `Cargo.toml`.

## Maintenance notes

- If download *resume* (HTTP Range) support is ever added, these timeouts are
  the trigger that makes resume valuable — a timed-out partial `.part` file
  is currently deleted; resume would keep it.
- Reviewer should scrutinize: that no total-request `timeout(...)` snuck in
  (it would cap large downloads on slow links), and the timeout values
  (30 s connect / 60 s idle are deliberately generous).
- Deferred: retry-with-backoff on transient failures — the UI retry button
  covers this; automatic retry is product behavior the maintainer should
  decide on.
