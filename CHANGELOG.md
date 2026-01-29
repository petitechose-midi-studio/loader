# Changelog

## v0.1.2

- Fix the GitHub Actions Intel macOS runner label in the release workflow.

## v0.1.1

- Build macOS ARM64 and X64 release artifacts.
- Include soft reboot events and HalfKay path in `flash --json` output.

## v0.1.0

- Initial working Teensy 4.1 HalfKay flasher.
- Windows: Win32 overlapped write backend for reliability.
- Optional USB-serial soft reboot (134 baud) when firmware exposes Serial.
