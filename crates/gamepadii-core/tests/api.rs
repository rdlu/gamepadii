//! The web-config client against the S16 capture (`tests/fixtures/api/`):
//! GP2040-CE v0.7.12, board config Haute42COSMOX, build 0014e4a, read
//! through the allowlist with the pad in web-config on 2026-10-17. It has no
//! serials or other personal data; the profile labels are the user's.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use gamepadii_core::api::{self, Endpoint};
use gamepadii_core::client::{self, Client};
use gamepadii_core::commands;
use gamepadii_core::error::exit;
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::sysfs::Root;
use gamepadii_core::transport::mock::{Mock, Request};
use gamepadii_core::transport::{ApiPath, Method, Reply, Transport};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn s16_dir() -> PathBuf {
    fixtures().join("api/s16-v0.7.12")
}

/// A mock the test keeps a handle on, to read its request log.
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

fn s16() -> (Client, Arc<Mock>) {
    with(Mock::new(s16_dir()))
}

fn with(mock: Mock) -> (Client, Arc<Mock>) {
    let mock = Arc::new(mock);
    let client = Client::open(Box::new(Shared(mock.clone()))).unwrap();
    (client, mock)
}

fn fixture(name: &str) -> Value {
    serde_json::from_slice(&fs::read(s16_dir().join(format!("{name}.json"))).unwrap()).unwrap()
}

/// 2026-10-17T16:57:03Z, when the capture was taken.
fn captured() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_792_256_223)
}

const FORBIDDEN: &[&str] = &[
    "getConfig",
    "getI2CPeripheralMap",
    "resetSettings",
    "getHeldPins",
    "abortGetHeldPins",
    "setConfig",
    "setGamepadOptions",
    "echo",
];

#[test]
fn allowlist_is_the_verified_set() {
    let names: Vec<&str> = Endpoint::ALL.iter().map(|e| e.name()).collect();
    assert_eq!(
        names,
        [
            "getAddonsOptions",
            "getButtonLayoutDefs",
            "getButtonLayouts",
            "getCustomTheme",
            "getDisplayOptions",
            "getExpansionPins",
            "getFirmwareVersion",
            "getGamepadOptions",
            "getHETriggerCalibration",
            "getHETriggerOptions",
            "getKeyMappings",
            "getLedOptions",
            "getMacroAddonOptions",
            "getMemoryReport",
            "getPeripheralOptions",
            "getPinMappings",
            "getProfileOptions",
            "getReactiveLEDs",
            "getSplashImage",
            "getUsedPins",
            "getWiiControls",
        ]
    );
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, names, "sorted, no duplicates");
    // Every fixture is in the allowlist and the other way round.
    let mut files: Vec<String> = fs::read_dir(s16_dir())
        .unwrap()
        .map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .replace(".json", "")
        })
        .collect();
    files.sort();
    assert_eq!(files, names);
}

#[test]
fn dangerous_endpoints_cannot_be_named() {
    for name in FORBIDDEN {
        assert_eq!(Endpoint::from_name(name), None, "{name}");
        assert!(Endpoint::ALL.iter().all(|e| e.name() != *name), "{name}");
    }
    for e in Endpoint::ALL {
        assert!(e.name().starts_with("get"), "{e}");
        assert_eq!(Endpoint::from_name(e.name()), Some(*e));
    }
}

#[test]
fn backup_never_asks_for_a_dangerous_endpoint() {
    let (client, mock) = s16();
    commands::backup_bundle(&client, captured()).unwrap();
    commands::info(&client).unwrap();
    let log = mock.requests();
    assert!(!log.is_empty());
    for Request { method, path, body } in &log {
        assert_eq!(*method, Method::Get, "{path}");
        assert_eq!(*body, None);
        let name = path.strip_prefix("/api/").unwrap();
        assert!(Endpoint::from_name(name).is_some(), "{path}");
        assert!(!FORBIDDEN.contains(&name), "{path}");
    }
}

#[test]
fn typed_reads() {
    let (client, _) = s16();
    let fw = client.firmware();
    assert_eq!(fw.version.as_deref(), Some("v0.7.12"));
    assert_eq!(fw.board_config.as_deref(), Some("Haute42COSMOX"));
    assert_eq!(fw.board_build.as_deref(), Some("0014e4a"));
    assert!(fw.extra.is_empty());
    let t = client.transport();
    let mem = api::memory_report(t).unwrap();
    assert_eq!(mem.total_flash, Some(2_097_152));
    assert_eq!(mem.used_heap, Some(19_808));
    let opts = api::gamepad_options(t).unwrap();
    assert_eq!(opts.input_mode, Some(0));
    assert_eq!(opts.profile_number, Some(2));
    assert_eq!(opts.input_mode_b4, Some(13));
    assert_eq!(opts.extra["usbDescProduct"], "GP2040-CE (Custom)");
    let base = api::pin_mappings(t).unwrap();
    assert_eq!(base.profile_label.as_deref(), Some("Default"));
    let pins = base.pins();
    assert_eq!(pins.len(), 30);
    assert_eq!(pins[&15].action, -10);
    let alts = api::profile_options(t).unwrap().alternative_pin_mappings;
    let labels: Vec<_> = alts
        .iter()
        .map(|p| p.profile_label.as_deref().unwrap())
        .collect();
    assert_eq!(labels, ["SilkSong", "Celeste", "DKC", "TGX"]);
    let keys = api::key_mappings(t).unwrap();
    assert_eq!(keys.0.len(), 32);
    assert_eq!(keys.0["Up"], 82);
}

/// Unknown fields survive a read → write, since `setGamepadOptions` zeroes
/// anything it isn't sent.
#[test]
fn typed_structs_round_trip() {
    fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(name: &str) {
        let raw = fixture(name);
        let typed: T = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), raw, "{name}");
    }
    round_trip::<api::FirmwareVersion>("getFirmwareVersion");
    round_trip::<api::MemoryReport>("getMemoryReport");
    round_trip::<api::GamepadOptions>("getGamepadOptions");
    round_trip::<api::PinMappings>("getPinMappings");
    round_trip::<api::ProfileOptions>("getProfileOptions");
    round_trip::<api::KeyMappings>("getKeyMappings");
    // A field from a newer firmware, and a missing one.
    let newer = json!({"inputMode": 3, "someNewOption": {"a": [1, 2]}});
    let typed: api::GamepadOptions = serde_json::from_value(newer.clone()).unwrap();
    assert_eq!(typed.profile_number, None);
    assert_eq!(serde_json::to_value(&typed).unwrap(), newer);
}

#[test]
fn info_from_the_s16() {
    let (client, _) = s16();
    let info = commands::info(&client).unwrap();
    assert_eq!(info.input_mode.mode, Some(InputMode::Xinput));
    assert_eq!(info.active_profile, Some(2));
    assert_eq!(info.profiles.len(), 5);
    assert!(info.profiles[1].active);
    assert_eq!(info.profiles[1].label.as_deref(), Some("SilkSong"));
    let boot: Vec<_> = info
        .boot_buttons
        .iter()
        .map(|b| (b.button.as_str(), b.setting.mode))
        .collect();
    assert_eq!(
        boot,
        [
            ("B1", Some(InputMode::Switch)),
            ("B2", Some(InputMode::Xinput)),
            ("B3", Some(InputMode::Ps3)),
            ("B4", Some(InputMode::Ps5)),
            ("L1", None),
            ("L2", None),
            ("R1", Some(InputMode::Xbone)),
            ("R2", Some(InputMode::Keyboard)),
        ]
    );
    assert_eq!(info.boot_buttons[4].setting.value, Some(-1));
}

#[test]
fn info_without_a_memory_report() {
    // A copy of the capture without getMemoryReport: an older firmware.
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("older");
    fs::create_dir(&old).unwrap();
    for e in Endpoint::ALL
        .iter()
        .filter(|e| **e != Endpoint::GetMemoryReport)
    {
        let name = format!("{}.json", e.name());
        fs::copy(s16_dir().join(&name), old.join(&name)).unwrap();
    }
    let (client, _) = with(Mock::new(&old));
    assert_eq!(commands::info(&client).unwrap().memory, None);
    let bundle = commands::backup_bundle(&client, captured()).unwrap();
    assert_eq!(bundle.endpoints["getMemoryReport"], Value::Null);
}

#[test]
fn backup_bundle_has_every_endpoint_verbatim() {
    let (client, _) = s16();
    let bundle = commands::backup_bundle(&client, captured()).unwrap();
    assert_eq!(bundle.schema, 1);
    assert_eq!(bundle.captured_at, "2026-10-17T16:57:03Z");
    assert_eq!(bundle.endpoints.len(), Endpoint::ALL.len());
    for e in Endpoint::ALL {
        assert_eq!(bundle.endpoints[e.name()], fixture(e.name()), "{e}");
    }
}

#[test]
fn backup_bundle_structure() {
    let (client, _) = s16();
    let bundle = commands::backup_bundle(&client, captured()).unwrap();
    let mut v = serde_json::to_value(&bundle).unwrap();
    // The payloads are checked verbatim above; here, only their shape.
    for (_, payload) in v["endpoints"].as_object_mut().unwrap() {
        let shape = match &*payload {
            Value::Object(o) => format!("object, {} keys", o.len()),
            other => other.to_string(),
        };
        *payload = json!(shape);
    }
    v["gamepadii_version"] = json!("[version]");
    insta::assert_json_snapshot!(v);
}

#[test]
fn backup_writes_a_file() {
    let (client, _) = s16();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/s16.json");
    let report = commands::backup(&client, Some(&path), captured()).unwrap();
    assert_eq!(report.path, path);
    assert_eq!(report.endpoints.len(), 21);
    assert!(report.unavailable.is_empty());
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(report.bytes, text.len() as u64);
    let back: commands::Bundle = serde_json::from_str(&text).unwrap();
    assert_eq!(back, commands::backup_bundle(&client, captured()).unwrap());
    assert!(!dir.path().join("sub/s16.json.tmp").exists());
}

#[test]
fn backup_file_name() {
    let (client, _) = s16();
    let bundle = commands::backup_bundle(&client, captured()).unwrap();
    assert_eq!(
        commands::backup_file_name(&bundle, captured()),
        "20261017-165703-Haute42COSMOX.json"
    );
}

#[test]
fn backup_stops_at_the_first_transport_error() {
    let (client, mock) = with(Mock::new(s16_dir()).hang_on("/api/getGamepadOptions"));
    let err = commands::backup_bundle(&client, captured()).unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    let last = mock.requests().last().unwrap().path;
    assert_eq!(last, "/api/getGamepadOptions");
}

#[test]
fn reboot_posts_the_boot_mode() {
    for (mode, value) in [
        (BootMode::Gamepad, 0),
        (BootMode::Webconfig, 1),
        (BootMode::Bootsel, 2),
    ] {
        let (client, mock) = s16();
        let r = commands::reboot(&client, mode).unwrap();
        assert_eq!(r.mode, mode);
        assert_eq!(r.host, "mock:s16-v0.7.12");
        let last = mock.requests().pop().unwrap();
        assert_eq!(last.method, Method::Post);
        assert_eq!(last.path, "/api/reboot");
        let body: Value = serde_json::from_slice(&last.body.unwrap()).unwrap();
        assert_eq!(body, json!({ "bootMode": value }));
    }
}

#[test]
fn a_pad_without_web_config_answers_404() {
    let err = Client::open(Box::new(Mock::new(fixtures().join("api/missing"))))
        .err()
        .unwrap();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert!(
        err.to_string().contains("getFirmwareVersion: HTTP 404"),
        "{err}"
    );
}

#[test]
fn connect_needs_web_config() {
    let root = |name: &str| Root::new(fixtures().join("sysfs").join(name));
    let err = client::require_webconfig(&root("s16-xinput")).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("XInput mode, not web-config"), "{msg}");
    assert!(msg.contains("S2+B3+B4 for 4 seconds"), "{msg}");
    let err = client::require_webconfig(&root("no-pad")).unwrap_err();
    assert_eq!(err.exit_code(), exit::NOT_FOUND);
    let dev = client::require_webconfig(&root("two-pads")).unwrap();
    assert_eq!(dev.mode, Some(InputMode::Webconfig));
    assert!(client::require_webconfig(&root("s16-webconfig")).is_ok());
}
