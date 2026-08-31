//! Window procedure and window management helpers.
//!
//! Ports the window handling portions of `IDirectDraw.c` / `main.c`.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::DDSCL_FULLSCREEN;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::state::{RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, TIMER_FIX_WINDOWPOS, state};

// Virtual key constants (numeric to avoid enum casts).
const VK_TAB: i32 = 0x09;
const VK_END: i32 = 0x23;
const VK_PRIOR: i32 = 0x21;
const VK_NEXT: i32 = 0x22;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_RMENU: i32 = 0xA5;
const VK_RCONTROL: i32 = 0xA3;
const VK_F4: i32 = 0x73;
const SC_CLOSE: u32 = 0xF060;

/// Replace the window's WndProc with our own and return the previous one.
pub(crate) unsafe fn subclass(hwnd: HWND) -> usize {
    let new_proc = wnd_proc as usize as isize;
    let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, new_proc as _);
    prev as usize
}

/// Replacement window procedure. Forwards unhandled messages to the original.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NULL => {
            // Answer the OS "is the window responsive?" probe immediately so it
            // never flags the window as Not Responding (fix_not_responding).
            return LRESULT(0);
        }
        WM_SIZE => {
            let mut st = state().lock().unwrap();
            let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd, &mut wr);
            st.win_rect = wr;
            st.render.invalidate = true;
            let center = st.center_window;
            let fixchilds = st.fixchilds;
            drop(st);
            if fixchilds > 0 {
                fix_childs(hwnd);
            }
            if center == 2 {
                maybe_recenter(hwnd);
            }
            crate::overlay::sync();
        }
        WM_MOVE => {
            let mut st = state().lock().unwrap();
            let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd, &mut wr);
            st.win_rect = wr;
            let center = st.center_window;
            drop(st);
            if center == 2 {
                maybe_recenter(hwnd);
            }
            crate::overlay::sync();
        }
        WM_DISPLAYCHANGE => {
            let center = { state().lock().unwrap().center_window };
            if center == 2 {
                maybe_recenter(hwnd);
            }
        }
        WM_ACTIVATEAPP => {
            let (noactivate, fix_alt) = {
                let st = state().lock().unwrap();
                (st.noactivateapp, st.fix_alt_key_stuck)
            };
            if noactivate {
                // Don't steal focus on app activation.
                return LRESULT(0);
            }
            let mut st = state().lock().unwrap();
            st.focus_gained = if wparam.0 != 0 { 1 } else { 0 };
            drop(st);
            if wparam.0 != 0 {
                if fix_alt {
                    reset_alt_key();
                }
                // Force a repaint now that we're back in the foreground, so
                // the window never lingers on a stale/black frame after
                // alt-tab or the Win-key start menu.
                crate::state::mark_dirty();
            }
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) != 0;
            let (noactivate, fix_alt, dw_flags) = {
                let st = state().lock().unwrap();
                (st.noactivateapp, st.fix_alt_key_stuck, st.dw_flags)
            };
            if noactivate {
                // noactivateapp: don't apply focus-activation effects (never
                // grab activation / don't run the mouse-lock block below).
                return LRESULT(0);
            }
            if !active && fix_alt {
                reset_alt_key();
            }
            let mut st = state().lock().unwrap();
            st.focus_gained = if active { 1 } else { 0 };
            drop(st);
            apply_window_style();
            if active {
                mouse_lock();
                // Kick a redraw (cnc-ddraw releases its render semaphore on
                // activation): the game resumes rendering on its own schedule
                // and may not mark the surface immediately.
                crate::state::mark_dirty();
            } else {
                if (dw_flags & DDSCL_FULLSCREEN as u32) != 0 {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                mouse_unlock(false);
            }
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            mouse_lock();
        }
        675 => {
            // WM_MOUSELEAVE
            mouse_unlock(false);
        }
        WM_CLOSE => {
            if state().lock().unwrap().terminate_process {
                std::process::exit(0);
            }
            mouse_unlock(false);
        }
        WM_PARENTNOTIFY => {
            // TS menu redraw workaround: force a repaint of the client area.
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_SYSCOMMAND => {
            if (wparam.0 & 0xfff0) == SC_CLOSE as usize && state().lock().unwrap().terminate_process {
                std::process::exit(0);
            }
        }
        WM_ERASEBKGND => {
            // The renderer thread paints the whole client area; let the OS
            // erase to the window's background brush would flash black between
            // presents (flicker). Claim we handled it so nothing is erased.
            return LRESULT(1);
        }
        WM_PAINT => {
            // Tell the render thread to repaint immediately: the window was
            // invalidated (start menu / alt-tab / occlusion). The game itself
            // is often paused at that moment, so without the kick nothing
            // repaints the client area and it can stay black on return.
            // Mirrors cnc-ddraw's WM_PAINT ReleaseSemaphore.
            let needs_repaint = state().lock().unwrap().primary.is_some();
            if needs_repaint {
                crate::state::mark_dirty();
            }
            // Validate the update region without repainting: our renderer
            // thread owns the pixels. Prevents the OS from painting/erasing.
            let _ = ValidateRect(Some(hwnd), None);
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            if crate::overlay::is_open() {
                let key = wparam.0 as i32;
                if crate::overlay::on_key(key) {
                    return LRESULT(0);
                }
            }
            let repeat = (lparam.0 & 0x4000_0000) != 0;
            handle_hotkeys(hwnd, wparam.0 as i32, repeat);
        }
        WM_SYSKEYDOWN => {
            if crate::overlay::is_open() {
                let key = wparam.0 as i32;
                if crate::overlay::on_key(key) {
                    return LRESULT(0);
                }
            }
            // Let Alt+F4 fall through to the default handler; everything else
            // is checked against the configured hotkeys.
            if wparam.0 as i32 != VK_F4 {
                let repeat = (lparam.0 & 0x4000_0000) != 0;
                handle_hotkeys(hwnd, wparam.0 as i32, repeat);
            }
        }
        WM_TIMER => {
            if wparam.0 == TIMER_FIX_WINDOWPOS {
                let (gw, gh) = {
                    let st = state().lock().unwrap();
                    (st.width, st.height)
                };
                set_window_size(gw, gh);
            } else if state().lock().unwrap().fix_not_responding {
                // Drain pending WM_NULL probes so the OS never marks us hung.
                let mut msg = std::mem::zeroed::<MSG>();
                while PeekMessageW(&mut msg, None, WM_NULL, WM_NULL, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
        }
        _ => {}
    }

    let prev = {
        let st = state().lock().unwrap();
        st.wnd_proc as usize
    };
    if prev != 0 {
        let prev_proc: WNDPROC = std::mem::transmute::<usize, WNDPROC>(prev);
        CallWindowProcW(prev_proc, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Keyboard hotkeys (WM_KEYDOWN / WM_SYSKEYDOWN). `repeat` is true for
/// auto-repeat key down events, used to debounce one-shot toggles.
unsafe fn handle_hotkeys(hwnd: HWND, key: i32, repeat: bool) {
    let ctrl = GetAsyncKeyState(VK_CONTROL) < 0;
    let rmenu = GetAsyncKeyState(VK_RMENU) < 0;
    let rctrl = GetAsyncKeyState(VK_RCONTROL) < 0;

    // Configured hotkey codes + mode snapshot (0 disables each key).
    let (k_fs, k_fs2, k_max, k_max2, k_un1, k_un2, k_shot, k_cfg, adj, savesettings) = {
        let st = state().lock().unwrap();
        (
            st.keytogglefullscreen,
            st.keytogglefullscreen2,
            st.keytogglemaximize,
            st.keytogglemaximize2,
            st.keyunlockcursor1,
            st.keyunlockcursor2,
            st.keyscreenshot,
            st.keyconfig,
            st.adjmouse,
            st.savesettings,
        )
    };

    // In-game config overlay hotkey (Ctrl+Alt+F11 by default). Requires the
    // modifiers so the bare F11 / Alt key stays functional for the game.
    if k_cfg != 0 && key == k_cfg && !repeat {
        let alt = GetAsyncKeyState(VK_MENU) < 0;
        if ctrl && alt {
            crate::dd_log!("hotkey: toggle config overlay (Ctrl+Alt, key=0x{:X})", k_cfg);
            crate::overlay::toggle();
            return;
        }
    }

    // Screenshot key (F12 by default). Guard against auto-repeat so holding
    // the key doesn't produce a burst of files.
    if k_shot != 0 && key == k_shot && !repeat {
        crate::dd_log!("hotkey: screenshot (key=0x{:X})", k_shot);
        crate::screenshot::screenshot();
    }

    // Unlock / re-lock the cursor with the configured keys (guarded by adjmouse).
    let unlock_pressed = (k_un1 != 0 && key == k_un1 && ctrl) || (k_un2 != 0 && key == k_un2 && rctrl);
    if unlock_pressed && adj {
        toggle_mouse_lock();
    }

    // Toggle fullscreen (primary key requires Alt — Alt+Enter by default; the
    // "2" variant works standalone).
    if !repeat {
        if (k_fs != 0 && key == k_fs && alt()) || (k_fs2 != 0 && key == k_fs2) {
            crate::dd_log!("hotkey: toggle fullscreen");
            toggle_fullscreen();
        }
        if (k_max != 0 && key == k_max && alt()) || (k_max2 != 0 && key == k_max2) {
            crate::dd_log!("hotkey: toggle maximize");
            toggle_maximize(savesettings != 0);
        }
    }

    // ---- existing rs-ddraw debug shortcuts (not config-driven) ----
    let mut st = state().lock().unwrap();
    if ctrl {
        match key {
            VK_END => {
                if st.auto_renderer {
                    let r = st.renderer;
                    drop(st);
                    // Cycle renderer: GDI -> OpenGL -> D3D9 -> GDI.
                    let next = if r == RENDERER_GDI {
                        RENDERER_OPENGL
                    } else if r == RENDERER_OPENGL {
                        RENDERER_D3D9
                    } else {
                        RENDERER_GDI
                    };
                    crate::state::set_renderer(next);
                    // Force the running renderer to re-init.
                    let mut s = state().lock().unwrap();
                    s.render.invalidate = true;
                }
            }
            VK_PRIOR => {
                // PageUp: increase target FPS by 5.
                let tfps = (st.target_fps + 5.0).min(1000.0);
                st.target_fps = tfps;
                st.target_frame_len = 1000.0 / tfps;
            }
            VK_NEXT => {
                // PageDown: decrease target FPS by 5 (min 1).
                let tfps = (st.target_fps - 5.0).max(1.0);
                st.target_fps = tfps;
                st.target_frame_len = 1000.0 / tfps;
            }
            _ => {}
        }
    } else if rmenu && rctrl {
        if adj {
            toggle_mouse_lock();
        }
    } else if rctrl && key == 'R' as i32 {
        st.draw_fps = !st.draw_fps;
    }
    let _ = hwnd;
}

/// Whether the Alt / Menu key is currently held.
fn alt() -> bool {
    unsafe { GetAsyncKeyState(VK_MENU) < 0 }
}

/// Toggle cursor lock using the shared mouse helpers (adjmouse-guarded).
fn toggle_mouse_lock() {
    if crate::mouse::is_locked() {
        crate::dd_log!("hotkey: unlock cursor");
        crate::mouse::unlock_cursor();
    } else {
        crate::dd_log!("hotkey: lock cursor");
        crate::mouse::lock_cursor();
    }
}

/// Toggle between fullscreen-exclusive and windowed mode. Flips the
/// `DDSCL_FULLSCREEN` cooperative flag and re-runs the resize path so the
/// window is sized appropriately.
pub(crate) unsafe fn toggle_fullscreen() {
    let (hwnd, gw, gh, bpp, fs) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.width, st.height, st.bpp, (st.dw_flags & DDSCL_FULLSCREEN as u32) != 0)
    };
    if hwnd.is_invalid() {
        return;
    }
    mouse_unlock(false);
    {
        let mut st = state().lock().unwrap();
        if fs {
            st.dw_flags &= !(DDSCL_FULLSCREEN as u32);
        } else {
            st.dw_flags |= DDSCL_FULLSCREEN as u32;
        }
    }
    set_window_size(gw, gh);
    set_display_mode(gw, gh, bpp);
}

/// Toggle borderless-maximized mode (border off + maximized) back to a normal
/// bordered window, and vice-versa. When `persist` is set the border change is
/// written to the ini.
pub(crate) unsafe fn toggle_maximize(persist: bool) {
    let (hwnd, gw, gh, resizable, maximized) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.width, st.height, st.resizable, (st.dw_flags & DDSCL_FULLSCREEN as u32) != 0)
    };
    if hwnd.is_invalid() || !resizable || maximized {
        return;
    }
    if IsZoomed(hwnd).as_bool() {
        {
            let mut st = state().lock().unwrap();
            st.border = true;
        }
        apply_window_style();
        let _ = ShowWindow(hwnd, SW_RESTORE);
        set_window_size(gw, gh);
        if persist {
            crate::config::save_setting("Border", "true");
        }
    } else {
        {
            let mut st = state().lock().unwrap();
            st.border = false;
        }
        apply_window_style();
        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        if persist {
            crate::config::save_setting("Border", "false");
        }
    }
}

/// Clear a stuck ALT / VK_MENU key (focus-loss workaround, fix_alt_key_stuck).
unsafe fn reset_alt_key() {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(VK_MENU as u16), dwFlags: KEYEVENTF_KEYUP, ..Default::default() },
        },
    };
    let cbsize = std::mem::size_of::<INPUT>() as i32;
    let _ = SendInput(&[input], cbsize);
}

/// Re-layout every child window to fill the parent's client area
/// (fixchilds). Mirrors cnc-ddraw's child-window resize handling.
unsafe fn fix_childs(hwnd: HWND) {
    EnumChildWindows(Some(hwnd), Some(fix_child_proc), LPARAM(hwnd.0 as isize));
}

extern "system" fn fix_child_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        let parent = HWND(lparam.0 as _);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(parent, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w > 0 && h > 0 {
            let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
    windows::core::BOOL(1)
}

/// Compute the render viewport (MaintainAspectRatio / Windowboxing /
/// StretchToFullscreen) from the game resolution. Updates `state.render`.
fn compute_viewport(game_w: i32, game_h: i32) {
    let (cfg_maintain, cfg_windowbox, cfg_stretch_fs, stretch_w, stretch_h, screen_w, screen_h) = {
        let st = state().lock().unwrap();
        (
            st.maintain_aspect_ratio,
            st.windowboxing,
            st.stretch_to_fullscreen,
            st.stretch_to_width,
            st.stretch_to_height,
            st.screen_width,
            st.screen_height,
        )
    };

    let gw = game_w.max(1);
    let gh = game_h.max(1);

    let mut render_w = stretch_w;
    let mut render_h = stretch_h;
    if cfg_stretch_fs {
        render_w = screen_w;
        render_h = screen_h;
    }
    if render_w < gw {
        render_w = gw;
    }
    if render_h < gh {
        render_h = gh;
    }

    let mut vp_left = 0;
    let mut vp_top = 0;
    let mut vp_w = render_w;
    let mut vp_h = render_h;

    if cfg_windowbox {
        vp_w = gw;
        vp_h = gh;
        for i in (2..=20).rev() {
            if gw * i <= render_w && gh * i <= render_h {
                vp_w *= i;
                vp_h *= i;
                break;
            }
        }
        vp_top = render_h / 2 - vp_h / 2;
        vp_left = render_w / 2 - vp_w / 2;
    } else if cfg_maintain {
        vp_w = render_w;
        vp_h = ((gh as f32 / gw as f32) * vp_w as f32) as i32;
        if vp_h > render_h {
            vp_w = ((vp_w as f32 / vp_h as f32) * render_h as f32) as i32;
            vp_h = render_h;
        }
        vp_top = render_h / 2 - vp_h / 2;
        vp_left = render_w / 2 - vp_w / 2;
    }

    let mut st = state().lock().unwrap();
    st.render.width = render_w;
    st.render.height = render_h;
    st.render.viewport.left = vp_left;
    st.render.viewport.top = vp_top;
    st.render.viewport.right = vp_left + vp_w;
    st.render.viewport.bottom = vp_top + vp_h;
    st.render.scale_w = vp_w as f32 / gw as f32;
    st.render.scale_h = vp_h as f32 / gh as f32;
    st.render.stretched = vp_w != gw || vp_h != gh || vp_left != 0 || vp_top != 0;
    st.render.invalidate = true;
}

// Window style constants (`GetWindowLongPtrW` / `SetWindowLongW` GWL_STYLE).
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;

/// Applied the window style / centering once per window. Idempotent via a
/// static flag so it runs on the first `WM_ACTIVATE` and at the start of
/// `set_window_size` without re-applying later.
static STYLE_APPLIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while this wrapper is itself repositioning the window (center / pos).
/// Consumed by the `WM_MOVE` / `WM_SIZE` re-center path to avoid an infinite
/// set <-> message loop when `center_window == 2` (always recenter).
static CENTERING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Apply border / resizable window styles and center the window, per config.
///
/// `border=false` removes the caption and system menu; `resizable=false`
/// removes the resize frame and maximize box. When `border=true` the caption,
/// system menu and minimize box are restored, and when `resizable=true` the
/// resize frame and maximize box are added back. `remove_menu` strips the
/// system menu on top of the border logic. On the first call the window is
/// either positioned at `pos_x`/`pos_y` (when set) or centered according to
/// the `center_window` mode (1 = once at creation).
pub(crate) unsafe fn apply_window_style() {
    let (border, resizable, remove_menu, center, pos_x, pos_y, hwnd) = {
        let st = state().lock().unwrap();
        (st.border, st.resizable, st.remove_menu, st.center_window, st.pos_x, st.pos_y, st.hwnd)
    };
    if hwnd.is_invalid() {
        return;
    }

    let old_style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let mut style = old_style;
    if resizable {
        style |= WS_THICKFRAME | WS_MAXIMIZEBOX;
    } else {
        style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
    }
    if border {
        style |= WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME;
    } else {
        style &= !(WS_CAPTION | WS_SYSMENU);
    }
    if remove_menu {
        style &= !WS_SYSMENU;
    }
    if style != old_style {
        SetWindowLongW(hwnd, GWL_STYLE, style as i32);
    }

    let first = !STYLE_APPLIED.swap(true, std::sync::atomic::Ordering::Relaxed);
    if first {
        if pos_x >= 0 && pos_y >= 0 {
            position_window(hwnd, pos_x, pos_y);
        } else if center > 0 {
            center_window(hwnd);
        }
    }
}

/// Move the window to an explicit (x, y) position without resizing it.
unsafe fn position_window(hwnd: HWND, x: i32, y: i32) {
    CENTERING.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = SetWindowPos(hwnd, Some(HWND_TOP), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
}

/// Center the window on the primary screen (keeps its current size).
unsafe fn center_window(hwnd: HWND) {
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let x = screen_w / 2 - w / 2;
    let y = screen_h / 2 - h / 2;
    position_window(hwnd, x, y);
}

/// Re-center the window when `center_window == 2` (always). Skips the move /
/// size message produced by our own `SetWindowPos` to avoid re-entrancy.
unsafe fn maybe_recenter(hwnd: HWND) {
    if !CENTERING.swap(false, std::sync::atomic::Ordering::Relaxed) {
        center_window(hwnd);
    }
}

/// Recompute the render viewport only (used when the game resizes our window).
pub(crate) unsafe fn recompute_viewport(game_w: i32, game_h: i32) {
    compute_viewport(game_w, game_h);
}

/// Resize and reposition the game window, computing the render viewport
/// (MaintainAspectRatio / Windowboxing / StretchToFullscreen).
///
/// `game_w`/`game_h` are the game's requested resolution (surface size).
pub(crate) unsafe fn set_window_size(game_w: i32, game_h: i32) {
    crate::dd_log!("set_window_size: begin compute_viewport");
    apply_window_style();
    compute_viewport(game_w, game_h);
    crate::dd_log!("set_window_size: compute_viewport done");

    let (hwnd, fullscreen, screen_w, screen_h, render_w, render_h) = {
        let st = state().lock().unwrap();
        (
            st.hwnd,
            (st.dw_flags & DDSCL_FULLSCREEN as u32) != 0,
            st.screen_width,
            st.screen_height,
            st.render.width,
            st.render.height,
        )
    };
    crate::dd_log!("set_window_size: locked ({})", if fullscreen { "fullscreen" } else { "windowed" });
    if !hwnd.is_invalid() {
        let (w, h) = if fullscreen { (screen_w, screen_h) } else { (render_w, render_h) };
        crate::dd_log!("set_window_size: SetWindowPos begin {}x{}", w, h);
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, w, h, SWP_SHOWWINDOW);
        crate::dd_log!("set_window_size: SetWindowPos done");
    }
}

/// Change the display resolution (fullscreen mode).
///
/// ts-ddraw forwards `SetDisplayMode` to the real DirectDraw and does not call
/// `ChangeDisplaySettings` itself; the wrapper window *is* the display. Calling
/// `ChangeDisplaySettingsA` here blocks on the display-driver reset (especially
/// for large resolutions like 2560x1600) and freezes the game, so we only
/// record state and confine the cursor.
pub(crate) unsafe fn set_display_mode(_width: i32, _height: i32, _bpp: i32) -> bool {
    let mut st = state().lock().unwrap();
    st.focus_gained = 1;
    drop(st);
    mouse_lock();
    true
}

/// Confine the cursor to the window's client area.
pub(crate) unsafe fn mouse_lock() {
    // Keep the cursor free while the config overlay is open (the menu is a
    // WS_EX_NOACTIVATE window, so the game stays focused and would otherwise
    // re-clip the pointer on every activation).
    if crate::overlay::is_open() {
        return;
    }
    crate::dd_log!("mouse_lock: begin");
    let hwnd = {
        let st = state().lock().unwrap();
        st.hwnd
    };
    if hwnd.is_invalid() {
        return;
    }
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(hwnd, &mut rc);
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(hwnd, &mut wr);
    let clip =
        RECT { left: wr.left + rc.left, top: wr.top + rc.top, right: wr.left + rc.right, bottom: wr.top + rc.bottom };
    crate::dd_log!("mouse_lock: ClipCursor begin");
    let _ = ClipCursor(Some(&clip));
    crate::dd_log!("mouse_lock: ClipCursor done");
    let mut st = state().lock().unwrap();
    st.mouse_is_locked = 1;
}

/// Release the cursor confinement and optionally show it.
pub(crate) unsafe fn mouse_unlock(show: bool) {
    let _ = ClipCursor(None);
    if show {
        let _ = ShowCursor(true);
    } else {
        let _ = ShowCursor(false);
    }
    let mut st = state().lock().unwrap();
    st.mouse_is_locked = 0;
}

/// Center the cursor within the window's client area.
pub(crate) unsafe fn center_mouse(hwnd: HWND) {
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(hwnd, &mut rc);
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(hwnd, &mut wr);
    let x = wr.left + (rc.left + rc.right) / 2;
    let y = wr.top + (rc.top + rc.bottom) / 2;
    let _ = SetCursorPos(x, y);
}
