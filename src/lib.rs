//! DirectDraw compatibility wrapper for legacy 2D games.

#![allow(dead_code)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_must_use)]
#![allow(function_casts_as_integer)]

// Exported globals consumed by laptop GPU drivers to force the discrete
// (high-performance) GPU, mirroring ts-ddraw's `main.c`.
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: i32 = 1;
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

pub(crate) mod log;

mod config;
mod counter;
mod ddraw;
mod debug;
mod dinput;
mod dll;
mod exports;
mod fps_limiter;
mod gamma;
mod hook;
mod media;
mod mouse;
mod overlay;
mod render;
mod screenshot;
mod state;
mod util;
mod window;
