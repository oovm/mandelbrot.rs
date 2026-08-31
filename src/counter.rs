//! High-resolution frame counter (ports `counter.c`).

use std::sync::OnceLock;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

pub type QPCounter = i64;

static FREQ: OnceLock<f64> = OnceLock::new();

fn freq() -> f64 {
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        (f as f64) / 1000.0
    })
}

pub fn counter_start() -> QPCounter {
    let mut li = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut li);
    }
    li
}

/// Returns elapsed milliseconds since `start`.
pub fn counter_get(start: QPCounter) -> f64 {
    let mut li = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut li);
    }
    (li - start) as f64 / freq()
}
