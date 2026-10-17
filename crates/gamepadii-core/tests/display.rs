//! `display layout` against the S16 capture: the Haute42 model table, the
//! write (through the mock's `setDisplayOptions`), the trip from a gamepad
//! mode, and the `info` hint.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use gamepadii_core::api::{DisplayOptions, PinMappings};
use gamepadii_core::client::Client;
use gamepadii_core::display::{self, HAUTE42, HAUTE42_BOARD, LayoutArg};
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Env, Silent, Timeouts};
use gamepadii_core::mode::InputMode;
use gamepadii_core::sim::SimPad;
use gamepadii_core::transport::Method;
use gamepadii_core::transport::mock::Mock;
use gamepadii_core::wait::ManualClock;
use serde_json::{Value, json};

fn s16_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(s16_dir().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

const NOW: u64 = 1_792_256_223;

fn named(s: &str) -> LayoutArg {
    LayoutArg::Named(s.into())
}

#[test]
fn the_model_table_follows_board_config_h() {
    let h = Some(HAUTE42_BOARD);
    for (name, want) in [
        ("s16", (36, 42)),
        ("G16", (36, 42)),
        ("alt2", (36, 42)),
        ("s13", (34, 40)),
        ("g13", (34, 40)),
        ("m13+", (35, 41)),
        ("t13", (35, 41)),
        ("M16plus", (37, 43)),
        ("t16", (37, 43)),
        ("alt4", (38, 44)),
        ("e16", (39, 45)),
        ("default", (27, 31)),
    ] {
        assert_eq!(display::resolve(&named(name), h).unwrap(), want, "{name}");
    }
    // Alternate n is buttonLayout 34+n and buttonLayoutRight 40+n.
    for (n, l) in HAUTE42.iter().skip(1).enumerate() {
        assert_eq!(
            (l.left, l.right),
            (34 + n as i64, 40 + n as i64),
            "{}",
            l.key
        );
    }
}

#[test]
fn names_and_values_are_checked() {
    let h = Some(HAUTE42_BOARD);
    let err = display::resolve(&named("s17"), h).unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("s16"), "{err}");
    // Model names are Haute42's only; raw values work anywhere.
    let err = display::resolve(&named("s16"), Some("Pico")).unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("--left N --right N"), "{err}");
    let raw = LayoutArg::Raw {
        left: 36,
        right: 42,
    };
    assert_eq!(display::resolve(&raw, Some("Pico")).unwrap(), (36, 42));
    for (left, right) in [(-1, 0), (42, 0), (0, 48)] {
        let err = display::precheck(&LayoutArg::Raw { left, right }).unwrap_err();
        assert_eq!(err.exit_code(), exit::USAGE);
    }
    assert!(display::precheck(&named("nope")).is_err());
    assert!(display::precheck(&named("s16")).is_ok());
}

fn s16() -> (Client, Arc<Mock>) {
    let mock = Arc::new(Mock::new(s16_dir()));
    (Client::open(Box::new(Shared(mock.clone()))).unwrap(), mock)
}

struct Shared(Arc<Mock>);

impl gamepadii_core::transport::Transport for Shared {
    fn request(
        &self,
        m: Method,
        p: gamepadii_core::transport::ApiPath,
        b: Option<&[u8]>,
    ) -> gamepadii_core::Result<gamepadii_core::transport::Reply> {
        self.0.request(m, p, b)
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
}

#[test]
fn status_shows_the_current_layout_and_the_choices() {
    let (client, _) = s16();
    let s = display::layout_status(&client).unwrap();
    assert_eq!(s.board.as_deref(), Some(HAUTE42_BOARD));
    assert_eq!((s.current.left, s.current.right), (Some(27), Some(31)));
    assert_eq!(s.current.layout.unwrap().key, "default");
    assert_eq!(s.choices.len(), HAUTE42.len());
    let current: Vec<_> = s.choices.iter().filter(|c| c.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].layout.key, "default");
}

#[test]
fn set_layout_backs_up_writes_and_reads_back() {
    let (client, mock) = s16();
    let dir = tempfile::tempdir().unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(NOW);
    let r = display::set_layout(&client, &named("s16"), Some(dir.path()), now).unwrap();
    assert_eq!((r.before.left, r.before.right), (Some(27), Some(31)));
    assert_eq!((r.after.left, r.after.right), (Some(36), Some(42)));
    assert_eq!(r.after.layout.unwrap().key, "alt2");
    let w = r.write.unwrap();
    assert!(w.posted);
    assert_eq!(w.changed.len(), 2);
    let backup = r.backup.unwrap();
    assert!(backup.exists());
    let bundle: Value = serde_json::from_str(&std::fs::read_to_string(&backup).unwrap()).unwrap();
    assert_eq!(
        bundle["endpoints"]["getDisplayOptions"]["buttonLayout"], 27,
        "the backup has the layout before the write"
    );
    // The backup's reads come before the one write, and nothing else is set.
    let log = mock.requests();
    let posts: Vec<_> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| r.method == Method::Post)
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].1.path, "/api/setDisplayOptions");
    assert!(
        log[..posts[0].0]
            .iter()
            .any(|r| r.path == "/api/getWiiControls"),
        "backed up first"
    );
    let stored = mock.current("getDisplayOptions").unwrap();
    let mut want = fixture("getDisplayOptions");
    want["buttonLayout"] = json!(36);
    want["buttonLayoutRight"] = json!(42);
    assert_eq!(stored, want);
}

#[test]
fn the_layout_it_has_already_is_neither_backed_up_nor_written() {
    let (client, mock) = s16();
    let dir = tempfile::tempdir().unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(NOW);
    let raw = LayoutArg::Raw {
        left: 27,
        right: 31,
    };
    let r = display::set_layout(&client, &raw, Some(dir.path()), now).unwrap();
    assert_eq!(r.write, None);
    assert_eq!(r.backup, None);
    assert!(mock.requests().iter().all(|r| r.method == Method::Get));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn another_board_takes_raw_values_only() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["getDisplayOptions", "getPinMappings"] {
        std::fs::copy(
            s16_dir().join(format!("{name}.json")),
            dir.path().join(format!("{name}.json")),
        )
        .unwrap();
    }
    std::fs::write(
        dir.path().join("getFirmwareVersion.json"),
        r#"{"version":"v0.7.12","boardConfig":"Pico"}"#,
    )
    .unwrap();
    let client = Client::open(Box::new(Mock::new(dir.path()))).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(NOW);
    let err = display::set_layout(&client, &named("s16"), None, now).unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    let s = display::layout_status(&client).unwrap();
    assert!(s.choices.is_empty());
    assert_eq!(s.current.layout, None);
    let raw = LayoutArg::Raw { left: 1, right: 1 };
    let r = display::set_layout(&client, &raw, None, now).unwrap();
    assert_eq!((r.after.left, r.after.right), (Some(1), Some(1)));
}

/// From XInput with the control channel: REBOOT into web-config, back up,
/// write, then `/api/reboot` back, after the write's reply.
#[test]
fn from_a_gamepad_mode_it_goes_through_web_config() {
    let pad = Arc::new(SimPad::new(s16_dir(), InputMode::Xinput).with_channel());
    let clock = ManualClock::at(NOW);
    let dirs = tempfile::tempdir().unwrap();
    let p = pad.clone();
    let open = move || p.open();
    let p = pad.clone();
    let control = move |d: &gamepadii_core::discovery::Device| p.open_control(d);
    let env = Env {
        scan: &*pad,
        clock: &clock,
        open: &open,
        control: Some(&control),
        cache_dir: Some(dirs.path().join("cache")),
        backups_dir: None,
        url: "http://192.168.7.1/".into(),
        timeouts: Timeouts::default(),
        poll: Duration::from_millis(200),
    };
    let backups = dirs.path().join("backups");
    let (r, trip) = flow::through_webconfig(
        &env,
        false,
        &mut Silent,
        |c| display::set_layout(c, &named("s16"), Some(&backups), env.clock.wall()),
        |_| "the layout is saved".into(),
    )
    .unwrap();
    assert!(r.write.is_some() && r.backup.is_some());
    let t = trip.unwrap();
    assert!(t.verified);
    assert_eq!(pad.mode(), Some(InputMode::Xinput));
    let posts: Vec<_> = pad
        .mock()
        .requests()
        .into_iter()
        .filter(|r| r.method == Method::Post)
        .map(|r| r.path)
        .collect();
    assert_eq!(posts, ["/api/setDisplayOptions", "/api/reboot"]);
    assert_eq!(
        pad.mock().current("getDisplayOptions").unwrap()["buttonLayout"],
        36
    );
}

fn display_options(left: i64, right: i64) -> DisplayOptions {
    let mut d: DisplayOptions = serde_json::from_value(fixture("getDisplayOptions")).unwrap();
    d.button_layout = Some(left);
    d.button_layout_right = Some(right);
    d
}

#[test]
fn info_hints_at_the_layout_for_a_16_button_mapping() {
    let base: PinMappings = serde_json::from_value(fixture("getPinMappings")).unwrap();
    let h = Some(HAUTE42_BOARD);
    let hint = display::generic_layout_hint(h, &display_options(27, 31), &base).unwrap();
    assert!(hint.contains("gamepadii display layout s16"), "{hint}");
    // Already on a board drawing, another board, or a pin left unmapped.
    assert_eq!(
        display::generic_layout_hint(h, &display_options(36, 42), &base),
        None
    );
    assert_eq!(
        display::generic_layout_hint(Some("Pico"), &display_options(27, 31), &base),
        None
    );
    let mut twelve = base.clone();
    twelve.extra["pin26"]["action"] = json!(-10);
    assert_eq!(
        display::generic_layout_hint(h, &display_options(27, 31), &twelve),
        None
    );
}

#[test]
fn info_reports_the_layout_and_the_hint() {
    let (client, _) = s16();
    let info = gamepadii_core::commands::info(&client).unwrap();
    let d = info.display_layout.unwrap();
    assert_eq!((d.left, d.right), (Some(27), Some(31)));
    assert_eq!(info.hints.len(), 1);
}
