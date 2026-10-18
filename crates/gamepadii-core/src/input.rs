//! The input snapshot over the control channel (v1.7, §3.12 of
//! `docs/design/control-channel.md`): READ_INPUT and PIN_SCAN.
//!
//! `gamepadii input` reads one snapshot: the GPIOs pressed now, each with
//! its action in the active profile and the key printed on the case, and
//! the gamepad state (buttons, D-pad, sticks, triggers). `gamepadii input
//! find-pin` reads again and again until a person pushes a button, and
//! tells which GPIO it is on. The pins in a snapshot are raw, so the host
//! does the debounce ([`Finder`]).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::buttons::{Button, Profiles};
use crate::channel::{InputSnapshot, Slot};
use crate::commands::Route;
use crate::slots::{action_name, follows_profile_1};

/// `GpioAction::NONE`.
pub const NONE: i16 = -10;

/// How many reads one after the other must show a new press before
/// `find-pin` accepts it, and a release before it stops.
pub const DEBOUNCE_READS: u32 = 3;

/// The names of the `GamepadState.buttons` bits (`GAMEPAD_MASK_*`), from
/// bit 0. Bits 16 to 19 are the DDI directions.
pub const BUTTON_BITS: [&str; 32] = [
    "B1",
    "B2",
    "B3",
    "B4",
    "L1",
    "R1",
    "L2",
    "R2",
    "S1",
    "S2",
    "L3",
    "R3",
    "A1",
    "A2",
    "A3",
    "A4",
    "ddi-up",
    "ddi-down",
    "ddi-left",
    "ddi-right",
    "E1",
    "E2",
    "E3",
    "E4",
    "E5",
    "E6",
    "E7",
    "E8",
    "E9",
    "E10",
    "E11",
    "E12",
];

/// The names of the `GamepadState.dpad` bits, from bit 0.
pub const DPAD_BITS: [&str; 4] = ["Up", "Down", "Left", "Right"];

/// `AUX_MASK_FUNCTION`: the Fn button, in `GamepadState.aux`.
pub const AUX_FUNCTION: u16 = 1 << 15;

/// The `(dpad, buttons, aux)` bits that a pin with `action` sets in the
/// gamepad state when it is pressed (with no SOCD or other processing).
pub fn state_bits(action: i16) -> (u8, u32, u16) {
    match action {
        1..=4 => (1 << (action - 1), 0, 0),
        // B1 to S2, then A1, A2, L3, R3: L3 and R3 come first in the mask.
        5..=14 => (0, 1 << (action - 5), 0),
        15 | 16 => (0, 1 << (action - 3), 0),
        17 | 18 => (0, 1 << (action - 7), 0),
        19 => (0, 0, AUX_FUNCTION),
        20..=23 => (0, 1 << (action - 4), 0),
        41 | 42 => (0, 1 << (action - 27), 0),
        43..=54 => (0, 1 << (action - 23), 0),
        _ => (0, 0, 0),
    }
}

/// The names of the bits set in `GamepadState.buttons`, and Fn from `aux`.
pub fn button_names(buttons: u32, aux: u16) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = BUTTON_BITS
        .iter()
        .enumerate()
        .filter(|(i, _)| buttons & (1 << i) != 0)
        .map(|(_, n)| *n)
        .collect();
    if aux & AUX_FUNCTION != 0 {
        names.push("Fn");
    }
    names
}

/// The names of the bits set in `GamepadState.dpad`.
pub fn dpad_names(dpad: u8) -> Vec<&'static str> {
    DPAD_BITS
        .iter()
        .enumerate()
        .filter(|(i, _)| dpad & (1 << i) != 0)
        .map(|(_, n)| *n)
        .collect()
}

/// Where the names of the pins come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NamesFrom {
    /// READ_SLOT of the active profile and of profile 1, live (v1.4).
    Slot,
    /// The device cache: the pin mappings of the last web-config read.
    Cache,
    /// Nothing: only the GPIO numbers.
    None,
}

/// The action of each GPIO in the active profile, and the key printed on
/// the case (profile 1's button there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinNames {
    /// The active profile, from STATUS; `None` when it did not answer.
    pub profile: Option<u8>,
    pub from: NamesFrom,
    actions: BTreeMap<u8, i16>,
    keys: BTreeMap<u8, Button>,
}

impl PinNames {
    /// No names: only the GPIO numbers.
    pub fn none(profile: Option<u8>) -> Self {
        Self {
            profile,
            from: NamesFrom::None,
            actions: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }

    /// From READ_SLOT of the active profile (`active`) and of profile 1
    /// (`base`), as the firmware runs them.
    pub fn from_slots(active: &Slot, base: &Slot) -> Self {
        let mut actions = BTreeMap::new();
        let mut keys = BTreeMap::new();
        for (gpio, b) in (0u8..).zip(&base.pins) {
            let a = active
                .pins
                .get(usize::from(gpio))
                .map_or(b.action, |p| p.action);
            let action =
                if active.profile == 1 || follows_profile_1(a) || follows_profile_1(b.action) {
                    b.action
                } else {
                    a
                };
            actions.insert(gpio, action);
            if let Some(k) = Button::from_action(b.action.into()) {
                keys.insert(gpio, k);
            }
        }
        Self {
            profile: Some(active.profile),
            from: NamesFrom::Slot,
            actions,
            keys,
        }
    }

    /// From the pin mappings of the device cache, with `active` as the
    /// active profile.
    pub fn from_profiles(profiles: &Profiles, active: u8) -> Self {
        let actions = profiles
            .functional(active.into())
            .into_iter()
            .filter_map(|(gpio, a)| i16::try_from(a).ok().map(|a| (gpio, a)))
            .collect();
        let keys = profiles
            .base
            .pins()
            .into_iter()
            .filter_map(|(gpio, m)| Button::from_action(m.action).map(|b| (gpio, b)))
            .collect();
        Self {
            profile: Some(active),
            from: NamesFrom::Cache,
            actions,
            keys,
        }
    }

    pub fn pin(&self, gpio: u8) -> PinInfo {
        let action = self.actions.get(&gpio).copied();
        PinInfo {
            gpio,
            action,
            action_name: action.and_then(action_name),
            key: self.keys.get(&gpio).copied(),
        }
    }
}

/// One GPIO, with what the active profile does with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinInfo {
    pub gpio: u8,
    /// The `GpioAction` in the active profile; `null` when gamepadii does
    /// not know the mapping.
    pub action: Option<i16>,
    /// [`action_name`]: `B1`, `Up`, `none`, `menu-navigation-toggle`.
    pub action_name: Option<String>,
    /// Profile 1's button on this GPIO, the name printed on the case;
    /// `null` when it has something else there.
    pub key: Option<Button>,
}

/// The analog values of the gamepad state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sticks {
    /// 0 to 65535; the centre is 32767.
    pub lx: u16,
    pub ly: u16,
    pub rx: u16,
    pub ry: u16,
    /// 0 to 255.
    pub lt: u8,
    pub rt: u8,
}

impl Sticks {
    /// The sticks at the centre and the triggers released.
    pub fn at_rest(&self) -> bool {
        let c = crate::channel::proto::AXIS_CENTRE;
        [self.lx, self.ly, self.rx, self.ry] == [c; 4] && self.lt == 0 && self.rt == 0
    }
}

/// `gamepadii input`: one READ_INPUT snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// The active profile; `null` when STATUS did not answer.
    pub profile: Option<u8>,
    /// Where the action and key names come from.
    pub names_from: NamesFrom,
    /// The GPIOs pressed now (raw, before the debounce).
    pub pressed: Vec<PinInfo>,
    /// The GPIOs the snapshot can read: a pin that is not here shows as
    /// not pressed.
    pub readable: Vec<u8>,
    /// The pin scan is on (`input find-pin --scan`).
    pub scan_on: bool,
    /// The gamepad state, after the active profile, SOCD and the other
    /// processing: `Up`, `Down`, `Left`, `Right`.
    pub dpad: Vec<&'static str>,
    /// [`BUTTON_BITS`], and `Fn`.
    pub buttons: Vec<&'static str>,
    pub buttons_mask: u32,
    pub aux: u16,
    pub sticks: Sticks,
}

impl InputReport {
    pub fn new(host: String, names: &PinNames, snap: &InputSnapshot) -> Self {
        Self {
            via: Route::Control,
            host,
            profile: names.profile,
            names_from: names.from,
            pressed: snap
                .pressed_gpios()
                .into_iter()
                .map(|g| names.pin(g))
                .collect(),
            readable: snap.readable_gpios(),
            scan_on: snap.scan_on(),
            dpad: dpad_names(snap.dpad),
            buttons: button_names(snap.buttons, snap.aux),
            buttons_mask: snap.buttons,
            aux: snap.aux,
            sticks: Sticks {
                lx: snap.lx,
                ly: snap.ly,
                rx: snap.rx,
                ry: snap.ry,
                lt: snap.lt,
                rt: snap.rt,
            },
        }
    }
}

/// `gamepadii input find-pin`: the GPIO of the button that was pushed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundPin {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    pub profile: Option<u8>,
    pub names_from: NamesFrom,
    #[serde(flatten)]
    pub pin: PinInfo,
    /// The pin has no action in the active profile (a pin that only the
    /// scan can read); `null` when gamepadii does not know the mapping.
    pub no_action: Option<bool>,
    /// The pin scan was on.
    pub scan: bool,
    /// The button was released before gamepadii stopped.
    pub released: bool,
    /// Something to know: for example, that the pin scan did not stop.
    pub note: Option<String>,
}

impl FoundPin {
    pub fn new(host: String, names: &PinNames, gpio: u8, scan: bool, released: bool) -> Self {
        let pin = names.pin(gpio);
        Self {
            via: Route::Control,
            host,
            profile: names.profile,
            names_from: names.from,
            no_action: pin.action.map(|a| a == NONE),
            pin,
            scan,
            released,
            note: None,
        }
    }
}

/// The debounce of "push the button to find its pin". Each read gives the
/// pressed mask. A pin that is pressed now, and was not in the read
/// before, is a new press: [`Finder::feed`] accepts it when
/// [`DEBOUNCE_READS`] reads one after the other show it. A pin that is
/// held at the first read is not a new press.
#[derive(Debug, Clone, Default)]
pub struct Finder {
    last: Option<u64>,
    candidate: Option<(u8, u32)>,
}

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One read's pressed mask; the GPIO when a new press is accepted.
    pub fn feed(&mut self, pressed: u64) -> Option<u8> {
        let before = self.last.replace(pressed);
        // A candidate that is not pressed now was a bounce.
        if let Some((gpio, n)) = self.candidate.take()
            && pressed & (1 << gpio) != 0
        {
            if n + 1 >= DEBOUNCE_READS {
                return Some(gpio);
            }
            self.candidate = Some((gpio, n + 1));
            return None;
        }
        let new = pressed & !before?;
        if new == 0 {
            return None;
        }
        let gpio = new.trailing_zeros() as u8;
        if DEBOUNCE_READS <= 1 {
            return Some(gpio);
        }
        self.candidate = Some((gpio, 1));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_bits_match_gamepad_state() {
        assert_eq!(state_bits(1), (1, 0, 0), "Up");
        assert_eq!(state_bits(4), (8, 0, 0), "Right");
        assert_eq!(state_bits(5), (0, 1, 0), "B1");
        assert_eq!(state_bits(12), (0, 1 << 7, 0), "R2");
        assert_eq!(state_bits(13), (0, 1 << 8, 0), "S1");
        assert_eq!(state_bits(14), (0, 1 << 9, 0), "S2");
        assert_eq!(state_bits(15), (0, 1 << 12, 0), "A1");
        assert_eq!(state_bits(16), (0, 1 << 13, 0), "A2");
        assert_eq!(state_bits(17), (0, 1 << 10, 0), "L3");
        assert_eq!(state_bits(18), (0, 1 << 11, 0), "R3");
        assert_eq!(state_bits(19), (0, 0, 1 << 15), "Fn");
        assert_eq!(state_bits(20), (0, 1 << 16, 0), "DDI up");
        assert_eq!(state_bits(41), (0, 1 << 14, 0), "A3");
        assert_eq!(state_bits(42), (0, 1 << 15, 0), "A4");
        assert_eq!(state_bits(43), (0, 1 << 20, 0), "E1");
        assert_eq!(state_bits(54), (0, 1 << 31, 0), "E12");
        assert_eq!(state_bits(78), (0, 0, 0), "menu toggle");
        assert_eq!(state_bits(NONE), (0, 0, 0));
        // Each button's bit has its name.
        for b in Button::ALL {
            let (_, mask, _) = state_bits(b.action() as i16);
            if mask != 0 {
                assert_eq!(button_names(mask, 0), [b.name()], "{b}");
            }
        }
        assert_eq!(button_names(0, AUX_FUNCTION), ["Fn"]);
        assert_eq!(dpad_names(0b1010), ["Down", "Right"]);
    }

    #[test]
    fn a_press_counts_after_the_debounce() {
        let mut f = Finder::new();
        let pin = 1u64 << 27;
        assert_eq!(f.feed(0), None);
        assert_eq!(f.feed(pin), None);
        assert_eq!(f.feed(pin), None);
        assert_eq!(f.feed(pin), Some(27));
    }

    #[test]
    fn a_bounce_and_a_held_pin_do_not_count() {
        let mut f = Finder::new();
        let (held, pin) = (1u64 << 3, 1u64 << 15);
        // Held at the start: not a new press while it stays held.
        for _ in 0..5 {
            assert_eq!(f.feed(held), None);
        }
        // One read only: a bounce.
        assert_eq!(f.feed(held | pin), None);
        assert_eq!(f.feed(held), None);
        assert_eq!(f.feed(held | pin), None);
        assert_eq!(f.feed(held | pin), None);
        assert_eq!(f.feed(held | pin), Some(15));
        // A held pin that is released and pushed again counts.
        let mut f = Finder::new();
        f.feed(held);
        f.feed(0);
        f.feed(held);
        f.feed(held);
        assert_eq!(f.feed(held), Some(3));
    }
}
