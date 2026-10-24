// Thin safe wrapper over whisper-sys: only the parts of whisper.cpp
// (whisper + parakeet) that Fing uses.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use whisper_sys as sys;

pub use sys::WHISPER_CPP_VERSION;

#[derive(Debug, Clone)]
pub enum FfiError {
    Failed(String),
    /// The abort flag passed to the call was set.
    Aborted,
}

impl fmt::Display for FfiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FfiError::Failed(message) => f.write_str(message),
            FfiError::Aborted => f.write_str("aborted"),
        }
    }
}

impl std::error::Error for FfiError {}

fn err(message: impl Into<String>) -> FfiError {
    FfiError::Failed(message.into())
}

/// The error for a failed `full` call: an abort makes whisper.cpp fail
/// with an ordinary error code, so the flag tells the two apart.
fn full_error(name: &str, code: c_int, abort: &AtomicBool) -> FfiError {
    if abort.load(Ordering::SeqCst) {
        FfiError::Aborted
    } else {
        err(format!("{name} failed ({code})"))
    }
}

/// Abort callback for both whisper and parakeet; `data` is the `AtomicBool`
/// passed to `full`. Checked after every encoder run and decoder step.
unsafe extern "C" fn abort_requested(data: *mut c_void) -> bool {
    unsafe { &*data.cast::<AtomicBool>() }.load(Ordering::Relaxed)
}

fn abort_user_data(abort: &AtomicBool) -> *mut c_void {
    (abort as *const AtomicBool).cast_mut().cast()
}

fn path_cstring(path: &Path) -> Result<CString, FfiError> {
    let path = path
        .to_str()
        .ok_or_else(|| err("model path is not valid UTF-8"))?;
    CString::new(path).map_err(|_| err("model path contains a NUL byte"))
}

fn n_samples(audio: &[f32]) -> Result<c_int, FfiError> {
    c_int::try_from(audio.len()).map_err(|_| err("audio buffer too long"))
}

fn lossy(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Whisper language id for a code like "en", or None if unknown.
pub fn lang_id(language: &str) -> Option<i32> {
    let language = CString::new(language).ok()?;
    let id = unsafe { sys::whisper_lang_id(language.as_ptr()) };
    (id >= 0).then_some(id)
}

/// Largest whisper language id.
fn lang_max_id() -> i32 {
    unsafe { sys::whisper_lang_max_id() }
}

pub type LogCallback =
    unsafe extern "C" fn(level: sys::ggml_log_level, text: *const c_char, user_data: *mut c_void);

pub use sys::{
    ggml_log_level, ggml_log_level_GGML_LOG_LEVEL_ERROR, ggml_log_level_GGML_LOG_LEVEL_INFO,
    ggml_log_level_GGML_LOG_LEVEL_WARN,
};

/// Route whisper.cpp, parakeet and ggml logs to `callback` (its `user_data`
/// is null).
///
/// # Safety
/// `callback` must be safe to call from any thread.
pub unsafe fn set_log_callback(callback: LogCallback) {
    unsafe {
        sys::whisper_log_set(Some(callback), std::ptr::null_mut());
        sys::parakeet_log_set(Some(callback), std::ptr::null_mut());
    }
}

// ---------------------------------------------------------------------------
// Stage timing
// ---------------------------------------------------------------------------

/// Where the time of one `full` call went, from whisper.cpp callbacks (the
/// per-state counters behind `whisper_get_timings` are not exposed for the
/// separate states Fing uses).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stages {
    /// Mel spectrogram, up to the first encoder run.
    pub mel_ms: u64,
    /// Encoder runs, each up to its first decoder step (so this includes the
    /// prompt prefill). With no decoder step at all, the rest of the call.
    pub encode_ms: u64,
    /// Everything else: decoding, temperature fallbacks, segment output.
    pub decode_ms: u64,
}

/// Collects stage boundaries; the callbacks get a pointer to it as user data
/// and run on the calling thread while `full` runs.
struct StageClock {
    started: Instant,
    first_encode: Option<Instant>,
    /// Start of the current encoder run, until its first decoder step.
    encode_start: Option<Instant>,
    encode: Duration,
}

impl StageClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            first_encode: None,
            encode_start: None,
            encode: Duration::ZERO,
        }
    }

    fn encoder_begin(&mut self) {
        let now = Instant::now();
        self.end_encode(now);
        self.first_encode.get_or_insert(now);
        self.encode_start = Some(now);
    }

    fn end_encode(&mut self, now: Instant) {
        if let Some(start) = self.encode_start.take() {
            self.encode += now - start;
        }
    }

    fn finish(mut self) -> Stages {
        let now = Instant::now();
        self.end_encode(now);
        let total = now - self.started;
        let mel = self.first_encode.map_or(total, |at| at - self.started);
        Stages {
            mel_ms: mel.as_millis() as u64,
            encode_ms: self.encode.as_millis() as u64,
            decode_ms: total.saturating_sub(mel + self.encode).as_millis() as u64,
        }
    }

    fn user_data(&mut self) -> *mut c_void {
        (self as *mut Self).cast()
    }
}

unsafe fn clock<'a>(user_data: *mut c_void) -> &'a mut StageClock {
    unsafe { &mut *user_data.cast::<StageClock>() }
}

/// Encoder-begin callback for both whisper and parakeet.
unsafe extern "C" fn encoder_begin<Ctx, State>(
    _ctx: *mut Ctx,
    _state: *mut State,
    user_data: *mut c_void,
) -> bool {
    unsafe { clock(user_data) }.encoder_begin();
    true
}

unsafe extern "C" fn whisper_logits_filter(
    _ctx: *mut sys::whisper_context,
    _state: *mut sys::whisper_state,
    _tokens: *const sys::whisper_token_data,
    _n_tokens: c_int,
    _logits: *mut f32,
    user_data: *mut c_void,
) {
    unsafe { clock(user_data) }.end_encode(Instant::now());
}

unsafe extern "C" fn parakeet_new_token(
    _ctx: *mut sys::parakeet_context,
    _state: *mut sys::parakeet_state,
    _token_data: *const sys::parakeet_token_data,
    user_data: *mut c_void,
) {
    unsafe { clock(user_data) }.end_encode(Instant::now());
}

// ---------------------------------------------------------------------------
// Whisper
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ContextParams {
    pub use_gpu: bool,
    pub gpu_device: i32,
}

impl Default for ContextParams {
    fn default() -> Self {
        Self {
            use_gpu: true,
            gpu_device: 0,
        }
    }
}

pub struct WhisperContext {
    ptr: NonNull<sys::whisper_context>,
}

// whisper_context is immutable after load; per-call mutable data lives in whisper_state.
unsafe impl Send for WhisperContext {}
unsafe impl Sync for WhisperContext {}

impl WhisperContext {
    pub fn new(model_path: &Path, params: ContextParams) -> Result<Self, FfiError> {
        let path = path_cstring(model_path)?;
        let mut raw = unsafe { sys::whisper_context_default_params() };
        raw.use_gpu = params.use_gpu;
        raw.gpu_device = params.gpu_device;
        // Explicit so a whisper.cpp bump can't silently change it. Faster on
        // Metal and lighter on CPU; Whisper only (Parakeet does not use it).
        raw.flash_attn = true;
        let ptr = unsafe { sys::whisper_init_from_file_with_params_no_state(path.as_ptr(), raw) };
        NonNull::new(ptr)
            .map(|ptr| Self { ptr })
            .ok_or_else(|| err("whisper_init_from_file_with_params_no_state failed"))
    }

    pub fn create_state(&self) -> Result<WhisperState<'_>, FfiError> {
        let ptr = unsafe { sys::whisper_init_state(self.ptr.as_ptr()) };
        NonNull::new(ptr)
            .map(|ptr| WhisperState { ctx: self, ptr })
            .ok_or_else(|| err("whisper_init_state failed"))
    }

    pub fn tokenize(&self, text: &str, max_tokens: usize) -> Result<Vec<i32>, FfiError> {
        let text = CString::new(text).map_err(|_| err("prompt contains a NUL byte"))?;
        let mut tokens = vec![0 as sys::whisper_token; max_tokens];
        let max = c_int::try_from(max_tokens).map_err(|_| err("max_tokens too large"))?;
        let count = unsafe {
            sys::whisper_tokenize(self.ptr.as_ptr(), text.as_ptr(), tokens.as_mut_ptr(), max)
        };
        if count < 0 {
            return Err(err(format!(
                "prompt needs {} tokens (max {max_tokens})",
                -count
            )));
        }
        tokens.truncate(count as usize);
        Ok(tokens)
    }
}

impl Drop for WhisperContext {
    fn drop(&mut self) {
        unsafe { sys::whisper_free(self.ptr.as_ptr()) };
    }
}

pub struct WhisperState<'ctx> {
    ctx: &'ctx WhisperContext,
    ptr: NonNull<sys::whisper_state>,
}

impl WhisperState<'_> {
    pub fn pcm_to_mel(&mut self, audio: &[f32], threads: usize) -> Result<(), FfiError> {
        let code = unsafe {
            sys::whisper_pcm_to_mel_with_state(
                self.ctx.ptr.as_ptr(),
                self.ptr.as_ptr(),
                audio.as_ptr(),
                n_samples(audio)?,
                threads as c_int,
            )
        };
        if code != 0 {
            return Err(err(format!("whisper_pcm_to_mel failed ({code})")));
        }
        Ok(())
    }

    /// Per-language probabilities (indexed by language id) for the start of
    /// the mel already in the state.
    pub fn lang_probabilities(&mut self, threads: usize) -> Result<Vec<f32>, FfiError> {
        let mut probabilities = vec![0.0_f32; (lang_max_id() + 1).max(0) as usize];
        let id = unsafe {
            sys::whisper_lang_auto_detect_with_state(
                self.ctx.ptr.as_ptr(),
                self.ptr.as_ptr(),
                0,
                threads as c_int,
                probabilities.as_mut_ptr(),
            )
        };
        if id < 0 {
            return Err(err(format!("whisper_lang_auto_detect failed ({id})")));
        }
        Ok(probabilities)
    }

    /// Run `whisper_full`. Setting `abort` stops it after the current
    /// encoder run or decoder step with [`FfiError::Aborted`].
    pub fn full(
        &mut self,
        params: &FullParams<'_>,
        audio: &[f32],
        abort: &AtomicBool,
    ) -> Result<Stages, FfiError> {
        let n_samples = n_samples(audio)?;
        let mut clock = StageClock::new();
        let mut raw = params.raw;
        raw.encoder_begin_callback =
            Some(encoder_begin::<sys::whisper_context, sys::whisper_state>);
        raw.encoder_begin_callback_user_data = clock.user_data();
        raw.logits_filter_callback = Some(whisper_logits_filter);
        raw.logits_filter_callback_user_data = clock.user_data();
        raw.abort_callback = Some(abort_requested);
        raw.abort_callback_user_data = abort_user_data(abort);
        let code = unsafe {
            sys::whisper_full_with_state(
                self.ctx.ptr.as_ptr(),
                self.ptr.as_ptr(),
                raw,
                audio.as_ptr(),
                n_samples,
            )
        };
        if code != 0 {
            return Err(full_error("whisper_full", code, abort));
        }
        Ok(clock.finish())
    }

    pub fn full_n_segments(&self) -> i32 {
        unsafe { sys::whisper_full_n_segments_from_state(self.ptr.as_ptr()) }
    }

    pub fn segment_text_lossy(&self, segment: i32) -> String {
        lossy(unsafe { sys::whisper_full_get_segment_text_from_state(self.ptr.as_ptr(), segment) })
    }
}

impl Drop for WhisperState<'_> {
    fn drop(&mut self) {
        unsafe { sys::whisper_free_state(self.ptr.as_ptr()) };
    }
}

/// Greedy `whisper_full` parameters for dictation: nothing printed, multiple
/// segments, context kept across windows. Borrows the language and prompt
/// tokens it points at.
pub struct FullParams<'a> {
    raw: sys::whisper_full_params,
    _language: Option<CString>,
    _tokens: std::marker::PhantomData<&'a [i32]>,
}

impl<'a> FullParams<'a> {
    pub fn greedy() -> Self {
        let mut raw = unsafe {
            sys::whisper_full_default_params(sys::whisper_sampling_strategy_WHISPER_SAMPLING_GREEDY)
        };
        raw.greedy.best_of = 1;
        raw.print_special = false;
        raw.print_progress = false;
        raw.print_realtime = false;
        raw.print_timestamps = false;
        raw.single_segment = false;
        raw.no_context = false;
        Self {
            raw,
            _language: None,
            _tokens: std::marker::PhantomData,
        }
    }

    pub fn set_language(&mut self, language: &str) {
        let language = CString::new(language).ok();
        self.raw.language = language
            .as_ref()
            .map_or(std::ptr::null(), |language| language.as_ptr());
        // The CString's heap buffer does not move when the owner is moved into self.
        self._language = language;
    }

    pub fn set_tokens(&mut self, tokens: &'a [i32]) {
        self.raw.prompt_tokens = tokens.as_ptr();
        self.raw.prompt_n_tokens = tokens.len() as c_int;
    }
}

// ---------------------------------------------------------------------------
// Parakeet
// ---------------------------------------------------------------------------

pub struct ParakeetContext {
    ctx: NonNull<sys::parakeet_context>,
    state: NonNull<sys::parakeet_state>,
}

// The context and its single state are only touched through `&mut self`;
// the engine keeps this behind a Mutex.
unsafe impl Send for ParakeetContext {}
unsafe impl Sync for ParakeetContext {}

impl ParakeetContext {
    pub fn new(model_path: &Path, params: ContextParams) -> Result<Self, FfiError> {
        let path = path_cstring(model_path)?;
        let mut raw = unsafe { sys::parakeet_context_default_params() };
        raw.use_gpu = params.use_gpu;
        raw.gpu_device = params.gpu_device;
        let ctx = unsafe { sys::parakeet_init_from_file_with_params_no_state(path.as_ptr(), raw) };
        let ctx = NonNull::new(ctx)
            .ok_or_else(|| err("parakeet_init_from_file_with_params_no_state failed"))?;
        let state = unsafe { sys::parakeet_init_state(ctx.as_ptr()) };
        let Some(state) = NonNull::new(state) else {
            unsafe { sys::parakeet_free(ctx.as_ptr()) };
            return Err(err("parakeet_init_state failed"));
        };
        Ok(Self { ctx, state })
    }

    /// Run encoder + TDT decoder over the whole clip and join the segment texts.
    /// Clips up to the model's audio context (~50 s) take the single-chunk path.
    /// Setting `abort` stops it after the current encoder run or decoder step
    /// with [`FfiError::Aborted`]; the state is reset on the next call.
    pub fn transcribe(
        &mut self,
        audio: &[f32],
        threads: usize,
        abort: &AtomicBool,
    ) -> Result<(String, Stages), FfiError> {
        let n_samples = n_samples(audio)?;
        let mut params = unsafe {
            sys::parakeet_full_default_params(
                sys::parakeet_sampling_strategy_PARAKEET_SAMPLING_GREEDY,
            )
        };
        params.n_threads = threads as c_int;
        params.no_context = true;

        let mut clock = StageClock::new();
        params.encoder_begin_callback =
            Some(encoder_begin::<sys::parakeet_context, sys::parakeet_state>);
        params.encoder_begin_callback_user_data = clock.user_data();
        params.new_token_callback = Some(parakeet_new_token);
        params.new_token_callback_user_data = clock.user_data();
        params.abort_callback = Some(abort_requested);
        params.abort_callback_user_data = abort_user_data(abort);

        let code = unsafe {
            sys::parakeet_full_with_state(
                self.ctx.as_ptr(),
                self.state.as_ptr(),
                params,
                audio.as_ptr(),
                n_samples,
            )
        };
        if code != 0 {
            return Err(full_error("parakeet_full", code, abort));
        }
        let stages = clock.finish();

        let segments = unsafe { sys::parakeet_full_n_segments_from_state(self.state.as_ptr()) };
        let mut text = String::new();
        for segment in 0..segments {
            text.push_str(&lossy(unsafe {
                sys::parakeet_full_get_segment_text_from_state(self.state.as_ptr(), segment)
            }));
        }
        Ok((text, stages))
    }
}

impl Drop for ParakeetContext {
    fn drop(&mut self) {
        unsafe {
            sys::parakeet_free_state(self.state.as_ptr());
            sys::parakeet_free(self.ctx.as_ptr());
        }
    }
}
