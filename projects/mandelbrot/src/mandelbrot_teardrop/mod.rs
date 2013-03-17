use std::ops::Div;
use image::{ImageBuffer, RgbaImage};
use num::Complex;
use crate::palette::EscapeSpeed;

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotTeardrop {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub power: f32,
    pub zoom: f32,
}

impl Default for MandelbrotTeardrop {
    fn default() -> Self {
        Self { center: Complex::new(1.24, 0.0), max_iterations: 128, power: 2.0, zoom: 0.48 }
    }
}

impl MandelbrotTeardrop {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> MandelbrotTeardrop {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> MandelbrotTeardrop {
        Self { zoom, ..self }
    }
}
//
// impl MandelbrotTeardrop {
//     pub fn render(&self, width: u32, height: u32) -> RgbaImage {
//         let w = width.div(2) as f32;
//         let h = height.div(2) as f32;
//         let mut buffer = ImageBuffer::new(width, height);
//         let pt = self.zoom.recip() / h;
//         let tx = self.center.re - pt * w;
//         let ty = self.center.im + pt * h;
//         for (x, y, pixel) in buffer.enumerate_pixels_mut() {
//             let dx = pt * x as f32;
//             let dy = pt * y as f32;
//             let t = self.escape(Complex::new(tx + dx, ty - dy));
//             *pixel = crate::julia_set::color((2.0 * t + 0.5) % 1.0);
//         }
//         buffer
//     }
//     pub fn escape(&self, c: Complex<f32>) -> EscapeSpeed {
//         let mut z = Complex::default();
//         let mut i = 0;
//         while i < self.max_iterations && z.norm() < 32.0 {
//             z = z.powf(self.power) + 1.0 / c;
//             i += 1;
//         }
//         EscapeSpeed {
//             point: Default::default(),
//             iterations: 0,
//             is_escaped: false,
//         }
//     }
// }