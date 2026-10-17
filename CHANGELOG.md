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
- `info` and `backup` from a gamepad mode, over the channel: REBOOT into
  web-config, read, `/api/reboot` back, and a check that the pad returns
  in the mode it was in. `--stay` leaves it in web-config, `--timeout` sets
  the wait after each reboot. If anything fails on the way, the pad is
  still sent back (over the channel when HTTP doesn't answer) and the
  error says whether that worked. The JSON gains `trip` (`null` when the
  pad was in web-config already; additive, still schema 1).
- After that trip, a RAM-only profile the restart dropped is switched back
  to (SET_PROFILE, RAM only, checked with STATUS, and recorded in the
  device cache): "restored profile 3 Celeste (RAM only) after the restart".
  If that fails, the note says the profile was dropped and why, with a
  warning on stderr; the exit code stays 0, since the read worked. With
  `--stay` nothing is switched and the note says the profile was dropped.
  `trip` gains `restored_profile` (the number, or `null`; additive, still
  schema 1).
- The device cache records the profile the control channel reports
  running (`running_profile`, next to the saved `profileNumber`):
  `profile switch` updates it (RAM-only the running one, `--save` both),
  any STATUS refreshes it, and a restart gamepadii makes clears it. The
  key hints use the live STATUS profile when the channel answers, else the
  cache's.
- `status` JSON: `control_channel.profiles`, the enabled profiles with
  their labels (additive, still schema 1).
- The TUI (first cut, #4): `gamepadii` with no command on a terminal, or
  `gamepadii tui`. One screen with the pad's status (mode, confidence,
  firmware, board, running profile, the control channel), the input modes
  (⏎ switches after a prompt, through the `mode` flow), the profiles (⏎
  switches RAM only, `S` saves after a prompt), `w` into web-config and
  back, `b` backup, `r` refresh and a `?` help overlay. Vim keys and
  arrows. The flows run on a worker thread, with their progress and
  button-press guides on screen; USB is watched once a second for
  hotplug. Off a terminal, `gamepadii` still prints usage and `gamepadii
  tui` exits 3.
- `gamepadii display layout [MODEL | --left N --right N]`: the OLED's
  button drawing. Haute42 pads (Haute42COSMOX) take their model (`s16`,
  `g16`, `s13`, `m16+`, `e16`, ...) or the layout (`default`,
  `alt0`..`alt5`), from the board's BoardConfig.h; S16/G16 is Board
  Defined Alternate 2 (36/42). Any board takes the raw values. With no
  argument it shows the current layout and the choices. It backs up
  first (`--no-backup` to skip), writes with a read-modify-write of the
  whole `setDisplayOptions` object (which zeroes whatever it isn't sent),
  and reads it back; from a gamepad mode the control channel takes the
  pad to web-config and back, like `info`.
- `info` shows the OLED layout, and on a Haute42 still on the generic
  12-button drawing with a 16-button pin mapping, suggests
  `gamepadii display layout s16`. The JSON gains `display_layout` and
  `hints` (additive, still schema 1).

### Fixed

- `info` no longer marks the saved profile as active after a RAM-only
  `profile switch` (#14). Web-config only knows the saved profile, so the
  rows are now labelled `saved` and `running` separately; the running one
  comes from the control channel's STATUS once the trip is over (after any
  restore), and is never guessed from the saved one. JSON gains
  `saved_profile`, `running_profile` (`null` when unknown), each profile's
  `saved` and `running` (`null` when unknown), and `trip.running_profile`;
  `active` and `active_profile` stay, meaning the saved one (additive,
  still schema 1). When STATUS doesn't answer after the trip, the trip
  still succeeds and says the running profile is unknown.
- Writes (`setGamepadOptions`, so `gamepadii mode` in web-config) failed
  with "Peer disconnected": lwIP's httpd matches `Content-Length:`
  case-sensitively and ureq sends lowercase header names, so the pad
  dropped every POST with a body. The transport is now a small HTTP/1.0
  client that sends the headers with their exact casing.

### Changed

- The device cache is kept per pad, by the serial XInput and Xbox One
  build from the board's unique id (#11): two pads with the same board
  config no longer share `<board>.json`, and the hints in a gamepad mode
  come from that pad's cache rather than the newest. Web-config can't say
  which pad it is (every pad is `123456`), so gamepadii records the serial
  it saw just before the pad went into web-config (the `info`/`backup`
  trip, `webconfig`, `reboot webconfig`, the guided `mode`) against that
  web-config enumeration (`webconfig-sessions.json`). A read it can't
  attribute stays in `<board>.json` until it sees that pad leave
  web-config with a serial. Existing `<board>.json` files still load, and
  the first read attributed to a pad of that board replaces them. The
  cache file gains `pad` and `read_in` (additive).
- Backups and the device cache keep each endpoint's keys in the pad's order
  (serde_json `preserve_order`).
- Exit code 1 is now used: a verify that fails after a reboot or a write.
