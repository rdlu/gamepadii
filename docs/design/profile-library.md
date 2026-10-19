# The profile library

Status: issue #5 (2026-10-18). The parser, the checks, the board file of
the S16, the name systems, the themes, and `library list`, `library
show`, `library pull`, `library diff`, `library apply` and `library
theme` operate. `apply` writes the pin mapping and the label over the
control channel, then the theme of the profile. The keymap and the
gamepad options come later.

## Intent

The user wants more profiles than the 6 slots of the pad, and wants to keep
them in readable files. gamepadii keeps the profiles on the computer, in a
library of KDL 2.0 files. The library is the source of truth. The pad is
only a target: gamepadii writes one profile into one slot when the user
needs it.

## Files

- The library is `$XDG_CONFIG_HOME/gamepadii/profiles/`
  (`~/.config/gamepadii/profiles/`). gamepadii reads each `.kdl` file in it,
  in name order. A file can have one profile or more.
- gamepadii ships example profiles (`crates/gamepadii-core/data/examples/`).
  They are in the library too, after the files of the user. A profile of
  the user hides a shipped profile with the same name.
- Profile names are unique in the library. Case does not matter. A second
  profile with the same name is an error, at its own file and line.
- The parser is the `kdl` crate, version 6.5.0. It reads KDL 2.0 only.
  Versions 6.6 and later need Rust 1.95, and the MSRV of gamepadii is 1.88.

## Board files

A board file gives the name of each physical button and its GPIO. gamepadii
ships the board files (`crates/gamepadii-core/data/boards/`), so a profile
names buttons, never GPIOs. The first one is `haute42-s16`:

```kdl
board "haute42-s16" {
    firmware-board "Haute42COSMOX"
    button left gpio=5
    button m1 gpio=27
    button up gpio=2 size="26.6mm"
    button share gpio=16 label="Share / Back" alias="back"
    // ...
}
```

- `firmware-board` is `GP2040_BOARDCONFIG` of the firmware. `library pull`
  finds the board file with it.
- `button NAME gpio=N`, with an optional `label` (the text on the case),
  `alias` (one or more) and `size`. A name or a GPIO can be in the board
  one time only. `default` is not a button name.
- An alias is a second name for the button, from a label on the case. A
  profile and a theme can use the name or an alias. An alias must not be
  the name or the alias of another button of the board: the board-file
  check refuses that. A profile that names one button two times, by its
  name and by an alias, is an error. gamepadii writes the name, never the
  alias (`library pull`, `library show`, the JSON).

The S16 has 21 buttons. The names follow §1.3 of
[the research](../research/keyboard-profiles.md): `left` 5, `down` 3,
`right` 4, `m1` 27, `up` 2 (the big thumb button), `m2` 26, `rs` 19, `ls`
18, `p1`–`p4` 10–13, `k1`–`k4` 6–9, `turbo` 14, and the 4 small buttons.

The 4 small buttons are top right. From left to right they are GPIO 21,
20, 16 and 17. The case prints Touchpad, Home, Share and Options on them
(PlayStation), and −, Home, Back and Start under them (Xbox). The names
are the top labels, and the labels under them are aliases:

| Button | GPIO | Alias | Profile 1 |
|---|---|---|---|
| `touchpad` | 21 | `minus` | A2 |
| `home` | 20 | – | A1 |
| `share` | 16 | `back` | S1 |
| `options` | 17 | `start` | S2 |

The turbo button (GPIO 14) is top left. No button has the label FN: `Fn` is
an action, not a button. Before release, the 4 small buttons had the names
`fn1` to `fn4`. Those names gave the wrong idea of the `Fn` action, so
they changed. An old name gives an error that names the new name
(`button "fn3" is now "share" on haute42-s16`).

A test checks the board file against profile 1 of the S16 fixture
(`tests/fixtures/api/s16-v0.7.12`): each button has a button press there,
`turbo` has `MENU_NAVIGATION_TOGGLE`, and each GPIO with an action has a
button. The fixture and the draft of §6.2 agree on every GPIO. Two buttons
press Up in profile 1 (GPIO 2 and 27), and two press L3 (GPIO 18 and 26).

## Profiles

```kdl
profile "My Game" {
    base "Default"           // optional
    board "haute42-s16"      // optional when a base gives it
    label "My Game"          // optional: the name when absent
    input-mode xinput        // optional
    names xbox               // optional: the name system of the pins
    theme "purple-green"     // optional: the LED theme
    pins {
        default none         // optional
        up A                 // B1: A in Xbox names
        m2 LB
        back Guide           // back is an alias of share
        k4 custom-button-combo button-mask=0x3 dpad-mask=0x0
    }
}
```

- `pins` gives an action to a button: `BUTTON ACTION`. The action is a
  logical button in the name system of the profile (`B1`, or `A` in Xbox
  names), a direction (`Up`), `Fn`, the short name of another
  `GpioAction` (`none`, `turbo`, `macro-1`, `menu-navigation-toggle`), the
  enum name (`BUTTON_PRESS_B1`), or its number. Without a name system,
  these are the names of a slot file (`slots::action_from_name`).
- `theme` names a theme of the library (see [Themes](#themes)).
- `button-mask` and `dpad-mask` are the masks of `custom-button-combo`.
- `default ACTION` gives an action to each button of the board that the
  profile and its bases do not name. Without it, those buttons follow
  profile 1 of the pad.
- `label` is the text that the pad shows, at most 16 bytes. When there is
  no label, the name is the label.
- `input-mode` is the input mode that the profile is for. It is a
  requirement: the pad has one input mode for all profiles. gamepadii
  shows it, and `library apply` gives a note when the pad runs another
  input mode. It does not change the input mode.

## Bases

A profile takes what it leaves out from its base, and the base from its own
base. In the chain, a later profile changes an earlier one:

- `board`, `input-mode`, `theme` and `default` come from the base when the
  profile does not give them.
- `names` does not come from the base: each profile is read in its own
  name system. A base in Xbox names and a child in Switch names are
  correct.
- Each button gets the action of the last profile in the chain that names
  it.
- `label` does not come from the base: a profile without a label has its
  own name as the label.

## Name systems

The user that writes KDL by hand can name the actions in the names of
their console. The same word has two meanings in two systems: `A` is B1
for Xbox and B2 for Switch. Thus a file or a profile gives its system one
time, with a `names` node:

- `names SYSTEM` at the top of the file applies to each profile of the
  file. `names SYSTEM` in a profile replaces it for that profile. The
  place of the node in the file or in the profile does not matter.
- The systems are `gp2040` (the default), `xbox`, `playstation` (also
  `ps`) and `switch`. The names come from the table in `buttons.rs`,
  which follows `www/src/Data/Buttons.js` of GP2040-CE:

| GP2040 | Xbox | PlayStation | Switch |
|---|---|---|---|
| B1 | A | Cross | B |
| B2 | B | Circle | A |
| B3 | X | Square | Y |
| B4 | Y | Triangle | X |
| L1 | LB | L1 | L |
| R1 | RB | R1 | R |
| L2 | LT | L2 | ZL |
| R2 | RT | R2 | ZR |
| S1 | Back, View | Share, Create | Minus |
| S2 | Start, Menu | Options | Plus |
| A1 | Guide | PS | Home |
| A2 | – | Touchpad | Capture |
| L3 | LS | L3 | LS |
| R3 | RS | R3 | RS |

- Xbox has no name for A2 (XInput has no such button). Write `A2`.
  Buttons.js calls the PlayStation A2 "Touchpad Center"; gamepadii calls
  it `Touchpad`. View, Menu and Create are the names of newer consoles,
  and are also valid.
- In each system, the GP2040 names (`B1`, `S2`, `A2`), the directions
  (`Up`), `Fn`, `turbo`, `none`, the other short names, the enum names and
  the numbers are also valid. No name of a system is the GP2040 name of
  another action (a test checks it), so this does not make a clash. `L1`
  is L1 in PlayStation and in GP2040.
- Case does not matter, and `-` and `_` do not count.
- An unknown action gives an error that lists the names of the active
  system.
- `library pull --names SYSTEM` writes the actions in that system, with
  `names SYSTEM` at the top. `library show` shows the actions in the
  system of the profile. The JSON keeps the GP2040 names in
  `action_name`, and adds `names`. The slot file is the same in each
  system: a test resolves the same pull in all 4 systems and compares the
  slot files.

## Themes

A theme gives the colours and the effects of one lights profile of the
pad, by the names of the buttons. gamepadii makes a theme file from it,
the JSON of `gamepadii leds theme apply` (`colors::ThemeFile`).

```kdl
theme "snes" {
    board "haute42-s16"                        // necessary for button nodes
    effect idle="static-color" pressed="static-color"
    idle "#5a5a8c"                             // all buttons
    pressed "white"
    button k1 idle="#e0c020"                   // one button: name or alias
    button k2 idle="#d03030" pressed="red"
    case "purple"                              // all case lights
    lights press_fade_ms=200 idle_cycle_time=3 // other lights-file fields
}
```

- **A colour** is a built-in colour (`black`, `white`, `red`, `orange`,
  `yellow`, `limegreen`, `green`, `seafoam`, `aqua`, `skyblue`, `blue`,
  `purple`, `pink`, `magenta`), `custom N` (0 to 15, a custom colour of
  the pad), or `"#rrggbb"` (in quotes: `#` starts a keyword in KDL 2.0).
- **Custom colours.** gamepadii puts each different `#rrggbb` into a
  custom colour, in the order of first use in the file: the first is
  custom colour 0 (colour number 14), the next is custom colour 1, and so
  on. The same value two times uses one custom colour. A built-in colour
  stays built in. The pad keeps 16 custom colours, so a 17th `#rrggbb`
  is an error at its line. The theme file then sets those custom colours,
  and the colours after them become black.
- **The custom colours are global**: the 4 lights profiles of the pad
  share them. A theme with `#rrggbb` changes the colours of the other
  lights profiles that use colour numbers 14 to 29. `library show` and
  the apply give a note.
- `custom N` uses a custom colour that the pad keeps. A theme with
  `custom N` and no `#rrggbb` does not change the custom colours. A theme
  with both is an error: the `#rrggbb` values replace the custom colours,
  so `custom N` would show another colour.
- **One colour for each GPIO.** The colour lists of a lights profile
  (`notPressedStaticColors`, `pressedStaticColors`) have one byte for each
  GPIO: the firmware reads the colour of a button LED at the index of its
  GPIO (`AnimationStation::StaticGetNonPressedColorForLight`). A `button`
  node gives the colours of one button, by its board name or alias, at its
  GPIO. Each other GPIO gets `idle` and `pressed`, black when absent. A
  GPIO with no button in the board file also gets `idle` and `pressed`:
  thus a theme with no board, or a board file that does not name each LED,
  still lights each LED. The lists have 30 entries (`NUM_BANK0_GPIOS` of
  the RP2040), or more for a board with a higher GPIO.
- `case COLOUR` sets all 32 case colours (`nonButtonStaticColors`; the
  firmware reads them by the index of the case light). The S16 has no case
  lights.
- `effect idle= pressed= case=` gives the 3 effects, by name or number.
  `lights KEY=VALUE ...` gives the other fields of a lights file, by their
  keys (`press_fade_ms`, `idle_cycle_time`, `idle_special_color`, ...),
  with the values of a lights file. Each field has one place: `lights`
  refuses the colour lists and the effects. gamepadii checks each value as
  `leds profile write` does, at its line.
- `board` is optional. A theme with `button` nodes needs a board: its own,
  or the board of the profile that names it. When both give one, they
  must be the same.
- A theme that sets nothing is an error.
- Theme names and profile names are unique together: `library show NAME`
  shows a profile or a theme. A user theme hides a shipped theme with the
  same name.

The theme file of a theme has `board` (the firmware board), `colors` (the
custom colours, or `null`), and `lights` (the lights file with only the
fields that the theme sets). `library show NAME --theme-file` prints it,
so `gamepadii library show snes --theme-file | gamepadii leds theme apply
-` is the same as `gamepadii library theme snes`. A test parses the output
with `ThemeFile::parse`.

## Checks

Each error gives the file, the line and the column (from 1, in
characters): `profiles/nms.kdl:12:9: unknown button "thumb": ...`. The
parser checks:

- the KDL 2.0 syntax (the message of the `kdl` crate);
- unknown nodes and properties, and a value of the wrong type;
- an unknown action (in the name system of the profile), an unknown name
  system, and an unknown input mode;
- in a theme: an unknown colour, more than 16 `#rrggbb`, `custom N` with
  `#rrggbb`, a lights field with a bad value;
- a button that is twice in one `pins`;
- `input-mode` twice in one profile ("input-mode conflict" when the two
  values are different), and any other node twice.

Then `library show` and `library list` apply the bases and check:

- a base that is not in the library, and a loop of bases;
- a board that gamepadii does not ship, or no board in the chain;
- a board that is different from the board of a base ("board conflict");
- an input mode that is different from the input mode of a base
  ("input-mode conflict"): a profile can add an input mode, but not change
  it;
- a button that is not on the board, or an old button name (it gives the
  new name), and one button two times by its name and an alias;
- a label longer than 16 bytes;
- a theme that is not in the library, and each check of the theme on the
  board of the profile.

Nodes of the §6 draft that gamepadii does not read yet (`keymap`,
`hotkeys`, `loadout`, `host`) are errors, not ignored, so that a file does
not seem to do something that it does not do.

## Commands

The pad has `gamepadii profile switch`, `profile show [N|LABEL]` and
`profile write FILE --slot N`. These operate on the slots of the pad. The
library commands operate on files on the computer, so they have their own
name, `library`. Thus `show` has one meaning in each place, and `profile
show celeste` (slot 3 of the pad) is not the same as `library show
celeste` (a file).

- `library list`: each profile with its name, base, label, file and line,
  each theme with its board, file and line, and the errors. It exits 0
  when files have errors; the errors are in the output (`"errors"` in
  JSON).
- `library show NAME`: the profile with its bases applied. For each
  button: its GPIO, its action (in the name system of the profile), and
  the profile that gives the action. For a theme: its custom colours,
  `idle`, `pressed`, the buttons with their GPIOs and colours, `case`, and
  the other fields. `--theme-file` prints the theme file of a theme, or of
  the theme of a profile.
  `--slot-file` prints the slot file for `profile write`: the board config
  (`Haute42COSMOX`), the label, and the GPIO and action of each button that
  has an action, in GPIO order. The pins that are not in the file follow
  profile 1.
- `library pull`: reads the enabled slots with READ_SLOT (control channel
  1.4) and gives KDL. It reads only. Profile 1 becomes a profile with each
  button of the board. Each other slot becomes a profile with profile 1 as
  its base, and only the buttons that it changes. A pin that follows
  profile 1 (0 or -5) is not written. A pin with an action that is not on
  a button of the board is not pulled; a comment and `"skipped"` in JSON
  tell the user. A slot with no label gets the name `Profile N`; two slots
  with the same label get `Label (N)`. `--write [FILE]` writes the file
  into the library (a bare file name) or to the path, with the board name
  as the default name (`haute42-s16.kdl`). It does not replace a file
  without `--force`, and it does not write a profile name that another
  file of the library has. `--names SYSTEM` writes the actions in a name
  system.
- `library theme NAME [--profile N] [--save] [--no-backup]`: applies a
  theme with `flow::theme_apply`, as `leds theme apply` does: the custom
  colours (WRITE_CUSTOM_COLORS), a wait until LIGHTS_STATUS PENDING is
  clear, then the lights profile (WRITE_LIGHTS) with ENABLED and ACTIVE.
  The lights profile is the one that runs, or `--profile N`. RAM only
  unless `--save`. gamepadii backs up the colours and the profile first.

A pull and then `library show NAME --slot-file` give the same mapping as
the slot: a test writes each pulled profile as a slot file, plans it with
`slots::plan`, and compares it with the slot.

## Apply and diff

```sh
gamepadii library diff NAME [--slot N]
gamepadii library apply NAME [--slot N] [--select] [--save] [--no-backup]
                        [--no-lights]
```

The intent: the user starts a game and wants its profile on the pad, with
no reboot and no flash write. `apply` writes the profile into one slot in
RAM and selects it. `diff` shows what `apply` writes, before it writes.

`apply` makes the slot file of the profile (`Resolved::slot_file`) and
gives it to `profile write` (`flow::profile_write`, through the same
function). Thus the checks, the backup, the WRITE_SLOT, the read-back and
the verify are the same as for `profile write`. The flags are the flags of
`profile write`, with the same meaning: `--select`, `--save` and
`--no-backup`. There is no `--label`: the label is in the profile.

- **RAM only by default.** `--save` also writes the flash. Do not use it
  at each game start: each save writes the flash. RAM is sufficient until
  the pad restarts.
- **The slot.** `--slot N` gives the slot, 2 to 6. With no `--slot`, the
  slot is the enabled slot with the label of the profile (case does not
  matter). Thus `library apply silksong` after a `library pull` goes back
  to the slot that it came from. When no slot has the label, gamepadii
  stops and asks for `--slot`: it does not select a slot itself.
- **Never profile 1.** Profile 1 is the base of the other slots: the pins
  that a slot file leaves out follow it. WRITE_SLOT refuses profile 1, and
  `slots::plan` refuses it before. gamepadii also refuses it earlier:
  `--slot` takes only 2 to 6, and when the label of the profile is the
  label of profile 1 ("Default" after a pull), gamepadii stops and asks
  for `--slot`. `diff` has the same rule, so `diff` never says that
  `apply` can write a slot that `apply` refuses.
- **The board.** The firmware board of the board file
  (`Haute42COSMOX`) must be the board config of the pad. gamepadii refuses
  a profile for another board, and a pad that does not give its board
  config.
- **The input mode.** The pad has one input mode for all profiles, and
  SET_INPUT_MODE writes the flash and restarts the pad. `apply` does not
  change it. When the profile has an input mode and the pad runs another
  one, a note gives the command: `gamepadii mode xinput`.
- **Refusals first.** Each refusal comes before the backup and before the
  first write: an unknown profile, a profile with errors, another board,
  profile 1, no slot with the label. With no control channel 1.4 (stock
  firmware, or an older build), `apply` and `diff` exit 5 with the message
  of `profile write`.
- **The theme.** When the profile has a theme, `apply` writes the pins
  first, then the theme, as `library theme` does, to the lights profile
  that runs. `--save` saves both. `--no-lights` writes the pins only. The
  checks of the theme also come before the first write: no lights
  profiles on the channel (v1.5), `#rrggbb` colours with no custom
  colours (v1.9), a web-config preview, the lights off, another board.
  `apply` writes the theme also when the slot has the profile already.
  The JSON has `theme` (the name) and `lights` (the report of the theme
  apply), `null` without a theme. The choice: one command for "this game
  now" is the use of `apply` at each game start; `library theme` is for a
  theme alone. `diff` does not compare the LEDs.

### The diff

`diff` reads profile 1 and the slot (READ_SLOT), and plans the WRITE_SLOT
of the profile (`slots::plan`). It compares the slot now with what the pad
keeps after the write (`slots::expected`: the pins that profile 1 reserves
or gives to an add-on keep the action of profile 1). It shows:

- the label and the state (enabled), now and after;
- each pin that changes, with the button of the board file, and its action
  now and after. A pin that follows profile 1 shows "same as profile 1"
  and the action of profile 1.

A pin can change its stored value but not its action. For example, a slot
that `library pull` read gives `none` to the GPIOs that have no button,
and the slot file leaves them out, so they get 0 (follow profile 1). When
profile 1 has `none` there too, the button does the same. Such a pin is
`same_action`. `diff` shows these pins on one line, and only the other
changes count:

- `changed` (in JSON) is `true` when the label, the state or the action
  of a pin changes. Then `apply` writes, and it writes all the pins of the
  slot file, also the `same_action` pins.
- With no other change, `apply` writes nothing (as `restore` does with a
  config that the pad has already). `--select` of a slot that is not the
  active one still writes (WRITE_SLOT selects it). `--save` always writes:
  RAM and flash can hold different slots, and READ_SLOT gives only RAM.

Exit codes: `diff` exits 0 with no change and 1 when `apply` writes, as
`diff(1)` does. 1 is "check failed" in the exit codes of gamepadii, and a
script can use `library diff NAME || library apply NAME`. The other codes
are the same as for the other commands (3 usage, 5 no channel). The JSON
of `diff` is the report, not an error object, also with exit 1. The pins
are a list, `pins`, in GPIO order: each with `gpio`, `button` (`null` for
a GPIO with no button), `now`, `after` and `same_action`.

## Not in this step

From the §6 draft:

- `keymap`, `requires` (gamepad options), `hotkeys`, `loadout` and `host`.
  The pad can not take them live yet: the keymap and the gamepad options
  need web-config. Their form depends on the open questions of #5 (the
  keymap, the hotkeys, the slots).
- The `bind` form of §6.4, where gamepadii gives the keys to slots.
- The profiles "NMS Foot", "NMS Flight" and "OBS Pad". Their pin mappings
  use the E1–E12 slots of a keymap that the user has not agreed yet, and
  the open questions 3 to 7 change them.
