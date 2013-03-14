use image::{ImageBuffer, Rgba, RgbaImage};
use num::{complex::ComplexFloat, Complex, Signed};
use std::cmp::min;

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotTeardrop {}

fn julia(x: f32, y: f32) -> f32 {
    let mut z = Complex { re: x, im: y };
    let c = Complex { re: 0.48, im: 0.28 };
    let iterations = 256;
    let mut i = 0;
    while i < iterations && z.norm() < 32.0 {
        z = z.powf(2.0) + c;
        i += 1;
    }
    return (i as f32 - z.norm().log2().log2()) / (iterations as f32);
}

fn color(t: f32) -> Rgba<u8> {
    let a = (0.5, 0.5, 0.5);
    let b = (0.5, 0.5, 0.5);
    let c = (1.0, 1.0, 1.0);
    let d = (0.0, 0.10, 0.20);
    let r = b.0 * (6.28318 * (c.0 * t + d.0)).cos() + a.0;
    let g = b.1 * (6.28318 * (c.1 * t + d.1)).cos() + a.1;
    let b = b.2 * (6.28318 * (c.2 * t + d.2)).cos() + a.2;
    // if !r.is_sign_positive() || !g.is_sign_positive() || !b.is_sign_positive() {
    //     return Rgba([0, 0, 0, 0]);
    // }

    Rgba([(255.0 * r) as u8, (255.0 * g) as u8, (255.0 * b) as u8, 255])
}
