//! IDirectDraw implementation (ports `IDirectDraw.c`).

pub(crate) mod clipper;
pub(crate) mod palette;
pub(crate) mod surface;

use std::sync::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::{
    CDS_TYPE, ChangeDisplaySettingsA, DEVMODEA, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
    EnumDisplaySettingsA, GetDC, HDC, PALETTEENTRY,
};
use windows::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_SUPPORT_OPENGL, PFD_SWAP_EXCHANGE, PFD_TYPE_RGBA,
    PIXELFORMATDESCRIPTOR, SetPixelFormat,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, GetSystemMetrics, HWND_TOP, SM_CXSCREEN, SM_CYSCREEN, SWP_SHOWWINDOW, SetWindowPos,
};
use windows::core::*;

use self::clipper::ClipperImpl;
use self::palette::PaletteImpl;
use self::surface::SurfaceImpl;
use crate::state::{RENDERER_OPENGL, state};
use crate::util::is_windows_xp;
use crate::window;

const DDERR_INVALIDMODE: HRESULT = HRESULT(0x8876_0248u32 as i32);

fn fill_pixel_format(pf: &mut DDPIXELFORMAT, bpp: i32) {
    pf.dwSize = std::mem::size_of::<DDPIXELFORMAT>() as u32;
    pf.dwFlags = DDPF_RGB as u32;
    pf.Anonymous1.dwRGBBitCount = bpp as u32;
    if bpp == 8 {
        pf.dwFlags = (DDPF_PALETTEINDEXED8 | DDPF_RGB) as u32;
    } else if bpp == 16 {
        pf.Anonymous2.dwRBitMask = 0xF800;
        pf.Anonymous3.dwGBitMask = 0x07E0;
        pf.Anonymous4.dwBBitMask = 0x001F;
    } else if bpp == 24 || bpp == 32 {
        pf.Anonymous2.dwRBitMask = 0xFF0000;
        pf.Anonymous3.dwGBitMask = 0x00FF00;
        pf.Anonymous4.dwBBitMask = 0x0000FF;
    }
}

fn fill_caps(caps: &mut DDCAPS_DX7) {
    caps.dwSize = std::mem::size_of::<DDCAPS_DX7>() as u32;
    caps.dwCaps = (DDCAPS_BLT
        | DDCAPS_PALETTE
        | DDCAPS_BLTCOLORFILL
        | DDCAPS_BLTSTRETCH
        | DDCAPS_CANCLIP
        | DDCAPS_CANCLIPSTRETCHED
        | DDCAPS_COLORKEY) as u32;
    caps.dwCaps2 = DDCAPS2_NOPAGELOCKREQUIRED as u32;
    caps.dwPalCaps = (DDPCAPS_8BIT | DDPCAPS_PRIMARYSURFACE) as u32;
    caps.dwVidMemTotal = 16 * 1024 * 1024;
    caps.dwVidMemFree = 16 * 1024 * 1024;
    caps.ddsCaps.dwCaps = (DDSCAPS_BACKBUFFER
        | DDSCAPS_COMPLEX
        | DDSCAPS_FLIP
        | DDSCAPS_FRONTBUFFER
        | DDSCAPS_OFFSCREENPLAIN
        | DDSCAPS_PRIMARYSURFACE
        | DDSCAPS_VIDEOMEMORY
        | DDSCAPS_OWNDC
        | DDSCAPS_LOCALVIDMEM) as u32;
}

fn create_clipper() -> Result<IDirectDrawClipper> {
    Ok(ClipperImpl { hwnd: Mutex::new(0), clip: Mutex::new(Vec::new()), changed: Mutex::new(false) }.into())
}

fn create_palette(dwflags: u32, lpddcolorarray: *mut PALETTEENTRY) -> Result<IDirectDrawPalette> {
    let mut entries = [[0u8; 4]; 256];
    if !lpddcolorarray.is_null() {
        unsafe {
            for (i, e) in entries.iter_mut().enumerate() {
                let pe = *lpddcolorarray.add(i);
                *e = [pe.peBlue, pe.peGreen, pe.peRed, pe.peFlags];
            }
        }
    }
    Ok(PaletteImpl { flags: dwflags, entries: entries.into() }.into())
}

fn create_surface(
    width: i32,
    height: i32,
    bpp: i32,
    is_primary: bool,
    has_flip: bool,
    backbuffer_count: u32,
) -> SurfaceImpl {
    let hdc = state().lock().unwrap().hdc;
    crate::dd_log!(
        "create_surface: {}x{} bpp={} is_primary={} has_flip={} backbuffer_count={}",
        width,
        height,
        bpp,
        is_primary,
        has_flip,
        backbuffer_count
    );
    let s = SurfaceImpl::new(hdc, width, height, bpp, is_primary);
    let s = if is_primary && has_flip && backbuffer_count > 0 { s.with_backbuffer(hdc, width, height, bpp) } else { s };

    if is_primary {
        let buffers = s.buffers.clone();
        let mut st = state().lock().unwrap();
        st.primary = Some(buffers);
        drop(st);
        crate::dd_log!(
            "primary surface created: {}x{} bpp={} (has_flip={}, backbuffer_count={})",
            width,
            height,
            bpp,
            has_flip,
            backbuffer_count
        );
        crate::render::start();
    }
    s
}

fn enum_display_modes(
    filter_desc: *mut DDSURFACEDESC,
    context: *mut core::ffi::c_void,
    callback: LPDDENUMMODESCALLBACK,
) -> Result<()> {
    let callback = match callback {
        Some(f) => f,
        None => return Ok(()),
    };
    let filter_res = if !filter_desc.is_null() {
        let desc = unsafe { &*filter_desc };
        if (desc.dwFlags & (DDSD_WIDTH | DDSD_HEIGHT) as u32) == (DDSD_WIDTH | DDSD_HEIGHT) as u32 {
            Some((desc.dwWidth, desc.dwHeight))
        } else {
            None
        }
    } else {
        None
    };

    for &bpp in &[8u32, 16, 32] {
        let mut dm = DEVMODEA::default();
        let mut i = 0u32;
        while unsafe { EnumDisplaySettingsA(None, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm).as_bool() } {
            let w = dm.dmPelsWidth;
            let h = dm.dmPelsHeight;
            if let Some((fw, fh)) = filter_res
                && (w != fw || h != fh)
            {
                i += 1;
                continue;
            }
            let mut desc = DDSURFACEDESC {
                dwSize: std::mem::size_of::<DDSURFACEDESC>() as u32,
                dwFlags: (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT | DDSD_REFRESHRATE) as u32,
                dwWidth: w,
                dwHeight: h,
                ..Default::default()
            };
            desc.Anonymous2.dwRefreshRate = 60;
            let bytes_pp = (bpp / 8).max(1);
            desc.Anonymous1.lPitch = (w * bytes_pp) as i32;
            fill_pixel_format(&mut desc.ddpfPixelFormat, bpp as i32);
            let hr = unsafe { callback(&mut desc, context) };
            if hr == HRESULT(0) {
                return Ok(());
            }
            i += 1;
        }
    }
    Ok(())
}

fn enum_display_modes2(
    filter_desc: *mut DDSURFACEDESC2,
    context: *mut core::ffi::c_void,
    callback: LPDDENUMMODESCALLBACK2,
) -> Result<()> {
    let callback = match callback {
        Some(f) => f,
        None => return Ok(()),
    };
    let filter_res = if !filter_desc.is_null() {
        let desc = unsafe { &*filter_desc };
        if (desc.dwFlags & (DDSD_WIDTH | DDSD_HEIGHT) as u32) == (DDSD_WIDTH | DDSD_HEIGHT) as u32 {
            Some((desc.dwWidth, desc.dwHeight))
        } else {
            None
        }
    } else {
        None
    };

    for &bpp in &[8u32, 16, 32] {
        let mut dm = DEVMODEA::default();
        let mut i = 0u32;
        while unsafe { EnumDisplaySettingsA(None, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm).as_bool() } {
            let w = dm.dmPelsWidth;
            let h = dm.dmPelsHeight;
            if let Some((fw, fh)) = filter_res
                && (w != fw || h != fh)
            {
                i += 1;
                continue;
            }
            let mut desc = DDSURFACEDESC2 {
                dwSize: std::mem::size_of::<DDSURFACEDESC2>() as u32,
                dwFlags: (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT | DDSD_REFRESHRATE) as u32,
                dwWidth: w,
                dwHeight: h,
                ..Default::default()
            };
            desc.Anonymous3.dwRefreshRate = 60;
            let bytes_pp = (bpp / 8).max(1);
            desc.Anonymous1.lPitch = (w * bytes_pp) as i32;
            unsafe {
                fill_pixel_format(&mut desc.Anonymous5.ddpfPixelFormat, bpp as i32);
            }
            let hr = unsafe { callback(&mut desc, context) };
            if hr == HRESULT(0) {
                return Ok(());
            }
            i += 1;
        }
    }
    Ok(())
}

#[implement(IDirectDraw7, IDirectDraw4, IDirectDraw2, IDirectDraw)]
pub struct DirectDrawImpl {}

impl IDirectDraw_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> {
        Ok(())
    }
    fn CreateClipper(
        &self,
        _dwflags: u32,
        lplpddclipper: OutRef<'_, IDirectDrawClipper>,
        _punkouter: Ref<'_, IUnknown>,
    ) -> Result<()> {
        let _ = lplpddclipper.write(Some(create_clipper()?));
        Ok(())
    }
    fn CreatePalette(
        &self,
        dwflags: u32,
        lpddcolorarray: *mut PALETTEENTRY,
        lplpddpalette: OutRef<'_, IDirectDrawPalette>,
        _punkouter: Ref<'_, IUnknown>,
    ) -> Result<()> {
        let _ = lplpddpalette.write(Some(create_palette(dwflags, lpddcolorarray)?));
        Ok(())
    }
    fn CreateSurface(
        &self,
        lpddsd: *mut DDSURFACEDESC,
        lplpddsurface: OutRef<'_, IDirectDrawSurface>,
        _punkouter: Ref<'_, IUnknown>,
    ) -> Result<()> {
        crate::dd_log!("IDirectDraw::CreateSurface(lpDDSD={:p})", lpddsd);
        if lpddsd.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let desc = unsafe { &*lpddsd };
        let is_primary = (desc.ddsCaps.dwCaps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
        let has_flip = (desc.ddsCaps.dwCaps & DDSCAPS_FLIP as u32) != 0;
        let (w, h, bpp) = if is_primary {
            let st = state().lock().unwrap();
            (st.width, st.height, st.bpp)
        } else {
            let w =
                if desc.dwWidth > 0 { (desc.dwWidth.div_ceil(2) * 2) as i32 } else { state().lock().unwrap().width };
            let h = if desc.dwHeight > 0 { desc.dwHeight as i32 } else { state().lock().unwrap().height };
            let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
                unsafe { desc.ddpfPixelFormat.Anonymous1.dwRGBBitCount as i32 }
            } else {
                16
            };
            (w, h, bpp)
        };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 { desc.dwBackBufferCount } else { 0 };
        let surface = create_surface(w, h, bpp, is_primary, has_flip, bbc);
        let surface: IDirectDrawSurface = surface.into();
        let _ = lplpddsurface.write(Some(surface));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface>) -> Result<IDirectDrawSurface> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn EnumDisplayModes(
        &self,
        _dwflags: u32,
        lpddsd: *mut DDSURFACEDESC,
        lpcontext: *mut core::ffi::c_void,
        callback: LPDDENUMMODESCALLBACK,
    ) -> Result<()> {
        enum_display_modes(lpddsd, lpcontext, callback)
    }
    fn EnumSurfaces(
        &self,
        _a: u32,
        _b: *mut DDSURFACEDESC,
        _c: *mut core::ffi::c_void,
        _d: LPDDENUMSURFACESCALLBACK,
    ) -> Result<()> {
        Ok(())
    }
    fn FlipToGDISurface(&self) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, driver: *mut DDCAPS_DX7, emul: *mut DDCAPS_DX7) -> Result<()> {
        if !driver.is_null() {
            unsafe { fill_caps(&mut *driver) };
        }
        if !emul.is_null() {
            unsafe {
                (*emul).dwSize = std::mem::size_of::<DDCAPS_DX7>() as u32;
                (*emul).dwCaps = DDCAPS_BLTSTRETCH as u32;
            }
        }
        Ok(())
    }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let st = state().lock().unwrap();
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
            (*desc).dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT) as u32;
            (*desc).dwWidth = st.width as u32;
            (*desc).dwHeight = st.height as u32;
            let bytes_pp = (st.bpp / 8).max(1);
            (*desc).Anonymous1.lPitch = st.width * bytes_pp;
            fill_pixel_format(&mut (*desc).ddpfPixelFormat, st.bpp);
        }
        Ok(())
    }
    fn GetFourCCCodes(&self, _a: *mut u32, _b: *mut u32) -> Result<()> {
        Ok(())
    }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn GetMonitorFrequency(&self, f: *mut u32) -> Result<()> {
        if !f.is_null() {
            unsafe { *f = 60 };
        }
        Ok(())
    }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> {
        if !a.is_null() {
            unsafe { *a = 0 };
        }
        Ok(())
    }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> {
        if !a.is_null() {
            unsafe { *a = BOOL(0) };
        }
        Ok(())
    }
    fn Initialize(&self, _a: *mut GUID) -> Result<()> {
        Ok(())
    }
    fn RestoreDisplayMode(&self) -> Result<()> {
        let st = state().lock().unwrap();
        unsafe {
            ChangeDisplaySettingsA(Some(&st.win_mode), CDS_TYPE(0));
        }
        Ok(())
    }
    fn SetCooperativeLevel(&self, hwnd: HWND, flags: u32) -> Result<()> {
        crate::dd_log!("IDirectDraw::SetCooperativeLevel(hwnd={:p}, flags={:#x})", hwnd.0, flags);
        if hwnd.is_invalid() {
            return Err(E_INVALIDARG.into());
        }
        // `nonexclusive` (cnc-ddraw): strip the exclusive/fullscreen bits so the
        // game always runs windowed instead of taking the monitor into exclusive
        // fullscreen mode.
        let effective = {
            let st = state().lock().unwrap();
            if st.nonexclusive && (flags & (DDSCL_EXCLUSIVE as u32 | DDSCL_FULLSCREEN as u32)) != 0 {
                crate::dd_log!(
                    "SetCooperativeLevel: nonexclusive downgrade {:#x} -> {:#x}",
                    flags,
                    flags & !(DDSCL_EXCLUSIVE as u32 | DDSCL_FULLSCREEN as u32)
                );
                flags & !(DDSCL_EXCLUSIVE as u32 | DDSCL_FULLSCREEN as u32)
            } else {
                flags
            }
        };
        unsafe {
            let mut st = state().lock().unwrap();
            st.bpp = 16;
            st.dw_flags = effective;
            st.hwnd = hwnd;
            st.hdc = GetDC(Some(hwnd));
            st.wnd_proc = window::subclass(hwnd) as isize;
            crate::dd_log!("SetCooperativeLevel: subclass done");

            if !st.pixel_format_set && !is_windows_xp() {
                st.pixel_format_set = true;
                let mut pfd: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
                pfd.nSize = std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16;
                pfd.nVersion = 1;
                if st.renderer == RENDERER_OPENGL {
                    pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER | PFD_SWAP_EXCHANGE;
                } else {
                    pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_DOUBLEBUFFER;
                }
                pfd.iPixelType = PFD_TYPE_RGBA;
                pfd.cColorBits = st.bpp as u8;
                pfd.iLayerType = 0;
                crate::dd_log!("SetCooperativeLevel: SetPixelFormat begin");
                if SetPixelFormat(st.hdc, ChoosePixelFormat(st.hdc, &pfd), &pfd).is_err() {
                    crate::dd_log!("SetPixelFormat failed");
                }
                crate::dd_log!("SetCooperativeLevel: SetPixelFormat done");
            }

            if st.screen_width == 0 {
                st.screen_width = GetSystemMetrics(SM_CXSCREEN);
                st.screen_height = GetSystemMetrics(SM_CYSCREEN);
            }

            if st.win_mode.dmSize == 0 {
                st.win_mode.dmSize = std::mem::size_of::<DEVMODEA>() as u16;
                if EnumDisplaySettingsA(None, ENUM_CURRENT_SETTINGS, &mut st.win_mode).as_bool() {
                    st.screen_width = st.win_mode.dmPelsWidth as i32;
                    st.screen_height = st.win_mode.dmPelsHeight as i32;
                }
            }
            crate::dd_log!("SetCooperativeLevel: screen dims done ({})", st.screen_width);

            if effective & DDSCL_FULLSCREEN as u32 != 0 {
                crate::dd_log!("SetCooperativeLevel: fullscreen mode");
            }

            if effective & DDSCL_FULLSCREEN as u32 == 0 {
                // Snapshot the layout before repositioning: SetWindowPos sends
                // WM_SIZE/WM_MOVE synchronously and `wnd_proc` re-locks
                // `state()`, so the lock must not be held across the call
                // (the std mutex is not reentrant; this previously deadlocked
                // the game's load thread right after this log line).
                let (w, h, sw, sh, win_right, win_bottom) = (
                    st.width,
                    st.height,
                    st.screen_width,
                    st.screen_height,
                    st.win_rect.right,
                    st.win_rect.bottom,
                );
                drop(st);
                let rw = if win_right != 0 { win_right } else { w };
                let rh = if win_bottom != 0 { win_bottom } else { h };
                let x = (sw / 2) - (rw / 2);
                let y = (sh / 2) - (rh / 2);
                let mut dst = RECT { left: x, top: y, right: rw + x, bottom: rh + y };
                adjust_window_rect(&mut dst, hwnd);
                crate::dd_log!("SetCooperativeLevel: windowed SetWindowPos begin");
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    dst.left,
                    dst.top,
                    dst.right - dst.left,
                    dst.bottom - dst.top,
                    SWP_SHOWWINDOW,
                );
                crate::dd_log!("SetCooperativeLevel: windowed SetWindowPos done");
            }
        }
        crate::dd_log!("SetCooperativeLevel: end");
        Ok(())
    }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32) -> Result<()> {
        crate::dd_log!("IDirectDraw::SetDisplayMode({}, {}, {})", w, h, bpp);
        if bpp != 8 && bpp != 16 && bpp != 32 {
            return Err(Error::from(DDERR_INVALIDMODE));
        }
        let gw = if w > 0 { w as i32 } else { 640 };
        let gh = if h > 0 { h as i32 } else { 480 };
        let mut st = state().lock().unwrap();
        st.width = gw;
        st.height = gh;
        st.bpp = bpp as i32;
        drop(st);
        crate::dd_log!("SetDisplayMode: bpp={} before set_window_size", bpp);
        unsafe {
            window::set_window_size(gw, gh);
        }
        crate::dd_log!("SetDisplayMode: after set_window_size");
        // Under `nonexclusive` we never force the monitor into the requested
        // mode; the game stays windowed so the desktop mode is preserved.
        let nonexclusive = state().lock().unwrap().nonexclusive;
        if !nonexclusive {
            unsafe {
                window::set_display_mode(gw, gh, bpp as i32);
            }
        }
        crate::dd_log!("SetDisplayMode: after set_display_mode");
        Ok(())
    }
    fn WaitForVerticalBlank(&self, _a: u32, _b: HANDLE) -> Result<()> {
        // Signal a complete frame, then throttle the game to roughly the target
        // refresh. Real ddraw blocks here until the next vertical blank; if we
        // return immediately the game spins at 100% CPU, which starves audio and
        // the render thread (black screen / stutter). cnc-ddraw does the same
        // via its flip_limiter timer.
        crate::state::mark_frame_ready();
        let interval_ms = {
            let st = crate::state::state().lock().unwrap();
            let fps = st.target_fps;
            if fps > 0.0 { 1000.0 / fps } else { 1000.0 / 60.0 }
        };
        if interval_ms > 0.5 {
            static LAST: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
            let last = LAST.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
            let mut guard = last.lock().unwrap();
            let now = std::time::Instant::now();
            let elapsed = now.saturating_duration_since(*guard);
            let target = std::time::Duration::from_secs_f64(interval_ms / 1000.0);
            if elapsed < target {
                std::thread::sleep(target - elapsed);
            }
            *guard = std::time::Instant::now();
        }
        Ok(())
    }
}

impl IDirectDraw2_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> {
        Ok(())
    }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> {
        IDirectDraw_Impl::CreateClipper(self, a, b, c)
    }
    fn CreatePalette(
        &self,
        a: u32,
        b: *mut PALETTEENTRY,
        c: OutRef<'_, IDirectDrawPalette>,
        d: Ref<'_, IUnknown>,
    ) -> Result<()> {
        IDirectDraw_Impl::CreatePalette(self, a, b, c, d)
    }
    fn CreateSurface(
        &self,
        a: *mut DDSURFACEDESC,
        b: OutRef<'_, IDirectDrawSurface>,
        c: Ref<'_, IUnknown>,
    ) -> Result<()> {
        IDirectDraw_Impl::CreateSurface(self, a, b, c)
    }
    fn DuplicateSurface(&self, a: Ref<'_, IDirectDrawSurface>) -> Result<IDirectDrawSurface> {
        IDirectDraw_Impl::DuplicateSurface(self, a)
    }
    fn EnumDisplayModes(
        &self,
        a: u32,
        b: *mut DDSURFACEDESC,
        c: *mut core::ffi::c_void,
        d: LPDDENUMMODESCALLBACK,
    ) -> Result<()> {
        IDirectDraw_Impl::EnumDisplayModes(self, a, b, c, d)
    }
    fn EnumSurfaces(
        &self,
        a: u32,
        b: *mut DDSURFACEDESC,
        c: *mut core::ffi::c_void,
        d: LPDDENUMSURFACESCALLBACK,
    ) -> Result<()> {
        IDirectDraw_Impl::EnumSurfaces(self, a, b, c, d)
    }
    fn FlipToGDISurface(&self) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> {
        IDirectDraw_Impl::GetCaps(self, a, b)
    }
    fn GetDisplayMode(&self, a: *mut DDSURFACEDESC) -> Result<()> {
        IDirectDraw_Impl::GetDisplayMode(self, a)
    }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetFourCCCodes(self, a, b)
    }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface> {
        IDirectDraw_Impl::GetGDISurface(self)
    }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetMonitorFrequency(self, a)
    }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetScanLine(self, a)
    }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> {
        IDirectDraw_Impl::GetVerticalBlankStatus(self, a)
    }
    fn Initialize(&self, a: *mut GUID) -> Result<()> {
        IDirectDraw_Impl::Initialize(self, a)
    }
    fn RestoreDisplayMode(&self) -> Result<()> {
        Ok(())
    }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> {
        IDirectDraw_Impl::SetCooperativeLevel(self, a, b)
    }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, _r: u32, _f: u32) -> Result<()> {
        IDirectDraw_Impl::SetDisplayMode(self, w, h, bpp)
    }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> {
        IDirectDraw_Impl::WaitForVerticalBlank(self, a, b)
    }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS, b: *mut u32, c: *mut u32) -> Result<()> {
        if !b.is_null() {
            unsafe { *b = 16 * 1024 * 1024 };
        }
        if !c.is_null() {
            unsafe { *c = 16 * 1024 * 1024 };
        }
        Ok(())
    }
}

impl IDirectDraw4_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> {
        Ok(())
    }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> {
        IDirectDraw_Impl::CreateClipper(self, a, b, c)
    }
    fn CreatePalette(
        &self,
        a: u32,
        b: *mut PALETTEENTRY,
        c: OutRef<'_, IDirectDrawPalette>,
        d: Ref<'_, IUnknown>,
    ) -> Result<()> {
        IDirectDraw_Impl::CreatePalette(self, a, b, c, d)
    }
    fn CreateSurface(
        &self,
        lpddsd: *mut DDSURFACEDESC2,
        lplpddsurface: OutRef<'_, IDirectDrawSurface4>,
        _punkouter: Ref<'_, IUnknown>,
    ) -> Result<()> {
        if lpddsd.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let desc = unsafe { &*lpddsd };
        let is_primary = (desc.ddsCaps.dwCaps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
        let has_flip = (desc.ddsCaps.dwCaps & DDSCAPS_FLIP as u32) != 0;
        let (w, h, bpp) = if is_primary {
            let st = state().lock().unwrap();
            (st.width, st.height, st.bpp)
        } else {
            let w =
                if desc.dwWidth > 0 { (desc.dwWidth.div_ceil(2) * 2) as i32 } else { state().lock().unwrap().width };
            let h = if desc.dwHeight > 0 { desc.dwHeight as i32 } else { state().lock().unwrap().height };
            let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
                unsafe { desc.Anonymous5.ddpfPixelFormat.Anonymous1.dwRGBBitCount as i32 }
            } else {
                16
            };
            (w, h, bpp)
        };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 {
            unsafe { desc.Anonymous2.dwBackBufferCount }
        } else {
            0
        };
        let surface = create_surface(w, h, bpp, is_primary, has_flip, bbc);
        let surface_v1: IDirectDrawSurface = surface.into();
        let surface_v4: IDirectDrawSurface4 = surface_v1.cast()?;
        let _ = lplpddsurface.write(Some(surface_v4));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface4>) -> Result<IDirectDrawSurface4> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn EnumDisplayModes(
        &self,
        _a: u32,
        b: *mut DDSURFACEDESC2,
        c: *mut core::ffi::c_void,
        d: LPDDENUMMODESCALLBACK2,
    ) -> Result<()> {
        enum_display_modes2(b, c, d)
    }
    fn EnumSurfaces(
        &self,
        _a: u32,
        _b: *mut DDSURFACEDESC2,
        _c: *mut core::ffi::c_void,
        _d: Option<
            unsafe extern "system" fn(
                Ref<'_, IDirectDrawSurface4>,
                *mut DDSURFACEDESC2,
                *mut core::ffi::c_void,
            ) -> HRESULT,
        >,
    ) -> Result<()> {
        Ok(())
    }
    fn FlipToGDISurface(&self) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> {
        IDirectDraw_Impl::GetCaps(self, a, b)
    }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let st = state().lock().unwrap();
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC2>() as u32;
            (*desc).dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT) as u32;
            (*desc).dwWidth = st.width as u32;
            (*desc).dwHeight = st.height as u32;
            let bytes_pp = (st.bpp / 8).max(1);
            (*desc).Anonymous1.lPitch = st.width * bytes_pp;
            fill_pixel_format(&mut (*desc).Anonymous5.ddpfPixelFormat, st.bpp);
        }
        Ok(())
    }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetFourCCCodes(self, a, b)
    }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface4> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetMonitorFrequency(self, a)
    }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetScanLine(self, a)
    }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> {
        IDirectDraw_Impl::GetVerticalBlankStatus(self, a)
    }
    fn Initialize(&self, a: *mut GUID) -> Result<()> {
        IDirectDraw_Impl::Initialize(self, a)
    }
    fn RestoreDisplayMode(&self) -> Result<()> {
        Ok(())
    }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> {
        IDirectDraw_Impl::SetCooperativeLevel(self, a, b)
    }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, _r: u32, _f: u32) -> Result<()> {
        IDirectDraw_Impl::SetDisplayMode(self, w, h, bpp)
    }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> {
        IDirectDraw_Impl::WaitForVerticalBlank(self, a, b)
    }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS2, b: *mut u32, c: *mut u32) -> Result<()> {
        if !b.is_null() {
            unsafe { *b = 16 * 1024 * 1024 };
        }
        if !c.is_null() {
            unsafe { *c = 16 * 1024 * 1024 };
        }
        Ok(())
    }
    fn GetSurfaceFromDC(&self, _hdc: HDC) -> Result<IDirectDrawSurface4> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn RestoreAllSurfaces(&self) -> Result<()> {
        Ok(())
    }
    fn TestCooperativeLevel(&self) -> Result<()> {
        if crate::fps_limiter::limiter_applies(crate::fps_limiter::LIMIT_TEST_COOPERATIVE_LEVEL) {
            crate::fps_limiter::wait_game_tick();
        }
        Ok(())
    }
    fn GetDeviceIdentifier(&self, _a: *mut DDDEVICEIDENTIFIER, _b: u32) -> Result<()> {
        Ok(())
    }
}

impl IDirectDraw7_Impl for DirectDrawImpl_Impl {
    fn Compact(&self) -> Result<()> {
        Ok(())
    }
    fn CreateClipper(&self, a: u32, b: OutRef<'_, IDirectDrawClipper>, c: Ref<'_, IUnknown>) -> Result<()> {
        IDirectDraw_Impl::CreateClipper(self, a, b, c)
    }
    fn CreatePalette(
        &self,
        a: u32,
        b: *mut PALETTEENTRY,
        c: OutRef<'_, IDirectDrawPalette>,
        d: Ref<'_, IUnknown>,
    ) -> Result<()> {
        IDirectDraw_Impl::CreatePalette(self, a, b, c, d)
    }
    fn CreateSurface(
        &self,
        lpddsd: *mut DDSURFACEDESC2,
        lplpddsurface: OutRef<'_, IDirectDrawSurface7>,
        _punkouter: Ref<'_, IUnknown>,
    ) -> Result<()> {
        if lpddsd.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let desc = unsafe { &*lpddsd };
        let is_primary = (desc.ddsCaps.dwCaps & DDSCAPS_PRIMARYSURFACE as u32) != 0;
        let has_flip = (desc.ddsCaps.dwCaps & DDSCAPS_FLIP as u32) != 0;
        let (w, h, bpp) = if is_primary {
            let st = state().lock().unwrap();
            (st.width, st.height, st.bpp)
        } else {
            let w =
                if desc.dwWidth > 0 { (desc.dwWidth.div_ceil(2) * 2) as i32 } else { state().lock().unwrap().width };
            let h = if desc.dwHeight > 0 { desc.dwHeight as i32 } else { state().lock().unwrap().height };
            let bpp = if desc.dwFlags & DDSD_PIXELFORMAT as u32 != 0 {
                unsafe { desc.Anonymous5.ddpfPixelFormat.Anonymous1.dwRGBBitCount as i32 }
            } else {
                16
            };
            (w, h, bpp)
        };
        let bbc = if desc.dwFlags & DDSD_BACKBUFFERCOUNT as u32 != 0 {
            unsafe { desc.Anonymous2.dwBackBufferCount }
        } else {
            0
        };
        let surface = create_surface(w, h, bpp, is_primary, has_flip, bbc);
        let surface_v1: IDirectDrawSurface = surface.into();
        let surface_v7: IDirectDrawSurface7 = surface_v1.cast()?;
        let _ = lplpddsurface.write(Some(surface_v7));
        Ok(())
    }
    fn DuplicateSurface(&self, _a: Ref<'_, IDirectDrawSurface7>) -> Result<IDirectDrawSurface7> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn EnumDisplayModes(
        &self,
        _a: u32,
        b: *mut DDSURFACEDESC2,
        c: *mut core::ffi::c_void,
        d: LPDDENUMMODESCALLBACK2,
    ) -> Result<()> {
        enum_display_modes2(b, c, d)
    }
    fn EnumSurfaces(
        &self,
        _a: u32,
        _b: *mut DDSURFACEDESC2,
        _c: *mut core::ffi::c_void,
        _d: Option<
            unsafe extern "system" fn(
                Ref<'_, IDirectDrawSurface7>,
                *mut DDSURFACEDESC2,
                *mut core::ffi::c_void,
            ) -> HRESULT,
        >,
    ) -> Result<()> {
        Ok(())
    }
    fn FlipToGDISurface(&self) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, a: *mut DDCAPS_DX7, b: *mut DDCAPS_DX7) -> Result<()> {
        IDirectDraw_Impl::GetCaps(self, a, b)
    }
    fn GetDisplayMode(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let st = state().lock().unwrap();
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC2>() as u32;
            (*desc).dwFlags = (DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT) as u32;
            (*desc).dwWidth = st.width as u32;
            (*desc).dwHeight = st.height as u32;
            let bytes_pp = (st.bpp / 8).max(1);
            (*desc).Anonymous1.lPitch = st.width * bytes_pp;
            fill_pixel_format(&mut (*desc).Anonymous5.ddpfPixelFormat, st.bpp);
        }
        Ok(())
    }
    fn GetFourCCCodes(&self, a: *mut u32, b: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetFourCCCodes(self, a, b)
    }
    fn GetGDISurface(&self) -> Result<IDirectDrawSurface7> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn GetMonitorFrequency(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetMonitorFrequency(self, a)
    }
    fn GetScanLine(&self, a: *mut u32) -> Result<()> {
        IDirectDraw_Impl::GetScanLine(self, a)
    }
    fn GetVerticalBlankStatus(&self, a: *mut BOOL) -> Result<()> {
        IDirectDraw_Impl::GetVerticalBlankStatus(self, a)
    }
    fn Initialize(&self, a: *mut GUID) -> Result<()> {
        IDirectDraw_Impl::Initialize(self, a)
    }
    fn RestoreDisplayMode(&self) -> Result<()> {
        Ok(())
    }
    fn SetCooperativeLevel(&self, a: HWND, b: u32) -> Result<()> {
        IDirectDraw_Impl::SetCooperativeLevel(self, a, b)
    }
    fn SetDisplayMode(&self, w: u32, h: u32, bpp: u32, _r: u32, _f: u32) -> Result<()> {
        IDirectDraw_Impl::SetDisplayMode(self, w, h, bpp)
    }
    fn WaitForVerticalBlank(&self, a: u32, b: HANDLE) -> Result<()> {
        IDirectDraw_Impl::WaitForVerticalBlank(self, a, b)
    }
    fn GetAvailableVidMem(&self, _a: *mut DDSCAPS2, b: *mut u32, c: *mut u32) -> Result<()> {
        if !b.is_null() {
            unsafe { *b = 16 * 1024 * 1024 };
        }
        if !c.is_null() {
            unsafe { *c = 16 * 1024 * 1024 };
        }
        Ok(())
    }
    fn GetSurfaceFromDC(&self, _hdc: HDC) -> Result<IDirectDrawSurface7> {
        Err(Error::from(HRESULT(DXERR_GENERIC as i32)))
    }
    fn RestoreAllSurfaces(&self) -> Result<()> {
        Ok(())
    }
    fn TestCooperativeLevel(&self) -> Result<()> {
        if crate::fps_limiter::limiter_applies(crate::fps_limiter::LIMIT_TEST_COOPERATIVE_LEVEL) {
            crate::fps_limiter::wait_game_tick();
        }
        Ok(())
    }
    fn GetDeviceIdentifier(&self, _a: *mut DDDEVICEIDENTIFIER2, _b: u32) -> Result<()> {
        Ok(())
    }
    fn StartModeTest(&self, _a: *mut SIZE, _b: u32, _c: u32) -> Result<()> {
        Ok(())
    }
    fn EvaluateMode(&self, _a: u32, _b: *mut u32) -> Result<()> {
        Ok(())
    }
}

/// Adjust a window rect using the window's current styles.
unsafe fn adjust_window_rect(rc: &mut RECT, hwnd: HWND) {
    let style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::GWL_STYLE,
    ) as u32;
    let ex_style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
    ) as u32;
    let _ = AdjustWindowRectEx(
        rc,
        windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
        false,
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(ex_style),
    );
}
