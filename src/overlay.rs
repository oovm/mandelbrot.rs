//! In-game, semi-transparent configuration overlay.
//!
//! A keyboard-driven GDI panel rendered in a `WS_EX_LAYERED` top-most window
//! over the game window (the cnc-ddraw equivalent of the separate config
//! program, minus the external tool). While the overlay is open the game's
//! subclassed `WndProc` (window.rs) forwards navigation keys here; every edit
//! is applied to the live `DDrawState` immediately and (optionally) persisted
//! back to the ini through `config::save_setting`.
//!
//! Key handling: the overlay never has input focus (`WS_EX_NOACTIVATE` +
//! `WS_EX_TRANSPARENT`), so navigation keys arrive on the game window's
//! WndProc which delegates them to [`on_key`].

use std::sync::{Mutex, MutexGuard, Once};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::state::{state, DDrawState, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, RENDERER_OPENGL_CORE};

// Virtual key codes used by the overlay navigation.
const VK_UP: i32 = 0x26;
const VK_DOWN: i32 = 0x28;
const VK_LEFT: i32 = 0x25;
const VK_RIGHT: i32 = 0x27;
const VK_RETURN: i32 = 0x0D;
const VK_ESCAPE: i32 = 0x1B;
const VK_SPACE: i32 = 0x20;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_W: i32 = 0x57;
const VK_S: i32 = 0x53;
const VK_A: i32 = 0x41;
const VK_D: i32 = 0x44;

/// Item kinds. `Bool`/`Int`/`Choice` change live state via the per-item
/// `code`; `Action` runs a one-shot helper; `Text` is a read-only display row.
#[derive(Clone, Copy)]
enum Kind {
    Bool,
    Int(i32, i32, i32), // min, max, step
    Choice(&'static [&'static str]),
    Action,
    Text,
}

#[derive(Clone, Copy)]
struct Item {
    label: &'static str,
    kind: Kind,
    code: u8,
}

const CFG_RENDERER: &[&str] = &["自动", "D3D9", "OpenGL", "OpenGL核心", "GDI"];
const CFG_FILTER: &[&str] = &["最近邻", "双线性", "Catmull-Rom", "Lanczos", "xBR"];
const CFG_SHADER: &[&str] = &["默认", "nearest", "bilinear", "catmull-rom", "lanczos", "xbr"];
const CFG_LIMITER: &[&str] = &["自动", "TestCooperativeLevel", "BltFast", "Unlock", "PeekMessage"];
const CFG_CENTER: &[&str] = &["从不", "自动居中", "总是居中"];
const CFG_RESOLUTIONS: &[&str] = &["小", "中", "完整"];
const CFG_FAKEVER: &[&str] = &["真实版本", "Windows XP", "Windows Vista", "Windows 7"];

const PAGES: &[&str] = &[
    "渲染设置 Renderer",
    "窗口设置 Window",
    "显示设置 Display",
    "鼠标设置 Mouse",
    "兼容设置 Compat",
    "杂项 Misc",
];

const ITEMS_RENDERER: &[Item] = &[
    Item { label: "渲染器 Renderer", kind: Kind::Choice(CFG_RENDERER), code: 0 },
    Item { label: "滤镜 Filter", kind: Kind::Choice(CFG_FILTER), code: 1 },
    Item { label: "着色器 Shader", kind: Kind::Choice(CFG_SHADER), code: 2 },
    Item { label: "垂直同步 VSync", kind: Kind::Bool, code: 3 },
    Item { label: "最大FPS MaxFPS", kind: Kind::Int(0, 1000, 5), code: 4 },
    Item { label: "最小FPS MinFPS", kind: Kind::Int(0, 200, 5), code: 5 },
    Item { label: "逻辑帧率 MaxGameTicks", kind: Kind::Int(-2, 120, 1), code: 6 },
    Item { label: "限速方式 LimiterType", kind: Kind::Choice(CFG_LIMITER), code: 7 },
];

const ITEMS_WINDOW: &[Item] = &[
    Item { label: "窗口居中 CenterWindow", kind: Kind::Choice(CFG_CENTER), code: 0 },
    Item { label: "窗口边框 Border", kind: Kind::Bool, code: 1 },
    Item { label: "允许缩放 Resizable", kind: Kind::Bool, code: 2 },
    Item { label: "► 切换最大化", kind: Kind::Action, code: 3 },
    Item { label: "► 切换全屏", kind: Kind::Action, code: 4 },
    Item { label: "自定义宽度 Width", kind: Kind::Int(0, 10240, 16), code: 5 },
    Item { label: "自定义高度 Height", kind: Kind::Int(0, 7680, 16), code: 6 },
    Item { label: "窗口X PosX", kind: Kind::Int(-32000, 32767, 8), code: 7 },
    Item { label: "窗口Y PosY", kind: Kind::Int(-32000, 32767, 8), code: 8 },
];

const ITEMS_DISPLAY: &[Item] = &[
    Item { label: "保持纵横比 MaintainAspectRatio", kind: Kind::Bool, code: 0 },
    Item { label: "黑边模式 Windowboxing", kind: Kind::Bool, code: 1 },
    Item { label: "拉伸填充 StretchToFullscreen", kind: Kind::Bool, code: 2 },
    Item { label: "刷新率 RefreshRate", kind: Kind::Int(0, 240, 1), code: 3 },
    Item { label: "模式列表大小 Resolutions", kind: Kind::Choice(CFG_RESOLUTIONS), code: 4 },
    Item { label: "最大模式数 MaxResolutions", kind: Kind::Int(0, 200, 4), code: 5 },
    Item { label: "注入分辨率 InjectResolution", kind: Kind::Text, code: 6 },
    Item { label: "伪模式 FakeMode", kind: Kind::Text, code: 7 },
];

const ITEMS_MOUSE: &[Item] = &[
    Item { label: "鼠标修正 AdjMouse", kind: Kind::Bool, code: 0 },
    Item { label: "► 锁定/释放光标", kind: Kind::Action, code: 1 },
    Item { label: "锁定到左上角 LockMouseTopLeft", kind: Kind::Bool, code: 2 },
    Item { label: "中心修正 CenterCursorFix", kind: Kind::Bool, code: 3 },
];

const ITEMS_COMPAT: &[Item] = &[
    Item { label: "非独占模式 Nonexclusive", kind: Kind::Bool, code: 0 },
    Item { label: "TS渲染修正 TS hack", kind: Kind::Bool, code: 1 },
    Item { label: "FlipClear", kind: Kind::Bool, code: 2 },
    Item { label: "锁定表面 LockSurfaces", kind: Kind::Bool, code: 3 },
    Item { label: "不激活 NoActivateApp", kind: Kind::Bool, code: 4 },
    Item { label: "修复未响应 FixNotResponding", kind: Kind::Bool, code: 5 },
    Item { label: "修复Alt卡键 FixAltKeyStuck", kind: Kind::Bool, code: 6 },
    Item { label: "修复子窗口 FixChilds", kind: Kind::Int(0, 4, 1), code: 7 },
    Item { label: "去除菜单 RemoveMenu", kind: Kind::Bool, code: 8 },
    Item { label: "强制退出 TerminateProcess", kind: Kind::Bool, code: 9 },
    Item { label: "伪系统版本 FakeVersion", kind: Kind::Choice(CFG_FAKEVER), code: 10 },
];

const ITEMS_MISC: &[Item] = &[
    Item { label: "自动保存 SaveSettings", kind: Kind::Int(0, 2, 1), code: 0 },
    Item { label: "显示FPS DrawFPS", kind: Kind::Bool, code: 1 },
    Item { label: "钩子PeekMessage", kind: Kind::Bool, code: 2 },
    Item { label: "禁用DInput钩子", kind: Kind::Bool, code: 3 },
    Item { label: "► 截图 PNG", kind: Kind::Action, code: 4 },
    Item { label: "着色器目录 ShaderPath", kind: Kind::Text, code: 5 },
    Item { label: "Pass1着色器 ShaderPath.Pass1", kind: Kind::Text, code: 6 },
];

const ALL_PAGES: &[&[Item]] =
    &[ITEMS_RENDERER, ITEMS_WINDOW, ITEMS_DISPLAY, ITEMS_MOUSE, ITEMS_COMPAT, ITEMS_MISC];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Pages,
    Items,
}

struct Overlay {
    hwnd: Option<HWND>,
    font: Option<HFONT>,
    page: usize,
    item: usize,
    scroll: usize,
    mode: Mode,
}

static OVERLAY: Mutex<Overlay> = Mutex::new(Overlay {
    hwnd: None,
    font: None,
    page: 0,
    item: 0,
    scroll: 0,
    mode: Mode::Pages,
});

static REGISTER_ONCE: Once = Once::new();
// Class name and window title must live forever: keep them in static buffers
// (never dropped) so a long-lived pointer from the window-class / window
// structures can never dangle or be overwritten by reused stack.
static OVERLAY_CLASS_NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
static OVERLAY_TITLE: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();

fn overlay_class_name() -> &'static [u16] {
    OVERLAY_CLASS_NAME.get_or_init(|| utf16(b"rsd_overlay_cls"))
}

fn overlay_title() -> &'static [u16] {
    OVERLAY_TITLE.get_or_init(|| utf16(b"rsd_overlay"))
}

// The overlay is only ever touched from the game's UI thread; the Mutex guards
// the menu cursor across the WndProc reentry that window resizing triggers.
// The Win32 handle wrappers are not `Send`, so assert it manually (matching
// the existing `unsafe impl Send/Sync` on DDrawState).
unsafe impl Send for Overlay {}
unsafe impl Sync for Overlay {}

fn lock() -> MutexGuard<'static, Overlay> {
    OVERLAY.lock().unwrap()
}

fn utf16(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().map(|&b| b as u16).chain(std::iter::once(0)).collect::<Vec<u16>>()
}

fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Whether the overlay window is currently shown.
pub(crate) fn is_open() -> bool {
    lock().hwnd.is_some()
}

/// Toggle the overlay open/closed. Must be called from the game's UI thread
/// (the same thread that owns the WndProc the hotkey arrives on).
pub(crate) unsafe fn toggle() {
    if is_open() {
        close();
    } else {
        show();
    }
}

/// Show the overlay over the game window.
pub(crate) unsafe fn show() {
    let game = state().lock().unwrap().hwnd;
    if game.is_invalid() {
        crate::dd_log!("overlay: no game window, cannot show");
        return;
    }
    let created = {
        let mut o = lock();
        if o.hwnd.is_some() {
            None
        } else {
            REGISTER_ONCE.call_once(|| {
                register_class();
            });
            let class_name: PCWSTR = PCWSTR(overlay_class_name().as_ptr());
            let title = overlay_title();
            let ex_style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            match CreateWindowExW(
                ex_style,
                class_name,
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                0, 0, 320, 200,
                None,
                None,
                None,
                None,
            ) {
                Ok(hwnd) => {
                    o.hwnd = Some(hwnd);
                    o.font = create_font();
                    o.page = 0;
                    o.item = 0;
                    o.scroll = 0;
                    o.mode = Mode::Pages;
                    Some(hwnd)
                }
                Err(e) => {
                    crate::dd_log!("overlay: CreateWindowExW failed: {:?}", e);
                    None
                }
            }
        }
    };
    let Some(hwnd) = created else { return };
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 205, LWA_ALPHA);
    // Release the game's cursor clip and bring the pointer back so the menu is
    // usable. The overlay is WS_EX_NOACTIVATE, so without this the pointer stays
    // trapped (and hidden) in the game area.
    crate::window::mouse_unlock(true);
    sync();
    repaint();
    crate::dd_log!("overlay: shown");
}

/// Reposition/resize the overlay to cover the game's client area. Called from
/// the game WndProc on WM_SIZE/WM_MOVE and after every navigation key.
pub(crate) unsafe fn sync() {
    let hwnd = { lock().hwnd };
    let Some(hwnd) = hwnd else { return };
    let game = state().lock().unwrap().hwnd;
    if game.is_invalid() {
        close();
        return;
    }
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(game, &mut rc);
    let mut pt = POINT { x: rc.left, y: rc.top };
    let _ = ClientToScreen(game, &mut pt);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), pt.x, pt.y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
}

/// Close and destroy the overlay.
pub(crate) fn close() {
    crate::dd_log!("overlay: close");
    let (hwnd, font) = {
        let mut o = lock();
        let hwnd = o.hwnd.take();
        let font = o.font.take();
        o.page = 0;
        o.item = 0;
        o.scroll = 0;
        o.mode = Mode::Pages;
        (hwnd, font)
    };
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    if let Some(f) = font {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
    }
    // Re-trap and hide the cursor if the game is still focused: the overlay
    // was unlocked on open, and mouse_lock() no-ops while the overlay exists,
    // so this must run after the overlay is destroyed.
    if crate::state::focused() {
        unsafe {
            crate::window::mouse_lock();
        }
    }
}

/// Destroy the overlay on DLL detach.
pub(crate) fn cleanup() {
    close();
}

/// Handle a key pressed while the overlay is open. Returns true when the key
/// was consumed by the overlay (the game must not receive it).
pub(crate) unsafe fn on_key(key: i32) -> bool {
    if key == VK_ESCAPE {
        close();
        return true;
    }
    let kcfg = state().lock().unwrap().keyconfig;
    // The configured hotkey closes the overlay too, but like opening it the
    // bare key alone must not: require Ctrl+Alt as well.
    if kcfg != 0 && key == kcfg {
        let ctrl = GetAsyncKeyState(VK_CONTROL) < 0;
        let alt = GetAsyncKeyState(VK_MENU) < 0;
        if ctrl && alt {
            close();
            return true;
        }
    }
    let ctrl = GetAsyncKeyState(VK_CONTROL) < 0;

    // Deferred work: value adjustments / actions must run *without* the
    // OVERLAY mutex held, because adjusting a window setting can synchronously
    // send WM_SIZE/WM_MOVE to the game window whose WndProc calls sync().
    enum Pending {
        None,
        Adjust(i32),
        Action,
    }
    let mut pending = Pending::None;

    {
        let mut o = lock();
        if o.hwnd.is_none() {
            return false;
        }
        let pages_len = ALL_PAGES.len();
        let items = ALL_PAGES[o.page];
        match (o.mode, key) {
            (Mode::Pages, k) if k == VK_UP || k == VK_W => {
                o.page = (o.page + pages_len - 1) % pages_len;
            }
            (Mode::Pages, k) if k == VK_DOWN || k == VK_S => {
                o.page = (o.page + 1) % pages_len;
            }
            (Mode::Pages, k) if k == VK_RETURN || k == VK_RIGHT || k == VK_SPACE || k == VK_D => {
                o.mode = Mode::Items;
                o.item = 0;
                o.scroll = 0;
            }
            (Mode::Items, k) if ctrl && k == VK_UP => {
                o.page = (o.page + pages_len - 1) % pages_len;
                o.mode = Mode::Pages;
            }
            (Mode::Items, k) if ctrl && k == VK_DOWN => {
                o.page = (o.page + 1) % pages_len;
                o.mode = Mode::Pages;
            }
            (Mode::Items, k) if k == VK_UP || k == VK_W => {
                if o.item > 0 {
                    o.item -= 1;
                    if o.item < o.scroll {
                        o.scroll = o.item;
                    }
                }
            }
            (Mode::Items, k) if k == VK_DOWN || k == VK_S => {
                if o.item + 1 < items.len() {
                    o.item += 1;
                }
            }
            (Mode::Items, k) if k == VK_LEFT || k == VK_A => pending = Pending::Adjust(-1),
            (Mode::Items, k) if k == VK_RIGHT || k == VK_D => pending = Pending::Adjust(1),
            (Mode::Items, k) if k == VK_RETURN || k == VK_SPACE => {
                let it = items[o.item];
                match it.kind {
                    Kind::Action => pending = Pending::Action,
                    Kind::Bool | Kind::Choice(_) | Kind::Int(..) => pending = Pending::Adjust(1),
                    Kind::Text => {}
                }
            }
            _ => return false, // key not handled by the overlay
        }
    }

    match pending {
        Pending::Adjust(dir) => {
            let (page, code) = {
                let o = lock();
                (o.page, ALL_PAGES[o.page][o.item].code)
            };
            adjust(page, code, dir);
        }
        Pending::Action => {
            let (page, code) = {
                let o = lock();
                (o.page, ALL_PAGES[o.page][o.item].code)
            };
            run_action(page, code);
        }
        Pending::None => {}
    }
    repaint();
    true
}

/// Register the overlay window class once (idempotent: a second
/// `RegisterClassW` of the same name fails with ERROR_CLASS_ALREADY_EXISTS,
/// so this runs exactly once per process).
fn register_class() -> u16 {
    let hinst = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            crate::dd_log!("overlay: GetModuleHandleW failed: {:?}", e);
            return 0;
        }
    };
    let class = overlay_class_name();
    let wc = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(overlay_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: HINSTANCE(hinst.0),
        hIcon: HICON(std::ptr::null_mut()),
        hCursor: HCURSOR(std::ptr::null_mut()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class.as_ptr()),
    };
    // SAFETY: normal class registration on the UI thread.
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        let e = unsafe { windows::Win32::Foundation::GetLastError() };
        crate::dd_log!("overlay: RegisterClassW failed, error={:#x}", e.0);
    } else {
        crate::dd_log!("overlay: RegisterClassW ok (atom={})", atom);
    }
    atom
}

unsafe extern "system" fn overlay_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_font() -> Option<HFONT> {
    let h = unsafe {
        CreateFontW(
            -22, 0, 0, 0, 400, 0, 0, 0,
            FONT_CHARSET(1),           // DEFAULT_CHARSET
            FONT_OUTPUT_PRECISION(0),
            FONT_CLIP_PRECISION(0),
            FONT_QUALITY(6),           // CLEARTYPE_NATURAL_QUALITY
            0,
            windows::core::w!("Microsoft YaHei"),
        )
    };
    if h.is_invalid() { None } else { Some(h) }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_invalid() {
        return;
    }
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);
    if w <= 0 || h <= 0 {
        let _ = EndPaint(hwnd, &ps);
        return;
    }

    // Double-buffer into a memory DC so the layered alpha blend is one paint.
    let memdc = CreateCompatibleDC(Some(hdc));
    if memdc.is_invalid() {
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    if bmp.is_invalid() {
        let _ = DeleteDC(memdc);
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let old = SelectObject(memdc, HGDIOBJ(bmp.0));

    let bg = CreateSolidBrush(COLORREF(0x1C_1C_28));
    let _ = FillRect(memdc, &rc, bg);
    let _ = DeleteObject(HGDIOBJ(bg.0));

    let (font, page, item, scroll, mode) = {
        let o = lock();
        (o.font, o.page, o.item, o.scroll, o.mode)
    };
    if let Some(f) = font {
        let _ = SelectObject(memdc, HGDIOBJ(f.0));
    }
    let _ = SetBkMode(memdc, TRANSPARENT);

    let margin = 16i32;
    let title_h = 52i32;
    let line_h = 30i32;
    let page_col_w = 210i32;
    let page_x = margin;
    let item_x = page_x + page_col_w + 16;
    let value_right = (w - 16).max(item_x + 8);

    // title + hint
    set_color(memdc, COLORREF(0x5A_9E_FF));
    draw_text(memdc, 16, 10, "rs-ddraw 实时设置");
    set_color(memdc, COLORREF(0x9E_9E_9E));
    draw_text(memdc, 16, 34, "↑↓ 选择  Enter 切换  ←→ 调整  Ctrl+↑↓ 翻页  Esc 关闭");

    // left column: page list
    for (i, name) in PAGES.iter().enumerate() {
        let y = title_h + i as i32 * line_h;
        if mode == Mode::Pages && i == page {
            draw_select_bar(memdc, page_x - 4, y - 4, page_col_w + 8, line_h, COLORREF(0x1E_5A_9E));
        }
        let prefix = if i == page { "▶ " } else { "   " };
        set_color(memdc, if mode == Mode::Pages && i == page { COLORREF(0xFF_FF_FF) } else { COLORREF(0xBE_BE_BE) });
        draw_text(memdc, page_x, y + 4, &format!("{prefix}{name}"));
    }

    // right column: current page items
    let items = ALL_PAGES[page];
    let visible = (((h - title_h - 40) / line_h).max(1)) as usize;
    for (idx, it) in items.iter().enumerate().skip(scroll).take(visible) {
        let y = title_h + (idx - scroll) as i32 * line_h;
        if mode == Mode::Items && idx == item {
            draw_select_bar(memdc, item_x - 6, y - 4, value_right - item_x + 6, line_h, COLORREF(0x1E_5A_9E));
            set_color(memdc, COLORREF(0xFF_FF_FF));
        } else {
            set_color(memdc, COLORREF(0xD8_D8_D8));
        }
        draw_text(memdc, item_x, y + 4, it.label);
        let value = read(&state().lock().unwrap(), page, it.code);
        if !value.is_empty() {
            let vcolor = if matches!(it.kind, Kind::Action) {
                COLORREF(0x8A_E0_5A)
            } else {
                COLORREF(0xFF_D9_6E)
            };
            set_color(memdc, vcolor);
            draw_text_right(memdc, value_right, y + 4, &value);
        }
    }

    let _ = BitBlt(hdc, 0, 0, w, h, Some(memdc), 0, 0, SRCCOPY);
    let _ = SelectObject(memdc, old);
    let _ = DeleteObject(HGDIOBJ(bmp.0));
    let _ = DeleteDC(memdc);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn set_color(hdc: HDC, color: COLORREF) {
    let _ = SetTextColor(hdc, color);
}

unsafe fn draw_text(hdc: HDC, x: i32, y: i32, s: &str) {
    let v = to_utf16(s);
    if !v.is_empty() {
        let _ = TextOutW(hdc, x, y, &v);
    }
}

unsafe fn draw_text_right(hdc: HDC, right: i32, y: i32, s: &str) {
    let v = to_utf16(s);
    if v.is_empty() {
        return;
    }
    let mut sz = SIZE { cx: 0, cy: 0 };
    let _ = GetTextExtentPoint32W(hdc, &v, &mut sz);
    let x = (right - sz.cx).max(0);
    let _ = TextOutW(hdc, x, y, &v);
}

unsafe fn draw_select_bar(hdc: HDC, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    let rc = RECT { left: x, top: y, right: x + w, bottom: y + h };
    let _ = FillRect(hdc, &rc, brush);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

// ---- value model: read / adjust / apply / persist ----

fn read(st: &DDrawState, page: usize, code: u8) -> String {
    match (page, code) {
        (0, 0) => renderer_choice(st).to_string(),
        (0, 1) => pick(CFG_FILTER, st.filter),
        (0, 2) => shader_choice(st).to_string(),
        (0, 3) => if st.swap_interval > 0 { "开启" } else { "关闭" }.to_string(),
        (0, 4) => st.maxfps.to_string(),
        (0, 5) => st.minfps.to_string(),
        (0, 6) => st.maxgameticks.to_string(),
        (0, 7) => pick(CFG_LIMITER, st.limiter_type),
        (1, 0) => pick(CFG_CENTER, st.center_window),
        (1, 1) => yn(st.border),
        (1, 2) => yn(st.resizable),
        (1, 5) => st.res_width.to_string(),
        (1, 6) => st.res_height.to_string(),
        (1, 7) => st.pos_x.to_string(),
        (1, 8) => st.pos_y.to_string(),
        (2, 0) => yn(st.maintain_aspect_ratio),
        (2, 1) => yn(st.windowboxing),
        (2, 2) => yn(st.stretch_to_fullscreen),
        (2, 3) => st.refresh_rate.to_string(),
        (2, 4) => pick(CFG_RESOLUTIONS, st.resolutions),
        (2, 5) => st.max_resolutions.to_string(),
        (2, 6) => st.inject_resolution.clone(),
        (2, 7) => st.fake_mode.clone(),
        (3, 0) => yn(st.adjmouse),
        (3, 2) => yn(st.lock_mouse_top_left),
        (3, 3) => yn(st.center_cursor_fix),
        (4, 0) => yn(st.nonexclusive),
        (4, 1) => yn(st.tshack),
        (4, 2) => yn(st.flipclear),
        (4, 3) => yn(st.lock_surfaces),
        (4, 4) => yn(st.noactivateapp),
        (4, 5) => yn(st.fix_not_responding),
        (4, 6) => yn(st.fix_alt_key_stuck),
        (4, 7) => st.fixchilds.to_string(),
        (4, 8) => yn(st.remove_menu),
        (4, 9) => yn(st.terminate_process),
        (4, 10) => fakever_choice(st.fake_version).to_string(),
        (5, 0) => st.savesettings.to_string(),
        (5, 1) => yn(st.draw_fps),
        (5, 2) => yn(st.hook_peekmessage),
        (5, 3) => yn(st.no_dinput_hook),
        (5, 5) => st.shaderpath.clone(),
        (5, 6) => st.shaderpath_pass1.clone(),
        _ => String::new(),
    }
}

fn yn(b: bool) -> String {
    if b { "开启".to_string() } else { "关闭".to_string() }
}

fn pick(list: &'static [&str], idx: i32) -> String {
    let i = idx.clamp(0, (list.len() - 1) as i32) as usize;
    list[i].to_string()
}

fn renderer_choice(st: &DDrawState) -> &'static str {
    if st.auto_renderer {
        CFG_RENDERER[0]
    } else {
        match st.renderer {
            RENDERER_D3D9 => CFG_RENDERER[1],
            RENDERER_OPENGL => CFG_RENDERER[2],
            RENDERER_OPENGL_CORE => CFG_RENDERER[3],
            RENDERER_GDI => CFG_RENDERER[4],
            _ => CFG_RENDERER[1],
        }
    }
}

fn shader_choice(st: &DDrawState) -> &'static str {
    let n = st.shader.trim().to_ascii_lowercase();
    if n.is_empty() {
        CFG_SHADER[0]
    } else if n.starts_with("nearest") {
        CFG_SHADER[1]
    } else if n.starts_with("bilinear") {
        CFG_SHADER[2]
    } else if n.contains("catmull") {
        CFG_SHADER[3]
    } else if n.contains("lanczos") {
        CFG_SHADER[4]
    } else if n.contains("xbr") {
        CFG_SHADER[5]
    } else {
        CFG_SHADER[0]
    }
}

fn fakever_choice(v: (u32, u32)) -> &'static str {
    match v {
        (6, 1) => CFG_FAKEVER[3],
        (6, 0) => CFG_FAKEVER[2],
        (5, 1) => CFG_FAKEVER[1],
        _ => CFG_FAKEVER[0],
    }
}

fn renderer_index(st: &DDrawState) -> i32 {
    match renderer_choice(st) {
        x if x == CFG_RENDERER[0] => 0,
        x if x == CFG_RENDERER[1] => 1,
        x if x == CFG_RENDERER[2] => 2,
        x if x == CFG_RENDERER[3] => 3,
        _ => 4,
    }
}

fn shader_index(st: &DDrawState) -> i32 {
    match shader_choice(st) {
        x if x == CFG_SHADER[0] => 0,
        x if x == CFG_SHADER[1] => 1,
        x if x == CFG_SHADER[2] => 2,
        x if x == CFG_SHADER[3] => 3,
        x if x == CFG_SHADER[4] => 4,
        _ => 5,
    }
}

fn fakever_index(st: &DDrawState) -> i32 {
    match fakever_choice(st.fake_version) {
        x if x == CFG_FAKEVER[3] => 3,
        x if x == CFG_FAKEVER[2] => 2,
        x if x == CFG_FAKEVER[1] => 1,
        _ => 0,
    }
}

fn clamp_scroll(scroll: &mut usize, selected: usize) {
    if selected < *scroll {
        *scroll = selected;
    }
}

/// Run a one-shot action item (never called while the OVERLAY lock is held).
fn run_action(page: usize, code: u8) {
    match (page, code) {
        (1, 3) => unsafe {
            crate::window::toggle_maximize(true);
        },
        (1, 4) => unsafe {
            crate::window::toggle_fullscreen();
        },
        (3, 1) => {
            if crate::mouse::is_locked() {
                crate::mouse::unlock_cursor();
            } else {
                crate::mouse::lock_cursor();
            }
        }
        (5, 4) => {
            crate::screenshot::screenshot();
        }
        _ => {}
    }
}

/// Apply a navigation delta (-1 left, +1 right) to the live state.
fn adjust(page: usize, code: u8, dir: i32) {
    let changed = {
        let mut st = state().lock().unwrap();
        let c = write(&mut st, page, code, dir);
        if c && page == 0 && code == 4 && st.maxfps > 0 {
            st.target_fps = st.maxfps as f64;
            st.target_frame_len = 1000.0 / st.target_fps;
        }
        c
    };
    if changed {
        apply_setting(page, code);
    }
}

fn write(st: &mut DDrawState, page: usize, code: u8, dir: i32) -> bool {
    match (page, code) {
        (0, 0) => write_renderer(st, dir),
        (0, 1) => choice_adj(&mut st.filter, 0, 4, dir),
        (0, 2) => write_shader(st, dir),
        (0, 3) => int_switch(&mut st.swap_interval),
        (0, 4) => int_adj(&mut st.maxfps, 0, 1000, 5, dir),
        (0, 5) => int_adj(&mut st.minfps, 0, 200, 5, dir),
        (0, 6) => int_adj(&mut st.maxgameticks, -2, 120, 1, dir),
        (0, 7) => choice_adj(&mut st.limiter_type, 0, 4, dir),
        (1, 0) => choice_adj(&mut st.center_window, 0, 2, dir),
        (1, 1) => bool_toggle(&mut st.border),
        (1, 2) => bool_toggle(&mut st.resizable),
        (1, 5) => int_adj(&mut st.res_width, 0, 10240, 16, dir),
        (1, 6) => int_adj(&mut st.res_height, 0, 7680, 16, dir),
        (1, 7) => int_adj(&mut st.pos_x, -32000, 32767, 8, dir),
        (1, 8) => int_adj(&mut st.pos_y, -32000, 32767, 8, dir),
        (2, 0) => bool_toggle(&mut st.maintain_aspect_ratio),
        (2, 1) => bool_toggle(&mut st.windowboxing),
        (2, 2) => bool_toggle(&mut st.stretch_to_fullscreen),
        (2, 3) => int_adj(&mut st.refresh_rate, 0, 240, 1, dir),
        (2, 4) => choice_adj(&mut st.resolutions, 0, 2, dir),
        (2, 5) => int_adj(&mut st.max_resolutions, 0, 200, 4, dir),
        (3, 0) => bool_toggle(&mut st.adjmouse),
        (3, 2) => bool_toggle(&mut st.lock_mouse_top_left),
        (3, 3) => bool_toggle(&mut st.center_cursor_fix),
        (4, 0) => bool_toggle(&mut st.nonexclusive),
        (4, 1) => bool_toggle(&mut st.tshack),
        (4, 2) => bool_toggle(&mut st.flipclear),
        (4, 3) => bool_toggle(&mut st.lock_surfaces),
        (4, 4) => bool_toggle(&mut st.noactivateapp),
        (4, 5) => bool_toggle(&mut st.fix_not_responding),
        (4, 6) => bool_toggle(&mut st.fix_alt_key_stuck),
        (4, 7) => int_adj(&mut st.fixchilds, 0, 4, 1, dir),
        (4, 8) => bool_toggle(&mut st.remove_menu),
        (4, 9) => bool_toggle(&mut st.terminate_process),
        (4, 10) => write_fakever(st, dir),
        (5, 0) => int_adj(&mut st.savesettings, 0, 2, 1, dir),
        (5, 1) => bool_toggle(&mut st.draw_fps),
        (5, 2) => bool_toggle(&mut st.hook_peekmessage),
        (5, 3) => bool_toggle(&mut st.no_dinput_hook),
        _ => false,
    }
}

fn bool_toggle(v: &mut bool) -> bool {
    let n = !*v;
    if n == *v {
        false
    } else {
        *v = n;
        true
    }
}

fn int_switch(v: &mut i32) -> bool {
    let n = if *v > 0 { 0 } else { 1 };
    if n == *v {
        false
    } else {
        *v = n;
        true
    }
}

fn int_adj(v: &mut i32, min: i32, max: i32, step: i32, dir: i32) -> bool {
    let n = (*v + dir * step).clamp(min, max);
    if n == *v {
        false
    } else {
        *v = n;
        true
    }
}

fn choice_adj(v: &mut i32, min: i32, max: i32, dir: i32) -> bool {
    let cur = (*v).clamp(min, max);
    let mut n = cur + dir;
    if n < min {
        n = max;
    } else if n > max {
        n = min;
    }
    if n == cur {
        false
    } else {
        *v = n;
        true
    }
}

fn write_renderer(st: &mut DDrawState, dir: i32) -> bool {
    let cur = renderer_index(st);
    let mut n = cur + dir;
    if n < 0 {
        n = 4;
    } else if n > 4 {
        n = 0;
    }
    if n == cur {
        return false;
    }
    match n {
        0 => {
            st.auto_renderer = true;
            st.renderer = RENDERER_D3D9;
        }
        1 => {
            st.renderer = RENDERER_D3D9;
            st.auto_renderer = false;
        }
        2 => {
            st.renderer = RENDERER_OPENGL;
            st.auto_renderer = false;
        }
        3 => {
            st.renderer = RENDERER_OPENGL_CORE;
            st.auto_renderer = false;
        }
        _ => {
            st.renderer = RENDERER_GDI;
            st.auto_renderer = false;
        }
    }
    true
}

fn write_shader(st: &mut DDrawState, dir: i32) -> bool {
    let cur = shader_index(st);
    let mut n = cur + dir;
    if n < 0 {
        n = 5;
    } else if n > 5 {
        n = 0;
    }
    if n == cur {
        return false;
    }
    let name = match n {
        0 => "",
        1 => "nearest",
        2 => "bilinear",
        3 => "catmull-rom",
        4 => "lanczos",
        _ => "xbr",
    };
    st.shader = name.to_string();
    true
}

fn write_fakever(st: &mut DDrawState, dir: i32) -> bool {
    let cur = fakever_index(st);
    let mut n = cur + dir;
    if n < 0 {
        n = 3;
    } else if n > 3 {
        n = 0;
    }
    if n == cur {
        return false;
    }
    st.fake_version = match n {
        1 => (5, 1),
        2 => (6, 0),
        3 => (6, 1),
        _ => (0, 0),
    };
    true
}

/// Post-change side effects (limiter re-init, window resize, mouse scale, etc.).
fn apply_setting(page: usize, code: u8) {
    match (page, code) {
        (0, 0) => {
            let (r, a) = {
                let st = state().lock().unwrap();
                (st.renderer, st.auto_renderer)
            };
            if !a {
                crate::state::set_renderer(r);
            }
            state().lock().unwrap().render.invalidate = true;
        }
        (0, 1) | (0, 2) | (0, 3) => {
            state().lock().unwrap().render.invalidate = true;
        }
        (0, 6) | (0, 7) => {
            let st = state().lock().unwrap();
            let (t, l, m, r) = (st.maxgameticks, st.limiter_type, st.minfps, st.refresh_rate);
            drop(st);
            crate::fps_limiter::init(t, l, m, r);
        }
        (1, 0) | (1, 1) | (1, 2) | (1, 7) | (1, 8) => unsafe {
            crate::window::apply_window_style();
        },
        (1, 5) | (1, 6) => {
            let (rw, rh) = {
                let st = state().lock().unwrap();
                (st.res_width, st.res_height)
            };
            unsafe {
                if rw > 0 && rh > 0 {
                    crate::window::set_window_size(rw, rh);
                } else {
                    crate::window::apply_window_style();
                }
            }
        }
        (3, 0) | (3, 2) | (3, 3) => {
            crate::mouse::update_scale();
        }
        _ => {}
    }
    persist_setting(page, code);
}

/// Write the changed value back to the ini (respecting savesettings).
fn persist_setting(page: usize, code: u8) {
    if state().lock().unwrap().savesettings <= 0 {
        return;
    }
    let key = match (page, code) {
        (0, 0) => Some("Renderer"),
        (0, 1) => Some("Filter"),
        (0, 2) => Some("shader"),
        (0, 3) => Some("VSync"),
        (0, 4) => Some("maxfps"),
        (0, 5) => Some("minfps"),
        (0, 6) => Some("maxgameticks"),
        (0, 7) => Some("limiter_type"),
        (1, 0) => Some("center_window"),
        (1, 1) => Some("Border"),
        (1, 2) => Some("Resizable"),
        (1, 5) => Some("width"),
        (1, 6) => Some("height"),
        (1, 7) => Some("posx"),
        (1, 8) => Some("posy"),
        (2, 0) => Some("MaintainAspectRatio"),
        (2, 1) => Some("Windowboxing"),
        (2, 2) => Some("StretchToFullscreen"),
        (2, 3) => Some("refresh_rate"),
        (2, 4) => Some("resolutions"),
        (2, 5) => Some("max_resolutions"),
        (3, 0) => Some("adjmouse"),
        (3, 2) => Some("lock_mouse_top_left"),
        (3, 3) => Some("center_cursor_fix"),
        (4, 0) => Some("nonexclusive"),
        (4, 1) => Some("tshack"),
        (4, 2) => Some("flipclear"),
        (4, 3) => Some("lock_surfaces"),
        (4, 4) => Some("noactivateapp"),
        (4, 5) => Some("fix_not_responding"),
        (4, 6) => Some("fix_alt_key_stuck"),
        (4, 7) => Some("fixchilds"),
        (4, 8) => Some("remove_menu"),
        (4, 9) => Some("terminate_process"),
        (4, 10) => Some("WinVersion"),
        (5, 0) => Some("savesettings"),
        (5, 1) => Some("draw_fps"),
        (5, 2) => Some("hook_peekmessage"),
        (5, 3) => Some("no_dinput_hook"),
        _ => None,
    };
    if let Some(k) = key {
        let v = persist_value(page, code);
        crate::config::save_setting(k, &v);
    }
}

/// The ini-format value for a setting (not the display string).
fn persist_value(page: usize, code: u8) -> String {
    let st = state().lock().unwrap();
    if let Some(b) = bool_ini_value(&st, page, code) {
        return b;
    }
    match (page, code) {
        (0, 0) => {
            if st.auto_renderer {
                "auto".to_string()
            } else {
                match st.renderer {
                    RENDERER_D3D9 => "d3d9".to_string(),
                    RENDERER_OPENGL => "opengl".to_string(),
                    RENDERER_OPENGL_CORE => "openglcore".to_string(),
                    RENDERER_GDI => "gdi".to_string(),
                    _ => "d3d9".to_string(),
                }
            }
        }
        (0, 1) => match st.filter.clamp(0, 4) {
            0 => "nearest".to_string(),
            1 => "bilinear".to_string(),
            2 => "catmull".to_string(),
            3 => "lanczos".to_string(),
            _ => "xbr".to_string(),
        },
        (0, 2) => st.shader.clone(),
        (0, 7) => match st.limiter_type.clamp(0, 4) {
            0 => "auto".to_string(),
            1 => "testcooperativelevel".to_string(),
            2 => "bltfast".to_string(),
            3 => "unlock".to_string(),
            _ => "peekmessage".to_string(),
        },
        (4, 10) => match st.fake_version {
            (5, 1) => "5.1".to_string(),
            (6, 0) => "6.0".to_string(),
            (6, 1) => "6.1".to_string(),
            _ => String::new(),
        },
        _ => read(&st, page, code),
    }
}

/// Return the ini "1"/"0" string for Bool items, None otherwise.
fn bool_ini_value(st: &DDrawState, page: usize, code: u8) -> Option<String> {
    let b = match (page, code) {
        (0, 3) => st.swap_interval > 0,
        (1, 1) => st.border,
        (1, 2) => st.resizable,
        (2, 0) => st.maintain_aspect_ratio,
        (2, 1) => st.windowboxing,
        (2, 2) => st.stretch_to_fullscreen,
        (3, 0) => st.adjmouse,
        (3, 2) => st.lock_mouse_top_left,
        (3, 3) => st.center_cursor_fix,
        (4, 0) => st.nonexclusive,
        (4, 1) => st.tshack,
        (4, 2) => st.flipclear,
        (4, 3) => st.lock_surfaces,
        (4, 4) => st.noactivateapp,
        (4, 5) => st.fix_not_responding,
        (4, 6) => st.fix_alt_key_stuck,
        (4, 8) => st.remove_menu,
        (4, 9) => st.terminate_process,
        (5, 1) => st.draw_fps,
        (5, 2) => st.hook_peekmessage,
        (5, 3) => st.no_dinput_hook,
        _ => return None,
    };
    Some(if b { "1".to_string() } else { "0".to_string() })
}

fn repaint() {
    let hwnd = { lock().hwnd };
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = UpdateWindow(hwnd);
        }
    }
}

// Unused helpers kept referenced to mirror the two-column navigation model.
#[allow(dead_code)]
fn _mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Pages => "pages",
        Mode::Items => "items",
    }
}