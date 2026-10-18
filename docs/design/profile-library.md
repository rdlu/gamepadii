# The profile library

Status: first step of issue #5 (2026-10-18). The parser, the checks, the
board file of the S16, and `library list`, `library show` and
`library pull` operate. `apply` and `diff` come next.

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
    button fn2 gpio=20 label="Home"
    // ...
}
```

- `firmware-board` is `GP2040_BOARDCONFIG` of the firmware. `library pull`
  finds the board file with it.
- `button NAME gpio=N`, with an optional `label` (the text on the case) and
  `size`. A name or a GPIO can be in the board one time only. `default` is
  not a button name.

The S16 has 21 buttons. The names follow §1.3 of
[the research](../research/keyboard-profiles.md): `left` 5, `down` 3,
`right` 4, `m1` 27, `up` 2 (the big thumb button), `m2` 26, `rs` 19, `ls`
18, `p1`–`p4` 10–13, `k1`–`k4` 6–9, `fn1` 21, `fn2` 20, `fn3` 16, `fn4` 17,
`turbo` 14.

A test checks the board file against profile 1 of the S16 fixture
(`tests/fixtures/api/s16-v0.7.12`): each button has a button press there,
`turbo` has `MENU_NAVIGATION_TOGGLE`, and each GPIO with an action has a
button. The fixture and the draft of §6.2 agree on every GPIO. Two buttons
press Up in profile 1 (GPIO 2 and 27), and two press L3 (GPIO 18 and 26).
The labels of `fn1` to `fn4` and the place of `turbo` are not checked on the
pad yet (open question 4 of the research).

## Profiles

```kdl
profile "My Game" {
    base "Default"           // optional
    board "haute42-s16"      // optional when a base gives it
    label "My Game"          // optional: the name when absent
    input-mode xinput        // optional
    pins {
        default none         // optional
        up B1
        m2 L1
        k4 custom-button-combo button-mask=0x3 dpad-mask=0x0
    }
}
```

- `pins` gives an action to a button: `BUTTON ACTION`. The action is a
  logical button (`B1`, `Up`, `E12`, `Fn`), the short name of another
  `GpioAction` (`none`, `turbo`, `macro-1`, `menu-navigation-toggle`), the
  enum name (`BUTTON_PRESS_B1`), or its number. These are the names of a
  slot file (`slots::action_from_name`).
- `button-mask` and `dpad-mask` are the masks of `custom-button-combo`.
- `default ACTION` gives an action to each button of the board that the
  profile and its bases do not name. Without it, those buttons follow
  profile 1 of the pad.
- `label` is the text that the pad shows, at most 16 bytes. When there is
  no label, the name is the label.
- `input-mode` is the input mode that the profile is for. It is a
  requirement: the pad has one input mode for all profiles. In this step
  gamepadii shows it, and does not apply it.

## Bases

A profile takes what it leaves out from its base, and the base from its own
base. In the chain, a later profile changes an earlier one:

- `board`, `input-mode` and `default` come from the base when the profile
  does not give them.
- Each button gets the action of the last profile in the chain that names
  it.
- `label` does not come from the base: a profile without a label has its
  own name as the label.

## Checks

Each error gives the file, the line and the column (from 1, in
characters): `profiles/nms.kdl:12:9: unknown button "thumb": ...`. The
parser checks:

- the KDL 2.0 syntax (the message of the `kdl` crate);
- unknown nodes and properties, and a value of the wrong type;
- an unknown action, and an unknown input mode;
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
- a button that is not on the board;
- a label longer than 16 bytes.

Nodes of the §6 draft that gamepadii does not read yet (`keymap`,
`hotkeys`, `loadout`, `host`, `theme`) are errors, not ignored, so that a
file does not seem to do something that it does not do.

## Commands

The pad has `gamepadii profile switch`, `profile show [N|LABEL]` and
`profile write FILE --slot N`. These operate on the slots of the pad. The
library commands operate on files on the computer, so they have their own
name, `library`. Thus `show` has one meaning in each place, and `profile
show celeste` (slot 3 of the pad) is not the same as `library show
celeste` (a file).

- `library list`: each profile with its name, base, label, file and line,
  and the errors. It exits 0 when files have errors; the errors are in the
  output (`"errors"` in JSON).
- `library show NAME`: the profile with its bases applied. For each
  button: its GPIO, its action, and the profile that gives the action.
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
  file of the library has.

A pull and then `library show NAME --slot-file` give the same mapping as
the slot: a test writes each pulled profile as a slot file, plans it with
`slots::plan`, and compares it with the slot.

## Not in this step

From the §6 draft:

- `keymap`, `requires` (gamepad options), `hotkeys`, `loadout` and `host`.
  The pad can not take them live yet: the keymap and the gamepad options
  need web-config. Their form depends on the open questions of #5 (the
  keymap, the hotkeys, the slots).
- The theme node of the #5 update (custom colours and a lights profile).
- The `bind` form of §6.4, where gamepadii gives the keys to slots.
- The profiles "NMS Foot", "NMS Flight" and "OBS Pad". Their pin mappings
  use the E1–E12 slots of a keymap that the user has not agreed yet, and
  the open questions 3 to 7 change them.

## Ready for `apply` and `diff`

- `Resolved::slot_file` gives the slot file of a profile, so `apply` can
  call `flow::profile_write` with it (WRITE_SLOT, a backup first).
- `diff` can compare the slot file with `flow::profile_show` of a slot:
  the GPIO and action of each button.
- `Resolved::input_mode` tells `apply` which input mode the profile needs.
