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
        output::info(Style::plain(format), &commands::info(&s16()).unwrap(), None)
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
            &backup_report(),
            None
        ));
    }

    #[test]
    fn json_backup() {
        insta::assert_snapshot!(output::backup(
            Style::plain(Format::Json),
            &backup_report(),
            None
        ));
    }

    /// The S16 on the fork build (`tests/fixtures/api/s16-fork`): the
    /// backup has `getConfig` too.
    fn fork_backup_report() -> BackupReport {
        let fork =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-fork");
        let client = Client::open(Box::new(Mock::new(fork))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_792_256_223);
        let mut r = commands::backup(&client, Some(&dir.path().join("s16.json")), now).unwrap();
        r.path = PathBuf::from("/backups/s16.json");
        r
    }

    #[test]
    fn human_backup_on_the_fork() {
        insta::assert_snapshot!(output::backup(
            Style::plain(Format::Human),
            &fork_backup_report(),
            None
        ));
    }

    #[test]
    fn json_backup_on_the_fork() {
        insta::assert_snapshot!(output::backup(
            Style::plain(Format::Json),
            &fork_backup_report(),
            None
        ));
    }

    /// The S16 on stock upstream main (`tests/fixtures/api/s16-main`,
    /// synthetic): info, backup and leds on the main line.
    fn main_client() -> Client {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main");
        Client::open(Box::new(Mock::new(dir))).unwrap()
    }

    #[test]
    fn main_firmware_outputs() {
        let client = main_client();
        let info = commands::info(&client).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_792_256_223);
        let mut backup =
            commands::backup(&client, Some(&dir.path().join("s16.json")), now).unwrap();
        backup.path = PathBuf::from("/backups/s16.json");
        let leds = gamepadii_core::leds::read_webconfig(&client).unwrap();
        let human = Style::plain(Format::Human);
        let json = Style::plain(Format::Json);
        insta::assert_snapshot!(
            [
                output::info(human, &info, None),
                output::backup(human, &backup, None),
                output::leds(human, &leds, None),
                output::leds(json, &leds, None),
            ]
            .join("\n")
        );
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
    use gamepadii_core::commands::{self, Firmware};
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

        fn status(&self) -> gamepadii_core::commands::Status {
            let pad = self.pad.clone();
            let control = move |d: &gamepadii_core::discovery::Device| pad.open_control(d);
            gamepadii_core::commands::status_with(&*self.pad, Some(&control), None).unwrap()
        }

        fn run<T>(&self, f: impl FnOnce(&Env) -> T) -> T {
            let pad = self.pad.clone();
            let open = move || pad.open();
            let pad = self.pad.clone();
            let control = move |d: &gamepadii_core::discovery::Device| pad.open_control(d);
            let env = Env {
                scan: &*self.pad,
                clock: &self.clock,
                open: &open,
                control: Some(&control),
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

    // ---- the control channel ----------------------------------------------

    fn cc(mode: InputMode) -> Rig {
        Rig::new(SimPad::new(s16_dir(), mode).with_channel())
    }

    #[test]
    fn status_with_the_channel() {
        let rig = cc(InputMode::Xinput);
        let s = rig.status();
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}",
            output::status(human(), &s),
            output::status(json(), &s),
            output::status(Style::plain(Format::Waybar), &s)
        ));
    }

    #[test]
    fn mode_over_the_channel() {
        let r = cc(InputMode::Xinput).mode(InputMode::Keyboard, &mut Silent);
        insta::assert_snapshot!(format!(
            "{}\n{}",
            output::mode(human(), &r),
            output::mode(json(), &r)
        ));
    }

    #[test]
    fn webconfig_over_the_channel() {
        let r = cc(InputMode::Xinput)
            .run(|env| flow::webconfig(env, true, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(output::webconfig(human(), &r));
    }

    #[test]
    fn reboot_over_the_channel() {
        let rig = cc(InputMode::Xinput);
        let r = rig
            .run(|env| flow::reboot_pad(env, BootMode::Webconfig, true, &mut Silent))
            .unwrap();
        insta::assert_snapshot!(format!(
            "{}\n{}",
            output::reboot(human(), &r),
            output::reboot(json(), &r)
        ));
    }

    #[test]
    fn profile_switch_outputs() {
        let rig = cc(InputMode::Xinput);
        let switch = |which: &str, save| {
            let which: flow::ProfileArg = which.parse().unwrap();
            rig.run(|env| flow::profile_switch(env, &which, save, &mut Silent))
                .unwrap()
        };
        let ram = switch("3", false);
        let saved = switch("dkc", true);
        let already = switch("4", false);
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}",
            output::profile_switch(human(), &ram),
            output::profile_switch(human(), &saved),
            output::profile_switch(human(), &already),
            output::profile_switch(json(), &ram)
        ));
    }

    /// `info` and `backup` from XInput: REBOOT into web-config, read,
    /// `/api/reboot` back. The progress on stderr, then the result.
    #[test]
    fn info_and_backup_through_web_config() {
        let rig = cc(InputMode::Xinput);
        let mut stderr = Vec::new();
        let (info, trip) = {
            let mut p = Reporter::new(&mut stderr, false);
            rig.run(|env| flow::info(env, false, &mut p)).unwrap()
        };
        let out = rig.dirs.path().join("s16.json");
        let (mut backup, stayed) = rig
            .run(|env| {
                flow::through_webconfig(
                    env,
                    true,
                    &mut Silent,
                    |c| commands::backup(c, Some(&out), env.clock.wall()),
                    |_| String::new(),
                )
            })
            .unwrap();
        backup.path = PathBuf::from("/backups/s16.json");
        insta::assert_snapshot!(format!(
            "--- stderr\n{}--- info\n{}--- backup --stay\n{}",
            String::from_utf8(stderr).unwrap(),
            output::info(human(), &info, trip.as_ref()),
            output::backup(human(), &backup, stayed.as_ref())
        ));
        let v: Value = serde_json::from_str(&output::info(json(), &info, trip.as_ref())).unwrap();
        insta::assert_snapshot!(serde_json::to_string_pretty(&v["trip"]).unwrap());
    }

    /// A RAM-only profile the restart dropped: switched back to, or a
    /// warning when that fails. The progress on stderr, then the rows the
    /// trip adds and the JSON.
    #[test]
    fn info_restores_a_ram_only_profile() {
        let run = |pad: SimPad| {
            let rig = Rig::new(pad);
            let mut stderr = Vec::new();
            let (info, trip) = {
                let mut p = Reporter::new(&mut stderr, false);
                rig.run(|env| flow::info(env, false, &mut p)).unwrap()
            };
            let human = output::info(human(), &info, trip.as_ref());
            let rows = &human[human.find("profiles ").unwrap()..];
            let v: Value =
                serde_json::from_str(&output::info(json(), &info, trip.as_ref())).unwrap();
            format!(
                "--- stderr\n{}--- rows\n{rows}--- json\nrestored_profile: {}\n\
                 saved_profile: {}\nrunning_profile: {}\nrunning: {}\n",
                String::from_utf8(stderr).unwrap(),
                v["trip"]["restored_profile"],
                v["saved_profile"],
                v["running_profile"],
                v["profiles"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p["running"].to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        let channel = || SimPad::new(s16_dir(), InputMode::Xinput).with_channel();
        insta::assert_snapshot!(format!(
            "{}{}{}",
            run(channel().channel(|c| c.active = 3)),
            run(channel().channel(|c| {
                c.active = 3;
                c.enabled_profiles &= !0b100;
            })),
            // No STATUS: the running profile is unknown, and says so.
            run(channel().channel(|c| {
                c.caps = gamepadii_core::channel::Caps(
                    gamepadii_core::channel::sim::V1_CAPS & !gamepadii_core::channel::Caps::STATUS,
                );
            }))
        ));
    }

    /// What `info` says when the trip goes wrong: the pad is sent back
    /// whenever it can be, and the message says so.
    #[test]
    fn info_through_web_config_failures() {
        let run = |pad: SimPad| {
            let rig = Rig::new(pad);
            let err = rig
                .run(|env| {
                    flow::through_webconfig(env, false, &mut Silent, commands::info, |_| {
                        "the pad was read".to_string()
                    })
                })
                .unwrap_err();
            format!("exit {}: {err}", err.exit_code())
        };
        let channel = || SimPad::new(s16_dir(), InputMode::Xinput).with_channel();
        insta::assert_snapshot!(
            [
                run(SimPad::new(s16_dir(), InputMode::Xinput)),
                run(channel().channel(|c| c.forced_setup = 2)),
                run(channel().stays_away()),
                run(channel().server_down()),
                run(channel().stays_away_after(1)),
                run(channel().come_back_as(InputMode::Ps3)),
            ]
            .join("\n")
        );
    }

    #[test]
    fn udev_setup() {
        let dir = tempfile::tempdir().unwrap();
        let r = gamepadii_core::udev::report(&gamepadii_core::sysfs::Root::new(dir.path()));
        insta::assert_snapshot!(output::udev(human(), &r));
    }

    #[test]
    fn udev_rule_file() {
        insta::assert_snapshot!(gamepadii_core::udev::rule());
    }

    /// `display layout`: the choices, a write in web-config, one that was
    /// set already, and one from XInput through web-config; then the JSON.
    #[test]
    fn display_layout_outputs() {
        use gamepadii_core::display::{self, LayoutArg};
        let s16 = || LayoutArg::Named("s16".into());
        let backups = PathBuf::from("/backups");
        let set = |rig: &Rig, stay: bool| {
            rig.run(|env| {
                let dir = rig.dirs.path().join("backups");
                flow::through_webconfig(
                    env,
                    stay,
                    &mut Silent,
                    |c| display::set_layout(c, &s16(), Some(&dir), env.clock.wall()),
                    |_| String::new(),
                )
            })
            .map(|(mut r, t)| {
                if let Some(b) = &mut r.backup {
                    *b = backups.join(b.file_name().unwrap());
                }
                (r, t)
            })
            .unwrap()
        };
        let rig = Rig::s16(InputMode::Webconfig);
        let (status, _) = rig
            .run(|env| {
                flow::through_webconfig(env, false, &mut Silent, display::layout_status, |_| {
                    String::new()
                })
            })
            .unwrap();
        let (written, _) = set(&rig, false);
        let (already, _) = set(&rig, false);
        let (after_status, _) = rig
            .run(|env| {
                flow::through_webconfig(env, false, &mut Silent, display::layout_status, |_| {
                    String::new()
                })
            })
            .unwrap();
        let rig = cc(InputMode::Xinput);
        let (tripped, trip) = set(&rig, false);
        insta::assert_snapshot!(format!(
            "--- choices\n{}--- set in web-config\n{}--- already\n{}--- choices after\n{}\
             --- from XInput\n{}--- json\n{}",
            output::display_layout_status(human(), &status, None),
            output::display_layout(human(), &written, None),
            output::display_layout(human(), &already, None),
            output::display_layout_status(human(), &after_status, None),
            output::display_layout(human(), &tripped, trip.as_ref()),
            output::display_layout(json(), &written, None),
        ));
        let v: Value =
            serde_json::from_str(&output::display_layout_status(json(), &status, None)).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["current"]["layout"]["key"], "default");
        assert_eq!(v["choices"][3]["key"], "alt2");
        assert_eq!(v["choices"][3]["models"], serde_json::json!(["G16", "S16"]));
        assert!(v["trip"].is_null());
    }

    fn fork_dir() -> PathBuf {
        s16_dir().join("../s16-fork")
    }

    /// `leds` over each route: LED_STATUS, stock web-config, the fork's
    /// getConfig through a trip; and waybar.
    #[test]
    fn leds_outputs() {
        let read = |rig: &Rig| rig.run(|env| flow::leds(env, false, &mut Silent)).unwrap();
        let live = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
        let (channel, _) = read(&live);
        let (stock, _) = read(&Rig::s16(InputMode::Webconfig));
        let fork = Rig::new(SimPad::new(fork_dir(), InputMode::Xinput).with_channel());
        let (forked, trip) = read(&fork);
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            output::leds(human(), &channel, None),
            output::leds(human(), &stock, None),
            output::leds(human(), &forked, trip.as_ref()),
            output::waybar_leds(&channel).to_line(),
            output::leds(json(), &channel, None),
            output::leds(json(), &stock, None),
        ));
    }

    #[test]
    fn leds_set_outputs() {
        use gamepadii_core::leds::{LedChange, Level};
        let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
        let set = |c: LedChange| rig.run(|env| flow::leds_set(env, &c)).unwrap();
        let ram = set(LedChange {
            animation: Some("static-theme".parse().unwrap()),
            theme: Some("tekken".parse().unwrap()),
            brightness: Some(Level::Up),
            ..LedChange::default()
        });
        let saved = set(LedChange {
            save: true,
            ..LedChange::default()
        });
        let top = set(LedChange {
            brightness: Some(Level::To(5)),
            save: true,
            ..LedChange::default()
        });
        let already = set(LedChange {
            brightness: Some(Level::Up),
            ..LedChange::default()
        });
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}",
            output::leds_set(human(), &ram),
            output::leds_set(human(), &saved),
            output::leds_set(human(), &top),
            output::leds_set(human(), &already),
            output::leds_set(json(), &ram),
        ));
    }

    #[test]
    fn slot_outputs() {
        use gamepadii_core::slots::{SlotFile, SlotWrite};
        let rig = Rig::new(
            SimPad::new(s16_dir(), InputMode::Xinput)
                .with_channel()
                .channel(|c| *c = c.clone().with_slots()),
        );
        let show = rig.run(|env| flow::profile_show(env, None)).unwrap();
        let file = SlotFile::parse(
            r#"{"label": "Hades", "pins": [{"gpio": 2, "action": "B2"}, {"gpio": 0, "action": 7}]}"#,
        )
        .unwrap();
        let ram = rig
            .run(|env| {
                let write = SlotWrite {
                    slot: 6,
                    select: true,
                    ..SlotWrite::default()
                };
                flow::profile_write(env, &file, &write, &mut Silent)
            })
            .unwrap();
        let saved = rig
            .run(|env| {
                let write = SlotWrite {
                    slot: 5,
                    save: true,
                    ..SlotWrite::default()
                };
                flow::profile_write(env, &file, &write, &mut Silent)
            })
            .unwrap();
        // The backup path has a temporary directory in it.
        let plain = |r: &gamepadii_core::slots::SlotWriteReport| {
            let mut r = r.clone();
            r.backup = r.backup.map(|p| PathBuf::from(p.file_name().unwrap()));
            r
        };
        let show_json = output::profile_show(json(), &show);
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}",
            output::profile_show(human(), &show),
            output::profile_write(human(), &plain(&ram)),
            output::profile_write(human(), &plain(&saved)),
            &show_json[..show_json.find("\"pins\"").unwrap()],
            output::profile_write(json(), &plain(&ram)),
        ));
    }

    #[test]
    fn lights_profile_outputs() {
        use gamepadii_core::lights::{LightsFile, LightsWrite};
        let rig = Rig::new(
            SimPad::new(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main"),
                InputMode::Xinput,
            )
            .with_lights()
            .channel(|c| *c = c.clone().with_lights_profiles()),
        );
        let show = rig.run(|env| flow::lights_show(env, None)).unwrap();
        let file = LightsFile::parse(
            r#"{"idle_effect": "rain", "pressed_colors": [2, 2, 3, 3, 3, 13], "case_in_pressed": true}"#,
        )
        .unwrap();
        let write = |profile, select, save| {
            rig.run(|env| {
                let w = LightsWrite {
                    profile,
                    select,
                    save,
                };
                flow::lights_write(env, &file, &w, &mut Silent)
            })
            .unwrap()
        };
        let ram = write(2, true, false);
        let saved = write(3, false, true);
        let shown = rig.run(|env| flow::lights_show(env, Some(2))).unwrap();
        // The backup path has a temporary directory in it.
        let plain = |r: &gamepadii_core::lights::LightsWriteReport| {
            let mut r = r.clone();
            r.backup = r.backup.map(|p| PathBuf::from(p.file_name().unwrap()));
            r
        };
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            output::lights_show(human(), &show),
            output::lights_show(human(), &shown),
            output::lights_write(human(), &plain(&ram)),
            output::lights_write(human(), &plain(&saved)),
            output::lights_show(json(), &show),
            output::lights_write(json(), &plain(&ram)),
        ));
    }

    #[test]
    fn input_outputs() {
        use gamepadii_core::channel::sim::SimInput;
        let rig = |frames: std::collections::VecDeque<u64>| {
            Rig::new(
                SimPad::new(s16_dir(), InputMode::Xinput)
                    .with_channel()
                    .channel(|c| {
                        *c = c.clone().with_slots().with_input();
                        c.input.functions = 1 << 25;
                        c.input.frames = frames;
                    }),
            )
        };
        let rest = rig([0].into()).run(flow::input).unwrap();
        let held = rig([1 << 6 | 1 << 14 | 1 << 27].into())
            .run(flow::input)
            .unwrap();
        let mut stderr = Vec::new();
        let found = {
            let mut p = Reporter::new(&mut stderr, false);
            let find = flow::FindPin {
                scan: true,
                ..flow::FindPin::default()
            };
            rig(SimInput::press(1 << 22, 3, 4))
                .run(|env| flow::find_pin(env, &find, &mut p))
                .unwrap()
        };
        let up = rig(SimInput::press(1 << 27, 3, 4))
            .run(|env| flow::find_pin(env, &flow::FindPin::default(), &mut Silent))
            .unwrap();
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n--- stderr\n{}--- stdout\n{}\n{}\n{}\n{}",
            output::input(human(), &rest),
            output::input(human(), &held),
            output::input_line(human(), &rest),
            output::input_line(human(), &held),
            String::from_utf8(stderr).unwrap(),
            output::find_pin(human(), &found),
            output::find_pin(human(), &up),
            output::input_line(json(), &held),
            output::find_pin(json(), &found),
        ));
    }

    #[test]
    fn lights_outputs() {
        use gamepadii_core::leds::{LedChange, Level, LightsPick};
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main");
        let rig = Rig::new(SimPad::new(dir, InputMode::Xinput).with_lights());
        let (read, _) = rig.run(|env| flow::leds(env, false, &mut Silent)).unwrap();
        let set = |c: LedChange| rig.run(|env| flow::leds_set(env, &c)).unwrap();
        let ram = set(LedChange {
            lights_profile: Some(LightsPick::Number(3)),
            brightness: Some(Level::Down),
            ..LedChange::default()
        });
        let off = set(LedChange {
            lights_profile: Some(LightsPick::Off),
            save: true,
            ..LedChange::default()
        });
        let (dark, _) = rig.run(|env| flow::leds(env, false, &mut Silent)).unwrap();
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            output::leds(human(), &read, None),
            output::leds(human(), &dark, None),
            output::waybar_leds(&read).to_line(),
            output::waybar_leds(&dark).to_line(),
            output::leds_set(human(), &ram),
            output::leds_set(human(), &off),
            output::leds(json(), &read, None),
            output::leds_set(json(), &ram),
        ));
    }

    #[test]
    fn log_outputs() {
        use gamepadii_core::channel::proto::log_code as c;
        let rig = Rig::new(
            SimPad::new(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main"),
                InputMode::Xinput,
            )
            .with_lights()
            .channel(|ch| {
                *ch = ch.clone().with_lights_profiles().with_log();
                ch.panic("test 1 22 333 4444 abcde");
            }),
        );
        let s = rig.status();
        rig.pad.change_firmware(|ch| {
            ch.add_log(c::PROFILE, &[254, 2]);
            ch.add_log(c::COMMAND, &[0x10, 6, 0x8d, 0xb5]);
            ch.add_log(c::COMMAND, &[0x30, 9, 0, 0]);
            ch.add_log(c::SAVE, &[1, 0]);
            ch.add_log(c::RESTART, &[2]);
            ch.add_log(c::ERROR, b"no flash");
            ch.add_log(c::USB_HOST, &[1, 1, 0x5e, 0x04, 0x8e, 0x02]);
            ch.add_log(9, &[1, 2, 3]);
        });
        let all = rig.run(|env| flow::log(env, None)).unwrap();
        rig.pad.change_firmware(|ch| {
            for _ in 0..70 {
                ch.add_log(c::SAVE, &[0, 0]);
            }
        });
        let lost = rig.run(|env| flow::log(env, Some(all.next))).unwrap();
        let mut tail = rig.run(|env| flow::log(env, Some(lost.next - 2))).unwrap();
        tail.lost = 0;
        insta::assert_snapshot!(format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            output::log(human(), &all),
            output::log_follow(human(), &tail, false),
            output::log_follow(human(), &lost, false)
                .lines()
                .next()
                .unwrap(),
            output::status(human(), &s),
            output::status(Style::plain(Format::Waybar), &s),
            output::log_follow(json(), &tail, false),
            output::log(json(), &all),
            output::status(json(), &s),
        ));
    }

    #[test]
    fn config_blob_outputs() {
        use gamepadii_core::pb;
        let rig = Rig::new(
            SimPad::new(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main"),
                InputMode::Xinput,
            )
            .with_lights()
            .channel(|ch| {
                *ch = ch
                    .clone()
                    .with_slots()
                    .with_lights_profiles()
                    .with_log()
                    .with_config();
            }),
        );
        let file = rig.dirs.path().join("s16.json");
        let backup = rig
            .run(|env| flow::backup_channel(env, Some(&file)))
            .unwrap();
        let other = pb::encode(&[(
            2,
            pb::Value::Bytes(pb::encode(&[(1, pb::Value::Varint(1))])),
        )]);
        rig.pad.change_firmware(|ch| {
            let c = ch.config.as_mut().unwrap();
            c.saved = other;
            c.save_after_chunks = Some(3);
        });
        let bundle = commands::read_backup(&file).unwrap();
        let restore = |rig: &Rig| {
            rig.run(|env| flow::restore(env, &file, &bundle, &mut Silent))
                .unwrap()
        };
        let written = restore(&rig);
        let unchanged = restore(&rig);
        let dir = rig.dirs.path().display().to_string();
        let text = format!(
            "{}\n{}\n{}\n{}\n{}",
            output::backup(human(), &backup, None),
            output::backup(json(), &backup, None),
            output::restore(human(), &written),
            output::restore(human(), &unchanged),
            output::restore(json(), &written),
        )
        .replace(&dir, "/tmp");
        insta::assert_snapshot!(text);
    }
}
