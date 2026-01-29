# Roadmap

Scope for now: a minimal Teensy 4.1 flasher CLI.

## Milestone 1 - Flash in HalfKay mode (MVP)

- Parse Intel HEX (records 00/01/04; ignore other non-data records)
- Teensy 4.x mapping: `0x60000000` FlexSPI offset -> `0x00000000`
- HalfKay packet format: 64-byte header + 1024 payload
- Robust per-block retries + reopen device on failure
- `--wait` + `--wait-timeout-ms`
- `--json` progress events

## Milestone 2 - Reliability + diagnostics

- Better error reporting (likely competing process / permissions)
- Add `list` command (show HalfKay presence)
- Add `--retries`, backoff strategy and stable progress reporting

## Milestone 3 - Enter bootloader (optional)

- Best-effort "enter bootloader" (reduce manual reset) when possible
