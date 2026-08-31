# rs-ddraw

DirectDraw compatibility layer for legacy 2D games, written in Rust.

Fixes compatibility issues in older DirectDraw-based games on modern Windows: black screen, poor performance, crashes, and broken Alt+Tab.

## Usage

1. Build the project: `cargo build --release`
2. Copy `target/release/rs_ddraw.dll` to your game folder and rename it to `ddraw.dll`
3. Start the game

## Building

Requires Rust nightly and the Windows SDK (32-bit target).

```bash
rustup target add i686-pc-windows-msvc
cargo build --release --target i686-pc-windows-msvc
```

## License

MIT
