# Keyboard-mode profiles for the Haute42 S16: No Man's Sky and OBS sidekick

Status: research and design draft (2026-10-17). Nothing here has been written to the device.

Target: Haute42 S16, GP2040-CE v0.7.12, board config `Haute42COSMOX`.
Firmware sources were read at tag `v0.7.12` in `/home/rdlu/Projects/GP2040-CE`.
Device state comes from `tests/fixtures/api/s16-v0.7.12/`.

---

## 1. S16 physical layout

### 1.1 Sources

- `configs/Haute42COSMOX/BoardConfig.h@v0.7.12`. `DEFAULT_BOARD_LAYOUT_A_ALT2` / `B_ALT2` is commented
  "Stickless 16 1 — G16, S16" and gives each GPIO's x/y on the panel.
- `configs/Haute42COSMOX/assets/Haute42_G16.png`, the G16 panel drawing. It matches ALT2 and labels
  M1 = PIN27 and M2 = PIN26.
- Reviews (The Arcade Stick, juanminidiy) say the following:
  - 16 main buttons: 23.3 mm buttons plus one larger 26.6 mm thumb "jump" button.
  - The extra upper-left button acts as the **W of a WASD layout**.
  - Small function buttons labelled **Home, Options/Start, Share/Back, Touchpad, Turbo**.
  - An OLED screen and two USB-C ports.
- `getPinMappings.json` (profile 1 "Default") confirms the GPIO → action map.
  The user changed **GP14** from the board default (Turbo) to `MENU_NAVIGATION_TOGGLE` (78).

### 1.2 Grid

The grid shows the left hand on the left cluster, as on any leverless pad.

```
                 [M1 GP27]          [LS GP18]                 (fn1 GP21)(fn2 GP20)(fn3 GP16)(fn4 GP17)
 [◄ GP5] [▼ GP3]               [X GP10] [Y GP11] [RB GP12] [LB GP13]
                  [► GP4]      [A GP6 ] [B GP7 ] [RT GP8 ] [LT GP9 ]
                                       [RS GP19]
          [M2 GP26]   [ ▲ GP2 (big thumb) ]                           (+ Turbo GP14, position unverified)
```

In the drawing:
- The X/A column (GP10/GP6) sits slightly lower, just right of `►`. The left index finger reaches it,
  like R/F on a keyboard.
- RS (GP19) sits under B, to the right of the big thumb button.

### 1.3 Buttons, Default mapping and Xbox labels

| Physical name (proposed) | GPIO | Default logical | Xbox label | Notes |
|---|---|---|---|---|
| `left`  | 5  | Left  | D-Left  | left ring finger |
| `down`  | 3  | Down  | D-Down  | left middle finger |
| `right` | 4  | Right | D-Right | left index finger |
| `m1`    | 27 | Up    | D-Up    | above, between ▼ and ►; the "W" key |
| `up`    | 2  | Up    | D-Up    | **big thumb button** (26.6 mm) |
| `m2`    | 26 | L3    | LS click | thumb, left of `up` |
| `rs`    | 19 | R3    | RS click | thumb, right of `up` |
| `ls`    | 18 | L3    | LS click | top, above X/Y |
| `p1`    | 10 | B3    | X  | top row, column 1 |
| `p2`    | 11 | B4    | Y  | top row, column 2 |
| `p3`    | 12 | R1    | RB | top row, column 3 |
| `p4`    | 13 | L1    | LB | top row, column 4 |
| `k1`    | 6  | B1    | A  | bottom row, column 1 |
| `k2`    | 7  | B2    | B  | bottom row, column 2 |
| `k3`    | 8  | R2    | RT | bottom row, column 3 |
| `k4`    | 9  | L2    | LT | bottom row, column 4 |
| `fn1` (Touchpad?) | 21 | A2 | – (PS Touchpad / Switch Capture) | small; the label is a guess from the layout order |
| `fn2` (Home)      | 20 | A1 | Guide | small |
| `fn3` (Share/Back)| 16 | S1 | Back / View | small |
| `fn4` (Options/Start) | 17 | S2 | Start / Menu | small |
| `turbo` | 14 | MENU_NAVIGATION_TOGGLE (user) | – | board default is Turbo; not in the display layout |

The other GPIOs are not buttons:
- 0 and 1: I2C for the OLED.
- 23 and 24: USB peripheral.
- 28: LEDs.
- 15, 22, 25 and 29: NONE.

There are 16 main buttons, 4 small function buttons and Turbo, so **21 inputs**.

### 1.4 How the user customises (profiles 2–5)

| Profile | Changes vs Default |
|---|---|
| SilkSong | `up` (GP2) → B1. The bottom row and part of the top row are reshuffled: k1→L2, k2→L1, k3→R1, k4→S1, p2→R2, p3→B4, p4→B2. `ls`→R1 |
| Celeste  | `up` → B1 (only change) |
| DKC      | `up` → B1. k3→R1, k4→L1, p3→L1, p4→R1, `ls`→S1, `rs`→R1, `m2`→L1 |
| TGX      | `up` → B4. `ls`→B2, `rs`→R1, `m2`→L1 |

Patterns:
- The **big thumb button always becomes the primary action** (jump).
- `m1` stays as the only Up.
- The thumb-row extras (`m2`, `rs`) and `ls` become shoulder buttons.
- Duplicate assignments are normal.

The keyboard layouts below follow the same habits.

---

## 2. GP2040-CE v0.7.12 keyboard mode: verified facts

### 2.1 Keymap

There is **one keymap per device** (`KeyboardMapping`), not one per profile. It has 32 slots:

- Up, Down, Left, Right
- B1–B4, L1, R1, L2, R2
- S1, S2, L3, R3
- A1–A4
- E1–E12

### 2.2 How the USB report is built

The report is an **NKRO bitmap**: 256 bits, usages 0x00–0xFF on the Keyboard page, report ID 1.
`pressKey()` sets `keycode[code/8] |= 1<<(code%8)`.

What that allows:

- **Any key from 0x00 to 0xE7 works.** That includes F13–F24 (0x68–0x73), the keypad, and the
  International and LANG keys.
- **Modifiers** (0xE0–0xE7: LCtrl, LShift, LAlt, LGUI and the right-hand versions) are sent as
  ordinary bits in the bitmap. Linux maps them to KEY_LEFTCTRL and so on, so they work as held keys.
  `getModifier()` exists but is never called.
- The web UI key list (`www/src/Data/Keyboard.js`) offers F13–F24.

### 2.3 Media keys

Codes above 0xE7 go to a separate Consumer report (ID 2). Only 7 are supported:

| Code | Key |
|---|---|
| 0xE8 | Next track |
| 0xE9 | Previous track |
| 0xF0 | Stop |
| 0xF1 | Play/Pause |
| 0xF2 | Mute |
| 0xF3 | Volume up |
| 0xF4 | Volume down |

Limitations:

- **Only one media key at a time.** The value is assigned, not OR-ed.
- **The last key processed decides which report is sent.** If a media key slot comes after a held
  regular key in slot order, only the media report goes out that frame, and changes to regular keys
  wait until the media key is released.
- Keep media keys out of game profiles.

### 2.4 No chords from one slot

One slot sends exactly one key. Two ways around that:

- **Two physical buttons pressed together.** Hold a pad button that sends Ctrl, then press another.
- **`CUSTOM_BUTTON_COMBO` (GpioAction 40).** One GPIO presses several logical buttons. In v0.7.12
  this works **only for Up/Down/Left/Right, B1–B4, L1, R1, L2, R2, S1, S2, L3, R3, A1 and A2**,
  not A3, A4 or E1–E12 (see `assignCustomMappingToMaps` in `gamepad.cpp`).

  It is also unreliable for modifier chords. Both keys change in the same bitmap report, and the
  kernel's hid-input walks the bitmap in usage order. So F13 (0x68) gets its press event *before*
  LCtrl (0xE0) within one SYN frame, and the compositor probably sees a bare F13. This is my reading
  of hid-input and is untested; check with `wev`.

**Do not plan on single-button chords.**

### 2.5 Logical-button pipeline in keyboard mode

`read()` → `hotkey()` → `process()` (SOCD and d-pad mode) → add-ons → `KeyboardDriver::process()`.
Consequences:

- **D-pad slots go through SOCD, 4-way mode and d-pad mode.**
  - If `dpadMode` is LS or RS, `state.dpad` is cleared and **the Up/Down/Left/Right keys stop being
    sent**.
  - 4-way mode removes diagonals (W+A).
  - The current settings are fine: `dpadMode`=0 (digital), `fourWayMode`=0, `socdMode`=2.
    `socdMode` 2 is `SOCD_MODE_SECOND_INPUT_PRIORITY`, last input wins, which behaves like "snap tap".
  - The existing hotkeys S1+S2+Left/Right switch the d-pad to LS/RS, and that **persists**.
    gamepadii should check `dpadMode`=digital before calling a keyboard profile usable.
  - Only put keys on d-pad slots that are fine under SOCD, i.e. movement.
- All 32 slots, including A3, A4 and E1–E12, can be targets of `BUTTON_PRESS_*` pin actions and are
  sent by the keyboard driver.
- **FN** (`BUTTON_PRESS_FN`, GpioAction 19) sets `AUX_MASK_FUNCTION` and is **never turned into a
  key**. That makes it a silent layer key for hotkeys.

### 2.6 Pin profiles

There are **6 profiles**: profile 1 (core) plus `gpioMappingsSets` (`max_count = 5`).
**The user already has 5 in use** (Default, SilkSong, Celeste, DKC, TGX), so **only one is free**.
`profileNumber` is 2 right now.

### 2.7 Hotkeys

There are 16 global hotkey entries, the same for every profile. Each is aux mask + button mask +
d-pad mask, encoded in the API as `buttonsMask` with d-pad bits 16–19.

Relevant actions in v0.7.12:

| Action | Value |
|---|---|
| `HOTKEY_LOAD_PROFILE_1..4` | 15–18 |
| `HOTKEY_LOAD_PROFILE_5..6` | 71–72 |
| `HOTKEY_NEXT_PROFILE` | 35 |
| `HOTKEY_PREVIOUS_PROFILE` | 42 |
| `HOTKEY_MENU_NAV_TOGGLE` | 50 |
| `HOTKEY_SAVE_CONFIG` | 43 |

Seven hotkeys are configured now:

| Combo | Action |
|---|---|
| S1+S2+Up | Home button |
| S1+S2+Down | D-pad digital |
| S1+S2+Left | D-pad as left stick |
| S1+S2+Right | D-pad as right stick |
| S2+A1+Up | SOCD up priority |
| S2+A1+Down | SOCD neutral |
| S2+A1+Left | SOCD last input |

**Does a hotkey combo also send keys in keyboard mode?**
- **While the whole combo is held, no.** `selectHotkey()` clears the matched button and d-pad bits
  before the keyboard driver runs.
- **But any button pressed before the combo is complete sends its key for those frames.** Pressing
  Esc+M+W in quick succession would leak Esc and M first, then fire a hotkey.
- **FN never leaks.** Hold FN first, then the other button, and nothing is typed.
- `HOTKEY_HOME_BUTTON` and the other `HOTKEY_*_BUTTON` actions *inject* a logical button, and the
  keyboard driver then sends **that slot's key**.

### 2.8 A profile switch writes flash

`LOAD_PROFILE` and `NEXT/PREV` set `reqSave` and fire `GPStorageSaveEvent`. Each switch is a flash
write, a short hiccup plus wear. That is fine for occasional layer changes (on foot ↔ ship). It is
**not** fine for hold-to-layer, and **v0.7.12 has no momentary layers.**

### 2.9 Input mode is device-global and persistent

- Input mode is not per profile.
- Boot buttons use the **logical** buttons of the **active profile**: `inputModeR2`=3 (keyboard),
  `inputModeB2`=0 (XInput) and so on. In a keyboard profile, "hold R2 while plugging in" means
  whichever GPIO that profile maps to logical R2.
- The chosen mode is saved.
- gamepadii should set input mode through the config API and show the boot buttons for each profile.

### 2.10 The config API needs web-config mode

The HTTP API exists only in web-config mode (RNDIS, after a reboot into config). gamepadii can't
change the keymap or profiles *live*. **Live layering = the ≤6 stored profiles + hotkeys only.**

### 2.11 Pressing and holding

Keys mirror the button state 1:1. There is no tap/hold logic and no repeat on the pad side; the
OS's key repeat applies to held keys. Debounce is 5 ms.

### 2.12 USB identity

The keyboard descriptor is "GP2040-CE (Keyboard)", VID 0xCAFE / PID 0x0001 (hard-coded). This is
useful for per-device rules (keyd, udev).

---

## 3. Linux side: what F13–F24 turn into

With the standard `evdev` xkb rules, `inet(evdev)` is always included. I compiled it locally with
`xkbcli compile-keymap --layout us`:

| HID | evdev | Keysym on this machine | Safe? |
|---|---|---|---|
| F13 (0x68) | KEY_F13 | **XF86Tools** | yes (unbound) |
| F14–F18 (0x69–0x6D) | KEY_F14–F18 | **XF86Launch5–XF86Launch9** | yes |
| F19 (0x6E) | KEY_F19 | F19 | yes |
| F20 (0x6F) | KEY_F20 | **XF86AudioMicMute** | **no**: niri `binds.kdl` sends it to `noctalia msg mic-mute` (system mic mute) |
| F21–F23 | KEY_F21–F23 | **XF86TouchpadToggle / On / Off** | **no** (touchpad control) |
| F24 (0x73) | KEY_F24 | F24 | yes |

**Usable: F13–F19 and F24, eight keys.** Bind them by their keysym names (`XF86Tools`,
`XF86Launch5`, …), not "F13". OBS's own key translation will probably not even show
XF86Tools/XF86LaunchN as F13–F18.

F20 can be kept on purpose as a "system mic mute" key. It mutes the PipeWire default source, so
OBS's mic goes silent, but OBS's mixer will not show it as muted.

---

## 4. No Man's Sky (PC): left hand on the S16, right hand on the mouse

### 4.1 Default PC bindings

There is no official current list. These are cross-checked defaults, accurate as of my sources
(Miraheze wiki, Shacknews/NEXT, Steam threads). **Check in game (Options → Controls) before
flashing.**

| Context | Action | Key | Hold/tap |
|---|---|---|---|
| On foot | Move | W A S D | hold |
| | Jump / jetpack / swim up | Space | hold for jetpack |
| | Sprint | Left Shift | hold |
| | Interact / pick up / talk | E | often **hold** |
| | Scan (scanner) | C | tap |
| | Analysis visor | F | toggle/hold (sources differ) |
| | Melee / swim down | Q | tap / hold |
| | Reload | R | tap |
| | Switch weapon mode | G | tap |
| | Torch | T | tap |
| | Quick menu | X | tap/hold |
| | Build menu | Z | tap |
| | Inventory | Tab | tap |
| | Options / back | Esc | tap |
| | Show HUD | H | tap |
| | Auto-walk | K | toggle |
| | Visor survey mode prev/next | 1 / 3 | tap |
| | Fire / alt fire / tag | LMB / RMB / MMB, wheel | mouse |
| Starship | Thrust / brake / roll | W / S / A D | hold |
| | Boost | Left Shift | hold |
| | Pulse engine | Space | **hold** |
| | Land / exit | E | **hold** |
| | Take off | hold W (or Space) | hold |
| | Scan | C | tap |
| | Weapon mode | G | tap |
| | Free look | Left Alt | **hold** |
| | Next / previous target | `.` / `,` | tap |
| | Galactic map | M | tap |
| Exocraft | Accelerate / brake | W / S | hold |
| | Boost | Left Shift | hold |
| | Jump / vertical thrust | Space | hold |
| | Horn | Left Ctrl | tap |
| | Race checkpoint place / delete | V / B | tap (rare) |
| | Exit | E | hold |
| Galactic map | Leave | M | tap |
| | Set waypoint | F | tap |
| | Cycle destination | Q / E | tap |
| Build mode | Open | Z | tap |
| | Place / back | LMB / RMB | mouse |
| | Wire/switch toggle | Q | tap |
| | Rotate | R / wheel | **unverified for Worlds Part II** |
| Quick menu | Bind an action to a number | Ctrl + 0–9 while hovering the action | – |
| Photo mode | Entered via the quick menu / Options | mouse + WASD for the camera | **controls unverified** |

**Rebinding.** Full keyboard/mouse rebinding has existed since Beyond 2.09. The design below **uses
the defaults**, so NMS needs no changes.

**Conflicts.** None with the pad keys:
- NMS never sees F13–F24, because niri consumes them (see §5).
- NMS reuses the same keys across contexts (E, Space, Shift, C, G), which is why one layout covers
  on foot, ship and exocraft.

### 4.2 Shared keymap (32/32 slots, also used by OBS)

| Slot | Key | HID | Used by |
|---|---|---|---|
| Up | W | 26 | NMS move (d-pad slot → SOCD last-win) |
| Down | S | 22 | NMS |
| Left | A | 4 | NMS |
| Right | D | 7 | NMS |
| B1 | Space | 44 | jump / jetpack / pulse |
| B2 | Left Shift | 225 | sprint / boost |
| B3 | E | 8 | interact |
| B4 | C | 6 | scan |
| L1 | F | 9 | analysis visor |
| R1 | X | 27 | quick menu |
| L2 | Tab | 43 | inventory |
| R2 | Q | 20 | melee / swim down |
| S1 | M | 16 | galactic map (hotkey-sensitive slot, rare key) |
| S2 | Esc | 41 | options (hotkey-sensitive slot, rare key) |
| L3 | R | 21 | reload |
| R3 | G | 10 | weapon mode |
| A1 | **F24** | 115 | OBS: save replay (works in every profile) |
| A2 | **F19** | 110 | OBS: record toggle |
| A3 | T | 23 | torch |
| A4 | Z | 29 | build menu |
| E1 | Left Alt | 226 | ship free look |
| E2 | Left Ctrl | 224 | exocraft horn **and** OBS "shift" modifier |
| E3 | `,` | 54 | previous target |
| E4 | `.` | 55 | next target |
| E5 | K | 14 | auto-walk |
| E6 | H | 11 | HUD toggle (spare; swap for `2` if you use quick-menu numbers) |
| E7 | **F13** | 104 | OBS |
| E8 | **F14** | 105 | OBS |
| E9 | **F15** | 106 | OBS |
| E10 | **F16** | 107 | OBS |
| E11 | **F17** | 108 | OBS |
| E12 | **F18** | 109 | OBS |

**Does it fit in 32 slots? Yes, exactly:** 24 NMS keys (one shared Ctrl) plus 8 OBS keys.

Headroom comes only from dropping something. The candidates are H, K, or `,`/`.`, which could be
freed for the quick-menu numbers `2`/`4`. A rule worth keeping: keys on S1/S2/A1 must never be
pressed together with W, because of the existing S1+S2+dpad and S2+A1+dpad hotkeys.

### 4.3 Profile "NMS Foot"

Ranked by how often each action is used and how easy the button is to reach.

```
                 [m1: W]            [ls: X quick]               (fn1: K auto)(fn2: F24 clip)(fn3: M map)(fn4: Esc)
 [◄: A] [▼: S]               [p1: E ]  [p2: Tab] [p3: R  ] [p4: T  ]
                  [►: D]     [k1: F ]  [k2: Q  ] [k3: G  ] [k4: Z  ]
                                      [rs: C scan]
          [m2: Shift]   [ up: SPACE ]                       (turbo: FN, the silent layer key)
```

| Button | GPIO | → slot | Key | Why |
|---|---|---|---|---|
| m1 | 27 | Up | W | the "W" position: middle finger reaches up, as on a keyboard |
| left/down/right | 5/3/4 | Left/Down/Right | A/S/D | ring / middle / index |
| up (big thumb) | 2 | B1 | Space | jetpack/pulse, held a lot; matches the user's "thumb = jump" habit |
| m2 (thumb) | 26 | B2 | Left Shift | sprint/boost (hold). Next to `up`, so a flat thumb can hold sprint and jetpack together |
| rs (thumb) | 19 | B4 | C | scan, tapped often while moving |
| p1 | 10 | B3 | E | interact (hold). Index reach, like E/R on a keyboard |
| k1 | 6 | L1 | F | analysis visor. Index reach |
| ls | 18 | R1 | X | quick menu (photo mode, summon ship) |
| p2 / k2 | 11 / 7 | L2 / R2 | Tab / Q | inventory / melee. Stretch |
| p3 / k3 | 12 / 8 | L3 / R3 | R / G | reload / weapon mode. Hand shift |
| p4 / k4 | 13 / 9 | A3 / A4 | T / Z | torch / build. Rare |
| fn1 | 21 | E5 | K | auto-walk, which helps a one-handed setup |
| fn2 | 20 | A1 | F24 | **OBS save replay**, global via niri |
| fn3 | 16 | S1 | M | galactic map |
| fn4 | 17 | S2 | Esc | options |
| turbo | 14 | FN | – | layer key; sends nothing |

That is 20 keys on 21 inputs. **A single profile is enough for most play.** The flight layer is a
convenience.

### 4.4 Profile "NMS Flight" (base: NMS Foot; also covers the exocraft)

| Button | GPIO | → slot | Key | Why |
|---|---|---|---|---|
| k1 | 6 | E1 | Left Alt | free look (hold); the visor is useless in a ship |
| p3 | 12 | E4 | `.` | next target |
| p4 | 13 | E3 | `,` | previous target |
| k4 | 9 | E2 | Left Ctrl | exocraft horn |

Everything else is inherited: W/S thrust and brake, A/D roll, Space pulse, Shift boost, E land/exit,
C scan, G weapon mode, M map, X/Tab/Esc.

### 4.5 Switching layers (global hotkeys, FN-based so nothing leaks)

Hold `turbo` (FN) first, then press:

| Combo (physical) | Action |
|---|---|
| FN + `left`  | LOAD_PROFILE → NMS Foot |
| FN + `down`  | LOAD_PROFILE → NMS Flight |
| FN + `right` | LOAD_PROFILE → OBS Pad |
| FN + `fn2`   | MENU_NAV_TOGGLE (OLED menu, since `turbo` is no longer the menu button in these profiles) |

**Hotkeys match logical buttons, and physical `left` is `Left` in NMS profiles but `E7` in the OBS
profile.** So gamepadii must emit **one entry per distinct logical slot**:

- FN+Left and FN+E7 → Foot
- FN+Down and FN+E8 → Flight
- FN+Right and FN+E9 → OBS
- FN+A1 → menu

That is 7 entries, 14 of 16 with the existing 7. E bits (20–31) in `buttonsMask` are honoured by
`pressedButton()`, though the web UI may not show them.

A target maps to the same physical button in every profile, so a switch can't ping-pong.

In gamepad profiles `turbo` stays `MENU_NAVIGATION_TOGGLE` and there is no FN pin, so these hotkeys
can never fire there.

---

## 5. OBS sidekick

### 5.1 The Wayland problem

The user is on **niri 26.04 (Wayland)** with OBS 32.2.2. OBS's built-in hotkeys work only while
OBS has focus.

- **Native portal support is still a draft.** OBS PR #13680 (xdg-desktop-portal GlobalShortcuts)
  is a draft and unmerged. PR #13661 (compositor protocol) is open.
- **The plugins need a portal niri doesn't provide.** Community plugins (obs-wayland-hotkeys,
  obs-global-shortcuts) use the GlobalShortcuts portal. niri routes portals to gnome/gtk
  (`niri-portals.conf`), and I found **no evidence niri implements GlobalShortcuts**. niri PR #1165
  was a proposal.

**Recommended route: the compositor owns the keys, and obs-websocket does the work.** The pieces are
already installed:

- `obs-cmd` 1.0.2 is in `/usr/bin`.
- obs-websocket is **enabled on port 4459 with auth**.

The chain:

1. The pad sends F13–F19/F24.
2. A niri `binds` entry catches the keysym. It is consumed, so the game or OBS never sees it, and it
   works with any window focused.
3. The bind calls `spawn "obs-pad" "<action>"`.
4. The `obs-pad` wrapper runs `obs-cmd -w obsws://localhost:4459/$PASSWORD …`, reading the password
   from oo7/secret-tool or a 0600 file. **The password must not go in the niri config.**

Use `repeat=false` on the binds. Use `allow-inhibiting=false` so they still fire when a fullscreen
XWayland game takes the shortcut inhibitor.

Alternatives:

- **keyd** (`cachyos-extra-v3`) with a per-device `[ids] cafe:0001` section mapping F13 →
  `command(...)`. It works under any compositor, but commands run as root.
- **gamepadii itself as a daemon.** It reads the pad's evdev node (without grabbing it) and talks to
  obs-websocket directly. That needs the `input` group and duplicates what niri already does.

### 5.2 Actions

8 keys, with Left Ctrl on the pad as a "shift" key, give 16 actions:

| Key (keysym) | Plain | With Ctrl (pad `m2` held) |
|---|---|---|
| F13 (XF86Tools)   | Scene 1 | Scene 5 |
| F14 (XF86Launch5) | Scene 2 | Scene 6 |
| F15 (XF86Launch6) | Scene 3 | Scene 7 |
| F16 (XF86Launch7) | Scene 4 | Scene 8 |
| F17 (XF86Launch8) | Mic mute toggle (`obs-cmd audio toggle "Mic/Aux"`) | Recording pause toggle |
| F18 (XF86Launch9) | Desktop audio mute toggle | Virtual camera toggle |
| F19 | Recording start/stop | **Streaming start/stop** (behind the modifier on purpose) |
| F24 | **Save replay** (`obs-cmd replay save`) | Replay buffer start/stop |

### 5.3 Profile "OBS Pad"

```
                 [m1: F16 sc4]       [ls: –]                    (fn1: –)(fn2: F24 clip)(fn3: –)(fn4: –)
 [◄: F13 sc1] [▼: F14 sc2]   [p1: F17 mic] [p2: F18 desk] [p3: F19 rec] [p4: –]
                  [►: F15 sc3]   [k1: –]  [k2: –] [k3: –] [k4: –]
                                       [rs: –]
          [m2: CTRL (shift)]   [ up: F24 SAVE REPLAY ]               (turbo: FN)
```

| Button | GPIO | → slot | Key |
|---|---|---|---|
| left / down / right / m1 | 5 / 3 / 4 / 27 | E7 / E8 / E9 / E10 | F13–F16 (scenes; FN + left/down/right still switch layers) |
| p1 / p2 / p3 | 10 / 11 / 12 | E11 / E12 / A2 | F17 / F18 / F19 |
| up (big thumb) | 2 | A1 | F24 save replay |
| fn2 | 20 | A1 | F24 (same as in the NMS profiles) |
| m2 | 26 | E2 | Left Ctrl (shift modifier; hold first) |
| turbo | 14 | FN | – |
| all others | – | NONE (-10) | nothing, so nothing gets typed into the focused app |

Only 9 distinct keys are available, because the 32 slots are spent. Leaving buttons NONE is
deliberate, since the letter slots would type into apps. If more distinct OBS buttons are wanted,
move OBS to its own loadout with its own keymap (§6.3); that costs a config-mode reboot to switch.

### 5.4 Generated niri snippet (gamepadii could emit this)

```kdl
// ~/.config/niri/gamepadii-obs.kdl — include from config.kdl
binds {
    XF86Tools        repeat=false allow-inhibiting=false { spawn "obs-pad" "scene" "1"; }
    Ctrl+XF86Tools   repeat=false allow-inhibiting=false { spawn "obs-pad" "scene" "5"; }
    XF86Launch5      repeat=false allow-inhibiting=false { spawn "obs-pad" "scene" "2"; }
    // … XF86Launch6/7 = scenes 3/4 (Ctrl: 7/8)
    XF86Launch8      repeat=false allow-inhibiting=false { spawn "obs-pad" "mic-toggle"; }
    Ctrl+XF86Launch8 repeat=false allow-inhibiting=false { spawn "obs-pad" "record-pause"; }
    XF86Launch9      repeat=false allow-inhibiting=false { spawn "obs-pad" "desktop-toggle"; }
    F19              repeat=false allow-inhibiting=false { spawn "obs-pad" "record-toggle"; }
    Ctrl+F19         repeat=false allow-inhibiting=false { spawn "obs-pad" "stream-toggle"; }
    F24              repeat=false allow-inhibiting=false { spawn "obs-pad" "replay-save"; }
    Ctrl+F24         repeat=false allow-inhibiting=false { spawn "obs-pad" "replay-toggle"; }
}
```

Check that niri merges a second `binds {}` block from an included file. If it doesn't, generate
lines for `binds.kdl`.

---

## 6. Draft KDL 2.0 schema

### 6.1 Ideas

1. **A board file** (shipped with gamepadii) names the physical buttons → GPIOs, so profiles never
   mention GPIOs.
2. **A keymap** is a device-level node. One is active per device.
3. **A profile** has a `base` (inherit and override), `input-mode` (a *requirement*, since it is
   device-global), a `keymap` reference, `requires` (global options the profile depends on), and
   `pins` (physical name → logical slot or a special action).
4. **Hotkeys** are written with physical names. The compiler expands each into one entry per logical
   slot that the physical button has across the loadout's profiles.
5. **A loadout** is the ≤6 profiles stored on the device at once, plus the keymap and hotkeys.
   gamepadii swaps loadouts through config mode.
6. *(Later)* a higher-level `bind <button> <Key>` form, where the compiler allocates slots
   automatically:
   - movement-tagged keys go to d-pad slots;
   - rarely chorded keys go to S1/S2/A1;
   - it errors above 32 distinct keys or when a modifier+key chord would need one button.

Key names follow W3C `KeyboardEvent.code` (`KeyW`, `ShiftLeft`, `F13`, `Comma`). A typed literal
such as `(hid)0x68` covers anything else.

### 6.2 Board file excerpt

```kdl
board "haute42-s16" {
    firmware-board "Haute42COSMOX"
    button left   gpio=5;  button down gpio=3;  button right gpio=4
    button m1     gpio=27; button up   gpio=2   size="26.6mm"
    button m2     gpio=26; button rs   gpio=19; button ls gpio=18
    button p1 gpio=10; button p2 gpio=11; button p3 gpio=12; button p4 gpio=13
    button k1 gpio=6;  button k2 gpio=7;  button k3 gpio=8;  button k4 gpio=9
    button fn1 gpio=21 label="Touchpad?"; button fn2 gpio=20 label="Home"
    button fn3 gpio=16 label="Share";     button fn4 gpio=17 label="Options"
    button turbo gpio=14
}
```

### 6.3 Profiles

```kdl
keymap "kb-main" {
    Up KeyW; Down KeyS; Left KeyA; Right KeyD
    B1 Space; B2 ShiftLeft; B3 KeyE; B4 KeyC
    L1 KeyF; R1 KeyX; L2 Tab; R2 KeyQ
    S1 KeyM; S2 Escape; L3 KeyR; R3 KeyG
    A1 F24; A2 F19; A3 KeyT; A4 KeyZ
    E1 AltLeft; E2 ControlLeft; E3 Comma; E4 Period
    E5 KeyK; E6 KeyH
    E7 F13; E8 F14; E9 F15; E10 F16; E11 F17; E12 F18
}

profile "NMS Foot" {
    base "Default"
    input-mode keyboard
    keymap "kb-main"
    requires { dpad-mode digital; four-way #false; socd last-input }
    pins {
        m1 Up; left Left; down Down; right Right
        up B1; m2 B2; rs B4
        p1 B3; k1 L1; ls R1
        p2 L2; k2 R2; p3 L3; k3 R3; p4 A3; k4 A4
        fn1 E5; fn2 A1; fn3 S1; fn4 S2
        turbo fn
    }
}

profile "NMS Flight" {
    base "NMS Foot"
    pins { k1 E1; p3 E4; p4 E3; k4 E2 }
}

profile "OBS Pad" {
    base "Default"
    input-mode keyboard
    keymap "kb-main"
    pins {
        default none                 // every unlisted button → NONE
        left E7; down E8; right E9; m1 E10
        p1 E11; p2 E12; p3 A2
        up A1; fn2 A1; m2 E2
        turbo fn
    }
}

hotkeys {
    hotkey fn left  load-profile="NMS Foot"
    hotkey fn down  load-profile="NMS Flight"
    hotkey fn right load-profile="OBS Pad"
    hotkey fn fn2   action=menu-nav-toggle
}

loadout "desk-keyboard" {
    keymap "kb-main"
    profiles "Default" "NMS Foot" "NMS Flight" "OBS Pad" "SilkSong" "TGX"   // max 6, profile 1 first
}

host "niri" {
    obs websocket="obsws://localhost:4459" password-from="secret-tool:obs-websocket"
    on F13            { obs scene "Game" }
    on F13 ctrl=#true { obs scene "Scene 5" }
    on F24            { obs replay-save }
}
```

### 6.4 Future high-level form (slots compiled automatically)

```kdl
profile "NMS Foot" {
    input-mode keyboard
    bind m1 KeyW movement=#true "Forward"
    bind up Space hold=#true "Jump / jetpack"
    bind m2 ShiftLeft hold=#true "Sprint"
    bind rs KeyC "Scan"
    // …
}
```

---

## 7. Open questions for the user

1. **Profile slots.** Only 1 of the 6 is free, and Foot, Flight and OBS need 3. Options:
   - (a) A minimal setup: NMS Foot only, with OBS clip/record on the function row.
   - (b) Retire or merge Celeste. It is Default plus thumb=B1, so it could be rebuilt any time.
   - (c) gamepadii loadouts: swap the whole profile set, which costs a config-mode reboot.
2. **The current keymap gets replaced.** It is the board default arcade map (Shift/Z/X/V/Ctrl/Alt/
   Space/C/5/1/=/-/9/F2). Does anything rely on it, e.g. MAME?
3. **Hotkey cleanup.** May the existing S1+S2 and S2+A1 hotkeys move behind FN as well? That would
   remove any chance of firing them while gaming. It would also mean an FN pin in gamepad profiles,
   or accepting that those hotkeys exist only there.
4. **Physical check.** Are the 4 small buttons, left to right, really Touchpad/Home/Share/Options =
   GP21/20/16/17? Where is Turbo (GP14), and is it comfortable as the FN layer key?
5. **Sprint ergonomics.** Can the left thumb hold `m2` (Shift) and `up` (Space) together comfortably?
   If not, move Shift to `k1` and the visor to `ls`. Does NMS have a toggle-sprint option in the
   current build?
6. **NMS verification.** Check the in-game bindings for the analysis visor (F), the build-mode
   rotate/snap keys and the photo-mode controls in the current version. Are quick-menu number
   bindings (Ctrl+number) wanted? They would need H or K given up.
7. **OBS.** Scene names, the mic and desktop source names, and whether replay buffer and virtual
   camera are used. Is F20 (system mic mute) wanted as a feature or avoided?
8. **Delivery.** Should gamepadii write the niri snippet and the `obs-pad` wrapper itself, or only
   document them?

## Sources

- GP2040-CE v0.7.12:
  - `configs/Haute42COSMOX/BoardConfig.h` and `assets/Haute42_G16.png`
  - `src/drivers/keyboard/KeyboardDriver.cpp`, `headers/drivers/keyboard/KeyboardDescriptors.h`
  - `src/gamepad.cpp`, `headers/gamepad.h`, `src/gp2040.cpp`, `src/storagemanager.cpp`
  - `src/webconfig.cpp`, `proto/enums.proto`, `proto/config.proto`, `www/src/Data/Keyboard.js`
- Device fixtures: `tests/fixtures/api/s16-v0.7.12/*.json`
- [The Arcade Stick: Haute42 S16 review](https://thearcadestick.com/haute42-s16/)
- [The Arcade Stick: Haute42 S16-C review](https://thearcadestick.com/haute42-s16-c/)
- [juanminidiy: Haute42 S16 review](https://juanminidiy.com/haute42-s16-review/)
- [No Man's Sky Wiki (Miraheze): Controls](https://nomanssky.miraheze.org/wiki/Controls)
- [Shacknews: NMS NEXT PC keyboard controls](https://www.shacknews.com/article/106408/pc-keyboard-controls-and-key-bindings---no-mans-sky-next)
- [Steam discussion: visor key F, 1/3 clash with the quick menu](https://steamcommunity.com/app/275850/discussions/0/3801650561764166262)
- [OBS PR #13680: Wayland global hotkeys via portal (draft)](https://github.com/obsproject/obs-studio/pull/13680)
- [OBS PR #13661](https://obs-versions.com/community/pulls/13661)
- [obs-wayland-hotkeys (OBS forum)](https://obsproject.com/forum/resources/obs-wayland-hotkeys-scene-switching.2273)
- [niri PR #1165: GlobalShortcuts proposal](https://github.com/YaLTeR/niri/pull/1165)
- [obs-cmd](https://github.com/grigio/obs-cmd)
- Local checks:
  - `xkbcli compile-keymap --layout us` (F13–F24 keysyms)
  - `~/.config/niri/binds.kdl` (XF86AudioMicMute bound)
  - `~/.config/obs-studio/plugin_config/obs-websocket/config.json` (port 4459, auth on)
