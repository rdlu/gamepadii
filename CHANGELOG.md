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
- Fork firmware: a capability check for builds of our GP2040-CE fork with
  the web-config heap/reply and I2C fixes (a known fork commit in the
  version or `boardBuild`, or the control channel answering HELLO on a
  build that isn't a bare release tag). On those, `backup` also saves the
  whole `getConfig` document, as `full_config` in the bundle, and reports
  it (`full_config`, `full_config_evidence`, `full_config_error` in the
  JSON, a "full config" row for people). Stock firmware is never asked for
  `getConfig` or `getI2CPeripheralMap`; the backup there just has
  `"full_config": null` (additive, still schema 1).
- `gamepadii leds`: the per-button RGB LEDs, meaning the animation,
  brightness (level, steps, maximum), static and button colours, static
  theme, custom theme and its colours. It uses control channel v1.1's
  LED_STATUS when the firmware has it (capability `leds`, no reboot, any
  mode). Otherwise it reads from web-config (`getLedOptions`,
  `getCustomTheme`, and `getConfig`'s `animationOptions` on fork builds),
  going through web-config and back from a gamepad mode, like `info`. On
  stock firmware the animation and brightness show as unknown, because
  v0.7.12's web configurator doesn't expose them. `--format waybar` gives
  a status-bar line, over the channel only (it never reboots the pad).
- `gamepadii leds set [--animation NAME|N] [--brightness N|up|down]
  [--static-color NAME|N] [--button-color NAME|N] [--theme NAME|N]
  [--save]` and `gamepadii leds brightness up|down|N`: live LED changes
  over control channel v1.1 (SET_LED), RAM only unless `--save`, read
  back with LED_STATUS. The names are GP2040-CE v0.7.12's effects,
  colours and static themes. Without the LED commands (stock, or a v1.0
  channel), these commands exit 5, and a value the pad doesn't have
  exits 3.
- Control channel v1.1 on the host: LED_STATUS (`0x04`), SET_LED
  (`0x13`), capability bit 8, protocol minor 1; the simulator models them.
- Upstream GP2040-CE main (8013b41, unreleased, `v0.7.12-119-g8013b41`)
  alongside v0.7.12 (#21), built from main's source and not yet tried on a
  pad. gamepadii tells the firmware lines apart (a known build, the release
  tag, else the shape of `getGamepadOptions`: main's has the `switch2Pro*`
  keys) and keeps an allowlist per line: main drops `getCustomTheme`,
  `getHETriggerCalibration` and `getHETriggerOptions` and adds
  `getAnimationProtoOptions`, `getBoardDefinition`, `getBootModeOptions`,
  `getHETriggerCalibrations` and `getLightsDataOptions`; its ADC, GPIO,
  busy-wait and 32 KB endpoints stay out. The setter guards use main's
  key sets (`setGamepadOptions` reads 15 more keys, `setDisplayOptions`
  `displayContrast`). `info` and `backup` report the line
  (`firmware_line` in JSON, additive), and `leds` on main shows its LED
  profiles (`lights`). Stock main never counts as a fork build, so it is
  never asked for `getConfig`. Synthetic fixtures in
  `tests/fixtures/api/s16-main/`.
- Control channel 1.2 on main firmware: `leds` reads LIGHTS_STATUS (the
  lights profile, the brightness, the effects of the profile that runs),
  and `leds set --lights-profile N|off` and `--brightness` send
  SET_LIGHTS, live and RAM only unless `--save`. The S16 on main has no
  v1.1 LED commands, so before this change `leds set` could not change its
  LEDs. JSON: `lights_off` and `lights_profile` in the LED state, and the
  cycle times of each profile (additive). `leds` on main web-config no
  longer shows lights profile 0 when the lights are off.
  `--mock-channel lights` simulates it.
- Control channel 1.4, the profile slots: `gamepadii profile show
  [N|LABEL]` reads the pin mappings and the label of a profile live
  (READ_SLOT), in any mode. `gamepadii profile write FILE --slot N
  [--select] [--save] [--label TEXT]` writes them to profile 2 to 6
  (WRITE_SLOT), with no reboot, and RAM only unless `--save`. The JSON of
  `profile show` is a slot file. A slot file can leave out pins, and those
  pins follow profile 1. Actions can be numbers or names (`B1`, `none`,
  `turbo`). gamepadii checks the file before it sends a command. It backs
  up the profile first (`--no-backup` to skip). It waits for the pad to
  apply an active profile, and reads the profile back. A saved write also
  updates the device cache. `--mock-channel slots` simulates it.
- Control channel 1.5 builds: `leds set` on main waits while LIGHTS_STATUS
  shows a change that waits for core 1 (flag bit 3). The builds cb01f70,
  77620cd and 61ab30f count as main.
- Control channel 1.3: the board ID. STRING 0x05 gives the unique ID of the
  flash chip (16 hex digits). gamepadii asks for it only when HELLO gives
  1.3 or higher. `status` shows it (`board_id` in JSON, `null` before 1.3,
  additive). The device cache uses it as the key, in every mode and in
  web-config (`<board>-<board ID>.json`). A cache by serial still matches,
  because the firmware makes the serial from the ID, and the first read
  with the ID replaces that file. When two connected pads give the same
  ID, `status` tells them apart by USB port and shows a warning.
- Safety with other devices that use our USB ids (a real Xbox 360 pad, an
  8BitDo pad in XInput mode): a connection that answers HELLO without the
  channel is not asked again (`no-channel.json`), so a waybar module does
  not send vendor requests to it every few seconds. As root, gamepadii does
  not use the channel. Tests check that no OUT request follows a bad HELLO.
- Right after a pad connects, udev can apply the ACL a moment late. With
  the udev rule installed, gamepadii tries again for up to 1 s on a new
  node before it shows the udev hint.

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
- The simulated Xbox One serial took the v0.7.12 formula (`'A' + byte %
  25`) as the only one. Main uses `% 26`; the cache accepts both.
