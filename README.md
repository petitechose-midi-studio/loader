# midi-studio-loader

Minimal, robust flasher CLI for **Teensy 4.1** (HalfKay bootloader).

This tool is designed to be called programmatically (stable exit codes + optional JSON output).

## Status

Early MVP. Scope is intentionally limited to Teensy 4.1.

## Usage

Flash a firmware (Intel HEX) while the board is in HalfKay bootloader mode:

```bash
midi-studio-loader flash path/to/firmware.hex --wait
```

Try to enter HalfKay without the button (requires USB Serial in your firmware):

```bash
midi-studio-loader reboot --serial-port COM6
```

Machine-readable output:

```bash
midi-studio-loader flash path/to/firmware.hex --wait --json
```

Do not reboot after programming:

```bash
midi-studio-loader flash path/to/firmware.hex --wait --no-reboot
```

## Notes

- HalfKay VID/PID: `16C0:0478`
- Windows: HalfKay writes use a Win32 backend (not hidapi write) for reliability.
- Linux: you likely need udev rules for non-root access.
- This tool only supports Teensy 4.1 and rejects HEX data outside the expected address range.

## Library usage

The crate can be used as a library (disable default features to avoid pulling the CLI deps):

```toml
[dependencies]
midi-studio-loader = { git = "https://github.com/petitechose-midi-studio/loader", default-features = false }
```

Example:

```rust
use midi_studio_loader::api::{flash_teensy41, FlashOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hex = std::path::Path::new("firmware.hex");
    let opts = FlashOptions {
        wait: true,
        soft_reboot: true,
        serial_port: Some("COM6".to_string()),
        ..Default::default()
    };
    flash_teensy41(hex, &opts, |_| {})?;
    Ok(())
}
```

## Development

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --no-default-features
```

## Exit codes

- `0` success
- `10` no device (HalfKay not found)
- `11` invalid hex
- `12` write failed
- `20` unexpected error

## Reference

- PJRC docs: https://www.pjrc.com/teensy/loader_cli.html
