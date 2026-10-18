//! The guided flows (`mode`, `webconfig`, `reboot` with a wait) against a
//! simulated pad (`gamepadii_core::sim`) over the S16 capture, with a
//! manual clock: every branch, no real sleeps.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gamepadii_core::api::{PinMappings, ProfileOptions, RebootAck};
use gamepadii_core::buttons::{self, Button, Combo, Profiles};
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::channel::sim::SimInput;
use gamepadii_core::commands::Firmware;
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Awaiting, Env, Progress, Step, Timeouts, Via};
use gamepadii_core::input::{FoundPin, NamesFrom};
use gamepadii_core::leds::{LedChange, LedReport, LedSetReport, Level, LightsPick, Pick};
use gamepadii_core::lights::{LightsFile, LightsWrite, LightsWriteReport};
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::sim::{self, SimPad, UserAction};
use gamepadii_core::slots::{SlotFile, SlotWrite, SlotWriteReport};
use gamepadii_core::transport::Method;
use gamepadii_core::transport::mock::Mock;
use gamepadii_core::wait::{Clock, ManualClock};
use serde_json::Value;

fn s16_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(s16_dir().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// 2026-10-17T16:57:03Z.
const CAPTURED: u64 = 1_792_256_223;

/// The S16 capture as a cache, taken at [`CAPTURED`].
fn s16_cache() -> DeviceCache {
    let fw: gamepadii_core::api::FirmwareVersion =
        serde_json::from_value(fixture("getFirmwareVersion")).unwrap();
    DeviceCache::new(
        Firmware::from(&fw),
        std::time::UNIX_EPOCH + Duration::from_secs(CAPTURED),
        fixture("getGamepadOptions"),
        fixture("getPinMappings"),
        fixture("getProfileOptions"),
    )
}

fn s16_profiles() -> Profiles {
    let base: PinMappings = serde_json::from_value(fixture("getPinMappings")).unwrap();
    let alts: ProfileOptions = serde_json::from_value(fixture("getProfileOptions")).unwrap();
    Profiles::new(base, alts)
}

/// Records every step, for asserting on.
#[derive(Default)]
struct Log {
    guides: Vec<Vec<String>>,
    waits: Vec<(Awaiting, Duration)>,
    other: Vec<String>,
}

impl Progress for Log {
    fn step(&mut self, step: Step<'_>) {
        match step {
            Step::Guide(g) => self.guides.push(g.lines()),
            Step::Waiting {
                awaiting, elapsed, ..
            } => self.waits.push((awaiting, elapsed)),
            Step::BackedUp(_) => self.other.push("backed up".into()),
            Step::Writing(m) => self.other.push(format!("writing {m}")),
            Step::Rebooting(m) => self.other.push(format!("rebooting {m}")),
            Step::CacheFailed(e) => self.other.push(format!("cache failed: {e}")),
            Step::RestoringProfile(n) => self.other.push(format!("restoring profile {n}")),
            Step::RestoreFailed { profile, error } => self
                .other
                .push(format!("profile {profile} not restored: {error}")),
            Step::PushButton { .. } => self.other.push("push a button".into()),
        }
    }
}

/// A pad, a clock, and the directories a flow writes to.
struct Rig {
    pad: Arc<SimPad>,
    clock: ManualClock,
    dirs: tempfile::TempDir,
}

impl Rig {
    fn new(pad: SimPad) -> Self {
        let dirs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dirs.path().join("cache")).unwrap();
        Self {
            pad: Arc::new(pad),
            clock: ManualClock::at(CAPTURED),
            dirs,
        }
    }

    /// The S16 in `mode`.
    fn s16(mode: InputMode) -> Self {
        Self::new(SimPad::new(s16_dir(), mode))
    }

    /// The S16 in `mode`, running our firmware with the control channel.
    fn cc(mode: InputMode) -> Self {
        Self::new(SimPad::new(s16_dir(), mode).with_channel())
    }

    /// Records that a pad with the channel has been seen (as an earlier
    /// `status` would have).
    fn knows_the_channel(self) -> Self {
        std::fs::write(
            self.cache_dir().join(cache::CHANNELS_FILE),
            r#"{"schema":1,"boards":{"Haute42COSMOX":{"major":1,"minor":0,"capabilities":["status"]}}}"#,
        )
        .unwrap();
        self
    }

    /// OUT commands the pad got on EP0, by command byte.
    fn channel_commands(&self) -> Vec<u8> {
        self.pad
            .control_log()
            .into_iter()
            .filter(|s| !s.setup.is_in())
            .map(|s| s.setup.command_byte())
            .collect()
    }

    fn with_cache(self) -> Self {
        cache::store(&self.cache_dir(), &s16_cache()).unwrap();
        self
    }

    fn cache_dir(&self) -> PathBuf {
        self.dirs.path().join("cache")
    }

    fn backups_dir(&self) -> PathBuf {
        self.dirs.path().join("backups")
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
            cache_dir: Some(self.cache_dir()),
            backups_dir: Some(self.backups_dir()),
            url: "http://192.168.7.1/".into(),
            timeouts: Timeouts::default(),
            poll: Duration::from_millis(200),
        };
        f(&env)
    }

    fn mode(
        &self,
        target: InputMode,
        wait: bool,
        log: &mut Log,
    ) -> gamepadii_core::Result<flow::ModeReport> {
        self.run(|env| flow::mode(env, target, wait, log))
    }

    fn requests(&self) -> Vec<(Method, &'static str)> {
        self.pad
            .mock()
            .requests()
            .into_iter()
            .map(|r| (r.method, r.path))
            .collect()
    }

    fn writes(&self) -> Vec<Value> {
        self.pad
            .mock()
            .requests()
            .into_iter()
            .filter(|r| r.path == "/api/setGamepadOptions")
            .map(|r| r.json())
            .collect()
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

// ---- mode ----------------------------------------------------------------

#[test]
fn already_in_the_mode_does_nothing() {
    let rig = Rig::s16(InputMode::Xinput);
    let r = rig
        .mode(InputMode::Xinput, true, &mut Log::default())
        .unwrap();
    assert_eq!(r.via, Via::Already);
    assert!(r.verified);
    assert!(r.write.is_none());
    assert!(rig.requests().is_empty(), "no API call in a gamepad mode");
    assert_eq!(rig.clock.elapsed(), Duration::ZERO);
}

#[test]
fn from_web_config_it_writes_reboots_and_verifies() {
    let rig = Rig::s16(InputMode::Webconfig);
    let mut log = Log::default();
    let r = rig.mode(InputMode::Keyboard, true, &mut log).unwrap();
    assert_eq!(r.via, Via::WebConfig);
    assert!(r.verified);
    assert!(!r.waited_for_user);
    assert_eq!(r.after.as_ref().unwrap().usb_id, "cafe:0001");
    assert_eq!(r.saved_before.unwrap().mode, Some(InputMode::Xinput));
    assert_eq!(r.reboot, Some(RebootAck::Confirmed));
    let w = r.write.as_ref().unwrap();
    assert!(w.posted);
    assert_eq!(w.changed.len(), 1);
    assert_eq!(w.changed[0].key, "inputMode");
    assert_eq!(rig.pad.mode(), Some(InputMode::Keyboard));
    assert_eq!(
        log.other,
        ["backed up", "writing keyboard", "rebooting gamepad"]
    );

    // One full object posted; never a GET of a set path.
    let writes = rig.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].as_object().unwrap().len(),
        fixture("getGamepadOptions").as_object().unwrap().len()
    );
    assert_eq!(writes[0]["inputMode"], 3);
    for (m, p) in rig.requests() {
        if p.starts_with("/api/set") || p == "/api/reboot" {
            assert_eq!(m, Method::Post, "{p}");
        }
        assert_ne!(p, "/api/getConfig");
        assert_ne!(p, "/api/getI2CPeripheralMap");
    }
    // The last two requests: the write's verify read, then the reboot.
    let reqs = rig.requests();
    assert_eq!(
        &reqs[reqs.len() - 2..],
        [
            (Method::Get, "/api/getGamepadOptions"),
            (Method::Post, "/api/reboot"),
        ]
    );
    // Everything else the pad has is untouched.
    let after = rig.pad.mock().current("getGamepadOptions").unwrap();
    let before = fixture("getGamepadOptions");
    let diff =
        gamepadii_core::api::write::diff(before.as_object().unwrap(), after.as_object().unwrap());
    assert_eq!(diff.len(), 1, "{diff:?}");

    // A backup before the write, and the cache.
    let backups = files(&rig.backups_dir());
    assert_eq!(backups.len(), 1);
    assert!(backups[0].ends_with("-Haute42COSMOX.json"), "{backups:?}");
    assert_eq!(
        r.backup.as_deref(),
        Some(rig.backups_dir().join(&backups[0]).as_path())
    );
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX.json"]);
    // It watched USB: off for a few scans, then back.
    assert!(
        log.waits
            .iter()
            .all(|(a, _)| *a == Awaiting::Reboot(BootMode::Gamepad))
    );
    assert_eq!(log.waits.len(), sim::OFF_USB_SCANS);
}

#[test]
fn from_web_config_with_the_mode_already_saved_only_reboots() {
    let rig = Rig::s16(InputMode::Webconfig);
    let r = rig
        .mode(InputMode::Xinput, true, &mut Log::default())
        .unwrap();
    assert_eq!(r.via, Via::WebConfig);
    assert!(r.write.is_none());
    assert!(r.backup.is_none());
    assert!(r.verified);
    assert!(rig.writes().is_empty());
    assert!(files(&rig.backups_dir()).is_empty());
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
}

#[test]
fn from_a_gamepad_mode_it_waits_for_web_config_then_switches() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput).user(UserAction::Webconfig { after: 10 }),
    );
    let mut log = Log::default();
    let r = rig.mode(InputMode::Ps4, true, &mut log).unwrap();
    assert_eq!(r.via, Via::WebConfig);
    assert!(r.waited_for_user);
    assert!(r.verified);
    assert_eq!(r.before.mode, Some(InputMode::Xinput));
    assert_eq!(
        r.after.as_ref().unwrap().candidates,
        [InputMode::Ps4, InputMode::Ps5]
    );
    assert_eq!(log.guides.len(), 1);
    let guide = log.guides[0].join(" ");
    assert!(guide.contains("First switch it to web-config"), "{guide}");
    assert!(
        guide.contains("hold S2+B3+B4 (Start+X+Y) for 4 seconds"),
        "{guide}"
    );
    let user_waits = log
        .waits
        .iter()
        .filter(|(a, _)| matches!(a, Awaiting::User { .. }))
        .count();
    assert!(user_waits > 10, "{user_waits}");
    assert_eq!(rig.writes().len(), 1);
}

#[test]
fn a_cached_boot_button_needs_no_write() {
    // The S16 maps B1 to Switch; the user holds it while replugging.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput).user(UserAction::BootButton {
            after: 3,
            mode: InputMode::Switch,
        }),
    )
    .with_cache();
    let mut log = Log::default();
    let r = rig.mode(InputMode::Switch, true, &mut log).unwrap();
    assert_eq!(r.via, Via::BootButton);
    assert!(r.verified);
    assert!(r.write.is_none());
    assert!(rig.requests().is_empty(), "no API at all");
    let guide = log.guides[0].join("\n");
    assert!(guide.contains("hold B1 (A) (only that button)"), "{guide}");
    assert!(guide.contains("Nintendo Switch mode"), "{guide}");
    assert!(
        guide.contains("In profile 2 (SilkSong), B1 is the key Default calls Up."),
        "{guide}"
    );
}

#[test]
fn no_wait_exits_5_with_the_instructions() {
    let rig = Rig::s16(InputMode::Xinput).with_cache();
    let err = rig
        .mode(InputMode::Ps5, false, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("the pad is in XInput mode"), "{msg}");
    assert!(msg.contains("hold B4 (Y) (only that button)"), "{msg}");
    assert!(
        msg.contains("S2+B3+B4 are the keys Default calls S2 (Start), B3 (X) and R1 (RB)"),
        "{msg}"
    );
    assert!(msg.contains("2026-10-17T16:57:03Z"), "{msg}");
    assert!(rig.requests().is_empty());
    assert_eq!(rig.clock.elapsed(), Duration::ZERO);
}

#[test]
fn no_cache_means_logical_names_only() {
    let rig = Rig::s16(InputMode::Xinput);
    let err = rig
        .mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("No write needed"), "{msg}");
    assert!(msg.contains("These are logical buttons"), "{msg}");
}

#[test]
fn waiting_for_the_user_times_out() {
    let rig = Rig::s16(InputMode::Xinput);
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    assert!(
        err.to_string().contains("stopped waiting after 60 s"),
        "{err}"
    );
    assert_eq!(rig.clock.elapsed(), Duration::from_secs(60));
    assert!(rig.requests().is_empty());
}

#[test]
fn coming_back_in_another_mode_is_a_failed_check() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).come_back_as(InputMode::Ps3));
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    let msg = err.to_string();
    assert!(
        msg.contains("Keyboard is saved, but the pad came back as PS3"),
        "{msg}"
    );
}

#[test]
fn not_coming_back_times_out() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).stays_away());
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert!(
        err.to_string()
            .contains("hasn't come back on USB after 10 s"),
        "{err}"
    );
}

#[test]
fn a_dropped_reboot_reply_still_verifies() {
    let rig = Rig::new(SimPad::with_mock(
        Mock::new(s16_dir()).drop_reboot(),
        InputMode::Webconfig,
    ));
    let r = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap();
    assert_eq!(r.reboot, Some(RebootAck::Probable));
    assert!(r.verified);
}

#[test]
fn a_mode_newer_than_the_firmware_is_refused_before_writing() {
    let rig = Rig::s16(InputMode::Webconfig);
    let err = rig
        .mode(InputMode::Sinput, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("v0.7.12"), "{err}");
    assert!(rig.writes().is_empty());
}

#[test]
fn webconfig_and_bootsel_are_not_targets() {
    let rig = Rig::s16(InputMode::Webconfig);
    for m in [InputMode::Webconfig, InputMode::Bootsel] {
        let err = rig.mode(m, true, &mut Log::default()).unwrap_err();
        assert_eq!(err.exit_code(), exit::USAGE);
    }
    assert!(rig.requests().is_empty());
}

#[test]
fn no_pad_is_not_found() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).stays_away());
    // Reboot it away: a pad that never comes back looks like no pad.
    gamepadii_core::api::reboot(rig.pad.mock(), BootMode::Gamepad).unwrap();
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NOT_FOUND);
}

// ---- mode with no argument ------------------------------------------------

#[test]
fn mode_status_in_web_config_reads_the_pad() {
    let rig = Rig::s16(InputMode::Webconfig);
    let s = rig
        .run(|env| flow::mode_status(env, &mut Log::default()))
        .unwrap();
    assert_eq!(s.saved_from, Some("pad"));
    assert_eq!(s.saved.unwrap().mode, Some(InputMode::Xinput));
    assert_eq!(s.firmware.as_deref(), Some("v0.7.12"));
    let choice = |m| s.modes.iter().find(|c| c.mode == m).unwrap();
    assert_eq!(choice(InputMode::Switch).boot_button, Some(Button::B1));
    assert_eq!(choice(InputMode::Ps5).boot_button, Some(Button::B4));
    assert_eq!(choice(InputMode::Generic).boot_button, None);
    assert_eq!(choice(InputMode::Sinput).supported, Some(false));
    assert_eq!(choice(InputMode::Xinput).supported, Some(true));
    assert!(s.modes.iter().all(|c| c.mode != InputMode::Webconfig));
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX.json"]);
}

#[test]
fn mode_status_in_a_gamepad_mode_uses_the_cache() {
    let rig = Rig::s16(InputMode::Xinput).with_cache();
    let s = rig
        .run(|env| flow::mode_status(env, &mut Log::default()))
        .unwrap();
    assert_eq!(s.saved_from, Some("cache"));
    assert_eq!(s.cached_at.as_deref(), Some("2026-10-17T16:57:03Z"));
    assert!(
        s.modes
            .iter()
            .find(|c| c.mode == InputMode::Xinput)
            .unwrap()
            .current
    );
    assert!(rig.requests().is_empty());

    let rig = Rig::s16(InputMode::Xinput);
    let s = rig
        .run(|env| flow::mode_status(env, &mut Log::default()))
        .unwrap();
    assert_eq!((s.saved, s.saved_from, s.firmware), (None, None, None));
    assert!(
        s.modes
            .iter()
            .all(|c| c.boot_button.is_none() && c.supported.is_none())
    );
}

// ---- webconfig ------------------------------------------------------------

#[test]
fn webconfig_already_there_prints_the_url() {
    let rig = Rig::s16(InputMode::Webconfig);
    let r = rig
        .run(|env| flow::webconfig(env, true, &mut Log::default()))
        .unwrap();
    assert_eq!(r.url, "http://192.168.7.1/");
    assert!(!r.waited_for_user);
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX.json"]);
}

#[test]
fn webconfig_waits_for_the_user() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput).user(UserAction::Webconfig { after: 2 }),
    )
    .with_cache();
    let mut log = Log::default();
    let r = rig.run(|env| flow::webconfig(env, true, &mut log)).unwrap();
    assert!(r.waited_for_user);
    assert_eq!(r.device.usb_id, "cafe:4028");
    let guide = log.guides[0].join("\n");
    assert!(guide.starts_with("To switch it to web-config"), "{guide}");
    assert!(!guide.contains("No write needed"), "{guide}");
}

#[test]
fn webconfig_no_wait_and_timeout() {
    let rig = Rig::s16(InputMode::Keyboard);
    let err = rig
        .run(|env| flow::webconfig(env, false, &mut Log::default()))
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    assert!(
        err.to_string().contains("the pad is in Keyboard mode"),
        "{err}"
    );
    let err = rig
        .run(|env| flow::webconfig(env, true, &mut Log::default()))
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    assert_eq!(rig.clock.elapsed(), Duration::from_secs(60));
}

#[test]
fn from_bootsel_there_is_no_hotkey() {
    let rig = Rig::s16(InputMode::Bootsel);
    let err = rig
        .run(|env| flow::webconfig(env, false, &mut Log::default()))
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("hold S2 (Start) while plugging the pad in."),
        "{msg}"
    );
    assert!(!msg.contains("S2+B3+B4"), "{msg}");
}

// ---- reboot with a wait ---------------------------------------------------

#[test]
fn reboot_waits_and_verifies_the_saved_mode() {
    let rig = Rig::s16(InputMode::Webconfig);
    let r = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Gamepad, true, &mut Log::default())
        })
        .unwrap();
    assert!(r.waited);
    assert_eq!(r.expected, Some(InputMode::Xinput));
    assert_eq!(r.verified, Some(true));
    assert_eq!(r.after.unwrap().usb_id, "045e:028e");
}

#[test]
fn reboot_into_web_config_waits_for_a_new_enumeration() {
    // Already in web-config: the old device must not count as "back".
    let rig = Rig::s16(InputMode::Webconfig);
    let mut log = Log::default();
    let r = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Webconfig, true, &mut log)
        })
        .unwrap();
    assert_eq!(r.verified, Some(true));
    assert_eq!(log.waits.len(), sim::OFF_USB_SCANS);
}

#[test]
fn reboot_no_wait_and_bootsel() {
    let rig = Rig::s16(InputMode::Webconfig);
    let r = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Bootsel, false, &mut Log::default())
        })
        .unwrap();
    assert!(!r.waited);
    assert_eq!(r.after, None);
    let rig = Rig::s16(InputMode::Webconfig);
    let r = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Bootsel, true, &mut Log::default())
        })
        .unwrap();
    assert_eq!(r.after.unwrap().mode, Some(InputMode::Bootsel));
}

#[test]
fn reboot_that_comes_back_wrong_or_never() {
    let rig =
        Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).come_back_as(InputMode::Keyboard));
    let err = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Gamepad, true, &mut Log::default())
        })
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).stays_away());
    let err = rig
        .run(|env| {
            let client = (env.open)().unwrap();
            flow::reboot(env, &client, BootMode::Gamepad, true, &mut Log::default())
        })
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert_eq!(rig.clock.elapsed(), Duration::from_secs(10));
}

// ---- button hints on the S16 ----------------------------------------------

#[test]
fn silksong_moves_b4_onto_r1() {
    let p = s16_profiles();
    assert_eq!(p.label(2).as_deref(), Some("SilkSong"));
    let combo = Combo::new(&p, 2, &[Button::S2, Button::B3, Button::B4]);
    let gpios: Vec<Vec<u8>> = combo
        .buttons
        .iter()
        .map(|h| h.keys.iter().map(|k| k.gpio).collect())
        .collect();
    assert_eq!(gpios, [vec![17], vec![10], vec![12]]);
    let defaults: Vec<Option<Button>> = combo.buttons.iter().map(|h| h.keys[0].default).collect();
    assert_eq!(
        defaults,
        [Some(Button::S2), Some(Button::B3), Some(Button::R1)]
    );
    assert_eq!(
        combo.physical().as_deref(),
        Some(
            "in profile 2 (SilkSong), S2+B3+B4 are the keys Default calls S2 (Start), B3 (X) and R1 (RB)"
        )
    );
    assert!(combo.unreachable().is_empty());
}

#[test]
fn default_needs_no_hint() {
    let p = s16_profiles();
    let combo = Combo::new(&p, 1, &[Button::S2, Button::B3, Button::B4]);
    assert_eq!(combo.physical(), None);
}

#[test]
fn functional_mapping_follows_the_firmware() {
    let p = s16_profiles();
    let silksong = p.functional(2);
    // GPIO 6 is B1 in Default and L2 in SilkSong; GPIO 0 is an add-on pin
    // in both, so the profile can't touch it.
    assert_eq!(silksong[&6], Button::L2.action());
    assert_eq!(silksong[&0], 0);
    // Two keys can press the same button: S1 is GPIO 9 and 16 in SilkSong.
    let s1 = buttons::locate(&p, 2, Button::S1);
    let keys: Vec<String> = s1.keys.iter().map(|k| k.describe()).collect();
    assert_eq!(keys, ["L2 (LT)", "S1 (Back)"]);
    // A profile that doesn't exist, or is disabled, is the base mapping.
    assert_eq!(p.functional(9), p.functional(1));
    let mut disabled = p.clone();
    disabled.alternatives[0].enabled = Some(false);
    assert_eq!(disabled.functional(2), p.functional(1));
}

#[test]
fn every_s16_profile_can_reach_the_web_config_hotkey() {
    let p = s16_profiles();
    for n in 1..=5 {
        let combo = Combo::new(&p, n, &[Button::S2, Button::B3, Button::B4]);
        assert!(combo.unreachable().is_empty(), "profile {n}");
    }
}

// ---- the cache ------------------------------------------------------------

#[test]
fn cache_round_trips_and_the_newest_wins() {
    let dir = tempfile::tempdir().unwrap();
    assert!(cache::load_latest(dir.path()).is_none());
    let old = s16_cache();
    let path = cache::store(dir.path(), &old).unwrap();
    assert_eq!(path, dir.path().join("Haute42COSMOX.json"));
    assert_eq!(cache::load_latest(dir.path()), Some(old.clone()));
    // Another board, captured later; and junk that is ignored.
    let mut newer = old.clone();
    newer.firmware.board = Some("Pico".into());
    newer.captured_at = "2026-10-18T00:00:00Z".into();
    cache::store(dir.path(), &newer).unwrap();
    std::fs::write(dir.path().join("junk.json"), "{").unwrap();
    assert_eq!(cache::load_latest(dir.path()), Some(newer));
    // The cache keeps the pad's key order.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.find("\"dpadMode\"").unwrap() < text.find("\"inputMode\"").unwrap());
    let options = old.options().unwrap();
    assert_eq!(options.profile_number, Some(2));
    assert_eq!(old.profiles().unwrap().alternatives.len(), 4);
}

#[test]
fn info_and_backup_produce_the_cache() {
    let rig = Rig::s16(InputMode::Webconfig);
    let client = rig.pad.open().unwrap();
    let now = std::time::UNIX_EPOCH + Duration::from_secs(CAPTURED);
    let (_, from_info) = gamepadii_core::commands::info_with_cache(&client, now).unwrap();
    assert_eq!(from_info, s16_cache());
    let dir = tempfile::tempdir().unwrap();
    let (_, from_backup) =
        gamepadii_core::commands::backup_with_cache(&client, Some(&dir.path().join("b.json")), now)
            .unwrap();
    assert_eq!(from_backup, Some(s16_cache()));
}

// ---- the control channel --------------------------------------------------

#[test]
fn mode_over_the_channel_needs_no_web_config_and_no_button() {
    let rig = Rig::cc(InputMode::Xinput);
    let mut log = Log::default();
    let r = rig.mode(InputMode::Keyboard, true, &mut log).unwrap();
    assert_eq!(r.via, Via::Control);
    assert!(r.verified);
    assert!(!r.waited_for_user);
    assert_eq!(
        r.before.confidence,
        gamepadii_core::mode::Confidence::Confirmed
    );
    let after = r.after.as_ref().unwrap();
    assert_eq!(after.usb_id, "cafe:0001");
    assert_eq!(after.mode, Some(InputMode::Keyboard));
    assert_eq!(r.reboot, Some(RebootAck::Confirmed));
    assert!(r.write.is_none() && r.backup.is_none());
    assert!(log.guides.is_empty(), "no button press");
    assert!(rig.requests().is_empty(), "no web-config");
    assert_eq!(rig.channel_commands(), [0x12], "one SET_INPUT_MODE");
    assert_eq!(log.other, ["writing keyboard", "rebooting gamepad"]);
    assert_eq!(rig.pad.mode(), Some(InputMode::Keyboard));
    assert_eq!(
        rig.pad.mock().current("getGamepadOptions").unwrap()["inputMode"],
        3
    );
    // The channel is remembered for this board.
    assert!(cache::channel_known(Some(&rig.cache_dir())));
}

#[test]
fn the_channel_settles_ps4_or_ps5() {
    // 1532:0401 is PS4 or PS5; STATUS says which, so PS5 is "already".
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Ps5)
            .with_channel()
            .channel(|c| c.running = Some(13)),
    );
    let r = rig.mode(InputMode::Ps5, true, &mut Log::default()).unwrap();
    assert_eq!(r.via, Via::Already);
    assert_eq!(r.before.mode, Some(InputMode::Ps5));
    assert!(rig.channel_commands().is_empty());
}

#[test]
fn mode_over_the_channel_refuses_what_the_build_cant_boot() {
    let rig = Rig::cc(InputMode::Xinput);
    let err = rig
        .mode(InputMode::Sinput, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("has no SInput mode"), "{err}");
    assert!(rig.channel_commands().is_empty(), "nothing sent");
}

#[test]
fn mode_over_the_channel_honours_the_lock() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.forced_setup = 1),
    );
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("locks mode switching"), "{err}");
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
}

#[test]
fn mode_over_the_channel_checks_what_came_back() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .come_back_as(InputMode::Ps3),
    );
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    assert!(
        err.to_string()
            .contains("Keyboard is saved, but the pad came back as PS3"),
        "{err}"
    );
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .stays_away(),
    );
    let err = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
}

#[test]
fn stock_firmware_falls_back_to_the_guide() {
    // XInput answers HELLO with 0 bytes, Xbox One with garbage, Keyboard
    // STALLs: all three are "no channel", and the guided flow runs.
    for m in [InputMode::Xinput, InputMode::Xbone, InputMode::Keyboard] {
        let rig = Rig::s16(m);
        let err = rig
            .mode(InputMode::Switch, false, &mut Log::default())
            .unwrap_err();
        assert_eq!(err.exit_code(), exit::NEEDS_USER, "{m:?}");
        assert!(err.to_string().contains("S2+B3+B4"), "{m:?}: {err}");
        assert!(!err.to_string().contains("setup udev"), "{m:?}: {err}");
        assert_eq!(rig.pad.control_log().len(), 1, "one HELLO, {m:?}");
        assert!(rig.channel_commands().is_empty());
    }
}

#[test]
fn no_permission_on_a_known_channel_pad_adds_the_udev_hint() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .no_access(),
    )
    .knows_the_channel();
    let err = rig
        .mode(InputMode::Keyboard, false, &mut Log::default())
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("run `gamepadii setup udev` once"), "{msg}");
    assert!(msg.contains("/dev/bus/usb/003/007"), "{msg}");
    assert!(msg.contains("S2+B3+B4"), "the guide still follows: {msg}");
    // Without the record (stock firmware, as far as gamepadii knows): quiet.
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).no_access());
    let err = rig
        .mode(InputMode::Keyboard, false, &mut Log::default())
        .unwrap_err();
    assert!(!err.to_string().contains("setup udev"), "{err}");
}

#[test]
fn web_config_still_uses_http_for_mode() {
    let rig = Rig::cc(InputMode::Webconfig);
    let r = rig
        .mode(InputMode::Keyboard, true, &mut Log::default())
        .unwrap();
    assert_eq!(r.via, Via::WebConfig);
    assert!(r.backup.is_some(), "the HTTP path backs up first");
    assert!(rig.pad.control_log().is_empty(), "no probe in web-config");
}

#[test]
fn mode_status_with_the_channel() {
    let rig = Rig::cc(InputMode::Xinput);
    let s = rig
        .run(|env| flow::mode_status(env, &mut Log::default()))
        .unwrap();
    assert_eq!(
        s.current.confidence,
        gamepadii_core::mode::Confidence::Confirmed
    );
    let choice = |m| s.modes.iter().find(|c| c.mode == m).unwrap();
    assert_eq!(choice(InputMode::Sinput).supported, Some(false));
    assert_eq!(choice(InputMode::Generic).supported, Some(true));
    assert!(choice(InputMode::Xinput).current);
}

#[test]
fn webconfig_over_the_channel() {
    let rig = Rig::cc(InputMode::Xinput);
    let mut log = Log::default();
    let r = rig.run(|env| flow::webconfig(env, true, &mut log)).unwrap();
    assert_eq!(r.via, flow::WebconfigVia::Control);
    assert!(!r.waited_for_user);
    assert_eq!(r.device.usb_id, "cafe:4028");
    assert!(log.guides.is_empty());
    assert_eq!(log.other, ["rebooting webconfig"]);
    assert_eq!(rig.channel_commands(), [0x11]);
    // Once there, the cache is refreshed over HTTP.
    assert_eq!(
        files(&rig.cache_dir()),
        ["Haute42COSMOX.json", cache::CHANNELS_FILE]
    );
}

#[test]
fn webconfig_locked_by_forced_setup() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.forced_setup = 2),
    );
    let err = rig
        .run(|env| flow::webconfig(env, true, &mut Log::default()))
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("locks web-config"), "{err}");
}

fn reboot_pad(rig: &Rig, m: BootMode) -> gamepadii_core::Result<flow::RebootReport> {
    rig.run(|env| flow::reboot_pad(env, m, true, &mut Log::default()))
}

#[test]
fn reboot_from_a_gamepad_mode_over_the_channel() {
    let rig = Rig::cc(InputMode::Xinput);
    let r = reboot_pad(&rig, BootMode::Gamepad).unwrap();
    assert_eq!(r.via, flow::Route::Control);
    assert_eq!(r.host, "/dev/bus/usb/003/007");
    assert_eq!(r.expected, Some(InputMode::Xinput));
    assert_eq!(r.verified, Some(true));
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));

    let rig = Rig::cc(InputMode::Keyboard);
    let r = reboot_pad(&rig, BootMode::Webconfig).unwrap();
    assert_eq!(r.verified, Some(true));
    assert_eq!(r.after.unwrap().usb_id, "cafe:4028");
    assert!(rig.requests().is_empty());
}

#[test]
fn bootsel_from_a_gamepad_mode_goes_through_web_config() {
    let rig = Rig::cc(InputMode::Xinput);
    let r = reboot_pad(&rig, BootMode::Bootsel).unwrap();
    assert_eq!(r.via, flow::Route::WebConfig);
    assert_eq!(r.verified, Some(true));
    assert_eq!(rig.pad.mode(), Some(InputMode::Bootsel));
    assert!(r.note.unwrap().contains("went through web-config"));
    // REBOOT(1) on the channel, then /api/reboot {"bootMode": 2}.
    assert_eq!(rig.channel_commands(), [0x11]);
    let reboots: Vec<Value> = rig
        .pad
        .mock()
        .requests()
        .into_iter()
        .filter(|r| r.path == "/api/reboot")
        .map(|r| r.json())
        .collect();
    assert_eq!(reboots, [serde_json::json!({"bootMode": 2})]);
}

#[test]
fn reboot_in_web_config_uses_http_and_without_a_channel_guides() {
    let rig = Rig::cc(InputMode::Webconfig);
    let r = reboot_pad(&rig, BootMode::Gamepad).unwrap();
    assert_eq!(r.via, flow::Route::WebConfig);
    assert_eq!(r.host, "mock:s16-v0.7.12");

    let rig = Rig::s16(InputMode::Xinput);
    let err = reboot_pad(&rig, BootMode::Webconfig).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("XInput mode, not web-config"), "{msg}");
    assert!(msg.contains("S2+B3+B4 for 4 seconds"), "{msg}");
}

fn switch(rig: &Rig, which: &str, save: bool) -> gamepadii_core::Result<flow::ProfileReport> {
    let which: flow::ProfileArg = which.parse().unwrap();
    rig.run(|env| flow::profile_switch(env, &which, save, &mut Log::default()))
}

fn saved_profile(rig: &Rig) -> Value {
    rig.pad.mock().current("getGamepadOptions").unwrap()["profileNumber"].clone()
}

#[test]
fn profile_switch_is_ram_only_by_default() {
    let rig = Rig::cc(InputMode::Xinput);
    let r = switch(&rig, "3", false).unwrap();
    assert_eq!(r.before.display(), "2 SilkSong");
    assert_eq!(r.profile.display(), "3 Celeste");
    assert!(r.changed && r.applied && !r.saved);
    assert!(r.note.unwrap().contains("not saved"));
    assert_eq!(rig.pad.firmware().active, 3);
    assert_eq!(saved_profile(&rig), 2, "flash untouched");
    assert!(rig.requests().is_empty());
}

#[test]
fn profile_switch_by_label_and_saved() {
    let rig = Rig::cc(InputMode::Keyboard);
    let r = switch(&rig, "dkc", true).unwrap();
    assert_eq!(r.profile.display(), "4 DKC");
    assert!(r.saved);
    assert_eq!(r.note, None);
    assert_eq!(saved_profile(&rig), 4);
}

#[test]
fn profile_switch_already_active() {
    let rig = Rig::cc(InputMode::Xinput);
    let r = switch(&rig, "SilkSong", false).unwrap();
    assert!(!r.changed);
    assert!(rig.channel_commands().is_empty(), "nothing sent");
    // --save on the active profile still saves it.
    let r = switch(&rig, "2", true).unwrap();
    assert!(r.saved);
    assert_eq!(rig.channel_commands(), [0x10]);
}

#[test]
fn profile_switch_refuses_unknown_and_disabled_profiles() {
    let rig = Rig::cc(InputMode::Xinput);
    let err = switch(&rig, "Hades", false).unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(
        err.to_string().contains(
            "no enabled profile is called \"Hades\"; the pad has 1 Default, 2 SilkSong, \
             3 Celeste, 4 DKC, 5 TGX"
        ),
        "{err}"
    );
    let err = switch(&rig, "6", false).unwrap_err();
    assert!(err.to_string().contains("profile 6 isn't enabled"), "{err}");
    assert!("7".parse::<flow::ProfileArg>().is_err());
    assert!("0".parse::<flow::ProfileArg>().is_err());
    assert!(rig.channel_commands().is_empty());
}

#[test]
fn profile_switch_without_the_channel() {
    let rig = Rig::s16(InputMode::Xinput);
    let err = switch(&rig, "2", false).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("has no control channel"), "{msg}");
    assert!(msg.contains("LOAD_PROFILE_n"), "{msg}");

    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .no_access(),
    );
    let err = switch(&rig, "2", false).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    assert!(err.to_string().contains("gamepadii setup udev"), "{err}");
}

#[test]
fn a_ram_switch_is_undone_by_a_reboot() {
    let rig = Rig::cc(InputMode::Xinput);
    switch(&rig, "5", false).unwrap();
    reboot_pad(&rig, BootMode::Gamepad).unwrap();
    assert_eq!(rig.pad.firmware().active, 2);
}

// ---- status ----------------------------------------------------------------

fn status(rig: &Rig) -> gamepadii_core::commands::Status {
    let pad = rig.pad.clone();
    let control = move |d: &gamepadii_core::discovery::Device| pad.open_control(d);
    let dir = rig.cache_dir();
    gamepadii_core::commands::status_with(&*rig.pad, Some(&control), Some(&dir)).unwrap()
}

#[test]
fn status_with_the_channel_is_confirmed() {
    let rig = Rig::cc(InputMode::Xinput);
    let s = status(&rig);
    let d = &s.devices[0];
    assert_eq!(
        d.device.confidence,
        gamepadii_core::mode::Confidence::Confirmed
    );
    assert_eq!(d.device.note, None);
    let c = d.control_channel.as_ref().unwrap();
    assert_eq!(c.firmware.version.as_deref(), Some("v0.7.12"));
    assert_eq!(c.firmware.board.as_deref(), Some("Haute42COSMOX"));
    assert_eq!(c.input_mode.mode, Some(InputMode::Xinput));
    assert_eq!(c.active_profile.display(), "2 SilkSong");
    assert_eq!(c.enabled_profiles, [1, 2, 3, 4, 5]);
    assert_eq!(c.protocol, "1.0");
    assert!(cache::channel_known(Some(&rig.cache_dir())));
    assert!(rig.channel_commands().is_empty(), "status only reads");
}

#[test]
fn status_without_the_channel() {
    let rig = Rig::s16(InputMode::Xinput);
    let d = &status(&rig).devices[0];
    assert!(d.control_channel.is_none());
    assert_eq!(d.channel_note, None);
    assert_eq!(
        d.device.confidence,
        gamepadii_core::mode::Confidence::Likely
    );

    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).no_access()).knows_the_channel();
    let d = &status(&rig).devices[0];
    assert!(
        d.channel_note
            .as_ref()
            .unwrap()
            .contains("gamepadii setup udev")
    );

    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).no_access());
    assert_eq!(status(&rig).devices[0].channel_note, None);

    let rig = Rig::cc(InputMode::Bootsel);
    assert!(status(&rig).devices[0].control_channel.is_none());
    assert!(rig.pad.control_log().is_empty(), "no HELLO to the boot ROM");
}

// ---- the cache follows the running profile --------------------------------

fn cached(rig: &Rig) -> DeviceCache {
    cache::load(&rig.cache_dir(), "Haute42COSMOX").unwrap()
}

#[test]
fn a_ram_switch_moves_only_the_running_profile() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "3", false).unwrap();
    let c = cached(&rig);
    let running = c.running_profile.as_ref().unwrap();
    assert_eq!(running.number, 3);
    assert_eq!(running.seen_at, "2026-10-17T16:57:03Z");
    assert_eq!(c.saved_profile(), Some(2), "the saved profile is unchanged");
    assert_eq!(c.active_profile(), Some(3));
    // --save moves both.
    switch(&rig, "dkc", true).unwrap();
    let c = cached(&rig);
    assert_eq!(c.running_profile.as_ref().unwrap().number, 4);
    assert_eq!(c.saved_profile(), Some(4));
    assert_eq!(c.gamepad_options["profileNumber"], 4);
    // Nothing else in the options moved.
    let mut want = s16_cache().gamepad_options;
    want["profileNumber"] = 4.into();
    assert_eq!(c.gamepad_options, want);
}

#[test]
fn no_cache_for_the_board_is_left_alone() {
    let rig = Rig::cc(InputMode::Xinput);
    switch(&rig, "3", false).unwrap();
    assert!(files(&rig.cache_dir()).is_empty(), "no cache made up");
}

#[test]
fn status_refreshes_the_running_profile() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.active = 5),
    )
    .with_cache();
    status(&rig);
    assert_eq!(cached(&rig).active_profile(), Some(5));
    // Written only on a change: the timestamp stays.
    let before = cached(&rig);
    rig.clock.advance(Duration::from_secs(60));
    status(&rig);
    assert_eq!(cached(&rig), before);
}

#[test]
fn a_reboot_puts_the_cache_back_on_the_saved_profile() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "5", false).unwrap();
    assert_eq!(cached(&rig).active_profile(), Some(5));
    reboot_pad(&rig, BootMode::Gamepad).unwrap();
    assert_eq!(cached(&rig).running_profile, None);
    assert_eq!(cached(&rig).active_profile(), Some(2));
}

#[test]
fn an_old_cache_file_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = cache::store(dir.path(), &s16_cache()).unwrap();
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v["running_profile"].is_null(), "written as null");
    v.as_object_mut().unwrap().remove("running_profile");
    std::fs::write(&path, v.to_string()).unwrap();
    let c = cache::load_latest(dir.path()).unwrap();
    assert_eq!(c.running_profile, None);
    assert_eq!(c.active_profile(), Some(2));
}

#[test]
fn hints_use_the_profile_the_channel_reports() {
    // A channel with no SET_INPUT_MODE: `mode` falls back to the guide,
    // but STATUS still says the pad runs profile 1 (Default), not the
    // cached 2 (SilkSong), so no key is moved.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.caps = gamepadii_core::channel::Caps(
                    gamepadii_core::channel::sim::V1_CAPS
                        & !gamepadii_core::channel::Caps::SET_INPUT_MODE,
                );
                c.active = 1;
            }),
    )
    .with_cache();
    let err = rig
        .mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("Default calls"), "{msg}");
    assert!(
        msg.contains("in the profile the pad reports running now"),
        "{msg}"
    );
    // On profile 3 (Celeste) the hint names it.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.caps = gamepadii_core::channel::Caps(gamepadii_core::channel::Caps::STATUS);
                c.active = 3;
            }),
    )
    .with_cache();
    let msg = rig
        .mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err()
        .to_string();
    assert!(msg.contains("In profile 3 (Celeste)"), "{msg}");
}

#[test]
fn without_the_channel_hints_use_the_recorded_profile() {
    let rig = Rig::s16(InputMode::Xinput).with_cache();
    let now = std::time::UNIX_EPOCH + Duration::from_secs(CAPTURED + 3600);
    cache::record_profile(
        &rig.cache_dir(),
        &cache::Key::board("Haute42COSMOX"),
        1,
        false,
        now,
    )
    .unwrap();
    let msg = rig
        .mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err()
        .to_string();
    assert!(!msg.contains("Default calls"), "{msg}");
    assert!(
        msg.contains("in the profile the pad last reported running (on 2026-10-17T17:57:03Z)"),
        "{msg}"
    );
    // The saved profile, with no record: SilkSong's moved key.
    let rig = Rig::s16(InputMode::Xinput).with_cache();
    let msg = rig
        .mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("Default calls S2 (Start), B3 (X) and R1 (RB)"),
        "{msg}"
    );
}

// ---- info and backup through web-config -------------------------------------

type Read = (gamepadii_core::commands::Info, Option<flow::Trip>);

fn info_trip(rig: &Rig, stay: bool, log: &mut Log) -> gamepadii_core::Result<Read> {
    rig.run(|env| {
        flow::through_webconfig(env, stay, log, gamepadii_core::commands::info, |_| {
            "the pad was read".to_string()
        })
    })
}

fn reboots(rig: &Rig) -> Vec<Value> {
    rig.pad
        .mock()
        .requests()
        .into_iter()
        .filter(|r| r.path == "/api/reboot")
        .map(|r| r.json())
        .collect()
}

#[test]
fn info_from_xinput_goes_to_web_config_and_back() {
    let rig = Rig::cc(InputMode::Xinput);
    let mut log = Log::default();
    let (info, trip) = info_trip(&rig, false, &mut log).unwrap();
    assert_eq!(info.firmware.version.as_deref(), Some("v0.7.12"));
    let t = trip.unwrap();
    assert_eq!(t.via, flow::Route::Control);
    assert_eq!(t.host, "/dev/bus/usb/003/007");
    assert_eq!(t.before.mode, Some(InputMode::Xinput));
    assert_eq!(t.profile, Some(2));
    assert_eq!(t.back_via, Some(flow::Route::WebConfig));
    let after = t.after.unwrap();
    assert_eq!(after.mode, Some(InputMode::Xinput));
    assert_eq!(
        after.confidence,
        gamepadii_core::mode::Confidence::Confirmed
    );
    assert!(t.verified && !t.stayed);
    assert_eq!(t.note, None);
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
    // REBOOT(1) on the channel, then /api/reboot {"bootMode": 0}; no write.
    assert_eq!(rig.channel_commands(), [0x11]);
    assert_eq!(reboots(&rig), [serde_json::json!({"bootMode": 0})]);
    assert!(rig.writes().is_empty());
    assert_eq!(log.other, ["rebooting webconfig", "rebooting gamepad"]);
    assert!(log.guides.is_empty(), "no button press");
}

#[test]
fn backup_from_keyboard_goes_to_web_config_and_back() {
    // A pad in Keyboard has Keyboard saved.
    let mock = Mock::new(s16_dir());
    let mut o = mock.current("getGamepadOptions").unwrap();
    o["inputMode"] = 3.into();
    mock.store("getGamepadOptions", o);
    let rig = Rig::new(SimPad::with_mock(mock, InputMode::Keyboard).with_channel());
    let out = rig.dirs.path().join("b.json");
    let (report, trip) = rig
        .run(|env| {
            flow::through_webconfig(
                env,
                false,
                &mut Log::default(),
                |c| gamepadii_core::commands::backup(c, Some(&out), env.clock.wall()),
                |b| format!("the backup is saved in {}", b.path.display()),
            )
        })
        .unwrap();
    assert_eq!(report.endpoints.len(), 21);
    assert!(out.exists());
    assert!(trip.unwrap().verified);
    assert_eq!(rig.pad.mode(), Some(InputMode::Keyboard));
}

fn fork_dir() -> PathBuf {
    s16_dir().join("../s16-fork")
}

/// Whether the pad's web-config server was asked for `getConfig`.
fn asked_for_get_config(rig: &Rig) -> bool {
    rig.requests().iter().any(|(_, p)| *p == "/api/getConfig")
}

fn backup(rig: &Rig) -> (gamepadii_core::commands::BackupReport, Option<flow::Trip>) {
    rig.run(|env| flow::backup(env, None, false, &mut Log::default()))
        .unwrap()
}

#[test]
fn backup_on_stock_never_asks_for_get_config() {
    // Stock without the channel, already in web-config.
    let rig = Rig::s16(InputMode::Webconfig);
    let (report, _) = backup(&rig);
    assert!(!report.full_config);
    assert!(!asked_for_get_config(&rig));
    // A v0.7.12 release with the channel, from XInput: still no.
    let rig = Rig::cc(InputMode::Xinput);
    let (report, trip) = backup(&rig);
    assert!(trip.unwrap().verified);
    assert!(!report.full_config);
    assert_eq!(report.full_config_evidence, None);
    assert!(!asked_for_get_config(&rig));
}

#[test]
fn backup_on_the_fork_build_has_get_config() {
    let rig = Rig::new(SimPad::new(fork_dir(), InputMode::Xinput).with_channel());
    let (report, trip) = backup(&rig);
    assert!(trip.unwrap().verified);
    assert!(report.full_config);
    assert_eq!(
        report.full_config_evidence,
        Some(gamepadii_core::api::Evidence::KnownBuild)
    );
    assert!(asked_for_get_config(&rig));
    let text = std::fs::read_to_string(&report.path).unwrap();
    let saved: Value = serde_json::from_str(&text).unwrap();
    let anim = &saved["full_config"]["config"]["animationOptions"];
    assert_eq!(anim["brightness"], 3);
}

#[test]
fn a_dev_build_qualifies_by_its_control_channel() {
    // A fork build gamepadii doesn't know the commit of: the HELLO it
    // answers in web-config is the evidence.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("BASE"), fork_dir().to_str().unwrap()).unwrap();
    std::fs::write(
        dir.path().join("getFirmwareVersion.json"),
        r#"{"version":"v0.7.12-9-g1234567","boardBuild":"1234567","boardConfig":"Haute42COSMOX"}"#,
    )
    .unwrap();
    let rig = Rig::new(SimPad::new(dir.path(), InputMode::Webconfig).with_channel());
    let (report, trip) = backup(&rig);
    assert_eq!(trip, None);
    assert_eq!(
        report.full_config_evidence,
        Some(gamepadii_core::api::Evidence::ControlChannel)
    );
    assert!(report.full_config);
    // Without the channel, the same build is left alone.
    let rig = Rig::new(SimPad::new(dir.path(), InputMode::Webconfig));
    let (report, _) = backup(&rig);
    assert_eq!(report.full_config_evidence, None);
    assert!(!asked_for_get_config(&rig));
}

#[test]
fn stay_leaves_it_in_web_config() {
    let rig = Rig::cc(InputMode::Xinput);
    let (_, trip) = info_trip(&rig, true, &mut Log::default()).unwrap();
    let t = trip.unwrap();
    assert!(t.stayed);
    assert_eq!(t.back_via, None);
    assert_eq!(t.after, None);
    assert!(t.note.unwrap().contains("gamepadii webconfig --exit"));
    assert_eq!(rig.pad.mode(), Some(InputMode::Webconfig));
    assert!(reboots(&rig).is_empty());
}

#[test]
fn in_web_config_there_is_no_trip() {
    let rig = Rig::cc(InputMode::Webconfig);
    let (_, trip) = info_trip(&rig, false, &mut Log::default()).unwrap();
    assert_eq!(trip, None);
    assert!(rig.pad.control_log().is_empty(), "no probe in web-config");
    assert!(reboots(&rig).is_empty());
}

#[test]
fn without_the_channel_info_needs_the_user_as_before() {
    let rig = Rig::s16(InputMode::Xinput);
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    let msg = err.to_string();
    assert!(msg.contains("XInput mode, not web-config"), "{msg}");
    assert!(msg.contains("S2+B3+B4 for 4 seconds"), "{msg}");
    assert!(rig.requests().is_empty());
    // A pad known to have the channel, with no permission: the udev hint too.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .no_access(),
    )
    .knows_the_channel();
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
    assert!(err.to_string().contains("gamepadii setup udev"), "{err}");
    // BOOTSEL: the boot ROM has no channel.
    let rig = Rig::cc(InputMode::Bootsel);
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::NEEDS_USER);
}

#[test]
fn a_failed_read_still_sends_the_pad_back() {
    let rig = Rig::cc(InputMode::Xinput);
    let err = rig
        .run(|env| {
            flow::through_webconfig(
                env,
                false,
                &mut Log::default(),
                |_| -> gamepadii_core::Result<()> {
                    Err(gamepadii_core::Error::io(
                        "GET http://192.168.7.1/api/getPinMappings",
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
                    ))
                },
                |_| unreachable!(),
            )
        })
        .unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert_eq!(
        err.to_string(),
        "in web-config: GET http://192.168.7.1/api/getPinMappings: timed out; the pad \
         was sent back and is in XInput mode again"
    );
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
    assert_eq!(reboots(&rig), [serde_json::json!({"bootMode": 0})]);
}

#[test]
fn a_silent_web_configurator_sends_it_back_over_the_channel() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .server_down(),
    );
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    let msg = err.to_string();
    assert!(
        msg.starts_with("the pad is in web-config, but its web configurator didn't answer"),
        "{msg}"
    );
    assert!(
        msg.ends_with("; the pad was sent back and is in XInput mode again"),
        "{msg}"
    );
    // REBOOT(1) from XInput, REBOOT(0) from web-config: the channel
    // answers there too.
    assert_eq!(rig.channel_commands(), [0x11, 0x11]);
    assert!(reboots(&rig).is_empty());
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
}

#[test]
fn a_pad_lost_on_the_way_there() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .stays_away(),
    );
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    let msg = err.to_string();
    assert!(msg.contains("asked for web-config"), "{msg}");
    assert!(msg.contains("nothing was read"), "{msg}");
    assert!(msg.contains("`gamepadii reboot gamepad`"), "{msg}");
}

#[test]
fn a_pad_lost_on_the_way_back() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .stays_away_after(1),
    );
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert_eq!(
        err.to_string(),
        "the pad was read, but on the way back: asked for XInput and the pad \
         was rebooted: it hasn't come back on USB after 10 s"
    );
}

#[test]
fn coming_back_in_another_mode_fails_the_check() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .come_back_as(InputMode::Ps3),
    );
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::CHECK_FAILED);
    assert_eq!(
        err.to_string(),
        "check failed: the pad was read, but on the way back: the pad came \
         back as PS3 (054c:0268), not XInput"
    );
}

#[test]
fn a_locked_web_config_changes_nothing() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.forced_setup = 2),
    );
    let err = info_trip(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(err.exit_code(), exit::USAGE);
    assert!(err.to_string().contains("locks web-config"), "{err}");
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
}

#[test]
fn the_trip_restores_a_ram_only_profile() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "3", false).unwrap();
    let mut log = Log::default();
    let (_, trip) = info_trip(&rig, false, &mut log).unwrap();
    let t = trip.unwrap();
    assert_eq!(t.profile, Some(3));
    assert_eq!(t.restored_profile, Some(3));
    assert!(t.verified);
    assert_eq!(
        t.note.as_deref(),
        Some("restored profile 3 Celeste (RAM only) after the restart")
    );
    assert_eq!(
        log.other,
        [
            "rebooting webconfig",
            "rebooting gamepad",
            "restoring profile 3"
        ]
    );
    let fw = rig.pad.firmware();
    assert_eq!((fw.active, fw.saved_profile), (3, 2), "RAM only");
    assert_eq!(saved_profile(&rig), 2, "flash untouched");
    assert!(rig.writes().is_empty());
    // SET_PROFILE(3) by the switch, REBOOT(1), SET_PROFILE(3) RAM only:
    // the pad went back over HTTP.
    assert_eq!(rig.channel_commands(), [0x10, 0x11, 0x10]);
    let last = rig
        .pad
        .control_log()
        .into_iter()
        .rfind(|s| !s.setup.is_in())
        .unwrap();
    assert_eq!(&last.data[last.data.len() - 2..], [3, 0], "persist = 0");
    let c = cached(&rig);
    assert_eq!(c.active_profile(), Some(3));
    assert_eq!(c.saved_profile(), Some(2));
}

#[test]
fn a_failed_restore_is_a_warning_and_the_old_note() {
    // The pad runs profile 3, which a restart can't get back to: the
    // firmware refuses it as disabled.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.active = 3;
                c.enabled_profiles &= !0b100;
            }),
    )
    .with_cache();
    let mut log = Log::default();
    let (_, trip) = info_trip(&rig, false, &mut log).unwrap();
    let t = trip.unwrap();
    assert_eq!(t.restored_profile, None);
    assert!(t.verified);
    let note = t.note.unwrap();
    assert!(
        note.starts_with(
            "it ran profile 3 before, not saved; the restart put it back on its saved \
             profile 2, and switching back failed (usage: the pad refused profile 3: that \
             profile is disabled)"
        ),
        "{note}"
    );
    assert!(
        note.ends_with("`gamepadii profile switch 3` tries again"),
        "{note}"
    );
    assert_eq!(
        log.other.last().unwrap(),
        "profile 3 not restored: usage: the pad refused profile 3: that profile is disabled"
    );
    assert_eq!(rig.pad.firmware().active, 2);
    assert_eq!(cached(&rig).active_profile(), Some(2));
    // A channel without SET_PROFILE: the same, with its reason.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.active = 3;
                c.caps = gamepadii_core::channel::Caps(
                    gamepadii_core::channel::sim::V1_CAPS
                        & !gamepadii_core::channel::Caps::SET_PROFILE,
                );
            }),
    );
    let (_, trip) = info_trip(&rig, false, &mut Log::default()).unwrap();
    let t = trip.unwrap();
    assert_eq!(t.restored_profile, None);
    let note = t.note.unwrap();
    assert!(
        note.contains("control channel has no SET_PROFILE"),
        "{note}"
    );
}

#[test]
fn the_saved_profile_needs_no_restore() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    let mut log = Log::default();
    let (_, trip) = info_trip(&rig, false, &mut log).unwrap();
    let t = trip.unwrap();
    assert_eq!(
        (t.profile, t.restored_profile, t.note),
        (Some(2), None, None)
    );
    assert_eq!(rig.channel_commands(), [0x11], "no SET_PROFILE");
    assert!(!log.other.iter().any(|l| l.contains("restor")));
}

#[test]
fn stay_drops_a_ram_only_profile_and_says_so() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "3", false).unwrap();
    let (_, trip) = info_trip(&rig, true, &mut Log::default()).unwrap();
    let t = trip.unwrap();
    assert_eq!(t.restored_profile, None);
    let note = t.note.unwrap();
    assert!(
        note.starts_with("left in web-config at http://192.168.7.1/"),
        "{note}"
    );
    assert!(
        note.contains(
            "; it ran profile 3 before, not saved; the restart put it back on its saved \
             profile 2 (`gamepadii profile switch 3` switches again once it is back in a \
             gamepad mode)"
        ),
        "{note}"
    );
    assert_eq!(rig.pad.mode(), Some(InputMode::Webconfig));
    assert_eq!(
        rig.channel_commands(),
        [0x10, 0x11],
        "no restore in web-config"
    );
    assert_eq!(rig.pad.firmware().active, 2);
    assert_eq!(cached(&rig).active_profile(), Some(2));
}

// ---- info labels the saved and the running profile (#14) -------------------

fn info_read(rig: &Rig, stay: bool) -> gamepadii_core::Result<Read> {
    rig.run(|env| flow::info(env, stay, &mut Log::default()))
}

/// `(number, saved, running)` for each profile row.
fn labels(info: &gamepadii_core::commands::Info) -> Vec<(u32, bool, Option<bool>)> {
    info.profiles
        .iter()
        .map(|p| (p.number, p.saved, p.running))
        .collect()
}

#[test]
fn info_labels_the_running_profile_when_it_is_the_saved_one() {
    let rig = Rig::cc(InputMode::Xinput);
    let (info, trip) = info_read(&rig, false).unwrap();
    assert_eq!(trip.unwrap().running_profile, Some(2));
    assert_eq!(
        (info.saved_profile, info.running_profile),
        (Some(2), Some(2))
    );
    assert_eq!(info.active_profile, Some(2), "kept for older readers");
    let l = labels(&info);
    assert_eq!(l[1], (2, true, Some(true)));
    assert!(
        l.iter()
            .filter(|(n, ..)| *n != 2)
            .all(|(_, saved, running)| !saved && *running == Some(false)),
        "{l:?}"
    );
}

#[test]
fn info_after_a_ram_only_switch_labels_both() {
    // The pad runs 3 (RAM only), saved 2: web-config lists 2 as saved; the
    // trip restores 3, and STATUS says so.
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "3", false).unwrap();
    let (info, trip) = info_read(&rig, false).unwrap();
    assert_eq!(trip.unwrap().restored_profile, Some(3));
    assert_eq!(
        (info.saved_profile, info.running_profile),
        (Some(2), Some(3))
    );
    let l = labels(&info);
    assert_eq!(l[1], (2, true, Some(false)));
    assert_eq!(l[2], (3, false, Some(true)));
    // The cache keeps both too.
    let c = cached(&rig);
    assert_eq!((c.saved_profile(), c.active_profile()), (Some(2), Some(3)));
}

#[test]
fn info_after_a_failed_restore_labels_what_the_pad_runs() {
    // Profile 3 can't be switched back to: the pad runs its saved 2.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.active = 3;
                c.enabled_profiles &= !0b100;
            }),
    );
    let (info, _) = info_read(&rig, false).unwrap();
    assert_eq!(
        (info.saved_profile, info.running_profile),
        (Some(2), Some(2))
    );
}

#[test]
fn info_with_stay_labels_what_web_config_runs() {
    let rig = Rig::cc(InputMode::Xinput);
    switch(&rig, "3", false).unwrap();
    let (info, trip) = info_read(&rig, true).unwrap();
    assert!(trip.unwrap().stayed);
    // The restart dropped 3; STATUS in web-config says 2.
    assert_eq!(info.running_profile, Some(2));
    assert_eq!(labels(&info)[1], (2, true, Some(true)));
}

#[test]
fn info_without_status_after_the_trip_says_unknown() {
    // A channel with no STATUS: the trip works, the running profile is
    // unknown, and nothing stands in for it.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                c.caps = gamepadii_core::channel::Caps(
                    gamepadii_core::channel::sim::V1_CAPS & !gamepadii_core::channel::Caps::STATUS,
                );
            }),
    );
    let (info, trip) = info_read(&rig, false).unwrap();
    let t = trip.unwrap();
    assert!(t.verified, "the VID:PID checked out");
    assert_eq!(t.running_profile, None);
    assert!(
        t.note.as_deref().unwrap().contains("didn't answer STATUS"),
        "{:?}",
        t.note
    );
    assert_eq!(info.saved_profile, Some(2));
    assert_eq!(info.running_profile, None);
    assert!(
        labels(&info)
            .iter()
            .all(|(_, _, running)| running.is_none())
    );
}

#[test]
fn info_in_web_config_does_not_guess_the_running_profile() {
    let rig = Rig::cc(InputMode::Webconfig);
    let (info, trip) = info_read(&rig, false).unwrap();
    assert_eq!(trip, None);
    assert_eq!((info.saved_profile, info.running_profile), (Some(2), None));
    assert!(rig.pad.control_log().is_empty(), "no probe in web-config");
}

// ---- the cache is kept per pad, by its gamepad-mode serial (#11) ------------

/// XInput serial `0C0FFEE`.
const PAD_A: [u8; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0xC0, 0xFF, 0xEE];
/// XInput serial `0BADDAD`.
const PAD_B: [u8; 8] = [0x66, 0x77, 0x88, 0x99, 0xAA, 0xBA, 0xDD, 0xAD];
/// XInput serial `0F00BAA`.
const PAD_C: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0xF0, 0x0B, 0xAA];

/// The S16 capture with profile 2 called `label`, so its cache can be told
/// apart.
fn relabelled(label: &str) -> Mock {
    let mock = Mock::new(s16_dir());
    let mut p = mock.current("getProfileOptions").unwrap();
    p["alternativePinMappings"][0]["profileLabel"] = label.into();
    mock.store("getProfileOptions", p);
    mock
}

/// The guide `mode switch --no-wait` gives on a pad without the channel.
fn guide_text(rig: &Rig) -> String {
    rig.mode(InputMode::Switch, false, &mut Log::default())
        .unwrap_err()
        .to_string()
}

fn cache_file(rig: &Rig, name: &str) -> DeviceCache {
    let b = std::fs::read(rig.cache_dir().join(name)).unwrap();
    serde_json::from_slice(&b).unwrap()
}

fn profile2(c: &DeviceCache) -> String {
    c.profile_options["alternativePinMappings"][0]["profileLabel"]
        .as_str()
        .unwrap()
        .to_string()
}

impl Rig {
    /// Another pad (or the same one again) in the same socket, with the
    /// same cache. A replug is a new connection: a new device number.
    fn replug(&mut self, pad: SimPad) {
        let devnum = gamepadii_core::discovery::Scan::scan(&*self.pad)
            .first()
            .and_then(|d| d.devnum)
            .unwrap_or(7);
        self.pad = Arc::new(pad.with_devnum(devnum + 1));
    }
}

#[test]
fn only_modes_with_a_unique_serial_name_the_pad() {
    let id = |mode| cache::PadId::of(&sim::device_with_id(mode, 7, Some(PAD_A)));
    assert_eq!(
        id(InputMode::Xinput),
        Some(cache::PadId {
            serial: Some("0C0FFEE".into()),
            mode: Some(InputMode::Xinput),
            ..cache::PadId::default()
        })
    );
    let xbone = id(InputMode::Xbone).unwrap();
    assert_eq!(xbone.serial.as_deref(), Some("RJBSKRFN8ABCDEFGH"));
    for mode in [
        InputMode::Webconfig,
        InputMode::SwitchPro,
        InputMode::Keyboard,
        InputMode::Generic,
        InputMode::Ps4,
    ] {
        assert_eq!(id(mode), None, "{mode:?}");
    }
    // usbDescOverride: XInput's serial is the user's string.
    let mut d = sim::device_with_id(InputMode::Xinput, 7, Some(PAD_A));
    d.serial = Some("1.0".into());
    assert_eq!(cache::PadId::of(&d), None);
    d.serial = None;
    assert_eq!(cache::PadId::of(&d), None);
}

#[test]
fn two_pads_with_one_board_keep_separate_caches() {
    let mut rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    info_read(&rig, false).unwrap();
    rig.clock.advance(Duration::from_secs(60));
    rig.replug(
        SimPad::with_mock(relabelled("Hollow"), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_B),
    );
    info_read(&rig, false).unwrap();
    assert_eq!(
        files(&rig.cache_dir()),
        [
            "Haute42COSMOX-0BADDAD.json",
            "Haute42COSMOX-0C0FFEE.json",
            cache::CHANNELS_FILE,
            cache::SESSIONS_FILE,
        ]
    );
    let a = cache_file(&rig, "Haute42COSMOX-0C0FFEE.json");
    assert_eq!(a.pad.as_ref().unwrap().serial.as_deref(), Some("0C0FFEE"));
    assert_eq!(profile2(&a), "SilkSong");
    let b = cache_file(&rig, "Haute42COSMOX-0BADDAD.json");
    assert_eq!(b.pad.as_ref().unwrap().serial.as_deref(), Some("0BADDAD"));
    assert_eq!(profile2(&b), "Hollow");
    assert!(b.captured_at > a.captured_at);

    // In a gamepad mode, on stock firmware (the guide), each pad gets its
    // own hints, not the newest.
    rig.replug(SimPad::new(s16_dir(), InputMode::Xinput).unique_id(PAD_A));
    let msg = guide_text(&rig);
    assert!(msg.contains("In profile 2 (SilkSong)"), "{msg}");
    assert!(!msg.contains("Hollow"), "{msg}");
    rig.replug(SimPad::with_mock(relabelled("Hollow"), InputMode::Xinput).unique_id(PAD_B));
    let msg = guide_text(&rig);
    assert!(msg.contains("In profile 2 (Hollow)"), "{msg}");
    // A third pad of that board: neither cache is its.
    rig.replug(SimPad::new(s16_dir(), InputMode::Xinput).unique_id(PAD_C));
    let msg = guide_text(&rig);
    assert!(
        !msg.contains("SilkSong") && !msg.contains("Hollow"),
        "{msg}"
    );
    // Keyboard has no serial: it can't be told apart, so the newest.
    let mock = relabelled("Hollow");
    let mut o = mock.current("getGamepadOptions").unwrap();
    o["inputMode"] = 3.into();
    mock.store("getGamepadOptions", o);
    rig.replug(SimPad::with_mock(mock, InputMode::Keyboard).unique_id(PAD_B));
    let msg = guide_text(&rig);
    assert!(msg.contains("In profile 2 (Hollow)"), "{msg}");
}

#[test]
fn the_running_profile_goes_to_the_right_pad() {
    let mut rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    info_read(&rig, false).unwrap();
    rig.clock.advance(Duration::from_secs(60));
    rig.replug(
        SimPad::with_mock(relabelled("Hollow"), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_B),
    );
    info_read(&rig, false).unwrap();
    // Back to A, the older cache: a RAM-only switch is recorded in A's.
    rig.replug(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    switch(&rig, "3", false).unwrap();
    let a = cache_file(&rig, "Haute42COSMOX-0C0FFEE.json");
    assert_eq!(a.active_profile(), Some(3));
    let b = cache_file(&rig, "Haute42COSMOX-0BADDAD.json");
    assert_eq!(b.active_profile(), Some(2), "B's untouched");
}

#[test]
fn an_old_board_file_serves_until_the_pad_is_read() {
    let mut rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).unique_id(PAD_A)).with_cache();
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX.json"]);
    let msg = guide_text(&rig);
    assert!(msg.contains("In profile 2 (SilkSong)"), "{msg}");
    // The first read of a pad of that board replaces it.
    rig.replug(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    info_read(&rig, false).unwrap();
    assert_eq!(
        files(&rig.cache_dir()),
        [
            "Haute42COSMOX-0C0FFEE.json",
            cache::CHANNELS_FILE,
            // The stock firmware's connection, before the replug.
            cache::NO_CHANNEL_FILE,
            cache::SESSIONS_FILE,
        ]
    );
}

#[test]
fn a_pad_without_a_unique_serial_stays_by_board() {
    // Keyboard has no serial: the trip still works, the cache is the
    // board's as before.
    let mock = Mock::new(s16_dir());
    let mut o = mock.current("getGamepadOptions").unwrap();
    o["inputMode"] = 3.into();
    mock.store("getGamepadOptions", o);
    let rig = Rig::new(
        SimPad::with_mock(mock, InputMode::Keyboard)
            .with_channel()
            .unique_id(PAD_A),
    );
    info_read(&rig, false).unwrap();
    assert_eq!(
        files(&rig.cache_dir()),
        ["Haute42COSMOX.json", cache::CHANNELS_FILE]
    );
    let c = cached(&rig);
    assert_eq!(c.pad, None);
}

#[test]
fn web_config_reads_go_to_the_pad_seen_going_there() {
    // `gamepadii webconfig` over the channel: gamepadii saw 0C0FFEE go in.
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    rig.run(|env| flow::webconfig(env, true, &mut Log::default()))
        .unwrap();
    assert_eq!(rig.pad.mode(), Some(InputMode::Webconfig));
    let keyed = rig.cache_dir().join("Haute42COSMOX-0C0FFEE.json");
    assert!(keyed.exists(), "{:?}", files(&rig.cache_dir()));
    // A later `info`, in web-config: the same pad (serial 123456 there).
    std::fs::remove_file(&keyed).unwrap();
    info_read(&rig, false).unwrap();
    assert!(keyed.exists(), "{:?}", files(&rig.cache_dir()));
    assert!(!rig.cache_dir().join("Haute42COSMOX.json").exists());
}

#[test]
fn a_replug_in_web_config_is_another_session() {
    let mut rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    rig.run(|env| flow::webconfig(env, true, &mut Log::default()))
        .unwrap();
    // Replugged holding S2: a new enumeration, which could be any pad.
    rig.replug(SimPad::new(s16_dir(), InputMode::Webconfig).unique_id(PAD_B));
    info_read(&rig, false).unwrap();
    let c = cached(&rig);
    assert_eq!(c.pad, None, "board file");
    // The reboot into web-config made device 8, the replug device 9.
    assert_eq!(c.read_in.as_ref().unwrap().devnum, 9);
}

#[test]
fn a_read_by_hand_is_attributed_when_the_pad_leaves_web_config() {
    // Put into web-config by hand: gamepadii can't tell which pad it is.
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).unique_id(PAD_A));
    info_read(&rig, false).unwrap();
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX.json"]);
    // `gamepadii webconfig --exit`: it comes back in XInput as 0C0FFEE.
    rig.run(|env| {
        let client = (env.open)().unwrap();
        flow::reboot(env, &client, BootMode::Gamepad, true, &mut Log::default())
    })
    .unwrap();
    assert_eq!(files(&rig.cache_dir()), ["Haute42COSMOX-0C0FFEE.json"]);
    let c = cache_file(&rig, "Haute42COSMOX-0C0FFEE.json");
    assert_eq!(c.pad.unwrap().serial.as_deref(), Some("0C0FFEE"));
    assert_eq!(c.read_in, None);
}

// ---- leds -------------------------------------------------------------------------

fn leds_read(rig: &Rig) -> gamepadii_core::Result<(LedReport, Option<flow::Trip>)> {
    rig.run(|env| flow::leds(env, false, &mut Log::default()))
}

fn leds_set(rig: &Rig, change: &LedChange) -> gamepadii_core::Result<LedSetReport> {
    rig.run(|env| flow::leds_set(env, change))
}

fn pick(s: &str) -> Option<Pick> {
    Some(s.parse().unwrap())
}

#[test]
fn leds_over_the_channel_need_no_reboot() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let (r, trip) = leds_read(&rig).unwrap();
    assert_eq!(trip, None);
    assert_eq!(r.via, flow::Route::Control);
    assert_eq!(
        r.state.animation.as_ref().unwrap().display(),
        "custom-theme (4)"
    );
    assert_eq!(r.state.brightness, Some(1));
    assert_eq!(r.state.brightness_steps, Some(5));
    assert_eq!(r.state.static_color.unwrap().name.as_deref(), Some("red"));
    assert_eq!(r.state.unsaved, Some(false));
    assert_eq!(r.custom_theme_colors, None);
    assert!(reboots(&rig).is_empty() && rig.requests().is_empty());
    // In web-config the channel answers too: still LED_STATUS.
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Webconfig).with_leds());
    let (r, _) = leds_read(&rig).unwrap();
    assert_eq!(r.via, flow::Route::Control);
    assert!(rig.requests().is_empty());
}

#[test]
fn leds_on_stock_in_web_config_are_partly_unknown() {
    let rig = Rig::s16(InputMode::Webconfig);
    let (r, trip) = leds_read(&rig).unwrap();
    assert_eq!(trip, None);
    assert_eq!(r.via, flow::Route::WebConfig);
    assert_eq!(r.state.animation, None);
    assert_eq!(r.state.brightness, None);
    assert_eq!(r.state.brightness_steps, Some(5));
    assert_eq!(r.state.brightness_maximum, Some(100));
    assert_eq!(r.state.custom_theme, Some(true));
    assert_eq!(r.state.running, Some(true));
    let colors = r.custom_theme_colors.unwrap();
    assert_eq!(colors.len(), 18);
    assert_eq!(colors[0].button, "Up");
    assert_eq!(colors[0].normal.as_deref(), Some("#0000ff"));
    assert_eq!(colors[0].pressed.as_deref(), Some("#0080ff"));
    assert!(r.note.unwrap().contains("unknown on stock firmware"));
    assert!(!asked_for_get_config(&rig));
}

#[test]
fn leds_on_stock_in_a_gamepad_mode_need_the_user() {
    let rig = Rig::s16(InputMode::Xinput);
    let e = leds_read(&rig).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("XInput mode, not web-config"), "{e}");
}

#[test]
fn leds_on_a_v1_0_fork_go_through_web_config() {
    // The S16 today: the fork's v1.0 channel, no LED commands.
    let rig = Rig::new(SimPad::new(fork_dir(), InputMode::Xinput).with_channel());
    let (r, trip) = leds_read(&rig).unwrap();
    let trip = trip.unwrap();
    assert!(trip.verified);
    assert_eq!(r.via, flow::Route::WebConfig);
    assert!(asked_for_get_config(&rig));
    assert_eq!(r.state.animation.unwrap().display(), "chase (2)");
    assert_eq!(r.state.brightness, Some(3));
    assert_eq!(
        r.state.theme.unwrap().name.as_deref(),
        Some("static-rainbow")
    );
    assert_eq!(r.note, None);
    assert_eq!(rig.pad.mode(), Some(InputMode::Xinput));
}

#[test]
fn leds_set_is_ram_only_by_default_and_verified() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let change = LedChange {
        animation: pick("rainbow"),
        brightness: Some(Level::To(3)),
        ..LedChange::default()
    };
    let r = leds_set(&rig, &change).unwrap();
    assert_eq!(r.changed, ["animation", "brightness"]);
    assert!(r.verified && !r.saved);
    assert_eq!(r.leds.animation.unwrap().display(), "rainbow (1)");
    assert_eq!(r.leds.unsaved, Some(true));
    assert!(r.note.unwrap().contains("not saved"));
    assert_eq!(
        rig.pad.firmware().saved_leds.animation,
        4,
        "flash untouched"
    );
    assert_eq!(rig.channel_commands(), [0x13]);
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
}

#[test]
fn leds_set_save_and_a_bare_save() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let r = leds_set(
        &rig,
        &LedChange {
            theme: pick("tekken"),
            save: true,
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(r.saved && r.verified);
    assert_eq!(r.note, None);
    assert_eq!(rig.pad.firmware().saved_leds.theme, 13);
    // Nothing unsaved: --save alone sends nothing.
    let r = leds_set(
        &rig,
        &LedChange {
            save: true,
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(!r.saved && r.changed.is_empty());
    assert!(r.note.unwrap().contains("nothing to save"));
    assert_eq!(rig.channel_commands(), [0x13]);
    // A RAM change, then --save alone saves it.
    leds_set(
        &rig,
        &LedChange {
            button_color: pick("blue"),
            ..LedChange::default()
        },
    )
    .unwrap();
    let r = leds_set(
        &rig,
        &LedChange {
            save: true,
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(r.saved);
    assert_eq!(rig.pad.firmware().saved_leds.button_color, 10);
}

#[test]
fn leds_brightness_steps_and_stops_at_the_ends() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let step = |level| {
        leds_set(
            &rig,
            &LedChange {
                brightness: Some(level),
                ..LedChange::default()
            },
        )
        .unwrap()
    };
    assert_eq!(step(Level::Down).leds.brightness, Some(0));
    let r = step(Level::Down);
    assert!(r.changed.is_empty());
    assert!(r.note.unwrap().contains("dimmest"));
    assert_eq!(step(Level::To(5)).leds.brightness, Some(5));
    assert!(step(Level::Up).note.unwrap().contains("brightest"));
}

#[test]
fn leds_set_refuses_what_the_pad_lacks() {
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let e = leds_set(
        &rig,
        &LedChange {
            static_color: pick("mauve"),
            ..LedChange::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(rig.channel_commands().is_empty(), "nothing sent");
    // A v1.0 channel, stock firmware, no channel, BOOTSEL: exit 5.
    let change = LedChange {
        brightness: Some(Level::Up),
        ..LedChange::default()
    };
    for (rig, says) in [
        (Rig::cc(InputMode::Xinput), "v1.0 without the LED commands"),
        (Rig::s16(InputMode::Xinput), "LED hotkeys"),
        (Rig::cc(InputMode::Bootsel), "BOOTSEL"),
    ] {
        let e = leds_set(&rig, &change).unwrap_err();
        assert_eq!(e.exit_code(), exit::NEEDS_USER, "{e}");
        assert!(e.to_string().contains(says), "{e}");
        assert!(rig.channel_commands().is_empty());
    }
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_leds()
            .no_access(),
    );
    let e = leds_set(&rig, &change).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("gamepadii setup udev"), "{e}");
    // waybar's read never takes a trip.
    let rig = Rig::cc(InputMode::Xinput);
    let e = rig.run(flow::leds_now).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(reboots(&rig).is_empty());
}

// ---- lights on main (control channel v1.2) ------------------------------------

fn main_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main")
}

fn main_lights(mode: InputMode) -> Rig {
    Rig::new(SimPad::new(main_dir(), mode).with_lights())
}

/// The IN commands the pad got.
fn reads(rig: &Rig) -> Vec<u8> {
    rig.pad
        .control_log()
        .into_iter()
        .filter(|s| s.setup.is_in())
        .map(|s| s.setup.command_byte())
        .collect()
}

#[test]
fn leds_on_main_read_lights_status_with_no_reboot() {
    let rig = main_lights(InputMode::Xinput);
    let (r, trip) = leds_read(&rig).unwrap();
    assert_eq!(trip, None);
    assert_eq!(r.via, flow::Route::Control);
    assert_eq!(
        (r.state.lights_off, r.state.lights_profile),
        (Some(false), Some(0))
    );
    assert_eq!(r.state.brightness, Some(5));
    assert_eq!(r.state.brightness_steps, Some(10));
    assert_eq!(r.state.animation, None);
    let lights = r.lights.unwrap();
    assert_eq!(lights.profile, Some(0));
    assert_eq!(
        lights.profiles[0].idle_effect.as_ref().unwrap().display(),
        "rainbow-synced (1)"
    );
    assert_eq!(lights.profiles[1].idle_effect, None);
    assert_eq!(r.note, None);
    assert!(reads(&rig).contains(&0x05) && !reads(&rig).contains(&0x04));
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
    // waybar's read takes the same path.
    let r = rig.run(flow::leds_now).unwrap();
    assert_eq!(r.state.lights_profile, Some(0));
}

#[test]
fn leds_set_on_main_is_set_lights() {
    let rig = main_lights(InputMode::Xinput);
    let r = leds_set(
        &rig,
        &LedChange {
            lights_profile: Some(LightsPick::Number(3)),
            brightness: Some(Level::Up),
            ..LedChange::default()
        },
    )
    .unwrap();
    assert_eq!(r.changed, ["lights_profile", "brightness"]);
    assert!(r.verified && !r.saved);
    assert_eq!(
        (r.before.lights_profile, r.leds.lights_profile),
        (Some(0), Some(3))
    );
    assert_eq!(r.leds.brightness, Some(6));
    assert!(r.note.unwrap().contains("not saved"));
    assert_eq!(
        rig.pad.firmware().saved_lights.profile,
        0,
        "flash untouched"
    );
    assert_eq!(rig.channel_commands(), [0x14]);
    // Off, and saved.
    let r = leds_set(
        &rig,
        &LedChange {
            lights_profile: Some(LightsPick::Off),
            save: true,
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(r.saved && r.verified);
    assert_eq!(
        (r.leds.lights_off, r.leds.lights_profile),
        (Some(true), None)
    );
    assert_eq!(r.note, None);
    let saved = rig.pad.firmware().saved_lights;
    assert_eq!((saved.profile, saved.brightness), (-1, 6));
    // Nothing left to save: --save alone sends nothing.
    let r = leds_set(
        &rig,
        &LedChange {
            save: true,
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(r.changed.is_empty() && !r.saved);
    assert_eq!(rig.channel_commands(), [0x14, 0x14]);
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
}

#[test]
fn leds_set_on_main_refuses_what_main_lacks() {
    let rig = Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| c.lights_limits.profiles[1].enabled = false),
    );
    for (change, says) in [
        (
            LedChange {
                animation: pick("rainbow"),
                ..LedChange::default()
            },
            "option --animation: main firmware",
        ),
        (
            LedChange {
                lights_profile: Some(LightsPick::Number(1)),
                ..LedChange::default()
            },
            "lights profile 1 is disabled; this pad has 0, 2, 3, off",
        ),
        (
            LedChange {
                brightness: Some(Level::To(11)),
                ..LedChange::default()
            },
            "levels 0 to 10",
        ),
    ] {
        let e = leds_set(&rig, &change).unwrap_err();
        assert_eq!(e.exit_code(), exit::USAGE, "{e}");
        assert!(e.to_string().contains(says), "{e}");
    }
    assert!(rig.channel_commands().is_empty(), "nothing sent");
    // A web-config preview runs: the user stops it first.
    let rig = Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| c.lights_limits.preview = true),
    );
    let (r, _) = leds_read(&rig).unwrap();
    assert!(r.note.unwrap().contains("preview"));
    let e = leds_set(
        &rig,
        &LedChange {
            brightness: Some(Level::Down),
            ..LedChange::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("stop the preview"), "{e}");
    assert!(rig.channel_commands().is_empty());
    // A v1.1 pad has no lights profiles.
    let rig = Rig::new(SimPad::new(s16_dir(), InputMode::Xinput).with_leds());
    let e = leds_set(
        &rig,
        &LedChange {
            lights_profile: Some(LightsPick::Off),
            ..LedChange::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(rig.channel_commands().is_empty());
}

#[test]
fn leds_set_on_main_waits_for_pending() {
    // v1.5: LIGHTS_STATUS shows the change before core 1 applies it.
    let rig = Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| c.lights_limits.pending_reads = 3),
    );
    let r = leds_set(
        &rig,
        &LedChange {
            brightness: Some(Level::To(2)),
            ..LedChange::default()
        },
    )
    .unwrap();
    assert!(r.verified);
    assert_eq!(r.leds.brightness, Some(2));
    let reads = rig
        .pad
        .control_log()
        .into_iter()
        .filter(|s| s.setup.is_in() && s.setup.command_byte() == 0x05)
        .count();
    // The read before, 3 that show PENDING, then the one without it.
    assert!(reads >= 5, "{reads}");
    assert!(rig.pad.firmware().lights_pending == 0);
}

// ---- profile slots (control channel v1.4) ------------------------------------

fn slots_rig(mode: InputMode) -> Rig {
    Rig::new(
        SimPad::new(s16_dir(), mode)
            .with_channel()
            .channel(|c| *c = c.clone().with_slots()),
    )
}

fn slot_file(json: &str) -> SlotFile {
    SlotFile::parse(json).unwrap()
}

fn write_slot(
    rig: &Rig,
    file: &SlotFile,
    write: &SlotWrite,
) -> gamepadii_core::Result<SlotWriteReport> {
    rig.run(|env| flow::profile_write(env, file, write, &mut Log::default()))
}

#[test]
fn profile_show_reads_a_slot_live() {
    let rig = slots_rig(InputMode::Xinput);
    let r = rig.run(|env| flow::profile_show(env, None)).unwrap();
    assert_eq!((r.profile, r.label.as_str()), (2, "SilkSong"));
    assert!(r.active && r.enabled);
    assert_eq!(r.board.as_deref(), Some("Haute42COSMOX"));
    assert_eq!(r.pins.len(), 30);
    assert_eq!(r.pins[2].action_name.as_deref(), Some("B1"));
    assert!(r.pins[0].from_profile_1);
    let by_label = rig
        .run(|env| flow::profile_show(env, Some(&"celeste".parse().unwrap())))
        .unwrap();
    assert_eq!(by_label.profile, 3);
    let base = rig
        .run(|env| flow::profile_show(env, Some(&"1".parse().unwrap())))
        .unwrap();
    assert!(base.pins.iter().all(|p| !p.from_profile_1));
    assert!(rig.channel_commands().is_empty(), "reads only");
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
}

#[test]
fn profile_write_selects_a_scratch_slot_in_ram() {
    let rig = slots_rig(InputMode::Xinput).with_cache();
    let file = slot_file(
        r#"{"label": "Hades", "pins": [
            {"gpio": 2, "action": "B2"}, {"gpio": 13, "action": "B1"},
            {"gpio": 0, "action": "B3"}
        ]}"#,
    );
    let write = SlotWrite {
        slot: 6,
        select: true,
        ..SlotWrite::default()
    };
    let r = write_slot(&rig, &file, &write).unwrap();
    assert_eq!((r.profile, r.label.as_str()), (6, "Hades"));
    assert!(r.active && r.enabled && !r.saved && r.verified);
    assert_eq!(r.applied, Some(true));
    assert_eq!((r.before_label.as_str(), r.before_enabled), ("", false));
    let note = r.note.unwrap();
    assert!(note.contains("GPIO 0 keeps profile 1's action"), "{note}");
    assert!(note.contains("not saved"), "{note}");
    assert_eq!(rig.channel_commands(), [0x15]);
    let fw = rig.pad.firmware();
    assert_eq!(fw.active, 6);
    assert_eq!(fw.slots[&6].pins[2].action, 6);
    assert!(!fw.saved_slots.contains_key(&6), "flash untouched");
    // The backup is the empty slot 6, as a slot file.
    let backup = r.backup.unwrap();
    let back = SlotFile::parse(&std::fs::read_to_string(&backup).unwrap()).unwrap();
    assert_eq!((back.profile, back.enabled), (Some(6), Some(false)));
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("-Haute42COSMOX-profile6.json")
    );
    // The cache follows the running profile, and keeps the saved pins.
    let c = cache::load_latest(&rig.cache_dir()).unwrap();
    assert_eq!(c.running_profile.unwrap().number, 6);
    assert!(c.profile_options["alternativePinMappings"].get(4).is_none());
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
}

#[test]
fn profile_write_save_updates_flash_and_the_cache() {
    let rig = slots_rig(InputMode::Xinput).with_cache();
    let shown = rig
        .run(|env| flow::profile_show(env, Some(&"3".parse().unwrap())))
        .unwrap();
    let mut file = shown.file();
    file.pins[6].action = gamepadii_core::slots::Action(8);
    let write = SlotWrite {
        slot: 3,
        save: true,
        label: Some("Celeste 2".into()),
        ..SlotWrite::default()
    };
    let r = write_slot(&rig, &file, &write).unwrap();
    assert_eq!(r.changed_pins, [6]);
    assert!(r.saved && !r.active);
    assert_eq!(r.applied, None, "not the active profile");
    assert_eq!(r.note, None);
    let fw = rig.pad.firmware();
    assert_eq!(fw.saved_slots[&3].label, "Celeste 2");
    let c = cache::load_latest(&rig.cache_dir()).unwrap();
    let alt = &c.profile_options["alternativePinMappings"][1];
    assert_eq!(alt["profileLabel"], "Celeste 2");
    assert_eq!(alt["pin06"]["action"], 8);
    // The backup undoes it.
    let backup = SlotFile::parse(&std::fs::read_to_string(r.backup.unwrap()).unwrap()).unwrap();
    let undo = SlotWrite {
        slot: 3,
        save: true,
        ..SlotWrite::default()
    };
    let r = write_slot(&rig, &backup, &undo).unwrap();
    assert_eq!(
        (r.label.as_str(), r.changed_pins.as_slice()),
        ("Celeste", &[6u8][..])
    );
}

#[test]
fn profile_write_refuses_before_it_sends() {
    let rig = slots_rig(InputMode::Xinput);
    let w = |slot, select| SlotWrite {
        slot,
        select,
        ..SlotWrite::default()
    };
    for (json, write, says) in [
        (
            r#"{"board": "Pico", "pins": []}"#,
            w(6, false),
            "for board Pico",
        ),
        (
            r#"{"pins": [{"gpio": 30, "action": 5}]}"#,
            w(6, false),
            "GPIO 0 to 29",
        ),
        (
            r#"{"label": "an 18-byte label!!", "pins": []}"#,
            w(6, false),
            "16",
        ),
        (
            r#"{"enabled": false, "pins": []}"#,
            w(2, false),
            "the active profile",
        ),
        (
            r#"{"enabled": false, "pins": []}"#,
            w(6, true),
            "the one to select",
        ),
        (r#"{"pins": []}"#, w(1, false), "profiles 2 to 6"),
    ] {
        let e = write_slot(&rig, &slot_file(json), &write).unwrap_err();
        assert_eq!(e.exit_code(), exit::USAGE, "{e}");
        assert!(e.to_string().contains(says), "{e}");
    }
    assert!(rig.channel_commands().is_empty(), "nothing sent");
    assert!(files(&rig.backups_dir()).is_empty(), "no backup either");
    // No v1.4 channel: exit 5, nothing sent.
    let file = slot_file(r#"{"pins": []}"#);
    for (rig, says) in [
        (Rig::cc(InputMode::Xinput), "v1.0"),
        (Rig::s16(InputMode::Xinput), "control channel v1.4"),
        (main_lights(InputMode::Xinput), "v1.2"),
    ] {
        let e = write_slot(&rig, &file, &w(6, true)).unwrap_err();
        assert_eq!(e.exit_code(), exit::NEEDS_USER, "{e}");
        assert!(e.to_string().contains(says), "{e}");
        assert!(rig.channel_commands().is_empty());
        let e = rig.run(|env| flow::profile_show(env, None)).unwrap_err();
        assert_eq!(e.exit_code(), exit::NEEDS_USER);
    }
}

// ---- lights profiles (control channel v1.5) ----------------------------------

fn lights_profiles_rig(pending_reads: u8) -> Rig {
    Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| {
                *c = c.clone().with_lights_profiles();
                c.lights_limits.pending_reads = pending_reads;
            }),
    )
}

fn write_lights(
    rig: &Rig,
    file: &str,
    write: &LightsWrite,
) -> gamepadii_core::Result<LightsWriteReport> {
    let file = LightsFile::parse(file)?;
    rig.run(|env| flow::lights_write(env, &file, write, &mut Log::default()))
}

#[test]
fn lights_show_reads_a_lights_profile_live() {
    let rig = lights_profiles_rig(0);
    let r = rig.run(|env| flow::lights_show(env, None)).unwrap();
    assert_eq!(r.file.profile, Some(0));
    assert!(r.active && r.file.enabled == Some(true));
    assert_eq!(r.file.board.as_deref(), Some("Haute42COSMOX"));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["idle_effect_name"], "rainbow-synced");
    let r = rig.run(|env| flow::lights_show(env, Some(3))).unwrap();
    assert!(!r.active);
    assert!(rig.channel_commands().is_empty(), "reads only");
    assert!(rig.requests().is_empty() && reboots(&rig).is_empty());
}

#[test]
fn lights_show_needs_a_number_when_the_lights_are_off() {
    let rig = Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| {
                *c = c.clone().with_lights_profiles();
                c.lights.profile = -1;
            }),
    );
    let e = rig.run(|env| flow::lights_show(env, None)).unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(e.to_string().contains("the lights are off"), "{e}");
}

#[test]
fn lights_write_selects_in_ram_and_waits_for_core_1() {
    let rig = lights_profiles_rig(3);
    let write = LightsWrite {
        profile: 2,
        select: true,
        ..LightsWrite::default()
    };
    let r = write_lights(
        &rig,
        r#"{"idle_effect": "rain", "idle_cycle_time": 4}"#,
        &write,
    )
    .unwrap();
    assert!(r.active && r.applied && r.verified && !r.saved);
    assert_eq!(r.changed, ["idle_effect", "idle_cycle_time"]);
    assert!(r.note.unwrap().contains("not saved"));
    assert_eq!(rig.channel_commands(), [0x16]);
    let fw = rig.pad.firmware();
    assert_eq!((fw.lights.profile, fw.lights_pending), (2, 0));
    assert!(fw.saved_lights_profiles.is_some(), "flash untouched");
    // The backup is profile 2 as it was: a write of it undoes the change.
    let backup = r.backup.unwrap();
    assert!(
        backup
            .to_str()
            .unwrap()
            .ends_with("-Haute42COSMOX-lights2.json")
    );
    let text = std::fs::read_to_string(&backup).unwrap();
    let undo = LightsWrite {
        profile: 2,
        ..LightsWrite::default()
    };
    let r = write_lights(&rig, &text, &undo).unwrap();
    assert_eq!(r.changed, ["idle_effect", "idle_cycle_time"]);
    assert!(r.active, "it still runs");
}

#[test]
fn lights_write_save_survives_a_restart() {
    let rig = lights_profiles_rig(0);
    let write = LightsWrite {
        profile: 1,
        save: true,
        ..LightsWrite::default()
    };
    let r = write_lights(&rig, r##"{"pressed_special_color": "#123456"}"##, &write).unwrap();
    assert!(r.saved && !r.active && r.note.is_none());
    let fw = rig.pad.firmware();
    assert!(fw.saved_lights_profiles.is_none(), "flash has it");
    assert_eq!(
        fw.lights_fields[1][&12],
        gamepadii_core::pb::Value::Varint(0x123456)
    );
}

#[test]
fn lights_write_refuses_before_it_sends() {
    let rig = lights_profiles_rig(0);
    let w = |profile, select| LightsWrite {
        profile,
        select,
        ..LightsWrite::default()
    };
    for (json, write, says) in [
        (r#"{"board": "Pico"}"#, w(1, false), "for board Pico"),
        (r#"{"enabled": false}"#, w(0, false), "runs now"),
        (r#"{"enabled": false}"#, w(1, true), "the one to select"),
        (r#"{"idle_effect": 20}"#, w(1, false), "0 to 19"),
        (r#"{"case_cycle_time": -1}"#, w(1, false), "0 to 9"),
        (r#"{}"#, w(4, false), "0 to 3"),
    ] {
        let e = write_lights(&rig, json, &write).unwrap_err();
        assert_eq!(e.exit_code(), exit::USAGE, "{e}");
        assert!(e.to_string().contains(says), "{e}");
    }
    assert!(rig.channel_commands().is_empty(), "nothing sent");
    assert!(files(&rig.backups_dir()).is_empty(), "no backup either");
    // A web-config preview: exit 5, nothing sent.
    let rig = Rig::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| {
                *c = c.clone().with_lights_profiles();
                c.lights_limits.preview = true;
            }),
    );
    let e = write_lights(&rig, "{}", &w(1, false)).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(rig.channel_commands().is_empty());
    // No v1.5 channel: exit 5, nothing sent.
    for (rig, says) in [
        (main_lights(InputMode::Xinput), "v1.2"),
        (Rig::s16(InputMode::Xinput), "control channel v1.5"),
        (slots_rig(InputMode::Xinput), "v1.4"),
    ] {
        let e = write_lights(&rig, "{}", &w(1, false)).unwrap_err();
        assert_eq!(e.exit_code(), exit::NEEDS_USER, "{e}");
        assert!(e.to_string().contains(says), "{e}");
        assert!(rig.channel_commands().is_empty());
        let e = rig.run(|env| flow::lights_show(env, Some(0))).unwrap_err();
        assert_eq!(e.exit_code(), exit::NEEDS_USER);
    }
}

// ---- the board ID (control channel v1.3) -------------------------------------

/// The flash chip ID `1122334455C0FFEE`, XInput serial `0C0FFEE`.
const PAD_A_ID: &str = "1122334455C0FFEE";

/// STRING requests the pad got, by id.
fn strings_asked(rig: &Rig) -> Vec<u16> {
    rig.pad
        .control_log()
        .into_iter()
        .filter(|s| s.setup.is_in() && s.setup.command_byte() == 0x03)
        .map(|s| s.setup.value)
        .collect()
}

fn status_of(rig: &Rig) -> gamepadii_core::commands::Status {
    let control = |d: &gamepadii_core::discovery::Device| rig.pad.open_control(d);
    let dir = rig.cache_dir();
    gamepadii_core::commands::status_with(&*rig.pad, Some(&control), Some(&dir)).unwrap()
}

#[test]
fn only_a_1_3_pad_is_asked_for_its_board_id() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.minor = 2),
    );
    let s = status_of(&rig);
    let report = s.devices[0].control_channel.as_ref().unwrap();
    assert_eq!(report.board_id, None);
    assert!(
        !strings_asked(&rig).contains(&5),
        "{:?}",
        strings_asked(&rig)
    );

    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A)
            .with_board_id(),
    );
    let s = status_of(&rig);
    let report = s.devices[0].control_channel.as_ref().unwrap();
    assert_eq!(report.protocol, "1.3");
    assert_eq!(report.board_id.as_deref(), Some(PAD_A_ID));
    assert!(strings_asked(&rig).contains(&5));
}

#[test]
fn a_1_3_pad_is_kept_by_its_board_id() {
    let mut rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A),
    );
    info_read(&rig, false).unwrap();
    assert!(files(&rig.cache_dir()).contains(&"Haute42COSMOX-0C0FFEE.json".to_string()));

    // The same pad with firmware 1.3: its cache goes by the board ID, and
    // replaces the one by serial.
    rig.clock.advance(Duration::from_secs(60));
    rig.replug(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .unique_id(PAD_A)
            .with_board_id(),
    );
    info_read(&rig, false).unwrap();
    assert_eq!(
        files(&rig.cache_dir()),
        [
            format!("Haute42COSMOX-{PAD_A_ID}.json"),
            cache::CHANNELS_FILE.to_string(),
            cache::SESSIONS_FILE.to_string(),
        ]
    );
    let c = cache_file(&rig, &format!("Haute42COSMOX-{PAD_A_ID}.json"));
    let pad = c.pad.unwrap();
    assert_eq!(pad.board_id.as_deref(), Some(PAD_A_ID));
    assert_eq!(pad.serial.as_deref(), Some("0C0FFEE"));

    // On stock firmware, the serial still finds it: the firmware makes the
    // serial from the board ID.
    rig.replug(SimPad::new(s16_dir(), InputMode::Xinput).unique_id(PAD_A));
    let msg = guide_text(&rig);
    assert!(msg.contains("In profile 2 (SilkSong)"), "{msg}");
}

#[test]
fn board_ids_and_serials_name_the_same_pad() {
    // The S16: flash chip ID `C008856.` in ASCII, XInput serial `035362E`.
    let s16 = cache::PadId::board("433030383835362E");
    assert_eq!(s16.serials_in(InputMode::Xinput), ["035362E"]);
    let mut d = sim::device(InputMode::Xinput, 7);
    d.serial = Some("035362E".into());
    let by_serial = cache::PadId::of(&d).unwrap();
    assert_eq!(s16.same(&by_serial), Some(true));
    assert_eq!(by_serial.same(&s16), Some(true));

    let xbone = sim::device_with_id(InputMode::Xbone, 7, Some(PAD_A));
    let xbone = cache::PadId::of(&xbone).unwrap();
    assert_eq!(cache::PadId::board(PAD_A_ID).same(&xbone), Some(true));
    assert_eq!(s16.same(&xbone), Some(false));

    assert_eq!(
        s16.same(&cache::PadId::board(PAD_A_ID)),
        Some(false),
        "another board ID"
    );
    let a = cache::PadId::of(&sim::device_with_id(InputMode::Xinput, 7, Some(PAD_A))).unwrap();
    assert_eq!(a.same(&xbone), None, "serials of two modes can't compare");
    assert!(
        cache::PadId::board("not hex")
            .serials_in(InputMode::Xinput)
            .is_empty()
    );
    assert!(s16.serials_in(InputMode::Keyboard).is_empty());
    // Xbox One: v0.7.12 and main make the letters in two ways.
    assert_eq!(
        cache::PadId::board(PAD_A_ID).serials_in(InputMode::Xbone),
        ["RJBSKRFN8ABCDEFGH", "RIZQHKVE8ABCDEFGH"]
    );
}

/// Some devices on USB.
struct Many(Vec<gamepadii_core::discovery::Device>);

impl gamepadii_core::discovery::Scan for Many {
    fn scan(&self) -> Vec<gamepadii_core::discovery::Device> {
        self.0.clone()
    }
}

#[test]
fn two_pads_with_the_same_board_id_go_by_their_port() {
    use gamepadii_core::channel::sim::{SimChannel, Standalone};
    use gamepadii_core::channel::{ControlTransport, node_path};
    let pad = |port: &str, devnum| {
        let mut d = sim::device(InputMode::Xinput, devnum);
        d.port = port.into();
        d
    };
    let scan = Many(vec![pad("3-1", 7), pad("3-2", 8), pad("3-3", 9)]);
    let open = |d: &gamepadii_core::discovery::Device| {
        // The pad on 3-3 has its own ID.
        let id = if d.port == "3-3" {
            PAD_A_ID
        } else {
            "433030383835362E"
        };
        let sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_board_id(id);
        Ok(Box::new(Standalone::new(sim, node_path(d).unwrap())) as Box<dyn ControlTransport>)
    };
    let dir = tempfile::tempdir().unwrap();
    let s = gamepadii_core::commands::status_with(&scan, Some(&open), Some(dir.path())).unwrap();
    let notes: Vec<_> = s
        .devices
        .iter()
        .map(|d| d.channel_note.as_deref())
        .collect();
    let shared = Some(gamepadii_core::commands::SHARED_BOARD_ID);
    assert_eq!(notes, [shared, shared, None]);
    let ports: Vec<_> = s
        .devices
        .iter()
        .map(|d| d.pad.as_ref().unwrap().port.as_deref())
        .collect();
    assert_eq!(ports, [Some("3-1"), Some("3-2"), None]);
}

// ---- the input snapshot (control channel v1.7) --------------------------------

/// The S16 (profile 2 active) on a v1.7 build with the slots, with the
/// inputs from `f`. GPIO 25 has a function, as on the S16.
fn input_rig(f: impl FnOnce(&mut gamepadii_core::channel::sim::SimChannel)) -> Rig {
    Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                *c = c.clone().with_slots().with_input();
                c.input.functions = 1 << 25;
                f(c);
            }),
    )
}

fn find(rig: &Rig, scan: bool, log: &mut Log) -> gamepadii_core::Result<FoundPin> {
    let find = flow::FindPin {
        scan,
        ..flow::FindPin::default()
    };
    rig.run(|env| flow::find_pin(env, &find, log))
}

#[test]
fn input_names_each_pressed_pin_by_its_action_and_key() {
    let rig = input_rig(|c| c.input.frames = [1 << 6 | 1 << 14 | 1 << 27].into());
    let r = rig.run(flow::input).unwrap();
    assert_eq!((r.profile, r.names_from), (Some(2), NamesFrom::Slot));
    let pins: Vec<_> = r
        .pressed
        .iter()
        .map(|p| (p.gpio, p.action_name.clone(), p.key))
        .collect();
    assert_eq!(
        pins,
        [
            // SilkSong has L2 on the key that Default calls B1.
            (6, Some("L2".into()), Some(Button::B1)),
            (14, Some("menu-navigation-toggle".into()), None),
            (27, Some("Up".into()), Some(Button::Up)),
        ]
    );
    assert_eq!((r.dpad, r.buttons), (vec!["Up"], vec!["L2"]));
    assert!(r.sticks.at_rest() && !r.scan_on);
    assert!(rig.channel_commands().is_empty(), "reads only");
    assert!(rig.requests().is_empty());
}

#[test]
fn input_without_slots_uses_the_cache_or_only_numbers() {
    let pad = || {
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| {
                *c = c.clone().with_input();
                c.input.frames = [1 << 6].into();
            })
    };
    let rig = Rig::new(pad()).with_cache();
    let r = rig.run(flow::input).unwrap();
    assert_eq!((r.profile, r.names_from), (Some(2), NamesFrom::Cache));
    assert_eq!(r.pressed[0].action_name.as_deref(), Some("L2"));
    assert_eq!(r.pressed[0].key, Some(Button::B1));
    let rig = Rig::new(pad());
    let r = rig.run(flow::input).unwrap();
    assert_eq!(r.names_from, NamesFrom::None);
    assert_eq!((r.pressed[0].gpio, r.pressed[0].action), (6, None));
}

#[test]
fn find_pin_accepts_a_press_after_the_debounce() {
    let rig = input_rig(|c| c.input.frames = SimInput::press(1 << 27, 5, 6));
    let mut log = Log::default();
    let r = find(&rig, false, &mut log).unwrap();
    assert_eq!((r.pin.gpio, r.pin.key), (27, Some(Button::Up)));
    assert_eq!(r.pin.action_name.as_deref(), Some("Up"));
    assert_eq!(
        (r.no_action, r.scan, r.released),
        (Some(false), false, true)
    );
    assert_eq!(log.other, ["push a button"]);
    assert_eq!(log.waits.first().map(|w| w.0), Some(Awaiting::Press));
    assert!(log.waits.iter().any(|w| w.0 == Awaiting::Release));
    // Without --scan, no PIN_SCAN at all.
    assert!(rig.channel_commands().is_empty());
    // A read every 15 ms: the press, three reads, then the release.
    assert_eq!(rig.pad.firmware().input.reads, 5 + 3 + 3 + 3);
}

#[test]
fn find_pin_ignores_a_bounce_and_a_held_button() {
    let (b3, b15, b27) = (1u64 << 3, 1u64 << 15, 1u64 << 27);
    let rig = input_rig(|c| {
        // GPIO 27 is held at the start; GPIO 3 bounces for one read.
        c.input.frames = [b27, b27, b27 | b3, b27, b27, b27, b27 | b15, b15]
            .into_iter()
            .chain([b3; 4])
            .chain([0])
            .collect();
    });
    let mut log = Log::default();
    let r = find(&rig, false, &mut log).unwrap();
    // GPIO 15 has no action: without the scan, the pad can't read it.
    assert_eq!(r.pin.gpio, 3);
    assert_eq!(r.pin.action_name.as_deref(), Some("Down"));
}

#[test]
fn find_pin_waits_for_the_release() {
    let rig = input_rig(|c| c.input.frames = SimInput::press(1 << 5, 2, 100));
    let r = find(&rig, false, &mut Log::default()).unwrap();
    assert_eq!((r.pin.gpio, r.released), (5, true));
    assert!(rig.clock.elapsed() >= Duration::from_millis(15 * 100));
    // Never released: found, but not released after 5 s.
    let rig = input_rig(|c| c.input.frames = [0, 1 << 5].into());
    let r = find(&rig, false, &mut Log::default()).unwrap();
    assert_eq!((r.pin.gpio, r.released), (5, false));
    assert!(r.note.is_none());
}

#[test]
fn find_pin_with_the_scan_finds_a_pin_with_no_action() {
    let rig = input_rig(|c| c.input.frames = SimInput::press(1 << 22, 3, 4));
    let mut log = Log::default();
    let r = find(&rig, true, &mut log).unwrap();
    assert_eq!(r.pin.gpio, 22);
    assert_eq!((r.no_action, r.scan), (Some(true), true));
    assert_eq!(r.pin.action_name.as_deref(), Some("none"));
    assert_eq!(rig.channel_commands(), [0x19, 0x19], "on, then off");
    let last = rig.pad.control_log().pop().unwrap();
    assert_eq!(last.data[8..], [0]);
    assert!(!rig.pad.firmware().input.scan_on);
}

#[test]
fn find_pin_times_out_with_exit_5_and_stops_the_scan() {
    let rig = input_rig(|_| {});
    let mut log = Log::default();
    let e = find(&rig, true, &mut log).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("no button press in 15 s"), "{e}");
    assert_eq!(rig.clock.elapsed(), Duration::from_secs(15));
    assert_eq!(rig.channel_commands(), [0x19, 0x19]);
    assert!(!rig.pad.firmware().input.scan_on);
    // The spinner moves every 200 ms, not at each read.
    assert_eq!(log.waits.len(), 75);
    // Without the scan: no PIN_SCAN on a timeout either.
    let rig = input_rig(|_| {});
    let e = find(&rig, false, &mut Log::default()).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(rig.channel_commands().is_empty());
}

#[test]
fn input_needs_control_channel_1_7() {
    let rig = Rig::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| *c = c.clone().with_slots()),
    );
    let e = rig.run(flow::input).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("control channel v1.7"), "{e}");
    assert!(e.to_string().contains("speaks v1.4"), "{e}");
    let mut log = Log::default();
    let e = find(&rig, true, &mut log).unwrap_err();
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(rig.channel_commands().is_empty());
    assert!(log.other.is_empty(), "no prompt");
}
