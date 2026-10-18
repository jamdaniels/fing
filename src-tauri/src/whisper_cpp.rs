// Thin safe wrapper over fing-whisper-sys: only the parts of whisper.cpp
// (whisper + parakeet) that Fing uses.

use fing_whisper_sys as sys;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;

pub use sys::WHISPER_CPP_VERSION;

#[derive(Debug, Clone)]
pub struct FfiError(String);

impl fmt::Display for FfiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FfiError {}

fn err(message: impl Into<String>) -> FfiError {
    FfiError(message.into())
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
pub fn lang_max_id() -> i32 {
    unsafe { sys::whisper_lang_max_id() }
}

pub type LogCallback =
    unsafe extern "C" fn(level: sys::ggml_log_level, text: *const c_char, user_data: *mut c_void);

pub use sys::{
    ggml_log_level, ggml_log_level_GGML_LOG_LEVEL_ERROR, ggml_log_level_GGML_LOG_LEVEL_WARN,
};

/// Route whisper.cpp, parakeet and ggml logs to `callback`.
///
/// # Safety
/// `callback` must be safe to call from any thread with any `user_data` passed here.
pub unsafe fn set_log_callback(callback: Option<LogCallback>, user_data: *mut c_void) {
    unsafe {
        sys::whisper_log_set(callback, user_data);
        sys::parakeet_log_set(callback, user_data);
    }
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

    /// Detect the language of the mel already in the state. Returns the best
    /// language id and the per-language probabilities (indexed by id).
    pub fn lang_detect(
        &mut self,
        offset_ms: usize,
        threads: usize,
    ) -> Result<(i32, Vec<f32>), FfiError> {
        let mut probabilities = vec![0.0_f32; (lang_max_id() + 1).max(0) as usize];
        let id = unsafe {
            sys::whisper_lang_auto_detect_with_state(
                self.ctx.ptr.as_ptr(),
                self.ptr.as_ptr(),
                offset_ms as c_int,
                threads as c_int,
                probabilities.as_mut_ptr(),
            )
        };
        if id < 0 {
            return Err(err(format!("whisper_lang_auto_detect failed ({id})")));
        }
        Ok((id, probabilities))
    }

    pub fn full(&mut self, params: &FullParams<'_>, audio: &[f32]) -> Result<(), FfiError> {
        let code = unsafe {
            sys::whisper_full_with_state(
                self.ctx.ptr.as_ptr(),
                self.ptr.as_ptr(),
                params.raw,
                audio.as_ptr(),
                n_samples(audio)?,
            )
        };
        if code != 0 {
            return Err(err(format!("whisper_full failed ({code})")));
        }
        Ok(())
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

/// Greedy `whisper_full` parameters. Borrows the language and prompt tokens it points at.
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

    pub fn set_print_special(&mut self, value: bool) {
        self.raw.print_special = value;
    }

    pub fn set_print_progress(&mut self, value: bool) {
        self.raw.print_progress = value;
    }

    pub fn set_print_realtime(&mut self, value: bool) {
        self.raw.print_realtime = value;
    }

    pub fn set_print_timestamps(&mut self, value: bool) {
        self.raw.print_timestamps = value;
    }

    pub fn set_single_segment(&mut self, value: bool) {
        self.raw.single_segment = value;
    }

    pub fn set_no_context(&mut self, value: bool) {
        self.raw.no_context = value;
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
    pub fn transcribe(&mut self, audio: &[f32], threads: usize) -> Result<String, FfiError> {
        let mut params = unsafe {
            sys::parakeet_full_default_params(
                sys::parakeet_sampling_strategy_PARAKEET_SAMPLING_GREEDY,
            )
        };
        params.n_threads = threads as c_int;
        params.no_context = true;

        let code = unsafe {
            sys::parakeet_full_with_state(
                self.ctx.as_ptr(),
                self.state.as_ptr(),
                params,
                audio.as_ptr(),
                n_samples(audio)?,
            )
        };
        if code != 0 {
            return Err(err(format!("parakeet_full failed ({code})")));
        }

        let segments = unsafe { sys::parakeet_full_n_segments_from_state(self.state.as_ptr()) };
        let mut text = String::new();
        for segment in 0..segments {
            text.push_str(&lossy(unsafe {
                sys::parakeet_full_get_segment_text_from_state(self.state.as_ptr(), segment)
            }));
        }
        Ok(text)
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
