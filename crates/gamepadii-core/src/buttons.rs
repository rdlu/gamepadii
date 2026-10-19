//! GP2040-CE's logical buttons, their labels, and which physical key each
//! one is on.
//!
//! The hotkeys and boot buttons are logical (`S2`, `B3`, ...): the firmware
//! checks them after the active profile's pin mapping. On a remapped
//! profile, "hold S2+B3+B4" may mean other keys than the ones printed on
//! the case. Profile 1 (`Default`, the board's own mapping) is what the
//! printed labels follow, so [`locate`] finds the GPIO a logical button is
//! on in the active profile and names it by what that GPIO is in profile 1.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Serialize, Serializer};

use crate::api::{PinMappings, ProfileOptions};

macro_rules! buttons {
    ($($variant:ident => $action:literal, $name:literal, $xbox:literal, $ps:literal,
       $switch:literal;)*) => {
        /// A logical button, as `GpioAction::BUTTON_PRESS_*` names it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Button {
            $($variant,)*
        }

        impl Button {
            pub const ALL: &[Button] = &[$(Button::$variant,)*];

            /// The `GpioAction` value that presses it (`proto/enums.proto`).
            pub const fn action(self) -> i64 {
                match self {
                    $(Button::$variant => $action,)*
                }
            }

            /// GP2040-CE's name, as in the web configurator's "GP2040"
            /// labels (`www/src/Data/Buttons.js`): `B1`, `S2`, `Up`.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Button::$variant => $name,)*
                }
            }

            /// The XInput label from the same file (`A`, `Start`, `LB`), or
            /// the GP2040 name where Xbox has none.
            pub const fn xbox(self) -> &'static str {
                match self {
                    $(Button::$variant => $xbox,)*
                }
            }

            /// The PS4 label from the same file (`Cross`, `Options`), or
            /// the GP2040 name where PlayStation has none. The file calls
            /// A2 "Touchpad Center"; this table calls it `Touchpad`.
            pub const fn playstation(self) -> &'static str {
                match self {
                    $(Button::$variant => $ps,)*
                }
            }

            /// The Switch label from the same file (`B`, `Plus`, `ZL`), or
            /// the GP2040 name where Switch has none.
            pub const fn switch(self) -> &'static str {
                match self {
                    $(Button::$variant => $switch,)*
                }
            }
        }
    };
}

buttons! {
    Up => 1, "Up", "Up", "Up", "Up";
    Down => 2, "Down", "Down", "Down", "Down";
    Left => 3, "Left", "Left", "Left", "Left";
    Right => 4, "Right", "Right", "Right", "Right";
    B1 => 5, "B1", "A", "Cross", "B";
    B2 => 6, "B2", "B", "Circle", "A";
    B3 => 7, "B3", "X", "Square", "Y";
    B4 => 8, "B4", "Y", "Triangle", "X";
    L1 => 9, "L1", "LB", "L1", "L";
    R1 => 10, "R1", "RB", "R1", "R";
    L2 => 11, "L2", "LT", "L2", "ZL";
    R2 => 12, "R2", "RT", "R2", "ZR";
    S1 => 13, "S1", "Back", "Share", "Minus";
    S2 => 14, "S2", "Start", "Options", "Plus";
    A1 => 15, "A1", "Guide", "PS", "Home";
    A2 => 16, "A2", "A2", "Touchpad", "Capture";
    L3 => 17, "L3", "LS", "L3", "LS";
    R3 => 18, "R3", "RS", "R3", "RS";
    Fn => 19, "Fn", "Fn", "Fn", "Fn";
    A3 => 41, "A3", "A3", "A3", "A3";
    A4 => 42, "A4", "A4", "A4", "A4";
    E1 => 43, "E1", "E1", "E1", "E1";
    E2 => 44, "E2", "E2", "E2", "E2";
    E3 => 45, "E3", "E3", "E3", "E3";
    E4 => 46, "E4", "E4", "E4", "E4";
    E5 => 47, "E5", "E5", "E5", "E5";
    E6 => 48, "E6", "E6", "E6", "E6";
    E7 => 49, "E7", "E7", "E7", "E7";
    E8 => 50, "E8", "E8", "E8", "E8";
    E9 => 51, "E9", "E9", "E9", "E9";
    E10 => 52, "E10", "E10", "E10", "E10";
    E11 => 53, "E11", "E11", "E11", "E11";
    E12 => 54, "E12", "E12", "E12", "E12";
}

impl Button {
    pub fn from_action(action: i64) -> Option<Button> {
        Button::ALL.iter().copied().find(|b| b.action() == action)
    }

    /// A boot-mode button by its `inputModeXX` suffix (`B1`, `R2`, ...).
    pub fn from_name(name: &str) -> Option<Button> {
        Button::ALL.iter().copied().find(|b| b.name() == name)
    }

    /// `S2 (Start)`, or just `Up` when the labels agree.
    pub fn both(self) -> String {
        if self.name() == self.xbox() {
            self.name().to_string()
        } else {
            format!("{} ({})", self.name(), self.xbox())
        }
    }
}

impl fmt::Display for Button {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for Button {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// A name system: the names of the logical buttons on one console. A
/// library file names its actions in one system, because one word can
/// have two meanings: `A` is B1 for Xbox and B2 for Switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NameSystem {
    /// GP2040-CE's names: `B1`, `S2`.
    #[default]
    Gp2040,
    Xbox,
    Playstation,
    Switch,
}

/// More names for a button in a system: the names of newer consoles.
pub const MORE_NAMES: &[(NameSystem, &str, Button)] = &[
    (NameSystem::Xbox, "View", Button::S1),
    (NameSystem::Xbox, "Menu", Button::S2),
    (NameSystem::Playstation, "Create", Button::S1),
];

/// Lower case, letters and digits only: `Touch-pad` is `touchpad`.
fn normal(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl NameSystem {
    pub const ALL: &[NameSystem] = &[
        NameSystem::Gp2040,
        NameSystem::Xbox,
        NameSystem::Playstation,
        NameSystem::Switch,
    ];

    /// The name in a library file and on the command line.
    pub const fn key(self) -> &'static str {
        match self {
            NameSystem::Gp2040 => "gp2040",
            NameSystem::Xbox => "xbox",
            NameSystem::Playstation => "playstation",
            NameSystem::Switch => "switch",
        }
    }

    /// The name of `b` in this system; the GP2040 name where the system
    /// has none.
    pub const fn label(self, b: Button) -> &'static str {
        match self {
            NameSystem::Gp2040 => b.name(),
            NameSystem::Xbox => b.xbox(),
            NameSystem::Playstation => b.playstation(),
            NameSystem::Switch => b.switch(),
        }
    }

    /// The button that `name` names in this system (any case), by its
    /// label or by one of [`MORE_NAMES`]. GP2040 names are not in it.
    pub fn button(self, name: &str) -> Option<Button> {
        let want = normal(name);
        Button::ALL
            .iter()
            .copied()
            .find(|b| normal(self.label(*b)) == want)
            .or_else(|| {
                MORE_NAMES
                    .iter()
                    .find(|(s, n, _)| *s == self && normal(n) == want)
                    .map(|(_, _, b)| *b)
            })
    }

    /// The names of the system, for an error: `A (B1), B (B2), ...`, with
    /// the more names after the label: `Back/View (S1)`.
    pub fn names(self) -> String {
        Button::ALL
            .iter()
            .filter(|b| self == NameSystem::Gp2040 || self.label(**b) != b.name())
            .map(|b| {
                let mut n = self.label(*b).to_string();
                for (_, more, _) in MORE_NAMES.iter().filter(|(s, _, o)| *s == self && o == b) {
                    n.push('/');
                    n.push_str(more);
                }
                match self {
                    NameSystem::Gp2040 => n,
                    _ => format!("{n} ({})", b.name()),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for NameSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl std::str::FromStr for NameSystem {
    type Err = String;

    /// The key in any case; `ps` is `playstation`.
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.to_ascii_lowercase();
        if s == "ps" {
            return Ok(NameSystem::Playstation);
        }
        NameSystem::ALL
            .iter()
            .copied()
            .find(|n| n.key() == s)
            .ok_or_else(|| {
                format!("{s:?} is not a name system (gp2040, xbox, playstation, switch)")
            })
    }
}

/// `GpioAction` values that keep a pin out of profile switching.
const RESERVED: i64 = -5;
const ASSIGNED_TO_ADDON: i64 = 0;

/// Every pin-mapping profile on the pad: profile 1 is `getPinMappings`,
/// profiles 2.. are `getProfileOptions`' alternatives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Profiles {
    pub base: PinMappings,
    pub alternatives: Vec<PinMappings>,
}

impl Profiles {
    pub fn new(base: PinMappings, options: ProfileOptions) -> Self {
        Self {
            base,
            alternatives: options.alternative_pin_mappings,
        }
    }

    /// The label of profile `number` (from 1).
    pub fn label(&self, number: i64) -> Option<String> {
        let p = match number {
            1 => &self.base,
            n => self.alternatives.get(usize::try_from(n - 2).ok()?)?,
        };
        p.profile_label.clone()
    }

    /// GPIO → action as the firmware runs profile `number`
    /// (`Storage::setFunctionalPinMappings`): the profile's action for each
    /// pin, except where it or the base has the pin RESERVED or
    /// ASSIGNED_TO_ADDON; a profile that doesn't exist or isn't enabled
    /// means the base mapping.
    pub fn functional(&self, number: i64) -> BTreeMap<u8, i64> {
        let base: BTreeMap<u8, i64> = self
            .base
            .pins()
            .into_iter()
            .map(|(pin, m)| (pin, m.action))
            .collect();
        let alt = usize::try_from(number - 2)
            .ok()
            .and_then(|i| self.alternatives.get(i))
            .filter(|p| p.enabled != Some(false));
        let Some(alt) = alt else { return base };
        let alt = alt.pins();
        base.into_iter()
            .map(|(pin, base_action)| {
                let special = |a: i64| a == RESERVED || a == ASSIGNED_TO_ADDON;
                let action = match alt.get(&pin) {
                    Some(m) if !special(m.action) && !special(base_action) => m.action,
                    _ => base_action,
                };
                (pin, action)
            })
            .collect()
    }
}

/// One physical key: a GPIO and what profile 1 calls it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Key {
    pub gpio: u8,
    /// Profile 1's button on this GPIO; `null` when it has something else
    /// there (a menu or SOCD action, nothing).
    pub default: Option<Button>,
}

impl Key {
    /// `R1 (RB)`, or `GPIO 14` when profile 1 has no button there.
    pub fn describe(&self) -> String {
        match self.default {
            Some(b) => b.both(),
            None => format!("GPIO {}", self.gpio),
        }
    }
}

/// Where a logical button is, physically, in the active profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ButtonHint {
    pub button: Button,
    /// Its XInput label (`Start`).
    pub xbox: &'static str,
    /// Every key that presses it now; empty when no key does.
    pub keys: Vec<Key>,
    /// Whether the active profile moved it: some key isn't the one
    /// profile 1 calls by the same name.
    pub moved: bool,
}

/// Where `button` is in profile `active`.
pub fn locate(profiles: &Profiles, active: i64, button: Button) -> ButtonHint {
    let base: BTreeMap<u8, i64> = profiles
        .base
        .pins()
        .into_iter()
        .map(|(pin, m)| (pin, m.action))
        .collect();
    let keys: Vec<Key> = profiles
        .functional(active)
        .into_iter()
        .filter(|(_, action)| *action == button.action())
        .map(|(gpio, _)| Key {
            gpio,
            default: base.get(&gpio).copied().and_then(Button::from_action),
        })
        .collect();
    let moved = keys.iter().any(|k| k.default != Some(button));
    ButtonHint {
        button,
        xbox: button.xbox(),
        keys,
        moved,
    }
}

/// A combination to hold, located in the active profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Combo {
    /// The active profile's number and label.
    pub profile: i64,
    pub profile_label: Option<String>,
    pub buttons: Vec<ButtonHint>,
}

impl Combo {
    pub fn new(profiles: &Profiles, active: i64, buttons: &[Button]) -> Self {
        Self {
            profile: active,
            profile_label: profiles.label(active),
            buttons: buttons
                .iter()
                .map(|b| locate(profiles, active, *b))
                .collect(),
        }
    }

    /// `S2+B3+B4`.
    pub fn logical(&self) -> String {
        self.buttons
            .iter()
            .map(|h| h.button.name())
            .collect::<Vec<_>>()
            .join("+")
    }

    /// The buttons no key presses in this profile.
    pub fn unreachable(&self) -> Vec<Button> {
        self.buttons
            .iter()
            .filter(|h| h.keys.is_empty())
            .map(|h| h.button)
            .collect()
    }

    /// One sentence on which physical keys to press, or `None` when the
    /// profile doesn't move any of them:
    /// `in profile 2 (SilkSong), S2+B3+B4 are the keys Default calls S2
    /// (Start), B3 (X) and R1 (RB)`.
    pub fn physical(&self) -> Option<String> {
        if !self.buttons.iter().any(|h| h.moved) {
            return None;
        }
        let names: Vec<String> = self
            .buttons
            .iter()
            .map(|h| match h.keys.as_slice() {
                [] => format!("(no key for {})", h.button),
                keys => keys
                    .iter()
                    .map(Key::describe)
                    .collect::<Vec<_>>()
                    .join(" or "),
            })
            .collect();
        let profile = match &self.profile_label {
            Some(l) => format!("profile {} ({l})", self.profile),
            None => format!("profile {}", self.profile),
        };
        let what = if self.buttons.len() == 1 {
            format!("{} is the key", self.logical())
        } else {
            format!("{} are the keys", self.logical())
        };
        Some(format!(
            "in {profile}, {what} Default calls {}",
            and_list(&names)
        ))
    }
}

/// `a`, `a and b`, `a, b and c`.
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_match_the_proto() {
        assert_eq!(Button::B1.action(), 5);
        assert_eq!(Button::S2.action(), 14);
        assert_eq!(Button::R3.action(), 18);
        assert_eq!(Button::A3.action(), 41);
        assert_eq!(Button::E12.action(), 54);
        assert_eq!(Button::from_action(-10), None);
        assert_eq!(Button::from_action(78), None);
        for b in Button::ALL {
            assert_eq!(Button::from_action(b.action()), Some(*b));
            assert_eq!(Button::from_name(b.name()), Some(*b));
        }
    }

    #[test]
    fn xbox_labels() {
        let labels: Vec<_> = [
            Button::B1,
            Button::B2,
            Button::B3,
            Button::B4,
            Button::L1,
            Button::R1,
            Button::L2,
            Button::R2,
            Button::S1,
            Button::S2,
            Button::L3,
            Button::R3,
            Button::A1,
        ]
        .iter()
        .map(|b| b.xbox())
        .collect();
        assert_eq!(
            labels,
            [
                "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "Back", "Start", "LS", "RS", "Guide"
            ]
        );
        assert_eq!(Button::S2.both(), "S2 (Start)");
        assert_eq!(Button::Up.both(), "Up");
    }

    #[test]
    fn name_systems() {
        use NameSystem::*;
        assert_eq!(Xbox.button("a"), Some(Button::B1));
        assert_eq!(Switch.button("A"), Some(Button::B2));
        assert_eq!(Playstation.button("cross"), Some(Button::B1));
        assert_eq!(Playstation.button("Touch-pad"), Some(Button::A2));
        assert_eq!(Switch.button("capture"), Some(Button::A2));
        assert_eq!(Xbox.button("view"), Some(Button::S1));
        assert_eq!(Xbox.button("Menu"), Some(Button::S2));
        assert_eq!(Playstation.button("create"), Some(Button::S1));
        assert_eq!(Switch.button("ZR"), Some(Button::R2));
        assert_eq!(Gp2040.button("a"), None);
        assert_eq!(Switch.button("View"), None, "an Xbox name");
        // Xbox has no name for A2.
        assert_eq!(Xbox.label(Button::A2), "A2");
        assert_eq!("PS".parse(), Ok(Playstation));
        assert_eq!("Switch".parse(), Ok(Switch));
        assert!("snes".parse::<NameSystem>().is_err());
        assert!(Xbox.names().starts_with("A (B1), B (B2), X (B3)"));
        assert!(Xbox.names().contains("Back/View (S1)"));
        assert!(Gp2040.names().starts_with("Up, Down, Left, Right, B1"));
    }

    #[test]
    fn lists() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(and_list(&s(&["a"])), "a");
        assert_eq!(and_list(&s(&["a", "b"])), "a and b");
        assert_eq!(and_list(&s(&["a", "b", "c"])), "a, b and c");
    }
}
