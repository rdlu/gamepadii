//! Typed views of the endpoints the client reasons about, in the firmware's
//! own JSON shape (camelCase, both ways). Fields the firmware may not have
//! are `Option`s that are left out again when absent, and every struct
//! keeps the fields it doesn't name in `extra`, so a read → write round
//! trip loses nothing: `setGamepadOptions` zeroes whatever it isn't sent.
//!
//! These are wire types; the commands turn them into snake_case output.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `getFirmwareVersion`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareVersion {
    /// `v0.7.12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `rp2040` or `rp2350`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_architecture: Option<String>,
    /// The git commit the build came from: `0014e4a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_build: Option<String>,
    /// `Release` or `Debug`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_build_type: Option<String>,
    /// For people: `Haute42 COSMOX`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_config_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_config_file_name: Option<String>,
    /// The `configs/` directory the build used: `Haute42COSMOX`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_config: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `getMemoryReport`, in bytes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_flash: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_flash: Option<u64>,
    /// The flash chip's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_flash: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_allocs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_heap: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_heap: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `getGamepadOptions`. Only the fields gamepadii reads are named; the
/// hotkeys, USB descriptor overrides, auth types and the rest stay in
/// `extra`, untouched.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GamepadOptions {
    /// The persisted input mode: the protobuf `InputMode` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpad_mode: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socd_mode: Option<i64>,
    /// The active pin-mapping profile, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_number: Option<i64>,
    /// The boot-mode button map: hold the button while plugging in to boot
    /// into this `InputMode` value (`-1`: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_b1: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_b2: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_b3: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_b4: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_l1: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_l2: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_r1: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode_r2: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GamepadOptions {
    /// The boot-mode buttons in the firmware's order, with their raw values.
    pub fn boot_buttons(&self) -> [(&'static str, Option<i64>); 8] {
        [
            ("B1", self.input_mode_b1),
            ("B2", self.input_mode_b2),
            ("B3", self.input_mode_b3),
            ("B4", self.input_mode_b4),
            ("L1", self.input_mode_l1),
            ("L2", self.input_mode_l2),
            ("R1", self.input_mode_r1),
            ("R2", self.input_mode_r2),
        ]
    }
}

/// `getDisplayOptions`. Only the OLED button layout is named; the splash,
/// the screen saver, the status-bar flags, the custom layout parameters and
/// the rest stay in `extra`, untouched (`setDisplayOptions` zeroes whatever
/// it isn't sent, like `setGamepadOptions`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayOptions {
    /// The left half of the drawing: the protobuf `ButtonLayout` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_layout: Option<i64>,
    /// The right half: the protobuf `ButtonLayoutRight` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_layout_right: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One GPIO's mapping.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinMapping {
    /// The `GpioAction` value (`-10`: none, `0`: button Up, ...).
    pub action: i64,
    #[serde(default)]
    pub custom_button_mask: u64,
    #[serde(default)]
    pub custom_dpad_mask: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A pin-mapping profile: `getPinMappings` (profile 1) or one entry of
/// `getProfileOptions` (profiles 2 and up). The pins are `pin00`..`pin29`
/// keys beside the label, kept in `extra` so unknown keys survive; read
/// them with [`PinMappings::pins`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinMappings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PinMappings {
    /// GPIO number → mapping, for every `pinNN` key that parses.
    pub fn pins(&self) -> BTreeMap<u8, PinMapping> {
        self.extra
            .iter()
            .filter_map(|(k, v)| {
                let n = k.strip_prefix("pin")?.parse().ok()?;
                Some((n, serde_json::from_value(v.clone()).ok()?))
            })
            .collect()
    }
}

/// `getProfileOptions`: the profiles after the base one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOptions {
    #[serde(default)]
    pub alternative_pin_mappings: Vec<PinMappings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `getKeyMappings`: logical button (`Up`, `B1`, `E12`, ...) → HID keycode
/// (`0`: none), for keyboard mode. There is one keymap, not one per profile.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyMappings(pub BTreeMap<String, u16>);
