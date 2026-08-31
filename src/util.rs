//! Misc helper functions ported from ts-ddraw `main.c` / `IDirectDraw.c`.

use windows::Win32::Foundation::RECT;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::UI::HiDpi::{PROCESS_DPI_AWARENESS, SetProcessDpiAwareness};
use windows::core::PCSTR;

fn pcstr(bytes: &[u8]) -> PCSTR {
    PCSTR(bytes.as_ptr())
}

/// Returns true when running under Wine.
pub unsafe fn is_wine() -> bool {
    if let Ok(dll) = GetModuleHandleA(pcstr(b"ntdll\0"))
        && !dll.is_invalid()
    {
        return GetProcAddress(dll, pcstr(b"wine_get_version\0")).is_some();
    }
    false
}

/// Returns true when running on Windows XP (and not Wine).
pub unsafe fn is_windows_xp() -> bool {
    let version = windows::Win32::System::SystemInformation::GetVersion();
    let major = (version & 0x0000_00FF) as u8;
    let minor = ((version & 0x0000_FF00) >> 8) as u8;
    if major == 5
        && minor == 1
        && let Ok(dll) = GetModuleHandleA(pcstr(b"ntdll\0"))
        && !dll.is_invalid()
    {
        return GetProcAddress(dll, pcstr(b"wine_get_version\0")).is_none();
    }
    false
}

/// Inverse of `AdjustWindowRectEx`.
pub unsafe fn unadjust_window_rect_ex(prc: *mut RECT, dw_style: u32, f_menu: bool, dw_ex_style: u32) -> bool {
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if windows::Win32::UI::WindowsAndMessaging::AdjustWindowRectEx(
        &mut rc,
        windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(dw_style),
        f_menu,
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(dw_ex_style),
    )
    .is_ok()
    {
        if !prc.is_null() {
            (*prc).left -= rc.left;
            (*prc).top -= rc.top;
            (*prc).right -= rc.right;
            (*prc).bottom -= rc.bottom;
        }
        true
    } else {
        false
    }
}

/// One-time system initialization mirroring ts-ddraw's `main.c`:
/// per-monitor DPI awareness.
pub unsafe fn init_system() {
    let _ = SetProcessDpiAwareness(PROCESS_DPI_AWARENESS(2));
}
