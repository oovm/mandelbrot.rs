use std::ffi::c_void;
use std::sync::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::core::*;

use crate::dd_log;
use crate::ddraw::DirectDrawImpl;
use crate::ddraw::clipper::ClipperImpl;
use crate::state;

type EnumCallbackA = unsafe extern "system" fn(
    *const GUID,
    *mut core::ffi::c_char,
    *mut core::ffi::c_char,
    *mut core::ffi::c_void,
) -> HRESULT;
type EnumCallbackW = unsafe extern "system" fn(*const GUID, *mut u16, *mut u16, *mut core::ffi::c_void) -> HRESULT;
type EnumCallbackExA = unsafe extern "system" fn(
    *const GUID,
    *mut core::ffi::c_char,
    *mut core::ffi::c_char,
    *mut core::ffi::c_void,
    u32,
) -> HRESULT;
type EnumCallbackExW =
    unsafe extern "system" fn(*const GUID, *mut u16, *mut u16, *mut core::ffi::c_void, u32) -> HRESULT;

const GUID_NULL: GUID = GUID { data1: 0, data2: 0, data3: 0, data4: [0; 8] };

fn enum_primary_a(lpcallback: *mut c_void, lpcontext: *mut c_void, dwflags: Option<u32>) -> HRESULT {
    if lpcallback.is_null() {
        return HRESULT(0);
    }
    let description: &[u8] = b"Primary Display Driver\0";
    let name: &[u8] = b"display\0";
    let lpguid: *const GUID = &GUID_NULL;
    let d = description.as_ptr() as *mut core::ffi::c_char;
    let n = name.as_ptr() as *mut core::ffi::c_char;
    match dwflags {
        Some(flags) => unsafe {
            let cb: EnumCallbackExA = std::mem::transmute(lpcallback);
            cb(lpguid, d, n, lpcontext, flags)
        },
        None => unsafe {
            let cb: EnumCallbackA = std::mem::transmute(lpcallback);
            cb(lpguid, d, n, lpcontext)
        },
    }
}

fn enum_primary_w(lpcallback: *mut c_void, lpcontext: *mut c_void, dwflags: Option<u32>) -> HRESULT {
    if lpcallback.is_null() {
        return HRESULT(0);
    }
    let description: Vec<u16> = "Primary Display Driver\0".encode_utf16().collect();
    let name: Vec<u16> = "display\0".encode_utf16().collect();
    let lpguid: *const GUID = &GUID_NULL;
    let d: *mut u16 = description.as_ptr() as *mut u16;
    let n: *mut u16 = name.as_ptr() as *mut u16;
    match dwflags {
        Some(flags) => unsafe {
            let cb: EnumCallbackExW = std::mem::transmute(lpcallback);
            cb(lpguid, d, n, lpcontext, flags)
        },
        None => unsafe {
            let cb: EnumCallbackW = std::mem::transmute(lpcallback);
            cb(lpguid, d, n, lpcontext)
        },
    }
}

// --- Core DirectDraw exports ---

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreate(
    lpguid: *mut GUID,
    lplpdd: *mut Option<IDirectDraw>,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!("DirectDrawCreate(lpGUID={:p}, lplpDD={:p}, pUnkOuter={:p})", lpguid, lplpdd, punkouter);
    if lplpdd.is_null() {
        dd_log!("  -> E_INVALIDARG (lplpDD is null)");
        return E_INVALIDARG;
    }
    unsafe {
        crate::config::load();
        crate::hook::init();
        crate::util::init_system();
    }
    // Log the resolved settings here (after the ini is loaded), so the renderer
    // line reflects the actual configuration instead of the pre-config defaults.
    crate::debug::log_init_info();
    let dd: IDirectDraw = DirectDrawImpl {}.into();
    unsafe { *lplpdd = Some(dd) };
    dd_log!("  -> DD_OK (IDirectDraw created)");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreateEx(
    lpguid: *mut GUID,
    lplpdd: *mut Option<IDirectDraw7>,
    iid: *const GUID,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!("DirectDrawCreateEx(lpGUID={:p}, lplpDD={:p}, iid={:p}, pUnkOuter={:p})", lpguid, lplpdd, iid, punkouter);
    if lplpdd.is_null() {
        dd_log!("  -> E_INVALIDARG (lplpDD is null)");
        return E_INVALIDARG;
    }
    let dd: IDirectDraw7 = DirectDrawImpl {}.into();
    unsafe {
        crate::config::load();
        crate::hook::init();
        crate::util::init_system();
        *lplpdd = Some(dd);
    }
    crate::debug::log_init_info();
    dd_log!("  -> DD_OK (IDirectDraw7 created)");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawCreateClipper(
    dwflags: u32,
    lplpddclipper: *mut Option<IDirectDrawClipper>,
    punkouter: *mut Option<IUnknown>,
) -> HRESULT {
    dd_log!(
        "DirectDrawCreateClipper(dwFlags={:#x}, lplpDDClipper={:p}, pUnkOuter={:p})",
        dwflags,
        lplpddclipper,
        punkouter
    );
    if lplpddclipper.is_null() {
        dd_log!("  -> E_INVALIDARG");
        return E_INVALIDARG;
    }
    let clipper: IDirectDrawClipper =
        ClipperImpl { hwnd: Mutex::new(0), clip: Mutex::new(Vec::new()), changed: Mutex::new(false) }.into();
    unsafe { *lplpddclipper = Some(clipper) };
    dd_log!("  -> DD_OK");
    HRESULT(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateA(lpcallback: *mut c_void, lpcontext: *mut c_void) -> HRESULT {
    dd_log!("DirectDrawEnumerateA: enumerating primary display");
    enum_primary_a(lpcallback, lpcontext, None)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateW(lpcallback: *mut c_void, lpcontext: *mut c_void) -> HRESULT {
    dd_log!("DirectDrawEnumerateW: enumerating primary display");
    enum_primary_w(lpcallback, lpcontext, None)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateExA(
    lpcallback: *mut c_void,
    lpcontext: *mut c_void,
    dwflags: u32,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateExA: enumerating primary display (flags={:#x})", dwflags);
    enum_primary_a(lpcallback, lpcontext, Some(dwflags))
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectDrawEnumerateExW(
    lpcallback: *mut c_void,
    lpcontext: *mut c_void,
    dwflags: u32,
) -> HRESULT {
    dd_log!("DirectDrawEnumerateExW: enumerating primary display (flags={:#x})", dwflags);
    enum_primary_w(lpcallback, lpcontext, Some(dwflags))
}

// --- DirectInput forwarders ---

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DirectInputCreateA(
    hinst: windows::Win32::Foundation::HINSTANCE,
    dwversion: u32,
    riidltf: *const GUID,
    ppvout: *mut *mut c_void,
    punkouter: *mut c_void,
) -> i32 {
    dd_log!("DirectInputCreateA(hInst={:p}, dwVersion={:#x})", hinst.0, dwversion);
    crate::dinput::direct_input_create_a(hinst, dwversion, riidltf, ppvout, punkouter)
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DirectInputCreateW(
    hinst: windows::Win32::Foundation::HINSTANCE,
    dwversion: u32,
    riidltf: *const GUID,
    ppvout: *mut *mut c_void,
    punkouter: *mut c_void,
) -> i32 {
    dd_log!("DirectInputCreateW(hInst={:p}, dwVersion={:#x})", hinst.0, dwversion);
    crate::dinput::direct_input_create_w(hinst, dwversion, riidltf, ppvout, punkouter)
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DirectInputCreateEx(
    hinst: windows::Win32::Foundation::HINSTANCE,
    dwversion: u32,
    riidltf: *const GUID,
    ppvout: *mut *mut c_void,
    punkouter: *mut c_void,
) -> i32 {
    dd_log!("DirectInputCreateEx(hInst={:p}, dwVersion={:#x})", hinst.0, dwversion);
    crate::dinput::direct_input_create_ex(hinst, dwversion, riidltf, ppvout, punkouter)
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DirectInput8Create(
    hinst: windows::Win32::Foundation::HINSTANCE,
    dwversion: u32,
    riidltf: *const GUID,
    ppvout: *mut *mut c_void,
    punkouter: *mut c_void,
) -> i32 {
    dd_log!("DirectInput8Create(hInst={:p}, dwVersion={:#x})", hinst.0, dwversion);
    crate::dinput::direct_input8_create(hinst, dwversion, riidltf, ppvout, punkouter)
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DDEnableZoom() -> HRESULT {
    dd_log!("DDEnableZoom -> E_NOTIMPL");
    E_NOTIMPL
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DDIsWindowed() -> i32 {
    let st = state::state().lock().unwrap();
    let windowed = (st.dw_flags & DDSCL_FULLSCREEN as u32) == 0;
    dd_log!("DDIsWindowed -> {}", if windowed { 1 } else { 0 });
    if windowed { 1 } else { 0 }
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn GameHandlesClose() -> i32 {
    1
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn pvBmpBits() -> *mut c_void {
    let Some(buffers) = state::state().lock().unwrap().primary.clone() else {
        dd_log!("pvBmpBits: no primary surface");
        return std::ptr::null_mut();
    };
    let surface = buffers.surface as *mut c_void;
    if surface.is_null() {
        dd_log!("pvBmpBits: primary surface has no lockable bits");
        return std::ptr::null_mut();
    }
    dd_log!("pvBmpBits -> {:p}", surface);
    surface
}

// --- Stub exports ---

#[unsafe(no_mangle)]
pub extern "system" fn AcquireDDThreadLock() -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseDDThreadLock() -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn CompleteCreateSysmemSurface(_a: u32) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn D3DParseUnknownCommand(_lpcmd: *const c_void, _lpretcmd: *mut *mut c_void) -> HRESULT {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn DDInternalLock(_a: u32, _b: u32) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DDInternalUnlock(_a: u32) -> u32 {
    0
}
