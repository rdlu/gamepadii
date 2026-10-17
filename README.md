# gamepadii

A Linux CLI and TUI for GP2040-CE gamepads, with a Tauri GUI to follow. It finds the pad on USB, tells which input mode it is in (XInput,
Keyboard, HID, Switch, PS4/PS5, web-config, BOOTSEL, ...), and will switch
modes and apply profiles (pin mappings, keyboard keymaps) without the
built-in web configurator, with as few button presses and reboots as the
firmware allows.

**Status: pre-release, in development.** `status`, `info`, `backup`,
`reboot`, `mode`, `webconfig`, `profile switch` and `setup udev` work so
far, and a first TUI (`gamepadii` on a terminal) covers status, modes,
profiles, web-config and backups. The roadmap is tracked in
[GitHub issues](https://github.com/rdlu/gamepadii/issues/1), and the design
notes are in [docs/DESIGN.md](docs/DESIGN.md).

The documentation will live at **<https://gamepadii.dlu.one/>**.

## Install

There are no releases yet; build from source (Rust 1.88+):

```sh
cargo install --locked --git https://github.com/rdlu/gamepadii gamepadii
```

or `mise run install` in a clone.

## Commands

```text
gamepadii                         the TUI, on a terminal (usage otherwise)
gamepadii tui                     the same, explicitly
gamepadii status                  the pad: input mode, USB ids, bus and port
gamepadii status --json           the same as JSON ("schema": 1)
gamepadii status --format waybar  one line for a waybar custom module
gamepadii info                    firmware, board, memory, input mode,
                                  profiles and boot-mode buttons
gamepadii backup [-o FILE]        every readable setting in one JSON file
                                  (both from a gamepad mode too, with the
                                  control channel; --stay stays in
                                  web-config)
gamepadii reboot <gamepad|webconfig|bootsel>
                                  restart, wait for the pad on USB, and
                                  check what came back
gamepadii mode                    the current and saved input mode, and
                                  the modes there are (with boot buttons)
gamepadii mode <MODE>             switch: xinput, keyboard, hid, switch,
                                  ps4, ps5, ... (gamepadii mode --help)
gamepadii webconfig [--open]      get the pad into web-config, print the URL
gamepadii webconfig --exit        back to gamepad mode
gamepadii profile switch <N|LABEL> [--save]
                                  switch the active profile, live (needs
                                  the control channel); RAM only unless
                                  --save
gamepadii setup udev [--print|--install]
                                  the udev rule the control channel needs
```

`--no-channel` (any command) leaves the control channel alone.

### The TUI

`gamepadii` with no command opens it when stdout is a terminal. One
screen shows the pad (what `status` says: mode and confidence, firmware,
board, the running profile, the control channel or its absence), the
input modes and the pad's profiles, the last result, and a status line
with what is running. Every action is a command too, and runs the same
code:

```text
j k  ↓ ↑  gg G       move in the list; h l ← → Tab: the other list
⏎ on a mode          switch to it (asks first: it saves the mode)
⏎ on a profile       switch to it, RAM only (needs the control channel)
S on a profile       switch to it and save it (asks first)
w                    into web-config, or back to the gamepad mode
b                    back up every setting
r                    read the pad again
?                    every key;  q quit (asks while something runs)
```

Reboots and waits run on a worker thread, so the screen keeps up; when a
flow needs a button press (stock firmware), the result panel says which.
It watches USB once a second and picks up a pad plugged in, out, or back
after a reboot.

### The control channel

The gamepadii build of GP2040-CE (a fork, on its way upstream; see
[docs/design/control-channel.md](docs/design/control-channel.md)) answers
vendor requests on the USB control endpoint in every mode, with no change
to what consoles see. When gamepadii finds it (a HELLO with a random nonce
that only that firmware echoes), it needs no web-config and no button
press:

- `status` is `confirmed`, and shows the firmware version, board, running
  input mode and active profile;
- `mode <MODE>` has the pad save the mode and restart into it, then checks
  the new VID:PID and asks the firmware again;
- `webconfig` and `reboot gamepad|webconfig` reboot it directly;
  `reboot bootsel` goes through web-config (BOOTSEL isn't offered on the
  channel);
- `profile switch` changes the active pin-mapping profile with no reboot.
  Without `--save` nothing is written to flash, so the pad goes back to its
  saved profile when it restarts (unless something else, such as a hotkey,
  saves the settings first). Either way the device cache (below) records
  which profile the pad runs, so the key hints follow;
- `info` and `backup` reboot it into web-config, read, then reboot it back
  (`/api/reboot`) and check it returns in the mode it was in, which takes
  a few seconds; the output says so ("via the control channel: web-config
  and back"). `--stay` leaves it in web-config. If anything fails on the
  way, gamepadii still sends the pad back and says whether that worked. The
  restart drops a RAM-only profile; gamepadii switches back to it (RAM
  only, nothing saved) and says so, or warns if that failed. With `--stay`
  it only notes that the profile was dropped.

The channel needs read-write access to the pad's device node
(`/dev/bus/usb/BBB/DDD`), which is root-only by default. `gamepadii setup
udev` explains the rule, and `--install` writes
`/etc/udev/rules.d/70-gamepadii.rules` (through pkexec) and reloads udev.
It is opt-in for a reason: GP2040-CE borrows most of its USB ids from real
controllers, so the rule also grants the logged-in user that access to a
real Xbox 360, HORI, DualShock or Switch pad. It touches no input device
(evdev, hidraw).

On stock firmware everything below works as before.

`info` and `backup` talk to the pad's web configurator, so without the
control channel the pad has to be in web-config mode (`reboot` too). In a
gamepad mode (XInput, Keyboard, ...) stock GP2040-CE has no control
channel, and they exit with code 5 and say how to switch: hold S2 (Start) while plugging the pad in, or hold S2+B3+B4 for 4
seconds. Those are logical buttons, so on a remapped profile they may be
other keys. `--host HOST` skips the USB check and talks to HOST instead of
192.168.7.1.

`mode <MODE>` does the whole switch:

- already in that mode: nothing to do;
- in web-config: it backs up, writes `inputMode` (the whole options object,
  read back and checked), reboots, waits for the pad on USB and checks its
  VID:PID;
- in a gamepad mode, with the control channel: as above, without
  web-config (no backup is taken: the pad saves only `inputMode`, as a
  boot button would);
- in a gamepad mode without it: it says what to press and waits (60 s,
  `--timeout SECS`; Ctrl-C is safe) for the pad to show up in web-config,
  then goes on as above. If the target mode has a boot button, that is the
  quickest route: unplug, hold the button, plug in. No write is needed, and
  the pad keeps the mode. `--no-wait` prints the instructions and exits 5.

`webconfig` waits the same way, then prints `http://192.168.7.1/`
(`--open` opens it).

Whenever the pad is in web-config, `info`, `backup`, `mode` and `webconfig`
save its options and pin-mapping profiles in
`~/.cache/gamepadii/<board>.json` (`$XDG_CACHE_HOME` is honoured). In a
gamepad mode, gamepadii can then name the boot buttons and say which keys
the hotkey is on in the active profile, by the names profile 1 (Default)
gives them, with Xbox labels:

```text
In profile 2 (SilkSong), S2+B3+B4 are the keys Default calls S2 (Start),
B3 (X) and R1 (RB).
```

The active profile is the saved one as read in web-config, unless the
control channel has said otherwise since: whenever it answers (`status`,
`profile switch`, the other commands), the cache records the profile the
pad runs. A RAM-only `profile switch` changes only that, `--save` the saved
one too, and a restart gamepadii makes puts it back on the saved one.

Web-config mode has no unique serial (every pad says `123456`), so the cache
goes by board config, and with several boards the newest one is used.

`backup` writes to `~/.local/share/gamepadii/backups/<time>-<board>.json`
(`$XDG_DATA_HOME` is honoured) unless given `-o FILE`. The bundle holds each
endpoint's JSON exactly as the pad sent it. The client only reads endpoints
on an allowlist verified safe on the S16: some GP2040-CE endpoints
(`getConfig`, `getI2CPeripheralMap`) hang the device on v0.7.12, and
`resetSettings` wipes it.

Exit codes are stable: 0 ok, 1 check failed (the pad came back in another
mode, or a write didn't read back), 2 no device found, 3 usage,
4 transport (or the pad didn't come back on USB), 5 needs a button press on
the pad (or the udev rule). With `--json` a failure
still prints a JSON object (`{"schema": 1, "error": {...}}`), and waybar
always gets a valid line.

### How the mode is detected

Without the control channel, `status` reads the USB vendor and product ids
from sysfs and looks them up in a table taken from the
firmware's descriptors. Most of those ids are borrowed from real controllers
(XInput is `045e:028e`, an Xbox 360 pad), so the result carries a
confidence: `confirmed` when the ids or strings can only be GP2040-CE
(`cafe:*`, a `GP2040-CE` product string), `likely` otherwise. With the
channel, the firmware says which mode it runs, so it is always `confirmed`
(and PS4 and PS5, which share an id, are told apart). Web-config
(`cafe:4028`) also reports its network interface.

## Supported devices

- **Haute42 S16** (leverless), tested. It runs stock GP2040-CE v0.7.12
  with the `Haute42COSMOX` board config.
- Other GP2040-CE controllers should work the same way: detection relies on
  the firmware, not the board.

## License

MIT, see [LICENSE](LICENSE).
