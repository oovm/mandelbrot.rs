//! Direct3D 9 presentation backend.
//!
//! Uploads the game's primary surface into an `A8R8G8B8` texture each frame and
//! draws a textured quad to the swap chain, honouring the same viewport /
//! aspect-ratio / letterbox rules the GDI and OpenGL backends use. D3D9 is the
//! most robust backend on modern Windows (it does not depend on a legacy GL
//! context like the OpenGL path does).

use std::sync::atomic::Ordering;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D9::IDirect3DBaseTexture9;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::render::scale;
use crate::state::{SurfaceBuffers, state};

/// Compiled `ps_2_0` Catmull-Rom (bicubic) upscale pixel shader, taken from
/// cnc-ddraw's `d3d9shader.h::D3D9_CATMULL_ROM_SHADER`. Samples sampler `s0`
/// (our stage-0 texture) and takes the texture size in constant register `c0`
/// (*not* `c(0).xy` is (w,h); `c0.zw` is (1/w,1/h) via the shader's own
/// division). Used for the bicubic/lanczos/xBR filters on the GPU path.
const D3D9_CATMULL_ROM_SHADER: [u8; 1200] = [
    0, 2, 255, 255, 254, 255, 44, 0, 67, 84, 65, 66, 28, 0, 0, 0, 131, 0, 0, 0, 0, 2, 255, 255, 2, 0, 0, 0, 28, 0, 0,
    0, 0, 1, 0, 0, 124, 0, 0, 0, 68, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 80, 0, 0, 0, 0, 0, 0, 0, 96, 0, 0, 0, 2, 0, 0, 0,
    1, 0, 2, 0, 108, 0, 0, 0, 0, 0, 0, 0, 83, 117, 114, 102, 97, 99, 101, 84, 101, 120, 0, 171, 4, 0, 12, 0, 1, 0, 1,
    0, 1, 0, 0, 0, 0, 0, 0, 0, 84, 101, 120, 116, 117, 114, 101, 83, 105, 122, 101, 0, 1, 0, 3, 0, 1, 0, 4, 0, 1, 0, 0,
    0, 0, 0, 0, 0, 112, 115, 95, 50, 95, 48, 0, 77, 105, 99, 114, 111, 115, 111, 102, 116, 32, 40, 82, 41, 32, 72, 76,
    83, 76, 32, 83, 104, 97, 100, 101, 114, 32, 67, 111, 109, 112, 105, 108, 101, 114, 32, 49, 48, 46, 49, 0, 171, 81,
    0, 0, 5, 1, 0, 15, 160, 0, 0, 0, 191, 0, 0, 0, 63, 0, 0, 128, 63, 0, 0, 32, 64, 81, 0, 0, 5, 2, 0, 15, 160, 0, 0,
    192, 63, 0, 0, 32, 192, 0, 0, 0, 64, 0, 0, 0, 0, 31, 0, 0, 2, 0, 0, 0, 128, 0, 0, 3, 176, 31, 0, 0, 2, 0, 0, 0,
    144, 0, 8, 15, 160, 1, 0, 0, 2, 0, 0, 8, 128, 1, 0, 0, 160, 4, 0, 0, 4, 0, 0, 3, 128, 0, 0, 228, 176, 0, 0, 228,
    160, 0, 0, 255, 128, 19, 0, 0, 2, 0, 0, 12, 128, 0, 0, 27, 128, 2, 0, 0, 3, 0, 0, 3, 128, 0, 0, 27, 129, 0, 0, 228,
    128, 2, 0, 0, 3, 0, 0, 12, 128, 0, 0, 27, 128, 1, 0, 0, 160, 6, 0, 0, 2, 1, 0, 1, 128, 0, 0, 0, 160, 6, 0, 0, 2, 1,
    0, 2, 128, 0, 0, 85, 160, 5, 0, 0, 3, 2, 0, 3, 128, 0, 0, 27, 128, 1, 0, 228, 128, 1, 0, 0, 2, 3, 0, 1, 128, 2, 0,
    0, 128, 2, 0, 0, 3, 0, 0, 12, 128, 0, 0, 27, 128, 1, 0, 85, 160, 2, 0, 0, 3, 0, 0, 3, 128, 0, 0, 228, 128, 1, 0,
    255, 160, 5, 0, 0, 3, 0, 0, 3, 128, 1, 0, 228, 128, 0, 0, 228, 128, 4, 0, 0, 4, 1, 0, 12, 128, 0, 0, 27, 176, 0, 0,
    27, 160, 0, 0, 228, 129, 4, 0, 0, 4, 2, 0, 12, 128, 1, 0, 228, 128, 2, 0, 0, 161, 2, 0, 170, 160, 4, 0, 0, 4, 2, 0,
    12, 128, 1, 0, 228, 128, 2, 0, 228, 128, 1, 0, 85, 160, 5, 0, 0, 3, 3, 0, 12, 128, 1, 0, 228, 128, 2, 0, 228, 128,
    4, 0, 0, 4, 4, 0, 3, 128, 1, 0, 27, 128, 2, 0, 0, 160, 2, 0, 85, 160, 5, 0, 0, 3, 4, 0, 12, 128, 1, 0, 228, 128, 1,
    0, 228, 128, 4, 0, 0, 4, 4, 0, 3, 128, 4, 0, 27, 128, 4, 0, 228, 128, 1, 0, 170, 160, 4, 0, 0, 4, 2, 0, 12, 128, 1,
    0, 228, 128, 2, 0, 228, 128, 4, 0, 27, 128, 6, 0, 0, 2, 4, 0, 1, 128, 2, 0, 255, 128, 6, 0, 0, 2, 4, 0, 2, 128, 2,
    0, 170, 128, 4, 0, 0, 4, 0, 0, 12, 128, 3, 0, 228, 128, 4, 0, 27, 128, 0, 0, 228, 128, 5, 0, 0, 3, 1, 0, 3, 128, 1,
    0, 228, 128, 0, 0, 27, 128, 1, 0, 0, 2, 3, 0, 2, 128, 1, 0, 85, 128, 1, 0, 0, 2, 4, 0, 2, 128, 3, 0, 85, 128, 1, 0,
    0, 2, 2, 0, 1, 128, 1, 0, 0, 128, 1, 0, 0, 2, 5, 0, 1, 128, 2, 0, 0, 128, 1, 0, 0, 2, 4, 0, 1, 128, 0, 0, 0, 128,
    1, 0, 0, 2, 5, 0, 2, 128, 0, 0, 85, 128, 66, 0, 0, 3, 0, 0, 15, 128, 3, 0, 228, 128, 0, 8, 228, 160, 66, 0, 0, 3,
    3, 0, 15, 128, 1, 0, 228, 128, 0, 8, 228, 160, 66, 0, 0, 3, 6, 0, 15, 128, 2, 0, 228, 128, 0, 8, 228, 160, 66, 0,
    0, 3, 5, 0, 15, 128, 5, 0, 228, 128, 0, 8, 228, 160, 66, 0, 0, 3, 7, 0, 15, 128, 4, 0, 228, 128, 0, 8, 228, 160, 4,
    0, 0, 4, 1, 0, 3, 128, 1, 0, 27, 128, 1, 0, 85, 161, 1, 0, 170, 160, 4, 0, 0, 4, 1, 0, 3, 128, 1, 0, 27, 128, 1, 0,
    228, 128, 1, 0, 0, 160, 5, 0, 0, 3, 1, 0, 3, 128, 1, 0, 228, 128, 1, 0, 27, 128, 4, 0, 0, 4, 1, 0, 12, 128, 1, 0,
    228, 128, 1, 0, 85, 160, 1, 0, 0, 160, 5, 0, 0, 3, 1, 0, 12, 128, 1, 0, 228, 128, 4, 0, 228, 128, 5, 0, 0, 3, 0, 0,
    8, 128, 2, 0, 170, 128, 1, 0, 0, 128, 5, 0, 0, 3, 0, 0, 7, 128, 0, 0, 255, 128, 0, 0, 228, 128, 4, 0, 0, 4, 0, 0,
    8, 128, 2, 0, 255, 128, 1, 0, 85, 128, 0, 0, 255, 128, 5, 0, 0, 3, 3, 0, 8, 128, 1, 0, 85, 128, 2, 0, 255, 128, 4,
    0, 0, 4, 0, 0, 8, 128, 2, 0, 255, 128, 2, 0, 170, 128, 0, 0, 255, 128, 4, 0, 0, 4, 0, 0, 8, 128, 1, 0, 255, 128, 2,
    0, 170, 128, 0, 0, 255, 128, 4, 0, 0, 4, 0, 0, 8, 128, 2, 0, 255, 128, 1, 0, 170, 128, 0, 0, 255, 128, 6, 0, 0, 2,
    0, 0, 8, 128, 0, 0, 255, 128, 4, 0, 0, 4, 0, 0, 7, 128, 6, 0, 228, 128, 3, 0, 255, 128, 0, 0, 228, 128, 5, 0, 0, 3,
    3, 0, 8, 128, 2, 0, 170, 128, 2, 0, 255, 128, 4, 0, 0, 4, 0, 0, 7, 128, 3, 0, 228, 128, 3, 0, 255, 128, 0, 0, 228,
    128, 5, 0, 0, 3, 5, 0, 8, 128, 2, 0, 170, 128, 1, 0, 255, 128, 5, 0, 0, 3, 7, 0, 8, 128, 1, 0, 170, 128, 2, 0, 255,
    128, 4, 0, 0, 4, 0, 0, 7, 128, 7, 0, 228, 128, 5, 0, 255, 128, 0, 0, 228, 128, 4, 0, 0, 4, 0, 0, 7, 128, 5, 0, 228,
    128, 7, 0, 255, 128, 0, 0, 228, 128, 5, 0, 0, 3, 0, 0, 7, 128, 0, 0, 255, 128, 0, 0, 228, 128, 1, 0, 0, 2, 0, 0, 8,
    128, 1, 0, 170, 160, 1, 0, 0, 2, 0, 8, 15, 128, 0, 0, 228, 128, 255, 255, 0, 0,
];

pub(crate) struct D3D9State {
    d3d: IDirect3D9,
    device: IDirect3DDevice9,
    hwnd: HWND,
    tex: Option<IDirect3DTexture9>,
    tex_w: i32,
    tex_h: i32,
    surf_bpp: i32,
    client_w: i32,
    client_h: i32,
    vsync: bool,
    filter: i32,
    last_rw: i32,
    last_rh: i32,
    stage: Vec<u32>,
    /// ConvertOnGPU: keep a persistent dynamic texture (D3DPOOL_DEFAULT +
    /// D3DUSAGE_DYNAMIC) updated in place with `D3DLOCK_DISCARD` each frame.
    convert_gpu: bool,
    /// Whether the dynamic/GPU texture path is actually active (falls back to
    /// the CPU/managed path if the dynamic texture cannot be created).
    gpu_path: bool,
    /// PrimarySurface2Tex: when true, the primary surface region is used
    /// directly as the texture source (the single-buffer pipeline default).
    primary_s2t: bool,
    /// Catmull-Rom pixel shader for the bicubic/lanczos/xBR filters.
    ps_upscale: Option<IDirect3DPixelShader9>,
}

impl D3D9State {
    pub(crate) fn new(hwnd: HWND, _width: i32, _height: i32) -> Option<D3D9State> {
        unsafe {
            let d3d = match Direct3DCreate9(D3D_SDK_VERSION) {
                Some(d) => d,
                None => {
                    crate::dd_log!("d3d9: Direct3DCreate9 returned null");
                    return None;
                }
            };

            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if !hwnd.is_invalid() {
                GetClientRect(hwnd, &mut rc);
            }
            let cw = (rc.right - rc.left).max(1);
            let ch = (rc.bottom - rc.top).max(1);

            let vsync = { state().lock().unwrap().swap_interval != 0 };
            let filter = { state().lock().unwrap().filter };
            let convert_gpu = { state().lock().unwrap().convert_on_gpu };
            let primary_s2t = { state().lock().unwrap().primary_surface2tex };

            let mut pp = D3DPRESENT_PARAMETERS {
                BackBufferWidth: cw as u32,
                BackBufferHeight: ch as u32,
                BackBufferFormat: D3DFMT_UNKNOWN,
                BackBufferCount: 1,
                MultiSampleType: D3DMULTISAMPLE_NONE,
                MultiSampleQuality: 0,
                SwapEffect: D3DSWAPEFFECT_DISCARD,
                hDeviceWindow: hwnd,
                Windowed: TRUE,
                EnableAutoDepthStencil: FALSE,
                AutoDepthStencilFormat: D3DFMT_UNKNOWN,
                Flags: 0,
                FullScreen_RefreshRateInHz: 0,
                PresentationInterval: if vsync {
                    D3DPRESENT_INTERVAL_ONE as u32
                } else {
                    D3DPRESENT_INTERVAL_IMMEDIATE as u32
                },
            };

            let mut device: Option<IDirect3DDevice9> = None;
            let hr = d3d.CreateDevice(
                D3DADAPTER_DEFAULT,
                D3DDEVTYPE_HAL,
                hwnd,
                D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32,
                &mut pp,
                &mut device,
            );
            let device = match device {
                Some(d) => d,
                None => {
                    crate::dd_log!("d3d9: CreateDevice failed: {:?}", hr);
                    return None;
                }
            };

            // Compile the upscale pixel shader (Catmull-Rom bicubic) once; used
            // for the bicubic/lanczos/xBR filters when the surface is scaled.
            let ps_upscale = if filter >= 2 {
                match device.CreatePixelShader(D3D9_CATMULL_ROM_SHADER.as_ptr() as *const u32) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        crate::dd_log!("d3d9: CreatePixelShader(catmull) failed: {:?}", e);
                        None
                    }
                }
            } else {
                None
            };

            let st = D3D9State {
                d3d,
                device,
                hwnd,
                tex: None,
                tex_w: 0,
                tex_h: 0,
                surf_bpp: 0,
                client_w: cw,
                client_h: ch,
                vsync,
                filter,
                last_rw: 0,
                last_rh: 0,
                stage: Vec::new(),
                convert_gpu,
                gpu_path: convert_gpu,
                primary_s2t,
                ps_upscale,
            };
            st.apply_states();
            Some(st)
        }
    }

    /// (Re)apply the fixed-function pipeline state needed for a textured quad.
    fn apply_states(&self) {
        unsafe {
            let _ = self.device.SetRenderState(D3DRS_ZENABLE, 0);
            let _ = self.device.SetRenderState(D3DRS_LIGHTING, 0);
            let _ = self.device.SetTextureStageState(0, D3DTSS_COLOROP, D3DTOP_SELECTARG1.0 as u32);
            let _ = self.device.SetTextureStageState(0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
            let _ = self.device.SetSamplerState(0, D3DSAMP_MINFILTER, D3DTEXF_POINT.0 as u32);
            let _ = self.device.SetSamplerState(0, D3DSAMP_MAGFILTER, D3DTEXF_POINT.0 as u32);
            let _ = self.device.SetSamplerState(0, D3DSAMP_ADDRESSU, D3DTADDRESS_CLAMP.0 as u32);
            let _ = self.device.SetSamplerState(0, D3DSAMP_ADDRESSV, D3DTADDRESS_CLAMP.0 as u32);
        }
    }

    fn present_params(&self, w: i32, h: i32) -> D3DPRESENT_PARAMETERS {
        D3DPRESENT_PARAMETERS {
            BackBufferWidth: w.max(1) as u32,
            BackBufferHeight: h.max(1) as u32,
            BackBufferFormat: D3DFMT_UNKNOWN,
            BackBufferCount: 1,
            MultiSampleType: D3DMULTISAMPLE_NONE,
            MultiSampleQuality: 0,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: self.hwnd,
            Windowed: TRUE,
            EnableAutoDepthStencil: FALSE,
            AutoDepthStencilFormat: D3DFMT_UNKNOWN,
            Flags: 0,
            FullScreen_RefreshRateInHz: 0,
            PresentationInterval: if self.vsync {
                D3DPRESENT_INTERVAL_ONE as u32
            } else {
                D3DPRESENT_INTERVAL_IMMEDIATE as u32
            },
        }
    }

    /// Recreate the swap chain if the window client area changed size.
    fn ensure_size(&mut self) {
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if !self.hwnd.is_invalid() {
            unsafe {
                GetClientRect(self.hwnd, &mut rc);
            }
        }
        let cw = (rc.right - rc.left).max(1);
        let ch = (rc.bottom - rc.top).max(1);
        if cw == self.client_w && ch == self.client_h {
            return;
        }
        self.client_w = cw;
        self.client_h = ch;
        // Back buffer size change requires a Reset; drop the texture (recreated
        // lazily on the next upload) so it isn't tied to the old size.
        self.tex = None;
        let mut pp = self.present_params(cw, ch);
        unsafe {
            if self.device.Reset(&mut pp).is_ok() {
                self.apply_states();
            }
        }
    }

    /// (Re)allocate the stage texture. When `dynamic` is true the texture is
    /// created as `D3DPOOL_DEFAULT` with `D3DUSAGE_DYNAMIC`, so it can be
    /// re-locked in place with `D3DLOCK_DISCARD` every frame (the ConvertOnGPU
    /// path). Otherwise a `D3DPOOL_MANAGED` texture is used (the CPU path).
    fn ensure_texture(&mut self, w: i32, h: i32, bpp: i32, dynamic: bool) {
        if self.tex.is_some() && w == self.tex_w && h == self.tex_h && bpp == self.surf_bpp {
            return;
        }
        unsafe {
            self.tex = None;
            let mut tex: Option<IDirect3DTexture9> = None;
            let usage: u32 = if dynamic { D3DUSAGE_DYNAMIC as u32 } else { 0 };
            let pool: D3DPOOL = if dynamic { D3DPOOL_DEFAULT } else { D3DPOOL_MANAGED };
            let hr = self.device.CreateTexture(
                w as u32,
                h as u32,
                1,
                usage,
                D3DFMT_A8R8G8B8,
                pool,
                &mut tex,
                std::ptr::null_mut(),
            );
            match tex {
                Some(t) => {
                    self.tex = Some(t);
                    self.tex_w = w;
                    self.tex_h = h;
                    self.surf_bpp = bpp;
                    if dynamic {
                        self.gpu_path = true;
                    }
                }
                None => {
                    crate::dd_log!("d3d9: CreateTexture({}x{}, dynamic={}) failed: {:?}", w, h, dynamic, hr);
                    if dynamic {
                        // Fall back to the CPU/managed path.
                        self.gpu_path = false;
                    }
                }
            }
        }
    }

    /// Copy the game surface into the D3D texture, converting to A8R8G8B8.
    /// With `discard` (ConvertOnGPU path) the texture is locked with
    /// `D3DLOCK_DISCARD`, re-uploading the whole primary each frame in place.
    fn upload(&self, buffers: &SurfaceBuffers, discard: bool) {
        let tex = match self.tex.as_ref() {
            Some(t) => t,
            None => return,
        };
        unsafe {
            let mut lr = D3DLOCKED_RECT { Pitch: 0, pBits: std::ptr::null_mut() };
            let flags: u32 = if discard { D3DLOCK_DISCARD as u32 } else { 0 };
            if tex.LockRect(0, &mut lr, std::ptr::null(), flags).is_err() {
                return;
            }
            let rgb555 = crate::state::RGB555.load(Ordering::Relaxed);
            let bpp = buffers.bpp;
            let src = buffers.surface;
            let src_pitch = buffers.pitch as usize;
            let dst = lr.pBits as *mut u32;
            let dst_pitch = (lr.Pitch as usize) / 4;
            let width = buffers.width as usize;
            let height = buffers.height as usize;

            // Read the source surface under its lock.
            let _guard = buffers.lock.lock();
            let palette = if bpp == 8 { crate::state::active_palette_entries() } else { None };

            for y in 0..height {
                let srow = src.add(y * src_pitch);
                let drow = dst.add(y * dst_pitch);
                for x in 0..width {
                    let v = match bpp {
                        8 => {
                            let idx = *srow.add(x) as usize;
                            if let Some(pal) = palette.as_ref() {
                                let e = pal[idx];
                                (e[0] as u32) | ((e[1] as u32) << 8) | ((e[2] as u32) << 16) | 0xFF00_0000
                            } else {
                                let v = idx as u32;
                                v | (v << 8) | (v << 16) | 0xFF00_0000
                            }
                        }
                        32 => {
                            let p = srow.add(x * 4);
                            let b = *p;
                            let g = *p.add(1);
                            let r = *p.add(2);
                            (b as u32) | ((g as u32) << 8) | ((r as u32) << 16) | 0xFF00_0000
                        }
                        16 => {
                            let p = srow.add(x * 2) as *const u16;
                            let v = *p;
                            let (r, g, b) = if rgb555 {
                                (((v >> 10) & 0x1F) as u32, ((v >> 5) & 0x1F) as u32, (v & 0x1F) as u32)
                            } else {
                                (((v >> 11) & 0x1F) as u32, ((v >> 5) & 0x3F) as u32, (v & 0x1F) as u32)
                            };
                            let r8 = r * 255 / 31;
                            let g8 = if rgb555 { g * 255 / 31 } else { g * 255 / 63 };
                            let b8 = b * 255 / 31;
                            b8 | (g8 << 8) | (r8 << 16) | 0xFF00_0000
                        }
                        _ => 0xFF00_0000,
                    };
                    *drow.add(x) = v;
                }
            }
            let _ = tex.UnlockRect(0);
        }
    }

    /// Software-scale (and convert) the surface into a 32-bit staging buffer
    /// and upload it as an A8R8G8B8 texture. Used when `filter > 0`.
    fn upload_scaled(&mut self, buffers: &SurfaceBuffers, w: i32, h: i32) {
        let tex = match self.tex.as_ref() {
            Some(t) => t,
            None => return,
        };
        unsafe {
            let filter = self.filter;
            let mut lr = D3DLOCKED_RECT { Pitch: 0, pBits: std::ptr::null_mut() };
            if tex.LockRect(0, &mut lr, std::ptr::null(), 0).is_err() {
                return;
            }
            // Hold the surface lock while reading its pixels.
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
                crate::state::RGB555.load(Ordering::Relaxed),
                crate::state::active_palette_entries().as_ref(),
                filter,
                &mut self.stage,
                w,
                h,
            );
            drop(guard);
            let src = self.stage.as_ptr();
            let dst = lr.pBits as *mut u32;
            let dst_pitch = (lr.Pitch as usize) / 4;
            for y in 0..h {
                let drow = dst.add(y as usize * dst_pitch);
                let srow = src.add(y as usize * w as usize);
                std::ptr::copy_nonoverlapping(srow, drow, w as usize);
            }
            let _ = tex.UnlockRect(0);
        }
    }

    pub(crate) fn present(&mut self, buffers: &SurfaceBuffers, upload: bool) {
        unsafe {
            // Recover from a lost device (e.g. Alt+Tab) before drawing.
            if self.device.TestCooperativeLevel().is_err() {
                self.tex = None;
                let mut pp = self.present_params(self.client_w, self.client_h);
                if self.device.Reset(&mut pp).is_err() {
                    return;
                }
                self.apply_states();
            }

            self.ensure_size();

            let (render_w, render_h) = {
                let st = state().lock().unwrap();
                (st.render.width.max(1), st.render.height.max(1))
            };

            // ---- upload the primary into the texture ----
            // ConvertOnGPU: persistent dynamic texture re-locked with
            // D3DLOCK_DISCARD each frame; the pixel shader / sampler does the
            // filtering during the quad (no software scale). CPU fallback keeps
            // the old software-scale path. When filter >= 2, always upload at
            // source resolution so the GPU pixel shader can handle filtering.
            let gpu = self.gpu_path;
            if upload && gpu {
                self.ensure_texture(buffers.width, buffers.height, buffers.bpp, true);
                // PrimarySurface2Tex: the primary surface region is the texture
                // source. Our single-buffer pipeline always presents the primary
                // (`buffers`), so this is the primary region either way.
                if self.tex.is_some() {
                    self.upload(buffers, true);
                }
            } else if upload {
                if self.filter == 1 && !gpu {
                    // Software bilinear scale to the render target size; the
                    // quad covers the whole viewport 1:1, so the render-size
                    // texture maps one-to-one.
                    if self.tex.is_none() || self.last_rw != render_w || self.last_rh != render_h {
                        self.ensure_texture(render_w, render_h, 32, false);
                        self.last_rw = render_w;
                        self.last_rh = render_h;
                    }
                    self.upload_scaled(buffers, render_w, render_h);
                } else {
                    self.ensure_texture(buffers.width, buffers.height, buffers.bpp, false);
                    self.upload(buffers, false);
                }
            }

            let _ = self.device.BeginScene();
            let _ = self.device.Clear(0, std::ptr::null(), D3DCLEAR_TARGET as u32, 0x0000_0000, 0.0, 0);

            // D3D viewport origin is top-left (Windows coordinates), matching
            // `st.render.viewport`, so no Y flip is needed (unlike OpenGL).
            let (vx, vy, vw, vh) = {
                let st = state().lock().unwrap();
                let vp = st.render.viewport;
                if vp.right > vp.left && vp.bottom > vp.top {
                    (vp.left as u32, vp.top as u32, (vp.right - vp.left) as u32, (vp.bottom - vp.top) as u32)
                } else {
                    (0, 0, self.client_w as u32, self.client_h as u32)
                }
            };
            let vp = D3DVIEWPORT9 { X: vx, Y: vy, Width: vw, Height: vh, MinZ: 0.0, MaxZ: 1.0 };
            let _ = self.device.SetViewport(&vp);

            if let Some(tex) = self.tex.as_ref() {
                let base: &IDirect3DBaseTexture9 = tex;
                let _ = self.device.SetTexture(0, Some(base));
                // Filtering: nearest/bilinear via the sampler state; the heavier
                // filters via the Catmull-Rom pixel shader when the surface is
                // actually being scaled (1:1 blits stay on the sampler so text
                // stays pixel-crisp). The PS is applied whenever a source-size
                // texture is being upscaled, on both the GPU and CPU-managed
                // paths (both upload at source resolution for filter >= 2).
                let scaling = render_w != self.tex_w || render_h != self.tex_h;
                let (mag, min) = match self.filter {
                    0 => (D3DTEXF_POINT, D3DTEXF_POINT),
                    _ => (D3DTEXF_LINEAR, D3DTEXF_LINEAR),
                };
                let _ = self.device.SetSamplerState(0, D3DSAMP_MAGFILTER, mag.0 as u32);
                let _ = self.device.SetSamplerState(0, D3DSAMP_MINFILTER, min.0 as u32);
                let use_ps = scaling && (self.filter == 2 || self.filter == 3 || self.filter == 4);
                if use_ps && let Some(ps) = self.ps_upscale.as_ref() {
                    let _ = self.device.SetPixelShader(Some(ps));
                    // c0.xy = texture size; the shader computes 1/size itself.
                    let texsize = [self.tex_w as f32, self.tex_h as f32, 0.0f32, 0.0f32];
                    let _ = self.device.SetPixelShaderConstantF(0, texsize.as_ptr(), 1);
                } else {
                    let _ = self.device.SetPixelShader(None::<&IDirect3DPixelShader9>);
                }
                // Triangle strip quad in NDC. Offset UVs by half a texel so each
                // destination pixel samples a texel *center* (D3D9 texel centers
                // are at (x+0.5)/w) -- otherwise a 1:1 blit samples texel borders
                // and the game text/UI looks soft.
                let tw = self.tex_w.max(1) as f32;
                let th = self.tex_h.max(1) as f32;
                let u0 = 0.5 / tw;
                let v0 = 0.5 / th;
                let u1 = (tw - 0.5) / tw;
                let v1 = (th - 0.5) / th;
                #[rustfmt::skip]
                let verts: [f32; 20] = [
                    -1.0,  1.0, 0.0,  u0, v0,
                     1.0,  1.0, 0.0,  u1, v0,
                    -1.0, -1.0, 0.0,  u0, v1,
                     1.0, -1.0, 0.0,  u1, v1,
                ];
                let _ = self.device.SetFVF(D3DFVF_XYZ | D3DFVF_TEX1);
                let _ =
                    self.device.DrawPrimitiveUP(D3DPT_TRIANGLESTRIP, 2, verts.as_ptr() as *const core::ffi::c_void, 20);
                let _ = self.device.SetPixelShader(None::<&IDirect3DPixelShader9>);
                let _ = self.device.SetTexture(0, None);
            }

            let _ = self.device.EndScene();
            if self.device.Present(std::ptr::null(), std::ptr::null(), HWND::default(), std::ptr::null()).is_err() {
                return;
            }

            // GDI overlays (FPS text + child-window compositing) drawn after
            // present; best effort on top of the D3D content.
            let (hwnd, draw_fps, fps, hdc) = {
                let st = state().lock().unwrap();
                (st.hwnd, st.draw_fps, st.fps, st.hdc)
            };
            if draw_fps {
                crate::render::draw_fps(hdc, fps);
            }
            crate::render::composite_child_windows(hwnd, buffers.hdc);
        }
    }

    pub(crate) fn release(self) {
        // COM pointers are released by their Drop impls when `self` is dropped.
    }
}
