use std::ops::Div;
use image::Rgba;
use num::{ Complex};
use crate::{CanvasRenderer, EscapeSpeed};

mod presets;

#[derive(Copy, Clone, Debug)]
pub struct JuliaSet {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub tint_magnification: f32,
    pub shift: Complex<f32>,
    pub power: f32,
    pub zoom: f32,
}



impl JuliaSet {
    pub fn escape(&self, mut z: Complex<f32>) -> EscapeSpeed {
        let mut i = 0;
        while i < self.max_iterations && z.norm() < 32.0 {
            z = z.powf(self.power) + self.shift;
            i += 1;
        }
        EscapeSpeed { point: z, iterations: i, is_escaped: z.norm() >= 32.0 }
    }
}

