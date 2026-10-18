//! The pad's slots (READ_SLOT) → library profiles.
//!
//! Profile 1 becomes a profile with every button of the board. Each other
//! slot becomes a profile with profile 1 as its base, with only the
//! buttons that it changes. A slot pin that follows profile 1 (0 or -5)
//! is not written: the base gives it.

use std::collections::BTreeSet;

use serde::Serialize;

use super::board::Board;
use super::{Pin, Profile, write};
use crate::channel::{Slot, SlotPin};
use crate::slots::{action_name, follows_profile_1};

/// A pin of a slot that is not on a button of the board file, with an
/// action that the pull cannot keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub profile: u8,
    pub gpio: u8,
    pub action: i16,
    pub action_name: Option<String>,
}

/// The profiles of `slots` (profile 1 first, then the others in order),
/// and the pins that the board file has no name for.
pub fn profiles(board: &Board, slots: &[Slot]) -> (Vec<Profile>, Vec<Skipped>) {
    let none = Slot::default();
    let base = slots.iter().find(|s| s.profile == 1).unwrap_or(&none);
    let mut names = BTreeSet::new();
    let mut name_of = |s: &Slot| {
        let mut name = match s.label.trim() {
            "" => format!("Profile {}", s.profile),
            l => l.to_string(),
        };
        if !names.insert(name.to_ascii_lowercase()) {
            name = format!("{name} ({})", s.profile);
            names.insert(name.to_ascii_lowercase());
        }
        name
    };
    let base_name = name_of(base);
    let pin_of = |p: &SlotPin| Pin {
        action: p.action,
        button_mask: p.button_mask,
        dpad_mask: p.dpad_mask,
    };
    // A slot with no label gets its name ("Profile 4") as the label.
    let label = |name: &str, s: &Slot| {
        (!s.label.trim().is_empty() && s.label != name).then(|| s.label.clone())
    };
    let mut skipped = Vec::new();
    let mut first = Profile {
        name: base_name.clone(),
        board: Some(board.name.clone()),
        label: label(&base_name, base),
        ..Profile::default()
    };
    for b in &board.buttons {
        if let Some(p) = base.pins.get(usize::from(b.gpio)) {
            first.pins.push((b.name.clone(), pin_of(p)));
        }
    }
    for (gpio, p) in base.pins.iter().zip(0u8..).map(|(p, g)| (g, p)) {
        if board.by_gpio(gpio).is_none() && !matches!(p.action, -10 | -5 | 0) {
            skipped.push(skip(1, gpio, p));
        }
    }
    let mut out = vec![first];
    for s in slots.iter().filter(|s| s.profile != 1) {
        let name = name_of(s);
        let mut p = Profile {
            name: name.clone(),
            base: Some(base_name.clone()),
            label: label(&name, s),
            ..Profile::default()
        };
        for (gpio, pin) in s.pins.iter().zip(0u8..).map(|(p, g)| (g, p)) {
            let core = base
                .pins
                .get(usize::from(gpio))
                .copied()
                .unwrap_or_default();
            if follows_profile_1(pin.action) || follows_profile_1(core.action) || *pin == core {
                continue;
            }
            match board.by_gpio(gpio) {
                Some(b) => p.pins.push((b.name.clone(), pin_of(pin))),
                None => skipped.push(skip(s.profile, gpio, pin)),
            }
        }
        // In the order of the board file.
        p.pins
            .sort_by_key(|(b, _)| board.buttons.iter().position(|o| &o.name == b));
        out.push(p);
    }
    (out, skipped)
}

fn skip(profile: u8, gpio: u8, p: &SlotPin) -> Skipped {
    Skipped {
        profile,
        gpio,
        action: p.action,
        action_name: action_name(p.action),
    }
}

/// The KDL text of a pull.
pub fn text(board: &Board, profiles: &[Profile], skipped: &[Skipped]) -> String {
    let mut header = vec![
        "The profiles of the pad, from `gamepadii library pull`.".to_string(),
        format!(
            "Board: {} (firmware board {}).",
            board.name, board.firmware_board
        ),
    ];
    for s in skipped {
        header.push(format!(
            "Not pulled: profile {}, GPIO {} has {}; the board file has no button there.",
            s.profile,
            s.gpio,
            s.action_name
                .clone()
                .unwrap_or_else(|| s.action.to_string())
        ));
    }
    write::file(&header, profiles)
}
