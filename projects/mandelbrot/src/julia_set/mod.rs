use image::{ Rgb, Rgba, };
use num::{complex::ComplexFloat, Complex};



pub struct JuliaSet {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub power: f32,
    pub zoom: f32,
}

impl JuliaSet {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> JuliaSet {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> JuliaSet {
        Self { zoom, ..self }
    }
}


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

