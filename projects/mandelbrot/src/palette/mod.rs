use std::ops::Div;
use image::{ImageBuffer, Rgb, Rgba, RgbaImage};
use num::Complex;

pub trait CanvasRender {
    fn center_x(&self) -> f32;
    fn center_y(&self) -> f32;
    fn zoom_reciprocal(&self) -> f32;
    fn render(&self, width: u32, height: u32) -> RgbaImage {
        let mut buffer = ImageBuffer::new(width, height);
        let w = width.div(2) as f32;
        let h = height.div(2) as f32;
        let pt = self.zoom_reciprocal() / h;
        let tx = self.center_x() - pt * w;
        let ty = self.center_y() + pt * h;
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            let dx = pt * x as f32;
            let dy = pt * y as f32;
            *pixel = self.render_pixel(Complex::new(tx + dx, ty - dy));
        }
        buffer
    }
    fn render_pixel(&self, c: Complex<f32>) -> Rgba<u8>;
}

#[derive(Copy, Clone, Debug)]
pub struct EscapeSpeed {
    pub point: Complex<f32>,
    pub iterations: u32,
    pub is_escaped: bool,
}

impl EscapeSpeed {
    pub fn tint_by_max(&self, magnification: f32) -> Rgba<u8> {
        let t = (self.iterations as f32 - self.point.norm().log2().log2()) / magnification;
        color((2.0 * t + 0.5) % 1.0)
    }
    pub fn tint(&self) -> Rgba<u8> {
        let t = (self.iterations as f32 - self.point.norm().log2().log2()) / (self.iterations as f32);
        color((2.0 * t + 0.5) % 1.0)
    }
}

pub fn color(depth: f32) -> Rgba<u8> {
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
