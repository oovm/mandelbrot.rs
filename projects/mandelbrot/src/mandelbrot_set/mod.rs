use std::ops::Div;
use image::{Rgba};
use num::Complex;
use crate::CanvasRenderer;
use crate::palette::EscapeSpeed;

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotSet {
    pub center: Complex<f32>,
    pub max_iterations: u32,
    pub tint_magnification: f32,
    pub power: f32,
    pub zoom: f32,
}

impl Default for MandelbrotSet {
    fn default() -> Self {
        Self { center: Complex::new(-0.75, 0.0), max_iterations: 128, tint_magnification: 1.0, power: 2.0, zoom: 0.75 }
    }
}

impl MandelbrotSet {
    pub fn with_center(self, x: f32, y: f32) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> MandelbrotSet {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_tint_coefficient(self, tint_coefficient: f32) -> MandelbrotSet {
        Self { tint_magnification: tint_coefficient, ..self }
    }
    pub fn with_zoom(self, zoom: f32) -> MandelbrotSet {
        Self { zoom, ..self }
    }
}


impl CanvasRenderer for MandelbrotSet {
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

impl MandelbrotSet {
    pub fn escape(&self, c: Complex<f32>) -> EscapeSpeed {
        let mut z = Complex::default();
        let mut i = 0;
        while i < self.max_iterations && z.norm() < 32.0 {
            z = z.powf(self.power) + c;
            i += 1;
        }
        EscapeSpeed {
            point: z,
            iterations: i,
            is_escaped: z.norm() >= 32.0,
        }
    }
}


