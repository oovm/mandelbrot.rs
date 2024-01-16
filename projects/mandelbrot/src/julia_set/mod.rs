use std::ops::Div;
use image::Rgba;
use num::{ Complex};
use crate::{CanvasRenderer, EscapeSpeed};

mod presets;

#[derive(Copy, Clone, Debug)]
pub struct JuliaSet {
    pub center: Complex<f64>,
    pub max_iterations: u32,
    pub tint_magnification: f64,
    pub shift: Complex<f64>,
    pub power: f64,
    pub zoom: f64,
}



impl JuliaSet {
    pub fn escape(&self, mut z: Complex<f64>) -> EscapeSpeed {
        let mut i = 0;
        while i < self.max_iterations && z.norm() < 32.0 {
            z = z.powf(self.power) + self.shift;
            i += 1;
        }
        EscapeSpeed { point: z, iterations: i, is_escaped: z.norm() >= 32.0 }
    }
}

