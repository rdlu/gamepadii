//! Discovery against sysfs fixture trees in `tests/fixtures/sysfs/`:
//!
//! - `s16-webconfig`: captured from the Haute42 S16 in web-config mode
//!   (stock v0.7.12), with this machine's root hub and two USB receivers
//!   that GP2040-CE never uses. Root hub and receiver serials are redacted.
//! - `s16-xinput`: the same tree with the S16's attributes as read live in
//!   XInput (045e:028e, four vendor interfaces); serial replaced.
//! - `keyboard`, `bootsel`, `two-pads`, `no-pad`: built from the capture,
//!   with the ids and strings from the firmware's descriptors.

use std::path::PathBuf;

use gamepadii_core::commands;
use gamepadii_core::discovery::scan;
use gamepadii_core::mode::{Confidence, InputMode};
use gamepadii_core::sysfs::Root;

fn fixture(name: &str) -> Root {
    Root::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sysfs")
            .join(name),
    )
}

const S16_SYSFS: &str = "/sys/devices/pci0000:00/0000:00:08.1/0000:05:00.4/usb3/3-1";

#[test]
fn s16_in_webconfig() {
    let devices = scan(&fixture("s16-webconfig"));
    assert_eq!(devices.len(), 1, "{devices:#?}");
    let d = &devices[0];
    assert_eq!(d.mode, Some(InputMode::Webconfig));
    assert_eq!(d.confidence, Confidence::Confirmed);
    assert_eq!(d.usb_id(), "cafe:4028");
    assert_eq!(d.product.as_deref(), Some("TinyUSB Device"));
    assert_eq!(d.serial.as_deref(), Some("123456"));
    assert_eq!((d.bus, d.devnum), (Some(3), Some(7)));
    assert_eq!(d.port, "3-1");
    assert_eq!(d.sysfs_path, Some(PathBuf::from(S16_SYSFS)));
    assert_eq!(d.netdev.as_deref(), Some("enp5s0f4u1"));
}

#[test]
fn s16_in_xinput_is_likely() {
    let devices = scan(&fixture("s16-xinput"));
    assert_eq!(devices.len(), 1, "{devices:#?}");
    let d = &devices[0];
    assert_eq!(d.mode, Some(InputMode::Xinput));
    assert_eq!(d.confidence, Confidence::Likely);
    assert!(d.note.as_deref().unwrap().contains("Xbox 360"));
    assert_eq!(d.manufacturer.as_deref(), Some("©Microsoft Corporation"));
    assert_eq!(d.netdev, None);
}

#[test]
fn keyboard_and_bootsel() {
    let kb = scan(&fixture("keyboard"));
    assert_eq!(kb[0].mode, Some(InputMode::Keyboard));
    assert_eq!(kb[0].confidence, Confidence::Confirmed);
    let boot = scan(&fixture("bootsel"));
    assert_eq!(boot[0].mode, Some(InputMode::Bootsel));
    assert_eq!(boot[0].confidence, Confidence::Likely);
}

#[test]
fn two_pads_sorted_by_bus() {
    let devices = scan(&fixture("two-pads"));
    let ids: Vec<String> = devices.iter().map(|d| d.usb_id()).collect();
    assert_eq!(ids, ["cafe:4028", "1532:0401"]);
    assert_eq!(devices[1].mode, None);
    assert_eq!(devices[1].candidates, [InputMode::Ps4, InputMode::Ps5]);
    assert_eq!(devices[1].port, "6-1.3");
}

#[test]
fn no_pad_is_not_found() {
    assert!(scan(&fixture("no-pad")).is_empty());
    let err = commands::status(&fixture("no-pad")).unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert_eq!(err.kind(), "not_found");
}

#[test]
fn missing_sysfs_is_empty() {
    assert!(scan(&Root::new("/nonexistent")).is_empty());
}
