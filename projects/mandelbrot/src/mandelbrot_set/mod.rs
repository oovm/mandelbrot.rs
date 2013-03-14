use image::{ImageBuffer, RgbaImage};
use num::Complex;

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotSet {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub power: f32,
    pub zoom: f32,
}

impl Default for MandelbrotSet {
    fn default() -> Self {
        Self { center: Complex::new(-1.0, 0.0), max_iterations: 256, power: 2.0, zoom: 1.0 }
    }
}

impl MandelbrotSet {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> MandelbrotSet {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> MandelbrotSet {
        Self { zoom, ..self }
    }
}

impl MandelbrotSet {
    pub fn render(&self, width: u32, height: u32) -> RgbaImage {
        let mut buffer = ImageBuffer::new(width, height);
        let tx = self.center.re - self.zoom.recip();
        let ty = self.center.im - self.zoom.recip();
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let dx = (x as f32 / height as f32) / self.zoom;
            let dy = (y as f32 / height as f32) / self.zoom;
            let t = self.escape(Complex::new(tx + dx, ty + dy));
            *pixel = crate::julia_set::color((2.0 * t + 0.5) % 1.0);
        }
        buffer
    }
    pub fn escape(&self, c: Complex<f32>) -> f32 {
        let mut z = Complex::default();
        let mut i = 0;
        while i < self.max_iterations && z.norm() < 32.0 {
            z = z.powf(self.power) + c;
            i += 1;
        }
        return (i as f32 - z.norm().log2().log2()) / (self.max_iterations as f32);
    }
}
