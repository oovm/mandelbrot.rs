use image::Rgba;
use num::Complex;


fn mandelbrot(x: f32, y: f32) -> f32 {
    let mut z = Complex { re: 0.0, im: 0.0 };
    let c = Complex { re: x, im: y };
    let iterations = 256;
    let mut i = 0;
    while i < iterations && z.norm() < 32.0 {
        z = z * z + c;
        i += 1;
    }
    return (i as f32 - z.norm().log2().log2()) / (iterations as f32);
}

fn julia(x: f32, y: f32) -> f32 {
    let mut z = Complex { re: x, im: y };
    let c = Complex { re: 0.38, im: 0.28 };
    let iterations = 256;
    let mut i = 0;
    while i < iterations && z.norm() < 32.0 {
        z = z * z * z + c;
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
    Rgba([(255.0 * r) as u8, (255.0 * g) as u8, (255.0 * b) as u8, 255])
}

#[test]
fn main() {
    let image_width = 800;
    let image_height = 600;

    let mut image_buffer = image::ImageBuffer::new(image_width, image_height);
    for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
        let u = x as f32 / image_height as f32;
        let v = y as f32 / image_height as f32;
        let t = mandelbrot(2.5 * (u - 0.5) - 1.4, 2.5 * (v - 0.5));
        *pixel = color((2.0 * t + 0.5) % 1.0);
    }
    image_buffer.save("mandelbrot.png").unwrap();


    let mut image_buffer = image::ImageBuffer::new(image_width, image_height);
    for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
        let u = x as f32 / image_height as f32;
        let v = y as f32 / image_height as f32;
        let t = julia(2.5 * (u - 0.5) - 1.4, 2.5 * (v - 0.5));
        *pixel = color((2.0 * t + 0.5) % 1.0);
    }
    image_buffer.save("julia.png").unwrap();
}