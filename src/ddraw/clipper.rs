use std::sync::Mutex;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::{RDH_RECTANGLES, RGNDATA, RGNDATAHEADER};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::*;

#[implement(IDirectDrawClipper)]
pub struct ClipperImpl {
    pub hwnd: Mutex<isize>,
    pub clip: Mutex<Vec<RECT>>,
    pub changed: Mutex<bool>,
}

fn bbox(rects: &[RECT]) -> RECT {
    if rects.is_empty() {
        return RECT::default();
    }
    let mut b = RECT { left: i32::MAX, top: i32::MAX, right: i32::MIN, bottom: i32::MIN };
    for r in rects {
        b.left = b.left.min(r.left);
        b.top = b.top.min(r.top);
        b.right = b.right.max(r.right);
        b.bottom = b.bottom.max(r.bottom);
    }
    b
}

impl IDirectDrawClipper_Impl for ClipperImpl_Impl {
    fn GetClipList(&self, prc: *mut RECT, lpcliplist: *mut RGNDATA, lpdwsize: *mut u32) -> Result<()> {
        if lpdwsize.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let rects: Vec<RECT> = {
            let clip = self.clip.lock().unwrap();
            if !clip.is_empty() {
                clip.clone()
            } else {
                let hwnd = *self.hwnd.lock().unwrap();
                if hwnd != 0 {
                    let mut rc = RECT::default();
                    unsafe {
                        let _ = GetClientRect(HWND(hwnd as *mut core::ffi::c_void), &mut rc);
                    }
                    vec![rc]
                } else {
                    Vec::new()
                }
            }
        };
        let bb = bbox(&rects);
        let needed = std::mem::size_of::<RGNDATAHEADER>() + rects.len() * std::mem::size_of::<RECT>();
        if lpcliplist.is_null() {
            unsafe { *lpdwsize = needed as u32 };
            if !prc.is_null() {
                unsafe { *prc = bb };
            }
            return Ok(());
        }
        let avail = unsafe { *lpdwsize } as usize;
        if avail < needed {
            return Err(Error::from(HRESULT(DXERR_GENERIC as i32)));
        }
        unsafe {
            let out = &mut *lpcliplist;
            let mut rdh = out.rdh;
            rdh.dwSize = std::mem::size_of::<RGNDATAHEADER>() as u32;
            rdh.iType = RDH_RECTANGLES;
            rdh.nCount = rects.len() as u32;
            rdh.nRgnSize = (rects.len() * std::mem::size_of::<RECT>()) as u32;
            rdh.rcBound = bb;
            out.rdh = rdh;
            let dst = (lpcliplist as *mut u8).add(std::mem::size_of::<RGNDATAHEADER>()) as *mut RECT;
            for (i, r) in rects.iter().enumerate() {
                *dst.add(i) = *r;
            }
            *lpdwsize = needed as u32;
        }
        if !prc.is_null() {
            unsafe { *prc = bb };
        }
        Ok(())
    }

    fn GetHWnd(&self, lphwnd: *mut HWND) -> Result<()> {
        if lphwnd.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *lphwnd = HWND((*self.hwnd.lock().unwrap()) as *mut core::ffi::c_void) };
        Ok(())
    }

    fn Initialize(&self, _lpdd: Ref<'_, IDirectDraw>, _dwflags: u32) -> Result<()> {
        Ok(())
    }

    fn IsClipListChanged(&self, lpb: *mut BOOL) -> Result<()> {
        if !lpb.is_null() {
            let changed = *self.changed.lock().unwrap();
            unsafe { *lpb = BOOL(if changed { 1 } else { 0 }) };
            *self.changed.lock().unwrap() = false;
        }
        Ok(())
    }

    fn SetClipList(&self, lpcliplist: *mut RGNDATA, _dwflags: u32) -> Result<()> {
        if lpcliplist.is_null() {
            *self.clip.lock().unwrap() = Vec::new();
            *self.changed.lock().unwrap() = true;
            return Ok(());
        }
        let n = unsafe { (*lpcliplist).rdh }.nCount as usize;
        let src = unsafe { (lpcliplist as *mut u8).add(std::mem::size_of::<RGNDATAHEADER>()) } as *const RECT;
        let rects: Vec<RECT> = if n > 0 { unsafe { std::slice::from_raw_parts(src, n).to_vec() } } else { Vec::new() };
        *self.clip.lock().unwrap() = rects;
        *self.changed.lock().unwrap() = true;
        Ok(())
    }

    fn SetHWnd(&self, _dwflags: u32, hwnd: HWND) -> Result<()> {
        *self.hwnd.lock().unwrap() = hwnd.0 as isize;
        Ok(())
    }
}
