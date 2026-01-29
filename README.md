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
- Linux: you likely need udev rules for non-root access.
- This tool only supports Teensy 4.1 and rejects HEX data outside the expected address range.

## Exit codes

- `0` success
- `10` no device (HalfKay not found)
- `11` invalid hex
- `12` write failed
- `20` unexpected error

## Reference

- PJRC docs: https://www.pjrc.com/teensy/loader_cli.html
