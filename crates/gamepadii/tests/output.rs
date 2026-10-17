//! `status` rendered from the sysfs fixtures (see gamepadii-core's
//! `tests/discovery.rs` for where each tree comes from).

use std::path::PathBuf;

use gamepadii::output::{self, Format, Style};
use gamepadii_core::commands;
use gamepadii_core::sysfs::Root;

fn status(fixture: &str, format: Format) -> String {
    let root = Root::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sysfs")
            .join(fixture),
    );
    output::status(Style::plain(format), &commands::status(&root).unwrap())
}

fn waybar(fixture: &str) -> serde_json::Value {
    let line = status(fixture, Format::Waybar);
    assert!(!line.contains('\n'), "waybar output must be one line");
    serde_json::from_str(&line).unwrap()
}

#[test]
fn human_s16_webconfig() {
    insta::assert_snapshot!(status("s16-webconfig", Format::Human));
}

#[test]
fn human_s16_xinput() {
    insta::assert_snapshot!(status("s16-xinput", Format::Human));
}

#[test]
fn human_two_pads() {
    insta::assert_snapshot!(status("two-pads", Format::Human));
}

#[test]
fn json_s16_webconfig() {
    insta::assert_snapshot!(status("s16-webconfig", Format::Json));
}

#[test]
fn json_s16_xinput() {
    insta::assert_snapshot!(status("s16-xinput", Format::Json));
}

#[test]
fn json_keeps_nulls() {
    let v: serde_json::Value = serde_json::from_str(&status("bootsel", Format::Json)).unwrap();
    assert_eq!(v["schema"], 1);
    let d = &v["devices"][0];
    assert_eq!(d["mode"], "bootsel");
    assert!(d.get("netdev").is_some_and(|n| n.is_null()));
    let v: serde_json::Value = serde_json::from_str(&status("two-pads", Format::Json)).unwrap();
    let d = &v["devices"][1];
    assert!(d["mode"].is_null());
    assert_eq!(d["candidates"], serde_json::json!(["ps4", "ps5"]));
    assert!(d["serial"].is_null());
}

#[test]
fn waybar_lines() {
    let v = waybar("s16-xinput");
    assert_eq!(v["class"], "xinput");
    assert_eq!(v["alt"], "xinput");
    assert_eq!(v["text"], format!("{} XInput", output::GLYPH));
    assert!(
        v["tooltip"]
            .as_str()
            .unwrap()
            .starts_with("XInput (likely)\n")
    );
    let v = waybar("two-pads");
    assert_eq!(v["class"], "webconfig");
    assert_eq!(v["text"], format!("{} web-config +1", output::GLYPH));
    assert!(
        v["tooltip"]
            .as_str()
            .unwrap()
            .contains("PS4 or PS5 (likely)")
    );
}

#[test]
fn waybar_not_found() {
    let err = gamepadii_core::Error::NotFound("no device".into());
    let v: serde_json::Value = serde_json::from_str(&output::waybar_error(&err).to_line()).unwrap();
    assert_eq!(v["class"], "disconnected");
    assert_eq!(v["text"], output::GLYPH);
}

/// The web-config commands against the S16 capture
/// (`tests/fixtures/api/s16-v0.7.12`, described in gamepadii-core's
/// `tests/api.rs`).
mod webconfig {
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use gamepadii::output::{self, Format, Style};
    use gamepadii_core::client::Client;
    use gamepadii_core::commands::{self, BackupReport};
    use gamepadii_core::mode::BootMode;
    use gamepadii_core::transport::mock::Mock;

    fn s16() -> Client {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12");
        Client::open(Box::new(Mock::new(dir))).unwrap()
    }

    fn info(format: Format) -> String {
        output::info(Style::plain(format), &commands::info(&s16()).unwrap())
    }

    /// A report with a fixed path, as if written to `/backups/s16.json`.
    fn backup_report() -> BackupReport {
        let dir = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_792_256_223);
        let mut r = commands::backup(&s16(), Some(&dir.path().join("s16.json")), now).unwrap();
        r.path = PathBuf::from("/backups/s16.json");
        r
    }

    #[test]
    fn human_info() {
        insta::assert_snapshot!(info(Format::Human));
    }

    #[test]
    fn json_info() {
        insta::assert_snapshot!(info(Format::Json));
    }

    #[test]
    fn human_backup() {
        insta::assert_snapshot!(output::backup(
            Style::plain(Format::Human),
            &backup_report()
        ));
    }

    #[test]
    fn json_backup() {
        insta::assert_snapshot!(output::backup(Style::plain(Format::Json), &backup_report()));
    }

    #[test]
    fn reboot_without_waiting() {
        let human = |m| {
            let rig = super::flows::Rig::s16(gamepadii_core::mode::InputMode::Webconfig);
            output::reboot(Style::plain(Format::Human), &rig.reboot(m, false).unwrap())
        };
        insta::assert_snapshot!(
            [BootMode::Gamepad, BootMode::Webconfig, BootMode::Bootsel]
                .map(human)
                .join("\n")
        );
    }

    #[test]
    fn bytes() {
        assert_eq!(output::bytes(512), "512 B");
        assert_eq!(output::bytes(19_808), "19.3 KiB");
        assert_eq!(output::bytes(2_097_152), "2.0 MiB");
    }
}

/// The guided flows against a simulated S16 (`gamepadii_core::sim`) with a
/// manual clock: their human and JSON output, and the progress on stderr.
mod flows {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use gamepadii::output::{self, Format, Style};
    use gamepadii::progress::Reporter;
    use gamepadii_core::cache::{self, DeviceCache};
    use gamepadii_core::commands::Firmware;
    use gamepadii_core::flow::{self, Env, ModeReport, Progress, Silent, Timeouts};
    use gamepadii_core::mode::{BootMode, InputMode};
    use gamepadii_core::sim::{SimPad, UserAction};
    use gamepadii_core::wait::ManualClock;
    use serde_json::Value;

    const CAPTURED: u64 = 1_792_256_223;

    fn s16_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
    }

    fn fixture(name: &str) -> Value {
        let text = std::fs::read_to_string(s16_dir().join(format!("{name}.json"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    pub struct Rig {
        pad: Arc<SimPad>,
        clock: ManualClock,
        dirs: tempfile::TempDir,
    }

    impl Rig {
        pub fn new(pad: SimPad) -> Self {
            Self {
                pad: Arc::new(pad),
                clock: ManualClock::at(CAPTURED),
                dirs: tempfile::tempdir().unwrap(),
            }
        }

        pub fn s16(mode: InputMode) -> Self {
            Self::new(SimPad::new(s16_dir(), mode))
        }

        fn with_cache(self) -> Self {
            let fw: gamepadii_core::api::FirmwareVersion =
                serde_json::from_value(fixture("getFirmwareVersion")).unwrap();
            let c = DeviceCache::new(
                Firmware::from(&fw),
                std::time::UNIX_EPOCH + Duration::from_secs(CAPTURED),
                fixture("getGamepadOptions"),
                fixture("getPinMappings"),
                fixture("getProfileOptions"),
            );
            cache::store(&self.dirs.path().join("cache"), &c).unwrap();
            self
        }

        fn run<T>(&self, f: impl FnOnce(&Env) -> T) -> T {
            let pad = self.pad.clone();
            let open = move || pad.open();
            let env = Env {
                scan: &*self.pad,
                clock: &self.clock,
                open: &open,
                cache_dir: Some(self.dirs.path().join("cache")),
                backups_dir: Some(self.dirs.path().join("backups")),
                url: "http://192.168.7.1/".into(),
                timeouts: Timeouts::default(),
                poll: Duration::from_millis(200),
            };
            f(&env)
        }

        fn mode(&self, m: InputMode, p: &mut dyn Progress) -> ModeReport {
            let mut r = self.run(|env| flow::mode(env, m, true, p)).unwrap();
            if r.backup.is_some() {
                r.backup = Some(PathBuf::from("/backups/20261017-165703-Haute42COSMOX.json"));
            }
            r
        }

        pub fn reboot(
            &self,
            m: BootMode,
            wait: bool,
        ) -> gamepadii_core::Result<flow::RebootReport> {
            self.run(|env| {
                let client = (env.open)().unwrap();
                flow::reboot(env, &client, m, wait, &mut Silent)
            })
        }
    }

    fn human() -> Style {
        Style::plain(Format::Human)
    }

    fn json() -> Style {
        Style::plain(Format::Json)
    }

    #[test]
    fn human_mode_from_web_config() {
        let r = Rig::s16(InputMode::Webconfig).mode(InputMode::Keyboard, &mut Silent);
        insta::assert_snapshot!(output::mode(human(), &r));
    }

    #[test]
    fn json_mode_from_web_config() {
        let r = Rig::s16(InputMode::Webconfig).mode(InputMode::Keyboard, &mut Silent);
        insta::assert_snapshot!(output::mode(json(), &r));
    }

    #[test]
    fn human_mode_already() {
        let r = Rig::s16(InputMode::Xinput).mode(InputMode::Xinput, &mut Silent);
        insta::assert_snapshot!(output::mode(human(), &r));
    }

    #[test]
    fn human_mode_already_saved() {
        let r = Rig::s16(InputMode::Webconfig).mode(InputMode::Xinput, &mut Silent);
        insta::assert_snapshot!(output::mode(human(), &r));
    }

    /// From XInput, with the cache: the guide, the waits (one line each
    /// without a terminal), then the result.
    #[test]
    fn human_mode_guided_with_progress() {
        let rig = Rig::new(
            SimPad::new(s16_dir(), InputMode::Xinput).user(UserAction::Webconfig { after: 5 }),
        )
        .with_cache();
        let mut stderr = Vec::new();
        let r = {
            let mut p = Reporter::new(&mut stderr, false);
            rig.mode(InputMode::Keyboard, &mut p)
        };
        let stderr = String::from_utf8(stderr)
            .unwrap()
            .replace(&rig.dirs.path().display().to_string(), "[tmp]");
        insta::assert_snapshot!(format!(
            "--- stderr\n{stderr}--- stdout\n{}",
            output::mode(human(), &r)
        ));
    }

    #[test]
    fn human_mode_boot_button() {
        let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).user(
            UserAction::BootButton {
                after: 2,
                mode: InputMode::Ps5,
            },
        ))
        .with_cache();
        let r = rig.mode(InputMode::Ps5, &mut Silent);
        insta::assert_snapshot!(output::mode(human(), &r));
    }

    #[test]
    fn no_wait_instructions() {
        let rig = Rig::s16(InputMode::Xinput).with_cache();
        let err = rig
            .run(|env| flow::mode(env, InputMode::Switch, false, &mut Silent))
            .unwrap_err();
        insta::assert_snapshot!(err.to_string());
    }

    #[test]
    fn spinner_redraws_one_line() {
        let rig = Rig::new(
            SimPad::new(s16_dir(), InputMode::Xinput).user(UserAction::Webconfig { after: 1 }),
        );
        let mut stderr = Vec::new();
        {
            let mut p = Reporter::new(&mut stderr, true);
            rig.run(|env| flow::webconfig(env, true, &mut p)).unwrap();
        }
        let text = String::from_utf8(stderr).unwrap();
        assert!(
            text.contains("\r\x1b[K⠋ waiting for the pad in web-config, 60 s left"),
            "{text:?}"
        );
        assert!(
            text.ends_with("\r\x1b[K"),
            "the spinner line is cleared: {text:?}"
        );
    }

    #[test]
    fn human_mode_status_in_web_config() {
        let s = Rig::s16(InputMode::Webconfig)
            .run(|env| flow::mode_status(env, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(output::mode_status(human(), &s));
    }

    #[test]
    fn human_mode_status_from_the_cache() {
        let s = Rig::s16(InputMode::Xinput)
            .with_cache()
            .run(|env| flow::mode_status(env, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(output::mode_status(human(), &s));
    }

    #[test]
    fn json_mode_status_without_a_cache() {
        let s = Rig::s16(InputMode::Xinput)
            .run(|env| flow::mode_status(env, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(output::mode_status(json(), &s));
    }

    #[test]
    fn webconfig_outputs() {
        let rig = Rig::s16(InputMode::Webconfig);
        let r = rig
            .run(|env| flow::webconfig(env, true, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(format!(
            "{}\n{}",
            output::webconfig(human(), &r),
            output::webconfig(json(), &r)
        ));
    }

    #[test]
    fn reboot_waited() {
        let rig = Rig::s16(InputMode::Webconfig);
        let r = rig.reboot(BootMode::Gamepad, true).unwrap();
        insta::assert_snapshot!(format!(
            "{}\n{}",
            output::reboot(human(), &r),
            output::reboot(json(), &r)
        ));
    }
}
