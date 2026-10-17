# gamepadii — design

A Linux CLI, then a ratatui TUI, then a Tauri GUI, for GP2040-CE gamepads.
The first target is a Haute42 S16 leverless. The goal is to stop using the
built-in web configurator: check status, switch input modes, and apply
profiles (pin mappings, keyboard keymaps) with as few button presses and
reboots as possible.

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
- **Git:** commits go straight to `main` until TUI work starts. From the
  first TUI change on: feature branches, PRs, Conventional Commits, and a
  protected `main` (decided 2026-10-17).
- **Order after the MVP** (decided 2026-10-17): the firmware fork and its
  control channel first, then the TUI, then the rest. Linux first; Windows
  and macOS wait until the GUI. The core stays portable and Linux-only code
  stays behind cfg gates.
- **Firmware fork:** fine to carry, upstream first: freeze reports, then a
  design discussion for the control channel (decided 2026-10-17).

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
  - The server ignores the HTTP method, and a GET to a `set*` endpoint replays
    the previous POST body.
  - POST bodies are capped at 16 KB, and requests must be serialised.
  - `resetSettings` is a destructive GET.
  - **Never call `getConfig` on v0.7.12.** On the S16 (Haute42COSMOX build
    0014e4a) it hangs the network stack: USB stays enumerated, the device
    stops answering ping, and only a replug brings it back (seen
    2026-10-17). Back up through the per-section `get*` endpoints instead,
    as the web UI does.
  - **Never call `getI2CPeripheralMap` either.** It runs a live I2C bus scan,
    and that hung the S16 in the same way. The client keeps an explicit
    allowlist of endpoints verified as safe to read; a `get*` name alone
    doesn't make an endpoint safe. Verified safe on the S16 so far (the
    allowlist in `gamepadii-core/src/api/mod.rs`): getAddonsOptions,
    getButtonLayoutDefs, getButtonLayouts, getCustomTheme,
    getDisplayOptions, getExpansionPins, getFirmwareVersion,
    getGamepadOptions, getHETriggerCalibration, getHETriggerOptions,
    getKeyMappings, getLedOptions, getMacroAddonOptions, getMemoryReport,
    getPeripheralOptions, getPinMappings, getProfileOptions,
    getReactiveLEDs, getSplashImage, getUsedPins, getWiiControls. Their
    responses are the fixtures in `tests/fixtures/api/s16-v0.7.12/`.
    `getHeldPins` blocks until a button is pressed and stays out of the
    read path; it gets its own function with the button test.
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
  byte (`'A' + b % 25`) and then `8ABCDEFGH`. Web-config always says
  `123456`, Switch Pro `000000000001`; SInput and Switch 2 Pro send fixed
  strings, and Keyboard, HID, Switch, PS3, PS4/PS5 and the rest have no
  serial. The two unique serials are different strings for the same pad,
  so neither can be derived from the other. The control channel (v1)
  exposes no id either: the HELLO token mixes the unique id in, but it is
  random per boot.
- **BOOTSEL / PICOBOOT:** the config lives at 0x101F8000 (32 KB, nanopb
  `Config`, followed by a 12-byte footer of `{size, crc32, magic 0xD2F1E365}`).
  This allows an offline backup and a mode change with no network.
  gp2040ce-binary-tools (Python) is the prior art.
- **Firmware drift:** endpoints differ between v0.7.12 and main. The tool
  probes `getFirmwareVersion` and endpoint availability, and round-trips any
  JSON fields it doesn't know. The S16's exact build is still unknown; it may
  be a Haute42 vendor build.

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
  `reboot`; the channel there is only probed by `status`.
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
- The device cache is kept per pad, keyed by the serial the pad has in a
  gamepad mode that has a unique one (XInput, Xbox One; `cache::PadId`,
  #11): `~/.cache/gamepadii/<board>-<serial>.json`. Web-config can't say
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
  and the first read attributed to a pad of that board replaces them. A
  STRING with the unique id on the control channel would make this exact
  in every mode; that needs a firmware change (protocol v1.1).

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
  channel/       control channel: proto.rs (wire v1), usbfs.rs (Linux),
                 sim.rs (simulated firmware), mod.rs (probe, Channel)
  udev.rs        the opt-in uaccess rule (70-gamepadii.rules)
  api/           typed endpoint structs + raw serde_json::Value passthrough
  client.rs      connect, firmware probe
  buttons.rs     logical buttons, labels, per-profile GPIO → button, key hints
  cache.rs       last web-config read per pad (~/.cache/gamepadii/<board>-<serial>.json,
                 else <board>.json), the profile the channel last reported
                 running, and which pad went into which web-config session
  flow.rs        mode / webconfig / reboot / profile switch over Scan + Clock,
                 the control channel first, the guided flows as fallback;
                 the web-config round trip for info and backup
  sim.rs         a simulated pad (Scan + mock Transport + EP0) for flow tests
  profile.rs     KDL profile files (~/.config/gamepadii/profiles), diff/apply
  commands.rs    status, info, backup, profile … → plain data
crates/gamepadii/src/
  main.rs cli.rs output.rs tui/{mod,app,ui,keys}.rs
```

## Testing

- The JSON fixtures come from GP2040-CE's `www/server/app.js` mock and from
  live captures of the S16, taken with a `capture` subcommand.
- insta snapshots cover the human output, the JSON output, the TUI (rendered
  through `TestBackend`) and `--help`.
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
