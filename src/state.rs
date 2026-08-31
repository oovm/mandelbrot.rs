//! Shared global DirectDraw state.
//!
//! Mirrors the `IDirectDrawImpl` struct from ts-ddraw. All COM objects and the
//! renderer thread reach shared state through [`state()`].

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use windows::Win32::System::Threading::GetCurrentThreadId;

/// A per-thread reentrant mutual exclusion lock.
///
/// This mirrors cnc-ddraw's per-surface critical section (`lock_surfaces`):
/// `Lock` enters it and `Unlock` leaves it, so the game's entire draw into the
/// surface happens while the lock is held, and the renderer thread takes the
/// same lock while uploading — guaranteeing it only ever reads a complete
/// frame. `std::sync::Mutex` is not reentrant, so a game that calls `Blt`
/// while a `Lock` is held would deadlock without this.
pub struct ReentrantLock {
    owner: AtomicU64,
    count: AtomicU32,
    inner: Mutex<()>,
    cv: Condvar,
}

/// RAII guard returned by [`ReentrantLock::lock`]; releases the lock on drop.
pub struct ReentrantGuard<'a> {
    lock: &'a ReentrantLock,
}

impl<'a> Drop for ReentrantGuard<'a> {
    fn drop(&mut self) {
        self.lock.release();
    }
}

impl ReentrantLock {
    pub fn new() -> Self {
        ReentrantLock { owner: AtomicU64::new(0), count: AtomicU32::new(0), inner: Mutex::new(()), cv: Condvar::new() }
    }

    /// Acquire the lock, recursively if the calling thread already holds it.
    /// For the persistent `Lock`→`Unlock` case, call [`acquire`](Self::acquire)
    /// and [`release`](Self::release) explicitly (a guard can't span two COM
    /// calls).
    pub fn acquire(&self) {
        let tid = unsafe { GetCurrentThreadId() } as u64;
        if self.owner.load(Ordering::Relaxed) == tid {
            self.count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.owner.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            self.count.store(1, Ordering::Relaxed);
            return;
        }
        // Owned by another thread: block until it is released.
        let mut g = self.inner.lock().unwrap();
        loop {
            if self.owner.load(Ordering::Relaxed) == 0 {
                self.owner.store(tid, Ordering::Relaxed);
                self.count.store(1, Ordering::Relaxed);
                return;
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    /// Current owning thread id (0 if free). Diagnostic use only.
    pub fn owner(&self) -> u64 {
        self.owner.load(Ordering::Relaxed)
    }

    /// Release one level of recursion. When the last level is released the
    /// lock becomes available to other threads.
    pub fn release(&self) {
        let tid = unsafe { GetCurrentThreadId() } as u64;
        debug_assert_eq!(self.owner.load(Ordering::Relaxed), tid);
        let c = self.count.fetch_sub(1, Ordering::Relaxed);
        if c == 1 {
            self.owner.store(0, Ordering::Relaxed);
            self.cv.notify_one();
        }
    }

    /// Acquire, returning an RAII guard (for locks that live within a single
    /// call, e.g. `Blt`/`Flip`/present/upload).
    pub fn lock(&self) -> ReentrantGuard<'_> {
        self.acquire();
        ReentrantGuard { lock: self }
    }
}

unsafe impl Send for ReentrantLock {}
unsafe impl Sync for ReentrantLock {}

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::DirectDraw::IDirectDrawPalette;
use windows::Win32::Graphics::Gdi::{DEVMODEA, HBITMAP, HDC, HGDIOBJ, PALETTEENTRY};
use windows::Win32::Graphics::OpenGL::{HGLRC, PIXELFORMATDESCRIPTOR};

// Renderer types
pub const RENDERER_GDI: i32 = 0;
pub const RENDERER_OPENGL: i32 = 1;
pub const RENDERER_D3D9: i32 = 2;
/// OpenGL 3.2 core-profile renderer (cnc-ddraw `openglcore`).
pub const RENDERER_OPENGL_CORE: i32 = 3;

// DMDFO (fixed display output)
pub const DMDFO_DEFAULT: u32 = 0x0000_0000;
pub const DMDFO_STRETCH: u32 = 0x0000_0001;
pub const DMDFO_CENTER: u32 = 0x0000_0002;

// Edge detection
pub const EDGE_NULL: i32 = 1;
pub const EDGE_X: i32 = 2;
pub const EDGE_Y: i32 = 3;

// Timers
pub const TIMER_FIX_WINDOWPOS: usize = 78;
pub const TIMER_EDGE: usize = 79;

/// Pixel buffer shared between a surface and the renderer thread.
///
/// Owned by an `Arc` so the renderer thread can keep reading the primary
/// surface even while the game holds COM references to it.
pub struct SurfaceBuffers {
    pub hdc: HDC,
    pub bitmap: HBITMAP,
    pub default_bm: HGDIOBJ,
    pub surface: *mut u8,
    pub width: i32,
    pub height: i32,
    pub pitch: i32,
    pub bpp: i32,
    pub using_pbo: bool,
    pub lock: ReentrantLock,
}

unsafe impl Send for SurfaceBuffers {}
unsafe impl Sync for SurfaceBuffers {}

impl Drop for SurfaceBuffers {
    fn drop(&mut self) {
        unsafe {
            if !self.hdc.is_invalid() && !self.default_bm.is_invalid() {
                windows::Win32::Graphics::Gdi::SelectObject(self.hdc, self.default_bm);
            }
            if !self.bitmap.is_invalid() {
                windows::Win32::Graphics::Gdi::DeleteObject(HGDIOBJ(self.bitmap.0));
            }
            if !self.hdc.is_invalid() {
                windows::Win32::Graphics::Gdi::DeleteDC(self.hdc);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Viewport {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Viewport {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

pub struct RenderInfo {
    pub invalidate: bool,
    pub stretched: bool,
    pub width: i32,
    pub height: i32,
    pub scale_h: f32,
    pub scale_w: f32,
    pub viewport: Viewport,
}

impl Default for RenderInfo {
    fn default() -> Self {
        Self {
            invalidate: false,
            stretched: false,
            width: 0,
            height: 0,
            scale_h: 1.0,
            scale_w: 1.0,
            viewport: Viewport { left: 0, top: 0, right: 0, bottom: 0 },
        }
    }
}

pub struct GlInfo {
    pub gl_supported: bool,
    pub initialized: bool,
    pub hrc_render: HGLRC,
    pub hrc_main: HGLRC,
    pub texture_id: u32,
    pub pbo_supported: bool,
    pub primary_pbo: bool,
}

impl Default for GlInfo {
    fn default() -> Self {
        Self {
            gl_supported: false,
            initialized: false,
            hrc_render: HGLRC(std::ptr::null_mut()),
            hrc_main: HGLRC(std::ptr::null_mut()),
            texture_id: 0,
            pbo_supported: false,
            primary_pbo: false,
        }
    }
}

pub struct DDrawState {
    pub hwnd: HWND,
    pub hdc: HDC,
    pub screen_width: i32,
    pub screen_height: i32,
    pub win_rect: RECT,
    pub width: i32,
    pub height: i32,
    pub bpp: i32,
    pub dw_flags: u32,
    pub pixel_format_set: bool,
    pub pfd: PIXELFORMATDESCRIPTOR,
    pub win_mode: DEVMODEA,
    pub mode: DEVMODEA,
    pub render: RenderInfo,
    pub gl_info: GlInfo,
    pub focus_gained: i32,
    pub mouse_is_locked: i32,
    pub edge_dimension: i32,
    pub edge_value: i32,
    pub edge_timeout_ms: i32,
    pub wnd_proc: isize,
    pub renderer: i32,
    pub auto_renderer: bool,
    pub target_fps: f64,
    pub target_frame_len: f64,
    pub draw_fps: bool,
    pub fps: f64,
    pub swap_interval: i32,
    pub primary_surface2tex: bool,
    pub gl_finish: bool,
    pub convert_on_gpu: bool,
    pub gl_fence_sync: bool,
    pub fixed_output: u32,
    pub maintain_aspect_ratio: bool,
    pub windowboxing: bool,
    pub stretch_to_fullscreen: bool,
    pub stretch_to_width: i32,
    pub stretch_to_height: i32,
    pub system_affinity: usize,
    pub proc_affinity: usize,
    pub primary: Option<Arc<SurfaceBuffers>>,
    pub render_thread: Option<windows::Win32::Foundation::HANDLE>,
    pub running: AtomicBool,
    pub present_dirty: AtomicBool,
    /// Set by the game when it finishes composing a full frame (e.g. via
    /// `WaitForVerticalBlank` / `Flip`). The renderer presents only on this
    /// signal once the game is known to emit it, so we never show a
    /// half-composed (torn) single-buffered primary.
    pub frame_ready: AtomicBool,
    /// Whether the game relies on vertical blank synchronization. Detected by
    /// the first `WaitForVerticalBlank` call; until then we fall back to the
    /// dirty flag so non-vblank games still update.
    pub uses_vblank: AtomicBool,
    /// Upscale filter for all backends: 0=nearest, 1=bilinear, 2=catmull-rom,
    /// 3=lanczos, 4=xBR. Applied in software before the final present.
    pub filter: i32,
    /// Directory for F12 screenshots (default: next to the DLL).
    pub screenshot_dir: Option<String>,
    /// Keep the window border visible in windowed mode.
    pub border: bool,
    /// Allow the window to be resized by the user.
    pub resizable: bool,
    /// Fake window/client size reported to the game via GetWindowRect /
    /// GetClientRect (0,0 = report the real size).
    pub fake_size: (i32, i32),
    /// Fake OS version reported when the game calls GetVersionEx /
    /// RtlGetVersion-style checks. (0,0) = real version. e.g. (5,1) = WinXP.
    pub fake_version: (u32, u32),
    /// The palette most recently attached to a surface by the game (single
    /// global palette, like cnc-ddraw). 8-bit surfaces are expanded through it.
    pub primary_palette: Option<IDirectDrawPalette>,

    // ---- window / game compatibility (cnc-ddraw keys) ----
    pub noactivateapp: bool,
    pub fix_not_responding: bool,
    pub no_compat_warning: bool,
    pub game_handles_close: bool,
    pub terminate_process: bool,
    pub remove_menu: bool,
    pub fix_alt_key_stuck: bool,
    pub fixchilds: i32,
    pub lock_surfaces: bool,
    pub flipclear: bool,
    pub tshack: bool,
    pub vhack: bool,
    pub devmode: bool,
    pub limit_gdi_handles: bool,
    pub guard_lines: i32,
    pub min_font_size: i32,
    pub anti_aliased_fonts_min_size: i32,
    pub pos_x: i32,
    pub pos_y: i32,
    #[allow(clippy::doc_lazy_continuation)]
    pub savesettings: i32,
    /// 0 = never, 1 = auto, 2 = always.
    pub center_window: i32,
    /// Disable fullscreen-exclusive mode (cnc-ddraw `nonexclusive`); forces
    /// `SetCooperativeLevel(DDSCL_EXCLUSIVE...)` to behave as windowed.
    pub nonexclusive: bool,

    // ---- display / resolution ----
    /// Custom resolution override (0 = use the game-requested mode).
    pub res_width: i32,
    pub res_height: i32,
    pub refresh_rate: i32,
    /// EnumDisplaySettings list size: 0 small, 1 mini, 2 full.
    pub resolutions: i32,
    /// Cap on the number of enumerated display modes.
    pub max_resolutions: i32,
    pub inject_resolution: String,
    pub fake_mode: String,

    // ---- mouse / input ----
    pub adjmouse: bool,
    pub lock_mouse_top_left: bool,
    pub center_cursor_fix: bool,
    pub hook_peekmessage: bool,
    pub no_dinput_hook: bool,
    /// game -> window mouse scaling (adjmouse).
    pub mouse_scale_x: f64,
    pub mouse_scale_y: f64,
    /// window -> game inverse mouse scaling.
    pub mouse_scale_ix: f64,
    pub mouse_scale_iy: f64,

    // ---- fps / speed limiter ----
    /// 0 auto, 1 TestCooperativeLevel, 2 BltFast, 3 Unlock, 4 PeekMessage.
    pub limiter_type: i32,
    /// Max game logic ticks/sec: -1 disabled, -2 refresh rate, 0 60Hz, n custom.
    pub maxgameticks: i32,
    /// Force minimum FPS: 0 disabled, -1 use maxfps, -2 force redraw, n cap.
    pub minfps: i32,
    /// cnc-ddraw `maxfps` (cap: -1 screen rate, 0 unlimited, n target fps).
    pub maxfps: i32,

    // ---- hotkeys (virtual key codes) ----
    pub keytogglefullscreen: i32,
    pub keytogglefullscreen2: i32,
    pub keytogglemaximize: i32,
    pub keytogglemaximize2: i32,
    pub keyunlockcursor1: i32,
    pub keyunlockcursor2: i32,
    pub keyscreenshot: i32,
    /// Hotkey (F11 by default) that toggles the in-game config overlay. 0 disables.
    pub keyconfig: i32,
    pub toggle_borderless: bool,
    pub toggle_upscaled: bool,

    // ---- renderer / GL ----
    /// Shader selection: built-in name ("nearest"/"bilinear"/"bicubic"/
    /// "lanczos"/"xbr-lv2") or path to a libretro-style .glsl file.
    pub shader: String,
    pub shaderpath: String,
    pub shaderpath_pass1: String,

    // ---- gamma ----
    /// 256 RGB triples stored by IDirectDrawGammaControl::SetGammaRamp.
    pub gamma_ramp: Option<[u16; 768]>,
}

unsafe impl Send for DDrawState {}
unsafe impl Sync for DDrawState {}

impl Default for DDrawState {
    fn default() -> Self {
        unsafe {
            Self {
                hwnd: HWND(std::ptr::null_mut()),
                hdc: HDC(std::ptr::null_mut()),
                screen_width: 0,
                screen_height: 0,
                win_rect: zero_rect(),
                width: 640,
                height: 480,
                bpp: 16,
                dw_flags: 0,
                pixel_format_set: false,
                pfd: std::mem::zeroed(),
                win_mode: DEVMODEA::default(),
                mode: DEVMODEA::default(),
                render: RenderInfo::default(),
                gl_info: GlInfo::default(),
                focus_gained: 0,
                mouse_is_locked: 0,
                edge_dimension: EDGE_NULL,
                edge_value: 0,
                edge_timeout_ms: 0,
                wnd_proc: 0,
                renderer: RENDERER_OPENGL,
                auto_renderer: true,
                target_fps: 60.0,
                target_frame_len: 1000.0 / 60.0,
                draw_fps: false,
                fps: 0.0,
                swap_interval: 0,
                primary_surface2tex: true,
                gl_finish: false,
                convert_on_gpu: true,
                gl_fence_sync: false,
                fixed_output: DMDFO_STRETCH,
                maintain_aspect_ratio: false,
                windowboxing: false,
                stretch_to_fullscreen: false,
                stretch_to_width: 0,
                stretch_to_height: 0,
                system_affinity: 0,
                proc_affinity: 0,
                primary: None,
                render_thread: None,
                running: AtomicBool::new(true),
                present_dirty: AtomicBool::new(false),
                frame_ready: AtomicBool::new(false),
                uses_vblank: AtomicBool::new(false),
                filter: 0,
                screenshot_dir: None,
                border: false,
                resizable: false,
                fake_size: (0, 0),
                fake_version: (0, 0),
                primary_palette: None,
                noactivateapp: false,
                fix_not_responding: false,
                no_compat_warning: false,
                game_handles_close: false,
                terminate_process: false,
                remove_menu: false,
                fix_alt_key_stuck: false,
                fixchilds: 2,
                lock_surfaces: false,
                flipclear: false,
                tshack: false,
                vhack: false,
                devmode: false,
                limit_gdi_handles: false,
                guard_lines: 200,
                min_font_size: 0,
                anti_aliased_fonts_min_size: 13,
                pos_x: -32000,
                pos_y: -32000,
                savesettings: 1,
                center_window: 1,
                nonexclusive: true,
                res_width: 0,
                res_height: 0,
                refresh_rate: 0,
                resolutions: 0,
                max_resolutions: 0,
                inject_resolution: String::new(),
                fake_mode: String::new(),
                adjmouse: true,
                lock_mouse_top_left: false,
                center_cursor_fix: false,
                hook_peekmessage: false,
                no_dinput_hook: false,
                mouse_scale_x: 1.0,
                mouse_scale_y: 1.0,
                mouse_scale_ix: 1.0,
                mouse_scale_iy: 1.0,
                limiter_type: 0,
                maxgameticks: 0,
                minfps: 0,
                maxfps: 0,
                keytogglefullscreen: 0x0D,
                keytogglefullscreen2: 0,
                keytogglemaximize: 0x22,
                keytogglemaximize2: 0,
                keyunlockcursor1: 0x09,
                keyunlockcursor2: 0xA3,
                keyscreenshot: 0x7B,
                keyconfig: 0x7A,
                toggle_borderless: false,
                toggle_upscaled: false,
                shader: "catmull-rom-bilinear.glsl".to_string(),
                shaderpath: String::new(),
                shaderpath_pass1: String::new(),
                gamma_ramp: None,
            }
        }
    }
}

static STATE: OnceLock<Arc<Mutex<DDrawState>>> = OnceLock::new();

/// Whether 16-bit primary surfaces use RGB555 (X1R5G5B5) instead of the
/// default RGB565. Set from the `rgb555` INI option during config load. Kept as
/// a lock-free atomic so surface/renderer code can read it without touching the
/// main state mutex (avoids re-entrant lock deadlocks).
pub static RGB555: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Access the global shared DirectDraw state.
pub fn state() -> &'static Arc<Mutex<DDrawState>> {
    STATE.get_or_init(|| Arc::new(Mutex::new(DDrawState::default())))
}

pub fn zero_rect() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}

pub fn make_rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

/// Register the palette the game most recently attached to a surface. 8-bit
/// surfaces (blits and renderer uploads) are expanded through it, mirroring
/// cnc-ddraw's single global `ddState.palette`. Keeping the COM interface
/// means later `SetEntries` calls are automatically visible to the renderer.
pub fn register_palette(p: &IDirectDrawPalette) {
    state().lock().unwrap().primary_palette = Some(p.clone());
}

/// Snapshot of the active 8-bit palette as `[B, G, R, Flags]` per entry.
/// Returns `None` when no palette has been attached yet.
pub fn active_palette_entries() -> Option<[[u8; 4]; 256]> {
    let pal = state().lock().unwrap().primary_palette.clone()?;
    unsafe {
        let mut entries = [std::mem::zeroed::<PALETTEENTRY>(); 256];
        if pal.GetEntries(0, 0, 256, entries.as_mut_ptr()).is_err() {
            return None;
        }
        let mut out = [[0u8; 4]; 256];
        for (i, e) in entries.iter().enumerate() {
            out[i] = [e.peBlue, e.peGreen, e.peRed, e.peFlags];
        }
        Some(out)
    }
}

/// Interlocked-style read of the current renderer.
pub fn renderer() -> i32 {
    state().lock().unwrap().renderer
}

/// Whether the game window currently holds focus (as far as we know).
pub fn focused() -> bool {
    state().lock().unwrap().focus_gained != 0
}

/// Mark the primary surface as changed so the renderer thread presents it.
/// Called whenever the game draws to / flips the primary surface.
pub fn mark_dirty() {
    state().lock().unwrap().present_dirty.store(true, Ordering::Relaxed);
}

/// Consume the dirty flag: returns true once if the primary changed since the
/// last call. Lets the renderer present only on real frame updates, avoiding
/// showing mid-composition (torn) frames repeatedly.
pub fn take_dirty() -> bool {
    state().lock().unwrap().present_dirty.swap(false, Ordering::Relaxed)
}

/// Signal that the game completed a full frame (via WaitForVerticalBlank).
/// NOTE: this is intentionally NOT used as an upload trigger. Games may call
/// WaitForVerticalBlank *before* drawing the frame, so presenting on it would
/// upload a half-drawn primary (black screen). cnc-ddraw uploads only on
/// `surface_updated` (our `present_dirty`, set on Blt/Flip/Unlock/ReleaseDC),
/// which is raised *after* the draw completes. We keep this only for stats.
pub fn mark_frame_ready() {
    let st = state().lock().unwrap();
    st.uses_vblank.store(true, Ordering::Relaxed);
    st.frame_ready.store(true, Ordering::Relaxed);
}

/// Switch renderer (used by hotkey / auto-fallback).
pub fn set_renderer(r: i32) {
    state().lock().unwrap().renderer = r;
}
