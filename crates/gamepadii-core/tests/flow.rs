//! The guided flows (`mode`, `webconfig`, `reboot` with a wait) against a
//! simulated pad (`gamepadii_core::sim`) over the S16 capture, with a
//! manual clock: every branch, no real sleeps.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gamepadii_core::api::{PinMappings, ProfileOptions, RebootAck};
use gamepadii_core::buttons::{self, Button, Combo, Profiles};
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::commands::Firmware;
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Awaiting, Env, Progress, Step, Timeouts, Via};
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::sim::{self, SimPad, UserAction};
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
    cache::record_profile(&rig.cache_dir(), "Haute42COSMOX", 1, false, now).unwrap();
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
fn the_trip_drops_a_ram_only_profile_and_says_so() {
    let rig = Rig::cc(InputMode::Xinput).with_cache();
    switch(&rig, "3", false).unwrap();
    let (_, trip) = info_trip(&rig, false, &mut Log::default()).unwrap();
    let note = trip.unwrap().note.unwrap();
    assert!(
        note.contains(
            "it ran profile 3 before, not saved; the restart put it back on its saved profile 2"
        ),
        "{note}"
    );
    assert_eq!(rig.pad.firmware().active, 2);
    assert_eq!(cached(&rig).active_profile(), Some(2));
}
