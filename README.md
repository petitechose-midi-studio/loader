# midi-studio-loader

Minimal, robust flasher CLI for **Teensy 4.1** (HalfKay bootloader).

This tool is designed to be called programmatically (stable exit codes + optional JSON output).

## Status

Early MVP. Scope is intentionally limited to Teensy 4.1.

## Usage

List detected targets (HalfKay + PJRC USB serial):

```bash
midi-studio-loader list
```

Flash a firmware (Intel HEX):

```bash
midi-studio-loader flash path/to/firmware.hex
```

Validate selection and HEX without flashing:

```bash
midi-studio-loader flash path/to/firmware.hex --dry-run
```

### Output contract

- Default mode prints human-readable progress/logs to stderr.
- `--json` prints JSON lines to stdout. When `--json` is used, stdout is reserved for JSON.
- Exit codes:
  - 0: success
  - 10: no device / no targets
  - 11: invalid HEX
  - 12: write/flash failed
  - 13: ambiguous target selection
  - 20: unexpected/internal error

If multiple targets are connected, select one:

```bash
midi-studio-loader flash path/to/firmware.hex --device serial:COM6
```

Or flash all detected targets sequentially:

```bash
midi-studio-loader flash path/to/firmware.hex --all
```

Enter HalfKay without the button (requires USB Serial in your firmware):

```bash
midi-studio-loader reboot --device serial:COM6
```

Diagnose your setup (targets + oc-bridge status):

```bash
midi-studio-loader doctor
```

Bridge control (optional):

```bash
midi-studio-loader flash path/to/firmware.hex --device serial:COM6 --bridge-control-port 7999
midi-studio-loader flash path/to/firmware.hex --device serial:COM6 --no-bridge-control
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

### oc-bridge coordination

When flashing a `serial:*` target, `midi-studio-loader` attempts to temporarily pause `oc-bridge`
to release the serial port:

1) Prefer localhost IPC (`oc-bridge ctl pause/resume`, default port `7999`)
2) Fallback: stop/start the OS service (if installed)

## Library usage

The crate can be used as a library (disable default features to avoid pulling the CLI deps):

```toml
[dependencies]
midi-studio-loader = { git = "https://github.com/petitechose-midi-studio/loader", default-features = false }
```

Example:

```rust
use midi_studio_loader::api::{flash_teensy41_with_selection, FlashOptions, FlashSelection};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hex = std::path::Path::new("firmware.hex");
    let opts = FlashOptions {
        wait: true,
        serial_port: Some("COM6".to_string()),
        ..Default::default()
    };
    flash_teensy41_with_selection(hex, &opts, FlashSelection::Auto, |_| {})?;
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
- `13` ambiguous target
- `20` unexpected error

## Reference

- PJRC docs: https://www.pjrc.com/teensy/loader_cli.html
