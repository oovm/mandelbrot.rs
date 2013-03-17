use mandelbrot::{CanvasRender, MandelbrotSet, MandelbrotTeardrop};

#[test]
fn ready() {
    println!("it works!")
}

const MAX_ITERATIONS: u32 = 256;
const IMAGE_WIDTH: u32 = 1600 /4 ;
const IMAGE_HEIGHT: u32 = 900 /4 ;


#[test]
fn draw_mandelbrot() {
    let m = MandelbrotSet::default().with_max_iterations(MAX_ITERATIONS).with_tint_coefficient(4.0);
    let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
    image_buffer.save("mandelbrot.png").unwrap();
}

// #[test]
// fn draw_teardrop() {
//     let m = MandelbrotTeardrop::default().with_max_iterations(MAX_ITERATIONS);
//     let image_buffer = m.render(IMAGE_WIDTH, IMAGE_HEIGHT);
//     image_buffer.save("teardrop.png").unwrap();
// }
