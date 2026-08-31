//! Software upscale filters (cnc-ddraw's opengl shaders equivalent, CPU side).
//! filter 0=nearest 1=bilinear 2=catmull-rom 3=lanczos 4=xbr

use crate::state::{RGB555, SurfaceBuffers, active_palette_entries};
use std::sync::atomic::Ordering;

/// Read a single source pixel and return it as `0x00BBGGRR` (no alpha).
///
/// Source is `bpp`-bit (8/16/32) at `(x, y)` with the given pitch.
/// Out-of-range `y` is clamped.  Out-of-range `x` returns black.
#[allow(clippy::too_many_arguments)]
pub(crate) fn src_pixel(
    src: *const u8,
    pitch: usize,
    bpp: i32,
    rgb555: bool,
    x: i32,
    y: i32,
    palette: Option<&[[u8; 4]; 256]>,
    sw: i32,
    sh: i32,
) -> u32 {
    if x < 0 || x >= sw {
        return 0;
    }
    let y = y.clamp(0, sh - 1);
    unsafe {
        let row = src.add((y as usize) * pitch);
        match bpp {
            8 => {
                let idx = *row.add(x as usize) as usize;
                if let Some(pal) = palette {
                    let e = pal[idx];
                    let b = e[0] as u32;
                    let g = e[1] as u32;
                    let r = e[2] as u32;
                    b | (g << 8) | (r << 16)
                } else {
                    let v = idx as u32;
                    v | (v << 8) | (v << 16)
                }
            }
            16 => {
                let p = row.add((x as usize) * 2) as *const u16;
                let v = *p;
                let (r5, g5, b5) = if rgb555 {
                    (((v >> 10) & 0x1F) as u32, ((v >> 5) & 0x1F) as u32, (v & 0x1F) as u32)
                } else {
                    (((v >> 11) & 0x1F) as u32, ((v >> 5) & 0x3F) as u32, (v & 0x1F) as u32)
                };
                let r8 = r5 * 255 / 31;
                let g8 = if rgb555 { g5 * 255 / 31 } else { g5 * 255 / 63 };
                let b8 = b5 * 255 / 31;
                b8 | (g8 << 8) | (r8 << 16)
            }
            32 => {
                let p = row.add((x as usize) * 4);
                let b = *p as u32;
                let g = *p.add(1) as u32;
                let r = *p.add(2) as u32;
                b | (g << 8) | (r << 16)
            }
            _ => 0,
        }
    }
}

#[inline]
fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Nearest-neighbour sample from a 32-bit BGRA buffer.
fn sample_nearest(src: &[u32], sw: i32, sh: i32, fx: f32, fy: f32) -> u32 {
    let x = clamp_i32(fx.round() as i32, 0, sw - 1);
    let y = clamp_i32(fy.round() as i32, 0, sh - 1);
    src[(y as usize) * (sw as usize) + (x as usize)]
}

/// Bilinear sample — 4-tap with clamp at edges.
fn sample_bilinear(src: &[u32], sw: i32, sh: i32, fx: f32, fy: f32) -> u32 {
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let frac_x = fx - fx.floor();
    let frac_y = fy - fy.floor();

    let x0c = clamp_i32(x0, 0, sw - 1);
    let x1c = clamp_i32(x0 + 1, 0, sw - 1);
    let y0c = clamp_i32(y0, 0, sh - 1);
    let y1c = clamp_i32(y0 + 1, 0, sh - 1);

    let p00 = src[(y0c as usize) * (sw as usize) + (x0c as usize)];
    let p10 = src[(y0c as usize) * (sw as usize) + (x1c as usize)];
    let p01 = src[(y1c as usize) * (sw as usize) + (x0c as usize)];
    let p11 = src[(y1c as usize) * (sw as usize) + (x1c as usize)];

    blend_4(p00, p10, p01, p11, frac_x, frac_y)
}

fn blend_4(p00: u32, p10: u32, p01: u32, p11: u32, fx: f32, fy: f32) -> u32 {
    let a = 1.0 - fx;
    let b = fx;
    let c = 1.0 - fy;
    let d = fy;

    // BGRA layout: channel 0 = blue, 1 = green, 2 = red.
    let b0 = p00 & 0xFF;
    let g0 = (p00 >> 8) & 0xFF;
    let r0 = (p00 >> 16) & 0xFF;
    let b1 = p10 & 0xFF;
    let g1 = (p10 >> 8) & 0xFF;
    let r1 = (p10 >> 16) & 0xFF;
    let b2 = p01 & 0xFF;
    let g2 = (p01 >> 8) & 0xFF;
    let r2 = (p01 >> 16) & 0xFF;
    let b3 = p11 & 0xFF;
    let g3 = (p11 >> 8) & 0xFF;
    let r3 = (p11 >> 16) & 0xFF;

    let bm = (b0 as f32 * a * c + b1 as f32 * b * c + b2 as f32 * a * d + b3 as f32 * b * d) as u32;
    let gm = (g0 as f32 * a * c + g1 as f32 * b * c + g2 as f32 * a * d + g3 as f32 * b * d) as u32;
    let rm = (r0 as f32 * a * c + r1 as f32 * b * c + r2 as f32 * a * d + r3 as f32 * b * d) as u32;

    (bm.min(255)) | ((gm.min(255)) << 8) | ((rm.min(255)) << 16)
}

/// Catmull-Rom cubic interpolation weight.
fn cubic(t: f32, a0: f32, a1: f32, a2: f32, a3: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * a1)
        + (-a0 + a2) * t
        + (2.0 * a0 - 5.0 * a1 + 4.0 * a2 - a3) * t2
        + (-a0 + 3.0 * a1 - 3.0 * a2 + a3) * t3)
}

/// Sample a single channel from the 32-bit buffer at clamped (x, y).
#[inline]
fn sample_ch(src: &[u32], sw: i32, sh: i32, ch: usize, x: i32, y: i32) -> f32 {
    let cx = clamp_i32(x, 0, sw - 1);
    let cy = clamp_i32(y, 0, sh - 1);
    let p = src[(cy as usize) * (sw as usize) + (cx as usize)];
    ((p >> (ch * 8)) & 0xFF) as f32
}

/// Catmull-Rom (4x4 taps) resample.
fn sample_catmull_rom(src: &[u32], sw: i32, sh: i32, fx: f32, fy: f32) -> u32 {
    let x1 = fx.floor() as i32;
    let y1 = fy.floor() as i32;
    let dx = fx - fx.floor();
    let dy = fy - fy.floor();

    // BGRA layout: ch 0=blue, 1=green, 2=red.
    let mut col = [0.0f32; 4];
    let mut out = [0.0f32; 3];
    for (ch, out_ch) in out.iter_mut().enumerate() {
        for (j, col_j) in col.iter_mut().enumerate() {
            let yy = y1 - 1 + j as i32;
            *col_j = cubic(
                dy,
                sample_ch(src, sw, sh, ch, x1 - 1, yy),
                sample_ch(src, sw, sh, ch, x1, yy),
                sample_ch(src, sw, sh, ch, x1 + 1, yy),
                sample_ch(src, sw, sh, ch, x1 + 2, yy),
            );
        }
        *out_ch = cubic(dx, col[0], col[1], col[2], col[3]);
    }
    let b = clamp_ch(out[0]);
    let g = clamp_ch(out[1]);
    let r = clamp_ch(out[2]);
    b | (g << 8) | (r << 16)
}

#[inline]
fn clamp_ch(v: f32) -> u32 {
    (v.round() as i32).clamp(0, 255) as u32
}

/// 2-lobe Lanczos resample (a=2, 4x4 taps).
fn sample_lanczos(src: &[u32], sw: i32, sh: i32, fx: f32, fy: f32) -> u32 {
    let x1 = fx.floor() as i32;
    let y1 = fy.floor() as i32;
    let dx = fx - fx.floor();
    let dy = fy - fy.floor();
    let a: f32 = 2.0;

    fn kernel(t: f32, a: f32) -> f32 {
        let t = t.abs();
        if t < 1e-6 {
            1.0
        } else if t < a {
            let pi_t = std::f32::consts::PI * t;
            a * pi_t.sin() * (pi_t / a).sin() / (pi_t * pi_t)
        } else {
            0.0
        }
    }

    let mut result = [0.0f32; 3];
    for (ch, result_ch) in result.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        let mut wsum = 0.0f32;
        for j in -1..=2 {
            for i in -1..=2 {
                let sx = x1 + i;
                let sy = y1 + j;
                let wx = kernel(dx - i as f32, a);
                let wy = kernel(dy - j as f32, a);
                let w = wx * wy;
                sum += sample_ch(src, sw, sh, ch, sx, sy) * w;
                wsum += w;
            }
        }
        *result_ch = if wsum > 0.0 { sum / wsum } else { 0.0 };
    }
    let b = clamp_ch(result[0]);
    let g = clamp_ch(result[1]);
    let r = clamp_ch(result[2]);
    b | (g << 8) | (r << 16)
}

/// Sample from a 32-bit BGRA buffer using the given filter.
pub(crate) fn sample(src: &[u32], sw: i32, sh: i32, fx: f32, fy: f32, filter: i32) -> u32 {
    match filter {
        0 => sample_nearest(src, sw, sh, fx, fy),
        1 => sample_bilinear(src, sw, sh, fx, fy),
        2 => sample_catmull_rom(src, sw, sh, fx, fy),
        3 => sample_lanczos(src, sw, sh, fx, fy),
        // filter 4 (xBR) approximated by catmull-rom
        _ => sample_catmull_rom(src, sw, sh, fx, fy),
    }
}

pub(crate) struct Scaler {
    pub buf: Vec<u32>,
    pub w: i32,
    pub h: i32,
}

impl Scaler {
    pub fn new() -> Self {
        Scaler { buf: Vec::new(), w: 0, h: 0 }
    }

    /// Resize the scaler buffer to match the given output dimensions and
    /// convert + scale from the primary surface.
    pub fn resize(&mut self, buffers: &SurfaceBuffers, filter: i32, out_w: i32, out_h: i32) {
        let rgb555 = RGB555.load(Ordering::Relaxed);
        let palette = active_palette_entries();
        let n = (out_w as usize) * (out_h as usize);
        if self.buf.len() < n {
            self.buf.resize(n, 0);
        }
        let guard = buffers.lock.lock();
        convert_scale(
            buffers.surface,
            buffers.pitch as usize,
            buffers.width,
            buffers.height,
            buffers.bpp,
            rgb555,
            palette.as_ref(),
            filter,
            &mut self.buf,
            out_w,
            out_h,
        );
        drop(guard);
        self.w = out_w;
        self.h = out_h;
    }
}

/// Convert and scale source surface into destination 32-bit BGRA buffer.
///
/// For filter==0, performs palette/colour expansion only (1:1 copy when sizes
/// match).  For filters 1–3, resamples using the selected algorithm.  Filter
/// 4 (xBR) is approximated by catmull-rom.
#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_scale(
    src: *const u8,
    src_pitch: usize,
    sw: i32,
    sh: i32,
    s_bpp: i32,
    rgb555: bool,
    palette: Option<&[[u8; 4]; 256]>,
    filter: i32,
    dst: &mut [u32],
    dw: i32,
    dh: i32,
) {
    let total = (dw as usize) * (dh as usize);
    if dst.len() < total {
        return;
    }
    let dst = &mut dst[..total];

    if filter == 0 && dw == sw && dh == sh {
        for y in 0..sh {
            for x in 0..sw {
                dst[(y as usize) * (dw as usize) + (x as usize)] =
                    src_pixel(src, src_pitch, s_bpp, rgb555, x, y, palette, sw, sh);
            }
        }
        return;
    }

    if filter == 0 {
        for y in 0..dh {
            let sy = clamp_i32(y, 0, sh - 1);
            for x in 0..dw {
                let sx = clamp_i32(x, 0, sw - 1);
                dst[(y as usize) * (dw as usize) + (x as usize)] =
                    src_pixel(src, src_pitch, s_bpp, rgb555, sx, sy, palette, sw, sh);
            }
        }
        return;
    }

    // For filtered paths, first expand source to a temporary 32-bit BGRA
    // buffer, then resample from that.
    let src32_len = (sw as usize) * (sh as usize);
    let mut src32 = vec![0u32; src32_len];
    for y in 0..sh {
        for x in 0..sw {
            src32[(y as usize) * (sw as usize) + (x as usize)] =
                src_pixel(src, src_pitch, s_bpp, rgb555, x, y, palette, sw, sh);
        }
    }

    for y in 0..dh {
        let fy = (y as f32) * (sh - 1) as f32 / (dh - 1).max(1) as f32;
        for x in 0..dw {
            let fx = (x as f32) * (sw - 1) as f32 / (dw - 1).max(1) as f32;
            dst[(y as usize) * (dw as usize) + (x as usize)] = sample(&src32, sw, sh, fx, fy, filter);
        }
    }
}
