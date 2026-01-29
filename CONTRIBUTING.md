# Contributing

Thanks for helping improve `midi-studio-loader`.

## Goals

- Keep the scope narrow: Teensy 4.1 + HalfKay.
- Prefer small, reviewable commits.
- Keep the codebase easy to read (minimal `cfg(...)` sprawl; clear module boundaries).

## Development setup

- Install Rust (stable): https://www.rust-lang.org/tools/install

Linux build dependencies (CI uses these):

```bash
sudo apt-get update
sudo apt-get install -y pkg-config libudev-dev libusb-1.0-0-dev
```

## Commands

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --no-default-features
```

The binary is gated behind the `cli` feature (enabled by default). To build just the library:

```bash
cargo build --no-default-features --lib
```

## Hardware testing

Flashing requires the board to appear as HalfKay (`16C0:0478`).

- `midi-studio-loader list` shows HalfKay devices.
- `midi-studio-loader flash ... --wait` waits for HalfKay.
- `midi-studio-loader flash ... --soft-reboot --serial-port COMx` can enter HalfKay without the
  button, but only if the running firmware exposes a USB Serial interface.

## Design notes

- High-level flows live in `src/api.rs` (programmatic API + structured events).
- Low-level USB writes live in `src/halfkay.rs` and platform backends under `src/halfkay/`.
- On Windows we use a Win32 overlapped write path for reliability.
