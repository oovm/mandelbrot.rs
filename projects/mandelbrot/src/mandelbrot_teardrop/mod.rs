use std::ops::Div;
use image::{Rgba};
use num::Complex;
use crate::CanvasRenderer;
use crate::palette::EscapeSpeed;

#[derive(Copy, Clone, Debug)]
pub struct MandelbrotTeardrop {
    pub center: Complex<f64>,
    pub max_iterations: u32,
    pub tint_magnification: f64,
    pub power: f64,
    pub zoom: f64,
}

impl Default for MandelbrotTeardrop {
    fn default() -> Self {
        Self { center: Complex::new(-0.75, 0.0), max_iterations: 128, tint_magnification: 1.0, power: 2.0, zoom: 0.75 }
    }
}

impl MandelbrotTeardrop {
    pub fn with_center(self, x: f64, y: f64) -> Self {
        Self { center: Complex::new(x, y), ..self }
    }
    pub fn with_max_iterations(self, iterations: u32) -> MandelbrotTeardrop {
        Self { max_iterations: iterations, ..self }
    }
    pub fn with_tint_coefficient(self, tint_coefficient: f64) -> MandelbrotTeardrop {
        Self { tint_magnification: tint_coefficient, ..self }
    }
    pub fn with_zoom(self, zoom: f64) -> MandelbrotTeardrop {
        Self { zoom, ..self }
    }
}


impl CanvasRenderer for MandelbrotTeardrop {
    fn center_x(&self) -> f64 {
        self.center.re
    }

    fn center_y(&self) -> f64 {
        self.center.im
    }

    fn zoom_reciprocal(&self) -> f64 {
        self.zoom.recip()
    }

    fn render_pixel(&self, c: Complex<f64>) -> Rgba<u8> {
        self.escape(c).tint_by_max((self.max_iterations as f64).div(self.tint_magnification))
    }
}

impl MandelbrotTeardrop {
    pub fn escape(&self, c: Complex<f64>) -> EscapeSpeed {
        let mut z = Complex::default();
        let mut i = 0;
        while i < self.max_iterations && z.norm() < 32.0 {
            z = z.powf(self.power) + 1.0 / c;
            i += 1;
        }
        EscapeSpeed {
            point: z,
            iterations: i,
            is_escaped: z.norm() >= 32.0,
        }
    }
}


