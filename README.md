# gamepadii

A Linux CLI for GP2040-CE gamepads, with a ratatui TUI and a Tauri GUI to
follow. It finds the pad on USB, tells which input mode it is in (XInput,
Keyboard, HID, Switch, PS4/PS5, web-config, BOOTSEL, ...), and will switch
modes and apply profiles (pin mappings, keyboard keymaps) without the
built-in web configurator, with as few button presses and reboots as the
firmware allows.

**Status: pre-release, in development.** Only `status` works so far; the
plan and milestones are in [docs/PLAN.md](docs/PLAN.md).

The documentation will live at **<https://gamepadii.dlu.one/>**.

## Install

There are no releases yet; build from source (Rust 1.88+):

```sh
cargo install --locked --git https://github.com/rdlu/gamepadii gamepadii
```

or `mise run install` in a clone.

## Commands

```text
gamepadii status                  the pad: input mode, USB ids, bus and port
gamepadii status --json           the same as JSON ("schema": 1)
gamepadii status --format waybar  one line for a waybar custom module
```

Exit codes are stable: 0 ok, 1 check failed, 2 no device found, 3 usage,
4 transport, 5 needs a button press on the pad. With `--json` a failure
still prints a JSON object (`{"schema": 1, "error": {...}}`), and waybar
always gets a valid line.

### How the mode is detected

In gamepad mode GP2040-CE has no control channel, so `status` reads the USB
vendor and product ids from sysfs and looks them up in a table taken from the
firmware's descriptors. Most of those ids are borrowed from real controllers
(XInput is `045e:028e`, an Xbox 360 pad), so the result carries a
confidence: `confirmed` when the ids or strings can only be GP2040-CE
(`cafe:*`, a `GP2040-CE` product string), `likely` otherwise. Web-config
(`cafe:4028`) also reports its network interface.

## Supported devices

- **Haute42 S16** (leverless), tested. It runs stock GP2040-CE v0.7.12
  with the `Haute42COSMOX` board config.
- Other GP2040-CE controllers should work the same way: detection relies on
  the firmware, not the board.

## License

MIT, see [LICENSE](LICENSE).
