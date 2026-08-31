//! Mouse scaling / cursor-lock helpers (ports `mouse.c`).
//!
//! `adjmouse` re-maps game-relative mouse coordinates to the actual (possibly
//! upscaled, letterboxed) window so the pointer lands where the game thinks it
//! is. The scale is derived from the render viewport vs. the game resolution
//! and feeds both the DInput device data and the `GetCursorPos` hook.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClientRect, GetCursorPos, GetWindowRect, SetCursorPos};

use crate::state::state;

/// Recompute `mouse_scale_*` from the current render viewport and the game's
/// requested resolution. Call whenever the viewport or resolution changes.
pub fn update_scale() {
    let st = state().lock().unwrap();
    let adj = st.adjmouse;
    let vw = st.render.viewport.width().max(1) as f64;
    let vh = st.render.viewport.height().max(1) as f64;
    let gw = st.width.max(1) as f64;
    let gh = st.height.max(1) as f64;
    drop(st);
    let mut st = state().lock().unwrap();
    if adj && gw > 0.0 && gh > 0.0 {
        st.mouse_scale_x = vw / gw;
        st.mouse_scale_y = vh / gh;
        st.mouse_scale_ix = gw / vw;
        st.mouse_scale_iy = gh / vh;
    } else {
        st.mouse_scale_x = 1.0;
        st.mouse_scale_y = 1.0;
        st.mouse_scale_ix = 1.0;
        st.mouse_scale_iy = 1.0;
    }
}

fn sc() -> (f64, f64) {
    let st = state().lock().unwrap();
    (st.mouse_scale_x, st.mouse_scale_y)
}

/// Map a game-relative coordinate to the window coordinate space.
pub fn mapped_pos(x: i32, y: i32) -> (i32, i32) {
    let (sx, sy) = sc();
    ((x as f64 * sx) as i32, (y as f64 * sy) as i32)
}

/// Map a window-space delta (e.g. relative mouse movement) into the game's
/// coordinate space (the inverse transformation).
pub fn mapped_delta(dx: i32, dy: i32) -> (i32, i32) {
    let st = state().lock().unwrap();
    let ix = st.mouse_scale_ix;
    let iy = st.mouse_scale_iy;
    drop(st);
    ((dx as f64 * ix) as i32, (dy as f64 * iy) as i32)
}

/// Whether the mouse is currently locked into the game window.
pub fn is_locked() -> bool {
    state().lock().unwrap().mouse_is_locked != 0
}

/// Whether the window is considered "active" (focused) by the game.
fn window_active() -> bool {
    let st = state().lock().unwrap();
    st.focus_gained != 0 || {
        // Fall back to foreground check when the game hasn't reported focus.
        let hwnd = st.hwnd;
        drop(st);
        !hwnd.is_invalid()
    }
}

/// Confine the cursor to the render area (ports `mouse_lock` from `mouse.c`).
///
/// Called repeatedly (e.g. by a WM_TIMER / frame tick) — ClipCursor picks up the
/// window's current position each time so the clip follows window moves.
pub fn lock_cursor() {
    // While the semi-transparent config overlay is open the cursor must stay
    // free: the menu window is WS_EX_NOACTIVATE, so the game never loses focus
    // and would otherwise (re)clip the pointer on every activation.
    if crate::overlay::is_open() {
        return;
    }
    let (adj, hwnd, top_left, locked) = {
        let st = state().lock().unwrap();
        (st.adjmouse, st.hwnd, st.lock_mouse_top_left, st.mouse_is_locked != 0)
    };
    if !adj || hwnd.is_invalid() || locked || !window_active() {
        return;
    }

    unsafe {
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetClientRect(hwnd, &mut rc).is_err() {
            return;
        }
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return;
        }
        let (left, top) = (wr.left + rc.left, wr.top + rc.top);
        let clip = if top_left {
            // Keep the pointer in a small top-left box inside the render area.
            RECT { left, top, right: left + 8, bottom: top + 8 }
        } else {
            RECT { left, top, right: left + (rc.right - rc.left), bottom: top + (rc.bottom - rc.top) }
        };
        let _ = ClipCursor(Some(&clip));
        state().lock().unwrap().mouse_is_locked = 1;
    }
}

/// Release the cursor confinement (ports `mouse_unlock` from `mouse.c`).
pub fn unlock_cursor() {
    let (adj, hwnd, center_fix) = {
        let st = state().lock().unwrap();
        (st.adjmouse, st.hwnd, st.center_cursor_fix)
    };
    if !adj || hwnd.is_invalid() {
        return;
    }
    let was_locked = {
        let mut st = state().lock().unwrap();
        let w = st.mouse_is_locked != 0;
        st.mouse_is_locked = 0;
        w
    };
    if !was_locked {
        return;
    }

    unsafe {
        let _ = ClipCursor(None);
        if center_fix {
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let _ = GetClientRect(hwnd, &mut rc);
            let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let _ = GetWindowRect(hwnd, &mut wr);
            let x = wr.left + (rc.left + rc.right) / 2;
            let y = wr.top + (rc.top + rc.bottom) / 2;
            let _ = SetCursorPos(x, y);
        }
    }
}

/// Convert the on-screen cursor position into the game's coordinate space.
///
/// Returns `false` (and leaves `*out` untouched) when adjmouse is disabled or
/// there is no window; the caller then forwards to the real `GetCursorPos`.
pub fn hook_get_cursor_pos(out: *mut POINT) -> bool {
    if out.is_null() {
        return false;
    }
    let (iw, hwnd) = {
        let st = state().lock().unwrap();
        (st.adjmouse, st.hwnd)
    };
    if !iw || hwnd.is_invalid() {
        return false;
    }

    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let _ = GetClientRect(hwnd, &mut rc);
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let _ = GetWindowRect(hwnd, &mut wr);
        let relx = pt.x - (wr.left + rc.left);
        let rely = pt.y - (wr.top + rc.top);

        let (ix, iy) = {
            let st = state().lock().unwrap();
            (st.mouse_scale_ix, st.mouse_scale_iy)
        };
        (*out).x = (relx as f64 * ix) as i32;
        (*out).y = (rely as f64 * iy) as i32;
    }
    true
}
