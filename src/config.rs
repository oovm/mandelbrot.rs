//! Runtime configuration loaded from `ddraw.ini`.
//!
//! Ports `Settings.c` from ts-ddraw. In addition to the main ini, a
//! per-executable override file may be loaded: a file named `<game_exe>.ini`
//! in the same directory as the DLL (e.g. `game.exe` → `game.ini`) is read
//! after the main ini and re-runs the same settings on top of the current
//! values, so a value in the per-exe file wins over the global ini.

use std::ffi::CString;
use std::path::Path;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameA, GetModuleHandleExA,
};
use windows::Win32::System::Threading::{GetProcessAffinityMask, SetProcessAffinityMask};
use windows::Win32::System::WindowsProgramming::{
    GetPrivateProfileIntA, GetPrivateProfileStringA, WritePrivateProfileStringA,
};
use windows::core::PCSTR;

use crate::state::{
    DMDFO_CENTER, DMDFO_DEFAULT, DMDFO_STRETCH, RENDERER_D3D9, RENDERER_GDI, RENDERER_OPENGL, RENDERER_OPENGL_CORE,
    state,
};

const SETTINGS_SECTION: &str = "ddraw";

fn section() -> CString {
    CString::new(SETTINGS_SECTION).unwrap()
}

/// 返回实际要读取的配置文件路径：优先 `rs-ddraw.ini`，不存在时回退到
/// `ddraw.ini`，两者都不存在则返回不存在的文件名（Windows INI API 会直接
/// 返回各项的默认值）。
///
/// 配置在 DLL 所在目录查找（与日志文件路径一致），因为游戏进程的 CWD
/// 往往不是游戏目录。
fn path() -> CString {
    let candidates = ["rs-ddraw.ini", "ddraw.ini"];
    if let Some(dir) = module_dir() {
        for name in candidates {
            let full = format!("{}\\{}", dir.trim_end_matches('\\'), name);
            if Path::new(&full).exists() {
                return CString::new(full).unwrap();
            }
        }
    }
    for name in candidates {
        if Path::new(name).exists() {
            return CString::new(name).unwrap();
        }
    }
    CString::new("rs-ddraw.ini").unwrap()
}

/// Per-executable override file: `<game_exe_base>.ini` next to the DLL.
/// Returns `None` when the exe base cannot be determined or no such file
/// exists, so `read_section` is invoked only with an existing file path.
fn override_path() -> Option<CString> {
    let mut buf = [0u8; 1024];
    let n = unsafe { GetModuleFileNameA(None, &mut buf) } as usize;
    if n == 0 {
        return None;
    }
    let exe = std::path::PathBuf::from(String::from_utf8_lossy(&buf[..n]).into_owned());
    let base = exe.file_stem()?.to_string_lossy().to_string().to_lowercase();
    let file = format!("{}.ini", base);
    let dir = module_dir()?;
    let full = format!("{}\\{}", dir.trim_end_matches('\\'), file);
    if Path::new(&full).exists() { CString::new(full).ok() } else { None }
}

/// 取得本 DLL 所在目录（通过模块地址反查模块句柄），用于定位配置文件。
fn module_dir() -> Option<String> {
    unsafe {
        let mut h = HMODULE::default();
        // 使用本函数地址反查自身模块句柄。
        let addr = module_dir as *const u8;
        if GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCSTR(addr), &mut h).is_ok() {
            let mut buf = [0u8; 1024];
            let n = GetModuleFileNameA(Some(h), &mut buf);
            if n > 0 {
                let path = String::from_utf8_lossy(&buf[..n as usize]);
                return std::path::Path::new(path.as_ref()).parent().map(|p| p.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn pcstr(c: &CString) -> PCSTR {
    PCSTR(c.as_c_str().as_ptr() as *const u8)
}

fn get_string(path: &CString, key: &str, default: &str) -> String {
    let key = CString::new(key).unwrap();
    let def = CString::new(default).unwrap();
    let mut buf = vec![0u8; 256];
    let n = unsafe {
        GetPrivateProfileStringA(pcstr(&section()), pcstr(&key), pcstr(&def), Some(buf.as_mut_slice()), pcstr(path))
    };
    if n == 0 {
        return default.to_string();
    }
    let end = (n as usize).min(buf.len());
    let len = buf[..end].iter().position(|&c| c == 0).unwrap_or(end);
    String::from_utf8_lossy(&buf[..len]).to_string()
}

fn get_bool(path: &CString, key: &str, default: bool) -> bool {
    let v = get_string(path, key, if default { "Yes" } else { "No" });
    let v = v.trim();
    v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true") || v == "1"
}

fn get_int(path: &CString, key: &str, default: i32) -> i32 {
    let key = CString::new(key).unwrap();
    unsafe { GetPrivateProfileIntA(pcstr(&section()), pcstr(&key), default, pcstr(path)) as i32 }
}

fn eq_ignore(value: &str, target: &str) -> bool {
    value.eq_ignore_ascii_case(target)
}

/// File name (without directory) used for the log file. Reads only the `Log`
/// key of the main ini without touching (locking/creating) the global state,
/// so [`crate::log::init`] can call it inside `DllMain` before the game runs.
pub fn log_file_name() -> String {
    let name = get_string(&path(), "Log", "rs-ddraw.log").trim().to_string();
    if name.is_empty() { "rs-ddraw.log".to_string() } else { name }
}

/// Parse the `Filter` value into the filter index (0..=4).
fn parse_filter(value: &str) -> i32 {
    let v = value.trim();
    if let Ok(n) = v.parse::<i32>() {
        return n.clamp(0, 4);
    }
    if eq_ignore(v, "nearest") {
        0
    } else if eq_ignore(v, "bilinear") {
        1
    } else if eq_ignore(v, "catmull") || eq_ignore(v, "catmull-rom") {
        2
    } else if eq_ignore(v, "lanczos") {
        3
    } else if eq_ignore(v, "xbr") {
        4
    } else {
        0
    }
}

/// Parse a "major.minor" version string (e.g. "6.1", "5.1", "10.0") into
/// `(major, minor)`, ignoring anything after the first two dot-separated
/// integers. Returns `(0, 0)` when not parseable.
fn parse_version(value: &str) -> (u32, u32) {
    let mut parts = value.trim().split('.');
    let maj = parts.next().and_then(|p| p.parse::<u32>().ok());
    let min = parts.next().and_then(|p| p.parse::<u32>().ok());
    match (maj, min) {
        (Some(m), Some(n)) => (m, n),
        _ => (0, 0),
    }
}

/// Read every config key from one ini file into `s`. Called for the main ini
/// and again for the per-exe override file (later wins).
fn read_section(s: &mut crate::state::DDrawState, path: &CString) {
    s.maintain_aspect_ratio = get_bool(path, "MaintainAspectRatio", s.maintain_aspect_ratio);
    s.windowboxing = get_bool(path, "Windowboxing", s.windowboxing);
    s.stretch_to_fullscreen = get_bool(path, "StretchToFullscreen", s.stretch_to_fullscreen);
    s.stretch_to_width = get_int(path, "StretchToWidth", s.stretch_to_width);
    s.stretch_to_height = get_int(path, "StretchToHeight", s.stretch_to_height);
    s.draw_fps = get_int(path, "DrawFPS", s.draw_fps as i32) != 0;

    let (r, auto) = get_renderer(path, "Renderer", "auto");
    s.renderer = r;
    s.auto_renderer = auto;

    s.primary_surface2tex = get_bool(path, "PrimarySurface2Tex", s.primary_surface2tex);
    s.gl_finish = get_bool(path, "GlFinish", s.gl_finish);
    s.convert_on_gpu = get_bool(path, "ConvertOnGPU", s.convert_on_gpu);

    let tfps = get_int(path, "TargetFPS", 0);
    if tfps > 0 {
        s.target_fps = tfps as f64;
        s.target_frame_len = 1000.0 / s.target_fps;
    } else {
        s.target_fps = 0.0;
        s.target_frame_len = 16.0;
    }

    if get_bool(path, "VSync", false) {
        s.swap_interval = 1;
    } else {
        s.swap_interval = 0;
    }

    apply_affinity(s, get_bool(path, "SingleProcAffinity", true) && get_bool(path, "singlecpu", true));

    s.edge_timeout_ms = get_int(path, "MonitorEdgeTimer", s.edge_timeout_ms);

    crate::state::RGB555.store(get_bool(path, "rgb555", false), std::sync::atomic::Ordering::Relaxed);

    s.gl_fence_sync = get_bool(path, "GlFenceSync", s.gl_fence_sync);

    s.fixed_output = get_fixed_output(path, "FixedOutput", "stretch");

    s.filter = parse_filter(&get_string(path, "Filter", "nearest"));

    let shot_dir = get_string(path, "ScreenshotDir", "").trim().to_string();
    s.screenshot_dir = if shot_dir.is_empty() { None } else { Some(shot_dir) };

    s.border = get_bool(path, "Border", s.border);
    s.resizable = get_bool(path, "Resizable", s.resizable);
    // center_window is an int mode (0 never, 1 auto, 2 always); the legacy
    // boolean CenterWindow key maps to mode 1.
    let cw = get_int(path, "center_window", -1);
    s.center_window = if cw >= 0 {
        cw.clamp(0, 2)
    } else if get_bool(path, "CenterWindow", false) {
        1
    } else {
        s.center_window
    };

    let fw = get_int(path, "FakeWidth", 0);
    let fh = get_int(path, "FakeHeight", 0);
    s.fake_size = if fw > 0 && fh > 0 { (fw, fh) } else { (0, 0) };

    s.fake_version = parse_version(&get_string(path, "WinVersion", ""));

    read_extended(s, path);
}

/// Parse the second tier of cnc-ddraw compatible keys (window/game hacks,
/// display, mouse, limiter, hotkeys, GL shader settings, resolution override).
fn read_extended(s: &mut crate::state::DDrawState, path: &CString) {
    // ---- window / game compatibility ----
    s.noactivateapp = get_bool(path, "noactivateapp", s.noactivateapp);
    s.fix_not_responding = get_bool(path, "fix_not_responding", s.fix_not_responding);
    s.no_compat_warning = get_bool(path, "no_compat_warning", s.no_compat_warning);
    s.game_handles_close = get_bool(path, "game_handles_close", s.game_handles_close);
    s.terminate_process = get_bool(path, "terminate_process", s.terminate_process);
    s.remove_menu = get_bool(path, "remove_menu", s.remove_menu);
    s.fix_alt_key_stuck = get_bool(path, "fix_alt_key_stuck", s.fix_alt_key_stuck);
    s.fixchilds = get_int(path, "fixchilds", s.fixchilds).clamp(0, 4);
    s.lock_surfaces = get_bool(path, "lock_surfaces", s.lock_surfaces);
    s.flipclear = get_bool(path, "flipclear", s.flipclear);
    s.tshack = get_bool(path, "tshack", s.tshack);
    s.vhack = get_bool(path, "vhack", s.vhack);
    s.devmode = get_bool(path, "devmode", s.devmode);
    s.limit_gdi_handles = get_bool(path, "limit_gdi_handles", s.limit_gdi_handles);
    s.guard_lines = get_int(path, "guard_lines", s.guard_lines);
    s.min_font_size = get_int(path, "min_font_size", s.min_font_size);
    s.anti_aliased_fonts_min_size = get_int(path, "anti_aliased_fonts_min_size", s.anti_aliased_fonts_min_size);
    s.pos_x = get_int(path, "posx", s.pos_x);
    s.pos_y = get_int(path, "posy", s.pos_y);
    s.savesettings = get_int(path, "savesettings", s.savesettings).clamp(0, 2);
    s.nonexclusive = get_bool(path, "nonexclusive", s.nonexclusive);

    // ---- display / resolution ----
    s.res_width = get_int(path, "width", s.res_width).max(0);
    s.res_height = get_int(path, "height", s.res_height).max(0);
    s.refresh_rate = get_int(path, "refresh_rate", s.refresh_rate).max(0);
    s.resolutions = get_int(path, "resolutions", s.resolutions).clamp(0, 2);
    s.max_resolutions = get_int(path, "max_resolutions", s.max_resolutions).max(0);
    s.inject_resolution = get_string(path, "inject_resolution", "").trim().to_string();
    s.fake_mode = get_string(path, "fake_mode", "").trim().to_string();

    // ---- mouse / input ----
    s.adjmouse = get_bool(path, "adjmouse", s.adjmouse);
    s.lock_mouse_top_left = get_bool(path, "lock_mouse_top_left", s.lock_mouse_top_left);
    s.center_cursor_fix = get_bool(path, "center_cursor_fix", s.center_cursor_fix);
    s.hook_peekmessage = get_bool(path, "hook_peekmessage", s.hook_peekmessage);
    s.no_dinput_hook = get_bool(path, "no_dinput_hook", s.no_dinput_hook);

    // ---- fps / speed limiter ----
    s.limiter_type = parse_limiter_type(&get_string(path, "limiter_type", ""), s.limiter_type);
    s.maxgameticks = get_int(path, "maxgameticks", s.maxgameticks);
    s.minfps = get_int(path, "minfps", s.minfps);
    s.maxfps = get_int(path, "maxfps", s.maxfps);
    if s.maxfps > 0 {
        // cnc-ddraw maxfps is a cap; map it onto our target-fps pacing.
        s.target_fps = s.maxfps as f64;
        s.target_frame_len = 1000.0 / s.target_fps;
    }

    // ---- hotkeys ----
    s.keytogglefullscreen = get_int(path, "keytogglefullscreen", s.keytogglefullscreen);
    s.keytogglefullscreen2 = get_int(path, "keytogglefullscreen2", s.keytogglefullscreen2);
    s.keytogglemaximize = get_int(path, "keytogglemaximize", s.keytogglemaximize);
    s.keytogglemaximize2 = get_int(path, "keytogglemaximize2", s.keytogglemaximize2);
    s.keyunlockcursor1 = get_int(path, "keyunlockcursor1", s.keyunlockcursor1);
    s.keyunlockcursor2 = get_int(path, "keyunlockcursor2", s.keyunlockcursor2);
    s.keyscreenshot = get_int(path, "keyscreenshot", s.keyscreenshot);
    s.keyconfig = get_int(path, "keyconfig", s.keyconfig);
    s.toggle_borderless = get_bool(path, "toggle_borderless", s.toggle_borderless);
    s.toggle_upscaled = get_bool(path, "toggle_upscaled", s.toggle_upscaled);

    // ---- renderer / GL ----
    s.shader = get_string(path, "shader", "catmull-rom-bilinear.glsl").trim().to_string();
    s.shaderpath = get_string(path, "shaderpath", "").trim().to_string();
    s.shaderpath_pass1 = get_string(path, "shaderpath.pass1", "").trim().to_string();

    // A fresh gamma ramp is installed later via IDirectDrawGammaControl.
    s.gamma_ramp = None;
}

fn parse_limiter_type(value: &str, default: i32) -> i32 {
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "auto" => 0,
        "testcooperativelevel" | "1" => 1,
        "bltfast" | "2" => 2,
        "unlock" | "3" => 3,
        "peekmessage" | "4" => 4,
        "" => default,
        _ => default,
    }
}

/// Load configuration from `ddraw.ini` (plus the per-exe override file) into
/// the global state.
pub unsafe fn load() {
    let mut s = state().lock().unwrap();

    read_section(&mut s, &path());

    if let Some(ov) = override_path() {
        crate::dd_log!("override ini found: {}", ov.to_string_lossy());
        read_section(&mut s, &ov);
    }

    drop(s);

    // Environment variable overrides (mirrors ts-ddraw's DDRAW_* vars).
    let mut s = state().lock().unwrap();
    if let Ok(v) = std::env::var("DDRAW_DRAW_FPS")
        && (v.trim().eq_ignore_ascii_case("yes") || v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
    {
        s.draw_fps = true;
    }
    if let Ok(v) = std::env::var("DDRAW_TARGET_FPS")
        && let Ok(n) = v.trim().parse::<i32>()
        && n > 0
    {
        s.target_fps = n as f64;
        s.target_frame_len = 1000.0 / s.target_fps;
    }

    crate::fps_limiter::init(s.maxgameticks, s.limiter_type, s.minfps, s.refresh_rate);
}

fn apply_affinity(s: &mut crate::state::DDrawState, single: bool) {
    let proc = unsafe { windows::Win32::System::Threading::GetCurrentProcess() };
    let mut sys_aff: usize = 0;
    let mut proc_aff: usize = 0;
    unsafe {
        if GetProcessAffinityMask(proc, &mut proc_aff, &mut sys_aff).is_ok() {
            s.proc_affinity = proc_aff;
            s.system_affinity = sys_aff;
        }
    }
    if single {
        unsafe {
            let _ = SetProcessAffinityMask(proc, 1);
        }
        s.proc_affinity = 0;
        s.system_affinity = 0;
    }
}

fn get_renderer(path: &CString, key: &str, default: &str) -> (i32, bool) {
    let value = get_string(path, key, default);
    let value = value.trim();
    if eq_ignore(value, "opengl") {
        (RENDERER_OPENGL, false)
    } else if eq_ignore(value, "openglcore") || eq_ignore(value, "opengl3") {
        (RENDERER_OPENGL_CORE, false)
    } else if eq_ignore(value, "gdi") {
        (RENDERER_GDI, false)
    } else if eq_ignore(value, "d3d9") {
        (RENDERER_D3D9, false)
    } else if eq_ignore(value, "auto") {
        match default {
            "d3d9" => (RENDERER_D3D9, true),
            "opengl" => (RENDERER_OPENGL, true),
            _ => (RENDERER_D3D9, true),
        }
    } else {
        (RENDERER_D3D9, false)
    }
}

fn get_fixed_output(path: &CString, key: &str, default: &str) -> u32 {
    let value = get_string(path, key, default);
    let value = value.trim();
    if eq_ignore(value, "default") {
        DMDFO_DEFAULT
    } else if eq_ignore(value, "center") {
        DMDFO_CENTER
    } else if eq_ignore(value, "stretch") {
        DMDFO_STRETCH
    } else {
        DMDFO_DEFAULT
    }
}

/// Persist a single `[ddraw]` key back into the active ini file (used by
/// `savesettings` window-state persistence and hotkey toggles). Mirrors
/// cnc-ddraw's `cfg_save_setting`.
pub fn save_setting(key: &str, value: &str) {
    let key = CString::new(key).unwrap();
    let value = CString::new(value).unwrap();
    unsafe {
        let _ = WritePrivateProfileStringA(pcstr(&section()), pcstr(&key), pcstr(&value), pcstr(&path()));
    }
}
