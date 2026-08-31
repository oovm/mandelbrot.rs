//! GDI (software) presentation backend (ports the GDI branch of `render.c`).

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::render::scale;
use crate::state::{SurfaceBuffers, state};

/// Reusable intermediate for software-scaled GDI present. Holds a 32-bit
/// top-down DIB whose DC is Blt'ed 1:1 onto the window when `filter > 0`.
struct GdiScaler {
    mem_dc: Option<HDC>,
    bmp: Option<HBITMAP>,
    bits: *mut u8,
    w: i32,
    h: i32,
    stage: Vec<u32>,
}

unsafe impl Send for GdiScaler {}
unsafe impl Sync for GdiScaler {}

impl GdiScaler {
    fn new() -> Self {
        GdiScaler { mem_dc: None, bmp: None, bits: std::ptr::null_mut(), w: 0, h: 0, stage: Vec::new() }
    }

    /// Scale+convert the (locked) surface into a 32-bit top-down DIB of
    /// `rw x rh` and StretchBlt it (1:1 when the viewport equals the render
    /// size) onto the destination rect.
    #[allow(clippy::too_many_arguments)]
    unsafe fn blit(
        &mut self,
        buffers: &SurfaceBuffers,
        rw: i32,
        rh: i32,
        dst_hdc: HDC,
        dx: i32,
        dy: i32,
        dw: i32,
        dh: i32,
    ) {
        let (filter, rgb555, palette) = {
            let st = state().lock().unwrap();
            let f = st.filter;
            drop(st);
            (f, crate::state::RGB555.load(std::sync::atomic::Ordering::Relaxed), crate::state::active_palette_entries())
        };
        let w = rw.max(1);
        let h = rh.max(1);

        // Read + scale the surface into the staging buffer under its lock.
        let guard = buffers.lock.lock();
        let n = (w as usize) * (h as usize);
        if self.stage.len() < n {
            self.stage.resize(n, 0);
        }
        scale::convert_scale(
            buffers.surface,
            buffers.pitch as usize,
            buffers.width,
            buffers.height,
            buffers.bpp,
            rgb555,
            palette.as_ref(),
            filter,
            &mut self.stage,
            w,
            h,
        );
        drop(guard);

        if self.w != w || self.h != h {
            // Recreate the intermediate DIB when the viewport size changes.
            if let Some(b) = self.bmp {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
            if let Some(dc) = self.mem_dc {
                let _ = DeleteDC(dc);
            }
            self.mem_dc = None;
            self.bmp = None;
            self.bits = std::ptr::null_mut();

            let bmh = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative height => top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let info = BITMAPINFO { bmiHeader: bmh, bmiColors: [RGBQUAD::default(); 1] };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dst_hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
            if let Ok(bmp) = bmp {
                let dc = CreateCompatibleDC(Some(dst_hdc));
                let _ = SelectObject(dc, HGDIOBJ(bmp.0));
                self.bmp = Some(bmp);
                self.mem_dc = Some(dc);
                self.bits = bits as *mut u8;
                self.w = w;
                self.h = h;
            } else {
                crate::dd_log!("gdi: CreateDIBSection(scale) failed");
                return;
            }
        }

        let (dc, bits) = match (self.mem_dc, self.bits.is_null()) {
            (Some(d), false) => (d, self.bits),
            _ => return,
        };

        // Copy the staged BGRA rows into the DIB section (pitch is w*4).
        for y in 0..h {
            let dst_row = bits.add(y as usize * (w as usize * 4));
            let src_row = self.stage.as_ptr().add(y as usize * w as usize) as *const u8;
            std::ptr::copy_nonoverlapping(src_row, dst_row, w as usize * 4);
        }

        let _ = StretchBlt(dst_hdc, dx, dy, dw, dh, Some(dc), 0, 0, w, h, SRCCOPY);
    }
}

/// Present the primary surface to the window using GDI Bit/StretchBlt.
pub(crate) fn present(buffers: &SurfaceBuffers) {
    let (hwnd, dst_hdc, vp, fps, draw_fps) = {
        let st = state().lock().unwrap();
        (st.hwnd, st.hdc, st.render.viewport, st.fps, st.draw_fps)
    };
    if hwnd.is_invalid() || dst_hdc.is_invalid() {
        return;
    }
    let (filter, render_w, render_h) = {
        let st = state().lock().unwrap();
        (st.filter, st.render.width.max(1), st.render.height.max(1))
    };

    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            return;
        }

        // Destination rectangle: the configured viewport, or the full client
        // area when no viewport has been computed yet.
        let (dx, dy, dw, dh) = if vp.right > vp.left && vp.bottom > vp.top {
            (vp.left, vp.top, vp.right - vp.left, vp.bottom - vp.top)
        } else {
            (0, 0, w, h)
        };

        // Only clear the letterbox margins (outside the viewport) to black.
        let full = dx == 0 && dy == 0 && dw == w && dh == h;
        if !full {
            let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            if dy > 0 {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: 0, right: w, bottom: dy }, brush);
            }
            if dy + dh < h {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: dy + dh, right: w, bottom: h }, brush);
            }
            if dx > 0 {
                let _ = FillRect(dst_hdc, &RECT { left: 0, top: dy, right: dx, bottom: dy + dh }, brush);
            }
            if dx + dw < w {
                let _ = FillRect(dst_hdc, &RECT { left: dx + dw, top: dy, right: w, bottom: dy + dh }, brush);
            }
        }

        // 8-bit primaries: refresh the surface DIB's color table from the
        // active palette so GDI interprets the indices with current colors.
        if buffers.bpp == 8
            && let Some(entries) = crate::state::active_palette_entries()
        {
            let mut table = [RGBQUAD::default(); 256];
            for (i, e) in entries.iter().enumerate() {
                table[i] = RGBQUAD { rgbBlue: e[0], rgbGreen: e[1], rgbRed: e[2], rgbReserved: 0 };
            }
            let _ = SetDIBColorTable(buffers.hdc, 0, &table);
        }

        if filter > 0 {
            // Software-scale into a reusable 32-bit DIB, then Blt 1:1 to the
            // viewport. The surface lock is taken inside `blit`.
            static SCALER: std::sync::Mutex<Option<GdiScaler>> = std::sync::Mutex::new(None);
            let mut scaler = SCALER.lock().unwrap();
            let s = scaler.get_or_insert_with(GdiScaler::new);
            s.blit(buffers, render_w, render_h, dst_hdc, dx, dy, dw, dh);
        } else {
            // Read the surface under its lock so we never StretchBlt while the
            // game is mid-Blt into the same DIB (avoids torn frames).
            let _guard = buffers.lock.lock();
            let _ =
                StretchBlt(dst_hdc, dx, dy, dw, dh, Some(buffers.hdc), 0, 0, buffers.width, buffers.height, SRCCOPY);
        }

        if draw_fps {
            crate::render::draw_fps(dst_hdc, fps);
        }

        crate::render::composite_child_windows(hwnd, buffers.hdc);
    }
}
