//! IDirectDrawGammaControl support (ports `IDirectDrawGammaControl.c`).
//!
//! Games obtain this interface by `QueryInterface(IID_IDirectDrawGammaControl)`
//! on the primary surface and call `SetGammaRamp` (the C&C series use it for
//! their in-game brightness slider). The ramp is stored so renderers can honour
//! it (or ignore it, like cnc-ddraw's stub), and reported back on
//! `GetGammaRamp`.

use crate::state::state;

/// 256 entries of three 16-bit channels (R,G,B) = 768 u16.
pub type GammaRamp = [u16; 768];

/// Store an installed gamma ramp (all-channels identity when `None`).
pub fn set_ramp(ramp: &GammaRamp) {
    state().lock().unwrap().gamma_ramp = Some(*ramp);
}

/// The currently installed ramp, or the identity ramp when none was set.
pub fn get_ramp_identity_filled() -> GammaRamp {
    let mut ramp = [0u16; 768];
    for i in 0..256 {
        let v = (i as u16) << 8 | (i as u16);
        ramp[i * 3] = v;
        ramp[i * 3 + 1] = v;
        ramp[i * 3 + 2] = v;
    }
    match state().lock().unwrap().gamma_ramp {
        Some(r) => r,
        None => ramp,
    }
}
