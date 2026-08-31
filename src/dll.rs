use crate::dd_log;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleHandleA, GetProcAddress};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::core::{BOOL, PCSTR};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

/// Force-quit hotkey monitor. Runs on its own thread so it still works when
/// the game's main thread is deadlocked (a WndProc hotkey would never fire).
/// Press Ctrl+Shift+Q to terminate the process immediately.
fn start_kill_switch() {
    std::thread::spawn(|| {
        // VK_CONTROL=0x11, VK_SHIFT=0x10, VK_Q=0x51
        loop {
            unsafe {
                if GetAsyncKeyState(0x11) < 0 && GetAsyncKeyState(0x10) < 0 && GetAsyncKeyState(0x51) < 0 {
                    dd_log!("kill-switch (Ctrl+Shift+Q) triggered, forcing exit");
                    std::process::exit(0);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllMain(h_module: HMODULE, dw_reason: u32, _lp_reserved: *const u8) -> i32 {
    match dw_reason {
        DLL_PROCESS_ATTACH => {
            crate::log::init(h_module);
            dd_log!("DllMain(DLL_PROCESS_ATTACH) hModule={:p}", h_module.0);
            set_dpi_aware();
            start_kill_switch();
            crate::debug::rotate_if_needed();
            unsafe {
                crate::debug::install_handler();
            }
            crate::media::init();
            crate::dinput::init();
            unsafe {
                let _ = DisableThreadLibraryCalls(h_module);
            }
        }
        DLL_PROCESS_DETACH => {
            dd_log!("DllMain(DLL_PROCESS_DETACH)");
            crate::overlay::cleanup();
            crate::state::state().lock().unwrap().running.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        _ => {}
    }
    1
}

/// Make the process DPI aware so window coordinates match device pixels.
unsafe fn set_dpi_aware() {
    if let Ok(user32) = GetModuleHandleA(PCSTR::from_raw(c"user32.dll".as_ptr().cast())) {
        let name = PCSTR::from_raw(c"SetProcessDPIAware".as_ptr().cast());
        if let Some(proc) = GetProcAddress(user32, name) {
            let f: extern "system" fn() -> BOOL = std::mem::transmute(proc);
            let _ = f();
        }
    }
}
