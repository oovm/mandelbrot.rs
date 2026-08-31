//! Crash dumps, log rotation and startup diagnostics (ports `debug.c`).
//!
//! Installs an unhandled-exception filter that writes a minidump next to the
//! log, and a vectored handler that logs every exception without swallowing it.

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_CONTINUE_SEARCH, EXCEPTION_EXECUTE_HANDLER};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameA, GetModuleFileNameW, GetModuleHandleExA,
    GetModuleHandleW, GetProcAddress, LoadLibraryW,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};
use windows::core::{PCSTR, PCWSTR};

type UnhandledFilter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;
type VectoredHandler = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;
type MiniDumpWriteDumpFn =
    unsafe extern "system" fn(HANDLE, u32, HANDLE, u32, *const c_void, *const c_void, *const c_void) -> i32;

/// Mirror of `_EXCEPTION_POINTERS`. The crate's type is gated behind the
/// (disabled) `Win32_System_Kernel` feature, so we carry our own `repr(C)` copy
/// of the NT layout.
#[repr(C)]
struct ExceptionPointers {
    exception_record: *mut ExceptionRecord,
    context_record: *mut c_void,
}

#[repr(C)]
struct ExceptionRecord {
    exception_code: u32,
    exception_flags: u32,
    exception_record: *mut ExceptionRecord,
    exception_address: *mut c_void,
    number_parameters: u32,
    exception_information: [usize; 15],
}

#[repr(C)]
struct MiniDumpExceptionInformation {
    thread_id: u32,
    exception_pointers: *const ExceptionPointers,
    client_pointers: i32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn load_module(name: &str) -> Option<HMODULE> {
    unsafe {
        let w = wide(name);
        if let Ok(h) = GetModuleHandleW(PCWSTR::from_raw(w.as_ptr())) {
            return Some(h);
        }
        LoadLibraryW(PCWSTR::from_raw(w.as_ptr())).ok()
    }
}

/// Load `dbghelp.dll` once and resolve `MiniDumpWriteDump` so the DLL is not
/// hard-imported (mirrors cnc-ddraw's `real_LoadLibraryA("Dbghelp.dll")`).
fn minidump_proc() -> Option<MiniDumpWriteDumpFn> {
    static MDW: OnceLock<Option<MiniDumpWriteDumpFn>> = OnceLock::new();
    *MDW.get_or_init(|| unsafe {
        let dbghelp = load_module("dbghelp.dll")?;
        let proc = GetProcAddress(dbghelp, PCSTR::from_raw(c"MiniDumpWriteDump".as_ptr().cast()))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, MiniDumpWriteDumpFn>(proc))
    })
}

/// Write a `MiniDumpNormal` dump to `rs-ddraw.{pid}.dmp` next to the log file.
fn write_minidump(exception: *const ExceptionPointers) {
    let Some(mdw) = minidump_proc() else {
        crate::dd_log!("minidump: MiniDumpWriteDump unavailable");
        return;
    };
    let mut dmp_path = log_file_path_buf();
    dmp_path.set_file_name(format!("rs-ddraw.{}.dmp", std::process::id()));
    let Ok(file) = std::fs::File::create(&dmp_path) else {
        crate::dd_log!("minidump: failed to create {}", dmp_path.display());
        return;
    };
    let hfile = HANDLE(file.as_raw_handle());
    let mut info = MiniDumpExceptionInformation {
        thread_id: unsafe { GetCurrentThreadId() },
        exception_pointers: exception,
        client_pointers: 1,
    };
    let ok = unsafe {
        mdw(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            hfile,
            0, // MiniDumpNormal
            &mut info as *mut MiniDumpExceptionInformation as *const c_void,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    drop(file);
    crate::dd_log!("minidump written to {} ok={}", dmp_path.display(), ok != 0);
}

unsafe extern "system" fn unhandled_exception_filter(exception: *const ExceptionPointers) -> i32 {
    write_minidump(exception);
    if !exception.is_null() && !(*exception).exception_record.is_null() {
        let rec = &*(*exception).exception_record;
        crate::dd_log!(
            "unhandled exception at {:p}, code=0x{:08X}, flags=0x{:X}",
            rec.exception_address,
            rec.exception_code,
            rec.exception_flags
        );
    }
    EXCEPTION_EXECUTE_HANDLER
}

unsafe extern "system" fn vectored_exception_handler(exception: *mut ExceptionPointers) -> i32 {
    if !exception.is_null() && !(*exception).exception_record.is_null() {
        let rec = &*(*exception).exception_record;
        crate::dd_log!("exception at {:p}, code=0x{:08X}", rec.exception_address, rec.exception_code);
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Resolve `AddVectoredExceptionHandler` from kernel32 at runtime rather than
/// importing it, and register the logging handler (runs first, never swallows).
fn install_vectored_handler() {
    unsafe {
        let Some(kmod) = load_module("kernel32.dll") else { return };
        let Some(proc) = GetProcAddress(kmod, PCSTR::from_raw(c"AddVectoredExceptionHandler".as_ptr().cast())) else {
            return;
        };
        type AddVectoredFn = unsafe extern "system" fn(u32, Option<VectoredHandler>) -> *mut c_void;
        let add: AddVectoredFn = std::mem::transmute(proc);
        let handle = add(1, Some(vectored_exception_handler));
        crate::dd_log!("AddVectoredExceptionHandler installed (handle={:p})", handle);
    }
}

/// Resolve `SetUnhandledExceptionFilter` from kernel32 (the crate only emits it
/// under the disabled `Win32_System_Kernel` feature) and register the dump
/// filter. Returns `EXCEPTION_EXECUTE_HANDLER`, terminating the process.
fn install_unhandled_filter() {
    unsafe {
        let Some(kmod) = load_module("kernel32.dll") else { return };
        let Some(proc) = GetProcAddress(kmod, PCSTR::from_raw(c"SetUnhandledExceptionFilter".as_ptr().cast())) else {
            return;
        };
        type SetFilterFn = unsafe extern "system" fn(Option<UnhandledFilter>) -> Option<UnhandledFilter>;
        let set: SetFilterFn = std::mem::transmute(proc);
        let prev = set(Some(unhandled_exception_filter));
        let uef: UnhandledFilter = unhandled_exception_filter;
        crate::dd_log!(
            "SetUnhandledExceptionFilter registered at {:#x} (previous={:#x?})",
            uef as usize,
            prev.map(|p| p as usize)
        );
    }
}

/// Install the unhandled-exception filter (MiniDumpWriteDump) and the vectored
/// handler that logs every exception. Called once during DLL attach.
pub unsafe fn install_handler() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    rotate_if_needed();
    install_unhandled_filter();
    install_vectored_handler();
}

fn os_version() -> Option<OSVERSIONINFOW> {
    unsafe {
        let ntdll = load_module("ntdll.dll")?;
        // RtlGetVersion is only emitted under the (disabled) Wdk feature, so
        // resolve it from ntdll directly.
        let proc = GetProcAddress(ntdll, PCSTR::from_raw(c"RtlGetVersion".as_ptr().cast()))?;
        type RtlGetVersionFn = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32;
        let f: RtlGetVersionFn = std::mem::transmute(proc);
        let mut info =
            OSVERSIONINFOW { dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
        if f(&mut info) == 0 { Some(info) } else { None }
    }
}

fn dll_base() -> usize {
    unsafe {
        if let Ok(h) = GetModuleHandleW(PCWSTR::from_raw(wide("ddraw.dll").as_ptr())) {
            return h.0 as usize;
        }
        let mut h = HMODULE::default();
        let addr = dll_base as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_ok() {
            return h.0 as usize;
        }
    }
    0
}

/// Path of this DLL (for replicating the log module's directory choice).
fn module_file_name() -> Option<std::path::PathBuf> {
    unsafe {
        let mut h = HMODULE::default();
        let addr = module_file_name as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_err() {
            return None;
        }
        let mut buf = [0u8; 1024];
        let len = GetModuleFileNameA(Some(h), &mut buf) as usize;
        if len > 0
            && let Ok(s) = std::str::from_utf8(&buf[..len])
        {
            return Some(std::path::PathBuf::from(s));
        }
    }
    None
}

fn log_file_path_buf() -> std::path::PathBuf {
    if let Some(mut p) = module_file_name() {
        let name = crate::config::log_file_name();
        p.set_file_name(if name.is_empty() { "rs-ddraw.log" } else { name.as_str() });
        return p;
    }
    std::path::PathBuf::from("rs-ddraw.log")
}

/// Full path of the active log file (same directory/name as the log module).
pub fn log_file_path() -> String {
    log_file_path_buf().to_string_lossy().into_owned()
}

/// Rotate the log file when it exceeds 100 MB (like cnc-ddraw): rename it to
/// `{log}.old`. Best-effort: failures (e.g. the file held open) are logged.
pub fn rotate_if_needed() {
    const MAX_LOG_BYTES: u64 = 100 * 1024 * 1024;
    let path = log_file_path_buf();
    let Ok(meta) = std::fs::metadata(&path) else { return };
    if meta.len() <= MAX_LOG_BYTES {
        return;
    }
    let mut old_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    old_name.push(".old");
    let old = path.with_file_name(old_name);
    match std::fs::rename(&path, &old) {
        Ok(()) => crate::dd_log!("log rotated: {} -> {}", path.display(), old.display()),
        Err(e) => crate::dd_log!("log rotation failed ({} -> {}): {}", path.display(), old.display(), e),
    }
}

fn renderer_name(r: i32) -> &'static str {
    match r {
        crate::state::RENDERER_GDI => "gdi",
        crate::state::RENDERER_OPENGL => "opengl",
        crate::state::RENDERER_D3D9 => "d3d9",
        crate::state::RENDERER_OPENGL_CORE => "openglcore",
        _ => "unknown",
    }
}

/// Log the process name, DLL base address, Windows version and a digest of the
/// current DDrawState at startup.
pub fn log_init_info() {
    crate::dd_log!(
        "rs-ddraw {} (process {}, module base {:#x})",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        dll_base()
    );
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if n > 0 && n < buf.len() {
        crate::dd_log!("process: {}", String::from_utf16_lossy(&buf[..n]));
    } else {
        crate::dd_log!("process: <unknown>");
    }
    match os_version() {
        Some(v) => crate::dd_log!(
            "windows version: {}.{} (build {}, platform {})",
            v.dwMajorVersion,
            v.dwMinorVersion,
            v.dwBuildNumber,
            v.dwPlatformId
        ),
        None => crate::dd_log!("windows version: <unknown>"),
    }
    let st = crate::state::state().lock().unwrap();
    crate::dd_log!(
        "ddraw: renderer={} (auto={}), primary {}x{}x{}bpp",
        renderer_name(st.renderer),
        st.auto_renderer,
        st.width,
        st.height,
        st.bpp
    );
    crate::dd_log!(
        "ddraw: windowed(border={}, resizable={}, nonexclusive={}), fixed_output={}, center_window={}, savesettings={}",
        st.border,
        st.resizable,
        st.nonexclusive,
        st.fixed_output,
        st.center_window,
        st.savesettings
    );
    crate::dd_log!(
        "ddraw: adjmouse={}, filter={}, swap_interval={}, aspect={}, windowboxing={}, stretch_to_fullscreen={}",
        st.adjmouse,
        st.filter,
        st.swap_interval,
        st.maintain_aspect_ratio,
        st.windowboxing,
        st.stretch_to_fullscreen
    );
    crate::dd_log!(
        "ddraw: limiter(type={}, maxgameticks={}, minfps={}, maxfps={}, target_fps={:.1})",
        st.limiter_type,
        st.maxgameticks,
        st.minfps,
        st.maxfps,
        st.target_fps
    );
}
