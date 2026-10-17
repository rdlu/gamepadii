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
