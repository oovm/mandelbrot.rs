//! Renderer thread and backend orchestration (ports `render.c`).

pub(crate) mod d3d9;
pub(crate) mod gdi;
pub(crate) mod opengl;
pub(crate) mod scale;

use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, GetDC, HDC, ReleaseDC, SRCCOPY, SetBkMode, SetTextColor, TRANSPARENT, TextOutA,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClientRect, GetCursorPos, GetWindowRect, SetCursorPos,
};
use windows::core::{BOOL, PCSTR, PCWSTR};

static PRESENT_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NO_PRIMARY_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

use crate::counter::{counter_get, counter_start};
use crate::state::{EDGE_X, EDGE_Y, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, SurfaceBuffers, state};

/// Human-readable name for a `RENDERER_*` id (used in logs).
fn renderer_name(r: i32) -> &'static str {
    match r {
        RENDERER_D3D9 => "d3d9",
        RENDERER_OPENGL => "opengl",
        RENDERER_GDI => "gdi",
        _ => "unknown",
    }
}

/// Draw the current FPS counter on the destination DC (used when `DrawFPS` is on).
pub(crate) unsafe fn draw_fps(hdc: HDC, fps: f64) {
    let text = format!("{:.0} FPS", fps);
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(0x00FFFF00));
    let _ = TextOutA(hdc, 5, 5, text.as_bytes());
}

/// Synchronise with the DWM compositor, if available. Resolved via
/// `GetProcAddress("dwmapi.dll","DwmFlush")` at runtime so the DLL is never
/// hard-imported (no crate feature required). Returns true when the flush
/// actually ran (composition enabled); the caller falls back to its normal
/// pacing otherwise. Used when `uses_vblank` is set to avoid tearing while
/// matching the refresh rate.
unsafe fn dwm_flush() -> bool {
    type DwmFlushFn = unsafe extern "system" fn() -> i32;
    static DWMFLUSH: OnceLock<Option<DwmFlushFn>> = OnceLock::new();
    let f = DWMFLUSH.get_or_init(|| unsafe {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let name = wide("dwmapi.dll");
        let h: HMODULE = match GetModuleHandleW(PCWSTR::from_raw(name.as_ptr())) {
            Ok(m) => m,
            Err(_) => LoadLibraryW(PCWSTR::from_raw(name.as_ptr())).ok()?,
        };
        let proc = GetProcAddress(h, PCSTR::from_raw(c"DwmFlush".as_ptr().cast()))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, DwmFlushFn>(proc))
    });
    match f.as_ref() {
        Some(func) => func() >= 0,
        None => false,
    }
}

/// State passed to the child-window enumeration callback.
struct ChildComposite {
    surface_hdc: HDC,
    win_left: i32,
    win_top: i32,
}

/// Paint the region of the primary surface that lies under each game child
/// window into that child window, so child windows (e.g. in-game menus)
/// aren't left blank. Ports ts-ddraw's `EnumChildProc` / `EnumChildWindows`.
extern "system" fn enum_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let data = &*(lparam.0 as *const ChildComposite);
        let child_dc = GetDC(Some(hwnd));
        let mut size = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut size);
        let mut pos = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(hwnd, &mut pos);
        let w = size.right - size.left;
        let h = size.bottom - size.top;
        if w > 0 && h > 0 {
            let _ = BitBlt(
                child_dc,
                0,
                0,
                w,
                h,
                Some(data.surface_hdc),
                pos.left - data.win_left,
                pos.top - data.win_top,
                SRCCOPY,
            );
        }
        let _ = ReleaseDC(Some(hwnd), child_dc);
        BOOL(1)
    }
}

/// Composite the surface into any child windows of the game window.
pub(crate) unsafe fn composite_child_windows(main_hwnd: HWND, surface_hdc: HDC) {
    if main_hwnd.is_invalid() || surface_hdc.is_invalid() {
        return;
    }
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(main_hwnd, &mut wr);
    let data = ChildComposite { surface_hdc, win_left: wr.left, win_top: wr.top };
    EnumChildWindows(Some(main_hwnd), Some(enum_child_proc), LPARAM(&data as *const ChildComposite as isize));
}

/// Teleport the cursor to the opposite edge when it lingers too long near a
/// window edge, preventing the pointer from hitting the monitor boundary and
/// locking the mouse (Windows pointer-ballistic wrap).
fn edge_timer_logic(hwnd: HWND) {
    static LAST_EDGE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

    let (edge_timeout_ms, mouse_is_locked, edge_dim) = {
        let st = state().lock().unwrap();
        (st.edge_timeout_ms, st.mouse_is_locked, st.edge_dimension)
    };

    if edge_timeout_ms <= 0 || mouse_is_locked == 0 || hwnd.is_invalid() {
        LAST_EDGE.store(0, Ordering::Relaxed);
        return;
    }

    unsafe {
        let mut pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
        let _ = GetCursorPos(&mut pt);

        // Window rect in screen coordinates; the client area sits inside it.
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let _ = GetWindowRect(hwnd, &mut wr);
        let rw = wr.right - wr.left;
        let rh = wr.bottom - wr.top;
        if rw <= 0 || rh <= 0 {
            return;
        }

        let left = wr.left;
        let top = wr.top;
        let right = wr.right;
        let bottom = wr.bottom;

        let d = edge_dim;
        let near_left = pt.x <= left + d;
        let near_right = pt.x >= right - d;
        let near_top = pt.y <= top + d;
        let near_bottom = pt.y >= bottom - d;

        let hit_edge = near_left || near_right || near_top || near_bottom;

        if !hit_edge {
            LAST_EDGE.store(0, Ordering::Relaxed);
            return;
        }

        let now_ms =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let last = LAST_EDGE.load(Ordering::Relaxed);
        if last != 0 && (now_ms - last) < edge_timeout_ms as i64 {
            return;
        }

        let mut new_x = pt.x;
        let mut new_y = pt.y;
        let mut edge_val = 0i32;
        if near_left {
            new_x = right - 2;
            edge_val = EDGE_X;
        } else if near_right {
            new_x = left + 2;
            edge_val = EDGE_X;
        } else if near_top {
            new_y = bottom - 2;
            edge_val = EDGE_Y;
        } else if near_bottom {
            new_y = top + 2;
            edge_val = EDGE_Y;
        }

        // Already in screen coordinates.
        let _ = SetCursorPos(new_x, new_y);
        {
            let mut st = state().lock().unwrap();
            st.edge_value = edge_val;
        }
        LAST_EDGE.store(now_ms, Ordering::Relaxed);
    }
}

/// Spawn the renderer thread exactly once.
pub(crate) fn start() {
    let already = {
        let st = state().lock().unwrap();
        st.render_thread.is_some()
    };
    if already {
        return;
    }
    let _handle = thread::spawn(render_thread);
    let mut st = state().lock().unwrap();
    st.render_thread = None;
}

fn render_thread() {
    // Wait for the primary surface to be created.
    loop {
        {
            let st = state().lock().unwrap();
            if st.primary.is_some() {
                break;
            }
            if !st.running.load(Ordering::Relaxed) {
                return;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    enum Backend {
        D3D9(d3d9::D3D9State),
        GL(Box<opengl::OglState>),
        Gdi,
    }

    let (hwnd, hdc, w, h, renderer, auto) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.hdc, st.width, st.height, st.renderer, st.auto_renderer)
    };

    // Auto order: D3D9 -> OpenGL -> GDI. An explicit choice falls back to GDI if
    // its backend fails to initialize.
    let order: [i32; 4] = if auto {
        [RENDERER_D3D9, RENDERER_OPENGL, RENDERER_GDI, RENDERER_GDI]
    } else {
        [renderer, RENDERER_GDI, RENDERER_GDI, RENDERER_GDI]
    };

    let mut backend = {
        let mut chosen = RENDERER_GDI;
        let mut result: Option<Backend> = None;
        for &r in &order {
            let b = match r {
                RENDERER_D3D9 => d3d9::D3D9State::new(hwnd, w, h).map(Backend::D3D9),
                RENDERER_OPENGL => opengl::OglState::new(hdc, w, h).map(|s| Backend::GL(Box::new(s))),
                RENDERER_GDI => Some(Backend::Gdi),
                _ => None,
            };
            match b {
                Some(bk) => {
                    chosen = r;
                    result = Some(bk);
                    break;
                }
                None => {
                    crate::dd_log!(
                        "renderer {} unavailable, switching to {}",
                        renderer_name(r),
                        renderer_name(if r == RENDERER_GDI {
                            RENDERER_GDI
                        } else {
                            order[order.iter().position(|&x| x == r).unwrap() + 1]
                        })
                    );
                }
            }
        }
        if let Some(bk) = result {
            if chosen != RENDERER_GDI {
                if auto {
                    crate::dd_log!("auto: selected renderer {}", renderer_name(chosen));
                } else {
                    crate::dd_log!("renderer {} initialized", renderer_name(chosen));
                }
            }
            state().lock().unwrap().renderer = chosen;
            bk
        } else {
            Backend::Gdi
        }
    };

    crate::dd_log!(
        "render thread started, backend={}, target_fps={}",
        match backend {
            Backend::D3D9(_) => "d3d9",
            Backend::GL(_) => "opengl",
            Backend::Gdi => "gdi",
        },
        { state().lock().unwrap().target_fps }
    );

    let mut start = counter_start();
    let mut last_w: i32 = -1;
    let mut last_h: i32 = -1;

    loop {
        if !state().lock().unwrap().running.load(Ordering::Relaxed) {
            break;
        }

        let primary: Option<std::sync::Arc<SurfaceBuffers>> = { state().lock().unwrap().primary.clone() };
        // Present only when the game has produced a *complete* frame. cnc-ddraw
        // uploads on `surface_updated` (raised on Blt/Flip/Unlock/ReleaseDC of
        // the primary), i.e. *after* the draw finishes, never on
        // WaitForVerticalBlank, which many games call *before* drawing. Uploading
        // on vblank would capture a half-drawn (black) primary. Each backend
        // draws its last good frame when no new one is signalled; GDI only
        // repaints the window when a new frame is signalled.
        let mut dirty = crate::state::take_dirty();
        // Force an upload when the primary surface size changes (e.g. the game
        // recreates it after the loading screen). Without this the texture
        // would be the wrong size and the screen would freeze on the last good
        // frame even if the game keeps rendering.
        if let Some(ref buffers) = primary
            && (buffers.width != last_w || buffers.height != last_h)
        {
            dirty = true;
            last_w = buffers.width;
            last_h = buffers.height;
        }
        if let Some(ref buffers) = primary {
            if !PRESENT_LOGGED.swap(true, Ordering::Relaxed) {
                crate::dd_log!("first present: {}x{} bpp={}", buffers.width, buffers.height, buffers.bpp);
            }
            match &mut backend {
                Backend::D3D9(d) => d.present(buffers, dirty),
                Backend::GL(g) => g.present(buffers, dirty),
                Backend::Gdi => {
                    if dirty {
                        gdi::present(buffers);
                    }
                }
            }
        } else if !NO_PRIMARY_LOGGED.swap(true, Ordering::Relaxed) {
            crate::dd_log!("render loop tick: primary surface still not set");
        }

        edge_timer_logic(hwnd);

        let elapsed = counter_get(start);
        if elapsed > 0.0 {
            let fps = 1000.0 / elapsed;
            let mut st = state().lock().unwrap();
            // Exponential smoothing for a stable readout.
            st.fps = if st.fps == 0.0 { fps } else { st.fps * 0.9 + fps * 0.1 };
        }

        // ---- pacing: maxfps / minfps / vblank ----
        // Honour the pre-computed target_frame_len from state (set by config
        // from maxfps/TargetFPS/vsync). Fall back to 60 Hz when unset.
        let (frame_len, minfps) = {
            let st = state().lock().unwrap();
            let fl = if st.target_frame_len > 0.0 { st.target_frame_len } else { 1000.0 / 60.0 };
            (fl, st.minfps)
        };
        // minfps > 0: when the achieved fps drops below minfps, skip the
        // sleep so the loop spins and presents an extra frame, keeping the OS
        // responsive even when the game has stalled (port of render.c's
        // semaphore timeout path).
        let skip_sleep = minfps > 0 && {
            let fps = { state().lock().unwrap().fps };
            fps > 0.0 && fps < minfps as f64
        };

        let elapsed = counter_get(start);
        // uses_vblank: sync to the compositor via DwmFlush (resolved at
        // runtime from dwmapi.dll, no crate feature) so we don't tear or
        // run ahead of the refresh; falls back to the plain sleep pacing.
        let uses_vblank = { state().lock().unwrap().uses_vblank.load(Ordering::Relaxed) };
        let flushed = uses_vblank && unsafe { dwm_flush() };

        if skip_sleep {
            // Force a redraw/present of the last good frame at the minfps
            // rate so the window stays painted even when the game stalls.
            crate::state::mark_dirty();
        } else if !flushed && elapsed < frame_len {
            let sleep_ms = (frame_len - elapsed) as u64;
            if sleep_ms > 0 {
                thread::sleep(Duration::from_millis(sleep_ms));
            }
        }

        start = counter_start();
    }

    match backend {
        Backend::D3D9(d) => d.release(),
        Backend::GL(g) => g.release(),
        Backend::Gdi => {}
    }
}
