#![deny(missing_debug_implementations, missing_copy_implementations)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![doc = include_str!("../readme.md")]
#![doc(html_logo_url = "https://raw.githubusercontent.com/oovm/shape-rs/dev/projects/images/Trapezohedron.svg")]
#![doc(html_favicon_url = "https://raw.githubusercontent.com/oovm/shape-rs/dev/projects/images/Trapezohedron.svg")]

mod julia_set;
mod mandelbrot_set;
mod mandelbrot_teardrop;
mod palette;


mod errors;

pub use crate::{
    palette::{CanvasRenderer, EscapeSpeed},
    errors::{Error, Result},
    julia_set::JuliaSet,
    mandelbrot_set::MandelbrotSet,
    mandelbrot_teardrop::MandelbrotTeardrop
};
