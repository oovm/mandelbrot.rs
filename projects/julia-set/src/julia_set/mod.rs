use std::cmp::min;
use image::{ImageBuffer, Rgba};
use num::{Complex, Signed};

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotTeardrop {

}

#[derive(Copy, Clone, Debug)]
pub struct Mandelbrot {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub power: f32,
    pub zoom: f32,
}

impl Default for Mandelbrot {
    fn default() -> Self {
        Self {
            center: Complex::new(-1.5, 0.0),
            max_iterations: 256,
            power: 2.0,
            zoom: 2.5,
        }
    }
}

impl Mandelbrot {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self}
    }
    pub fn with_max_iterations(self, iterations: u32) -> Mandelbrot {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> Mandelbrot {
        Self { zoom, ..self }
    }
}

impl Mandelbrot {
    pub fn render(&self, width: u32, height: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {

        let mut image_buffer = ImageBuffer::new(width, height);
        for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
            let u = x as f32 / height as f32;
            let v = y as f32 / height as f32;
            let t = self.escape(Complex { re: 2.5 * (u - 0.5) - 2.0, im: 2.5 * (v - 0.5) - 1.0 });
            *pixel = color((2.0 * t + 0.5) % 1.0);
        }

        image_buffer

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

#[test]
fn main() {
    let image_width = 800;
    let image_height = 600;

    let m = crate::julia_set::Mandelbrot::default().with_max_iterations(512);

    let image_buffer = m.render(image_width, image_height);
    image_buffer.save("mandelbrot.png").unwrap();

    let mut image_buffer = ImageBuffer::new(image_width, image_height);
    for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
        let u = x as f32 / image_height as f32;
        let v = y as f32 / image_height as f32;
        let t = julia(2.5 * (u - 0.5) - 1.4, 2.5 * (v - 0.5));
        *pixel = color((2.0 * t + 0.5) % 1.0);
    }
    image_buffer.save("julia.png").unwrap();
}