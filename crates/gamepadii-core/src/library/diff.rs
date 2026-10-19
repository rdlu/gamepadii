//! `library diff` and `library apply`: a library profile next to a slot
//! of the pad.
//!
//! The diff compares what READ_SLOT gives now with what the pad keeps
//! after the WRITE_SLOT of the profile's slot file ([`slots::expected`]).
//! A pin can change its stored value but not its action: 0 ("the pin of
//! profile 1") in place of the same action as profile 1. Such a pin is
//! `same_action`. Only the other changes make `apply` write.

use serde::Serialize;

use super::Resolved;
use crate::channel::{Slot, SlotPin};
use crate::commands::Route;
use crate::mode::InputMode;
use crate::slots::{self, action_name, follows_profile_1};

/// A value now, and after `apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change<T> {
    pub now: T,
    pub after: T,
}

impl<T: PartialEq> Change<T> {
    pub fn changed(&self) -> bool {
        self.now != self.after
    }
}

/// One pin of a slot, now or after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinState {
    /// The `GpioAction` in the slot.
    pub action: i16,
    pub action_name: Option<String>,
    pub button_mask: u32,
    pub dpad_mask: u32,
    /// The pad uses the pin of profile 1 here.
    pub from_profile_1: bool,
    /// The action that the pad uses: the action of profile 1 when
    /// `from_profile_1`.
    pub pad_action: i16,
    pub pad_action_name: Option<String>,
}

/// A pin that `apply` changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinChange {
    pub gpio: u8,
    /// The button of the board file; `null` for a GPIO with no button.
    pub button: Option<String>,
    pub now: PinState,
    pub after: PinState,
    /// The stored value changes, but the pad uses the same action.
    pub same_action: bool,
}

/// `gamepadii library diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffReport {
    /// Always `control`: READ_SLOT.
    pub via: Route,
    pub host: String,
    /// The name of the library profile.
    pub name: String,
    pub board: String,
    pub firmware_board: String,
    /// The profile of the pad, 2–6.
    pub slot: u8,
    /// No `--slot`: the slot has the label of the profile.
    pub slot_by_label: bool,
    /// The slot is the active profile now.
    pub active: bool,
    pub label: Change<String>,
    pub enabled: Change<bool>,
    /// The pins that change, in GPIO order.
    pub pins: Vec<PinChange>,
    /// `apply` writes: the label, the state or the action of a pin
    /// changes. A `same_action` pin alone does not count.
    pub changed: bool,
    /// The input mode that the profile is for; `null`: any.
    pub input_mode: Option<InputMode>,
    /// The input mode that the pad runs; `null` when it does not say.
    pub pad_input_mode: Option<InputMode>,
    pub note: Option<String>,
}

fn state(pin: &SlotPin, core: &SlotPin, slot: u8) -> PinState {
    let from_profile_1 =
        slot != 1 && (follows_profile_1(pin.action) || follows_profile_1(core.action));
    let used = if from_profile_1 { core } else { pin };
    PinState {
        action: pin.action,
        action_name: action_name(pin.action),
        button_mask: pin.button_mask,
        dpad_mask: pin.dpad_mask,
        from_profile_1,
        pad_action: used.action,
        pad_action_name: action_name(used.action),
    }
}

fn used(s: &PinState, pin: &SlotPin, core: &SlotPin) -> SlotPin {
    if s.from_profile_1 { *core } else { *pin }
}

/// The changes from `before` (READ_SLOT now) to the WRITE_SLOT `planned`
/// of `profile`, next to profile 1 (`base`).
pub fn compare(
    profile: &Resolved,
    planned: &Slot,
    before: &Slot,
    base: &Slot,
) -> (Change<String>, Change<bool>, Vec<PinChange>) {
    let after = slots::expected(planned, base);
    let none = SlotPin::default();
    let mut pins = Vec::new();
    for gpio in slots::changed_pins(&before.pins, &after) {
        let at = usize::from(gpio);
        let core = base.pins.get(at).unwrap_or(&none);
        let was = before.pins.get(at).unwrap_or(&none);
        let will = after.get(at).unwrap_or(&none);
        let now = state(was, core, before.profile);
        let then = state(will, core, before.profile);
        let same_action = used(&now, was, core) == used(&then, will, core);
        pins.push(PinChange {
            gpio,
            button: profile
                .pins
                .iter()
                .find(|p| p.gpio == gpio)
                .map(|p| p.button.clone()),
            now,
            after: then,
            same_action,
        });
    }
    (
        Change {
            now: before.label.clone(),
            after: planned.label.clone(),
        },
        Change {
            now: before.enabled(),
            after: planned.enabled(),
        },
        pins,
    )
}

/// `gamepadii library apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    /// What the slot had, and what the profile writes.
    pub diff: DiffReport,
    /// gamepadii wrote the slot. `false` when the slot has the profile
    /// already.
    pub written: bool,
    /// The write; `null` when nothing was written.
    pub write: Option<slots::SlotWriteReport>,
    /// The theme of the profile that went to the LEDs; `null` with no
    /// theme, or with `--no-lights`.
    pub theme: Option<String>,
    /// The apply of the theme; `null` when there is no theme.
    pub lights: Option<crate::colors::ThemeApplyReport>,
}

/// What `library apply` asks for, besides the profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Apply {
    /// The profile of the pad to write, 2–6; `None`: the enabled profile
    /// with the label of the library profile.
    pub slot: Option<u8>,
    /// Make it the active profile now.
    pub select: bool,
    /// Save to flash.
    pub save: bool,
}
