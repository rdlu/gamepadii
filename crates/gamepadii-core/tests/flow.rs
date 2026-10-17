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
        let env = Env {
            scan: &*self.pad,
            clock: &self.clock,
            open: &open,
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
