use mandelbrot::MandelbrotSet;

#[test]
fn ready() {
    println!("it works!")
}

#[test]
fn main() {
    let image_width = 800;
    let image_height = 600;

    let m = MandelbrotSet::default().with_max_iterations(200).with_zoom(0.1);
    let image_buffer = m.render(image_width, image_height);
    image_buffer.save("mandelbrot.png").unwrap();

    // let mut image_buffer = ImageBuffer::new(image_width, image_height);
    // for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
    //     let u = x as f32 / image_height as f32;
    //     let v = y as f32 / image_height as f32;
    //     let t = julia(2.5 * (u - 0.5) - 1.4, 2.5 * (v - 0.5));
    //     *pixel = color((2.0 * t + 0.5) % 1.0);
    // }
    // image_buffer.save("julia.png").unwrap();
}
