//! F12 screenshot (PNG). Depends only on the `png` crate and the shared
//! surface state.
//!
//! Writes an 8/16/32-bit primary surface out as a PNG. 16-bit sources are
//! expanded using the RGB555 flag; 8-bit sources use the active palette.
//! Everything is best-effort: failures are logged via `dd_log!` and never
//! panic. A static lock guards against re-entry while a shot is in progress.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use png::ColorType;

use crate::state::{SurfaceBuffers, active_palette_entries, state};

/// Re-entry guard: only one screenshot may be in flight at a time.
static IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Take a screenshot of the current primary surface.
pub(crate) fn screenshot() {
    if IN_PROGRESS.swap(true, Ordering::SeqCst) {
        crate::dd_log!("screenshot: already in progress, skipping");
        return;
    }
    let result = take();
    IN_PROGRESS.store(false, Ordering::SeqCst);
    if let Err(e) = result {
        crate::dd_log!("screenshot failed: {}", e);
    }
}

fn take() -> std::result::Result<(), String> {
    let buffers = { state().lock().unwrap().primary.clone() };
    let Some(buffers) = buffers else {
        crate::dd_log!("screenshot: no primary surface");
        return Ok(());
    };

    let _g = buffers.lock.lock();
    let width = buffers.width;
    let height = buffers.height;
    let bpp = buffers.bpp;
    let pitch = buffers.pitch;
    let surface = buffers.surface;
    if surface.is_null() || width <= 0 || height <= 0 {
        crate::dd_log!("screenshot: invalid surface ({}x{} bpp={})", width, height, bpp);
        return Ok(());
    }

    let dir = {
        let st = state().lock().unwrap();
        st.screenshot_dir.clone().map(std::path::PathBuf::from).unwrap_or_else(|| {
            dll_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        })
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir {}: {}", dir.display(), e))?;

    let name = game_name();
    let stamp = timestamp();
    let path = dir.join(format!("{}_{}.png", name, stamp));

    write_png(&path, &buffers, width, height, bpp, pitch, surface)?;
    crate::dd_log!("screenshot saved to {}", path.display());
    Ok(())
}

/// Local time as `YYYY-MM-DD_HH-MM-SS` (mirrors cnc-ddraw's `%Y-%m-%d_%H-%M-%S`).
fn timestamp() -> String {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st: SYSTEMTIME = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond)
}

/// Directory of this DLL (for the default screenshot location when no
/// `ScreenshotDir` is configured).
fn dll_dir() -> Option<std::path::PathBuf> {
    unsafe {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameA, GetModuleHandleExA,
        };
        use windows::core::PCSTR;
        let mut h = HMODULE::default();
        let addr = dll_dir as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_ok() {
            let mut buf = [0u8; 1024];
            let n = GetModuleFileNameA(Some(h), &mut buf);
            if n > 0 {
                let path = std::path::PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).into_owned());
                return path.parent().map(|p| p.to_path_buf());
            }
        }
    }
    None
}

/// File-stem of the game executable (lowercase, spaces folded to `_`), falling
/// back to the DLL basename when the game name cannot be read.
fn game_name() -> String {
    unsafe {
        use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(None, &mut buf) as usize;
        if n > 0 && n < buf.len() {
            let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
            if let Some(stem) = exe.file_stem() {
                return normalize(stem.to_string_lossy().as_ref());
            }
        }
    }
    if let Some(p) = dll_path() {
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "ddraw".to_string());
        return normalize(&stem);
    }
    "ddraw".to_string()
}

fn dll_path() -> Option<std::path::PathBuf> {
    unsafe {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameA, GetModuleHandleExA,
        };
        use windows::core::PCSTR;
        let mut h = HMODULE::default();
        let addr = dll_path as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_ok() {
            let mut buf = [0u8; 1024];
            let n = GetModuleFileNameA(Some(h), &mut buf) as usize;
            if n > 0 {
                return Some(std::path::PathBuf::from(String::from_utf8_lossy(&buf[..n]).into_owned()));
            }
        }
    }
    None
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ' ' {
            out.push('_');
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Write a PNG file from the primary surface pixels (top-down rows).
fn write_png(
    path: &std::path::Path,
    _buffers: &std::sync::Arc<SurfaceBuffers>,
    width: i32,
    height: i32,
    bpp: i32,
    pitch: i32,
    surface: *const u8,
) -> std::result::Result<(), String> {
    let pitch = pitch.max(width * (bpp / 8));

    let mut rows: Vec<u8> = Vec::with_capacity((width as usize) * 4 * (height as usize));
    for row in 0..height {
        let base = (row as usize) * (pitch as usize);
        let src = unsafe { surface.add(base) };
        match bpp {
            8 => {
                let palette = active_palette_entries().unwrap_or([[0u8; 4]; 256]);
                for x in 0..width as usize {
                    let idx = unsafe { *src.add(x) } as usize;
                    let e = palette[idx.min(255)];
                    rows.extend_from_slice(&[e[2], e[1], e[0], 255]);
                }
            }
            16 => {
                let rgb555 = crate::state::RGB555.load(Ordering::Relaxed);
                for x in 0..width as usize {
                    let px = unsafe { *src.add(x * 2) } as u32 | ((unsafe { *src.add(x * 2 + 1) }) as u32) << 8;
                    let (r, g, b) = if rgb555 {
                        (((px >> 10) & 0x1f) as u8, ((px >> 5) & 0x1f) as u8, (px & 0x1f) as u8)
                    } else {
                        (((px >> 11) & 0x1f) as u8, ((px >> 5) & 0x3f) as u8, (px & 0x1f) as u8)
                    };
                    rows.extend_from_slice(&[r, g, b]);
                }
            }
            32 => {
                for x in 0..width as usize {
                    let o = x * 4;
                    let b = unsafe { *src.add(o) };
                    let g = unsafe { *src.add(o + 1) };
                    let r = unsafe { *src.add(o + 2) };
                    rows.extend_from_slice(&[r, g, b, 255]);
                }
            }
            _ => {
                let bpp_bytes = (bpp / 8).max(3) as usize;
                for x in 0..width as usize {
                    let o = x * bpp_bytes;
                    let b = unsafe { *src.add(o) };
                    let g = unsafe { *src.add(o + 1) };
                    let r = unsafe { *src.add(o + 2) };
                    rows.extend_from_slice(&[r, g, b, 255]);
                }
            }
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("png encode header: {}", e))?;
        writer.write_image_data(&rows).map_err(|e| format!("png encode data: {}", e))?;
    }

    let mut f = File::create(path).map_err(|e| format!("create {}: {}", path.display(), e))?;
    f.write_all(&out).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(())
}
