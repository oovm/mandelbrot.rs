//! FPS / game-speed limiter (ports `fps_limiter.c`).
//!
//! Two independent limiters, matching cnc-ddraw:
//! - **Render rate** (`maxfps`/`TargetFPS`/`VSync`): paces `Flip` /
//!   `WaitForVerticalBlank` and the render thread. Kept in the renderer.
//! - **Game logic ticks** (`maxgameticks`): throttles the game loop at the
//!   configured injection point (`limiter_type`): `TestCooperativeLevel` (1),
//!   `BltFast` (2), `Unlock` (3) or `PeekMessage` (4).

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;

// cnc-ddraw LIMIT_* values.
pub const LIMIT_AUTO: i32 = 0;
pub const LIMIT_TEST_COOPERATIVE_LEVEL: i32 = 1;
pub const LIMIT_BLTFAST: i32 = 2;
pub const LIMIT_UNLOCK: i32 = 3;
pub const LIMIT_PEEKMESSAGE: i32 = 4;

static CFG_LIMITER_TYPE: AtomicI32 = AtomicI32::new(0);
static CFG_MAXGAMETICKS: AtomicI32 = AtomicI32::new(0);
static CFG_MINFPS: AtomicI32 = AtomicI32::new(0);
static REFRESH_HZ: AtomicI32 = AtomicI32::new(60);

static mut LAST_TICK: Option<Instant> = None;
static mut TICK_LEN_MS: f64 = 1000.0 / 60.0;

/// (Re)initialise the game-speed limiter from config values.
pub fn init(maxgameticks: i32, limiter_type: i32, minfps: i32, refresh_rate: i32) {
    CFG_MAXGAMETICKS.store(maxgameticks, Ordering::Relaxed);
    CFG_LIMITER_TYPE.store(limiter_type.clamp(0, 4), Ordering::Relaxed);
    CFG_MINFPS.store(minfps, Ordering::Relaxed);
    REFRESH_HZ.store(if refresh_rate > 0 { refresh_rate } else { 60 }, Ordering::Relaxed);

    // Tick length: -2 = refresh rate, 0 = 60Hz emulation, n = custom.
    let hz = match maxgameticks {
        -2 => REFRESH_HZ.load(Ordering::Relaxed) as f64,
        0 => 60.0,
        n if n > 0 => n as f64,
        _ => 60.0, // -1 = disabled: still a sane default
    };
    unsafe {
        TICK_LEN_MS = 1000.0 / hz;
        LAST_TICK = Some(Instant::now());
    }
}

pub fn configured_limiter_type() -> i32 {
    CFG_LIMITER_TYPE.load(Ordering::Relaxed)
}

pub fn maxgameticks() -> i32 {
    CFG_MAXGAMETICKS.load(Ordering::Relaxed)
}

pub fn minfps() -> i32 {
    CFG_MINFPS.load(Ordering::Relaxed)
}

/// Whether the current limiter type matches `method` (AUTO selects based on
/// what the game is known to call every frame).
pub fn limiter_applies(method: i32) -> bool {
    let t = CFG_LIMITER_TYPE.load(Ordering::Relaxed);
    let m = CFG_MAXGAMETICKS.load(Ordering::Relaxed);
    if m == -1 {
        return false; // limiter disabled
    }
    match t {
        LIMIT_AUTO => method == LIMIT_BLTFAST,
        other => other == method,
    }
}

/// Block until the game-speed tick interval has elapsed since the last tick.
/// Returns at most once per `TICK_LEN_MS`; otherwise sleeps the remainder.
pub fn wait_game_tick() {
    let interval_ms = unsafe { TICK_LEN_MS };
    let now = Instant::now();
    let last = unsafe { LAST_TICK.unwrap_or(now) };
    let elapsed = now.duration_since(last).as_secs_f64() * 1000.0;
    if elapsed < interval_ms {
        let pad = std::time::Duration::from_secs_f64((interval_ms - elapsed) / 1000.0);
        std::thread::sleep(pad);
    }
    unsafe { LAST_TICK = Some(Instant::now()) };
}
