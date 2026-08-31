//! DirectInput bridging (ports `directinput.c`).
//!
//! The exe imports `DirectInputCreateA/W/Ex` (dinput.dll) and/or
//! `DirectInput8Create`. `hook.rs` patches those IAT thunks to the functions
//! below, which create the *real* device via the stock dinput implementation
//! and then wrap its mouse device so `GetDeviceState` / `GetDeviceData`
//! deliver adjmouse-scaled coordinates. See the `no_dinput_hook` config key.

use std::os::raw::c_void;

use windows::Win32::Foundation::{FARPROC, HANDLE, HINSTANCE, HWND};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, PCSTR, PCWSTR};

/// Fallback HRESULT returned on any wrapped-path failure (E_FAIL-style error
/// in the DI facility); the caller sees a failed DirectInputCreate.
const DIERR_FALLBACK: i32 = 0x8000_4018u32 as i32;

// ---- GUIDs (byte-exact values from the DirectInput SDK) ----
static GUID_SYSMOUSE: GUID =
    GUID { data1: 0x6F1D2B60, data2: 0x5A5A, data3: 0x11CF, data4: [0xBF, 0x06, 0x00, 0xA0, 0xC9, 0x22, 0xCF, 0x6E] };
static GUID_XAXIS: GUID =
    GUID { data1: 0xA36D02E0, data2: 0xC9F3, data3: 0x11CF, data4: [0xBF, 0xC7, 0x00, 0xA0, 0xC9, 0x22, 0xCF, 0x6E] };
static GUID_YAXIS: GUID =
    GUID { data1: 0xA36D02E1, data2: 0xC9F3, data3: 0x11CF, data4: [0xBF, 0xC7, 0x00, 0xA0, 0xC9, 0x22, 0xCF, 0x6E] };
static GUID_ZAXIS: GUID =
    GUID { data1: 0xA36D02E2, data2: 0xC9F3, data3: 0x11CF, data4: [0xBF, 0xC7, 0x00, 0xA0, 0xC9, 0x22, 0xCF, 0x6E] };

// DirectInput cooperative-level flags.
const DISCL_FOREGROUND: u32 = 0x0000_0001;
const DISCL_NONEXCLUSIVE: u32 = 0x0000_0002;

// Data-format offsets / object types.
const DIMOFS_X: u32 = 0;
const DIMOFS_Y: u32 = 4;
const DIMOFS_Z: u32 = 8;
const DIMOFS_BUTTON0: u32 = 12;
const DIDFT_AXIS: u32 = 0x0000_0003;
const DIDFT_BUTTON: u32 = 0x0000_0004;
const DIDFT_ANYINSTANCE: u32 = 0x00FF_FF00;

/// `DiObjectDataFormat`: one axis/button entry in a `DiDataFormat`.
#[repr(C)]
struct DiObjectDataFormat {
    pguid: *const GUID,
    dw_ofs: u32,
    dw_type: u32,
}

/// `DiDataFormat`: describes the layout of the data returned in a state read.
#[repr(C)]
struct DiDataFormat {
    dw_size: u32,
    dw_obj_size: u32,
    dw_flags: u32,
    dw_data_size: u32,
    dw_num_objs: u32,
    rgodf: *const DiObjectDataFormat,
}

// These descriptors are immutable read-only tables shared via `static`.
unsafe impl Sync for DiObjectDataFormat {}
unsafe impl Sync for DiDataFormat {}

/// `DiMouseState`: the standard 4-axis mouse data (3 axes + 4 buttons).
#[repr(C)]
struct DiMouseState {
    l_x: i32,
    l_y: i32,
    l_z: i32,
    rgb_buttons: [u8; 4],
}

/// `DiDeviceObjectData`: one buffered device-data record.
#[repr(C)]
struct DiDeviceObjectData {
    dw_ofs: u32,
    dw_data: u32,
    dw_time_stamp: u32,
    dw_sequence: u32,
}

static DIMOUSE_OBJECTS: [DiObjectDataFormat; 4] = [
    DiObjectDataFormat { pguid: &GUID_XAXIS, dw_ofs: DIMOFS_X, dw_type: DIDFT_AXIS | DIDFT_ANYINSTANCE },
    DiObjectDataFormat { pguid: &GUID_YAXIS, dw_ofs: DIMOFS_Y, dw_type: DIDFT_AXIS | DIDFT_ANYINSTANCE },
    DiObjectDataFormat { pguid: &GUID_ZAXIS, dw_ofs: DIMOFS_Z, dw_type: DIDFT_AXIS | DIDFT_ANYINSTANCE },
    DiObjectDataFormat { pguid: std::ptr::null(), dw_ofs: DIMOFS_BUTTON0, dw_type: DIDFT_BUTTON | DIDFT_ANYINSTANCE },
];

/// `c_dfDIMouse`: the standard mouse data format, shared by games.
static DIMOUSE_DATAFORMAT: DiDataFormat = DiDataFormat {
    dw_size: std::mem::size_of::<DiDataFormat>() as u32,
    dw_obj_size: std::mem::size_of::<DiObjectDataFormat>() as u32,
    dw_flags: 0,
    dw_data_size: std::mem::size_of::<DiMouseState>() as u32,
    dw_num_objs: 4,
    rgodf: &DIMOUSE_OBJECTS as *const DiObjectDataFormat,
};

type RealCreateAW = unsafe extern "system" fn(HINSTANCE, u32, *mut *mut c_void, *mut c_void) -> i32;
type RealCreateEx = unsafe extern "system" fn(HINSTANCE, u32, *const GUID, *mut *mut c_void, *mut c_void) -> i32;

// Resolved once by `init()` (lazy, process-lifetime).
static mut REAL_A: Option<RealCreateAW> = None;
static mut REAL_W: Option<RealCreateAW> = None;
static mut REAL_EX: Option<RealCreateEx> = None;
static mut REAL_8: Option<RealCreateEx> = None;

/// Cast a `FARPROC` returned by `GetProcAddress` into a typed function pointer.
unsafe fn to_fn<T>(p: FARPROC) -> Option<T> {
    p.map(|f| std::mem::transmute_copy(&f))
}

/// Lazily load the real system dinput dlls once and resolve every entry point.
pub fn init() {
    unsafe {
        let dinput = LoadLibraryW(PCWSTR::from_raw(windows::core::w!("C:\\Windows\\System32\\dinput.dll").as_ptr()));
        let dinput8 = LoadLibraryW(PCWSTR::from_raw(windows::core::w!("C:\\Windows\\System32\\dinput8.dll").as_ptr()));

        match dinput {
            Ok(m) => {
                REAL_A = to_fn(GetProcAddress(m, PCSTR::from_raw(c"DirectInputCreateA".as_ptr().cast())));
                REAL_W = to_fn(GetProcAddress(m, PCSTR::from_raw(c"DirectInputCreateW".as_ptr().cast())));
                REAL_EX = to_fn(GetProcAddress(m, PCSTR::from_raw(c"DirectInputCreateEx".as_ptr().cast())));
            }
            Err(e) => {
                crate::dd_log!("dinput::init: failed to load system32\\dinput.dll: {:?}", e);
            }
        }
        match dinput8 {
            Ok(m) => {
                REAL_8 = to_fn(GetProcAddress(m, PCSTR::from_raw(c"DirectInput8Create".as_ptr().cast())));
            }
            Err(e) => {
                crate::dd_log!("dinput::init: failed to load system32\\dinput8.dll: {:?}", e);
            }
        }

        let (a, w, ex, s8) = (
            std::ptr::addr_of!(REAL_A).read().is_some(),
            std::ptr::addr_of!(REAL_W).read().is_some(),
            std::ptr::addr_of!(REAL_EX).read().is_some(),
            std::ptr::addr_of!(REAL_8).read().is_some(),
        );
        crate::dd_log!("dinput::init: A={} W={} Ex={} 8={}", a, w, ex, s8);
    }
}

/// Whether the given module name is one of the DirectInput dlls we wrap.
pub fn wanted(fname: &str) -> bool {
    let low = fname.to_ascii_lowercase();
    low == "dinput.dll" || low == "dinput8.dll"
}

// ---------------------------------------------------------------------------
// Wrapped mouse-device vtable.
//
// We hand the game a boxed structure whose vtable forwards every method to the
// real device but swaps in our own `GetDeviceState`/`GetDeviceData` so the
// reported axes are adjmouse-scaled. AddRef/Release forward to the real device
// so lifetime is owned by the game. The canonical IDirectInputDevice vtable
// (identical for A/W/2/7 right through the common entries) has:
//   0 QueryInterface, 1 AddRef, 2 Release, 3 GetCapabilities, 4 EnumObjects,
//   5 GetProperty, 6 SetProperty, 7 Acquire, 8 Unacquire, 9 GetDeviceState,
//   10 GetDeviceData, 11 SetDataFormat, 12 SetEventNotification,
//   13 SetCooperativeLevel, 14 GetObjectInfo, 15 GetDeviceInfo, ...
// We patch indexes 9 and 10; DirectInputDevice8 keeps the same head offsets.
// ---------------------------------------------------------------------------

const DEVICE_VTABLE_ENTRIES: usize = 29;

#[repr(C)]
struct WrappedDevice {
    vtbl: *const *const c_void,
    table: [*const c_void; DEVICE_VTABLE_ENTRIES],
    real: *mut c_void,
    real_vtbl: *const *const c_void,
}

type EnumObjectsCb = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;

/// Generate a forwarding stub: translate `this` (our wrapper) to the real
/// device and call the real vtable entry at `$idx` with the same arguments.
macro_rules! fwd {
    ($name:ident, $idx:expr, [$($p:ident : $pt:ty),*], $r:ty) => {
        unsafe extern "system" fn $name(this: *mut c_void, $($p: $pt),*) -> $r {
            unsafe {
                let w = &*(this as *const WrappedDevice);
                let f = *(w.real_vtbl.add($idx) as *const unsafe extern "system" fn(*mut c_void, $($pt),*) -> $r);
                f(w.real, $($p),*)
            }
        }
    };
}

fwd!(stub_qi, 0, [riid: *const GUID, out: *mut *mut c_void], i32);
fwd!(stub_addref, 1, [], u32);
fwd!(stub_release, 2, [], u32);
fwd!(stub_caps, 3, [caps: *mut c_void], i32);
fwd!(stub_enumobj, 4, [cb: EnumObjectsCb, pv: *mut c_void, fl: u32], i32);
fwd!(stub_getprop, 5, [rguid: *const GUID, p: *mut c_void], i32);
fwd!(stub_setprop, 6, [rguid: *const GUID, p: *const c_void], i32);
fwd!(stub_acquire, 7, [], i32);
fwd!(stub_unacquire, 8, [], i32);
fwd!(stub_setdataformat, 11, [fmt: *const DiDataFormat], i32);
fwd!(stub_setevent, 12, [ev: HANDLE], i32);
fwd!(stub_setcoop, 13, [hwnd: HWND, fl: u32], i32);
fwd!(stub_getobjinfo, 14, [info: *mut c_void, which: u32, fl: u32], i32);
fwd!(stub_getdevinfo, 15, [info: *mut c_void], i32);
fwd!(stub_setactionmap, 16, [fmt: *const c_void, user: *const c_void, fl: u32], i32);
fwd!(stub_setactionmap_di, 17, [fmt: *const c_void, user: *const c_void, fl: u32], i32);
fwd!(stub_setactionmap_p, 18, [fmt: *const c_void, user: *const c_void, fl: u32], i32);
fwd!(stub_setactionmap_phx, 19, [fmt: *const c_void, user: *const c_void, fl: u32], i32);
fwd!(stub_getffstate, 20, [out: *mut u32], i32);
fwd!(stub_sendffcmd, 21, [fl: u32], i32);
fwd!(stub_geteffectstatus, 22, [effect: u32, out: *mut u32], i32);
fwd!(stub_stopeffect, 23, [], i32);
fwd!(stub_getffiface, 24, [rguid: *const GUID, out: *mut *mut c_void], i32);
fwd!(stub_img, 25, [info: *mut c_void], i32);
// DirectInputDevice8-only tail entries (rarely exercised; forward generically).
fwd!(stub_x1, 26, [info: *mut c_void], i32);
fwd!(stub_x2, 27, [a: u32], i32);
fwd!(stub_x3, 28, [a: u32, b: u32], i32);

/// Intercepted `GetDeviceState`: forward to the real device, then scale the
/// relative mouse axes through adjmouse.
unsafe extern "system" fn stub_getdevicestate(this: *mut c_void, cb: u32, data: *mut c_void) -> i32 {
    unsafe {
        let w = &*(this as *const WrappedDevice);
        let f = *(w.real_vtbl.add(9) as *const unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32);
        let hr = f(w.real, cb, data);
        if hr >= 0 && !data.is_null() && cb >= std::mem::size_of::<DiMouseState>() as u32 {
            let s = &mut *(data as *mut DiMouseState);
            let (x, y) = crate::mouse::mapped_delta(s.l_x, s.l_y);
            s.l_x = x;
            s.l_y = y;
        }
        hr
    }
}

/// Intercepted `GetDeviceData`: forward to the real device, then scale the
/// relative mouse object data (X/Y offsets) through adjmouse.
unsafe extern "system" fn stub_getdevicedata(
    this: *mut c_void,
    cb_obj: u32,
    rgdod: *mut c_void,
    inout: *mut u32,
    flags: u32,
) -> i32 {
    unsafe {
        let w = &*(this as *const WrappedDevice);
        let f = *(w.real_vtbl.add(10)
            as *const unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> i32);
        let hr = f(w.real, cb_obj, rgdod, inout, flags);
        if hr >= 0 {
            let cnt = inout.as_ref().map(|c| *c as usize).unwrap_or(0);
            if cnt > 0 && !rgdod.is_null() && cb_obj >= std::mem::size_of::<DiDeviceObjectData>() as u32 {
                for i in 0..cnt {
                    let d = &mut *((rgdod as *mut DiDeviceObjectData).add(i));
                    if d.dw_ofs == DIMOFS_X {
                        let (x, _) = crate::mouse::mapped_delta(d.dw_data as i32, 0);
                        d.dw_data = x as u32;
                    } else if d.dw_ofs == DIMOFS_Y {
                        let (_, y) = crate::mouse::mapped_delta(0, d.dw_data as i32);
                        d.dw_data = y as u32;
                    }
                }
            }
        }
        hr
    }
}

/// Box a wrapper device whose vtable forwards to the real device but patches
/// the GetDeviceState/GetDeviceData entries (indexes 9 and 10).
unsafe fn build_wrapper(real: *mut c_void, real_vtbl: *const *const c_void) -> *mut c_void {
    const N: usize = DEVICE_VTABLE_ENTRIES;
    let table: [*const c_void; N] = core::array::from_fn(|i| match i {
        0 => stub_qi as *const c_void,
        1 => stub_addref as *const c_void,
        2 => stub_release as *const c_void,
        3 => stub_caps as *const c_void,
        4 => stub_enumobj as *const c_void,
        5 => stub_getprop as *const c_void,
        6 => stub_setprop as *const c_void,
        7 => stub_acquire as *const c_void,
        8 => stub_unacquire as *const c_void,
        9 => stub_getdevicestate as *const c_void,
        10 => stub_getdevicedata as *const c_void,
        11 => stub_setdataformat as *const c_void,
        12 => stub_setevent as *const c_void,
        13 => stub_setcoop as *const c_void,
        14 => stub_getobjinfo as *const c_void,
        15 => stub_getdevinfo as *const c_void,
        16 => stub_setactionmap as *const c_void,
        17 => stub_setactionmap_di as *const c_void,
        18 => stub_setactionmap_p as *const c_void,
        19 => stub_setactionmap_phx as *const c_void,
        20 => stub_getffstate as *const c_void,
        21 => stub_sendffcmd as *const c_void,
        22 => stub_geteffectstatus as *const c_void,
        23 => stub_stopeffect as *const c_void,
        24 => stub_getffiface as *const c_void,
        25 => stub_img as *const c_void,
        26 => stub_x1 as *const c_void,
        27 => stub_x2 as *const c_void,
        28 => stub_x3 as *const c_void,
        _ => std::ptr::null(),
    });
    let mut w = Box::new(WrappedDevice { vtbl: std::ptr::null(), table, real, real_vtbl });
    let t = w.table.as_mut_ptr() as *const *const c_void;
    w.vtbl = t;
    Box::into_raw(w) as *mut c_void
}

/// Create the real mouse device on the given real DirectInput object, format /
/// configure it, and return a boxed wrapper whose vtable delivers scaled data.
unsafe fn hook_mouse_device(di_obj: *mut c_void) -> *mut c_void {
    unsafe {
        let di_vtbl: *const *const c_void = (*(di_obj as *const *const c_void)) as *const *const c_void;
        let create_device = *(di_vtbl.add(3)
            as *const unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void, *mut c_void) -> i32);
        let mut raw_dev: *mut c_void = std::ptr::null_mut();
        let hr = create_device(di_obj, &GUID_SYSMOUSE, &mut raw_dev, std::ptr::null_mut());
        if hr < 0 || raw_dev.is_null() {
            crate::dd_log!("dinput::hook_mouse_device: CreateDevice failed hr={:#x}", hr as u32);
            return std::ptr::null_mut();
        }

        let dev_vtbl: *const *const c_void = (*(raw_dev as *const *const c_void)) as *const *const c_void;
        let hwnd = crate::state::state().lock().unwrap().hwnd;

        let set_df = *(dev_vtbl.add(11) as *const unsafe extern "system" fn(*mut c_void, *const DiDataFormat) -> i32);
        let _ = set_df(raw_dev, &DIMOUSE_DATAFORMAT);

        let set_coop = *(dev_vtbl.add(13) as *const unsafe extern "system" fn(*mut c_void, HWND, u32) -> i32);
        let _ = set_coop(raw_dev, hwnd, DISCL_NONEXCLUSIVE | DISCL_FOREGROUND);

        build_wrapper(raw_dev, dev_vtbl)
    }
}

/// Common wrapped-path for the A/W entry points (no IID on their prototype).
unsafe fn create_wrapped_aw(
    real: RealCreateAW,
    instance: HINSTANCE,
    version: u32,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    unsafe {
        let mut di_obj: *mut c_void = std::ptr::null_mut();
        let hr = real(instance, version, &mut di_obj, punk_outer);
        if hr < 0 || di_obj.is_null() {
            return if hr < 0 { hr } else { DIERR_FALLBACK };
        }
        let wrapped = hook_mouse_device(di_obj);
        if wrapped.is_null() {
            return DIERR_FALLBACK;
        }
        *ppv_out = wrapped;
        hr
    }
}

/// Common wrapped-path for the Ex / DirectInput8 entry points (IID on proto).
unsafe fn create_wrapped_ex(
    real: RealCreateEx,
    instance: HINSTANCE,
    version: u32,
    riid: *const GUID,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    unsafe {
        let mut di_obj: *mut c_void = std::ptr::null_mut();
        let hr = real(instance, version, riid, &mut di_obj, punk_outer);
        if hr < 0 || di_obj.is_null() {
            return if hr < 0 { hr } else { DIERR_FALLBACK };
        }
        let wrapped = hook_mouse_device(di_obj);
        if wrapped.is_null() {
            return DIERR_FALLBACK;
        }
        *ppv_out = wrapped;
        hr
    }
}

/// `extern "system"` (stdcall) entry points referenced by `hook.rs`.
pub unsafe extern "system" fn direct_input_create_a(
    instance: windows::Win32::Foundation::HINSTANCE,
    version: u32,
    riid_const: *const GUID,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    let _ = (riid_const, punk_outer);
    unsafe {
        let Some(real) = REAL_A else {
            crate::dd_log!("DirectInputCreateA: real fn unavailable");
            return DIERR_FALLBACK;
        };
        let (no_hook, adj) = {
            let st = crate::state::state().lock().unwrap();
            (st.no_dinput_hook, st.adjmouse)
        };
        if no_hook || !adj {
            return real(instance, version, ppv_out, punk_outer);
        }
        create_wrapped_aw(real, instance, version, ppv_out, punk_outer)
    }
}

pub unsafe extern "system" fn direct_input_create_w(
    instance: windows::Win32::Foundation::HINSTANCE,
    version: u32,
    riid_const: *const GUID,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    let _ = (riid_const, punk_outer);
    unsafe {
        let Some(real) = REAL_W else {
            crate::dd_log!("DirectInputCreateW: real fn unavailable");
            return DIERR_FALLBACK;
        };
        let (no_hook, adj) = {
            let st = crate::state::state().lock().unwrap();
            (st.no_dinput_hook, st.adjmouse)
        };
        if no_hook || !adj {
            return real(instance, version, ppv_out, punk_outer);
        }
        create_wrapped_aw(real, instance, version, ppv_out, punk_outer)
    }
}

/// `DirectInputCreateEx` (used by many games).
pub unsafe extern "system" fn direct_input_create_ex(
    instance: windows::Win32::Foundation::HINSTANCE,
    version: u32,
    riid_const: *const GUID,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    unsafe {
        let Some(real) = REAL_EX else {
            crate::dd_log!("DirectInputCreateEx: real fn unavailable");
            return DIERR_FALLBACK;
        };
        let (no_hook, adj) = {
            let st = crate::state::state().lock().unwrap();
            (st.no_dinput_hook, st.adjmouse)
        };
        if no_hook || !adj {
            return real(instance, version, riid_const, ppv_out, punk_outer);
        }
        create_wrapped_ex(real, instance, version, riid_const, ppv_out, punk_outer)
    }
}

/// `DirectInput8Create` (the DInput8 API).
pub unsafe extern "system" fn direct_input8_create(
    instance: windows::Win32::Foundation::HINSTANCE,
    version: u32,
    riid_const: *const GUID,
    ppv_out: *mut *mut c_void,
    punk_outer: *mut c_void,
) -> i32 {
    unsafe {
        let Some(real) = REAL_8 else {
            crate::dd_log!("DirectInput8Create: real fn unavailable");
            return DIERR_FALLBACK;
        };
        let (no_hook, adj) = {
            let st = crate::state::state().lock().unwrap();
            (st.no_dinput_hook, st.adjmouse)
        };
        if no_hook || !adj {
            return real(instance, version, riid_const, ppv_out, punk_outer);
        }
        create_wrapped_ex(real, instance, version, riid_const, ppv_out, punk_outer)
    }
}
