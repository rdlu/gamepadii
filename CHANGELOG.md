# Changelog

All notable changes to gamepadii are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and gamepadii
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace: `gamepadii-core` (library) and `gamepadii` (CLI), with
  the `mise run check` gate and CI (rustfmt, clippy, tests, MSRV 1.88).
- `gamepadii status`: finds GP2040-CE devices on USB through sysfs and
  reports the input mode from the VID:PID, with a confidence (`confirmed`
  for ids only GP2040-CE uses, `likely` for ids borrowed from real
  controllers), the USB strings, bus, port and sysfs path, and the network
  interface in web-config mode.
- Output formats: `--format human|json|waybar` (`--json` is an alias). JSON
  carries `"schema": 1` and explicit `null`s; failures still print a JSON
  error object or a valid waybar line.
- Stable exit codes: 0 ok, 1 check failed, 2 no device, 3 usage,
  4 transport, 5 needs user action.
- `gamepadii info`: firmware version and build, board, flash and heap use,
  the saved input mode, the pin-mapping profiles (labels, which is active)
  and the boot-mode buttons decoded to input modes. Needs web-config; in a
  gamepad mode it exits 5 and says how to switch.
- `gamepadii backup [-o FILE]`: every allowlisted web-config endpoint in one
  versioned JSON bundle, by default under
  `~/.local/share/gamepadii/backups/`.
- `gamepadii reboot <gamepad|webconfig|bootsel>`.
- `--host HOST` to reach the web configurator without the USB check.
- Web-config client in `gamepadii-core`: serialised plain-HTTP transport
  with timeouts, a fixture-backed mock, and an endpoint allowlist that makes
  the endpoints known to hang or wipe the pad (`getConfig`,
  `getI2CPeripheralMap`, `resetSettings`) impossible to request.
