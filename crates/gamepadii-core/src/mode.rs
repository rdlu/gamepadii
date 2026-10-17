//! GP2040-CE input modes and the USB ids each one enumerates with, and the
//! boot modes the web-config reboot endpoint takes.
//!
//! [`InputMode`] mirrors `InputMode` in GP2040-CE's `proto/enums.proto`
//! (`INPUT_MODE_CONFIG = 255` is [`InputMode::Webconfig`]) plus BOOTSEL,
//! which is the RP2040 boot ROM rather than a firmware mode. [`KNOWN`] maps
//! each VID:PID the firmware's descriptors use (`headers/drivers/*/`,
//! `src/drivers/*/`, checked at v0.7.12 and main 8013b41) to the modes that
//! produce it.
//!
//! Most of those ids are borrowed from real controllers, so a match is only
//! [`Confidence::Likely`] unless the id or the USB strings can only come from
//! GP2040-CE.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Xinput,
    Switch,
    Ps3,
    Keyboard,
    Ps4,
    Xbone,
    Mdmini,
    Neogeo,
    Pcemini,
    Egret,
    Astro,
    Psclassic,
    Xboxoriginal,
    Ps5,
    /// Generic HID (`INPUT_MODE_GENERIC`; "HID" in the web configurator).
    Generic,
    SwitchPro,
    P5general,
    Sinput,
    Switch2Pro,
    /// The web configurator (`INPUT_MODE_CONFIG`): a USB network device.
    Webconfig,
    /// The RP2040 / RP2350 boot ROM (PICOBOOT + mass storage), not a
    /// firmware mode.
    Bootsel,
}

impl InputMode {
    pub const ALL: [InputMode; 21] = [
        InputMode::Xinput,
        InputMode::Switch,
        InputMode::Ps3,
        InputMode::Keyboard,
        InputMode::Ps4,
        InputMode::Xbone,
        InputMode::Mdmini,
        InputMode::Neogeo,
        InputMode::Pcemini,
        InputMode::Egret,
        InputMode::Astro,
        InputMode::Psclassic,
        InputMode::Xboxoriginal,
        InputMode::Ps5,
        InputMode::Generic,
        InputMode::SwitchPro,
        InputMode::P5general,
        InputMode::Sinput,
        InputMode::Switch2Pro,
        InputMode::Webconfig,
        InputMode::Bootsel,
    ];

    /// The `InputMode` value in the firmware's protobuf config; `None` for
    /// BOOTSEL, which has none.
    pub fn proto_value(self) -> Option<u32> {
        Some(match self {
            InputMode::Xinput => 0,
            InputMode::Switch => 1,
            InputMode::Ps3 => 2,
            InputMode::Keyboard => 3,
            InputMode::Ps4 => 4,
            InputMode::Xbone => 5,
            InputMode::Mdmini => 6,
            InputMode::Neogeo => 7,
            InputMode::Pcemini => 8,
            InputMode::Egret => 9,
            InputMode::Astro => 10,
            InputMode::Psclassic => 11,
            InputMode::Xboxoriginal => 12,
            InputMode::Ps5 => 13,
            InputMode::Generic => 14,
            InputMode::SwitchPro => 15,
            InputMode::P5general => 16,
            InputMode::Sinput => 17,
            InputMode::Switch2Pro => 19,
            InputMode::Webconfig => 255,
            InputMode::Bootsel => return None,
        })
    }

    /// The mode for a protobuf `InputMode` value (18 is unassigned).
    pub fn from_proto(value: u32) -> Option<InputMode> {
        InputMode::ALL
            .into_iter()
            .find(|m| m.proto_value() == Some(value))
    }

    /// The stable key: the JSON value, the waybar class and the CLI name.
    pub fn key(self) -> &'static str {
        match self {
            InputMode::Xinput => "xinput",
            InputMode::Switch => "switch",
            InputMode::Ps3 => "ps3",
            InputMode::Keyboard => "keyboard",
            InputMode::Ps4 => "ps4",
            InputMode::Xbone => "xbone",
            InputMode::Mdmini => "mdmini",
            InputMode::Neogeo => "neogeo",
            InputMode::Pcemini => "pcemini",
            InputMode::Egret => "egret",
            InputMode::Astro => "astro",
            InputMode::Psclassic => "psclassic",
            InputMode::Xboxoriginal => "xboxoriginal",
            InputMode::Ps5 => "ps5",
            InputMode::Generic => "generic",
            InputMode::SwitchPro => "switch_pro",
            InputMode::P5general => "p5general",
            InputMode::Sinput => "sinput",
            InputMode::Switch2Pro => "switch2_pro",
            InputMode::Webconfig => "webconfig",
            InputMode::Bootsel => "bootsel",
        }
    }

    /// A human name, worded as in the web configurator (`www/src/Locales/en`).
    pub fn label(self) -> &'static str {
        match self {
            InputMode::Xinput => "XInput",
            InputMode::Switch => "Nintendo Switch",
            InputMode::Ps3 => "PS3",
            InputMode::Keyboard => "Keyboard",
            InputMode::Ps4 => "PS4",
            InputMode::Xbone => "Xbox One",
            InputMode::Mdmini => "Sega Genesis/MegaDrive Mini",
            InputMode::Neogeo => "NEOGEO mini",
            InputMode::Pcemini => "PC Engine/Turbografx 16 Mini",
            InputMode::Egret => "EGRET II mini",
            InputMode::Astro => "ASTROCITY Mini",
            InputMode::Psclassic => "Playstation Classic",
            InputMode::Xboxoriginal => "Original Xbox",
            InputMode::Ps5 => "PS5",
            InputMode::Generic => "Generic HID",
            InputMode::SwitchPro => "Nintendo Switch Pro",
            InputMode::P5general => "P5General",
            InputMode::Sinput => "SInput",
            InputMode::Switch2Pro => "Nintendo Switch 2 Pro",
            InputMode::Webconfig => "web-config",
            InputMode::Bootsel => "BOOTSEL",
        }
    }
}

impl fmt::Display for InputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl FromStr for InputMode {
    type Err = String;

    /// A [`InputMode::key`], ignoring case and `-` vs `_`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.to_ascii_lowercase().replace('-', "_");
        InputMode::ALL
            .into_iter()
            .find(|m| m.key() == wanted)
            .ok_or_else(|| format!("unknown input mode {s:?}"))
    }
}

/// What `POST /api/reboot` restarts the pad into (`BOOT_MODES` in the
/// firmware's `webconfig.cpp`, which must match the web UI's `Navigation.jsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootMode {
    /// The persisted input mode (XInput, Keyboard, ...).
    Gamepad,
    /// The web configurator.
    Webconfig,
    /// The RP2040 boot ROM, for flashing.
    Bootsel,
}

impl BootMode {
    pub const ALL: [BootMode; 3] = [BootMode::Gamepad, BootMode::Webconfig, BootMode::Bootsel];

    /// The `bootMode` value the reboot endpoint takes.
    pub fn api_value(self) -> u8 {
        match self {
            BootMode::Gamepad => 0,
            BootMode::Webconfig => 1,
            BootMode::Bootsel => 2,
        }
    }

    /// The stable key: the JSON value and the CLI name.
    pub fn key(self) -> &'static str {
        match self {
            BootMode::Gamepad => "gamepad",
            BootMode::Webconfig => "webconfig",
            BootMode::Bootsel => "bootsel",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BootMode::Gamepad => "gamepad mode",
            BootMode::Webconfig => "web-config",
            BootMode::Bootsel => "BOOTSEL",
        }
    }
}

impl fmt::Display for BootMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl FromStr for BootMode {
    type Err = String;

    /// A [`BootMode::key`], ignoring case and `-` (`web-config`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.to_ascii_lowercase().replace('-', "");
        BootMode::ALL
            .into_iter()
            .find(|m| m.key() == wanted)
            .ok_or_else(|| format!("unknown boot mode {s:?}"))
    }
}

/// How sure a detection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Only GP2040-CE uses this id (or these USB strings).
    Confirmed,
    /// GP2040-CE uses this id, but so do real devices.
    Likely,
}

/// One VID:PID the firmware enumerates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub vid: u16,
    pub pid: u16,
    /// The modes that produce this id; more than one when the firmware
    /// reuses a descriptor (PS4 and PS5 share the PS4 driver).
    pub modes: &'static [InputMode],
    /// `None` when only GP2040-CE uses the id; otherwise who else does.
    pub shared_with: Option<&'static str>,
}

const fn known(vid: u16, pid: u16, modes: &'static [InputMode], shared: &'static str) -> Known {
    Known {
        vid,
        pid,
        modes,
        shared_with: Some(shared),
    }
}

const fn own(vid: u16, pid: u16, modes: &'static [InputMode]) -> Known {
    Known {
        vid,
        pid,
        modes,
        shared_with: None,
    }
}

use InputMode as M;

const PS4_OR_PS5: &[InputMode] = &[M::Ps4, M::Ps5];

/// Every VID:PID GP2040-CE enumerates with, from its USB descriptors.
pub const KNOWN: &[Known] = &[
    known(
        0x045e,
        0x028e,
        &[M::Xinput],
        "the Xbox 360 controller and its clones",
    ),
    // 0xCAFE is TinyUSB's example VID; GP2040-CE keeps it for these two.
    own(0xcafe, 0x0001, &[M::Keyboard]),
    // NetDriver: 0x4000 | ECM_RNDIS << 5 | HID(=2) << 2.
    own(0xcafe, 0x4028, &[M::Webconfig]),
    known(
        0x10c4,
        0x82c0,
        &[M::Generic],
        "Silicon Labs' example HID id",
    ),
    known(
        0x0f0d,
        0x0092,
        &[M::Switch],
        "HORI's Pokkén Tournament Pro Pad",
    ),
    known(0x054c, 0x0268, &[M::Ps3], "the DualShock 3"),
    // PS3 device types other than gamepad.
    known(0x046d, 0xc294, &[M::Ps3], "Logitech Driving Force wheels"),
    known(0x12ba, 0x0200, &[M::Ps3], "PS3 Guitar Hero guitars"),
    known(0x12ba, 0x0210, &[M::Ps3], "PS3 Rock Band drums"),
    known(
        0x1532,
        0x0401,
        PS4_OR_PS5,
        "the Razer Panthera arcade stick",
    ),
    // PS4 controller ID emulation.
    known(0x054c, 0x09cc, PS4_OR_PS5, "the DualShock 4"),
    // PS4 wheel device type.
    known(0x046d, 0xc260, PS4_OR_PS5, "the Logitech G29 wheel"),
    known(0x0e6f, 0x02a4, &[M::Xbone], "PDP's Xbox One controller"),
    known(
        0x0ca3,
        0x0024,
        &[M::Mdmini],
        "the Sega Genesis/MegaDrive Mini pad",
    ),
    known(0x20bc, 0x5500, &[M::Neogeo], "the NEOGEO mini pad"),
    known(0x0f0d, 0x0138, &[M::Pcemini], "HORI's PC Engine Mini pad"),
    known(0x0ae4, 0x0702, &[M::Egret], "the Egret II Mini panel"),
    known(0x0ca3, 0x0027, &[M::Astro], "the Astro City Mini panel"),
    known(
        0x054c,
        0x0cda,
        &[M::Psclassic],
        "the PlayStation Classic pad",
    ),
    known(
        0x045e,
        0x0289,
        &[M::Xboxoriginal],
        "the original Xbox controller",
    ),
    known(0x057e, 0x2009, &[M::SwitchPro], "the Switch Pro Controller"),
    known(
        0x057e,
        0x2069,
        &[M::Switch2Pro],
        "the Switch 2 Pro Controller",
    ),
    known(
        0x057e,
        0x2073,
        &[M::Switch2Pro],
        "the Switch 2 GameCube controller",
    ),
    known(0x057e, 0x2066, &[M::Switch2Pro], "the right Joy-Con 2"),
    known(0x057e, 0x2067, &[M::Switch2Pro], "the left Joy-Con 2"),
    // The descriptor bytes, not P5GENERAL_VENDOR_ID (0x28B1), which is unused.
    known(0x2b81, 0x0101, &[M::P5general], "P5General adapters"),
    known(0x2e8a, 0x10c6, &[M::Sinput], "other SInput controllers"),
    known(
        0x2e8a,
        0x0003,
        &[M::Bootsel],
        "every RP2040 board in BOOTSEL",
    ),
    known(
        0x2e8a,
        0x000f,
        &[M::Bootsel],
        "every RP2350 board in BOOTSEL",
    ),
];

/// The table entry for a VID:PID.
pub fn lookup(vid: u16, pid: u16) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.vid == vid && k.pid == pid)
}

/// What a USB device's ids and strings say about its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detection {
    /// The mode, when the id belongs to exactly one.
    pub mode: Option<InputMode>,
    /// Every mode that produces this id.
    pub candidates: Vec<InputMode>,
    pub confidence: Confidence,
    /// Why the confidence isn't higher; `None` when confirmed.
    pub note: Option<String>,
}

/// Substrings of the USB manufacturer or product string that only
/// GP2040-CE firmware uses ("GP2040-CE (Keyboard)", "Open Stick Community").
const OWN_STRINGS: &[&str] = &["GP2040", "Open Stick Community"];

/// The detection for a USB device, or `None` when GP2040-CE never uses its
/// id. The strings can confirm a borrowed id.
pub fn detect(
    vid: u16,
    pid: u16,
    manufacturer: Option<&str>,
    product: Option<&str>,
) -> Option<Detection> {
    let known = lookup(vid, pid)?;
    let own_strings = [manufacturer, product]
        .into_iter()
        .flatten()
        .any(|s| OWN_STRINGS.iter().any(|own| s.contains(own)));
    let (confidence, note) = match known.shared_with {
        None => (Confidence::Confirmed, None),
        Some(_) if own_strings => (Confidence::Confirmed, None),
        Some(who) => (
            Confidence::Likely,
            Some(format!("{vid:04x}:{pid:04x} is also used by {who}")),
        ),
    };
    Some(Detection {
        mode: (known.modes.len() == 1).then(|| known.modes[0]),
        candidates: known.modes.to_vec(),
        confidence,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn proto_values_round_trip() {
        for m in InputMode::ALL {
            if let Some(v) = m.proto_value() {
                assert_eq!(InputMode::from_proto(v), Some(m));
            }
        }
        assert_eq!(InputMode::from_proto(18), None);
        assert_eq!(InputMode::from_proto(255), Some(InputMode::Webconfig));
        assert_eq!(InputMode::Bootsel.proto_value(), None);
        assert_eq!(InputMode::Switch2Pro.proto_value(), Some(19));
    }

    #[test]
    fn boot_modes_match_the_firmware() {
        let values: Vec<u8> = BootMode::ALL.iter().map(|m| m.api_value()).collect();
        assert_eq!(values, [0, 1, 2]);
        for m in BootMode::ALL {
            assert_eq!(serde_json::to_value(m).unwrap(), m.key());
            assert_eq!(m.key().parse::<BootMode>(), Ok(m));
        }
        assert_eq!("Web-Config".parse(), Ok(BootMode::Webconfig));
        assert!("usb".parse::<BootMode>().is_err());
    }

    #[test]
    fn keys_match_serde_and_parse() {
        for m in InputMode::ALL {
            let json = serde_json::to_value(m).unwrap();
            assert_eq!(json, m.key(), "{m:?}");
            assert_eq!(m.key().parse::<InputMode>(), Ok(m));
        }
        assert_eq!("Switch-Pro".parse(), Ok(InputMode::SwitchPro));
        assert!("xbox".parse::<InputMode>().is_err());
    }

    #[test]
    fn table_has_no_duplicate_ids() {
        let mut seen = HashSet::new();
        for k in KNOWN {
            assert!(seen.insert((k.vid, k.pid)), "{:04x}:{:04x}", k.vid, k.pid);
            assert!(!k.modes.is_empty());
        }
    }

    #[test]
    fn every_firmware_mode_has_an_id() {
        for m in InputMode::ALL {
            assert!(
                KNOWN.iter().any(|k| k.modes.contains(&m)),
                "{m:?} has no VID:PID"
            );
        }
    }

    #[test]
    fn ids_from_the_plan() {
        let mode = |vid, pid| lookup(vid, pid).map(|k| k.modes);
        assert_eq!(mode(0x045e, 0x028e), Some(&[M::Xinput][..]));
        assert_eq!(mode(0xcafe, 0x0001), Some(&[M::Keyboard][..]));
        assert_eq!(mode(0x10c4, 0x82c0), Some(&[M::Generic][..]));
        assert_eq!(mode(0x0f0d, 0x0092), Some(&[M::Switch][..]));
        assert_eq!(mode(0x1532, 0x0401), Some(PS4_OR_PS5));
        assert_eq!(mode(0xcafe, 0x4028), Some(&[M::Webconfig][..]));
        assert_eq!(mode(0x2e8a, 0x0003), Some(&[M::Bootsel][..]));
        assert_eq!(mode(0x046d, 0xc52b), None);
    }

    #[test]
    fn xinput_is_only_likely() {
        let d = detect(
            0x045e,
            0x028e,
            Some("©Microsoft Corporation"),
            Some("Controller"),
        )
        .unwrap();
        assert_eq!(d.mode, Some(M::Xinput));
        assert_eq!(d.confidence, Confidence::Likely);
        assert_eq!(
            d.note.as_deref(),
            Some("045e:028e is also used by the Xbox 360 controller and its clones")
        );
    }

    #[test]
    fn cafe_ids_are_confirmed() {
        let d = detect(0xcafe, 0x4028, Some("TinyUSB"), Some("TinyUSB Device")).unwrap();
        assert_eq!(d.mode, Some(M::Webconfig));
        assert_eq!(d.confidence, Confidence::Confirmed);
        assert_eq!(d.note, None);
        let d = detect(0xcafe, 0x0001, None, None).unwrap();
        assert_eq!(d.confidence, Confidence::Confirmed);
    }

    #[test]
    fn firmware_strings_confirm_a_borrowed_id() {
        let d = detect(
            0x10c4,
            0x82c0,
            Some("Open Stick Community"),
            Some("GP2040-CE (Generic)"),
        )
        .unwrap();
        assert_eq!(d.mode, Some(M::Generic));
        assert_eq!(d.confidence, Confidence::Confirmed);
        let d = detect(0x10c4, 0x82c0, Some("Silicon Labs"), None).unwrap();
        assert_eq!(d.confidence, Confidence::Likely);
    }

    #[test]
    fn ps4_and_ps5_share_ids() {
        let d = detect(0x1532, 0x0401, None, None).unwrap();
        assert_eq!(d.mode, None);
        assert_eq!(d.candidates, vec![M::Ps4, M::Ps5]);
    }

    #[test]
    fn unknown_ids_are_not_detected() {
        assert_eq!(detect(0x1d6b, 0x0002, None, None), None);
    }
}
