# gamepadii — plan

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
- **Git:** the MVP lands on `main`. After that, feature branches, PRs,
  Conventional Commits, and a protected `main`.

## What the firmware allows (GP2040-CE, checked at v0.7.12 and main 8013b41)

- **Gamepad mode has no control channel.** In XInput, HID, Switch and the
  other modes there is no vendor report and no serial interface. The only ways
  into web-config are physical:
  - S2+B3+B4 while running (this toggles between gamepad and web-config)
  - holding S2 at plug-in
  - the OLED mini-menu
  - since v0.7.12, the reboot-to-webconfig hotkey

  The tool therefore *guides* the user ("press S2+B3+B4"), detects the
  re-enumeration through udev, and carries on from there.
- **Web-config mode:**
  - The device enumerates as `CAFE:4028` (RNDIS/ECM). It is reachable at
    192.168.7.1 and its own DHCP server hands out the host address.
  - The JSON HTTP API is in `src/webconfig.cpp`. `POST /api/reboot
    {"bootMode":0|1|2}` returns it to gamepad mode, web-config or BOOTSEL.
- **API hazards**, which the client must guard against:
  - `setGamepadOptions` zeroes every key it isn't sent, so always do a full
    read-modify-write.
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
  profile.
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
- **BOOTSEL / PICOBOOT:** the config lives at 0x101F8000 (32 KB, nanopb
  `Config`, followed by a 12-byte footer of `{size, crc32, magic 0xD2F1E365}`).
  This allows an offline backup and a mode change with no network.
  gp2040ce-binary-tools (Python) is the prior art.
- **Firmware drift:** endpoints differ between v0.7.12 and main. The tool
  probes `getFirmwareVersion` and endpoint availability, and round-trips any
  JSON fields it doesn't know. The S16's exact build is still unknown; it may
  be a Haute42 vendor build.

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
  discovery.rs   udev scan + hotplug watch → Device { mode, vid_pid, sysfs path, netdev }
  mode.rs        InputMode / BootMode enums, VID:PID table
  transport/     http.rs (ureq, serialised, retries), mock.rs (fixtures)
  api/           typed endpoint structs + raw serde_json::Value passthrough
  client.rs      read-modify-write helpers, capability probing, reboot + wait
  profile.rs     TOML profile files (~/.config/gamepadii/profiles), diff/apply
  commands.rs    status, info, mode, webconfig, backup, profile … → plain data
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

## Milestones

0. **Scaffold** (on `main`): the workspace, LICENSE, README, CLAUDE.md,
   CHANGELOG, mise.toml with the `check` gate, CI, `git init`, and the GitHub
   repo.
1. **`status`**: udev discovery and mode detection from VID:PID, with human
   and JSON output plus waybar. It already works with the pad in XInput.
2. **Web-config client**: `info` (firmware, board, memory), `config dump`,
   `backup` / `restore --dry-run`. Fixtures come from the mock and from a
   live capture.
3. **Guided mode flow**: `webconfig` (wait for the button press, then open or
   print the URL), `mode <xinput|keyboard|hid|switch|ps4|…>` (guide into
   web-config, read-modify-write `inputMode`, reboot, wait, verify), and
   `reboot <gamepad|webconfig|bootsel>`. This is the MVP; then switch to the
   PR workflow and protect `main`.
4. **Profile library**: KDL bundles with `profile
   list/show/diff/apply/pull`, plus example profiles for No Man's Sky and the
   sidekick.
5. **TUI**: status, mode switcher, profile browser with diffs, live
   button-test view (`getHeldPins`).
6. **`slots load`**: pack library profiles into the 6 device slots, using
   the superset keymap for keyboard profiles, and set up the hotkeys.
7. **PICOBOOT transport**: offline backup/restore and mode change with no
   web-config.
8. **Host remapper**: evdev → uinput, with per-app profiles.
8b. **Firmware fork: fixes and a control channel.** Build the Haute42COSMOX
   UF2 from `~/Projects/GP2040-CE` (pico-sdk 2.1.1, arm-none-eabi-gcc from
   pacman).
   - First, the two freeze fixes from branch `gamepadii/freeze-investigation`.
   - Then a gamepadii control channel: USB vendor control requests on EP0,
     caught in `tud_vendor_control_xfer_cb` (`src/usbdriver.cpp:66`) before
     the active driver sees them. That needs no descriptor change, so consoles
     don't notice. It provides status, live profile switching, setting the
     input mode with a reboot, rebooting into web-config or BOOTSEL, and later
     reading and writing config sections as protobuf.
   - It is off unless enabled in config. gamepadii detects it and otherwise
     falls back to stock web-config.
   - Upstream gets it first: open the freeze issues, then a design discussion
     for the channel. If upstream declines, we keep our own fork; the firmware
     changes slowly, so carrying a fork is cheap.
9. **Site and release**: the Hugo + Hextra docs site, release binaries, AUR.
10. **Tauri GUI** on top of `gamepadii-core`.

## References

- `~/Projects/GP2040-CE`: upstream firmware clone (main, plus the v0.7.12 tag).
  The API lives in `src/webconfig.cpp`, the schema in `proto/config.proto` and
  `proto/enums.proto`, the mock server in `www/server/app.js`, and the web
  client in `www/src/Services/WebApi.js`.
- `~/Projects/gp2040ce-binary-tools`: Python tools for the PICOBOOT config
  dump and edit, and the binary format.
