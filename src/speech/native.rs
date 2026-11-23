//! Minimal bindings to the pinned transcribe.cpp 0.4.2 ABI. Native inference
//! lives in the speech-server process, never in the UI/rendering process.
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    ptr,
};

#[repr(C)]
struct BackendParams {
    size: u64,
    directory: *const c_char,
    allowed: u32,
}
#[repr(C)]
struct LoadParams {
    size: u64,
    backend: i32,
    device: *mut c_void,
    no_mmap: bool,
}
#[repr(C)]
struct SessionParams {
    size: u64,
    threads: i32,
    kv_type: i32,
    context: i32,
}

type InitBackend = unsafe extern "C" fn(*const BackendParams) -> i32;
type InitLoad = unsafe extern "C" fn(*mut LoadParams);
type Load = unsafe extern "C" fn(*const c_char, *const LoadParams, *mut *mut c_void) -> i32;
type InitSession = unsafe extern "C" fn(*mut SessionParams);
type Session = unsafe extern "C" fn(*mut c_void, *const SessionParams, *mut *mut c_void) -> i32;
type Free = unsafe extern "C" fn(*mut c_void);
type Run = unsafe extern "C" fn(*mut c_void, *const f32, i32, *const c_void) -> i32;
type Text = unsafe extern "C" fn(*const c_void) -> *const c_char;
type Status = unsafe extern "C" fn(i32) -> *const c_char;
type LogCallback = unsafe extern "C" fn(i32, *const c_char, *mut c_void);
type SetLog = unsafe extern "C" fn(Option<LogCallback>, *mut c_void);

fn should_log(level: i32) -> bool {
    matches!(level, 2 | 3)
}

unsafe extern "C" fn native_log(level: i32, message: *const c_char, _: *mut c_void) {
    if !should_log(level) || message.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
        crate::runtime::log(true, text.trim_end());
    });
}

#[cfg(test)]
mod logging_tests {
    use super::should_log;

    #[test]
    fn native_warnings_and_errors_are_kept_but_progress_is_not() {
        assert!(should_log(2));
        assert!(should_log(3));
        for level in [0, 1, 4, 5] {
            assert!(!should_log(level));
        }
    }
}

pub struct Engine {
    _library: libloading::Library,
    _cuda: Option<libloading::Library>,
    model: *mut c_void,
    session: *mut c_void,
    free_model: Free,
    free_session: Free,
    run: Run,
    text: Text,
    status: Status,
    pub backend: String,
}

impl Engine {
    pub fn load(base: &Path, device: &str, threads: usize) -> Result<Self, String> {
        let cuda = base.join("speech-runtime/transcribe-native-windows-x86_64-cuda");
        let cpu = base.join("speech-runtime/transcribe-native-windows-x86_64-cpu-vulkan");
        let directory = std::env::var_os("REWORD_SPEECH_RUNTIME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                if device != "cpu" && cuda.exists() {
                    cuda
                } else {
                    cpu
                }
            });
        let model_path = std::env::var_os("REWORD_SPEECH_MODEL")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| base.join("models/parakeet-uk-v3-q8.gguf"));
        if !model_path.is_file() {
            return Err("Модель ще не завантажена. Запусти setup-speech.ps1".into());
        }
        let filename = if cfg!(windows) {
            "transcribe.dll"
        } else if cfg!(target_os = "macos") {
            "libtranscribe.dylib"
        } else {
            "libtranscribe.so"
        };
        let dll = directory.join(filename);
        unsafe {
            #[cfg(windows)]
            let library: libloading::Library =
                libloading::os::windows::Library::load_with_flags(&dll, 0x00001100)
                    .map_err(|e| format!("Не вдалося завантажити {}: {e}", dll.display()))?
                    .into();
            #[cfg(not(windows))]
            let library =
                libloading::Library::new(&dll).map_err(|e| format!("{}: {e}", dll.display()))?;
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|e| e.to_string())?
                };
            }
            let set_log = symbol!("transcribe_log_set", SetLog);
            set_log(Some(native_log), ptr::null_mut());
            let version = symbol!(
                "transcribe_version",
                unsafe extern "C" fn() -> *const c_char
            );
            if CStr::from_ptr(version()).to_bytes() != b"0.4.2" {
                return Err("Потрібен speech runtime 0.4.2; запусти setup-speech.ps1".into());
            }
            let status = symbol!("transcribe_status_string", Status);
            let error = |code| CStr::from_ptr(status(code)).to_string_lossy().into_owned();
            let init_backend = symbol!("transcribe_init_backends_ex", InitBackend);
            // ggml loads plugins after the top-level DLL. Preload the CUDA
            // plugin with its private dependency directory on Windows too.
            #[cfg(windows)]
            let cuda = if device != "cpu" && directory.join("ggml-cuda.dll").is_file() {
                match libloading::os::windows::Library::load_with_flags(
                    directory.join("ggml-cuda.dll"),
                    0x00001100,
                ) {
                    Ok(library) => Some(library.into()),
                    Err(error) if device == "cuda" => {
                        return Err(format!(
                            "CUDA runtime: {error}. Запусти setup-speech.ps1 -Backend cuda"
                        ));
                    }
                    Err(_) => None,
                }
            } else {
                None
            };
            #[cfg(not(windows))]
            let cuda = None;
            let init_backend_params = symbol!(
                "transcribe_backend_init_params_init",
                unsafe extern "C" fn(*mut BackendParams)
            );
            let directory_c =
                CString::new(directory.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let mut params: BackendParams = std::mem::zeroed();
            init_backend_params(&mut params);
            params.directory = directory_c.as_ptr();
            // Do not initialize Vulkan/Intel drivers when CPU or CUDA was chosen.
            params.allowed = if device == "cpu" { 1 } else { 1 | 8 };
            let code = init_backend(&params);
            if code != 0 {
                return Err(error(code));
            }
            let init_load = symbol!("transcribe_model_load_params_init", InitLoad);
            let load = symbol!("transcribe_model_load_file", Load);
            let mut params: LoadParams = std::mem::zeroed();
            init_load(&mut params);
            params.backend = match device {
                "cpu" => 1,
                "cuda" => 5,
                _ => 0,
            };
            let path_c =
                CString::new(model_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let mut model = ptr::null_mut();
            let mut code = load(path_c.as_ptr(), &params, &mut model);
            if code != 0 && device == "auto" {
                crate::runtime::log(true, "GPU model load failed; retrying on CPU");
                params.backend = 1;
                code = load(path_c.as_ptr(), &params, &mut model);
            }
            if code != 0 {
                return Err(format!("Завантаження Parakeet: {}", error(code)));
            }
            let free_model = symbol!("transcribe_model_free", Free);
            let init_session = symbol!("transcribe_session_params_init", InitSession);
            let session_init = symbol!("transcribe_session_init", Session);
            let mut params: SessionParams = std::mem::zeroed();
            init_session(&mut params);
            params.threads = threads.clamp(1, 32) as i32;
            let mut session = ptr::null_mut();
            let code = session_init(model, &params, &mut session);
            if code != 0 {
                free_model(model);
                return Err(error(code));
            }
            let backend = symbol!("transcribe_model_backend", Text);
            let engine = Self {
                free_session: symbol!("transcribe_session_free", Free),
                run: symbol!("transcribe_run", Run),
                text: symbol!("transcribe_full_text", Text),
                backend: CStr::from_ptr(backend(model))
                    .to_string_lossy()
                    .into_owned(),
                model,
                session,
                free_model,
                status,
                _library: library,
                _cuda: cuda,
            };
            Ok(engine)
        }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        // Silence padding protects the first/last phonemes at push-to-talk edges.
        let mut padded = vec![0.0; 1600];
        padded.extend_from_slice(samples);
        padded.resize(padded.len() + 3200, 0.0);
        unsafe {
            let code = (self.run)(
                self.session,
                padded.as_ptr(),
                padded.len() as i32,
                ptr::null(),
            );
            if code != 0 {
                return Err(CStr::from_ptr((self.status)(code))
                    .to_string_lossy()
                    .into_owned());
            }
            let text = (self.text)(self.session);
            if text.is_null() {
                return Ok(String::new());
            }
            Ok(CStr::from_ptr(text).to_string_lossy().trim().to_owned())
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            (self.free_session)(self.session);
            (self.free_model)(self.model);
        }
    }
}
