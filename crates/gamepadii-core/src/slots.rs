//! The pin mappings of a profile over the control channel (v1.4, §3.10 of
//! `docs/design/control-channel.md`): READ_SLOT and WRITE_SLOT.
//!
//! `gamepadii profile show` reads a slot live, in any mode. Its JSON is
//! also a slot file: `gamepadii profile write FILE --slot N` writes it to
//! profile N (2–6) in RAM, and selects and saves it on request. That is
//! the "per-game profile" path: write a scratch slot and select it, with
//! no reboot. gamepadii backs up the slot before it writes it, in the same
//! file format, so `profile write` of the backup undoes the write.
//!
//! In a slot, the action `ASSIGNED_TO_ADDON` (0) or `RESERVED` (-5) means
//! "the pin of profile 1". A pin that profile 1 reserves or gives to an
//! add-on keeps profile 1's action whatever the slot says. A slot file can
//! leave out pins: they get 0, so they follow profile 1.

use std::collections::BTreeSet;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::buttons::Button;
use crate::channel::{Slot, SlotPin};
use crate::commands::Route;
use crate::error::{Error, Result};

/// `GpioAction` in main (cb01f703, `proto/enums.proto`), by value.
pub const ACTIONS: &[(i16, &str)] = &[
    (-10, "NONE"),
    (-5, "RESERVED"),
    (0, "ASSIGNED_TO_ADDON"),
    (1, "BUTTON_PRESS_UP"),
    (2, "BUTTON_PRESS_DOWN"),
    (3, "BUTTON_PRESS_LEFT"),
    (4, "BUTTON_PRESS_RIGHT"),
    (5, "BUTTON_PRESS_B1"),
    (6, "BUTTON_PRESS_B2"),
    (7, "BUTTON_PRESS_B3"),
    (8, "BUTTON_PRESS_B4"),
    (9, "BUTTON_PRESS_L1"),
    (10, "BUTTON_PRESS_R1"),
    (11, "BUTTON_PRESS_L2"),
    (12, "BUTTON_PRESS_R2"),
    (13, "BUTTON_PRESS_S1"),
    (14, "BUTTON_PRESS_S2"),
    (15, "BUTTON_PRESS_A1"),
    (16, "BUTTON_PRESS_A2"),
    (17, "BUTTON_PRESS_L3"),
    (18, "BUTTON_PRESS_R3"),
    (19, "BUTTON_PRESS_FN"),
    (20, "BUTTON_PRESS_DDI_UP"),
    (21, "BUTTON_PRESS_DDI_DOWN"),
    (22, "BUTTON_PRESS_DDI_LEFT"),
    (23, "BUTTON_PRESS_DDI_RIGHT"),
    (24, "SUSTAIN_DP_MODE_DP"),
    (25, "SUSTAIN_DP_MODE_LS"),
    (26, "SUSTAIN_DP_MODE_RS"),
    (27, "SUSTAIN_SOCD_MODE_UP_PRIO"),
    (28, "SUSTAIN_SOCD_MODE_NEUTRAL"),
    (29, "SUSTAIN_SOCD_MODE_SECOND_WIN"),
    (30, "SUSTAIN_SOCD_MODE_FIRST_WIN"),
    (31, "SUSTAIN_SOCD_MODE_BYPASS"),
    (32, "BUTTON_PRESS_TURBO"),
    (33, "BUTTON_PRESS_MACRO"),
    (34, "BUTTON_PRESS_MACRO_1"),
    (35, "BUTTON_PRESS_MACRO_2"),
    (36, "BUTTON_PRESS_MACRO_3"),
    (37, "BUTTON_PRESS_MACRO_4"),
    (38, "BUTTON_PRESS_MACRO_5"),
    (39, "BUTTON_PRESS_MACRO_6"),
    (40, "CUSTOM_BUTTON_COMBO"),
    (41, "BUTTON_PRESS_A3"),
    (42, "BUTTON_PRESS_A4"),
    (43, "BUTTON_PRESS_E1"),
    (44, "BUTTON_PRESS_E2"),
    (45, "BUTTON_PRESS_E3"),
    (46, "BUTTON_PRESS_E4"),
    (47, "BUTTON_PRESS_E5"),
    (48, "BUTTON_PRESS_E6"),
    (49, "BUTTON_PRESS_E7"),
    (50, "BUTTON_PRESS_E8"),
    (51, "BUTTON_PRESS_E9"),
    (52, "BUTTON_PRESS_E10"),
    (53, "BUTTON_PRESS_E11"),
    (54, "BUTTON_PRESS_E12"),
    (55, "DIGITAL_DIRECTION_UP"),
    (56, "DIGITAL_DIRECTION_DOWN"),
    (57, "DIGITAL_DIRECTION_LEFT"),
    (58, "DIGITAL_DIRECTION_RIGHT"),
    (59, "ANALOG_DIRECTION_LS_X_NEG"),
    (60, "ANALOG_DIRECTION_LS_X_POS"),
    (61, "ANALOG_DIRECTION_LS_Y_NEG"),
    (62, "ANALOG_DIRECTION_LS_Y_POS"),
    (63, "ANALOG_DIRECTION_RS_X_NEG"),
    (64, "ANALOG_DIRECTION_RS_X_POS"),
    (65, "ANALOG_DIRECTION_RS_Y_NEG"),
    (66, "ANALOG_DIRECTION_RS_Y_POS"),
    (67, "ANALOG_DIRECTION_MOD_LOW"),
    (68, "ANALOG_DIRECTION_MOD_HIGH"),
    (69, "BUTTON_PRESS_INPUT_REVERSE"),
    (70, "SUSTAIN_FOCUS_MODE"),
    (71, "SUSTAIN_4_8_WAY_MODE"),
    (72, "MENU_NAVIGATION_UP"),
    (73, "MENU_NAVIGATION_DOWN"),
    (74, "MENU_NAVIGATION_LEFT"),
    (75, "MENU_NAVIGATION_RIGHT"),
    (76, "MENU_NAVIGATION_SELECT"),
    (77, "MENU_NAVIGATION_BACK"),
    (78, "MENU_NAVIGATION_TOGGLE"),
    (79, "MODE_GUITAR_FRET_GREEN"),
    (80, "MODE_GUITAR_FRET_RED"),
    (81, "MODE_GUITAR_FRET_YELLOW"),
    (82, "MODE_GUITAR_FRET_BLUE"),
    (83, "MODE_GUITAR_FRET_ORANGE"),
    (84, "MODE_GUITAR_FRET_SOLO_GREEN"),
    (85, "MODE_GUITAR_FRET_SOLO_RED"),
    (86, "MODE_GUITAR_FRET_SOLO_YELLOW"),
    (87, "MODE_GUITAR_FRET_SOLO_BLUE"),
    (88, "MODE_GUITAR_FRET_SOLO_ORANGE"),
    (89, "MODE_GUITAR_WHAMMY"),
    (90, "MODE_GUITAR_PICKUP"),
    (91, "MODE_GUITAR_TILT"),
    (92, "MODE_DRUM_RED_DRUMPAD"),
    (93, "MODE_DRUM_BLUE_DRUMPAD"),
    (94, "MODE_DRUM_YELLOW_DRUMPAD"),
    (95, "MODE_DRUM_GREEN_DRUMPAD"),
    (96, "MODE_DRUM_YELLOW_CYMBAL"),
    (97, "MODE_DRUM_BLUE_CYMBAL"),
    (98, "MODE_DRUM_GREEN_CYMBAL"),
    (99, "MODE_DRUM_KICK_PEDAL_LEFT"),
    (100, "MODE_DRUM_KICK_PEDAL_RIGHT"),
    (101, "MODE_HOTAS_RUDDER_LEFT"),
    (102, "MODE_HOTAS_RUDDER_RIGHT"),
    (103, "MODE_HOTAS_THROTTLE_FORWARD"),
    (104, "MODE_HOTAS_THROTTLE_REVERSE"),
    (105, "MODE_HOTAS_ROCKER_LEFT"),
    (106, "MODE_HOTAS_ROCKER_RIGHT"),
    (107, "MODE_HOTAS_PEDAL_LEFT"),
    (108, "MODE_HOTAS_PEDAL_RIGHT"),
    (109, "MODE_HOTAS_PEDAL_RUDDER_LEFT"),
    (110, "MODE_HOTAS_PEDAL_RUDDER_RIGHT"),
    (111, "MODE_WHEEL_STEERING_LEFT"),
    (112, "MODE_WHEEL_STEERING_RIGHT"),
    (113, "MODE_WHEEL_SHIFTER_GEAR_1"),
    (114, "MODE_WHEEL_SHIFTER_GEAR_2"),
    (115, "MODE_WHEEL_SHIFTER_GEAR_3"),
    (116, "MODE_WHEEL_SHIFTER_GEAR_4"),
    (117, "MODE_WHEEL_SHIFTER_GEAR_5"),
    (118, "MODE_WHEEL_SHIFTER_GEAR_6"),
    (119, "MODE_WHEEL_SHIFTER_GEAR_R"),
    (120, "MODE_WHEEL_SHIFTER_GEAR_N"),
    (121, "MODE_WHEEL_SHIFTER_GEAR_UP"),
    (122, "MODE_WHEEL_SHIFTER_GEAR_DOWN"),
    (123, "MODE_WHEEL_BUTTON_PLUS"),
    (124, "MODE_WHEEL_BUTTON_MINUS"),
    (125, "MODE_WHEEL_DIAL_UP"),
    (126, "MODE_WHEEL_DIAL_DOWN"),
    (127, "MODE_WHEEL_DIAL_ENTER"),
    (128, "MODE_WHEEL_PEDAL_GAS"),
    (129, "MODE_WHEEL_PEDAL_BRAKE"),
    (130, "MODE_WHEEL_PEDAL_CLUTCH"),
    (131, "SUSTAIN_PROFILE_SLIDER_POSITION_1"),
    (132, "SUSTAIN_PROFILE_SLIDER_POSITION_2"),
    (133, "SUSTAIN_PROFILE_SLIDER_POSITION_3"),
    (134, "SUSTAIN_PROFILE_SLIDER_POSITION_4"),
    (135, "SUSTAIN_PROFILE_SLIDER_POSITION_5"),
    (136, "SUSTAIN_PROFILE_SLIDER_POSITION_6"),
    (137, "SUSTAIN_PROFILE_SLIDER_POSITION_7"),
    (138, "SUSTAIN_PROFILE_SLIDER_POSITION_8"),
];

/// The action that means "the pin of profile 1" in a slot.
pub const FROM_PROFILE_1: i16 = 0;
const RESERVED: i16 = -5;

/// Whether `action` in a slot takes the pin of profile 1.
pub fn follows_profile_1(action: i16) -> bool {
    matches!(action, FROM_PROFILE_1 | RESERVED)
}

/// The short name of an action: the button for a button press (`B1`,
/// `Up`, `E12`), else the enum name in lower case with `-`
/// (`none`, `assigned-to-addon`, `turbo`, `macro-1`, `ddi-up`).
pub fn action_name(action: i16) -> Option<String> {
    if let Some(b) = Button::from_action(action.into()) {
        return Some(b.name().to_string());
    }
    let (_, name) = ACTIONS.iter().find(|(v, _)| *v == action)?;
    let short = name.strip_prefix("BUTTON_PRESS_").unwrap_or(name);
    Some(short.to_ascii_lowercase().replace('_', "-"))
}

fn normal(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The action a name gives: a short name ([`action_name`]) or the enum
/// name, in any case, with or without `-` and `_`.
pub fn action_from_name(name: &str) -> Option<i16> {
    let want = normal(name);
    ACTIONS
        .iter()
        .map(|(v, _)| *v)
        .find(|v| action_name(*v).is_some_and(|n| normal(&n) == want))
        .or_else(|| {
            ACTIONS
                .iter()
                .find(|(_, n)| normal(n) == want)
                .map(|(v, _)| *v)
        })
}

/// An action in a slot file: its number, or a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action(pub i16);

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => n
                .as_i64()
                .and_then(|n| i16::try_from(n).ok())
                .map(Action)
                .ok_or_else(|| de::Error::custom(format!("action {n}: not a GpioAction"))),
            serde_json::Value::String(s) => action_from_name(&s)
                .map(Action)
                .ok_or_else(|| de::Error::custom(format!("unknown action {s:?}"))),
            other => Err(de::Error::custom(format!(
                "action: a number or a name, not {other}"
            ))),
        }
    }
}

/// One pin, as `profile show` gives it and a slot file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinView {
    pub gpio: u8,
    /// The `GpioAction`.
    pub action: i16,
    /// [`action_name`]; `null` for a value gamepadii doesn't know.
    pub action_name: Option<String>,
    pub button_mask: u32,
    pub dpad_mask: u32,
    /// Profile 1's button on this GPIO, the name printed on the case;
    /// `null` when it has something else there.
    pub key: Option<Button>,
    /// The pad uses profile 1's pin here: the slot has 0 or -5, or
    /// profile 1 reserves the pin or gives it to an add-on.
    pub from_profile_1: bool,
}

/// `gamepadii profile show`: one slot, read live with READ_SLOT. Its JSON
/// is a slot file for `profile write` (the extra keys are ignored there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// `GP2040_BOARDCONFIG`, so a file isn't written to another board by
    /// mistake; `null` when the pad doesn't say.
    pub board: Option<String>,
    pub profile: u8,
    pub label: String,
    pub enabled: bool,
    pub active: bool,
    pub pins: Vec<PinView>,
}

impl SlotReport {
    /// `slot` as the pad runs it, next to profile 1 (`base`).
    pub fn new(host: String, board: Option<String>, slot: &Slot, base: &Slot) -> Self {
        let pins = slot
            .pins
            .iter()
            .zip(0u8..)
            .map(|(p, gpio)| {
                let core = base.pins.get(usize::from(gpio)).map_or(-10, |b| b.action);
                PinView {
                    gpio,
                    action: p.action,
                    action_name: action_name(p.action),
                    button_mask: p.button_mask,
                    dpad_mask: p.dpad_mask,
                    key: Button::from_action(core.into()),
                    from_profile_1: slot.profile != 1
                        && (follows_profile_1(p.action) || follows_profile_1(core)),
                }
            })
            .collect();
        SlotReport {
            via: Route::Control,
            host,
            board,
            profile: slot.profile,
            label: slot.label.clone(),
            enabled: slot.enabled(),
            active: slot.active(),
            pins,
        }
    }

    /// The slot file of this report: what a backup holds.
    pub fn file(&self) -> SlotFile {
        SlotFile {
            board: self.board.clone(),
            profile: Some(self.profile),
            label: Some(self.label.clone()),
            enabled: Some(self.enabled),
            pins: self
                .pins
                .iter()
                .map(|p| FilePin {
                    gpio: p.gpio,
                    action: Action(p.action),
                    button_mask: p.button_mask,
                    dpad_mask: p.dpad_mask,
                })
                .collect(),
        }
    }
}

/// A slot file: the JSON of `profile show`, a backup, or one written by
/// hand. Every key but `pins` is optional.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SlotFile {
    /// Checked against the pad's board config when both are there.
    #[serde(default)]
    pub board: Option<String>,
    /// The profile it was read from; `--slot` decides where it goes.
    #[serde(default)]
    pub profile: Option<u8>,
    #[serde(default)]
    pub label: Option<String>,
    /// `false` writes the profile disabled; absent is enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    pub pins: Vec<FilePin>,
}

/// One pin of a slot file. A pin that isn't in the file follows profile 1.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FilePin {
    pub gpio: u8,
    /// A number or a name (`B1`, `none`, `turbo`, `BUTTON_PRESS_UP`).
    pub action: Action,
    #[serde(default)]
    pub button_mask: u32,
    #[serde(default)]
    pub dpad_mask: u32,
}

impl Serialize for SlotFile {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Pin {
            gpio: u8,
            action: i16,
            action_name: Option<String>,
            button_mask: u32,
            dpad_mask: u32,
        }
        #[derive(Serialize)]
        struct File<'a> {
            schema: u32,
            board: &'a Option<String>,
            profile: Option<u8>,
            label: &'a Option<String>,
            enabled: Option<bool>,
            pins: Vec<Pin>,
        }
        File {
            schema: 1,
            board: &self.board,
            profile: self.profile,
            label: &self.label,
            enabled: self.enabled,
            pins: self
                .pins
                .iter()
                .map(|p| Pin {
                    gpio: p.gpio,
                    action: p.action.0,
                    action_name: action_name(p.action.0),
                    button_mask: p.button_mask,
                    dpad_mask: p.dpad_mask,
                })
                .collect(),
        }
        .serialize(s)
    }
}

impl SlotFile {
    pub fn parse(text: &str) -> Result<SlotFile> {
        serde_json::from_str(text).map_err(|e| Error::Usage(format!("not a slot file: {e}")))
    }
}

/// What `profile write` asks for, besides the file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlotWrite {
    /// The profile to write, 2–6.
    pub slot: u8,
    /// Select it (make it the active profile) now.
    pub select: bool,
    /// Save to flash.
    pub save: bool,
    /// In place of the file's label.
    pub label: Option<String>,
}

/// The WRITE_SLOT for `file` on a pad with `pins` pins, that runs
/// profile `active` and has board config `board`; `Usage` for what the pad
/// would refuse, before anything is sent.
pub fn plan(
    file: &SlotFile,
    write: &SlotWrite,
    pins: usize,
    active: u8,
    board: Option<&str>,
) -> Result<Slot> {
    if let (Some(want), Some(have)) = (file.board.as_deref(), board)
        && want != have
    {
        return Err(Error::Usage(format!(
            "the file is for board {want}, but the pad is {have}"
        )));
    }
    let mut slot_pins = vec![SlotPin::default(); pins];
    let mut seen = BTreeSet::new();
    for p in &file.pins {
        if !seen.insert(p.gpio) {
            return Err(Error::Usage(format!(
                "GPIO {} is twice in the file",
                p.gpio
            )));
        }
        let Some(pin) = slot_pins.get_mut(usize::from(p.gpio)) else {
            return Err(Error::Usage(format!(
                "GPIO {}: this pad has GPIO 0 to {}",
                p.gpio,
                pins.saturating_sub(1)
            )));
        };
        *pin = SlotPin {
            action: p.action.0,
            button_mask: p.button_mask,
            dpad_mask: p.dpad_mask,
        };
    }
    let flag = |on: bool, bit: u8| if on { bit } else { 0 };
    let slot = Slot {
        profile: write.slot,
        flags: flag(file.enabled != Some(false), Slot::ENABLED)
            | flag(write.select, Slot::ACTIVE)
            | flag(write.save, Slot::SAVE),
        label: write
            .label
            .clone()
            .or_else(|| file.label.clone())
            .unwrap_or_default(),
        pins: slot_pins,
    };
    slot.check(pins, active).map_err(Error::Usage)?;
    Ok(slot)
}

/// What the pad keeps of `sent` next to profile 1 (`base`): the pins that
/// profile 1 reserves or gives to an add-on take its action, masks 0.
pub fn expected(sent: &Slot, base: &Slot) -> Vec<SlotPin> {
    sent.pins
        .iter()
        .zip(&base.pins)
        .map(|(p, b)| {
            if follows_profile_1(b.action) {
                SlotPin {
                    action: b.action,
                    ..SlotPin::default()
                }
            } else {
                *p
            }
        })
        .collect()
}

/// The GPIOs whose mapping differs between `a` and `b`.
pub fn changed_pins(a: &[SlotPin], b: &[SlotPin]) -> Vec<u8> {
    let n = a.len().max(b.len());
    (0..n)
        .filter(|i| a.get(*i) != b.get(*i))
        .filter_map(|i| u8::try_from(i).ok())
        .collect()
}

/// `gamepadii profile write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotWriteReport {
    /// Always `control`: WRITE_SLOT.
    pub via: Route,
    pub host: String,
    pub profile: u8,
    /// The label and state before the write.
    pub before_label: String,
    pub before_enabled: bool,
    pub label: String,
    pub enabled: bool,
    /// The GPIOs whose mapping changed.
    pub changed_pins: Vec<u8>,
    /// The profile is the active one now (`--select`, or it was already).
    pub active: bool,
    /// Saved to flash (`--save`).
    pub saved: bool,
    /// STATUS says the pad applied the new mapping (the reinit is done);
    /// `null` when the profile isn't the active one.
    pub applied: Option<bool>,
    /// READ_SLOT gives what was written.
    pub verified: bool,
    /// The slot as it was, in the slot file format; `null` with
    /// `--no-backup`.
    pub backup: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::proto;

    #[test]
    fn the_action_table_is_main_s() {
        assert_eq!(ACTIONS.len(), 141);
        assert_eq!(ACTIONS.first(), Some(&(-10, "NONE")));
        assert_eq!(
            ACTIONS.last(),
            Some(&(138, "SUSTAIN_PROFILE_SLIDER_POSITION_8"))
        );
        assert_eq!(ACTIONS.last().unwrap().0, proto::ACTION_MAX);
        assert_eq!(ACTIONS.first().unwrap().0, proto::ACTION_MIN);
    }

    #[test]
    fn actions_have_short_names() {
        assert_eq!(action_name(5).as_deref(), Some("B1"));
        assert_eq!(action_name(1).as_deref(), Some("Up"));
        assert_eq!(action_name(54).as_deref(), Some("E12"));
        assert_eq!(action_name(-10).as_deref(), Some("none"));
        assert_eq!(action_name(0).as_deref(), Some("assigned-to-addon"));
        assert_eq!(action_name(32).as_deref(), Some("turbo"));
        assert_eq!(action_name(34).as_deref(), Some("macro-1"));
        assert_eq!(action_name(20).as_deref(), Some("ddi-up"));
        assert_eq!(action_name(139), None);
        for (v, _) in ACTIONS {
            let name = action_name(*v).unwrap();
            assert_eq!(action_from_name(&name), Some(*v), "{name}");
        }
        assert_eq!(action_from_name("BUTTON_PRESS_B1"), Some(5));
        assert_eq!(action_from_name("b1"), Some(5));
        assert_eq!(action_from_name("Macro_1"), Some(34));
        assert_eq!(action_from_name("sparkle"), None);
    }

    fn base() -> Slot {
        let mut pins = vec![
            SlotPin {
                action: -10,
                ..SlotPin::default()
            };
            30
        ];
        pins[0].action = 5;
        pins[1].action = 6;
        pins[2].action = -5;
        pins[3].action = 0;
        Slot {
            profile: 1,
            flags: Slot::ENABLED,
            label: "Default".into(),
            pins,
        }
    }

    #[test]
    fn a_file_plans_a_slot() {
        let file = SlotFile::parse(
            r#"{"label": "Hades", "pins": [
                {"gpio": 0, "action": "B2"},
                {"gpio": 1, "action": 5},
                {"gpio": 4, "action": "custom-button-combo", "button_mask": 3}
            ]}"#,
        )
        .unwrap();
        let write = SlotWrite {
            slot: 6,
            select: true,
            ..SlotWrite::default()
        };
        let s = plan(&file, &write, 30, 2, Some("Haute42COSMOX")).unwrap();
        assert_eq!(s.flags, Slot::ENABLED | Slot::ACTIVE);
        assert_eq!(s.label, "Hades");
        assert_eq!((s.pins[0].action, s.pins[1].action), (6, 5));
        assert_eq!((s.pins[4].action, s.pins[4].button_mask), (40, 3));
        assert_eq!(
            s.pins[29].action, FROM_PROFILE_1,
            "left out: profile 1's pin"
        );
        // The pad keeps profile 1's reserved and add-on pins.
        let mut sent = s.clone();
        sent.pins[2].action = 7;
        sent.pins[3] = SlotPin {
            action: 8,
            button_mask: 1,
            dpad_mask: 1,
        };
        let e = expected(&sent, &base());
        assert_eq!((e[2].action, e[3].action, e[3].button_mask), (-5, 0, 0));
        assert_eq!(changed_pins(&sent.pins, &e), [2, 3]);
        // --label wins, and --save sets the flag.
        let write = SlotWrite {
            slot: 5,
            save: true,
            label: Some("Hades II".into()),
            ..SlotWrite::default()
        };
        let s = plan(&file, &write, 30, 2, None).unwrap();
        assert_eq!(
            (s.flags, s.label.as_str()),
            (Slot::ENABLED | Slot::SAVE, "Hades II")
        );
    }

    #[test]
    fn a_bad_file_is_a_usage_error() {
        let p = |json: &str, slot| {
            let file = SlotFile::parse(json)?;
            plan(
                &file,
                &SlotWrite {
                    slot,
                    ..SlotWrite::default()
                },
                30,
                2,
                Some("Haute42COSMOX"),
            )
        };
        let msg = |r: Result<Slot>| r.unwrap_err().to_string();
        assert!(msg(p(r#"{"pins": [{"gpio": 30, "action": 5}]}"#, 6)).contains("GPIO 0 to 29"));
        assert!(
            msg(p(
                r#"{"pins": [{"gpio": 1, "action": 5}, {"gpio": 1, "action": 6}]}"#,
                6
            ))
            .contains("GPIO 1 is twice")
        );
        assert!(
            msg(p(r#"{"pins": [{"gpio": 1, "action": "sparkle"}]}"#, 6)).contains("unknown action")
        );
        assert!(msg(p(r#"{"pins": [{"gpio": 1, "action": 139}]}"#, 6)).contains("action 139"));
        assert!(msg(p(r#"{"pins": []}"#, 1)).contains("profiles 2 to 6"));
        assert!(msg(p(r#"{"board": "Pico", "pins": []}"#, 6)).contains("for board Pico"));
        assert!(msg(p(r#"{"label": "an 18-byte label!!", "pins": []}"#, 6)).contains("18 bytes"));
        assert!(msg(p(r#"{"enabled": false, "pins": []}"#, 2)).contains("the active profile"));
        assert!(msg(p(r#"{"pins": "all"}"#, 6)).contains("not a slot file"));
        let e = p(r#"{"pins": []}"#, 7).unwrap_err();
        assert_eq!(e.exit_code(), crate::error::exit::USAGE);
    }

    #[test]
    fn a_report_is_a_slot_file() {
        let mut slot = base();
        slot.profile = 3;
        slot.flags = Slot::ENABLED | Slot::ACTIVE;
        slot.label = "Celeste".into();
        slot.pins[1].action = 5;
        let r = SlotReport::new("node".into(), Some("Haute42COSMOX".into()), &slot, &base());
        assert_eq!(r.pins[0].key, Some(Button::B1));
        assert!(r.pins[2].from_profile_1 && r.pins[3].from_profile_1);
        assert!(!r.pins[0].from_profile_1);
        let json = serde_json::to_string(&r.file()).unwrap();
        let back = SlotFile::parse(&json).unwrap();
        assert_eq!(back, r.file());
        let again = plan(
            &back,
            &SlotWrite {
                slot: 3,
                ..SlotWrite::default()
            },
            30,
            3,
            Some("Haute42COSMOX"),
        )
        .unwrap();
        assert_eq!(again.pins, slot.pins);
        assert_eq!(again.label, "Celeste");
        // The report's own JSON reads as a file too.
        let report = serde_json::to_string(&r).unwrap();
        assert_eq!(SlotFile::parse(&report).unwrap().pins.len(), 30);
    }
}
