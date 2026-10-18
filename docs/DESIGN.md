# gamepadii — design

A Linux CLI and TUI for gamepads that use the GP2040-CE firmware. A Tauri
GUI will follow. The first pad that it supports is the Haute42 S16.

gamepadii shows the status of the pad and changes its settings from the
computer: the input mode, the profile, the OLED layout and the LEDs. You do
not have to open the web configurator, and you push fewer buttons. With our
firmware fork, most changes do not need a button press.

This file gives the decisions, the facts about the firmware, the
architecture and the test rules. The roadmap and the status are in the
GitHub issues (#1). The words that we use are in
[GLOSSARY.md](GLOSSARY.md).

## Decisions

- **Name:** `gamepadii`. Docs at `https://gamepadii.dlu.one` (the DNS CNAME
  already exists). Licence: MIT.
- **Layout:** a Cargo workspace.
  - `crates/gamepadii-core` is the library: discovery, transports, the API
    model, profiles. It never prints.
  - `crates/gamepadii` is the binary, holding the CLI and the TUI.
  - `crates/gamepadii-gui` (Tauri) comes later, as a third member.
- **Layering** (from sofleii): `discovery` → `transport` → `client` →
  `commands` → `output`. Commands return plain serde data, and the front ends
  do all the rendering.
- **Output contract** (from sofleii): `--format human|json|waybar`. JSON has
  `"schema": 1`, snake_case keys, and `null` for anything unknown (never
  omitted). Exit codes are stable: 0 ok, 1 check failed, 2 device not found,
  3 usage, 4 transport, 5 needs user action (a button press).
- **No async runtime:** std threads and channels, as in plugii.
- **Git:** each change goes through a PR, with Conventional Commits. `main`
  is protected: CI must pass, and force-push is not possible.
- **Order after the MVP** (decided 2026-10-17): the firmware fork and its
  control channel first, then the TUI, then the rest. Linux first; Windows
  and macOS wait until the GUI. The core stays portable and Linux-only code
  stays behind cfg gates.
- **Our own firmware, not upstream first** (decided 2026-10-18): we publish
  our own fork builds and do not wait for upstream to accept our changes.
  The upstream maintainers closed our first reports (too long, too
  technical). We do not send reports now. If we send one again, the owner
  writes it, it is short, and we send one at a time. gamepadii stays
  compatible with stock firmware for the basic functions. The advanced
  functions (the control channel, `getConfig`, the LED commands) need our
  fork.
- **Boards** (decided 2026-10-18): our firmware is only for RP2040 and
  RP2350 (Pico 2) boards. Other chips need their own maintainers. See
  "Fork firmware", "Boards".
- **Writing** (decided 2026-10-18): all text uses ASD-STE100 Simplified
  Technical English: issues, PRs, commits, docs, code comments and CLI
  messages. Technical words that are not in STE are in
  [GLOSSARY.md](GLOSSARY.md). Each issue and PR starts with the intent. The
  rules are in `CLAUDE.md`, "Writing".
- **Depend on the fork where useful** (decided 2026-10-17): gamepadii uses
  what only fork builds can do (the control channel, `getConfig`) without
  waiting for upstream, behind a capability check, and stock keeps working
  as before (see "Fork firmware" below).

## What the firmware allows (GP2040-CE, checked at v0.7.12 and main 8013b41)

- **Stock gamepad mode has no control channel.** In XInput, HID, Switch and
  the other modes there is no vendor report and no serial interface. (The
  gamepadii fork adds one on EP0, see below.) On stock firmware the only
  ways into web-config are physical:
  - S2+B3+B4 while running (this toggles between gamepad and web-config)
  - holding S2 at plug-in
  - the OLED mini-menu
  - since v0.7.12, the reboot-to-webconfig hotkey

  The tool therefore *guides* the user ("press S2+B3+B4"), detects the
  re-enumeration by polling sysfs every 200 ms, and carries on from there.
  In v0.7.12 (`src/gp2040.cpp`) the hotkey only fires when exactly
  S2+B3+B4 are held (`state.buttons == mask`) for 4 s, after the pad has
  seen no buttons held; S1+B3+B4 the same way goes to BOOTSEL. At plug-in, S2
  (with anything else) wins over everything but S1+S2+Up (BOOTSEL), unless
  `forcedSetupMode` is 2 or 3. All of these are logical buttons, after the
  active profile's pin mapping.
- **Web-config mode:**
  - The device enumerates as `CAFE:4028` (RNDIS/ECM). It is reachable at
    192.168.7.1 and its own DHCP server hands out the host address.
  - The JSON HTTP API is in `src/webconfig.cpp`. `POST /api/reboot
    {"bootMode":0|1|2}` returns it to gamepad mode, web-config or BOOTSEL.
- **API hazards**, which the client must guard against:
  - `setGamepadOptions` zeroes every key it isn't sent, so always do a full
    read-modify-write. In detail (v0.7.12 `webconfig.cpp`): every field goes
    through `readDoc`, which assigns unconditionally, so a missing key is 0
    (a missing `inputModeB1` becomes XInput, not -1 "none"). The three
    `usbDesc*` strings are `strncpy`'d from `doc[key]`, a NULL pointer when
    the key is missing, so they must always be sent. The getter prints
    `usbVendorID`/`usbProductID` as `%04X` strings, but the setter reads
    integers, and ArduinoJson 6.21 parses a string as decimal: echoing
    `"10C4"` back stores 0. The web UI converts them with `hexToInt`, and so
    does gamepadii. `fnButtonPin` is getter-only. Hotkeys round-trip (the
    getter folds the dpad mask into `buttonsMask` and the setter splits it
    out again). The reply echoes the body. The save happens on the next
    main-loop pass, before a following reboot (which waits 500 ms). A save
    event clears a pending reboot, so the reboot has to come after the
    write's reply, never alongside it. gamepadii's `api::write` refuses to
    post a body that lacks any key the setter reads, then reads the options
    back and fails on any difference.
  - `setDisplayOptions` (v0.7.12 `webconfig.cpp:421`) is the same kind of
    setter: `readDoc` for every field, so anything not sent is 0. It reads
    22 top-level numbers: `enabled`, `flipDisplay`, `invertDisplay`,
    `buttonLayout`, `buttonLayoutRight`, `splashMode`, `splashChoice`,
    `splashDuration`, `displaySaverTimeout`, `displaySaverMode`,
    `buttonLayoutOrientation`, `turnOffWhenSuspended`, `inputMode`,
    `turboMode`, `dpadMode`, `socdMode`, `macroMode`, `profileMode`,
    `inputHistoryEnabled`, `inputHistoryLength`, `inputHistoryCol`,
    `inputHistoryRow`; and 10 nested ones, `layout`, `startX`, `startY`,
    `buttonRadius` and `buttonPadding` under both
    `buttonLayoutCustomOptions.params` (left) and `.paramsRight`. A missing
    `buttonLayoutCustomOptions` zeroes all ten. There are no strings, and
    the getter returns exactly these keys (its `bool`s as 1/0, which
    ArduinoJson reads back as booleans), so the getter's object is posted
    back unchanged apart from the edit. The splash image has its own
    setter. It saves (`GPStorageSaveEvent`) and echoes the body like
    `setGamepadOptions`. `api::set_display_options` refuses a body missing
    any of the 32 and verifies by reading back; the mock models the setter.
  - The server ignores the HTTP method, and a GET to a `set*` endpoint replays
    the previous POST body.
  - POST bodies are capped at 16 KB, and requests must be serialised.
  - `resetSettings` is a destructive GET.
  - **Never call `getConfig` on stock v0.7.12.** On the S16 (Haute42COSMOX
    build 0014e4a) it hangs the network stack: USB stays enumerated, the
    device stops answering ping, and only a replug brings it back (seen
    2026-10-17). Back up through the per-section `get*` endpoints instead,
    as the web UI does. It is allowed on fork builds with the fixes, and
    only behind the capability check ("Fork firmware" below).
  - **Never call `getI2CPeripheralMap` on stock either.** It runs a live I2C
    bus scan, and that hung the S16 in the same way. Fork builds fixed it
    too, and it sits behind the same check. The client keeps an explicit
    allowlist of endpoints verified as safe to read; a `get*` name alone
    doesn't make an endpoint safe. Verified safe on the S16 so far (the
    v0.7.12 allowlist in `gamepadii-core/src/api/line.rs`): getAddonsOptions,
    getButtonLayoutDefs, getButtonLayouts, getCustomTheme,
    getDisplayOptions, getExpansionPins, getFirmwareVersion,
    getGamepadOptions, getHETriggerCalibration, getHETriggerOptions,
    getKeyMappings, getLedOptions, getMacroAddonOptions, getMemoryReport,
    getPeripheralOptions, getPinMappings, getProfileOptions,
    getReactiveLEDs, getSplashImage, getUsedPins, getWiiControls. Their
    responses are the fixtures in `tests/fixtures/api/s16-v0.7.12/`.
    `getHeldPins` blocks until a button is pressed and stays out of the
    read path; it gets its own function with the button test. That list is
    the v0.7.12 line's; upstream main has its own ("Firmware lines" below).
    (Read since in the source: v0.7.12's `getHETriggerCalibration` is not a
    plain read, it reads the previous POST body for a `targetId`, sets the
    HE mux select pins and samples the ADC. It answered harmlessly on the
    S16, which has no HE triggers, and main renamed it `getHETriggerVoltage`
    and keeps it out of the allowlist.)
- **Input mode persists.** It is written to flash and survives replugs. It is
  also selected at boot by holding a mapped button: `inputModeB1..R2`, and on
  newer firmware `bootModeOptions.inputModeMappings`, which also picks the
  profile. In v0.7.12 (`GP2040::getBootAction`) the held buttons must be
  exactly that one button (dpad aside); the mode it selects is saved to flash
  (`Storage::save(true)`) when it differs, so a boot button is a mode change
  with no write from the host. Only modes 0–15 are in its switch (P5General,
  SInput and Switch 2 Pro are newer than v0.7.12 anyway), and
  `forcedSetupMode` 1 or 3 turns it off. gamepadii caches the map (see
  `cache.rs`), since in gamepad mode it can't be read.
- **Profiles resolve per pin** (`Storage::setFunctionalPinMappings`): the
  active profile's action for each GPIO, unless that action or the base
  profile's is RESERVED or ASSIGNED_TO_ADDON. A disabled profile falls back to
  the base. gamepadii names a hotkey's keys by what profile 1 (Default, the
  printed labels) has on the same GPIO.
- **Profiles:** up to 6 pin-mapping profiles (GPIO → logical button). They can
  be switched live with hotkeys (`LOAD_PROFILE_n`, `NEXT_PROFILE`), with no
  reboot.
- **There is only one keyboard keymap** (logical button → HID keycode, 32
  slots: Up/Down/Left/Right, B1–B4, L1/R1/L2/R2, S1/S2, L3/R3, A1–A4, E1–E12).
- **Detecting the current mode from VID:PID:** XInput 045E:028E, Keyboard
  CAFE:0001, HID 10C4:82C0, Switch 0F0D:0092, PS4/5 1532:0401, web-config
  CAFE:4028, BOOTSEL 2E8A:0003. The full table is in the research notes.
  Several of these IDs belong to other vendors' devices too, so confirm through
  the API, a stored serial, or the sysfs path.
- **USB serials** (v0.7.12, `headers/drivers/*/*Descriptors.h`): only
  XInput and Xbox One build theirs from the RP2040's unique id. XInput sends
  `"0"` plus bytes 5–7 of the id in hex (`0C0FFEE`; `usbDescOverride`
  replaces it with the user's `usbDescVersion`), Xbox One a letter per
  byte (`'A' + b % 25` on v0.7.12) and then `8ABCDEFGH`. Web-config always says
  `123456`, Switch Pro `000000000001`; SInput and Switch 2 Pro send fixed
  strings, and Keyboard, HID, Switch, PS3, PS4/PS5 and the rest have no
  serial. Main takes `'A' + b % 26` for Xbox One. The two unique serials
  are different strings for the same pad, so neither can be derived from
  the other. From protocol 1.3, the control channel gives the unique id
  itself (STRING 0x05, the board ID), and both serials can be made from it
  (`cache::PadId::serials_in`).
- **BOOTSEL / PICOBOOT:** the config lives at 0x101F8000 (32 KB, nanopb
  `Config`, followed by a 12-byte footer of `{size, crc32, magic 0xD2F1E365}`).
  This allows an offline backup and a mode change with no network.
  gp2040ce-binary-tools (Python) is the prior art.
- **Firmware drift:** endpoints and setters differ between v0.7.12 and
  main ("Firmware lines" below). The tool works out the line, reads only
  that line's allowlist, and round-trips any JSON fields it doesn't know.
  The S16's stock build is 0014e4a (v0.7.12, Haute42COSMOX).

## Fork firmware

Our fork is [`rdlu/GP2040-CE`](https://github.com/rdlu/GP2040-CE). The
status of the fork, its branches and the to-do list are in
[#2](https://github.com/rdlu/gamepadii/issues/2).

### Boards

There are three support levels:

| Level | Boards |
|---|---|
| **Tested** | Haute42 S16 (`Haute42COSMOX`) |
| **Built, not tested** | First the other Haute42 models: `Haute42COSMOXMLite`, `Haute42COSMOXMUltra`, `Haute42COSMOXXAnalog`; on the main line also `Haute42COSMOXCAS` and `Haute42COSMOXCAT`. Later the Raspberry Pi Pico (`Pico`) and Pico 2 (`Pico2`). |
| **Needs a maintainer** | All other boards and chips |

A board moves to "Tested" when a person with that board tests it. All the
Haute42 configs use the RP2040. Most RP2350 boards are only on upstream
main, so Pico 2 builds need the main line (#21).

### Firmware lines

| Branch | Base | What it has |
|---|---|---|
| `gamepadii/main` | upstream main | the fixes, control channel v1.2 (lights), v1.3 (board ID), v1.4 (profile slots), v1.5 (LED profiles), v1.6 (config blob) and v1.7 (input snapshot). On the S16 now (`v0.7.12-133-g48a9521`). The tests of #21 step 5 are not complete. |
| `gamepadii/control-channel-led` | v0.7.12 | retired: the fixes, control channel v1.1 (LEDs). The tags `archive/v0.7.12/*` keep its branch heads. |

The main line is our one firmware line. gamepadii still supports pads on
the v0.7.12 line. Older branches (`gamepadii/freeze-investigation`,
`gamepadii/control-channel`) are kept for reference.

### The fixes

On top of v0.7.12: I2C access serialised between the cores with bounded
scan probes (a1b069d), web-config replies built in one exactly sized buffer
(7822dc7), lwIP's `fs_file` matching lib/httpd's (2435d1d), reply buffers
kept until lwIP has sent them (5d6f73a), and a 503 instead of running out
of heap (85ca0f6). `getConfig` and `getI2CPeripheralMap` were stress-tested
10/10 to 20/20 rounds ok on every build with the fixes. The main line has
the same fixes, ported.

**The capability check** (`api::fork::ForkFixes::check`, through
`Client::fork_fixes`). A pad has the fixes when:

1. its commit, the `-g<hash>` of the version string (`git describe`) or
   `boardBuild`, is one of `FIXED_BUILDS`: our builds that were
   stress-tested (85ca0f6, 8419a38, f19c11b, f507f0b, 5eea8a6, 1008647,
   0a23ea3);
   or
2. the control channel answered HELLO (only the fork has it, and its branch
   sits on the fixes) and the version isn't a bare release tag.

Anything else is stock, a bare tag such as `v0.7.12` above all, even with
something on EP0 that passes for the channel. So is stock upstream main
(`STOCK_MAIN_BUILDS`: 8013b41, `v0.7.12-119-g8013b41`), whatever answers:
it has upstream's I2C fix (#1739) but `getConfig` still runs it out of
heap, and our fixes for that are upstream PRs (#1781-#1783), not merged.
Our builds on main join `FIXED_BUILDS` (and `line::KNOWN_BUILDS`, as main)
once they pass the same stress test. Someone else's dev build
(`v0.7.12-3-g1234567`) isn't necessarily fixed, so it qualifies only with
the channel. The channel's STATUS and STRING have no feature field for the
fixes, so the version is the signal; a new fork build goes into
`FIXED_BUILDS` once it has passed the stress test. `getConfig` and
`getI2CPeripheralMap` take a `ForkFixes`, which only the check makes, so a
stock pad can't be asked.

Used by `backup`: on a fork build the bundle gets `full_config`
(`{"evidence", "config", "error"}`), read after every per-section endpoint;
a failure there (a 503, a timeout) is recorded and the rest is kept. On
stock it is `null`. `flow::backup` adds the channel's HELLO as evidence: on
a trip, the one from before the reboot; in web-config, one HELLO there.
`commands::full_config` returns the document for `gamepadii leds`
(`animationOptions` is only there, #19). The mock hangs on both endpoints
while its firmware version is a bare release tag, and the fork fixture
(`tests/fixtures/api/s16-fork/`, an overlay of the stock capture with a
synthetic `getConfig`) serves them.

## Firmware lines (v0.7.12 and main)

Upstream main (8013b41, 119 commits past v0.7.12, unreleased) changed the
web API, so gamepadii supports two lines (#21): `v0.7.12` (the release and
builds on it, our fork's included) and `main`. Everything here was read in
main's `src/webconfig.cpp`, `proto/` and `src/config_utils.cpp` at 8013b41;
no pad has run main yet, and the fixtures in `tests/fixtures/api/s16-main/`
are synthetic (see its README).

**Which line** (`api::line`, `Client::line`, remembered per connection).
Main's version string is `v0.7.12-119-g8013b41`, the same form as our fork's
`v0.7.12-6-gf507f0b`, and a commit count is no line, so:

1. a commit we know (the `-g<hash>`, or `boardBuild`) in `KNOWN_BUILDS`
   gives its line: the S16's stock 0014e4a, every fork build (all v0.7.12
   so far; a test makes each `FIXED_BUILDS` entry declare its line) and
   upstream's 8013b41 (main);
2. else the bare tag `v0.7.12` is the release line;
3. else the shape of `getGamepadOptions`: main's getter writes the 15
   `switch2Pro*` keys (Switch 2 Pro mode, after v0.7.12), v0.7.12's has
   none. It is a plain read on both lines and one gamepadii does anyway
   (`info` reuses its read, no second request).

`getFirmwareVersion` itself is the same on both lines. No probe asks for
an endpoint that one line lacks: with lwIP's `LWIP_HTTPD_DYNAMIC_HEADERS`
off and no 404 page in the firmware's file system, an unknown path most
likely gets no reply at all (a closed connection), which reads as a
dropped pad (from the lwIP 2.2.1 source; unverified on a pad). The mock
answers a 404 instead and the tests check that nothing outside the line's
allowlist is asked. A line's guess only decides what is read and which
keys a setter needs; a wrong guess fails safe (a refused write, a 404).

**Main's allowlist** (`FirmwareLine::Main.endpoints()`): v0.7.12's, minus
what main removed (`getCustomTheme`, `getHETriggerCalibration`,
`getHETriggerOptions`; `setHETriggerOptions` is still there, write-only),
plus its new reads that are clearly plain. Every new main GET, by its
handler:

| endpoint | what the handler does | allowed |
|---|---|---|
| getBootModeOptions | storage read, 1.6 KB document (`JSON_OBJECT_SIZE(100)`) | yes |
| getBoardDefinition | storage read and pin constants (`isAdcPin` is arithmetic), 1.6 KB | yes |
| getHETriggerCalibrations | storage read of the 32 HE triggers, 8 KB document like v0.7.12's `getHETriggerOptions` | yes |
| getAnimationProtoOptions | storage read of the LED profiles, a 16 KB document (twice v0.7.12's biggest getter), about 4 KB out; the web UI's LED page asks for it | yes |
| getLightsDataOptions | storage read of the light clusters, a 16 KB document, under 2 KB out; the LED page asks for it | yes |
| getLightsPresets/0..7 | board-config constants (`LIGHT_DATA_*`), not settings, a 16 KB document each | no: nothing to back up |
| getLightsDataPresets | the same, all eight in a 32 KB document | no: large heap, not settings |
| getHeldPins | `gpio_init`s the unused pins, then busy-waits up to 5 s for a press (main's version of v0.7.12's) | no: blocks, side effects |
| getHETriggerVoltage | v0.7.12's `getHETriggerCalibration` renamed: reads the previous POST body, sets mux select pins, samples the ADC | no: GPIO and ADC |
| getJoystickCenter, getJoystickCenter2 | with the analog add-on on, `adc_init()` and ADC samples on its pins | no: ADC |

The two 16 KB ones are the remaining heap risk (the S16 has about 100 KB
of free heap, and main keeps each reply in its own buffer until it is
sent); they are the first things to stress-test on a main pad.
`getConfig` and `getI2CPeripheralMap` stay behind `ForkFixes` on main too:
main has the I2C fix, but `getConfig` still hangs stock main.

**Setters on main** (`api::write`, which takes the line). Both are the same
kind of `readDoc`-everything setter as on v0.7.12, with more keys:

- `setGamepadOptions` reads the 15 `switch2Pro*` numbers on top of
  v0.7.12's 46 keys (`MAIN_SETTER_KEYS`: identity, two colour presets and
  twelve `uint32` colours), so a body without them zeroes them. Nothing
  else changed: the USB ids are still `%04X` strings out and integers in,
  the USB strings still `strncpy`'d, the hotkeys still fold the dpad mask.
- `setDisplayOptions` reads `displayContrast` (a `uint32`, default 255) on
  top of v0.7.12's 22 + 10.

The guards refuse a body missing any key the pad's line reads, so a main
pad whose getter lacked one is never written. The mock's setters keep the
getter's keys, which on the main fixture are main's, so it zeroes what a
partial body leaves out on both lines.

**Getters gamepadii reads, main vs v0.7.12:** `getGamepadOptions` and
`getDisplayOptions` gain the keys above; `getPinMappings`,
`getProfileOptions` (an exactly sized document now), `getKeyMappings`,
`getFirmwareVersion` and `getMemoryReport` keep their shape on the RP2040
(pins 30-47 only on the RP2350). `getLedOptions` lost the LED layout, the
button-to-LED map, the player-LED indexes and colour and the case RGB, and
prints `brightnessMaximum` as a percentage (the setter turns it back with
`int(p * 2.55f + 0.5f)`). `getButtonLayouts` dropped `ledLayout`;
`getAddonsOptions` gained analog calibration, ADS1115 and profile-slider
keys. Input modes: P5General (16), SInput (17) and Switch 2 Pro (19) are
in `proto/enums.proto` and already in `mode.rs`. Hotkeys 52-57 were
renumbered for LED profiles and 60-70 removed (gamepadii names no hotkey
actions).

**LEDs on main.** AnimationStation has LED profiles instead of v0.7.12's
animation, static colours and themes: `AnimationOptions` keeps
`brightness`, `baseProfileIndex`, `autoDisableTime`, `customColors` and up
to four `profiles` (idle, pressed and case effects from
`AnimationNonPressedEffects` / `AnimationPressedEffects`, per-GPIO colour
indexes), and the lights are clusters tied to GPIO pins
(`lightClusterData`). `leds` on main reads `getLedOptions`,
`getAnimationProtoOptions` and `getLightsDataOptions` and reports
`lights` (the start-up profile, each profile's effects by name, the light
count, the idle timeout); v0.7.12's animation, colours and theme stay
`null`, never mapped. Over the channel, main has the v1.2 lights commands
in place of v0.7.12's LED commands: `leds` reads LIGHTS_STATUS and `leds
set --lights-profile N|off --brightness ...` sends SET_LIGHTS. v1.2 does
not change the effects of a profile.

**To verify on a main pad** (the main session's captures replace the
synthetic fixtures): every allowlisted getter's real shape and its answer
(no hang) on the S16, stress-tested like `getConfig` was, the two 16 KB
ones above all; that an unknown endpoint gets no reply rather than a 404;
the `switch2Pro*` marker on the real getter; a `setGamepadOptions` and a
`setDisplayOptions` round trip (`GAMEPADII_LIVE_WRITE=1`, with the user's
OK); and the converted config's values after main's first boot.

## The control channel (gamepadii firmware)

Our fork answers vendor requests on EP0 in every mode, web-config
included, with no descriptor change; the wire protocol and its firmware
side are in [design/control-channel.md](design/control-channel.md).
Decided in issue #2: no BOOTSEL over it (gamepadii goes through web-config
for that), `profile switch` is RAM-only unless `--save`, and the udev rule
is opt-in through `gamepadii setup udev`.

Host side (`gamepadii-core/src/channel/`):

- `ControlTransport` is one vendor control IN or OUT. On Linux it is an
  `ioctl(USBDEVFS_CONTROL)` on `/dev/bus/usb/BBB/DDD` opened `O_RDWR`
  (`usbfs.rs`, through `libc`); vendor requests need no claimed interface,
  so xpad and usbhid stay bound. Errnos map to STALL, no access (`EACCES`:
  "run `gamepadii setup udev`", exit 5), gone (`ENODEV`, `ESHUTDOWN`,
  `EPROTO`: expected after a reboot) and timeout.
- `probe` sends HELLO (200 ms timeout) with a random non-zero nonce and
  accepts only a reply with the `GPii` magic, our nonce and major 1. A
  STALL, a short reply (stock XInput sends 0 bytes) or garbage (stock Xbox
  One sends uninitialised stack) mean "no channel", and the flows fall back
  to the guided button presses.
- `Channel` caches the HELLO (capabilities, token, bootable modes) and
  sends the commands with the token and a sequence number; a STALL on a
  command is explained by reading STATUS.
- Device nodes are opened under the sysfs `Root`, so a fixture root (tests,
  `--sysroot`) never reaches real hardware.
- Pads seen with the channel are recorded per board in
  `~/.cache/gamepadii/control-channel.json`; an `EACCES` on a pad like that
  adds the udev hint to the guide (stock pads stay quiet).
- `channel/sim.rs` is the firmware side, simulated (both stock behaviours
  and the channel, including the §3.6 details), used by `sim::SimPad` and
  the CLI's hidden `--mock-channel`.
- Web-config mode keeps the HTTP path for `mode` (with its backup) and
  `reboot`; the channel there is only probed by `status`, and by `backup`
  (one HELLO, as evidence for the fork's fixes).
- `info` and `backup` from a gamepad mode (`flow::through_webconfig`):
  REBOOT(1), wait for `cafe:4028` and the HTTP server, read, then
  `/api/reboot {"bootMode":0}` and check the VID:PID and STATUS show the
  mode it was in (`--stay` skips the return). On any failure past the
  REBOOT it still sends the pad back, over the channel if HTTP doesn't
  answer, and the error says how that went. The restart drops a RAM-only
  profile (STATUS before ≠ STATUS after): gamepadii switches back to it with
  a RAM-only SET_PROFILE, checks STATUS, updates the cache and notes
  "restored profile N (RAM only)" (`trip.restored_profile`). That puts back
  the mapping the user had, so the user allowed it as automatic; flash is
  never touched. A failed restore is a warning (stderr and the note), not
  an error: the read worked. With `--stay` nothing is switched (the pad
  stays in web-config); the note says the profile was dropped. Decided in
  #2 (follow-up 3; the restore decided later). `info` labels its profile
  rows `saved` (`profileNumber`, all web-config knows) and `running` (the
  STATUS taken after the trip, `trip.running_profile`); without a trip, or
  when STATUS doesn't answer, running is unknown (`null`), never the saved
  one (#14).
- The device cache (`cache.rs`) records the profile STATUS reports
  (`running_profile`) beside the saved `profileNumber`: `profile switch`
  sets it (and the saved one with `--save`), any STATUS refreshes it
  (written only on a change), and a reboot gamepadii causes clears it. The
  guide's key hints take the live STATUS profile, else that record, else
  the saved one. Decided in #2 (follow-up 2).
- The device cache is kept per pad. From control channel 1.3, the key is
  the board config and the board ID, in every mode and in web-config:
  `~/.cache/gamepadii/<board>-<board ID>.json`. The board ID is stable,
  but not always unique: a flash chip can give a fixed value (the S16 gives
  `C008856.` in ASCII). When two connected pads give the same one, `status`
  adds the USB port to the key and shows a warning. Without the board ID,
  the key is the serial the pad has in a
  gamepad mode that has a unique one (XInput, Xbox One; `cache::PadId`,
  #11): `~/.cache/gamepadii/<board>-<serial>.json`. A board ID and a
  serial compare: the firmware makes the serial from the ID, so the first
  read with the ID replaces the file by serial. Web-config can't say
  which pad it is, so whenever gamepadii sees a pad with such a serial go
  into web-config (the trip, `webconfig`, `reboot webconfig`, the guided
  `mode`), it records that serial against the web-config enumeration
  (bus, port, device number) in `webconfig-sessions.json`; reads in that
  enumeration go to that pad. A replug or a reboot gets a new device
  number, so a session never outlives the enumeration. A read it can't
  attribute (into web-config by hand, or from a mode with no unique
  serial) goes to `<board>.json` as before, with the enumeration it was
  read in, and is moved to the pad's file when gamepadii sees that
  enumeration come back in a mode with a serial (`webconfig --exit`,
  `reboot gamepad`, `mode` from web-config). Lookups prefer the pad's own
  file, however old, then the newest that can be this pad, never another
  pad's (same mode, another serial). Board files from before still load,
  and the first read attributed to a pad of that board replaces them.
- A HELLO that shows no channel is recorded for that connection (bus, port,
  device number, `no-channel.json`), and that connection is not asked
  again: `status` in a waybar module must not send vendor requests to a
  real Xbox 360 pad every few seconds. The other rules for devices that
  are not ours are in `docs/design/control-channel.md` §5.4.
- LEDs (#19, protocol v1.1, capability bit 8: LED_STATUS and SET_LED,
  `control-channel.md` §3.7). `gamepadii leds` (`flow::leds`,
  `leds.rs`) takes the best route there is:
  1. LED_STATUS, in any mode, with no reboot;
  2. otherwise web-config: `getLedOptions` and `getCustomTheme`, plus
     `getConfig`'s `animationOptions` on a fork build (`ForkFixes`). From a
     gamepad mode it gets there through the same trip as `info`;
  3. on stock firmware, the animation, brightness and colours are `null`
     ("unknown on stock firmware");
  4. on main firmware (no LED commands there yet), web-config's LED
     profiles instead ("Firmware lines" above).

  `leds set` and `leds brightness` use SET_LED only, never web-config.
  Changes are RAM only unless `--save`, with the same caveat as
  `profile switch`: any later save keeps them. The host checks each value
  against the counts in LED_STATUS before sending (exit 3). A pad without
  the capability gets exit 5. Every change is verified by polling
  LED_STATUS (1 s). In PS4/PS5 mode the firmware skips saves while the
  controller authentication needs the flash, so an unconfirmed save is a
  note, not an error. The names (`EFFECTS`, `COLORS`, `THEMES`) are
  v0.7.12's `AnimationEffects`, `AnimationColor` and the static theme
  order, and STRING `0x20 + n` names any effect past the table.
  `--format waybar` reads over the channel only: a status bar must never
  cause a reboot. LED profile blocks and a TUI panel come later.

## Profiles live in gamepadii, not on the pad

gamepadii keeps a host-side **profile library** with no size limit, stored
in `~/.config/gamepadii/profiles/`. Profiles are
written in **KDL 2.0** (the `kdl` crate), decided 2026-10-17. The pad is only a target that
gamepadii writes profiles into. The library is the source of truth, so the
device's 6 slots and its single keymap don't limit how many profiles you
have.

- **A profile is a full bundle.** Any of these can be included, and anything
  left out is inherited from a base profile: input mode, gamepad options
  (SOCD, D-pad mode, hotkeys), a pin mapping, and a keyboard keymap. Examples:
  `silksong`, `nms-keyboard`, `sidekick-obs`.
- **`profile pull`** imports what is on the device now (all 6 slots plus the
  keymap) into the library.
- **`profile apply <name>`** writes the bundle to the device. In gamepad
  mode it takes one guided button press and one reboot.
- **`slots load a b c …`** picks up to 6 library profiles that share an
  input mode and puts them in the device's slots, so the on-pad hotkeys can
  switch between them with no PC involved. For keyboard profiles this uses
  the superset trick: one 32-slot keymap with the pin mappings choosing keys
  per slot, and gamepadii reports when they don't fit.
- **Key combos** (for Adobe-style sidekicks): in v0.7.12 keyboard mode, one
  button sends exactly one key. Modifiers count as keys (`KeyboardDriver.cpp:31`),
  so Ctrl+Z needs two buttons held together. There are three ways to put a
  combo on one button:
  - the Macro addon (it presses several logical buttons; whether it works in
    keyboard mode is unverified)
  - the host remapper
  - a keymap extension in our firmware fork
- **Later: a host remapper.** The pad stays in XInput/HID while gamepadii
  turns its buttons into keys through evdev → uinput. Switching per app is
  then instant and unlimited, for the "sidekick keyboard". It only works
  where gamepadii is running.

## Architecture

```
crates/gamepadii-core/src/
  discovery.rs   sysfs scan → Device { mode, vid_pid, sysfs path, netdev }; Scan trait
  wait.rs        wait_for(predicate, timeout) over a Clock (ManualClock in tests)
  mode.rs        InputMode / BootMode enums, VID:PID table
  transport/     http.rs (own HTTP/1.0 client, serialised), mock.rs (fixtures)
  channel/       control channel: proto.rs (wire v1, v1.1 LEDs), usbfs.rs (Linux),
                 sim.rs (simulated firmware), mod.rs (probe, Channel)
  udev.rs        the opt-in uaccess rule (70-gamepadii.rules)
  api/           typed endpoint structs + raw serde_json::Value passthrough;
                 line.rs (v0.7.12 vs main, per-line allowlists), fork.rs,
                 write.rs (the setters, per-line key sets)
  client.rs      connect, firmware probe
  buttons.rs     logical buttons, labels, per-profile GPIO → button, key hints
  cache.rs       last web-config read per pad (~/.cache/gamepadii/<board>-<board ID or serial>.json,
                 else <board>.json), the profile the channel last reported
                 running, and which pad went into which web-config session
  flow.rs        mode / webconfig / reboot / profile switch / leds over
                 Scan + Clock, the control channel first, the guided flows
                 as fallback; the web-config round trip for info and backup
  leds.rs        LED names (v0.7.12 enums), LED_STATUS and web-config reads
                 into one LedState, the SET_LED plan for `leds set`
  sim.rs         a simulated pad (Scan + mock Transport + EP0) for flow tests
  profile.rs     KDL profile files (~/.config/gamepadii/profiles), diff/apply
  commands.rs    status, info, backup, profile … → plain data
crates/gamepadii/src/
  main.rs cli.rs output.rs
  tui/           mod.rs (terminal loop), app.rs (state: keys and replies in,
                 jobs out, no terminal), ui.rs (drawing), keys.rs (vim-first
                 BINDINGS), worker.rs (core commands and flows on a std
                 thread, progress back over a channel)
```

The TUI holds no logic of its own about the pad: every job is a
`gamepadii-core` command or flow (`status_with`, `mode`, `profile_switch`,
`webconfig`, `reboot_pad`, `through_webconfig` + `backup_in_dir`), and the
results panel shows `output.rs`'s human rendering of the report. The worker
builds the flow `Env` per job on its own thread; the UI only sends jobs and
reads replies. While idle it sends a sysfs-only scan once a second and asks
for a full `status` (HELLO included) only when the devices or their device
numbers change, so the control channel isn't polled.

## Testing

- The JSON fixtures come from GP2040-CE's `www/server/app.js` mock and from
  live captures of the S16, taken with a `capture` subcommand.
- insta snapshots cover the human output, the JSON output, the TUI (rendered
  through `TestBackend`) and `--help`.
- The simulated pad (`sim.rs`) keeps one simulated control channel for each
  USB node in a process. The hidden `--mock-channel` flag uses it, so the TUI
  sees the result of its last action, the same as on a real pad.
- The README demos (`docs/demos/`, `mise run demos`) use the same simulated
  S16, so that they are the same each time.
- Live tests are `#[ignore]` and run through `mise run test-live`. On the
  real pad, agents may read and reboot it; any write needs the user's OK and a
  backup first.

## Roadmap

The roadmap, milestones and open work are tracked in GitHub issues, under the
epic [#1](https://github.com/rdlu/gamepadii/issues/1). This file keeps the
facts, decisions and architecture they build on.

## References

- `~/Projects/GP2040-CE`: upstream firmware clone (main, plus the v0.7.12 tag).
  The API lives in `src/webconfig.cpp`, the schema in `proto/config.proto` and
  `proto/enums.proto`, the mock server in `www/server/app.js`, and the web
  client in `www/src/Services/WebApi.js`.
- `~/Projects/gp2040ce-binary-tools`: Python tools for the PICOBOOT config
  dump and edit, and the binary format.
