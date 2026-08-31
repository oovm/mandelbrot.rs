//! DirectDraw surface implementation (ports `IDirectDrawSurface.c`).
//!
//! Each surface owns a GDI DIB section so the game can write pixels directly
//! via `Lock`/`GetDC`, and the renderer thread can read the same memory to
//! present it to the screen.
//!
//! The surface is exposed through every DirectDraw version (1..7) because
//! games `QueryInterface` for the version they were built against (RA2 uses
//! the v3/v4 interfaces) and `CreateSurface` itself `cast`s the v1 object up
//! to v4/v7.

use std::sync::Arc;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, GetDC,
    HDC, HGDIOBJ, RGBQUAD, RGNDATA, RGNDATAHEADER, ReleaseDC, SelectObject, SetDIBColorTable,
};
use windows::core::*;

use crate::state::SurfaceBuffers;

#[implement(
    IDirectDrawSurface7,
    IDirectDrawSurface4,
    IDirectDrawSurface3,
    IDirectDrawSurface2,
    IDirectDrawSurface,
    IDirectDrawGammaControl
)]
pub struct SurfaceImpl {
    pub width: i32,
    pub height: i32,
    pub bpp: i32,
    pub pitch: i32,
    pub caps: u32,
    pub is_primary: bool,
    pub buffers: Arc<SurfaceBuffers>,
    pub backbuffer: Option<Arc<SurfaceBuffers>>,
    pub color_key_src: std::cell::RefCell<Option<u32>>,
    pub color_key_dest: std::cell::RefCell<Option<u32>>,
    pub clipper: std::cell::RefCell<Option<IDirectDrawClipper>>,
    pub palette_handle: std::cell::RefCell<Option<IDirectDrawPalette>>,
}

unsafe impl Send for SurfaceImpl {}
unsafe impl Sync for SurfaceImpl {}

fn pixel_type(bpp: i32) -> u32 {
    match bpp {
        8 => DDPF_PALETTEINDEXED8 as u32 | DDPF_RGB as u32,
        _ => DDPF_RGB as u32,
    }
}

fn fill_pixel_format(pf: &mut DDPIXELFORMAT, bpp: i32) {
    pf.dwSize = std::mem::size_of::<DDPIXELFORMAT>() as u32;
    pf.dwFlags = pixel_type(bpp);
    pf.Anonymous1.dwRGBBitCount = bpp as u32;
    if bpp == 16 {
        if crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed) {
            pf.Anonymous2.dwRBitMask = 0x7C00;
            pf.Anonymous3.dwGBitMask = 0x03E0;
            pf.Anonymous4.dwBBitMask = 0x001F;
        } else {
            pf.Anonymous2.dwRBitMask = 0xF800;
            pf.Anonymous3.dwGBitMask = 0x07E0;
            pf.Anonymous4.dwBBitMask = 0x001F;
        }
    }
}

/// Extra DIB scanlines allocated below the surface so buggy games that write
/// past the bottom of the framebuffer (e.g. Tiberian Sun) don't corrupt the
/// heap. Mirrors ts-ddraw's `guardLines = 200`.
const GUARD_LINES: i32 = 200;

/// Build a DIB section backing store for a surface.
unsafe fn make_buffers(hdc: HDC, width: i32, height: i32, bpp: i32) -> SurfaceBuffers {
    let bytes_pp = (bpp / 8).max(1);
    let pitch = width * bytes_pp;
    let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
    // 8-bit surfaces carry a 256-entry color table (RGBQUAD) after the header;
    // higher bit depths use the 3 DWORD bitmask block instead.
    let table_bytes = if bpp == 8 { 256 * std::mem::size_of::<RGBQUAD>() } else { 12 };
    let mut buf = vec![0u8; header_size + table_bytes];

    let dev_height = height + GUARD_LINES;
    let bmh = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -dev_height,
        biPlanes: 1,
        biBitCount: bpp as u16,
        biCompression: if bpp == 8 { BI_RGB.0 } else { BI_BITFIELDS.0 },
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: if bpp == 8 { 256 } else { 0 },
        biClrImportant: 0,
    };
    let bmh_ptr = buf.as_mut_ptr() as *mut BITMAPINFOHEADER;
    *bmh_ptr = bmh;

    if bpp == 8 {
        // Identity grayscale color table so GDI has valid colors until the
        // palette is installed via SetDIBColorTable.
        let table_ptr = buf.as_mut_ptr().add(header_size) as *mut RGBQUAD;
        for i in 0..256u32 {
            std::ptr::write(
                table_ptr.add(i as usize),
                RGBQUAD { rgbBlue: i as u8, rgbGreen: i as u8, rgbRed: i as u8, rgbReserved: 0 },
            );
        }
    } else {
        let masks: [u32; 3] = if bpp == 32 {
            // 32-bit BGRX (matches GL_BGRA upload order).
            [0x00FF0000, 0x0000FF00, 0x000000FF]
        } else if crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed) {
            [0x7C00, 0x03E0, 0x001F]
        } else {
            [0xF800, 0x07E0, 0x001F]
        };
        let mask_ptr = buf.as_mut_ptr().add(header_size) as *mut u32;
        std::ptr::copy_nonoverlapping(masks.as_ptr(), mask_ptr, 3);
    }

    let base = if hdc.is_invalid() { GetDC(None) } else { hdc };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bitmap =
        CreateDIBSection(Some(base), buf.as_ptr() as *const BITMAPINFO, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
    let memdc = CreateCompatibleDC(Some(base));
    let default_bm = SelectObject(memdc, HGDIOBJ(bitmap.0));
    if hdc.is_invalid() {
        let _ = ReleaseDC(None, base);
    }

    SurfaceBuffers {
        hdc: memdc,
        bitmap,
        default_bm,
        surface: bits as *mut u8,
        width,
        height,
        pitch,
        bpp,
        using_pbo: false,
        lock: crate::state::ReentrantLock::new(),
    }
}

/// Write the active palette (as `[B, G, R, Flags]` entries) into an 8-bit DIB's
/// color table so GDI and the palette expose the current colors.
fn apply_palette_to_dib(buffers: &SurfaceBuffers) {
    if buffers.bpp != 8 {
        return;
    }
    if let Some(entries) = crate::state::active_palette_entries() {
        let mut table = [RGBQUAD::default(); 256];
        for (i, e) in entries.iter().enumerate() {
            table[i] = RGBQUAD { rgbBlue: e[0], rgbGreen: e[1], rgbRed: e[2], rgbReserved: 0 };
        }
        unsafe {
            SetDIBColorTable(buffers.hdc, 0, &table);
        }
    }
}

impl SurfaceImpl {
    pub fn new(hdc: HDC, width: i32, height: i32, bpp: i32, is_primary: bool) -> Self {
        let buffers = unsafe { Arc::new(make_buffers(hdc, width, height, bpp)) };
        let caps = if is_primary {
            DDSCAPS_PRIMARYSURFACE as u32 | DDSCAPS_VIDEOMEMORY as u32
        } else {
            DDSCAPS_OFFSCREENPLAIN as u32 | DDSCAPS_SYSTEMMEMORY as u32
        };
        Self {
            width,
            height,
            bpp,
            pitch: buffers.pitch,
            caps,
            is_primary,
            buffers,
            backbuffer: None,
            color_key_src: std::cell::RefCell::new(None),
            color_key_dest: std::cell::RefCell::new(None),
            clipper: std::cell::RefCell::new(None),
            palette_handle: std::cell::RefCell::new(None),
        }
    }

    /// Attach a back buffer (shares a separate DIB with the primary).
    pub fn with_backbuffer(mut self, hdc: HDC, width: i32, height: i32, bpp: i32) -> Self {
        let bb = unsafe { Arc::new(make_buffers(hdc, width, height, bpp)) };
        self.backbuffer = Some(bb);
        self
    }

    fn from_buffers(buffers: Arc<SurfaceBuffers>, width: i32, height: i32, bpp: i32, is_primary: bool) -> Self {
        let caps = if is_primary {
            DDSCAPS_PRIMARYSURFACE as u32 | DDSCAPS_VIDEOMEMORY as u32
        } else {
            DDSCAPS_OFFSCREENPLAIN as u32 | DDSCAPS_SYSTEMMEMORY as u32
        };
        Self {
            width,
            height,
            bpp,
            pitch: buffers.pitch,
            caps,
            is_primary,
            buffers,
            backbuffer: None,
            color_key_src: std::cell::RefCell::new(None),
            color_key_dest: std::cell::RefCell::new(None),
            clipper: std::cell::RefCell::new(None),
            palette_handle: std::cell::RefCell::new(None),
        }
    }

    // ---- shared implementation helpers (use the v7/DDSURFACEDESC2 types) ----

    /// The game may read/write this surface's memory directly (instead of via
    /// the GPU/backbuffer path) when it is a system-memory surface OR when the
    /// `tshack` config is set (cnc-ddraw ddsurface.c:1476). Our DIB backing is
    /// always system memory, so this simply means: expose the raw bits directly.
    fn tshack_active(&self) -> bool {
        let tshack = crate::state::state().lock().unwrap().tshack;
        if (self.caps & DDSCAPS_SYSTEMMEMORY as u32) != 0 || tshack {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::dd_log!(
                    "Surface::tshack: direct system-memory access (caps={:#x}, tshack={})",
                    self.caps,
                    tshack
                );
            }
            true
        } else {
            false
        }
    }

    /// Inject the game-speed limiter at a given call site (cnc-ddraw
    /// `fps_limiter.c` injection points). Only throttles when the configured
    /// limiter matches, matching `limiter_type` semantics.
    fn throttle(&self, method: i32) {
        if crate::fps_limiter::limiter_applies(method) {
            crate::fps_limiter::wait_game_tick();
        }
    }

    /// `lock_surfaces` (cnc-ddraw): keep a persistent system-memory copy so a
    /// subsequent `Lock` returns it directly without any GPU re-acquire. Our DIB
    /// section already persists for the surface's lifetime; this simply records
    /// the (already persistent) backing and logs the first use.
    fn persistent_backing(&self) -> Option<*mut u8> {
        let lock_surfaces = crate::state::state().lock().unwrap().lock_surfaces;
        if !lock_surfaces {
            return None;
        }
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::dd_log!("Surface::lock_surfaces: persistent Lock buffer active");
        }
        Some(self.buffers.surface)
    }

    /// `flipclear` (cnc-ddraw): on Flip clear a surface's backing to pure black
    /// so unpainted regions don't show stale pixels. `target` is the buffer that
    /// will be drawn next (the back buffer, or the primary when unflipped). Logs
    /// the first use.
    fn flip_clear(&self, target: &SurfaceBuffers) {
        if !crate::state::state().lock().unwrap().flipclear {
            return;
        }
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::dd_log!("Surface::flipclear: clearing frame to black");
        }
        let _g = target.lock.lock();
        unsafe {
            std::ptr::write_bytes(target.surface, 0, (self.pitch as usize) * (self.height as usize));
        }
    }

    unsafe fn fill_desc2(&self, desc: &mut DDSURFACEDESC2, pixels: *mut u8) {
        desc.dwSize = std::mem::size_of::<DDSURFACEDESC2>() as u32;
        desc.dwFlags = (DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT | DDSD_LPSURFACE) as u32;
        desc.dwWidth = self.width as u32;
        desc.dwHeight = self.height as u32;
        desc.Anonymous1.lPitch = self.pitch;
        desc.lpSurface = pixels as *mut core::ffi::c_void;
        desc.ddsCaps.dwCaps = self.caps;
        fill_pixel_format(&mut desc.Anonymous5.ddpfPixelFormat, self.bpp);
    }

    fn lock_impl2(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, _flags: u32, _event: HANDLE) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // Acquire the surface lock and hold it until Unlock, matching
        // cnc-ddraw's `lock_surfaces`. While held, the game writes pixels into
        // the surface; the renderer thread also takes this lock during upload,
        // so it can only ever read a complete frame (no torn/blank captures).
        self.buffers.lock.acquire();
        // The surface's backing is always system-memory DIB bits that the game
        // may write directly. `tshack` (or a system-memory surface) forces this
        // direct read/write path; otherwise we route through the GPU path below
        // (which here is the same DIB, but we honour the flag for parity).
        let pixels = if self.tshack_active() {
            self.buffers.surface
        } else if let Some(p) = self.persistent_backing() {
            p
        } else {
            self.buffers.surface
        };
        unsafe {
            self.fill_desc2(&mut *desc, pixels);
            if !rect.is_null() {
                let r = &*rect;
                let bytes_pp = (self.bpp / 8).max(1);
                let offset = (r.top * self.pitch) + (r.left * bytes_pp);
                (*desc).lpSurface = pixels.add(offset as usize) as *mut core::ffi::c_void;
                (*desc).dwWidth = (r.right - r.left) as u32;
                (*desc).dwHeight = (r.bottom - r.top) as u32;
            }
        }
        Ok(())
    }

    fn lock_impl1(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, _flags: u32, _event: HANDLE) -> Result<()> {
        let mut d2 = DDSURFACEDESC2::default();
        self.lock_impl2(rect, &mut d2, 0, HANDLE(std::ptr::null_mut()))?;
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
            (*desc).dwFlags = d2.dwFlags;
            (*desc).dwHeight = d2.dwHeight;
            (*desc).dwWidth = d2.dwWidth;
            (*desc).Anonymous1.lPitch = d2.Anonymous1.lPitch;
            (*desc).lpSurface = d2.lpSurface;
            (*desc).ddsCaps.dwCaps = d2.ddsCaps.dwCaps;
            fill_pixel_format(&mut (*desc).ddpfPixelFormat, self.bpp);
        }
        Ok(())
    }

    fn get_surface_desc_impl2(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let _g = self.buffers.lock.lock();
        unsafe { self.fill_desc2(&mut *desc, self.buffers.surface) };
        Ok(())
    }

    fn get_surface_desc_impl1(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        if desc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let _g = self.buffers.lock.lock();
        let mut d2 = DDSURFACEDESC2::default();
        unsafe { self.fill_desc2(&mut d2, self.buffers.surface) };
        unsafe {
            (*desc).dwSize = std::mem::size_of::<DDSURFACEDESC>() as u32;
            (*desc).dwFlags = d2.dwFlags;
            (*desc).dwHeight = d2.dwHeight;
            (*desc).dwWidth = d2.dwWidth;
            (*desc).Anonymous1.lPitch = d2.Anonymous1.lPitch;
            (*desc).lpSurface = d2.lpSurface;
            (*desc).ddsCaps.dwCaps = d2.ddsCaps.dwCaps;
            fill_pixel_format(&mut (*desc).ddpfPixelFormat, self.bpp);
        }
        Ok(())
    }

    fn get_caps_impl2(&self, caps: *mut DDSCAPS2) -> Result<()> {
        if !caps.is_null() {
            unsafe { (*caps).dwCaps = self.caps };
        }
        Ok(())
    }

    fn get_caps_impl1(&self, caps: *mut DDSCAPS) -> Result<()> {
        if !caps.is_null() {
            unsafe { (*caps).dwCaps = self.caps };
        }
        Ok(())
    }

    fn get_attached_impl2(&self, caps: *mut DDSCAPS2) -> Result<IDirectDrawSurface7> {
        if caps.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let requested = unsafe { (*caps).dwCaps };
        if (requested & DDSCAPS_BACKBUFFER as u32) != 0
            && let Some(ref bb) = self.backbuffer
        {
            let mut s = SurfaceImpl::from_buffers(bb.clone(), self.width, self.height, self.bpp, false);
            s.caps = (DDSCAPS_BACKBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY) as u32;
            let surface: IDirectDrawSurface = s.into();
            return surface.cast::<IDirectDrawSurface7>();
        }
        Err(dderr(DXERR_GENERIC))
    }

    fn get_attached_impl1(&self, caps: *mut DDSCAPS) -> Result<IDirectDrawSurface7> {
        if caps.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let requested = unsafe { (*caps).dwCaps };
        if (requested & DDSCAPS_BACKBUFFER as u32) != 0
            && let Some(ref bb) = self.backbuffer
        {
            let mut s = SurfaceImpl::from_buffers(bb.clone(), self.width, self.height, self.bpp, false);
            s.caps = (DDSCAPS_BACKBUFFER | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY) as u32;
            let surface: IDirectDrawSurface = s.into();
            return surface.cast::<IDirectDrawSurface7>();
        }
        Err(dderr(DXERR_GENERIC))
    }

    fn get_dc_impl(&self, hdc: *mut HDC) -> Result<()> {
        if hdc.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *hdc = self.buffers.hdc };
        Ok(())
    }

    fn get_pixel_format_impl(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        if pf.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { fill_pixel_format(&mut *pf, self.bpp) };
        Ok(())
    }

    fn flip_impl(&self) -> Result<()> {
        let copied_bb = if let Some(ref bb) = self.backbuffer {
            {
                let _g = self.buffers.lock.lock();
                let _bg = bb.lock.lock();
                unsafe {
                    let dst = self.buffers.surface;
                    let src = bb.surface;
                    let len = (self.pitch as usize) * (self.height as usize);
                    std::ptr::copy_nonoverlapping(src, dst, len);
                }
            }
            // `flipclear` (cnc-ddraw clear_frame): after the back buffer has been
            // presented, clear it to pure black so any region the game doesn't
            // repaint next frame shows black rather than stale pixels.
            self.flip_clear(bb);
            true
        } else {
            self.flip_clear(&self.buffers);
            false
        };
        if self.is_primary {
            crate::state::mark_dirty();
            crate::state::mark_frame_ready();
        }
        let _ = copied_bb;
        Ok(())
    }

    // ---- color key / clipper / palette shared helpers ----

    fn set_color_key_impl(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        if key.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            let low = (*key).dwColorSpaceLowValue;
            if (flags & DDCKEY_SRCBLT as u32) != 0 {
                *self.color_key_src.borrow_mut() = Some(low);
            } else if (flags & DDCKEY_DESTBLT as u32) != 0 {
                *self.color_key_dest.borrow_mut() = Some(low);
            } else {
                *self.color_key_src.borrow_mut() = Some(low);
            }
        }
        Ok(())
    }

    fn get_color_key_impl(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        if key.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let v = if (flags & DDCKEY_DESTBLT as u32) != 0 {
            self.color_key_dest.borrow().as_ref().copied()
        } else {
            self.color_key_src.borrow().as_ref().copied()
        };
        if let Some(v) = v {
            unsafe {
                (*key).dwColorSpaceLowValue = v;
                (*key).dwColorSpaceHighValue = v;
            }
        }
        Ok(())
    }

    fn set_clipper_impl(&self, c: Option<&IDirectDrawClipper>) -> Result<()> {
        if let Some(c) = c {
            *self.clipper.borrow_mut() = Some(c.clone());
        } else {
            *self.clipper.borrow_mut() = None;
        }
        Ok(())
    }

    fn get_clipper_impl(&self) -> Result<IDirectDrawClipper> {
        self.clipper.borrow().clone().ok_or_else(|| dderr(DXERR_GENERIC))
    }

    fn set_palette_impl(&self, p: Option<&IDirectDrawPalette>) -> Result<()> {
        if let Some(p) = p {
            *self.palette_handle.borrow_mut() = Some(p.clone());
            crate::state::register_palette(p);
            if self.bpp == 8 {
                apply_palette_to_dib(&self.buffers);
            }
        } else {
            *self.palette_handle.borrow_mut() = None;
        }
        Ok(())
    }

    fn get_palette_impl(&self) -> Result<IDirectDrawPalette> {
        self.palette_handle.borrow().clone().ok_or_else(|| dderr(DXERR_GENERIC))
    }

    /// BltFast: blit the whole source rectangle to `(x, y)`, honouring the
    /// DDBLTFAST_* transparent flags.
    fn blt_fast_impl(&self, x: u32, y: u32, src: Option<IDirectDrawSurface7>, r: *mut RECT, trans: u32) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_BLTFAST);
        let s7 = match src {
            Some(s) => s,
            None => return Err(dderr(DXERR_GENERIC)),
        };
        let mut sd = DDSURFACEDESC2::default();
        if unsafe { s7.GetSurfaceDesc(&mut sd) }.is_err() {
            return Err(dderr(DXERR_GENERIC));
        }
        let sw = sd.dwWidth as i32;
        let sh = sd.dwHeight as i32;
        let mut dr = RECT { left: x as i32, top: y as i32, right: x as i32 + sw, bottom: y as i32 + sh };
        let flags = if (trans & DDBLTFAST_SRCCOLORKEY) != 0 {
            DDBLT_KEYSRC as u32
        } else if (trans & DDBLTFAST_DESTCOLORKEY) != 0 {
            DDBLT_KEYDEST as u32
        } else {
            0
        };
        self.blt_impl(&mut dr, Some(s7), r, flags, std::ptr::null_mut())
    }

    fn blt_impl(
        &self,
        dr: *mut RECT,
        src: Option<IDirectDrawSurface7>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        if (flags & (DDBLT_ROP as u32 | DDBLT_DDFX as u32)) != 0 {
            crate::dd_log!("Surface::Blt(flags={:#x})", flags);
        }

        // Resolve the source color key (if requested) from the source surface's
        // own GetColorKey first, then fall back to this surface's stored key.
        let mut src_key = self.color_key_src.borrow().as_ref().copied();
        if (flags & DDBLT_KEYSRC as u32) != 0
            && let Some(ref src_iface) = src
        {
            let mut k = DDCOLORKEY::default();
            if unsafe { src_iface.GetColorKey(DDCKEY_SRCBLT as u32, &mut k) }.is_ok() {
                src_key = Some(k.dwColorSpaceLowValue);
            }
        }
        let dest_key =
            if (flags & DDBLT_KEYDEST as u32) != 0 { self.color_key_dest.borrow().as_ref().copied() } else { None };

        // Copy the source pixels into a temporary buffer while holding ONLY the
        // source lock. Important: if the source is the same surface as `self`
        // (self-blit), locking `self.buffers.lock` here and then re-locking it
        // via `src_iface.Lock` would deadlock a non-reentrant mutex. Releasing
        // the source lock before acquiring `self`'s lock avoids that.
        let src_copy: Option<(Vec<u8>, usize, usize, usize, i32)> = if let Some(src_iface) = src {
            let mut src_desc = DDSURFACEDESC2::default();
            if unsafe { src_iface.Lock(sr, &mut src_desc, 0, HANDLE(std::ptr::null_mut())).is_ok() } {
                let sp = src_desc.lpSurface as *mut u8;
                let spitch = unsafe { src_desc.Anonymous1.lPitch } as usize;
                let sw = src_desc.dwWidth as usize;
                let sh = src_desc.dwHeight as usize;
                let sbpp = unsafe { src_desc.Anonymous5.ddpfPixelFormat.Anonymous1.dwRGBBitCount } as i32;
                let mut data = vec![0u8; sh * spitch];
                for y in 0..sh {
                    unsafe {
                        std::ptr::copy_nonoverlapping(sp.add(y * spitch), data.as_mut_ptr().add(y * spitch), spitch);
                    }
                }
                let _ = unsafe { src_iface.Unlock(std::ptr::null_mut::<RECT>()) };
                Some((data, sw, sh, spitch, sbpp))
            } else {
                None
            }
        } else {
            None
        };

        let _guard = self.buffers.lock.lock();

        let dst_ptr = self.buffers.surface;
        let dst_pitch = self.pitch as usize;
        let dst_w = self.width as usize;
        let dst_h = self.height as usize;
        let bytes_pp = (self.bpp / 8).max(1) as usize;

        let (dx0, dy0, dx1, dy1) = rect_clamp(dr, dst_w, dst_h);

        // Enumerate the clip rectangles if a clipper is attached. GetClipList
        // returns the required size when passed a null buffer; we then fetch the
        // RGNDATA and read its RECTs. On failure we fall back to the full
        // destination rect (no clipping).
        let clip_rects: Vec<(usize, usize, usize, usize)> = if let Some(cl) = (*self.clipper.borrow()).clone() {
            let mut size = 0u32;
            let mut rects = Vec::new();
            if unsafe { cl.GetClipList(std::ptr::null_mut(), std::ptr::null_mut(), &mut size).is_ok() }
                && size >= std::mem::size_of::<RGNDATAHEADER>() as u32
            {
                static MAX_CLIP_RECTS: usize = 4096;
                let nrects = ((size as usize - std::mem::size_of::<RGNDATAHEADER>()) / std::mem::size_of::<RECT>())
                    .min(MAX_CLIP_RECTS);
                if nrects > 0 {
                    let hdr_size = std::mem::size_of::<RGNDATAHEADER>();
                    let mut buf = vec![0u8; hdr_size + nrects * std::mem::size_of::<RECT>()];
                    let mut got = size;
                    if unsafe {
                        cl.GetClipList(std::ptr::null_mut(), buf.as_mut_ptr() as *mut RGNDATA, &mut got).is_ok()
                    } {
                        unsafe {
                            let header = &*(buf.as_ptr() as *const RGNDATAHEADER);
                            let count = (header.nCount as usize).min(nrects);
                            let rp = buf.as_ptr().add(hdr_size) as *const RECT;
                            for i in 0..count {
                                let r = &*rp.add(i);
                                if r.left < r.right && r.top < r.bottom {
                                    rects.push((r.left as usize, r.top as usize, r.right as usize, r.bottom as usize));
                                }
                            }
                        }
                    }
                }
            }
            rects
        } else {
            Vec::new()
        };

        // COLORFILL: write a solid color respecting bpp and clipping.
        if (flags & DDBLT_COLORFILL as u32) != 0 && !fx.is_null() {
            let fill = unsafe { (*fx).Anonymous5.dwFillColor };
            let fill_bytes: [u8; 4] = fill.to_le_bytes();
            if clip_rects.is_empty() {
                self.fill_region(dst_ptr, dst_pitch, dx0, dy0, dx1, dy1, &fill_bytes[..bytes_pp]);
            } else {
                for &(cx0, cy0, cx1, cy1) in &clip_rects {
                    if let Some((sx, sy, ex, ey)) = intersect_rect((dx0, dy0, dx1, dy1), (cx0, cy0, cx1, cy1)) {
                        self.fill_region(dst_ptr, dst_pitch, sx, sy, ex, ey, &fill_bytes[..bytes_pp]);
                    }
                }
            }
        }

        if let Some((data, src_w, src_h, src_pitch, src_bpp)) = src_copy {
            let (sx0, sy0, sx1, sy1) = rect_clamp(sr, src_w, src_h);
            if clip_rects.is_empty() {
                unsafe {
                    self.blit_region(
                        dst_ptr, dst_pitch, &data, src_pitch, src_bpp, dx0, dy0, dx1, dy1, sx0, sy0, sx1, sy1, dx0,
                        dy0, dx1, dy1, src_key, dest_key,
                    );
                }
            } else {
                for &(cx0, cy0, cx1, cy1) in &clip_rects {
                    if let Some((sx, sy, ex, ey)) = intersect_rect((dx0, dy0, dx1, dy1), (cx0, cy0, cx1, cy1)) {
                        unsafe {
                            self.blit_region(
                                dst_ptr, dst_pitch, &data, src_pitch, src_bpp, dx0, dy0, dx1, dy1, sx0, sy0, sx1, sy1,
                                sx, sy, ex, ey, src_key, dest_key,
                            );
                        }
                    }
                }
            }
        }

        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }

    /// Solid-fill a destination rectangle with `bytes` (already `bytes_pp` wide).
    #[allow(clippy::too_many_arguments)]
    fn fill_region(
        &self,
        dst_ptr: *mut u8,
        dst_pitch: usize,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        bytes: &[u8],
    ) {
        if x1 <= x0 || y1 <= y0 || bytes.is_empty() {
            return;
        }
        let n = bytes.len();
        for y in y0..y1 {
            unsafe {
                let row = dst_ptr.add(y * dst_pitch + x0 * n);
                for x in 0..(x1 - x0) {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), row.add(x * n), n);
                }
            }
        }
    }

    /// Copy pixels from a source snapshot into `self`, mapping the destination
    /// rect `(dx0,dy0)-(dx1,dy1)` from source rect `(sx0,sy0)-(sx1,sy1)` using
    /// nearest-neighbour scaling. Only the sub-rect `(subx0,suby0)-(subx1,suby1)`
    /// (a portion of the dest rect) is written, so clipped blits align with the
    /// full scaling factor. Applies source/dest color keys if requested.
    #[allow(clippy::too_many_arguments)]
    unsafe fn blit_region(
        &self,
        dst_ptr: *mut u8,
        dst_pitch: usize,
        src_data: &[u8],
        src_pitch: usize,
        src_bpp: i32,
        dx0: usize,
        dy0: usize,
        dx1: usize,
        dy1: usize,
        sx0: usize,
        sy0: usize,
        sx1: usize,
        sy1: usize,
        subx0: usize,
        suby0: usize,
        subx1: usize,
        suby1: usize,
        src_key: Option<u32>,
        dest_key: Option<u32>,
    ) {
        if subx1 <= subx0 || suby1 <= suby0 {
            return;
        }
        let dw = dx1.saturating_sub(dx0);
        let dh = dy1.saturating_sub(dy0);
        let sw = sx1.saturating_sub(sx0);
        let sh = sy1.saturating_sub(sy0);
        if dw == 0 || dh == 0 {
            return;
        }

        let src_bpp_us = (src_bpp.max(8) / 8) as usize;
        let dst_bpp_us = (self.bpp.max(8) / 8) as usize;
        let rgb555 = crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed);

        // Fast path: identical formats, 1:1, no keys, and no clipping/sub-rect.
        if sw == dw
            && sh == dh
            && src_bpp_us == dst_bpp_us
            && src_key.is_none()
            && dest_key.is_none()
            && subx0 == dx0
            && suby0 == dy0
            && subx1 == dx1
            && suby1 == dy1
        {
            for y in suby0..suby1 {
                let d = dst_ptr.add(y * dst_pitch + subx0 * dst_bpp_us);
                let s = src_data.as_ptr().add((sy0 + (y - dy0)) * src_pitch + (sx0 + (subx0 - dx0)) * src_bpp_us);
                std::ptr::copy_nonoverlapping(s, d, (subx1 - subx0) * dst_bpp_us);
            }
            return;
        }

        let palette = if src_bpp == 8 { crate::state::active_palette_entries() } else { None };

        for y in suby0..suby1 {
            // Map this dest row back to a source row.
            let sy = sy0 + ((y - dy0) * sh) / dh;
            let srow = src_data.as_ptr().add(sy * src_pitch);
            let drow = dst_ptr.add(y * dst_pitch);
            for x in subx0..subx1 {
                let sx = sx0 + ((x - dx0) * sw) / dw;

                // Read the source pixel, applying the source color key.
                let packed = read_pixel(srow.add(sx * src_bpp_us), src_bpp);
                if let Some(k) = src_key {
                    let kk = match src_bpp {
                        8 => k & 0xFF,
                        16 => k & 0xFFFF,
                        _ => k,
                    };
                    if packed.0 == kk {
                        continue;
                    }
                }

                // Destination color key: skip if the current dest pixel matches.
                if let Some(k) = dest_key {
                    let dp = drow.add(x * dst_bpp_us);
                    let kk = match self.bpp {
                        8 => k & 0xFF,
                        16 => k & 0xFFFF,
                        _ => k,
                    };
                    if read_pixel(dp, self.bpp).0 == kk {
                        continue;
                    }
                }

                // Expand the source pixel into (b, g, r).
                let (b, g, r) = match src_bpp {
                    8 => {
                        let i = packed.0 as usize & 0xFF;
                        match &palette {
                            Some(p) if i < 256 => (p[i][0], p[i][1], p[i][2]),
                            _ => (i as u8, i as u8, i as u8),
                        }
                    }
                    16 => {
                        let v = packed.0 as u16;
                        if rgb555 {
                            (
                                (((v & 0x7C00) >> 10) << 3) as u8,
                                (((v & 0x03E0) >> 5) << 3) as u8,
                                ((v & 0x001F) << 3) as u8,
                            )
                        } else {
                            (
                                (((v & 0xF800) >> 11) << 3) as u8,
                                (((v & 0x07E0) >> 5) << 2) as u8,
                                ((v & 0x001F) << 3) as u8,
                            )
                        }
                    }
                    _ => (packed.1, packed.2, packed.3),
                };

                // Write the expanded pixel into the destination.
                let dp = drow.add(x * dst_bpp_us);
                match self.bpp {
                    8 => {
                        *dp = packed.0 as u8;
                    }
                    16 => {
                        let val = if rgb555 {
                            (((r as u32) >> 3) << 10) | (((g as u32) >> 3) << 5) | ((b as u32) >> 3)
                        } else {
                            (((r as u32) >> 3) << 11) | (((g as u32) >> 2) << 5) | ((b as u32) >> 3)
                        };
                        *(dp as *mut u16) = val as u16;
                    }
                    32 => {
                        let val = 0xFF000000u32 | (b as u32) | ((g as u32) << 8) | ((r as u32) << 16);
                        *(dp as *mut u32) = val;
                    }
                    _ => {
                        *dp = packed.0 as u8;
                    }
                }
            }
        }
    }
}

fn dderr(code: u32) -> Error {
    Error::from(HRESULT(code as i32))
}

/// Returns clamped (x0, y0, x1, y1) for a rectangle pointer (or whole surface).
fn rect_clamp(r: *mut RECT, w: usize, h: usize) -> (usize, usize, usize, usize) {
    if r.is_null() {
        return (0, 0, w, h);
    }
    unsafe {
        let rc = &*r;
        let x0 = rc.left.max(0) as usize;
        let y0 = rc.top.max(0) as usize;
        let x1 = (rc.right as usize).min(w);
        let y1 = (rc.bottom as usize).min(h);
        (x0, y0, x1.max(x0), y1.max(y0))
    }
}

/// Intersect two clamped (x0, y0, x1, y1) rectangles; `None` if disjoint.
fn intersect_rect(
    a: (usize, usize, usize, usize),
    b: (usize, usize, usize, usize),
) -> Option<(usize, usize, usize, usize)> {
    let (ax0, ay0, ax1, ay1) = a;
    let (bx0, by0, bx1, by1) = b;
    let x0 = ax0.max(bx0);
    let y0 = ay0.max(by0);
    let x1 = ax1.min(bx1);
    let y1 = ay1.min(by1);
    if x1 > x0 && y1 > y0 { Some((x0, y0, x1, y1)) } else { None }
}

/// Read a pixel from `ptr` as its packed value and its `(B, G, R)` bytes.
/// Returns `(packed, b, g, r)`. For 8-bit packed is the palette index; for
/// 16-bit packed is the raw u16; for 32-bit packed is the raw BGRA u32.
unsafe fn read_pixel(ptr: *const u8, bpp: i32) -> (u32, u8, u8, u8) {
    match bpp {
        8 => {
            let v = *ptr as u32;
            (v, v as u8, v as u8, v as u8)
        }
        16 => {
            let v = *(ptr as *const u16) as u32;
            (v, 0, 0, 0)
        }
        32 => {
            let v = *(ptr as *const u32);
            let b = (v & 0xFF) as u8;
            let g = ((v >> 8) & 0xFF) as u8;
            let r = ((v >> 16) & 0xFF) as u8;
            (v, b, g, r)
        }
        _ => {
            let v = *ptr as u32;
            (v, v as u8, v as u8, v as u8)
        }
    }
}

// ===========================================================================
// IDirectDrawSurface (v1)
// ===========================================================================
impl IDirectDrawSurface_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, x: u32, y: u32, src: Ref<'_, IDirectDrawSurface>, r: *mut RECT, t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_fast_impl(x, y, s7, r, t)
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        self.get_clipper_impl()
    }
    fn GetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.get_color_key_impl(flags, key)
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Ok(())
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        self.get_palette_impl()
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        self.set_clipper_impl(c.as_ref())
    }
    fn SetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.set_color_key_impl(flags, key)
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn SetPalette(&self, p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        self.set_palette_impl(p.as_ref())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_UNLOCK);
        self.buffers.lock.release();
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface>) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface2 (v2) -- same descriptors as v1, extra methods
// ===========================================================================
impl IDirectDrawSurface2_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface2>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, x: u32, y: u32, src: Ref<'_, IDirectDrawSurface2>, r: *mut RECT, t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_fast_impl(x, y, s7, r, t)
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface2>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface2>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface2 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        self.get_clipper_impl()
    }
    fn GetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.get_color_key_impl(flags, key)
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Ok(())
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        self.get_palette_impl()
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        self.set_clipper_impl(c.as_ref())
    }
    fn SetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.set_color_key_impl(flags, key)
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn SetPalette(&self, p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        self.set_palette_impl(p.as_ref())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_UNLOCK);
        self.buffers.lock.release();
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface2>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface2>) -> Result<()> {
        Ok(())
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface3 (v3) -- DDSURFACEDESC/DDSCAPS, extra methods
// ===========================================================================
impl IDirectDrawSurface3_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface3>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, x: u32, y: u32, src: Ref<'_, IDirectDrawSurface3>, r: *mut RECT, t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_fast_impl(x, y, s7, r, t)
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface3>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS, out: OutRef<'_, IDirectDrawSurface3>) -> Result<()> {
        let s7 = self.get_attached_impl1(caps)?;
        let s: IDirectDrawSurface3 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS) -> Result<()> {
        self.get_caps_impl1(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        self.get_clipper_impl()
    }
    fn GetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.get_color_key_impl(flags, key)
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Ok(())
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        self.get_palette_impl()
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC) -> Result<()> {
        self.get_surface_desc_impl1(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl1(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        self.set_clipper_impl(c.as_ref())
    }
    fn SetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.set_color_key_impl(flags, key)
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn SetPalette(&self, p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        self.set_palette_impl(p.as_ref())
    }
    fn Unlock(&self, _r: *mut core::ffi::c_void) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_UNLOCK);
        self.buffers.lock.release();
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface3>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface3>) -> Result<()> {
        Ok(())
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC, _f: u32) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface4 (v4) -- DDSURFACEDESC2/DDSCAPS2
// ===========================================================================
impl IDirectDrawSurface4_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface4>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, x: u32, y: u32, src: Ref<'_, IDirectDrawSurface4>, r: *mut RECT, t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_fast_impl(x, y, s7, r, t)
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK2) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK2) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface4>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS2, out: OutRef<'_, IDirectDrawSurface4>) -> Result<()> {
        let s7 = self.get_attached_impl2(caps)?;
        let s: IDirectDrawSurface4 = s7.cast()?;
        let _ = out.write(Some(s));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS2) -> Result<()> {
        self.get_caps_impl2(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        self.get_clipper_impl()
    }
    fn GetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.get_color_key_impl(flags, key)
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Ok(())
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        self.get_palette_impl()
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        self.get_surface_desc_impl2(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC2) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl2(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        self.set_clipper_impl(c.as_ref())
    }
    fn SetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.set_color_key_impl(flags, key)
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn SetPalette(&self, p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        self.set_palette_impl(p.as_ref())
    }
    fn Unlock(&self, _r: *mut RECT) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_UNLOCK);
        self.buffers.lock.release();
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface4>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface4>) -> Result<()> {
        Ok(())
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC2, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: *mut u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn FreePrivateData(&self, _g: *const GUID) -> Result<()> {
        Ok(())
    }
    fn GetUniquenessValue(&self, v: *mut u32) -> Result<()> {
        if !v.is_null() {
            unsafe { *v = 0 }
        }
        Ok(())
    }
    fn ChangeUniquenessValue(&self) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawSurface7 (v7) -- DDSURFACEDESC2/DDSCAPS2
// ===========================================================================
impl IDirectDrawSurface7_Impl for SurfaceImpl_Impl {
    fn AddAttachedSurface(&self, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Ok(())
    }
    fn AddOverlayDirtyRect(&self, _r: *mut RECT) -> Result<()> {
        Ok(())
    }
    fn Blt(
        &self,
        dr: *mut RECT,
        src: Ref<'_, IDirectDrawSurface7>,
        sr: *mut RECT,
        flags: u32,
        fx: *mut DDBLTFX,
    ) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_impl(dr, s7, sr, flags, fx)
    }
    fn BltBatch(&self, _b: *mut DDBLTBATCH, _c: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn BltFast(&self, x: u32, y: u32, src: Ref<'_, IDirectDrawSurface7>, r: *mut RECT, t: u32) -> Result<()> {
        let s7 = src.as_ref().and_then(|s| s.cast::<IDirectDrawSurface7>().ok());
        self.blt_fast_impl(x, y, s7, r, t)
    }
    fn DeleteAttachedSurface(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Ok(())
    }
    fn EnumAttachedSurfaces(&self, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK7) -> Result<()> {
        Ok(())
    }
    fn EnumOverlayZOrders(&self, _f: u32, _c: *mut core::ffi::c_void, _cb: LPDDENUMSURFACESCALLBACK7) -> Result<()> {
        Ok(())
    }
    fn Flip(&self, _target: Ref<'_, IDirectDrawSurface7>, _flags: u32) -> Result<()> {
        self.flip_impl()
    }
    fn GetAttachedSurface(&self, caps: *mut DDSCAPS2, out: OutRef<'_, IDirectDrawSurface7>) -> Result<()> {
        let s7 = self.get_attached_impl2(caps)?;
        let _ = out.write(Some(s7));
        Ok(())
    }
    fn GetBltStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetCaps(&self, caps: *mut DDSCAPS2) -> Result<()> {
        self.get_caps_impl2(caps)
    }
    fn GetClipper(&self) -> Result<IDirectDrawClipper> {
        self.get_clipper_impl()
    }
    fn GetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.get_color_key_impl(flags, key)
    }
    fn GetDC(&self, hdc: *mut HDC) -> Result<()> {
        self.get_dc_impl(hdc)
    }
    fn GetFlipStatus(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetOverlayPosition(&self, _x: *mut i32, _y: *mut i32) -> Result<()> {
        Ok(())
    }
    fn GetPalette(&self) -> Result<IDirectDrawPalette> {
        self.get_palette_impl()
    }
    fn GetPixelFormat(&self, pf: *mut DDPIXELFORMAT) -> Result<()> {
        self.get_pixel_format_impl(pf)
    }
    fn GetSurfaceDesc(&self, desc: *mut DDSURFACEDESC2) -> Result<()> {
        self.get_surface_desc_impl2(desc)
    }
    fn Initialize(&self, _dd: Ref<'_, IDirectDraw>, _desc: *mut DDSURFACEDESC2) -> Result<()> {
        Ok(())
    }
    fn IsLost(&self) -> Result<()> {
        Ok(())
    }
    fn Lock(&self, rect: *mut RECT, desc: *mut DDSURFACEDESC2, flags: u32, event: HANDLE) -> Result<()> {
        self.lock_impl2(rect, desc, flags, event)
    }
    fn ReleaseDC(&self, _hdc: HDC) -> Result<()> {
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn Restore(&self) -> Result<()> {
        Ok(())
    }
    fn SetClipper(&self, c: Ref<'_, IDirectDrawClipper>) -> Result<()> {
        self.set_clipper_impl(c.as_ref())
    }
    fn SetColorKey(&self, flags: u32, key: *mut DDCOLORKEY) -> Result<()> {
        self.set_color_key_impl(flags, key)
    }
    fn SetOverlayPosition(&self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn SetPalette(&self, p: Ref<'_, IDirectDrawPalette>) -> Result<()> {
        self.set_palette_impl(p.as_ref())
    }
    fn Unlock(&self, _r: *mut RECT) -> Result<()> {
        self.throttle(crate::fps_limiter::LIMIT_UNLOCK);
        self.buffers.lock.release();
        if self.is_primary {
            crate::state::mark_dirty();
        }
        Ok(())
    }
    fn UpdateOverlay(
        &self,
        _sr: *mut RECT,
        _s: Ref<'_, IDirectDrawSurface7>,
        _dr: *mut RECT,
        _f: u32,
        _fx: *mut DDOVERLAYFX,
    ) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayDisplay(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn UpdateOverlayZOrder(&self, _f: u32, _s: Ref<'_, IDirectDrawSurface7>) -> Result<()> {
        Ok(())
    }
    fn GetDDInterface(&self, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn PageLock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn PageUnlock(&self, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetSurfaceDesc(&self, _d: *mut DDSURFACEDESC2, _f: u32) -> Result<()> {
        Ok(())
    }
    fn SetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: u32, _f: u32) -> Result<()> {
        Ok(())
    }
    fn GetPrivateData(&self, _g: *const GUID, _p: *mut core::ffi::c_void, _s: *mut u32) -> Result<()> {
        Err(dderr(DXERR_GENERIC))
    }
    fn FreePrivateData(&self, _g: *const GUID) -> Result<()> {
        Ok(())
    }
    fn GetUniquenessValue(&self, v: *mut u32) -> Result<()> {
        if !v.is_null() {
            unsafe { *v = 0 }
        }
        Ok(())
    }
    fn ChangeUniquenessValue(&self) -> Result<()> {
        Ok(())
    }
    fn SetPriority(&self, _p: u32) -> Result<()> {
        Ok(())
    }
    fn GetPriority(&self, _p: *mut u32) -> Result<()> {
        Ok(())
    }
    fn SetLOD(&self, _l: u32) -> Result<()> {
        Ok(())
    }
    fn GetLOD(&self, _l: *mut u32) -> Result<()> {
        Ok(())
    }
}

// ===========================================================================
// IDirectDrawGammaControl -- exposed via QueryInterface on the primary surface
// (games QI the primary for `IID_IDirectDrawGammaControl` to set their
// brightness slider). The ramp state is kept process-wide in `gamma.rs`.
// ===========================================================================
impl IDirectDrawGammaControl_Impl for SurfaceImpl_Impl {
    fn GetGammaRamp(&self, _flags: u32, ramp: *mut DDGAMMARAMP) -> Result<()> {
        if ramp.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let r = crate::gamma::get_ramp_identity_filled();
        unsafe {
            std::ptr::copy_nonoverlapping(r.as_ptr(), ramp as *mut u16, 768);
        }
        Ok(())
    }

    fn SetGammaRamp(&self, flags: u32, ramp: *mut DDGAMMARAMP) -> Result<()> {
        if ramp.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut out = [0u16; 768];
        unsafe {
            std::ptr::copy_nonoverlapping(ramp as *const u16, out.as_mut_ptr(), 768);
        }
        crate::gamma::set_ramp(&out);
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::dd_log!("IDirectDrawGammaControl::SetGammaRamp(flags={:#x}, first={:#06x})", flags, out[0]);
        }
        Ok(())
    }
}
