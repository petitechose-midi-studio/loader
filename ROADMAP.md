# Roadmap

Scope for now: a minimal Teensy 4.1 flasher CLI.

## Milestone 1 - Flash in HalfKay mode (MVP) (done)

- Parse Intel HEX (records 00/01/04; ignore other non-data records)
- Teensy 4.x mapping: `0x60000000` FlexSPI offset -> `0x00000000`
- HalfKay packet format: 64-byte header + 1024 payload
- Robust per-block retries + reopen device on failure
- `--wait` + `--wait-timeout-ms`
- `--json` progress events

## Milestone 2 - Windows reliability + diagnostics (done)

- Add `list` command (show HalfKay presence)
- Add `--retries`, backoff strategy and stable progress reporting
- Win32 backend for HalfKay writes (overlapped `WriteFile` + retry timeouts)
- Better Win32 error details (FormatMessage)

## Milestone 3 - Enter bootloader (optional) (done-ish)

- Best-effort "enter bootloader" via USB Serial (134 baud) when available

## Milestone 4 - Releases

- Tag-based GitHub release workflow (v*) producing platform binaries
- Cut a first tagged release and validate assets on all platforms

## Milestone 5 - Bootloader entry without USB Serial

- Optional device-side command (RawHID/MIDI) to reboot into HalfKay
- Host-side command to trigger it (so no button and no Serial needed)
