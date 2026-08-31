use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectDraw::*;
use windows::Win32::Graphics::Gdi::PALETTEENTRY;
use windows::core::*;

#[implement(IDirectDrawPalette)]
pub struct PaletteImpl {
    pub flags: u32,
    pub entries: RefCell<[[u8; 4]; 256]>, // B, G, R, Flags
}

impl IDirectDrawPalette_Impl for PaletteImpl_Impl {
    fn GetCaps(&self, lpdwflags: *mut u32) -> Result<()> {
        if !lpdwflags.is_null() {
            unsafe { *lpdwflags = self.flags };
        }
        Ok(())
    }

    fn GetEntries(&self, _dwflags: u32, dwstart: u32, dwcount: u32, lpentries: *mut PALETTEENTRY) -> Result<()> {
        if lpentries.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let start = dwstart as usize;
        let count = dwcount as usize;
        if start + count > 256 {
            return Err(E_INVALIDARG.into());
        }
        let entries = self.entries.borrow();
        unsafe {
            for i in 0..count {
                let e = &entries[start + i];
                *lpentries.add(i) = PALETTEENTRY { peRed: e[2], peGreen: e[1], peBlue: e[0], peFlags: e[3] };
            }
        }
        Ok(())
    }

    fn Initialize(&self, _lpdd: Ref<'_, IDirectDraw>, _dwflags: u32, _lpddcolorarray: *mut PALETTEENTRY) -> Result<()> {
        Ok(())
    }

    fn SetEntries(&self, _dwflags: u32, dwstart: u32, dwcount: u32, lpentries: *mut PALETTEENTRY) -> Result<()> {
        if lpentries.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let start = dwstart as usize;
        let count = dwcount as usize;
        if start + count > 256 {
            return Err(E_INVALIDARG.into());
        }
        let mut entries = self.entries.borrow_mut();
        unsafe {
            for i in 0..count {
                let pe = *lpentries.add(i);
                entries[start + i] = [pe.peBlue, pe.peGreen, pe.peRed, pe.peFlags];
            }
        }
        Ok(())
    }
}
