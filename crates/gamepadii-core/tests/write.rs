//! `setGamepadOptions` against the mock, which models the v0.7.12 setter:
//! missing keys zeroed, USB strings from NULL, hex ids parsed as decimal.

use std::path::PathBuf;
use std::sync::Arc;

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
    let body = write::setter_body(&current).unwrap();
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
    let body = write::setter_body(&current).unwrap();
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
        let err = write::setter_body(&o).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    let mut o = fixture_options();
    o["usbDescProduct"] = Value::Null;
    assert!(write::setter_body(&o).is_err());
    let mut o = fixture_options();
    o["hotkey03"] = json!({"action": 2});
    assert!(write::setter_body(&o).is_err());
    let mut o = fixture_options();
    o["usbVendorID"] = json!("XYZW");
    assert!(write::setter_body(&o).is_err());
}

#[test]
fn read_modify_write_posts_the_whole_object_and_verifies() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_gamepad_options(&t, |o| o.input_mode = Some(3)).unwrap();
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
    api::set_gamepad_options(&t, |o| o.input_mode = Some(1)).unwrap();
    let sent = mock.requests()[1].json();
    assert_eq!(sent["someNewOption"], 7);
}

#[test]
fn no_change_posts_nothing() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    let w = api::set_gamepad_options(&t, |o| o.input_mode = Some(0)).unwrap();
    assert!(!w.posted);
    assert!(w.changed.is_empty());
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));
}

#[test]
fn a_write_that_does_not_take_is_a_failed_check() {
    let (t, _) = s16(Mock::new(s16_dir()).set_fault(SetFault::Ignore));
    let err = api::set_gamepad_options(&t, |o| o.input_mode = Some(3)).unwrap_err();
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
    let err = api::set_gamepad_options(&t, |o| o.input_mode = Some(3)).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    let msg = err.to_string();
    assert!(msg.contains("socdMode: wanted 2, the pad has 0"), "{msg}");
    assert!(!msg.contains("inputMode"), "{msg}");
}

#[test]
fn set_endpoints_are_never_read() {
    let (t, mock) = s16(Mock::new(s16_dir()));
    api::set_gamepad_options(&t, |o| o.socd_mode = Some(1)).unwrap();
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
    assert!(api::set_gamepad_options(&t, |_| {}).is_err());
}
