# whisper.cpp in Fing

Fing transcribes with [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
through a small binding it owns. The same library runs two model families:
Whisper (three registry models) and Parakeet TDT 0.6B v3.

## How the binding is set up

Three layers, from upstream to app:

1. **Upstream source:** `src-tauri/crates/whisper-sys/whisper.cpp` is a
   git submodule pinned to a release tag (`v1.9.5`, commit `d1be6fde`). It is
   unmodified upstream code; Fing carries no patches.
2. **`whisper-sys`** (`build.rs`):
   - Builds whisper.cpp with CMake as static libraries, always the Release
     config (a debug ggml is far too slow, and RelWithDebInfo breaks the MSVC
     static CRT link).
   - Configures each cargo target itself (the universal macOS build compiles
     the crate once per architecture):
     - macOS arm64: Metal with the shader library embedded, Accelerate BLAS.
     - macOS x86_64: CPU with Accelerate BLAS, no Metal; AVX/F16C, no AVX2
       (macOS 11 still runs on the 2013 Mac Pro).
     - Windows: Vulkan (import library from `VULKAN_SDK`), AVX2/FMA/F16C,
       `/utf-8`, static CRT (`/MT`, matching `.cargo/config.toml`).
     - Always `GGML_NATIVE=OFF`, no OpenMP, no ccache: the build machine's
       CPU never decides the instruction set.
   - Links `whisper`, `parakeet`, `ggml`, `ggml-base`, `ggml-cpu` plus the
     backend libraries and system frameworks. No dylibs or DLLs are bundled.
   - Runs bindgen over `whisper.h` and `parakeet.h` on every build. The
     allowlist is `whisper_*`, `parakeet_*`, `ggml_backend_*` and the ggml log
     types. Nothing generated is committed, so the Rust declarations always
     match the checked-out headers.
   - `WHISPER_CPP_VERSION` is read from upstream's `CMakeLists.txt`.
3. **`src-tauri/src/whisper_cpp.rs`:** the hand-written safe layer.
   - RAII owners: `WhisperContext`, `WhisperState`, `FullParams`,
     `ParakeetContext`.
   - Wraps only the calls Fing uses: init, state, mel, language detection,
     `full` (with an abort flag), segments, tokenize, log callback, Parakeet
     init/full/segments.
   - Everything above it (`transcribe.rs`, `inference.rs`) uses only this
     module, except the Windows device enumeration in `inference.rs`, which
     calls `ggml_backend_*` directly.

**Build requirements:** CMake, a C/C++ toolchain, libclang (for bindgen), and
the submodule (`git submodule update --init --recursive`). Windows also needs
the Vulkan SDK. `src-tauri/.taurignore` keeps `tauri dev` from watching the
submodule: CMake configure rewrites `bindings/javascript/package.json` there,
which otherwise causes an endless rebuild loop.

## Engines

`engine.rs` defines the `TranscriptionEngine` trait; `transcribe.rs` holds one
`Arc<dyn TranscriptionEngine>`:

- **`WhisperEngine`:** forced language or detection among the selected
  languages (Cantonese excluded), the dictionary as a prompt of up to 256
  tokens, flash attention on, greedy decoding with timestamps (avoid
  `no_timestamps`: it translated a 30 s German window in one test).
- **`ParakeetEngine`:** ignores the language settings (it detects among its 25
  European languages) and has no prompt. The post-hoc dictionary correction
  still runs on its output.

Model switches hot-swap the engine without a restart (`model_loader.rs`); a
switch is rejected while Recording or Processing.

## Pre-transcription

`src-tauri/src/pretranscribe.rs`. While recording, a worker thread transcribes
finished chunks of the audio, so on release only the tail is left. There is no
UI; the kill switch is `PRETRANSCRIBE_ENABLED`.

- **Worker:** started with the level meter, bound to that recording's
  `AudioTap` and session id. Every 500 ms it copies the new native-rate
  samples, runs the planner, resamples each chunk with
  `audio::resample_slice_to_16k` and transcribes it. It never loads or unloads
  a model.
- **Release:** `on_key_up` stops the worker before `StopRecording`, which
  returns the recording at its native rate; the task joins the worker,
  resamples and transcribes only the tail and joins the texts. Empty and
  annotation-only chunk texts are dropped. Dictionary corrections and the blank
  filter run once on the joined text.
- **Fallback to one full pass:** no committed chunks, or the languages,
  dictionary prompt or loaded engine changed during the recording.
- **Language (Whisper, several languages):** the language detected on the
  first chunk is pinned for later chunks and the tail.

Planner thresholds (energy VAD, 30 ms frames):

| Constant | Value |
|---|---|
| Noise floor | 10th percentile of frame energy |
| Active frame | > floor + 10 dB, or > −35 dBFS |
| Pause for a cut | ≥ 600 ms, closed by ≥ 90 ms of activity; cut at its quietest frame |
| Min uncommitted audio before a cut | 12 s |
| Hard cap | 25 s: cut in the longest closed shorter pause in 12–25 s, else the quietest frame in 22–25 s |
| Min activity in a chunk | 500 ms |
| Give up (no cut possible) | 60 s uncommitted |

Pauses must be closed before a cut: a pause still in progress is where users
release, and a tail of room noise alone invites Whisper hallucinations.

## Cancel during Processing

Escape cancels a recording and also a transcription in progress, through
whisper.cpp's `abort_callback` on both engines. `on_key_down` creates one
`AbortFlag` per recording; the pre-transcription worker, the release pass and
`whisper_cpp.rs` share it. The cancel window closes just before the paste
(`claim_for_paste`); after that, Escape reaches the focused app.

whisper.cpp checks the flag between graphs. Whisper stops within a few ms;
Parakeet's encoder is one graph, so on the CPU it stops only after the encoder
(interrupting it would need an upstream patch).

## Blank filter

Recordings with a peak ≤ 0.001 are skipped before any engine runs (Whisper
writes `[Music]`, "you" or "Thank you." on digital silence); for parts of a
recording, `transcribe_audio_part` returns empty text instead. `is_blank_audio`
(`hotkey.rs`) drops text made only of `[…]` / `(…)` annotations and `♪`.
Parakeet returns empty text on noise.

## Measurements

M5 Pro, Metal, synthetic `say` clips (voices Samantha / Anna), one process per
model, flash attention on (2026-10-22). Latency and smoke only, not a quality
comparison.

| | Whisper Small Q5 | Whisper Small | Whisper Large (Turbo Q5) | Parakeet |
|---|---|---|---|---|
| Load, warm | 760 ms | 790 ms | 800 ms | 750 ms |
| EN 4.8 s / 11.5 s / 60.5 s | 87 / 155 / 785 ms | 97 / 178 / 816 ms | 210 / 249 / 1921 ms | 49 / 83 / 414 ms |
| DE 4.8 s / 14.7 s / 73.3 s, `[de]` | 100 / 174 / 1012 ms | 109 / 201 / 1021 ms | 217 / 265 / 1445 ms | 46 / 90 / 475 ms |
| DE same, `[en, de]` | 139 / 216 / 867 ms | 148 / 241 / 988 ms | 394 / 445 / 1170 ms | ignores languages |
| Release with pre-transcription, EN 60.5 s | 72–279 ms | 77–112 ms | 279–459 ms | 138 ms |
| Process RSS after load / runs | 344 / 362 MB | 676 / 694 MB | 740 / 757 MB | 570 / 585 MB |

- All four detect German correctly with `[en, de]`. Whisper Small and Small Q5
  translate German audio to English when forced to `[en]`; Large Turbo and
  Parakeet don't.
- Abort (`abort_smoke`, 73 s clip): Whisper returns 0–4 ms after the flag is
  set, Parakeet on Metal ≤ 123 ms. Text after aborts is identical to the
  baseline.
- `memory_estimate_mb` in `model.rs` feeds the Windows device-fit check, which
  adds max(25 %, 256 MB) of headroom; the measured RSS stays inside it. Metal
  buffers don't count toward RSS; Vulkan memory still needs measuring on
  Windows.
- Flash attention made Whisper Small 13–25 % faster on Metal with identical
  text (one punctuation change); on the CPU it is roughly neutral.

Run the smoke tests (they print transcripts, so use your own audio):

```
FING_SMOKE_MODELS=<models dir> FING_SMOKE_CLIPS=<clips dir> \
  bun run scripts/cargo-test.ts engine_smoke -- --ignored --nocapture --test-threads=1
```

`engine_smoke::engine_smoke`, `pretranscribe_smoke`, `silence_smoke` and
`abort_smoke` take the same variables. Clips are 16 kHz mono 16-bit WAV; a
clip named `de_*` also runs with `[de]`, `[en]` and `[en, de]`.
`FING_SMOKE_TERMS` is an optional comma-separated dictionary,
`FING_SMOKE_CPU=1` loads the models without the GPU (macOS).

## Updating whisper.cpp

```bash
git -C src-tauri/crates/whisper-sys/whisper.cpp fetch --tags
git -C src-tauri/crates/whisper-sys/whisper.cpp checkout v1.9.6   # new tag
bun run verify
git add src-tauri/crates/whisper-sys/whisper.cpp
git commit -m "chore(deps): whisper.cpp v1.9.6"
```

- **Automatic:** struct layouts, enum values and new functions (bindings are
  regenerated from the new headers). The version constant follows
  `CMakeLists.txt`.
- **Compile errors (good):** a renamed function or changed signature fails the
  build in `whisper_cpp.rs`, the one place to fix.
- **Check by hand on every bump:**
  - Default parameter semantics in `whisper_full_default_params`,
    `whisper_context_default_params` and `parakeet_full_default_params`.
    `whisper_cpp.rs` sets `flash_attn = true` explicitly.
  - The backend log markers ("using … backend", "failed to initialize", "no
    GPU found") that Windows device verification relies on, for Whisper and
    Parakeet. `observer_contract_matches_pinned_whisper_cpp_markers` in
    `inference.rs` also asserts the version, so it fails on purpose after a
    bump: re-check the markers in `src/whisper.cpp` and `src/parakeet.cpp`,
    then update the expected version.
  - The Parakeet API; it is new upstream and the most likely to change.
  - CMake option names in `build.rs` (`WHISPER_*` / `GGML_*`).
- **Afterwards:** run the smoke tests on all four models and compare with the
  table above.

## Rejected features

Measured and rejected. Don't re-add them without new numbers.

- **Silero VAD trimming** (both engines). The VAD was accurate, but it costs
  +885 KB of binary and ~1.3 ms per second of audio. Whisper always encodes
  whole 30 s windows, so trimming saves nothing; on Parakeet the saving is
  about the VAD's own cost. Text changed only in punctuation. Noise-only
  recordings got faster, but the blank filter already drops them.
- **`suppress_nst`** (Whisper). On noise it removes the bracketed markers, so
  Whisper writes real words instead ("Thank you.", "Bye."), which would be
  pasted. It also suppresses punctuation dictation needs: `john@example.com`
  became `johnetteexample.com`, `and/or` became `and or`.
- **No-speech probability filter** (Whisper).
  `whisper_full_get_segment_no_speech_prob` read 0.00 on every clip, pure
  noise included.
- **Previous-chunk context in the Whisper prompt** (pre-transcription). Adding
  the last 200 characters of the previous chunk didn't fix the chunked word
  errors.

## Suggested next features

Ordered by value for a dictation app relative to effort:

1. **Better dictionary for Parakeet:** phonetic post-hoc matching (Double
   Metaphone, Cologne phonetics for German) instead of spelling distance only;
   long term, token biasing via `parakeet_get_logits` (needs upstream work).
2. **Streaming preview:** decode while the key is held, so release-to-text is
   near zero for long dictations (`new_segment_callback`, `parakeet_chunk`
   with kept state).
3. **Hallucination guard for long holds:** check repetition and compression
   ratio on the output; token probabilities are reachable now.
4. **Model-in-memory policy:** expose memory per model and consider an idle
   timeout longer than 10 s as an option.
5. **Automatic engine choice by language:** today Fing only suggests Whisper
   when a selected language is outside Parakeet's 25.
