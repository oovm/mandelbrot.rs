//! Media / video codec support (ports `indeo.c` plus the media side of
//! `winapi_hooks.c`).
//!
//! - **Indeo5 / Cinepak registration**: writes the `vidc.iv31`/`iv32`/`iv41`/
//!   `iv50` entries into `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Drivers32`
//!   (the game loads FMV with these codecs). The codec DLLs themselves
//!   (`ir32_32.dll`, `ir41_32.ax`, `ir50_32.dll`) must be present on disk.
//! - **Media routing**: `hook.rs` consults [`is_media_clsid`] when the game
//!   CoCreates a DirectShow filter graph, and forwards `mciSendCommandA` /
//!   `AVIStreamGetFrameOpen` to the real implementations.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyW,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{GUID, PCWSTR, w};

/// `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Drivers32` — where VfW
/// video codecs (DECODER DLLs) are enumerated from.
const DRIVERS32_KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Drivers32");

/// Registry value name -> codec DLL, byte-for-byte as cnc-ddraw's `indeo.c`.
const CODEC_ENTRIES: [(&str, &str); 4] = [
    ("vidc.iv31", "ir32_32.dll"),
    ("vidc.iv32", "ir32_32.dll"),
    ("vidc.iv41", "ir41_32.ax"),
    ("vidc.iv50", "ir50_32.dll"),
];

const CLSID_FILTER_GRAPH: GUID = GUID::from_u128(0xE436EBB3_524F_11CE_9F53_0020AF0BA770);
const CLSID_FILTER_GRAPH_NO_THREAD: GUID = GUID::from_u128(0xE436EBB5_524F_11CE_9F53_0020AF0BA770);

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Install (and register) the Indeo codec registry entries.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !is_enabled() {
        crate::dd_log!("media: codec registration disabled");
        return;
    }
    register_codecs();
    crate::dd_log!("media: indeo codec registration complete");
}

/// True when codec registration is allowed. cnc-ddraw calls `indeo_enable()`
/// unconditionally from `DLL_PROCESS_ATTACH` (no `video_codec`/`no_codecs`
/// config key exists), so registration is always on.
fn is_enabled() -> bool {
    true
}

/// Write the `Drivers32` codec entries. Best effort: no-op if the registry
/// write fails (often simply already-registered on old systems).
pub fn register_codecs() {
    let Some(key) = open_drivers32() else {
        crate::dd_log!("media: unable to open/create Drivers32 key");
        return;
    };
    for (name, dll) in CODEC_ENTRIES {
        set_codec_value(key, name, dll);
    }
    crate::dd_log!("media: wrote {} codec registry entries", CODEC_ENTRIES.len());
    unsafe {
        let _ = RegCloseKey(key);
    }
}

/// Remove the codec registry entries (only the ones we registered).
pub fn unregister_codecs() {
    let Some(key) = open_drivers32() else {
        return;
    };
    for (name, _) in CODEC_ENTRIES {
        if codec_value_matches(key, name) {
            let name_w = wide(name);
            unsafe {
                let _ = RegDeleteValueW(key, PCWSTR(name_w.as_ptr()));
            }
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
}

/// True when `name` is a codec DLL we intercept on LoadLibrary so the game's
/// `LoadLibrary("ir50_32.dll")` keeps working even when it re-links late.
pub fn wants_codec_dll(name: &str) -> bool {
    is_codec_dll(name)
}

/// True when `name` should be let through / routed by the LoadLibraryA/W hook.
/// Mirrors the codec DLL set (cnc-ddraw's `fake_LoadLibrary` lets everything
/// through with no playback-specific names of its own; `winmm.dll` is not a
/// codec and is not filtered here).
pub fn wants_library(name: &str) -> bool {
    is_codec_dll(name)
}

/// True when the CLSID is a DirectShow/media filter-graph object we should
/// hand back (or block) rather than forwarding to the real COM object.
pub fn is_media_clsid(clsid: &GUID) -> bool {
    *clsid == CLSID_FILTER_GRAPH || *clsid == CLSID_FILTER_GRAPH_NO_THREAD
}

fn is_codec_dll(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "ir32_32.dll" || n == "ir41_32.ax" || n == "ir50_32.dll"
}

/// Open the `Drivers32` key with set+query access, creating it on demand.
/// `RegCreateKeyExW` is gated behind the windows crate's `Win32_Security`
/// feature (for `SECURITY_ATTRIBUTES`), which this project does not enable, so
/// open is done with least privilege and `RegCreateKeyW` covers the
/// create-if-missing case.
fn open_drivers32() -> Option<HKEY> {
    let mut key = HKEY::default();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, DRIVERS32_KEY, Some(0), KEY_SET_VALUE | KEY_QUERY_VALUE, &mut key) };
    if status == ERROR_SUCCESS {
        return Some(key);
    }
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        let created = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, DRIVERS32_KEY, &mut key) };
        if created == ERROR_SUCCESS {
            return Some(key);
        }
    }
    None
}

fn set_codec_value(key: HKEY, name: &str, dll: &str) {
    let name_w = wide(name);
    let data = sz_bytes(dll);
    unsafe {
        let _ = RegSetValueExW(key, PCWSTR(name_w.as_ptr()), Some(0), REG_SZ, Some(&data));
    }
}

/// Query a `Drivers32` value and say whether it names one of our codec DLLs.
/// Used by `unregister_codecs` so a value pointing at some other (user
/// installed) codec is never deleted.
fn codec_value_matches(key: HKEY, name: &str) -> bool {
    let mut ty = REG_VALUE_TYPE(0);
    let mut data = [0u8; 256];
    let mut cb = data.len() as u32;
    let name_w = wide(name);
    let status = unsafe {
        RegQueryValueExW(key, PCWSTR(name_w.as_ptr()), None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut cb))
    };
    if status != ERROR_SUCCESS || ty != REG_SZ {
        return false;
    }
    let raw = &data[..cb as usize];
    let mut text = Vec::with_capacity(raw.len() / 2);
    for i in (0..raw.len()).step_by(2) {
        let u = u16::from_le_bytes([raw[i], raw[i + 1]]);
        if u == 0 {
            break;
        }
        text.push(u);
    }
    is_codec_dll(&String::from_utf16_lossy(&text))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16LE bytes of `s` including the null terminator (REG_SZ payload).
fn sz_bytes(s: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len() * 2 + 2);
    for u in s.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    bytes
}
