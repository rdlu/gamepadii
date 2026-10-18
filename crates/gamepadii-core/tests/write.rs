//! `setGamepadOptions` against the mock, which models the v0.7.12 setter:
//! missing keys zeroed, USB strings from NULL, hex ids parsed as decimal.

use std::path::PathBuf;
use std::sync::Arc;

use gamepadii_core::api::FirmwareLine::{Main, V0712};
use gamepadii_core::api::write::{self, SETTER_KEYS};
use gamepadii_core::api::{self, RebootAck};
use gamepadii_core::error::exit;
use gamepadii_core::mode::BootMode;
use gamepadii_core::transport::mock::{self, Mock, SetFault};
use gamepadii_core::transport::{ApiPath, Method, Reply, Transport};
use serde_json::{Map, Value, json};

fn s16_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
}

#[derive(Clone)]
struct Shared(Arc<Mock>);

impl Transport for Shared {
    fn request(&self, m: Method, p: ApiPath, b: Option<&[u8]>) -> gamepadii_core::Result<Reply> {
        self.0.request(m, p, b)
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
}

fn s16(mock: Mock) -> (Shared, Arc<Mock>) {
    let mock = Arc::new(mock);
    (Shared(mock.clone()), mock)
}

fn fixture_options() -> Map<String, Value> {
    let text = std::fs::read_to_string(s16_dir().join("getGamepadOptions.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_mock_zeroes_what_a_partial_body_leaves_out() {
    // What the firmware would do with {"inputMode": 3} alone: the reason
    // gamepadii never sends one.
    let current = fixture_options();
    let after = mock::firmware_set_gamepad_options(&current, &json!({"inputMode": 3}));
    assert_eq!(after["inputMode"], 3);
    assert_eq!(after["socdMode"], 0, "was 2");
    assert_eq!(after["profileNumber"], 0, "was 2");
    assert_eq!(after["inputModeL1"], 0, "was -1 (none), now XInput");
    assert_eq!(after["usbDescProduct"], mock::NULL_STRING_JUNK);
    assert_eq!(
        after["hotkey01"],
        json!({"auxMask": 0, "buttonsMask": 0, "action": 0})
    );
    assert_eq!(after["fnButtonPin"], -1, "getter-only, untouched");
}

#[test]
fn the_mock_round_trips_a_full_web_ui_style_body() {
    let current = fixture_options();
    let body = write::setter_body(&current, V0712).unwrap();
    assert_eq!(
        mock::firmware_set_gamepad_options(&current, &Value::Object(body)),
        current
    );
    // Echoing the getter's hex strings would not survive: "10C4" parses to 0.
    let echoed = mock::firmware_set_gamepad_options(&current, &Value::Object(current.clone()));
    assert_eq!(echoed["usbVendorID"], "0000");
    assert_eq!(echoed["usbProductID"], "0000");
}

#[test]
fn setter_body_converts_the_usb_ids_and_keeps_everything() {
    let current = fixture_options();
    let body = write::setter_body(&current, V0712).unwrap();
    assert_eq!(body["usbVendorID"], 0x10c4);
    assert_eq!(body["usbProductID"], 0x82c0);
    assert_eq!(body.len(), current.len());
    assert!(body.keys().eq(current.keys()), "same keys, same order");
    for key in SETTER_KEYS {
        assert!(body.contains_key(*key), "{key}");
    }
}

#[test]
fn setter_body_refuses_an_incomplete_object() {
    for key in [
        "usbDescProduct",
        "inputModeB1",
        "hotkey16",
        "forcedSetupMode",
    ] {
        let mut o = fixture_options();
        o.remove(key);
        let err = write::setter_body(&o, V0712).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    let mut o = fixture_options();
    o["usbDescProduct"] = Value::Null;
    assert!(write::setter_body(&o, V0712).is_err());
    let mut o = fixture_options();
    o["hotkey03"] = json!({"action": 2});
    assert!(write::setter_body(&o, V0712).is_err());
    let mut o = fixture_options();
    o["usbVendorID"] = json!("XYZW");
    assert!(write::setter_body(&o, V0712).is_err());
}

#[test]
fn read_modify_write_posts_the_whole_object_and_verifies() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_gamepad_options(&t, V0712, |o| o.input_mode = Some(3)).unwrap();
    assert!(w.posted);
    assert_eq!(w.changed.len(), 1);
    assert_eq!(w.changed[0].key, "inputMode");
    assert_eq!(
        (&w.changed[0].before, &w.changed[0].after),
        (&json!(0), &json!(3))
    );

    let log = mock.requests();
    let calls: Vec<_> = log.iter().map(|r| (r.method, r.path)).collect();
    assert_eq!(
        calls,
        [
            (Method::Get, "/api/getGamepadOptions"),
            (Method::Post, "/api/setGamepadOptions"),
            (Method::Get, "/api/getGamepadOptions"),
        ]
    );
    let sent = log[1].json();
    let sent = sent.as_object().unwrap();
    let current = fixture_options();
    assert!(
        sent.keys().eq(current.keys()),
        "every key, in the pad's order"
    );
    for (k, v) in &current {
        match k.as_str() {
            "inputMode" => assert_eq!(sent[k], 3),
            "usbVendorID" | "usbProductID" => assert!(sent[k].is_u64(), "{k}"),
            _ => assert_eq!(&sent[k], v, "{k}"),
        }
    }
    // The pad now has exactly one change.
    let after = mock.current("getGamepadOptions").unwrap();
    let diff = write::diff(&current, after.as_object().unwrap());
    assert_eq!(diff.len(), 1, "{diff:?}");
}

#[test]
fn unknown_fields_survive_the_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = fixture_options();
    o.insert("someNewOption".into(), json!(7));
    std::fs::write(
        dir.path().join("getGamepadOptions.json"),
        Value::Object(o).to_string(),
    )
    .unwrap();
    let (t, mock) = s16(Mock::new(dir.path()));
    api::set_gamepad_options(&t, V0712, |o| o.input_mode = Some(1)).unwrap();
    let sent = mock.requests()[1].json();
    assert_eq!(sent["someNewOption"], 7);
}

#[test]
fn no_change_posts_nothing() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_gamepad_options(&t, V0712, |o| o.input_mode = Some(0)).unwrap();
    assert!(!w.posted);
    assert!(w.changed.is_empty());
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));
}

#[test]
fn a_write_that_does_not_take_is_a_failed_check() {
    let (t, _) = s16(Mock::new(s16_dir()).set_fault(SetFault::Ignore));
    let err = api::set_gamepad_options(&t, V0712, |o| o.input_mode = Some(3)).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    assert!(
        err.to_string()
            .contains("inputMode: wanted 3, the pad has 0"),
        "{err}"
    );
}

#[test]
fn a_write_that_changes_something_else_is_a_failed_check() {
    let (t, _) = s16(Mock::new(s16_dir()).set_fault(SetFault::Clobber("socdMode", json!(0))));
    let err = api::set_gamepad_options(&t, V0712, |o| o.input_mode = Some(3)).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    let msg = err.to_string();
    assert!(msg.contains("socdMode: wanted 2, the pad has 0"), "{msg}");
    assert!(!msg.contains("inputMode"), "{msg}");
}

#[test]
fn set_endpoints_are_never_read() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    api::set_gamepad_options(&t, V0712, |o| o.socd_mode = Some(1)).unwrap();
    for r in mock.requests() {
        if r.path.starts_with("/api/set") {
            assert_eq!(r.method, Method::Post, "{}", r.path);
        }
    }
}

#[test]
fn reboot_acks() {
    let (t, _) = s16(Mock::new(s16_dir()));
    assert_eq!(
        api::reboot(&t, BootMode::Gamepad).unwrap(),
        RebootAck::Confirmed
    );
    let (t, mock) = s16(Mock::new(s16_dir()).drop_reboot());
    assert_eq!(
        api::reboot(&t, BootMode::Gamepad).unwrap(),
        RebootAck::Probable
    );
    assert_eq!(mock.requests().len(), 1);
    // A hang before the reboot was ever sent is still an error.
    let (t, _) = s16(Mock::new(s16_dir()).hang_on("/api/getGamepadOptions"));
    assert!(api::set_gamepad_options(&t, V0712, |_| {}).is_err());
}

// ---- setDisplayOptions ------------------------------------------------------

fn fixture_display() -> Map<String, Value> {
    let text = std::fs::read_to_string(s16_dir().join("getDisplayOptions.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_mock_zeroes_what_a_partial_display_body_leaves_out() {
    let current = fixture_display();
    let after = mock::firmware_set_display_options(
        &current,
        &json!({"buttonLayout": 36, "buttonLayoutRight": 42}),
    );
    assert_eq!(after["buttonLayout"], 36);
    assert_eq!(after["buttonLayoutRight"], 42);
    assert_eq!(after["enabled"], 0, "the display, off");
    assert_eq!(after["splashDuration"], 0, "was 3000");
    assert_eq!(after["inputHistoryLength"], 0, "was 21");
    assert_eq!(
        after["buttonLayoutCustomOptions"]["params"],
        json!({"layout": 0, "startX": 0, "startY": 0, "buttonRadius": 0, "buttonPadding": 0})
    );
}

#[test]
fn the_mock_round_trips_the_display_getter() {
    let current = fixture_display();
    let body = write::display_setter_body(&current, V0712).unwrap();
    assert_eq!(body, current, "sent back as it is");
    assert_eq!(
        mock::firmware_set_display_options(&current, &Value::Object(body)),
        current
    );
    // bools come back as 1/0 whatever number was sent.
    let after = mock::firmware_set_display_options(&current, &json!({"enabled": 5}));
    assert_eq!(after["enabled"], 1);
}

#[test]
fn display_setter_body_refuses_an_incomplete_object() {
    for key in ["enabled", "buttonLayoutRight", "inputHistoryRow"] {
        let mut o = fixture_display();
        o.remove(key);
        let err = write::display_setter_body(&o, V0712).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
        assert!(
            err.to_string().contains("setDisplayOptions: not writing"),
            "{err}"
        );
    }
    let mut o = fixture_display();
    o["splashMode"] = json!("0");
    assert!(write::display_setter_body(&o, V0712).is_err());
    let mut o = fixture_display();
    o.remove("buttonLayoutCustomOptions");
    assert!(write::display_setter_body(&o, V0712).is_err());
    let mut o = fixture_display();
    o["buttonLayoutCustomOptions"]["paramsRight"]
        .as_object_mut()
        .unwrap()
        .remove("buttonPadding");
    let err = write::display_setter_body(&o, V0712).unwrap_err();
    assert!(err.to_string().contains("paramsRight"), "{err}");
}

#[test]
fn display_read_modify_write_posts_the_whole_object_and_verifies() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_display_options(&t, V0712, |d| {
        d.button_layout = Some(36);
        d.button_layout_right = Some(42);
    })
    .unwrap();
    assert!(w.posted);
    let changed: Vec<_> = w
        .changed
        .iter()
        .map(|c| (c.key.as_str(), c.before.clone(), c.after.clone()))
        .collect();
    assert_eq!(
        changed,
        [
            ("buttonLayout", json!(27), json!(36)),
            ("buttonLayoutRight", json!(31), json!(42)),
        ]
    );
    let log = mock.requests();
    let calls: Vec<_> = log.iter().map(|r| (r.method, r.path)).collect();
    assert_eq!(
        calls,
        [
            (Method::Get, "/api/getDisplayOptions"),
            (Method::Post, "/api/setDisplayOptions"),
            (Method::Get, "/api/getDisplayOptions"),
        ]
    );
    let sent = log[1].json();
    let sent = sent.as_object().unwrap();
    let current = fixture_display();
    assert!(
        sent.keys().eq(current.keys()),
        "every key, in the pad's order"
    );
    for (k, v) in &current {
        match k.as_str() {
            "buttonLayout" => assert_eq!(sent[k], 36),
            "buttonLayoutRight" => assert_eq!(sent[k], 42),
            _ => assert_eq!(&sent[k], v, "{k}"),
        }
    }
    // Nothing else moved on the pad.
    let after = mock.current("getDisplayOptions").unwrap();
    assert_eq!(write::diff(&current, after.as_object().unwrap()).len(), 2);
    // The gamepad options were never touched.
    assert!(log.iter().all(|r| !r.path.contains("Gamepad")));
}

#[test]
fn display_no_change_posts_nothing_and_faults_fail_the_check() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_display_options(&t, V0712, |d| d.button_layout = Some(27)).unwrap();
    assert!(!w.posted);
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));

    let (t, _) = s16(Mock::new(s16_dir()).set_fault(SetFault::Ignore));
    let err = api::set_display_options(&t, V0712, |d| d.button_layout = Some(36)).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    let msg = err.to_string();
    assert!(msg.contains("setDisplayOptions was written"), "{msg}");
    assert!(
        msg.contains("buttonLayout: wanted 36, the pad has 27"),
        "{msg}"
    );

    let (t, _) = s16(Mock::new(s16_dir()).set_fault(SetFault::Clobber("splashMode", json!(3))));
    let err = api::set_display_options(&t, V0712, |d| d.button_layout = Some(36)).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    assert!(
        err.to_string()
            .contains("splashMode: wanted 0, the pad has 3"),
        "{err}"
    );
}

// ---- upstream main (tests/fixtures/api/s16-main, synthetic) --------------

fn main_dir() -> PathBuf {
    s16_dir().join("../s16-main")
}

fn main_object(name: &str) -> Map<String, Value> {
    let text = std::fs::read_to_string(main_dir().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A main pad whose `name` getter lacks `key` (an overlay of s16-main).
fn main_without(name: &str, key: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let pad = dir.path().join("pad");
    std::fs::create_dir(&pad).unwrap();
    std::fs::write(pad.join("BASE"), main_dir().to_str().unwrap()).unwrap();
    let mut o = main_object(name);
    o.remove(key).unwrap();
    std::fs::write(
        pad.join(format!("{name}.json")),
        Value::Object(o).to_string(),
    )
    .unwrap();
    (dir, pad)
}

#[test]
fn mains_setters_read_more_keys() {
    let gamepad = write::setter_keys(Main);
    assert_eq!(gamepad.len(), SETTER_KEYS.len() + 15);
    assert!(gamepad.contains(&"switch2ProJoyCon2RightStickColor"));
    assert_eq!(write::setter_keys(V0712), SETTER_KEYS);
    let display = write::display_setter_keys(Main);
    assert_eq!(display.last(), Some(&"displayContrast"));
    assert_eq!(
        write::display_setter_keys(V0712),
        write::DISPLAY_SETTER_KEYS
    );
    // Main's getters return every key its setters read.
    let options = main_object("getGamepadOptions");
    for k in gamepad {
        assert!(options.contains_key(k), "{k}");
    }
    let display_options = main_object("getDisplayOptions");
    for k in display {
        assert!(display_options.contains_key(k), "{k}");
    }
}

#[test]
fn the_mock_zeroes_mains_new_keys_too() {
    let current = main_object("getGamepadOptions");
    assert_ne!(current["switch2ProBodyColor"], json!(0));
    let after = mock::firmware_set_gamepad_options(&current, &json!({"inputMode": 3}));
    assert_eq!(after["switch2ProBodyColor"], json!(0));
    let display = main_object("getDisplayOptions");
    let after = mock::firmware_set_display_options(&display, &json!({"enabled": 1}));
    assert_eq!(after["displayContrast"], json!(0));
}

#[test]
fn a_write_on_main_keeps_its_new_keys() {
    let (t, mock) = s16(Mock::new(main_dir()));
    let before = main_object("getGamepadOptions");
    let w = api::set_gamepad_options(&t, Main, |o| o.input_mode = Some(3)).unwrap();
    assert_eq!(w.changed.len(), 1);
    assert_eq!(w.changed[0].key, "inputMode");
    let sent = mock.requests()[1].json();
    for k in write::MAIN_SETTER_KEYS {
        assert_eq!(sent[*k], before[*k], "{k}");
    }
    let after = mock.current("getGamepadOptions").unwrap();
    assert_eq!(after["switch2ProBodyColor"], before["switch2ProBodyColor"]);

    let (t, mock) = s16(Mock::new(main_dir()));
    let w = api::set_display_options(&t, Main, |d| d.button_layout = Some(36)).unwrap();
    assert_eq!(w.changed.len(), 1);
    assert_eq!(mock.requests()[1].json()["displayContrast"], json!(255));
}

#[test]
fn main_refuses_a_getter_without_a_key_its_setter_reads() {
    let (_dir, pad) = main_without("getGamepadOptions", "switch2ProGripColor");
    let (t, mock) = s16(Mock::new(pad));
    let err = api::set_gamepad_options(&t, Main, |o| o.input_mode = Some(3)).unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    let msg = err.to_string();
    assert!(msg.contains("switch2ProGripColor"), "{msg}");
    assert!(msg.contains("main firmware reads it"), "{msg}");
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));

    let (_dir, pad) = main_without("getDisplayOptions", "displayContrast");
    let (t, mock) = s16(Mock::new(pad));
    let err = api::set_display_options(&t, Main, |d| d.button_layout = Some(36)).unwrap_err();
    assert!(err.to_string().contains("displayContrast"), "{err}");
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));
}
