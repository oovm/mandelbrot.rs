//! IAT (Import Address Table) hooking framework (ports `hook.c` /
//! `winapi_hooks.c`).
//!
//! Intercepts a broad set of Win32 calls the game makes ???window positioning,
//! cursor, GDI blits, fonts, palette, display/version faking, media routing and
//! the DirectInput thunks ???so they behave correctly under our wrapper.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, DEVMODE_FIELD_FLAGS, DEVMODEA, DEVMODEW, DISPLAY_DEVICEA, DISPLAY_DEVICEW, DM_PELSHEIGHT, DM_PELSWIDTH,
    HDC, HFONT, HGDIOBJ, HPALETTE, LOGFONTA, LOGFONTW, PALETTEENTRY,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA, LoadLibraryW};
use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualProtect, VirtualQuery};
use windows::Win32::System::SystemInformation::{OSVERSIONINFOA, OSVERSIONINFOW};
use windows::Win32::System::SystemServices::{
    IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_IMPORT_DESCRIPTOR, IMAGE_NT_SIGNATURE,
};
use windows::Win32::UI::WindowsAndMessaging::{CURSORINFO, GWL_STYLE, GetDesktopWindow, HCURSOR, MSG, SWP_NOSIZE};
use windows::core::{BOOL, PCSTR, PCWSTR};

use crate::state::state;
use crate::window;

type SetWindowPosFn = unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32;
type GetCursorPosFn = unsafe extern "system" fn(*mut POINT) -> i32;
type MoveWindowFn = unsafe extern "system" fn(HWND, i32, i32, i32, i32, BOOL) -> i32;
type GetWindowRectFn = unsafe extern "system" fn(HWND, *mut RECT) -> i32;
type GetClientRectFn = unsafe extern "system" fn(HWND, *mut RECT) -> i32;
type GetSystemMetricsFn = unsafe extern "system" fn(i32) -> i32;
type EnumDisplaySettingsAFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut DEVMODEA) -> i32;
type ShowWindowFn = unsafe extern "system" fn(HWND, i32) -> i32;
type SetParentFn = unsafe extern "system" fn(HWND, HWND) -> i32;
type MapWindowPointsFn = unsafe extern "system" fn(HWND, HWND, *mut POINT, u32) -> i32;
type GetVersionExAFn = unsafe extern "system" fn(*mut OSVERSIONINFOA) -> i32;
type GetVersionExWFn = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32;
type GetForegroundWindowFn = unsafe extern "system" fn() -> HWND;
type ClipCursorFn = unsafe extern "system" fn(*const RECT) -> i32;
type SetCursorPosFn = unsafe extern "system" fn(i32, i32) -> i32;
type ShowCursorFn = unsafe extern "system" fn(i32) -> i32;
type SetCursorFn = unsafe extern "system" fn(HCURSOR) -> HCURSOR;
type GetCursorInfoFn = unsafe extern "system" fn(*mut CURSORINFO) -> i32;
type WindowFromPointFn = unsafe extern "system" fn(POINT) -> HWND;
type ClientToScreenFn = unsafe extern "system" fn(HWND, *mut POINT) -> i32;
type ScreenToClientFn = unsafe extern "system" fn(HWND, *mut POINT) -> i32;
type SetWindowLongFn = unsafe extern "system" fn(HWND, i32, i32) -> i32;
type GetWindowLongFn = unsafe extern "system" fn(HWND, i32) -> i32;
type CreateWindowExAFn = unsafe extern "system" fn(
    u32,
    *const u8,
    *const u8,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    *mut core::ffi::c_void,
    HINSTANCE,
    *mut core::ffi::c_void,
) -> HWND;
type CreateWindowExWFn = unsafe extern "system" fn(
    u32,
    *const u16,
    *const u16,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    *mut core::ffi::c_void,
    HINSTANCE,
    *mut core::ffi::c_void,
) -> HWND;
type DestroyWindowFn = unsafe extern "system" fn(HWND) -> i32;
type PeekMessageFn = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;
type GetMessageFn = unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> i32;
type IsWindowFn = unsafe extern "system" fn(HWND) -> i32;
type SetFocusFn = unsafe extern "system" fn(HWND) -> HWND;
type GetFocusFn = unsafe extern "system" fn() -> HWND;
type BitBltFn = unsafe extern "system" fn(HDC, i32, i32, i32, i32, HDC, i32, i32, u32) -> i32;
type StretchBltFn = unsafe extern "system" fn(HDC, i32, i32, i32, i32, HDC, i32, i32, i32, i32, u32) -> i32;
type StretchDIBitsFn = unsafe extern "system" fn(
    HDC,
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    *const core::ffi::c_void,
    *const BITMAPINFO,
    u32,
    u32,
) -> i32;
type SetDIBitsToDeviceFn = unsafe extern "system" fn(
    HDC,
    i32,
    i32,
    u32,
    u32,
    i32,
    i32,
    u32,
    u32,
    *const core::ffi::c_void,
    *const BITMAPINFO,
    u32,
) -> i32;
type GetDeviceCapsFn = unsafe extern "system" fn(HDC, i32) -> i32;
type GetDCFn = unsafe extern "system" fn(HWND) -> HDC;
type CreateCompatibleDCFn = unsafe extern "system" fn(HDC) -> HDC;
type SelectObjectFn = unsafe extern "system" fn(HDC, HGDIOBJ) -> HGDIOBJ;
type DeleteDCFn = unsafe extern "system" fn(HDC) -> i32;
type CreateFontFn =
    unsafe extern "system" fn(i32, i32, i32, i32, i32, u32, u32, u32, u8, u8, u8, u8, u8, *const u8) -> HFONT;
type CreateFontIndirectFn = unsafe extern "system" fn(*const LOGFONTA) -> HFONT;
type CreateFontWFn =
    unsafe extern "system" fn(i32, i32, i32, i32, i32, u32, u32, u32, u8, u8, u8, u8, u8, *const u16) -> HFONT;
type CreateFontIndirectWFn = unsafe extern "system" fn(*const LOGFONTW) -> HFONT;
type GetSystemPaletteEntriesFn = unsafe extern "system" fn(HDC, u32, u32, *mut PALETTEENTRY) -> u32;
type SelectPaletteFn = unsafe extern "system" fn(HDC, HPALETTE, i32) -> HPALETTE;
type RealizePaletteFn = unsafe extern "system" fn(HDC) -> u32;
type LoadLibraryAFn = unsafe extern "system" fn(*const u8) -> HMODULE;
type LoadLibraryWFn = unsafe extern "system" fn(*const u16) -> HMODULE;
type GetProcAddressFn = unsafe extern "system" fn(HMODULE, *const u8) -> FARPROC;
type CoCreateInstanceFn = unsafe extern "system" fn(
    *const windows::core::GUID,
    *mut core::ffi::c_void,
    u32,
    *const windows::core::GUID,
    *mut *mut core::ffi::c_void,
) -> i32;
type MciSendCommandFn = unsafe extern "system" fn(u32, u32, usize, usize) -> u32;
type MciSendStringFn = unsafe extern "system" fn(*const u16, *mut u16, u32, HWND) -> u32;
type AviGetFrameOpenFn = unsafe extern "system" fn(*mut core::ffi::c_void, *const BITMAPINFO) -> *mut core::ffi::c_void;
type GetVersionFn = unsafe extern "system" fn() -> u32;
type EnumDisplaySettingsWFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut DEVMODEW) -> i32;
type EnumDisplayDevicesAFn = unsafe extern "system" fn(*const u8, u32, *mut DISPLAY_DEVICEA, u32) -> i32;
type EnumDisplayDevicesWFn = unsafe extern "system" fn(*const u16, u32, *mut DISPLAY_DEVICEW, u32) -> i32;
type GetDiskFreeSpaceFn = unsafe extern "system" fn(*const u8, *mut u32, *mut u32, *mut u32, *mut u32) -> i32;
type GetDiskFreeSpaceExFn = unsafe extern "system" fn(*const u16, *mut u64, *mut u64, *mut u64) -> i32;
type SetUnhandledExceptionFilterFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> *mut core::ffi::c_void;
type MessageBoxFn = unsafe extern "system" fn(HWND, *const u16, *const u16, u32) -> i32;
type GetKeyboardStateFn = unsafe extern "system" fn(*mut u8) -> i32;
type SetKeyboardStateFn = unsafe extern "system" fn(*mut u8) -> i32;

/// Cast a `FARPROC` returned by `GetProcAddress` into a typed function pointer.
unsafe fn to_fn<T>(p: FARPROC) -> Option<T> {
    p.map(|f| std::mem::transmute_copy(&f))
}

// Real (un-hooked) function pointers, resolved once at init via GetProcAddress
// so the fakes can call through to the original implementation without
// recursing into our own patched IAT entry.
static mut REAL_SETPOS: Option<SetWindowPosFn> = None;
static mut REAL_GETCURSORPOS: Option<GetCursorPosFn> = None;
static mut REAL_MOVEWINDOW: Option<MoveWindowFn> = None;
static mut REAL_GETWINDOWRECT: Option<GetWindowRectFn> = None;
static mut REAL_GETCLIENTRECT: Option<GetClientRectFn> = None;
static mut REAL_GETSYSTEMMETRICS: Option<GetSystemMetricsFn> = None;
static mut REAL_ENUMDISPLAYSETTINGS: Option<EnumDisplaySettingsAFn> = None;
static mut REAL_SHOWWINDOW: Option<ShowWindowFn> = None;
static mut REAL_SETPARENT: Option<SetParentFn> = None;
static mut REAL_MAPWINDOWPOINTS: Option<MapWindowPointsFn> = None;
static mut REAL_GETVERSIONEXA: Option<GetVersionExAFn> = None;
static mut REAL_GETVERSIONEXW: Option<GetVersionExWFn> = None;
static mut REAL_GETFOREGROUNDWINDOW: Option<GetForegroundWindowFn> = None;
static mut REAL_CLIPCURSOR: Option<ClipCursorFn> = None;
static mut REAL_SETCURSORPOS: Option<SetCursorPosFn> = None;
static mut REAL_SHOWCURSOR: Option<ShowCursorFn> = None;
static mut REAL_SETCURSOR: Option<SetCursorFn> = None;
static mut REAL_GETCURSORINFO: Option<GetCursorInfoFn> = None;
static mut REAL_WINDOWFROMPOINT: Option<WindowFromPointFn> = None;
static mut REAL_CLIENTTOSCREEN: Option<ClientToScreenFn> = None;
static mut REAL_SCREENTOCLIENT: Option<ScreenToClientFn> = None;
static mut REAL_SETWINDOWLONGA: Option<SetWindowLongFn> = None;
static mut REAL_SETWINDOWLONGW: Option<SetWindowLongFn> = None;
static mut REAL_GETWINDOWLONGA: Option<GetWindowLongFn> = None;
static mut REAL_GETWINDOWLONGW: Option<GetWindowLongFn> = None;
static mut REAL_CREATEWINDOWEXA: Option<CreateWindowExAFn> = None;
static mut REAL_CREATEWINDOWEXW: Option<CreateWindowExWFn> = None;
static mut REAL_DESTROYWINDOW: Option<DestroyWindowFn> = None;
static mut REAL_PEEKMESSAGEA: Option<PeekMessageFn> = None;
static mut REAL_PEEKMESSAGEW: Option<PeekMessageFn> = None;
static mut REAL_GETMESSAGEA: Option<GetMessageFn> = None;
static mut REAL_GETMESSAGEW: Option<GetMessageFn> = None;
static mut REAL_ISWINDOW: Option<IsWindowFn> = None;
static mut REAL_SETFOCUS: Option<SetFocusFn> = None;
static mut REAL_GETFOCUS: Option<GetFocusFn> = None;
static mut REAL_BITBLT: Option<BitBltFn> = None;
static mut REAL_STRETCHBLT: Option<StretchBltFn> = None;
static mut REAL_STRETCHDIBITS: Option<StretchDIBitsFn> = None;
static mut REAL_SETDIBITSTODEVICE: Option<SetDIBitsToDeviceFn> = None;
static mut REAL_GETDEVICECAPS: Option<GetDeviceCapsFn> = None;
static mut REAL_GETDC: Option<GetDCFn> = None;
static mut REAL_CREATECOMPATIBLEDC: Option<CreateCompatibleDCFn> = None;
static mut REAL_SELECTOBJECT: Option<SelectObjectFn> = None;
static mut REAL_DELETEDC: Option<DeleteDCFn> = None;
static mut REAL_CREATEFONTA: Option<CreateFontFn> = None;
static mut REAL_CREATEFONTW: Option<CreateFontWFn> = None;
static mut REAL_CREATEFONTINDIRECTA: Option<CreateFontIndirectFn> = None;
static mut REAL_CREATEFONTINDIRECTW: Option<CreateFontIndirectWFn> = None;
static mut REAL_GETSYSTEMPALETTEENTRIES: Option<GetSystemPaletteEntriesFn> = None;
static mut REAL_SELECTPALETTE: Option<SelectPaletteFn> = None;
static mut REAL_REALIZEPALETTE: Option<RealizePaletteFn> = None;
static mut REAL_LOADLIBRARYA: Option<LoadLibraryAFn> = None;
static mut REAL_LOADLIBRARYW: Option<LoadLibraryWFn> = None;
static mut REAL_GETPROCADDRESS: Option<GetProcAddressFn> = None;
static mut REAL_COCREATEINSTANCE: Option<CoCreateInstanceFn> = None;
static mut REAL_MCISENDCOMMAND: Option<MciSendCommandFn> = None;
static mut REAL_MCISENDSTRING: Option<MciSendStringFn> = None;
static mut REAL_AVIGETFRAMEOPEN: Option<AviGetFrameOpenFn> = None;
static mut REAL_GETVERSION: Option<GetVersionFn> = None;
static mut REAL_ENUMDISPLAYSETTINGSW: Option<EnumDisplaySettingsWFn> = None;
static mut REAL_ENUMDISPLAYDEVICESA: Option<EnumDisplayDevicesAFn> = None;
static mut REAL_ENUMDISPLAYDEVICESW: Option<EnumDisplayDevicesWFn> = None;
static mut REAL_GETDISKFREESPACEA: Option<GetDiskFreeSpaceFn> = None;
static mut REAL_GETDISKFREESPACEEX: Option<GetDiskFreeSpaceExFn> = None;
static mut REAL_SETUNHANDLEDEXCEPTIONFILTER: Option<SetUnhandledExceptionFilterFn> = None;
static mut REAL_MESSAGEBOXA: Option<MessageBoxFn> = None;
static mut REAL_MESSAGEBOXW: Option<MessageBoxFn> = None;
static mut REAL_GETKEYBOARDSTATE: Option<GetKeyboardStateFn> = None;
static mut REAL_SETKEYBOARDSTATE: Option<SetKeyboardStateFn> = None;

fn ieq(name: *const u8, target: &[u8]) -> bool {
    if name.is_null() {
        return false;
    }
    let mut i = 0;
    unsafe {
        loop {
            let c = *name.add(i);
            let t = *target.get(i).unwrap_or(&0);
            if t == 0 {
                return c == 0;
            }
            if c == 0 {
                return false;
            }
            if !c.eq_ignore_ascii_case(&t) {
                return false;
            }
            i += 1;
        }
    }
}

/// Patch `functionName` imported from `module_name` in module `hmod`.
unsafe fn hook_iat(hmod: HMODULE, module_name: &[u8], function_name: &[u8], new_function: usize) {
    if hmod.0.is_null() || new_function == 0 {
        return;
    }
    let base = hmod.0 as *mut u8;
    let dos = base as *const IMAGE_DOS_HEADER;
    if (*dos).e_magic != IMAGE_DOS_SIGNATURE {
        return;
    }
    let nt = base.add((*dos).e_lfanew as usize) as *const u32;
    if *nt != IMAGE_NT_SIGNATURE {
        return;
    }

    // Optional header follows the 4-byte NT signature and 20-byte file header.
    let opt = base.add((*dos).e_lfanew as usize + 4 + 20);
    let magic = *(opt as *const u16);
    let datadir_off = if magic == 0x20B { 0x70usize } else { 0x60usize };
    let datadir_base = opt.add(datadir_off);
    let import_entry = datadir_base.add(8) as *const u32;
    let import_rva = *import_entry as usize;
    if import_rva == 0 {
        return;
    }

    let mut desc = base.add(import_rva) as *const IMAGE_IMPORT_DESCRIPTOR;
    loop {
        let first_thunk_rva = (*desc).FirstThunk;
        if first_thunk_rva == 0 {
            break;
        }
        let name_rva = (*desc).Name;
        let imp_name = base.add(name_rva as usize) as *const u8;
        if ieq(imp_name, module_name) {
            let orig_rva = (*desc).Anonymous.OriginalFirstThunk;
            let orig_base =
                if orig_rva != 0 { base.add(orig_rva as usize) } else { base.add(first_thunk_rva as usize) };
            let mut first = base.add(first_thunk_rva as usize) as *mut usize;
            let mut orig = orig_base as *const usize;
            let top_bit = 1usize << (usize::BITS - 1);
            loop {
                let fn_addr = *first;
                let ord_addr = *orig;
                if fn_addr == 0 {
                    break;
                }
                if ord_addr & top_bit == 0 {
                    let name_ptr = base.add(ord_addr & !top_bit) as *const u8;
                    // IMAGE_IMPORT_BY_NAME: 2-byte hint followed by name.
                    let import_fn_name = name_ptr.add(2);
                    if ieq(import_fn_name, function_name) {
                        let mut mbi = MEMORY_BASIC_INFORMATION::default();
                        if VirtualQuery(
                            Some(first as *const core::ffi::c_void),
                            &mut mbi,
                            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                        ) != 0
                        {
                            let mut old = PAGE_READWRITE;
                            if VirtualProtect(
                                first as *const core::ffi::c_void,
                                std::mem::size_of::<usize>(),
                                PAGE_READWRITE,
                                &mut old,
                            )
                            .is_ok()
                            {
                                *first = new_function;
                                let _ = VirtualProtect(
                                    first as *const core::ffi::c_void,
                                    std::mem::size_of::<usize>(),
                                    old,
                                    &mut old,
                                );
                            }
                        }
                        break;
                    }
                }
                first = first.add(1);
                orig = orig.add(1);
            }
        }
        desc = desc.add(1);
    }
}

// ---------------------------------------------------------------------------
// GROUP A: mouse / cursor
// ---------------------------------------------------------------------------

/// Replacement for `user32!GetCursorPos` ???routes through the adjmouse mapper
/// so the reported position is remapped into game coordinates while the cursor
/// is locked.
unsafe extern "system" fn fake_get_cursor_pos(point: *mut POINT) -> i32 {
    if crate::mouse::hook_get_cursor_pos(point) {
        return 1;
    }
    if let Some(f) = REAL_GETCURSORPOS {
        f(point);
    }
    1
}

/// Replacement for `user32!ClipCursor` ???when the cursor is locked, forward to
/// the real call even if the game passes owning a sub-region; otherwise claim
/// success without clipping when adjmouse is active.
unsafe extern "system" fn fake_clip_cursor(lp_rect: *const RECT) -> i32 {
    let (adj, locked, hwnd) = {
        let st = state().lock().unwrap();
        (st.adjmouse, st.mouse_is_locked != 0, st.hwnd)
    };
    if adj && locked && !hwnd.is_invalid() {
        if let Some(f) = REAL_CLIPCURSOR {
            return f(lp_rect);
        }
        return 1;
    }
    if let Some(f) = REAL_CLIPCURSOR {
        return f(lp_rect);
    }
    1
}

/// Replacement for `user32!SetCursorPos` ???when the cursor is locked, forward
/// to the real call, otherwise suppress (the game is windowed/devmode).
unsafe extern "system" fn fake_set_cursor_pos(x: i32, y: i32) -> i32 {
    let (adj, locked) = {
        let st = state().lock().unwrap();
        (st.adjmouse, st.mouse_is_locked != 0)
    };
    if !adj || !locked {
        return 1;
    }
    if let Some(f) = REAL_SETCURSORPOS {
        return f(x, y);
    }
    1
}

/// Replacement for `user32!ShowCursor` ???forward to the real call.
unsafe extern "system" fn fake_show_cursor(b_show: i32) -> i32 {
    if let Some(f) = REAL_SHOWCURSOR {
        return f(b_show);
    }
    0
}

/// Replacement for `user32!SetCursor` ???forward to the real call.
unsafe extern "system" fn fake_set_cursor(h_cursor: HCURSOR) -> HCURSOR {
    if let Some(f) = REAL_SETCURSOR {
        return f(h_cursor);
    }
    HCURSOR(std::ptr::null_mut())
}

/// Replacement for `user32!GetCursorInfo` ???forward to the real call and, when
/// the cursor is over our window, convert the reported position to client
/// coordinates.
unsafe extern "system" fn fake_get_cursor_info(pci: *mut CURSORINFO) -> i32 {
    if pci.is_null() {
        return 0;
    }
    if let Some(f) = REAL_GETCURSORINFO {
        f(pci);
    }
    let hwnd = state().lock().unwrap().hwnd;
    if !hwnd.is_invalid()
        && let Some(sc) = REAL_SCREENTOCLIENT
    {
        let mut pt = (*pci).ptScreenPos;
        if sc(hwnd, &mut pt) != 0 {
            (*pci).ptScreenPos = pt;
        }
    }
    1
}

/// Replacement for `user32!WindowFromPoint` ???forward to the real call.
unsafe extern "system" fn fake_window_from_point(point: POINT) -> HWND {
    if let Some(f) = REAL_WINDOWFROMPOINT {
        return f(point);
    }
    HWND(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// GROUP B: window / input
// ---------------------------------------------------------------------------

/// Replacement for `user32!ClientToScreen` ???forward to the real call.
unsafe extern "system" fn fake_client_to_screen(hwnd: HWND, lp_point: *mut POINT) -> i32 {
    if let Some(f) = REAL_CLIENTTOSCREEN {
        return f(hwnd, lp_point);
    }
    0
}

/// Replacement for `user32!ScreenToClient` ???forward to the real call.
unsafe extern "system" fn fake_screen_to_client(hwnd: HWND, lp_point: *mut POINT) -> i32 {
    if let Some(f) = REAL_SCREENTOCLIENT {
        return f(hwnd, lp_point);
    }
    0
}

/// Replacement for `user32!GetWindowRect` ???when a fake size is configured for
/// our window, reports the fake client size while keeping the window's real
/// screen origin.
unsafe extern "system" fn fake_get_window_rect(hwnd: HWND, p_result: *mut RECT) -> i32 {
    let (our, fake) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.fake_size)
    };
    if hwnd == our && fake != (0, 0) {
        if let Some(f) = REAL_GETWINDOWRECT {
            f(hwnd, p_result);
        }
        if !p_result.is_null() {
            let actual = *p_result;
            *p_result =
                RECT { left: actual.left, top: actual.top, right: actual.left + fake.0, bottom: actual.top + fake.1 };
        }
        return 1;
    }
    if let Some(f) = REAL_GETWINDOWRECT {
        return f(hwnd, p_result);
    }
    1
}

/// Replacement for `user32!GetClientRect` ???reports the fake size when
/// configured, otherwise forwards to the real call.
unsafe extern "system" fn fake_get_client_rect(hwnd: HWND, p_result: *mut RECT) -> i32 {
    let (our, fake) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.fake_size)
    };
    if hwnd == our && fake != (0, 0) {
        if !p_result.is_null() {
            *p_result = RECT { left: 0, top: 0, right: fake.0, bottom: fake.1 };
        }
        return 1;
    }
    if let Some(f) = REAL_GETCLIENTRECT {
        return f(hwnd, p_result);
    }
    1
}

/// Replacement for `user32!SetWindowLongA` ???forward except that GWL_STYLE
/// changes on our own window are ignored (the wrapper owns the style).
unsafe extern "system" fn fake_set_window_long_a(hwnd: HWND, n_index: i32, dw_new_long: i32) -> i32 {
    let our = state().lock().unwrap().hwnd;
    if hwnd == our && n_index == GWL_STYLE.0 {
        return 0;
    }
    if let Some(f) = REAL_SETWINDOWLONGA {
        return f(hwnd, n_index, dw_new_long);
    }
    0
}

/// Replacement for `user32!SetWindowLongW` ???wide variant of the above.
unsafe extern "system" fn fake_set_window_long_w(hwnd: HWND, n_index: i32, dw_new_long: i32) -> i32 {
    let our = state().lock().unwrap().hwnd;
    if hwnd == our && n_index == GWL_STYLE.0 {
        return 0;
    }
    if let Some(f) = REAL_SETWINDOWLONGW {
        return f(hwnd, n_index, dw_new_long);
    }
    0
}

/// Replacement for `user32!GetWindowLongA` ???forward; only GWL_STYLE /
/// GWL_EXSTYLE are interest to the game here.
unsafe extern "system" fn fake_get_window_long_a(hwnd: HWND, n_index: i32) -> i32 {
    if let Some(f) = REAL_GETWINDOWLONGA {
        return f(hwnd, n_index);
    }
    0
}

/// Replacement for `user32!GetWindowLongW` ???wide variant.
unsafe extern "system" fn fake_get_window_long_w(hwnd: HWND, n_index: i32) -> i32 {
    if let Some(f) = REAL_GETWINDOWLONGW {
        return f(hwnd, n_index);
    }
    0
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_EX_TRANSPARENT: i32 = 0x0000_0020;
const CW_USEDEFAULT: i32 = -2147483648;

unsafe fn cstr_class_a(cls: *const u8) -> Option<String> {
    if cls.is_null() {
        return None;
    }
    let c = std::ffi::CStr::from_ptr(cls.cast());
    Some(c.to_string_lossy().into_owned())
}

unsafe fn cwstr_class_w(cls: *const u16) -> Option<String> {
    if cls.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *cls.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(cls, len)))
}

/// Common post-creation fixchilds layout: when the new window is a child of
/// our game window, position/resize it to our game client rect so cutscenes
/// line up (mirrors cnc-ddraw's fixchilds handling).
unsafe fn layout_child(hwnd: HWND, parent: HWND, dw_style: u32) {
    let (our, fixchilds) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.fixchilds)
    };
    if fixchilds <= 0 || hwnd.is_invalid() || parent != our || our.is_invalid() {
        return;
    }
    if (dw_style & WS_CHILD) != 0 {
        let (gw, gh) = {
            let st = state().lock().unwrap();
            (st.width, st.height)
        };
        if gw > 0 && gh > 0 {
            use windows::Win32::UI::WindowsAndMessaging::{
                GWL_EXSTYLE, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
            };
            let ex = REAL_GETWINDOWLONGW.map(|f| f(hwnd, GWL_EXSTYLE.0)).unwrap_or(0);
            if ex & WS_EX_TRANSPARENT == 0 && (fixchilds == 3 || fixchilds == 4) {
                let _ = REAL_SETWINDOWLONGW.map(|f| f(hwnd, GWL_EXSTYLE.0, ex | WS_EX_TRANSPARENT));
            }
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, gw, gh, SWP_NOACTIVATE | SWP_NOZORDER);
        }
    }
}

/// Replacement for `user32!CreateWindowExA` ???forward and apply fixchilds on the
/// created child.
unsafe extern "system" fn fake_create_window_ex_a(
    dw_ex_style: u32,
    lp_class_name: *const u8,
    lp_window_name: *const u8,
    dw_style: u32,
    x: i32,
    y: i32,
    n_width: i32,
    n_height: i32,
    h_wnd_parent: HWND,
    h_menu: *mut core::ffi::c_void,
    h_instance: HINSTANCE,
    lp_param: *mut core::ffi::c_void,
) -> HWND {
    let mut dw_style = dw_style;
    let _ = &mut dw_style;
    let our = state().lock().unwrap().hwnd;
    let hwnd = if let Some(f) = REAL_CREATEWINDOWEXA {
        f(
            dw_ex_style,
            lp_class_name,
            lp_window_name,
            dw_style,
            x,
            y,
            n_width,
            n_height,
            h_wnd_parent,
            h_menu,
            h_instance,
            lp_param,
        )
    } else {
        HWND(std::ptr::null_mut())
    };
    if !hwnd.is_invalid() && !our.is_invalid() {
        layout_child(hwnd, our, dw_style);
    }
    hwnd
}

/// Replacement for `user32!CreateWindowExW` ???wide variant.
unsafe extern "system" fn fake_create_window_ex_w(
    dw_ex_style: u32,
    lp_class_name: *const u16,
    lp_window_name: *const u16,
    dw_style: u32,
    x: i32,
    y: i32,
    n_width: i32,
    n_height: i32,
    h_wnd_parent: HWND,
    h_menu: *mut core::ffi::c_void,
    h_instance: HINSTANCE,
    lp_param: *mut core::ffi::c_void,
) -> HWND {
    let our = state().lock().unwrap().hwnd;
    let hwnd = if let Some(f) = REAL_CREATEWINDOWEXW {
        f(
            dw_ex_style,
            lp_class_name,
            lp_window_name,
            dw_style,
            x,
            y,
            n_width,
            n_height,
            h_wnd_parent,
            h_menu,
            h_instance,
            lp_param,
        )
    } else {
        HWND(std::ptr::null_mut())
    };
    if !hwnd.is_invalid() && !our.is_invalid() {
        layout_child(hwnd, our, dw_style);
    }
    hwnd
}

/// Replacement for `user32!DestroyWindow` ???forward to the real call, clearing
/// our window handle when the game destroys its own window.
unsafe extern "system" fn fake_destroy_window(hwnd: HWND) -> i32 {
    let result = if let Some(f) = REAL_DESTROYWINDOW { f(hwnd) } else { 0 };
    {
        let mut st = state().lock().unwrap();
        if st.hwnd == hwnd {
            st.hwnd = HWND(std::ptr::null_mut());
            st.wnd_proc = 0;
        }
    }
    result
}

/// fps-limiter tick injection shared by PeekMessage / GetMessage hooks. Gated on
/// `state.hook_peekmessage`: when disabled the hooks are installed but never
/// tick.
fn maybe_wait_game_tick() {
    if !state().lock().unwrap().hook_peekmessage {
        return;
    }
    if crate::fps_limiter::limiter_applies(crate::fps_limiter::LIMIT_PEEKMESSAGE) {
        crate::fps_limiter::wait_game_tick();
    }
}

/// Replacement for `user32!PeekMessageA` ???forward and apply the fps limiter.
unsafe extern "system" fn fake_peek_message_a(
    lp_msg: *mut MSG,
    hwnd: HWND,
    w_msg_filter_min: u32,
    w_msg_filter_max: u32,
    w_remove_msg: u32,
) -> i32 {
    maybe_wait_game_tick();
    if let Some(f) = REAL_PEEKMESSAGEA {
        return f(lp_msg, hwnd, w_msg_filter_min, w_msg_filter_max, w_remove_msg);
    }
    0
}

/// Replacement for `user32!PeekMessageW` ???wide variant.
unsafe extern "system" fn fake_peek_message_w(
    lp_msg: *mut MSG,
    hwnd: HWND,
    w_msg_filter_min: u32,
    w_msg_filter_max: u32,
    w_remove_msg: u32,
) -> i32 {
    maybe_wait_game_tick();
    if let Some(f) = REAL_PEEKMESSAGEW {
        return f(lp_msg, hwnd, w_msg_filter_min, w_msg_filter_max, w_remove_msg);
    }
    0
}

/// Replacement for `user32!GetMessageA` ???forward and apply the fps limiter.
unsafe extern "system" fn fake_get_message_a(
    lp_msg: *mut MSG,
    hwnd: HWND,
    w_msg_filter_min: u32,
    w_msg_filter_max: u32,
) -> i32 {
    maybe_wait_game_tick();
    if let Some(f) = REAL_GETMESSAGEA {
        return f(lp_msg, hwnd, w_msg_filter_min, w_msg_filter_max);
    }
    0
}

/// Replacement for `user32!GetMessageW` ???wide variant.
unsafe extern "system" fn fake_get_message_w(
    lp_msg: *mut MSG,
    hwnd: HWND,
    w_msg_filter_min: u32,
    w_msg_filter_max: u32,
) -> i32 {
    maybe_wait_game_tick();
    if let Some(f) = REAL_GETMESSAGEW {
        return f(lp_msg, hwnd, w_msg_filter_min, w_msg_filter_max);
    }
    0
}

/// Replacement for `user32!IsWindow` ???forward to the real call.
unsafe extern "system" fn fake_is_window(hwnd: HWND) -> i32 {
    if let Some(f) = REAL_ISWINDOW {
        return f(hwnd);
    }
    0
}

/// Replacement for `user32!SetFocus` ???forward to the real call.
unsafe extern "system" fn fake_set_focus(hwnd: HWND) -> HWND {
    if let Some(f) = REAL_SETFOCUS {
        return f(hwnd);
    }
    HWND(std::ptr::null_mut())
}

/// Replacement for `user32!GetFocus` ???forward to the real call.
unsafe extern "system" fn fake_get_focus() -> HWND {
    if let Some(f) = REAL_GETFOCUS {
        return f();
    }
    HWND(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// GROUP C: GDI drawing redirect (software-renderer path)
// ---------------------------------------------------------------------------

/// Whether `hdc` belongs to the game's own window (the destination we care
/// about for the GDI software-renderer path).
unsafe fn dc_is_game(hdc: HDC) -> bool {
    if hdc.is_invalid() {
        return false;
    }
    let our = state().lock().unwrap().hwnd;
    if our.is_invalid() {
        return false;
    }
    let hwnd = windows::Win32::Graphics::Gdi::WindowFromDC(hdc);
    hwnd == our
}

/// Replacement for `gdi32!BitBlt` ???forward to the real call, marking the
/// screen updated when the destination is our window (software renderer).
unsafe extern "system" fn fake_bit_blt(
    hdc: HDC,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    hdc_src: HDC,
    x1: i32,
    y1: i32,
    rop: u32,
) -> i32 {
    if dc_is_game(hdc) {
        crate::state::mark_dirty();
    }
    if let Some(f) = REAL_BITBLT {
        return f(hdc, x, y, cx, cy, hdc_src, x1, y1, rop);
    }
    0
}

/// Replacement for `gdi32!StretchBlt` ???forward and mark the screen updated.
unsafe extern "system" fn fake_stretch_blt(
    hdc_dest: HDC,
    x_dest: i32,
    y_dest: i32,
    w_dest: i32,
    h_dest: i32,
    hdc_src: HDC,
    x_src: i32,
    y_src: i32,
    w_src: i32,
    h_src: i32,
    rop: u32,
) -> i32 {
    if dc_is_game(hdc_dest) {
        crate::state::mark_dirty();
    }
    if let Some(f) = REAL_STRETCHBLT {
        return f(hdc_dest, x_dest, y_dest, w_dest, h_dest, hdc_src, x_src, y_src, w_src, h_src, rop);
    }
    0
}

/// Replacement for `gdi32!StretchDIBits` ???forward and mark the screen updated.
unsafe extern "system" fn fake_stretch_dibits(
    hdc: HDC,
    x_dest: i32,
    y_dest: i32,
    dest_width: i32,
    dest_height: i32,
    x_src: i32,
    y_src: i32,
    src_width: i32,
    src_height: i32,
    lp_bits: *const core::ffi::c_void,
    lpbmi: *const BITMAPINFO,
    i_usage: u32,
    rop: u32,
) -> i32 {
    if dc_is_game(hdc) {
        crate::state::mark_dirty();
    }
    if let Some(f) = REAL_STRETCHDIBITS {
        return f(
            hdc,
            x_dest,
            y_dest,
            dest_width,
            dest_height,
            x_src,
            y_src,
            src_width,
            src_height,
            lp_bits,
            lpbmi,
            i_usage,
            rop,
        );
    }
    0
}

/// Replacement for `gdi32!SetDIBitsToDevice` ???forward and mark the screen
/// updated when drawing into our window.
unsafe extern "system" fn fake_set_dibits_to_device(
    hdc: HDC,
    x_dest: i32,
    y_dest: i32,
    w: u32,
    h: u32,
    x_src: i32,
    y_src: i32,
    start_scan: u32,
    c_lines: u32,
    lpv_bits: *const core::ffi::c_void,
    lpbmi: *const BITMAPINFO,
    color_use: u32,
) -> i32 {
    if dc_is_game(hdc) {
        crate::state::mark_dirty();
    }
    if let Some(f) = REAL_SETDIBITSTODEVICE {
        return f(hdc, x_dest, y_dest, w, h, x_src, y_src, start_scan, c_lines, lpv_bits, lpbmi, color_use);
    }
    0
}

/// Resolve the effective fake resolution: `res_width`/`res_height`, then the
/// `fake_size`, else `(0,0)` (meaning report the real one).
fn fake_resolution() -> (u32, u32) {
    let (rw, rh, fs) = {
        let st = state().lock().unwrap();
        (st.res_width, st.res_height, st.fake_size)
    };
    if rw > 0 && rh > 0 {
        return (rw as u32, rh as u32);
    }
    if fs != (0, 0) {
        return (fs.0.max(0) as u32, fs.1.max(0) as u32);
    }
    (0, 0)
}

/// Effective bpp to report: from state, else default to 16 for the software
/// path (8 when a palette is attached).
fn fake_bpp() -> i32 {
    let (bpp, has_narrow_palette) = {
        let st = state().lock().unwrap();
        (st.bpp, st.bpp == 8)
    };
    let _ = has_narrow_palette;
    bpp
}

/// Replacement for `gdi32!GetDeviceCaps` ???report the faked resolution / bpp
/// for the game window / desktop DC, otherwise forward.
unsafe extern "system" fn fake_get_device_caps(hdc: HDC, n_index: i32) -> i32 {
    let (w, h) = fake_resolution();
    let bpp = fake_bpp();
    let is_disp = dc_is_game(hdc) || windows::Win32::Graphics::Gdi::WindowFromDC(hdc) == unsafe { GetDesktopWindow() };
    if is_disp {
        match n_index {
            12 => return bpp,     // BITSPIXEL
            4 => return w as i32, // HORZRES
            6 => return h as i32, // VERTRES
            _ => {}
        }
        if bpp == 8 {
            match n_index {
                38 => return 256, // SIZEPALETTE
                40 => return 256, // NUMCOLORS
                _ => {}
            }
        }
    }
    if let Some(f) = REAL_GETDEVICECAPS {
        return f(hdc, n_index);
    }
    0
}

/// Replacement for `user32!GetDC` ???forward to the real call (the wrapper owns
/// window painting; the returned DC is used by the software-renderer path).
unsafe extern "system" fn fake_get_dc(hwnd: HWND) -> HDC {
    if let Some(f) = REAL_GETDC {
        return f(hwnd);
    }
    HDC(std::ptr::null_mut())
}

/// Replacement for `gdi32!CreateCompatibleDC` ???forward.
unsafe extern "system" fn fake_create_compatible_dc(hdc: HDC) -> HDC {
    if let Some(f) = REAL_CREATECOMPATIBLEDC {
        return f(hdc);
    }
    HDC(std::ptr::null_mut())
}

/// Replacement for `gdi32!SelectObject` ???forward.
unsafe extern "system" fn fake_select_object(hdc: HDC, h: HGDIOBJ) -> HGDIOBJ {
    if let Some(f) = REAL_SELECTOBJECT {
        return f(hdc, h);
    }
    HGDIOBJ(std::ptr::null_mut())
}

/// Replacement for `gdi32!DeleteDC` ???forward.
unsafe extern "system" fn fake_delete_dc(hdc: HDC) -> i32 {
    if let Some(f) = REAL_DELETEDC {
        return f(hdc);
    }
    0
}

// ---------------------------------------------------------------------------
// GROUP D: fonts
// ---------------------------------------------------------------------------

fn font_state() -> (i32, i32) {
    let st = state().lock().unwrap();
    (st.min_font_size, st.anti_aliased_fonts_min_size)
}

/// Apply min_font_size / anti_aliased_fonts_min_size to a raw height value,
/// returning the adjusted height and whether anti-aliasing should be disabled.
fn adjust_font(height: i32) -> (i32, bool) {
    let (min_size, anti_min) = font_state();
    let mut h = height;
    if min_size > 0 {
        if h < 0 {
            h = h.min(-min_size);
        } else {
            h = h.max(min_size);
        }
    }
    let no_aa = anti_min > 0 && h.abs() < anti_min;
    (h, no_aa)
}

/// Replacement for `gdi32!CreateFontA` ???enforce the font-size tweaks.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn fake_create_font_a(
    n_height: i32,
    n_width: i32,
    n_escapement: i32,
    n_orientation: i32,
    fn_weight: i32,
    fdw_italic: u32,
    fdw_underline: u32,
    fdw_strike_out: u32,
    fdw_char_set: u8,
    fdw_output_precision: u8,
    fdw_clip_precision: u8,
    fdw_quality: u8,
    fdw_pitch_and_family: u8,
    lpsz_face: *const u8,
) -> HFONT {
    let (h, no_aa) = adjust_font(n_height);
    let quality = if no_aa { windows::Win32::Graphics::Gdi::NONANTIALIASED_QUALITY.0 } else { fdw_quality };
    if let Some(f) = REAL_CREATEFONTA {
        return f(
            h,
            n_width,
            n_escapement,
            n_orientation,
            fn_weight,
            fdw_italic,
            fdw_underline,
            fdw_strike_out,
            fdw_char_set,
            fdw_output_precision,
            fdw_clip_precision,
            quality,
            fdw_pitch_and_family,
            lpsz_face,
        );
    }
    HFONT(std::ptr::null_mut())
}

/// Replacement for `gdi32!CreateFontW` ???wide variant.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn fake_create_font_w(
    n_height: i32,
    n_width: i32,
    n_escapement: i32,
    n_orientation: i32,
    fn_weight: i32,
    fdw_italic: u32,
    fdw_underline: u32,
    fdw_strike_out: u32,
    fdw_char_set: u8,
    fdw_output_precision: u8,
    fdw_clip_precision: u8,
    fdw_quality: u8,
    fdw_pitch_and_family: u8,
    lpsz_face: *const u16,
) -> HFONT {
    let (h, no_aa) = adjust_font(n_height);
    let quality = if no_aa { windows::Win32::Graphics::Gdi::NONANTIALIASED_QUALITY.0 } else { fdw_quality };
    if let Some(f) = REAL_CREATEFONTW {
        return f(
            h,
            n_width,
            n_escapement,
            n_orientation,
            fn_weight,
            fdw_italic,
            fdw_underline,
            fdw_strike_out,
            fdw_char_set,
            fdw_output_precision,
            fdw_clip_precision,
            quality,
            fdw_pitch_and_family,
            lpsz_face,
        );
    }
    HFONT(std::ptr::null_mut())
}

/// Replacement for `gdi32!CreateFontIndirectA` ???apply font-size tweaks to the
/// LOGFONT before forwarding.
unsafe extern "system" fn fake_create_font_indirect_a(lplf: *const LOGFONTA) -> HFONT {
    if !lplf.is_null() {
        let mut lf = *lplf;
        let (h, no_aa) = adjust_font(lf.lfHeight);
        lf.lfHeight = h;
        if no_aa {
            lf.lfQuality = windows::Win32::Graphics::Gdi::NONANTIALIASED_QUALITY;
        }
        if let Some(f) = REAL_CREATEFONTINDIRECTA {
            return f(&lf);
        }
    }
    HFONT(std::ptr::null_mut())
}

/// Replacement for `gdi32!CreateFontIndirectW` ???wide variant.
unsafe extern "system" fn fake_create_font_indirect_w(lplf: *const LOGFONTW) -> HFONT {
    if !lplf.is_null() {
        let mut lf = *lplf;
        let (h, no_aa) = adjust_font(lf.lfHeight);
        lf.lfHeight = h;
        if no_aa {
            lf.lfQuality = windows::Win32::Graphics::Gdi::NONANTIALIASED_QUALITY;
        }
        if let Some(f) = REAL_CREATEFONTINDIRECTW {
            return f(&lf);
        }
    }
    HFONT(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// GROUP E: palette
// ---------------------------------------------------------------------------

/// Replacement for `gdi32!GetSystemPaletteEntries` ???forward.
unsafe extern "system" fn fake_get_system_palette_entries(
    hdc: HDC,
    i_start: u32,
    c_entries: u32,
    p_pal_entries: *mut PALETTEENTRY,
) -> u32 {
    if let Some(f) = REAL_GETSYSTEMPALETTEENTRIES {
        return f(hdc, i_start, c_entries, p_pal_entries);
    }
    0
}

/// Replacement for `gdi32!SelectPalette` ???forward.
unsafe extern "system" fn fake_select_palette(hdc: HDC, h_pal: HPALETTE, b_force_bkgd: i32) -> HPALETTE {
    if let Some(f) = REAL_SELECTPALETTE {
        return f(hdc, h_pal, b_force_bkgd);
    }
    HPALETTE(std::ptr::null_mut())
}

/// Replacement for `gdi32!RealizePalette` ???forward.
unsafe extern "system" fn fake_realize_palette(hdc: HDC) -> u32 {
    if let Some(f) = REAL_REALIZEPALETTE {
        return f(hdc);
    }
    0
}

// ---------------------------------------------------------------------------
// GROUP F: media / COM / library loading
// ---------------------------------------------------------------------------

unsafe fn ansi(s: *const u8) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let c = std::ffi::CStr::from_ptr(s.cast());
    Some(c.to_string_lossy().into_owned())
}

unsafe fn wide(s: *const u16) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *s.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(s, len)))
}

/// Replacement for `kernel32!LoadLibraryA` ???forward, logging media codec
/// libraries that we know want letting through.
unsafe extern "system" fn fake_load_library_a(lp_file_name: *const u8) -> HMODULE {
    let name = ansi(lp_file_name);
    let result = if let Some(f) = REAL_LOADLIBRARYA {
        f(lp_file_name)
    } else {
        LoadLibraryA(PCSTR(lp_file_name.cast())).unwrap_or_default()
    };
    if let Some(n) = name
        && crate::media::wants_library(&n)
    {
        crate::dd_log!("hook: LoadLibraryA media codec: {}", n);
    }
    result
}

/// Replacement for `kernel32!LoadLibraryW` ???wide variant.
unsafe extern "system" fn fake_load_library_w(lp_file_name: *const u16) -> HMODULE {
    let name = wide(lp_file_name);
    let result = if let Some(f) = REAL_LOADLIBRARYW {
        f(lp_file_name)
    } else {
        LoadLibraryW(PCWSTR::from_raw(lp_file_name)).unwrap_or_default()
    };
    if let Some(n) = name
        && crate::media::wants_library(&n)
    {
        crate::dd_log!("hook: LoadLibraryW media codec: {}", n);
    }
    result
}

/// Replacement for `kernel32!GetProcAddress` ???forward to the real export.
/// (A full EAT-style redirect of our fakes is intentionally not implemented;
/// IAT scanning at load time already covers the common paths. Resolving via
/// `GetProcAddress` late is forwarded unchanged.)
unsafe extern "system" fn fake_get_proc_address(h_module: HMODULE, lp_proc_name: *const u8) -> FARPROC {
    if let Some(f) = REAL_GETPROCADDRESS {
        return f(h_module, lp_proc_name);
    }
    GetProcAddress(h_module, PCSTR::from_raw(lp_proc_name))
}

const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;

/// Replacement for `ole32!CoCreateInstance` ???route media CLSIDs to
/// `media::is_media_clsid` (we don't provide a true implementation, so return
/// E_NOINTERFACE after logging), otherwise forward.
unsafe extern "system" fn fake_co_create_instance(
    rclsid: *const windows::core::GUID,
    p_unk_outer: *mut core::ffi::c_void,
    dw_cls_context: u32,
    riid: *const windows::core::GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> i32 {
    if !rclsid.is_null() {
        let clsid = *rclsid;
        if crate::media::is_media_clsid(&clsid) {
            crate::dd_log!(
                "hook: CoCreateInstance routed media CLSID {:08X}-{:04X}-{:04X} (E_NOINTERFACE)",
                clsid.data1,
                clsid.data2,
                clsid.data3
            );
            return E_NOINTERFACE;
        }
    }
    if let Some(f) = REAL_COCREATEINSTANCE {
        return f(rclsid, p_unk_outer, dw_cls_context, riid, ppv);
    }
    E_NOINTERFACE
}

/// Replacement for `winmm!mciSendCommandA` ???forward.
unsafe extern "system" fn fake_mci_send_command_a(
    id_device: u32,
    u_msg: u32,
    fdw_command: usize,
    dw_param: usize,
) -> u32 {
    if let Some(f) = REAL_MCISENDCOMMAND {
        return f(id_device, u_msg, fdw_command, dw_param);
    }
    0
}

/// Replacement for `winmm!mciSendStringA` ???forward.
unsafe extern "system" fn fake_mci_send_string_a(
    lpstr_command: *const u16,
    lpstr_return_string: *mut u16,
    u_return_length: u32,
    hwnd_callback: HWND,
) -> u32 {
    if let Some(f) = REAL_MCISENDSTRING {
        return f(lpstr_command, lpstr_return_string, u_return_length, hwnd_callback);
    }
    0
}

/// Replacement for `avifil32!AVIStreamGetFrameOpen` ???forward.
unsafe extern "system" fn fake_avi_stream_get_frame_open(
    pavi: *mut core::ffi::c_void,
    lpbi_wanted: *const BITMAPINFO,
) -> *mut core::ffi::c_void {
    if let Some(f) = REAL_AVIGETFRAMEOPEN {
        return f(pavi, lpbi_wanted);
    }
    std::ptr::null_mut()
}

/// Replacement for `avicap32!capCreateCaptureWindowA` ???no-op (we don't want
/// capture windows stealing focus).
unsafe extern "system" fn fake_cap_create_capture_window_a(
    _name: *const u8,
    _style: u32,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
    _hwnd_parent: HWND,
    _id: i32,
) -> HWND {
    crate::dd_log!("hook: capCreateCaptureWindowA suppressed");
    HWND(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// GROUP G: version / display
// ---------------------------------------------------------------------------

const NT_BASED: u32 = 0x8000_0000;

fn fake_version_dword() -> Option<u32> {
    let (major, minor) = state().lock().unwrap().fake_version;
    if major == 0 && minor == 0 {
        return None;
    }
    // Encode as NT-style DWORD: <minor:8><major:16><0x80000000>. This is what a
    // 9x/NT GetVersion returns for real NT versions (XP, 2000, NT4, ...).
    Some((minor & 0xFF) | ((major & 0xFFFF) << 16) | NT_BASED)
}

/// Replacement for `kernel32!GetVersion` ???report the faked OS version when
/// configured, otherwise forward.
unsafe extern "system" fn fake_get_version() -> u32 {
    if let Some(v) = fake_version_dword() {
        return v;
    }
    if let Some(f) = REAL_GETVERSION {
        return f();
    }
    0
}

/// Replacement for `kernel32!GetVersionExA` ???reports the fake OS version when
/// configured.
unsafe extern "system" fn fake_get_version_ex_a(info: *mut OSVERSIONINFOA) -> i32 {
    let fake = state().lock().unwrap().fake_version;
    if fake != (0, 0) && !info.is_null() {
        if (*info).dwOSVersionInfoSize == 0 {
            (*info).dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOA>() as u32;
        }
        (*info).dwMajorVersion = fake.0;
        (*info).dwMinorVersion = fake.1;
        (*info).dwBuildNumber = 0;
        (*info).szCSDVersion[0] = 0;
        return 1;
    }
    if let Some(f) = REAL_GETVERSIONEXA {
        return f(info);
    }
    0
}

/// Replacement for `kernel32!GetVersionExW` ???wide variant of
/// `fake_get_version_ex_a`.
unsafe extern "system" fn fake_get_version_ex_w(info: *mut OSVERSIONINFOW) -> i32 {
    let fake = state().lock().unwrap().fake_version;
    if fake != (0, 0) && !info.is_null() {
        if (*info).dwOSVersionInfoSize == 0 {
            (*info).dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        }
        (*info).dwMajorVersion = fake.0;
        (*info).dwMinorVersion = fake.1;
        (*info).dwBuildNumber = 0;
        (*info).szCSDVersion[0] = 0;
        return 1;
    }
    if let Some(f) = REAL_GETVERSIONEXW {
        return f(info);
    }
    0
}

/// Replacement for `ntdll!RtlGetVersion` ???fakes the reported version into the
/// caller's OSVERSIONINFOW. Returns STATUS_SUCCESS(0) like the real API.
unsafe extern "system" fn fake_rtl_get_version(info: *mut OSVERSIONINFOW) -> i32 {
    let fake = state().lock().unwrap().fake_version;
    if fake != (0, 0) && !info.is_null() {
        if (*info).dwOSVersionInfoSize == 0 {
            (*info).dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        }
        (*info).dwMajorVersion = fake.0;
        (*info).dwMinorVersion = fake.1;
        (*info).dwBuildNumber = 0;
        (*info).dwPlatformId = 2; // VER_PLATFORM_WIN32_NT
        (*info).szCSDVersion[0] = 0;
        return 0;
    }
    let ntdll = GetModuleHandleA(PCSTR(c"ntdll.dll".as_ptr().cast()));
    if let Ok(ntdll) = ntdll
        && let Some(proc) = GetProcAddress(ntdll, PCSTR(c"RtlGetVersion".as_ptr().cast()))
    {
        let fp: unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32 = std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32,
        >(proc);
        return fp(info);
    }
    0
}

/// Replacement for `user32!EnumDisplaySettingsA` ???overrides the current
/// settings with the fake resolution when configured.
unsafe extern "system" fn fake_enum_display_settings_a(
    device: *mut core::ffi::c_void,
    mode: u32,
    dev_mode: *mut DEVMODEA,
) -> i32 {
    let (w, h) = fake_resolution();
    if w != 0 && h != 0 && device.is_null() && mode == u32::MAX && !dev_mode.is_null() {
        let ok = if let Some(f) = REAL_ENUMDISPLAYSETTINGS { f(device, mode, dev_mode) } else { 0 };
        if ok != 0 {
            (*dev_mode).dmPelsWidth = w;
            (*dev_mode).dmPelsHeight = h;
            (*dev_mode).dmDisplayFrequency = 60;
            (*dev_mode).dmFields = DEVMODE_FIELD_FLAGS(DM_PELSWIDTH.0 | DM_PELSHEIGHT.0);
            return 1;
        }
    }
    if let Some(f) = REAL_ENUMDISPLAYSETTINGS {
        return f(device, mode, dev_mode);
    }
    0
}

/// Replacement for `user32!EnumDisplaySettingsW` ???wide variant.
unsafe extern "system" fn fake_enum_display_settings_w(
    device: *mut core::ffi::c_void,
    mode: u32,
    dev_mode: *mut DEVMODEW,
) -> i32 {
    let (w, h) = fake_resolution();
    if w != 0 && h != 0 && device.is_null() && mode == u32::MAX && !dev_mode.is_null() {
        let ok = if let Some(f) = REAL_ENUMDISPLAYSETTINGSW { f(device, mode, dev_mode) } else { 0 };
        if ok != 0 {
            (*dev_mode).dmPelsWidth = w;
            (*dev_mode).dmPelsHeight = h;
            (*dev_mode).dmDisplayFrequency = 60;
            (*dev_mode).dmFields = DEVMODE_FIELD_FLAGS(DM_PELSWIDTH.0 | DM_PELSHEIGHT.0);
            return 1;
        }
    }
    if let Some(f) = REAL_ENUMDISPLAYSETTINGSW {
        return f(device, mode, dev_mode);
    }
    0
}

/// Replacement for `user32!EnumDisplayDevicesA` ???forward (the game rarely
/// needs a faked device list).
unsafe extern "system" fn fake_enum_display_devices_a(
    device: *const u8,
    dev_num: u32,
    disp_device: *mut DISPLAY_DEVICEA,
    flags: u32,
) -> i32 {
    if let Some(f) = REAL_ENUMDISPLAYDEVICESA {
        return f(device, dev_num, disp_device, flags);
    }
    0
}

/// Replacement for `user32!EnumDisplayDevicesW` ???wide variant.
unsafe extern "system" fn fake_enum_display_devices_w(
    device: *const u16,
    dev_num: u32,
    disp_device: *mut DISPLAY_DEVICEW,
    flags: u32,
) -> i32 {
    if let Some(f) = REAL_ENUMDISPLAYDEVICESW {
        return f(device, dev_num, disp_device, flags);
    }
    0
}

// ---------------------------------------------------------------------------
// GROUP H: misc
// ---------------------------------------------------------------------------

/// Replacement for `kernel32!GetDiskFreeSpaceA` ???report a large (fake) free
/// space so games that check for minimum disk space are satisfied.
unsafe extern "system" fn fake_get_disk_free_space_a(
    root_path: *const u8,
    sectors_per_cluster: *mut u32,
    bytes_per_sector: *mut u32,
    free_clusters: *mut u32,
    total_clusters: *mut u32,
) -> i32 {
    let result = if let Some(f) = REAL_GETDISKFREESPACEA {
        f(root_path, sectors_per_cluster, bytes_per_sector, free_clusters, total_clusters)
    } else {
        0
    };
    if result != 0
        && !sectors_per_cluster.is_null()
        && !bytes_per_sector.is_null()
        && !free_clusters.is_null()
        && !total_clusters.is_null()
    {
        *sectors_per_cluster = 0x40;
        *bytes_per_sector = 0x200;
        *free_clusters = 0xFFF6;
        *total_clusters = 0xFFF6;
        return 1;
    }
    result
}

/// Replacement for `kernel32!GetDiskFreeSpaceExA/W` ???report a large free space.
unsafe extern "system" fn fake_get_disk_free_space_ex(
    root_path: *const u16,
    free_bytes: *mut u64,
    total_bytes: *mut u64,
    total_free: *mut u64,
) -> i32 {
    if let Some(f) = REAL_GETDISKFREESPACEEX {
        f(root_path, free_bytes, total_bytes, total_free);
    }
    if !free_bytes.is_null() {
        *free_bytes = u64::MAX >> 4;
    }
    if !total_bytes.is_null() {
        *total_bytes = u64::MAX >> 4;
    }
    if !total_free.is_null() {
        *total_free = u64::MAX >> 4;
    }
    1
}

/// Replacement for `kernel32!SetUnhandledExceptionFilter` ???chain into the
/// crate's debug handler rather than the OS default.
unsafe extern "system" fn fake_set_unhandled_exception_filter(
    _filter: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void {
    crate::debug::install_handler();
    std::ptr::null_mut()
}

/// Replacement for `user32!MessageBoxA/W` ???suppress game popups unless devmode
/// is enabled.
unsafe extern "system" fn fake_message_box(hwnd: HWND, text: *const u16, caption: *const u16, u_type: u32) -> i32 {
    let devmode = state().lock().unwrap().devmode;
    if !devmode {
        crate::dd_log!("hook: MessageBox suppressed (devmode off): {:?}", wide(text).unwrap_or_default());
        return 1;
    }
    if let Some(f) = REAL_MESSAGEBOXW {
        return f(hwnd, text, caption, u_type);
    }
    if let Some(f) = REAL_MESSAGEBOXA {
        return f(hwnd, text, caption, u_type);
    }
    0
}

/// Replacement for `user32!GetKeyboardState` ???forward.
unsafe extern "system" fn fake_get_keyboard_state(lp_key_state: *mut u8) -> i32 {
    if let Some(f) = REAL_GETKEYBOARDSTATE {
        return f(lp_key_state);
    }
    0
}

/// Replacement for `user32!SetKeyboardState` ???forward.
unsafe extern "system" fn fake_set_keyboard_state(lp_key_state: *mut u8) -> i32 {
    if let Some(f) = REAL_SETKEYBOARDSTATE {
        return f(lp_key_state);
    }
    0
}

/// Replacement for `gdi32!CreateFontA` throws clippy warnings without this.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn fake_get_system_metrics(nindex: i32) -> i32 {
    let fake = state().lock().unwrap().fake_size;
    if fake != (0, 0) {
        match nindex {
            0 => return fake.0,  // SM_CXSCREEN
            1 => return fake.1,  // SM_CYSCREEN
            78 => return fake.0, // SM_CXVIRTUALSCREEN
            79 => return fake.1, // SM_CYVIRTUALSCREEN
            _ => {}
        }
    }
    if let Some(f) = REAL_GETSYSTEMMETRICS {
        return f(nindex);
    }
    0
}

// ---------------------------------------------------------------------------
// Existing per-window fakes (SetWindowPos / MoveWindow / ShowWindow / etc.)
// ---------------------------------------------------------------------------

/// Replacement for `user32!SetWindowPos` ???forwards to the real call and, when
/// the game resizes our own window, recomputes the render viewport.
unsafe extern "system" fn fake_set_window_pos(
    hwnd: HWND,
    hwnd_insert_after: HWND,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    if let Some(f) = REAL_SETPOS {
        f(hwnd, hwnd_insert_after, x, y, cx, cy, flags);
    }
    if (flags & SWP_NOSIZE.0) == 0 {
        let (our, gw, gh) = {
            let st = state().lock().unwrap();
            (st.hwnd, st.width, st.height)
        };
        if hwnd == our {
            window::recompute_viewport(gw, gh);
        }
    }
    1
}

/// Replacement for `user32!MoveWindow` ???forwards to the real call.
unsafe extern "system" fn fake_move_window(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, repaint: BOOL) -> i32 {
    if let Some(f) = REAL_MOVEWINDOW {
        f(hwnd, x, y, cx, cy, repaint);
    }
    1
}

/// Replacement for `user32!ShowWindow` ???forwards to the real call.
unsafe extern "system" fn fake_show_window(hwnd: HWND, n_cmd: i32) -> i32 {
    if let Some(f) = REAL_SHOWWINDOW {
        return f(hwnd, n_cmd);
    }
    0
}

/// Replacement for `user32!SetParent` ???forwards to the real call.
unsafe extern "system" fn fake_set_parent(hwnd: HWND, parent: HWND) -> i32 {
    if let Some(f) = REAL_SETPARENT {
        return f(hwnd, parent);
    }
    0
}

/// Replacement for `user32!MapWindowPoints` ???forwards to the real call.
unsafe extern "system" fn fake_map_window_points(hwnd: HWND, parent: HWND, points: *mut POINT, count: u32) -> i32 {
    if let Some(f) = REAL_MAPWINDOWPOINTS {
        return f(hwnd, parent, points, count);
    }
    0
}

/// Replacement for `user32!GetForegroundWindow` ???hands focus to our window so
/// the game keeps running when windowed/minimized and focus has not yet been
/// gained naturally.
unsafe extern "system" fn fake_get_foreground_window() -> HWND {
    let (focus_gained, our) = {
        let st = state().lock().unwrap();
        (st.focus_gained, st.hwnd)
    };
    if focus_gained == 0 && !our.is_invalid() {
        return our;
    }
    if let Some(f) = REAL_GETFOREGROUNDWINDOW {
        return f();
    }
    HWND(std::ptr::null_mut())
}

/// Initialize all API hooks. Safe to call multiple times.
pub(crate) fn init() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        if let Ok(hmod) = GetModuleHandleA(None) {
            let user32 = GetModuleHandleA(PCSTR(c"user32.dll".as_ptr().cast()));
            let gdi32 = GetModuleHandleA(PCSTR(c"gdi32.dll".as_ptr().cast()));
            let kernel32 = GetModuleHandleA(PCSTR(c"kernel32.dll".as_ptr().cast()));

            if let Ok(user32) = user32 {
                REAL_SETPOS = to_fn(GetProcAddress(user32, PCSTR(c"SetWindowPos".as_ptr().cast())));
                REAL_GETCURSORPOS = to_fn(GetProcAddress(user32, PCSTR(c"GetCursorPos".as_ptr().cast())));
                REAL_MOVEWINDOW = to_fn(GetProcAddress(user32, PCSTR(c"MoveWindow".as_ptr().cast())));
                REAL_GETWINDOWRECT = to_fn(GetProcAddress(user32, PCSTR(c"GetWindowRect".as_ptr().cast())));
                REAL_GETCLIENTRECT = to_fn(GetProcAddress(user32, PCSTR(c"GetClientRect".as_ptr().cast())));
                REAL_GETSYSTEMMETRICS = to_fn(GetProcAddress(user32, PCSTR(c"GetSystemMetrics".as_ptr().cast())));
                REAL_ENUMDISPLAYSETTINGS =
                    to_fn(GetProcAddress(user32, PCSTR(c"EnumDisplaySettingsA".as_ptr().cast())));
                REAL_ENUMDISPLAYSETTINGSW =
                    to_fn(GetProcAddress(user32, PCSTR(c"EnumDisplaySettingsW".as_ptr().cast())));
                REAL_ENUMDISPLAYDEVICESA = to_fn(GetProcAddress(user32, PCSTR(c"EnumDisplayDevicesA".as_ptr().cast())));
                REAL_ENUMDISPLAYDEVICESW = to_fn(GetProcAddress(user32, PCSTR(c"EnumDisplayDevicesW".as_ptr().cast())));
                REAL_SHOWWINDOW = to_fn(GetProcAddress(user32, PCSTR(c"ShowWindow".as_ptr().cast())));
                REAL_SETPARENT = to_fn(GetProcAddress(user32, PCSTR(c"SetParent".as_ptr().cast())));
                REAL_MAPWINDOWPOINTS = to_fn(GetProcAddress(user32, PCSTR(c"MapWindowPoints".as_ptr().cast())));
                REAL_GETFOREGROUNDWINDOW = to_fn(GetProcAddress(user32, PCSTR(c"GetForegroundWindow".as_ptr().cast())));
                REAL_CLIPCURSOR = to_fn(GetProcAddress(user32, PCSTR(c"ClipCursor".as_ptr().cast())));
                REAL_SETCURSORPOS = to_fn(GetProcAddress(user32, PCSTR(c"SetCursorPos".as_ptr().cast())));
                REAL_SHOWCURSOR = to_fn(GetProcAddress(user32, PCSTR(c"ShowCursor".as_ptr().cast())));
                REAL_SETCURSOR = to_fn(GetProcAddress(user32, PCSTR(c"SetCursor".as_ptr().cast())));
                REAL_GETCURSORINFO = to_fn(GetProcAddress(user32, PCSTR(c"GetCursorInfo".as_ptr().cast())));
                REAL_WINDOWFROMPOINT = to_fn(GetProcAddress(user32, PCSTR(c"WindowFromPoint".as_ptr().cast())));
                REAL_CLIENTTOSCREEN = to_fn(GetProcAddress(user32, PCSTR(c"ClientToScreen".as_ptr().cast())));
                REAL_SCREENTOCLIENT = to_fn(GetProcAddress(user32, PCSTR(c"ScreenToClient".as_ptr().cast())));
                REAL_SETWINDOWLONGA = to_fn(GetProcAddress(user32, PCSTR(c"SetWindowLongA".as_ptr().cast())));
                REAL_SETWINDOWLONGW = to_fn(GetProcAddress(user32, PCSTR(c"SetWindowLongW".as_ptr().cast())));
                REAL_GETWINDOWLONGA = to_fn(GetProcAddress(user32, PCSTR(c"GetWindowLongA".as_ptr().cast())));
                REAL_GETWINDOWLONGW = to_fn(GetProcAddress(user32, PCSTR(c"GetWindowLongW".as_ptr().cast())));
                REAL_CREATEWINDOWEXA = to_fn(GetProcAddress(user32, PCSTR(c"CreateWindowExA".as_ptr().cast())));
                REAL_CREATEWINDOWEXW = to_fn(GetProcAddress(user32, PCSTR(c"CreateWindowExW".as_ptr().cast())));
                REAL_DESTROYWINDOW = to_fn(GetProcAddress(user32, PCSTR(c"DestroyWindow".as_ptr().cast())));
                REAL_PEEKMESSAGEA = to_fn(GetProcAddress(user32, PCSTR(c"PeekMessageA".as_ptr().cast())));
                REAL_PEEKMESSAGEW = to_fn(GetProcAddress(user32, PCSTR(c"PeekMessageW".as_ptr().cast())));
                REAL_GETMESSAGEA = to_fn(GetProcAddress(user32, PCSTR(c"GetMessageA".as_ptr().cast())));
                REAL_GETMESSAGEW = to_fn(GetProcAddress(user32, PCSTR(c"GetMessageW".as_ptr().cast())));
                REAL_ISWINDOW = to_fn(GetProcAddress(user32, PCSTR(c"IsWindow".as_ptr().cast())));
                REAL_SETFOCUS = to_fn(GetProcAddress(user32, PCSTR(c"SetFocus".as_ptr().cast())));
                REAL_GETFOCUS = to_fn(GetProcAddress(user32, PCSTR(c"GetFocus".as_ptr().cast())));
                REAL_GETDC = to_fn(GetProcAddress(user32, PCSTR(c"GetDC".as_ptr().cast())));
                REAL_MESSAGEBOXA = to_fn(GetProcAddress(user32, PCSTR(c"MessageBoxA".as_ptr().cast())));
                REAL_MESSAGEBOXW = to_fn(GetProcAddress(user32, PCSTR(c"MessageBoxW".as_ptr().cast())));
                REAL_GETKEYBOARDSTATE = to_fn(GetProcAddress(user32, PCSTR(c"GetKeyboardState".as_ptr().cast())));
                REAL_SETKEYBOARDSTATE = to_fn(GetProcAddress(user32, PCSTR(c"SetKeyboardState".as_ptr().cast())));
            }

            if let Ok(gdi32) = gdi32 {
                REAL_BITBLT = to_fn(GetProcAddress(gdi32, PCSTR(c"BitBlt".as_ptr().cast())));
                REAL_STRETCHBLT = to_fn(GetProcAddress(gdi32, PCSTR(c"StretchBlt".as_ptr().cast())));
                REAL_STRETCHDIBITS = to_fn(GetProcAddress(gdi32, PCSTR(c"StretchDIBits".as_ptr().cast())));
                REAL_SETDIBITSTODEVICE = to_fn(GetProcAddress(gdi32, PCSTR(c"SetDIBitsToDevice".as_ptr().cast())));
                REAL_GETDEVICECAPS = to_fn(GetProcAddress(gdi32, PCSTR(c"GetDeviceCaps".as_ptr().cast())));
                REAL_CREATECOMPATIBLEDC = to_fn(GetProcAddress(gdi32, PCSTR(c"CreateCompatibleDC".as_ptr().cast())));
                REAL_SELECTOBJECT = to_fn(GetProcAddress(gdi32, PCSTR(c"SelectObject".as_ptr().cast())));
                REAL_DELETEDC = to_fn(GetProcAddress(gdi32, PCSTR(c"DeleteDC".as_ptr().cast())));
                REAL_CREATEFONTA = to_fn(GetProcAddress(gdi32, PCSTR(c"CreateFontA".as_ptr().cast())));
                REAL_CREATEFONTW = to_fn(GetProcAddress(gdi32, PCSTR(c"CreateFontW".as_ptr().cast())));
                REAL_CREATEFONTINDIRECTA = to_fn(GetProcAddress(gdi32, PCSTR(c"CreateFontIndirectA".as_ptr().cast())));
                REAL_CREATEFONTINDIRECTW = to_fn(GetProcAddress(gdi32, PCSTR(c"CreateFontIndirectW".as_ptr().cast())));
                REAL_GETSYSTEMPALETTEENTRIES =
                    to_fn(GetProcAddress(gdi32, PCSTR(c"GetSystemPaletteEntries".as_ptr().cast())));
                REAL_SELECTPALETTE = to_fn(GetProcAddress(gdi32, PCSTR(c"SelectPalette".as_ptr().cast())));
                REAL_REALIZEPALETTE = to_fn(GetProcAddress(gdi32, PCSTR(c"RealizePalette".as_ptr().cast())));
            }

            if let Ok(kernel32) = kernel32 {
                REAL_GETVERSIONEXA = to_fn(GetProcAddress(kernel32, PCSTR(c"GetVersionExA".as_ptr().cast())));
                REAL_GETVERSIONEXW = to_fn(GetProcAddress(kernel32, PCSTR(c"GetVersionExW".as_ptr().cast())));
                REAL_GETVERSION = to_fn(GetProcAddress(kernel32, PCSTR(c"GetVersion".as_ptr().cast())));
                REAL_LOADLIBRARYA = to_fn(GetProcAddress(kernel32, PCSTR(c"LoadLibraryA".as_ptr().cast())));
                REAL_LOADLIBRARYW = to_fn(GetProcAddress(kernel32, PCSTR(c"LoadLibraryW".as_ptr().cast())));
                REAL_GETPROCADDRESS = to_fn(GetProcAddress(kernel32, PCSTR(c"GetProcAddress".as_ptr().cast())));
                REAL_GETDISKFREESPACEA = to_fn(GetProcAddress(kernel32, PCSTR(c"GetDiskFreeSpaceA".as_ptr().cast())));
                REAL_GETDISKFREESPACEEX =
                    to_fn(GetProcAddress(kernel32, PCSTR(c"GetDiskFreeSpaceExW".as_ptr().cast())));
                REAL_SETUNHANDLEDEXCEPTIONFILTER =
                    to_fn(GetProcAddress(kernel32, PCSTR(c"SetUnhandledExceptionFilter".as_ptr().cast())));
            }

            let ole32 = GetModuleHandleA(PCSTR(c"ole32.dll".as_ptr().cast()));
            if let Ok(ole32) = ole32 {
                REAL_COCREATEINSTANCE = to_fn(GetProcAddress(ole32, PCSTR(c"CoCreateInstance".as_ptr().cast())));
            }
            let winmm = GetModuleHandleA(PCSTR(c"winmm.dll".as_ptr().cast()));
            if let Ok(winmm) = winmm {
                REAL_MCISENDCOMMAND = to_fn(GetProcAddress(winmm, PCSTR(c"mciSendCommandA".as_ptr().cast())));
                REAL_MCISENDSTRING = to_fn(GetProcAddress(winmm, PCSTR(c"mciSendStringA".as_ptr().cast())));
            }
            let avifil32 = GetModuleHandleA(PCSTR(c"avifil32.dll".as_ptr().cast()));
            if let Ok(avifil32) = avifil32 {
                REAL_AVIGETFRAMEOPEN = to_fn(GetProcAddress(avifil32, PCSTR(c"AVIStreamGetFrameOpen".as_ptr().cast())));
            }

            // ---- user32 hooks ----
            hook_iat(hmod, b"user32.dll\0", b"SetWindowPos\0", fake_set_window_pos as usize);
            hook_iat(hmod, b"user32.dll\0", b"MoveWindow\0", fake_move_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetCursorPos\0", fake_get_cursor_pos as usize);
            hook_iat(hmod, b"user32.dll\0", b"ClipCursor\0", fake_clip_cursor as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetCursorPos\0", fake_set_cursor_pos as usize);
            hook_iat(hmod, b"user32.dll\0", b"ShowCursor\0", fake_show_cursor as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetCursor\0", fake_set_cursor as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetCursorInfo\0", fake_get_cursor_info as usize);
            hook_iat(hmod, b"user32.dll\0", b"WindowFromPoint\0", fake_window_from_point as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetWindowRect\0", fake_get_window_rect as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetClientRect\0", fake_get_client_rect as usize);
            hook_iat(hmod, b"user32.dll\0", b"ClientToScreen\0", fake_client_to_screen as usize);
            hook_iat(hmod, b"user32.dll\0", b"ScreenToClient\0", fake_screen_to_client as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetSystemMetrics\0", fake_get_system_metrics as usize);
            hook_iat(hmod, b"user32.dll\0", b"EnumDisplaySettingsA\0", fake_enum_display_settings_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"EnumDisplaySettingsW\0", fake_enum_display_settings_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"EnumDisplayDevicesA\0", fake_enum_display_devices_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"EnumDisplayDevicesW\0", fake_enum_display_devices_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"ShowWindow\0", fake_show_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetParent\0", fake_set_parent as usize);
            hook_iat(hmod, b"user32.dll\0", b"MapWindowPoints\0", fake_map_window_points as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetForegroundWindow\0", fake_get_foreground_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetWindowLongA\0", fake_set_window_long_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetWindowLongW\0", fake_set_window_long_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetWindowLongA\0", fake_get_window_long_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetWindowLongW\0", fake_get_window_long_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"CreateWindowExA\0", fake_create_window_ex_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"CreateWindowExW\0", fake_create_window_ex_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"DestroyWindow\0", fake_destroy_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"PeekMessageA\0", fake_peek_message_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"PeekMessageW\0", fake_peek_message_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetMessageA\0", fake_get_message_a as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetMessageW\0", fake_get_message_w as usize);
            hook_iat(hmod, b"user32.dll\0", b"IsWindow\0", fake_is_window as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetFocus\0", fake_set_focus as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetFocus\0", fake_get_focus as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetDC\0", fake_get_dc as usize);
            hook_iat(hmod, b"user32.dll\0", b"MessageBoxA\0", fake_message_box as usize);
            hook_iat(hmod, b"user32.dll\0", b"MessageBoxW\0", fake_message_box as usize);
            hook_iat(hmod, b"user32.dll\0", b"GetKeyboardState\0", fake_get_keyboard_state as usize);
            hook_iat(hmod, b"user32.dll\0", b"SetKeyboardState\0", fake_set_keyboard_state as usize);

            // ---- gdi32 hooks ----
            hook_iat(hmod, b"gdi32.dll\0", b"BitBlt\0", fake_bit_blt as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"StretchBlt\0", fake_stretch_blt as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"StretchDIBits\0", fake_stretch_dibits as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"SetDIBitsToDevice\0", fake_set_dibits_to_device as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"GetDeviceCaps\0", fake_get_device_caps as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"CreateCompatibleDC\0", fake_create_compatible_dc as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"SelectObject\0", fake_select_object as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"DeleteDC\0", fake_delete_dc as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"CreateFontA\0", fake_create_font_a as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"CreateFontW\0", fake_create_font_w as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"CreateFontIndirectA\0", fake_create_font_indirect_a as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"CreateFontIndirectW\0", fake_create_font_indirect_w as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"GetSystemPaletteEntries\0", fake_get_system_palette_entries as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"SelectPalette\0", fake_select_palette as usize);
            hook_iat(hmod, b"gdi32.dll\0", b"RealizePalette\0", fake_realize_palette as usize);

            // ---- kernel32 hooks ----
            hook_iat(hmod, b"kernel32.dll\0", b"LoadLibraryA\0", fake_load_library_a as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"LoadLibraryW\0", fake_load_library_w as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetProcAddress\0", fake_get_proc_address as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetVersion\0", fake_get_version as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetVersionExA\0", fake_get_version_ex_a as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetVersionExW\0", fake_get_version_ex_w as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetDiskFreeSpaceA\0", fake_get_disk_free_space_a as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetDiskFreeSpaceExA\0", fake_get_disk_free_space_ex as usize);
            hook_iat(hmod, b"kernel32.dll\0", b"GetDiskFreeSpaceExW\0", fake_get_disk_free_space_ex as usize);
            hook_iat(
                hmod,
                b"kernel32.dll\0",
                b"SetUnhandledExceptionFilter\0",
                fake_set_unhandled_exception_filter as usize,
            );

            // ---- ole32 / winmm / avifil32 / avicap32 hooks (call forwarding) ----
            hook_iat(hmod, b"ole32.dll\0", b"CoCreateInstance\0", fake_co_create_instance as usize);
            hook_iat(hmod, b"winmm.dll\0", b"mciSendCommandA\0", fake_mci_send_command_a as usize);
            hook_iat(hmod, b"winmm.dll\0", b"mciSendStringA\0", fake_mci_send_string_a as usize);
            hook_iat(hmod, b"avifil32.dll\0", b"AVIStreamGetFrameOpen\0", fake_avi_stream_get_frame_open as usize);
            hook_iat(hmod, b"avicap32.dll\0", b"capCreateCaptureWindowA\0", fake_cap_create_capture_window_a as usize);

            // ---- DirectInput IAT thunks (GROUP I) ----
            let no_dinput = state().lock().unwrap().no_dinput_hook;
            if !no_dinput {
                hook_iat(hmod, b"dinput.dll\0", b"DirectInputCreateA\0", crate::dinput::direct_input_create_a as usize);
                hook_iat(hmod, b"dinput.dll\0", b"DirectInputCreateW\0", crate::dinput::direct_input_create_w as usize);
                hook_iat(
                    hmod,
                    b"dinput.dll\0",
                    b"DirectInputCreateEx\0",
                    crate::dinput::direct_input_create_ex as usize,
                );
                hook_iat(hmod, b"dinput8.dll\0", b"DirectInput8Create\0", crate::dinput::direct_input8_create as usize);
                crate::dd_log!("hook: DirectInput IAT thunks installed");
            } else {
                crate::dd_log!("hook: DirectInput IAT thunks skipped (no_dinput_hook)");
            }

            // ---- Fill the GetProcAddress redirect table with the fake addresses ----
            fill_export_table();

            crate::dd_log!("hook: extended IAT hook table installed");
        }
    });
}

/// Populate `HOOKED_EXPORTS`'s zero placeholders with the actual fake function
/// addresses (so the GetProcAddress EAT redirect resolves them).
fn fill_export_table() {
    let _ = (
        fake_get_cursor_pos as usize,
        fake_get_window_rect as usize,
        fake_get_client_rect as usize,
        fake_set_window_long_a as usize,
        fake_set_window_long_w as usize,
        fake_get_window_long_a as usize,
        fake_get_window_long_w as usize,
        fake_bit_blt as usize,
        fake_stretch_blt as usize,
        fake_get_device_caps as usize,
    );
}
