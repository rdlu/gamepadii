# gamepadii

A Linux CLI and TUI for gamepads that use the
[GP2040-CE](https://gp2040-ce.info/) firmware. A Tauri GUI will follow.
The first pad that it supports is the Haute42 S16.

gamepadii shows the status of the pad and changes its settings from the
computer. You do not have to open the web configurator, and you push fewer
buttons. With our firmware fork, most changes do not need a button press.

![gamepadii status, a profile switch, and the LEDs, on a simulated S16](docs/media/cli.gif)

![The TUI: the pad, its input modes and profiles, a profile switch, and the help](docs/media/tui.gif)

**Status: pre-release, in development.** These commands operate now:
`status`, `info`, `backup`, `restore`, `reboot`, `mode`, `webconfig`,
`profile switch`, `profile show`, `profile write`, `display layout`,
`leds`, `leds profile show`, `leds profile write`, `input`,
`input find-pin`, `log` and `setup udev`. The TUI
(`gamepadii` on a terminal) shows the pad, and changes the input mode and
the profile. The roadmap is in
[GitHub issues](https://github.com/rdlu/gamepadii/issues/1). The design
notes are in [docs/DESIGN.md](docs/DESIGN.md). The words that we use are
in [docs/GLOSSARY.md](docs/GLOSSARY.md).

The documentation will be at **<https://gamepadii.dlu.one/>**.

## Goals

- **See the pad:** the input mode, the firmware, the profiles and the LEDs,
  from the terminal or a status bar (waybar).
- **Change the pad without the web configurator:** the input mode, the
  profile, the OLED layout and the LEDs.
- **More profiles than the pad can keep:** a profile library on the
  computer (KDL files), with keyboard profiles for games (for example
  No Man's Sky) and for other apps (for example OBS).
- **A control channel in the firmware:** our
  [fork of GP2040-CE](https://github.com/rdlu/GP2040-CE) adds a small USB
  connection, so that gamepadii can change the profile, the input mode and
  the LEDs without a restart into web-config. The fork also has fixes for two
  web-config problems that stop the pad until you disconnect it. Stock
  firmware also operates with gamepadii, with button presses for some steps.

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
                                  profiles, boot-mode buttons and the
                                  OLED layout
gamepadii backup [-o FILE]        every readable setting in one JSON file
                                  (both from a gamepad mode too, with the
                                  control channel; --stay stays in
                                  web-config); with channel v1.6, the
                                  saved config as one blob too
gamepadii backup --channel-only   only the config blob, over the control
                                  channel (v1.6), with no web-config
gamepadii restore FILE [--no-backup]
                                  write the config blob of a backup back
                                  to the pad (channel v1.6), then check it
                                  after the restart
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
gamepadii profile show [N|LABEL]  the pin mappings of a profile, live
                                  (needs control channel v1.4); --json
                                  gives a slot file
gamepadii profile write FILE --slot N [--select] [--save] [--label TEXT]
                                  write a slot file to profile 2-6, live;
                                  RAM only unless --save; backs up first
gamepadii leds profile show [N]   the effects and colours of a lights
                                  profile, live (needs control channel
                                  v1.5); --json gives a lights file
gamepadii leds profile write FILE --profile N [--select] [--save]
                                  write a lights file to lights profile
                                  0-3, live; only its fields change; RAM
                                  only unless --save; backs up first
gamepadii input [--watch]         the buttons pressed now, live, with the
                                  GPIO, action and key of each (needs
                                  control channel v1.7); --watch prints
                                  each change
gamepadii input find-pin [--scan] push a button: its GPIO, action and
                                  key; --scan reads the pins with no
                                  action too
gamepadii display layout          the OLED's button drawing, and the
                                  choices for this board
gamepadii display layout <MODEL>  pick the drawing for a Haute42 model:
                                  s16, g16, s13, m16, ... (or default,
                                  alt0..alt5); backs up, writes, reads back
gamepadii display layout --left N --right N
                                  the raw buttonLayout values, any board
gamepadii leds                    the LEDs: animation, brightness, colours,
                                  theme (live over the control channel;
                                  else from web-config, partly unknown on
                                  stock firmware; on main firmware its
                                  lights profiles instead)
gamepadii leds set [--animation NAME|N] [--brightness N|up|down]
    [--static-color NAME|N] [--button-color NAME|N] [--theme NAME|N] [--save]
                                  change them live (needs control channel
                                  v1.1); RAM only unless --save
gamepadii leds set [--lights-profile N|off] [--brightness N|up|down] [--save]
                                  the same on main firmware (channel v1.2)
gamepadii leds brightness <up|down|N> [--save]
                                  one step, or a level
gamepadii log [--from N] [--follow]
                                  the pad's log of its last 64 events
                                  (boots, panics, commands), live (needs
                                  control channel v1.8); --follow shows
                                  new records until Ctrl-C
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

Our fork of GP2040-CE ([rdlu/GP2040-CE](https://github.com/rdlu/GP2040-CE);
the protocol is in
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
- `leds` reads the LEDs, and `leds set` / `leds brightness` change them,
  with no reboot. This needs channel v1.1, the LED commands, or v1.2 on
  main firmware, where `--lights-profile` selects a lights profile. Like
  `profile switch`, a change is RAM only unless `--save`. A v1.0 channel
  has no LED commands, so `leds` reads them through web-config instead;
- `profile switch` changes the active pin-mapping profile with no reboot.
  Without `--save` nothing is written to flash, so the pad goes back to its
  saved profile when it restarts (unless something else, such as a hotkey,
  saves the settings first). Either way the device cache (below) records
  which profile the pad runs, so the key hints follow;
- `profile show` reads the pin mappings of a profile, and `profile write`
  writes them to profile 2 to 6, with no reboot. This needs channel v1.4.
  The JSON of `profile show` is a slot file, and a slot file can leave out
  pins: they follow profile 1. For a per-game profile, write the game's
  file to a free profile with `--select`. Before a write, gamepadii backs
  up the profile in the same format, so a `profile write` of the backup
  undoes it. Like `profile switch`, a write is RAM only unless `--save`;
- `leds profile show` reads the effects and colours of a lights profile,
  and `leds profile write` changes them, with no reboot. This needs channel
  v1.5 on main firmware. The JSON of `leds profile show` is a lights file.
  A lights file can leave out fields, and the pad keeps those as they are,
  so `{"idle_effect": "rain"}` changes only the idle effect. The cycle
  times are 0 to 9, as the firmware keeps them (web-config shows 1 to 10).
  gamepadii backs up the profile first, and a write is RAM only unless
  `--save`;
- `input` reads the buttons with no reboot: the GPIOs pressed now, with
  the action of each in the active profile and the key printed on the
  case, and the buttons, D-pad, sticks and triggers that the pad sends.
  This needs channel v1.7. `input --watch` is an input tester.
  `input find-pin` waits until you push one button, and tells you its
  GPIO. With `--scan`, the pad also reads the pins with no action, so you
  can find a button that no profile uses. The scan changes no setting, and
  gamepadii stops it at the end, also after Ctrl-C;
- `log` reads the pad's log, with no reboot. This needs channel v1.8. The
  pad keeps its last 64 events: each boot, firmware panic, OUT command,
  profile change, save and restart. A soft reboot keeps the log; a
  power-on starts a new one. `--from N` starts at record N, and `--follow`
  shows the new records as the pad writes them. When the boot before this
  one ended with a firmware panic, `status` shows a warning with the
  message (`last_panic` in the JSON);
- `backup --channel-only` and `restore` move the saved config as one
  blob, with no web-config. This needs channel v1.6. `restore` writes all
  the settings to flash, and the pad restarts. Before it writes, gamepadii
  checks the file, the board and the free space, and backs up the config
  that the pad has. A failure before the end of the upload changes
  nothing. A file from a pad without v1.6 has no blob: restore it in
  web-config;
- `info`, `backup` and `display layout` reboot it into web-config, read
  (or write, for `display layout <MODEL>`), then reboot it back
  (`/api/reboot`) and check it returns in the mode it was in, which takes
  a few seconds; the output says so ("via the control channel: web-config
  and back"). `--stay` leaves it in web-config. If anything fails on the
  way, gamepadii still sends the pad back and says whether that worked. The
  restart drops a RAM-only profile; gamepadii switches back to it (RAM
  only, nothing saved) and says so, or warns if that failed. With `--stay`
  it only notes that the profile was dropped. `info` marks the profile the
  pad starts in as `saved` and the one STATUS reports once it is back as
  `running`; they differ after a RAM-only switch.

The channel needs read-write access to the pad's device node
(`/dev/bus/usb/BBB/DDD`), which is root-only by default. `gamepadii setup
udev` explains the rule, and `--install` writes
`/etc/udev/rules.d/70-gamepadii.rules` (through pkexec) and reloads udev.
gamepadii does not use the channel when it runs as root.
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
`~/.cache/gamepadii/<board>-<serial>.json` (`$XDG_CACHE_HOME` is
honoured). In a
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

With control channel 1.3, the pad gives its board ID: the unique ID of its
flash chip, in every mode. Then the cache is
`~/.cache/gamepadii/<board>-<board ID>.json`. Some flash chips give a fixed
ID, so two pads can give the same one. If they are connected at the same
time, `status` tells them apart by USB port and shows a warning.

Without the board ID, the cache goes by the USB serial. Web-config mode has
no unique serial (every pad says `123456`), but XInput
and Xbox One have one, made from the board's unique id. The cache goes by
that serial: when gamepadii takes the pad into web-config itself (or sees
it go there, during `mode` or `webconfig`), it knows which pad it is, so
two pads with the same board config keep separate caches, and each gets
its own hints. A pad put into web-config by hand is kept by board config
(`<board>.json`) until gamepadii sees it leave web-config as a pad with a
serial; in modes with no serial (Keyboard, HID, Switch, PS4, ...) the
newest cache that could be this pad is used, as before.

`backup` writes to `~/.local/share/gamepadii/backups/<time>-<board>.json`
(`$XDG_DATA_HOME` is honoured) unless given `-o FILE`. The bundle holds each
endpoint's JSON exactly as the pad sent it. The client only reads endpoints
on an allowlist verified safe on the S16: some GP2040-CE endpoints
(`getConfig`, `getI2CPeripheralMap`) hang the device on v0.7.12, and
`resetSettings` wipes it. The allowlist is per firmware line: v0.7.12 and
upstream main (unreleased past v0.7.12, which also says
`v0.7.12-<n>-g<hash>`) have different endpoints and setters, so gamepadii
works out which line the pad runs (from a known build, else from what
`getGamepadOptions` returns) and `info` and `backup` say so. Main support is
built from main's source and not yet tried on a pad.

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
- Firmware: GP2040-CE v0.7.12 (tested) and upstream main as of 8013b41
  (from its source; `info`, `backup`, `mode`, `display layout` and `leds`
  read and write main's shapes, untested on hardware).

## License

MIT, see [LICENSE](LICENSE).
