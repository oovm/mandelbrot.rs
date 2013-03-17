use image::{ImageBuffer, Rgb, Rgba, RgbaImage};
use num::{complex::ComplexFloat, Complex, Signed};
use std::cmp::min;



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
