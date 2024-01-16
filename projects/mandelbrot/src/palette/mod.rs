use std::ops::Div;
use image::{ImageBuffer, Rgb, Rgba, RgbaImage};
use num::Complex;
use rayon::iter::ParallelIterator;
pub trait CanvasRenderer where Self: Sync {
    fn center_x(&self) -> f64;
    fn center_y(&self) -> f64;
    fn zoom_reciprocal(&self) -> f64;
    fn render(&self, width: u32, height: u32) -> RgbaImage {
        let mut buffer = ImageBuffer::new(width, height);
        let w = width.div(2) as f64;
        let h = height.div(2) as f64;
        let pt = self.zoom_reciprocal() / h;
        let tx = self.center_x() - pt * w;
        let ty = self.center_y() + pt * h;
        buffer.par_enumerate_pixels_mut().for_each(
             |(x, y, pixel)| {
                let dx = pt * x as f64;
                let dy = pt * y as f64;
                *pixel=self.render_pixel(Complex::new(tx + dx, ty - dy))
            }
        );
        buffer
    }
    fn render_pixel(&self, c: Complex<f64>) -> Rgba<u8>;
}

/// The Mandelbrot set is the set of complex numbers c for which the function
#[derive(Copy, Clone, Debug)]
pub struct EscapeSpeed {
    /// The point in the complex plane
    pub point: Complex<f64>,
    /// The number of iterations before the point escaped
    pub iterations: u32,
    /// Whether the point is escaped at max iterations
    pub is_escaped: bool,
}

impl EscapeSpeed {
    /// Get the color of the point by escaping speed and max iterations
    pub fn tint_by_max(&self, magnification: f64) -> Rgba<u8> {
        let t = (self.iterations as f64 - self.point.norm().log2().log2()) / magnification;
        color((2.0 * t + 0.5) % 1.0)
    }
    /// Get the color of the point by escaping speed and max iterations
    pub fn tint(&self) -> Rgba<u8> {
        let t = (self.iterations as f64 - self.point.norm().log2().log2()) / (self.iterations as f64);
        color((2.0 * t + 0.5) % 1.0)
    }
}

pub fn color(depth: f64) -> Rgba<u8> {
    let a = Rgb([0.5, 0.5, 0.5]);
    let b = Rgb([0.5, 0.5, 0.5]);
    let c = Rgb([1.0, 1.0, 1.0]);
    let d = Rgb([0.0, 0.10, 0.20]);
    let r = b.0[0] * (6.28318 * (c.0[0] * depth + d.0[0])).cos() + a.0[0];
    let g = b.0[1] * (6.28318 * (c.0[1] * depth + d.0[1])).cos() + a.0[1];
    let b = b.0[2] * (6.28318 * (c.0[2] * depth + d.0[2])).cos() + a.0[2];
    // if !r.is_sign_positive() || !g.is_sign_positive() || !b.is_sign_positive() {
    //     return Rgba([0, 0, 0, 0]);
    // }
    Rgba([(255.0 * r) as u8, (255.0 * g) as u8, (255.0 * b) as u8, 255])
}
