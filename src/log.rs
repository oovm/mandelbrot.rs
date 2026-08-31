use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameA;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds elapsed since the first log call (process start).
fn now_ms() -> u64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

/// Compute the path of `rs-ddraw.log` sitting next to this DLL so the log is
/// easy to find regardless of the game's current working directory.
fn log_path(h_module: HMODULE) -> std::path::PathBuf {
    let mut buf = [0u8; 1024];
    let len = unsafe { GetModuleFileNameA(Some(h_module), &mut buf) } as usize;
    if len > 0
        && let Ok(s) = std::str::from_utf8(&buf[..len])
    {
        let mut p = std::path::PathBuf::from(s);
        let name = crate::config::log_file_name();
        p.set_file_name(if name.is_empty() { String::from("rs-ddraw.log") } else { name });
        return p;
    }
    std::path::PathBuf::from("rs-ddraw.log")
}

pub fn init(h_module: HMODULE) {
    if let Ok(mut guard) = LOG.lock()
        && guard.is_none()
    {
        let path = log_path(h_module);
        if let Ok(f) = OpenOptions::new().create(true).write(true).truncate(true).open(&path) {
            let _ = writeln!(&f, "=== rs-ddraw log started ({}) ===", path.display());
            *guard = Some(f);
        }
    }
}

pub fn log(msg: &str) {
    if let Ok(mut guard) = LOG.lock()
        && let Some(ref mut f) = *guard
    {
        let _ = writeln!(f, "[{:>9} ms] {}", now_ms(), msg);
        let _ = f.flush();
    }
}

#[macro_export]
macro_rules! dd_log {
    ($($arg:tt)*) => {
        $crate::log::log(&format!($($arg)*))
    };
}
