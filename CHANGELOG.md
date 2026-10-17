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
- `gamepadii mode [MODE]`: switch the input mode. In web-config it
  backs up, writes `inputMode` with a read-modify-write of the whole
  `setGamepadOptions` object (read back and checked), reboots and verifies
  the VID:PID. In a gamepad mode it says which buttons to press and waits
  for web-config (`--timeout`, `--no-wait` exits 5); a cached boot button for
  the target mode is offered as the no-write route. With no MODE: the
  current and saved mode and every choice, with its boot button.
- `gamepadii webconfig [--open | --exit]`: get into web-config (guided, with
  a wait) and print or open the URL, or reboot back to gamepad mode.
- `reboot` waits for the pad to re-enumerate and checks what it came back as
  (`--no-wait`, `--timeout`). A connection dropped after the request went out
  counts as a probable success, settled by the re-enumeration.
- Physical key hints: hotkeys and boot buttons are logical, so gamepadii
  says which keys they are in the active profile, by the names Default
  gives them, with the web configurator's Xbox labels.
- A device cache, `~/.cache/gamepadii/<board>.json`, written whenever the
  pad is in web-config (`info`, `backup`, `mode`, `webconfig`), so gamepad
  mode commands can name boot buttons and keys.
- Web-config client in `gamepadii-core`: serialised plain-HTTP transport
  with timeouts, a fixture-backed mock, and an endpoint allowlist that makes
  the endpoints known to hang or wipe the pad (`getConfig`,
  `getI2CPeripheralMap`, `resetSettings`) impossible to request.

- The control channel of the gamepadii firmware (vendor requests on EP0,
  `docs/design/control-channel.md`): a Linux usbfs transport, HELLO
  detection that rejects stock firmware's 0-byte (XInput) and garbage
  (Xbox One) replies, and STATUS, STRING, SET_PROFILE, REBOOT and
  SET_INPUT_MODE, with refusals explained from STATUS.
- `status` with the channel: `confirmed`, plus firmware version, board,
  running mode and active profile. JSON adds `control_channel` and
  `channel_note` per device (additive; still schema 1).
- `mode`, `webconfig` and `reboot` use the channel from a gamepad mode: no
  web-config, no button press. `reboot bootsel` goes through web-config.
  The JSON of `mode` has `"via": "control"`, and `reboot` and `webconfig`
  gained a `via` field.
- `gamepadii profile switch <N|LABEL> [--save]`: live, RAM only unless
  `--save`.
- `gamepadii setup udev [--print|--install]`: the opt-in uaccess rule for
  the pads' device nodes, with what it grants (the borrowed ids include real
  Xbox 360 and HORI pads); `--install` goes through pkexec.
- `--no-channel` on every command.

### Fixed

- Writes (`setGamepadOptions`, so `gamepadii mode` in web-config) failed
  with "Peer disconnected": lwIP's httpd matches `Content-Length:`
  case-sensitively and ureq sends lowercase header names, so the pad
  dropped every POST with a body. The transport is now a small HTTP/1.0
  client that sends the headers with their exact casing.

### Changed

- Backups and the device cache keep each endpoint's keys in the pad's order
  (serde_json `preserve_order`).
- Exit code 1 is now used: a verify that fails after a reboot or a write.
