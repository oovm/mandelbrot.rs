use super::*;

impl Default for JuliaSet {
    fn default() -> Self {
        Self {
            center: Complex::default(),
            max_iterations: 128,
            tint_magnification: 1.0,
            shift: Complex::new(-0.4,0.6),
            power: 2.0,
            zoom: 1.0,
        }
    }
}

impl JuliaSet {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> JuliaSet {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_tint_coefficient(self, tint_coefficient: f32) -> JuliaSet {
        Self { tint_magnification: tint_coefficient, ..self }
    }
    pub fn with_shift(self, x: f32, y: f32) -> JuliaSet {
        Self { shift: Complex::new(x, y), ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> JuliaSet {
        Self { zoom, ..self }
    }
}



impl CanvasRenderer for JuliaSet {
    fn center_x(&self) -> f32 {
        self.center.re
    }

    fn center_y(&self) -> f32 {
        self.center.im
    }

    fn zoom_reciprocal(&self) -> f32 {
        self.zoom.recip()
    }

    fn render_pixel(&self, c: Complex<f32>) -> Rgba<u8> {
        self.escape(c).tint_by_max((self.max_iterations as f32).div(self.tint_magnification))
    }
}

