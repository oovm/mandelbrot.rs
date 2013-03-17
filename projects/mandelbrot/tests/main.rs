use mandelbrot::{CanvasRenderer, JuliaSet, MandelbrotSet};

#[test]
fn ready() {
    println!("it works!")
}

const MAX_ITERATIONS: u32 = 2048;
const IMAGE_WIDTH: u32 = 1000;
const IMAGE_HEIGHT: u32 = 1000;


#[test]
fn draw_mandelbrot() {
    let m = MandelbrotSet::default().with_max_iterations(MAX_ITERATIONS).with_tint_coefficient(4.0);
    let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
    image_buffer.save("mandelbrot.png").unwrap();
}

#[test]
fn draw_julia() {
    let m = JuliaSet::default().with_shift(0.285, 0.01).with_zoom(0.6).with_max_iterations(MAX_ITERATIONS).with_tint_coefficient(1.0);
    let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
    image_buffer.save("julia-1.png").unwrap();
}

#[test]
fn draw_julia2() {
    let m = JuliaSet::default().with_shift(-0.7269, 0.1889).with_zoom(0.6).with_max_iterations(MAX_ITERATIONS).with_tint_coefficient(1.0);
    let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
    image_buffer.save("julia-2.png").unwrap();
}

// #[test]
// fn draw_teardrop() {
//     let m = MandelbrotTeardrop::default().with_max_iterations(MAX_ITERATIONS);
//     let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
//     image_buffer.save("teardrop.png").unwrap();
// }
