//! The control channel's host client (`gamepadii_core::channel`) against
//! the simulated firmware: detection (including the stock firmware that
//! answers instead of STALLing), every command, and every refusal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gamepadii_core::api::RebootAck;
use gamepadii_core::channel::sim::{SimChannel, Standalone};
use gamepadii_core::channel::{
    self, Caps, Channel, ControlTransport, Outcome, Probe, Setup, UsbError, proto,
};
use gamepadii_core::error::exit;
use gamepadii_core::lights::LightsFile;
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::pb;
use gamepadii_core::sim::SimPad;

fn s16_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
}

/// The S16 with our firmware, in `mode`.
fn s16(mode: InputMode) -> Arc<SimPad> {
    Arc::new(SimPad::new(s16_dir(), mode).with_channel())
}

fn open(pad: &Arc<SimPad>) -> Box<dyn ControlTransport> {
    let d = gamepadii_core::discovery::Scan::scan(&**pad).remove(0);
    pad.open_control(&d).unwrap()
}

fn connect(pad: &Arc<SimPad>) -> Channel {
    match channel::hello(open(pad), 0x1234) {
        Probe::Found(c) => c,
        other => panic!("{other:?}"),
    }
}

fn standalone(sim: SimChannel) -> Box<dyn ControlTransport> {
    Box::new(Standalone::new(sim, "/dev/bus/usb/003/007"))
}

// ---- detection --------------------------------------------------------------

#[test]
fn hello_finds_our_firmware_and_its_capabilities() {
    let ch = connect(&s16(InputMode::Xinput));
    let h = ch.hello();
    assert_eq!((h.major, h.minor), (1, 0));
    assert_eq!(
        h.caps.names(),
        [
            "status",
            "string",
            "set_profile",
            "reboot",
            "set_input_mode"
        ]
    );
    assert!(!h.caps.has(Caps::REBOOT_BOOTSEL), "no BOOTSEL (issue #2)");
    assert!(h.boots(InputMode::Ps5) && !h.boots(InputMode::Sinput));
    assert_eq!(ch.node(), "/dev/bus/usb/003/007");
}

#[test]
fn stock_xinput_answers_with_zero_bytes_and_is_not_the_channel() {
    let mut sim = SimChannel::stock(Some(InputMode::Xinput));
    let reply = sim
        .control_in(Setup::read(proto::Command::Hello, 7), 64)
        .unwrap();
    assert!(reply.is_empty(), "XInput ACKs with a 0-byte data stage");
    let p = channel::hello(standalone(sim), 7);
    assert!(
        matches!(p, Probe::Stock(ref r) if r.contains("Short(0)")),
        "{p:?}"
    );
}

#[test]
fn stock_xbox_one_answers_with_garbage_and_is_not_the_channel() {
    let sim = SimChannel::stock(Some(InputMode::Xbone));
    let p = channel::hello(standalone(sim), 7);
    assert!(
        matches!(p, Probe::Stock(ref r) if r.contains("Magic")),
        "{p:?}"
    );

    // Even garbage that happens to hold an old HELLO (the magic, a stale
    // nonce) is rejected: only our fresh nonce counts.
    let mut sim = SimChannel::stock(Some(InputMode::Xbone));
    let mut stale = proto::Hello {
        major: 1,
        minor: 0,
        caps: Caps(0xff),
        token: 1,
        modes: 1,
    }
    .encode(0x0bad)
    .to_vec();
    stale.resize(64, 0);
    sim.garbage = stale;
    let p = channel::hello(standalone(sim), 0x1234);
    assert!(
        matches!(p, Probe::Stock(ref r) if r.contains("Nonce")),
        "{p:?}"
    );
}

#[test]
fn stock_modes_that_stall_are_not_the_channel() {
    for m in [
        InputMode::Keyboard,
        InputMode::Switch,
        InputMode::Ps4,
        InputMode::Webconfig,
    ] {
        let p = channel::hello(standalone(SimChannel::stock(Some(m))), 9);
        assert!(
            matches!(p, Probe::Stock(ref r) if r.contains("STALL")),
            "{m:?}: {p:?}"
        );
    }
    // The boot ROM has no firmware at all.
    let pad = Arc::new(SimPad::new(s16_dir(), InputMode::Bootsel).with_channel());
    assert!(matches!(channel::hello(open(&pad), 9), Probe::Stock(_)));
}

#[test]
fn a_newer_major_is_refused() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    let mut reply = sim
        .control_in(Setup::read(proto::Command::Hello, 5), 64)
        .unwrap();
    reply[6] = 2;
    struct Canned(Vec<u8>);
    impl ControlTransport for Canned {
        fn control_in(&self, _: Setup, _: u16, _: Duration) -> Result<Vec<u8>, UsbError> {
            Ok(self.0.clone())
        }
        fn control_out(&self, _: Setup, _: &[u8], _: Duration) -> Result<(), UsbError> {
            Ok(())
        }
        fn describe(&self) -> String {
            "canned".into()
        }
    }
    let p = channel::hello(Box::new(Canned(reply)), 5);
    assert!(matches!(p, Probe::Newer { major: 2, .. }), "{p:?}");
}

#[test]
fn no_permission_is_its_own_outcome() {
    let pad = Arc::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .no_access(),
    );
    let d = gamepadii_core::discovery::Scan::scan(&*pad).remove(0);
    let open = |d: &gamepadii_core::discovery::Device| pad.open_control(d);
    let p = channel::probe(&open, &d);
    assert!(
        matches!(p, Probe::NoAccess { ref node } if node == "/dev/bus/usb/003/007"),
        "{p:?}"
    );
    let e = UsbError::NoAccess.into_error("/dev/bus/usb/003/007", "HELLO");
    assert_eq!(e.exit_code(), exit::NEEDS_USER);
    assert!(e.to_string().contains("gamepadii setup udev"), "{e}");
}

#[test]
fn the_nonce_is_never_zero() {
    for _ in 0..100 {
        assert_ne!(channel::nonce(), 0);
    }
}

// ---- reads ------------------------------------------------------------------

#[test]
fn status_and_strings_from_the_s16() {
    let ch = connect(&s16(InputMode::Xinput));
    let st = ch.status().unwrap();
    assert_eq!(st.input_mode, 0);
    assert_eq!(st.profile, 2);
    assert_eq!(st.enabled(), [1, 2, 3, 4, 5]);
    assert_eq!(st.forced_setup, 0);
    assert!(!st.pending());
    let s = |id| ch.string(id).unwrap();
    assert_eq!(s(proto::string::VERSION).as_deref(), Some("v0.7.12"));
    assert_eq!(s(proto::string::BOARD).as_deref(), Some("Haute42COSMOX"));
    assert_eq!(
        s(proto::string::BOARD_LABEL).as_deref(),
        Some("Haute42 COSMOX")
    );
    assert_eq!(
        s(proto::string::profile_label(2)).as_deref(),
        Some("SilkSong")
    );
    assert_eq!(s(proto::string::profile_label(6)), None, "no sixth profile");
    assert_eq!(s(0x99), None, "an unknown id STALLs");
}

#[test]
fn the_channel_answers_in_web_config_too() {
    let ch = connect(&s16(InputMode::Webconfig));
    assert_eq!(ch.status().unwrap().input_mode, 255);
}

#[test]
fn foreign_requests_take_the_stock_path() {
    // With the channel built in, xpad's probe and a wrong magic still get
    // exactly what stock XInput gives them: 0 bytes.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    let xpad = Setup {
        request_type: 0xc1,
        request: 0x01,
        value: 0x0100,
        index: 0,
    };
    assert_eq!(sim.control_in(xpad, 20).unwrap(), Vec::<u8>::new());
    let wrong_magic = Setup {
        index: 0x4f01,
        ..Setup::read(proto::Command::Hello, 1)
    };
    assert_eq!(sim.control_in(wrong_magic, 64).unwrap(), Vec::<u8>::new());
    // A command sent the wrong way round is not ours either.
    let backwards = Setup {
        request_type: proto::TYPE_OUT,
        ..Setup::read(proto::Command::Status, 0)
    };
    assert!(sim.control_out(backwards, &[]).unwrap().is_none());
    // In keyboard mode the stock path STALLs.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Keyboard));
    assert!(matches!(sim.control_in(xpad, 20), Err(UsbError::Stall)));
    // An unknown command with our magic STALLs, as does wLength > 64.
    let unknown = Setup {
        index: 0x5077,
        ..Setup::read(proto::Command::Hello, 1)
    };
    assert!(matches!(sim.control_in(unknown, 64), Err(UsbError::Stall)));
    assert!(matches!(
        sim.control_in(Setup::read(proto::Command::Status, 0), 65),
        Err(UsbError::Stall)
    ));
}

// ---- commands -----------------------------------------------------------------

#[test]
fn set_profile_in_ram_and_persisted() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    ch.set_profile(3, false).unwrap();
    // Pending until the main loop's next pass (the next STATUS here).
    assert!(ch.status().unwrap().pending());
    let st = ch.status().unwrap();
    assert_eq!((st.profile, st.pending()), (3, false));
    let saved =
        |pad: &SimPad| pad.mock().current("getGamepadOptions").unwrap()["profileNumber"].clone();
    assert_eq!(saved(&pad), 2, "RAM only: flash keeps profile 2");
    ch.set_profile(4, true).unwrap();
    assert_eq!(saved(&pad), 4, "persisted");
    // The OUT payload is header + profile + persist.
    let last = pad
        .control_log()
        .into_iter()
        .rfind(|s| !s.setup.is_in())
        .unwrap();
    assert_eq!(last.setup.index, 0x5010);
    assert_eq!(last.data.len(), 10);
    assert_eq!(&last.data[8..], [4, 1]);
    assert_eq!(proto::header(&last.data).unwrap().0, pad.firmware().token);
}

#[test]
fn a_disabled_profile_is_refused_with_the_reason() {
    let ch = connect(&s16(InputMode::Xinput));
    let e = ch.set_profile(6, false).unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(e.to_string().contains("that profile is disabled"), "{e}");
    let st = ch.status().unwrap();
    assert_eq!(Outcome::from_byte(st.last_result), Outcome::ProfileDisabled);
    assert_eq!(st.last_command, 0x10);
    assert!(ch.set_profile(0, false).is_err(), "checked on the host");
}

#[test]
fn a_bad_token_or_length_stalls_and_says_why() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    let t = open(&pad);
    let token = ch.hello().token;
    let cmd = Setup::command(proto::Command::SetProfile);
    let r = t.control_out(
        cmd,
        &proto::payload(token ^ 1, 77, &[3, 0]),
        channel::TIMEOUT,
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    let st = ch.status().unwrap();
    assert_eq!(
        (st.last_seq, Outcome::from_byte(st.last_result)),
        (77, Outcome::BadToken)
    );
    let r = t.control_out(cmd, &proto::payload(token, 78, &[3]), channel::TIMEOUT);
    assert!(matches!(r, Err(UsbError::Stall)));
    let st = ch.status().unwrap();
    assert_eq!(
        (st.last_seq, Outcome::from_byte(st.last_result)),
        (78, Outcome::BadLength)
    );
    assert_eq!(st.profile, 2, "nothing applied");
}

#[test]
fn reboot_into_web_config_re_enumerates() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    assert_eq!(
        ch.reboot(BootMode::Webconfig).unwrap(),
        RebootAck::Confirmed
    );
    assert_eq!(pad.mode(), None, "off USB");
    // The old handle is stale.
    assert!(ch.status().is_err());
    for _ in 0..gamepadii_core::sim::OFF_USB_SCANS {
        gamepadii_core::discovery::Scan::scan(&*pad);
    }
    assert_eq!(pad.mode(), Some(InputMode::Webconfig));
    let again = connect(&pad);
    assert_ne!(
        again.hello().token,
        ch.hello().token,
        "a new token per boot"
    );
}

#[test]
fn bootsel_is_not_offered() {
    let ch = connect(&s16(InputMode::Xinput));
    let e = ch.reboot(BootMode::Bootsel).unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(e.to_string().contains("no REBOOT"), "{e}");
}

#[test]
fn forced_setup_locks_are_honoured() {
    let pad = Arc::new(
        SimPad::new(s16_dir(), InputMode::Xinput)
            .with_channel()
            .channel(|c| c.forced_setup = 3),
    );
    let ch = connect(&pad);
    let e = ch.reboot(BootMode::Webconfig).unwrap_err();
    assert!(
        e.to_string()
            .contains("locks web-config and mode switching"),
        "{e}"
    );
    let e = ch.set_input_mode(InputMode::Keyboard).unwrap_err();
    assert!(e.to_string().contains("forced setup mode"), "{e}");
    assert_eq!(pad.mode(), Some(InputMode::Xinput), "nothing happened");
    // A reboot into gamepad mode is still fine.
    ch.reboot(BootMode::Gamepad).unwrap();
}

#[test]
fn set_input_mode_saves_and_reboots() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    let e = ch.set_input_mode(InputMode::Sinput).unwrap_err();
    assert!(e.to_string().contains("not in this firmware build"), "{e}");
    assert!(ch.set_input_mode(InputMode::Webconfig).is_err());
    assert_eq!(
        ch.set_input_mode(InputMode::Keyboard).unwrap(),
        RebootAck::Confirmed
    );
    assert_eq!(
        pad.mock().current("getGamepadOptions").unwrap()["inputMode"],
        3
    );
    for _ in 0..gamepadii_core::sim::OFF_USB_SCANS {
        gamepadii_core::discovery::Scan::scan(&*pad);
    }
    assert_eq!(pad.mode(), Some(InputMode::Keyboard));
}

#[test]
fn a_ram_profile_is_lost_on_reboot() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    ch.set_profile(5, false).unwrap();
    ch.reboot(BootMode::Gamepad).unwrap();
    for _ in 0..gamepadii_core::sim::OFF_USB_SCANS {
        gamepadii_core::discovery::Scan::scan(&*pad);
    }
    assert_eq!(connect(&pad).status().unwrap().profile, 2);
}

#[test]
fn commands_the_build_lacks_are_usage_errors() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    sim.caps = Caps(Caps::STATUS);
    let ch = channel::hello(standalone(sim), 3).found().unwrap();
    let e = ch.set_profile(1, false).unwrap_err();
    assert!(e.to_string().contains("no SET_PROFILE"), "{e}");
}

// ---- behaviour pinned down during the firmware work (§3.6) -------------------

#[test]
fn reboot_to_bootsel_on_the_wire_is_unsupported() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::Reboot),
        &proto::payload(token, 1, &[2]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        Outcome::from_byte(sim.status().last_result),
        Outcome::Unsupported
    );
    let r = sim.control_out(
        Setup::command(proto::Command::Reboot),
        &proto::payload(token, 2, &[3]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        Outcome::from_byte(sim.status().last_result),
        Outcome::BadArg
    );
}

#[test]
fn after_an_accepted_reboot_everything_is_busy() {
    // Standalone ignores the reboot, so the pad stays to answer.
    let sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    let ch = channel::hello(standalone(sim), 11).found().unwrap();
    ch.reboot(BootMode::Gamepad).unwrap();
    assert!(ch.status().unwrap().reboot_pending());
    let e = ch.set_profile(1, false).unwrap_err();
    assert_eq!(e.exit_code(), exit::TRANSPORT);
    assert!(e.to_string().contains("about to restart"), "{e}");
}

#[test]
fn setup_stage_rejections_and_trailing_bytes() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    sim.enabled_profiles = 0b111;
    let token = sim.token;
    // Shorter than the header: rejected in SETUP, seq 0.
    let r = sim.control_out(Setup::command(proto::Command::SetProfile), &[1, 2, 3]);
    assert!(matches!(r, Err(UsbError::Stall)));
    let st = sim.status();
    assert_eq!((st.last_seq, st.last_command), (0, 0x10));
    assert_eq!(Outcome::from_byte(st.last_result), Outcome::BadLength);
    // An unknown OUT command with our magic: UNSUPPORTED.
    let v2 = Setup {
        index: 0x5020,
        ..Setup::command(proto::Command::SetProfile)
    };
    assert!(
        sim.control_out(v2, &proto::payload(token, 5, &[0, 0]))
            .is_err()
    );
    let st = sim.status();
    assert_eq!((st.last_seq, st.last_command), (0, 0x20));
    assert_eq!(Outcome::from_byte(st.last_result), Outcome::Unsupported);
    // Bytes past the arguments are ignored (room for later minors).
    sim.control_out(
        Setup::command(proto::Command::SetProfile),
        &proto::payload(token, 6, &[3, 0, 0xff, 0xff]),
    )
    .unwrap();
    assert_eq!(sim.status().profile, 3);
}

// ---- v1.1: the LED commands (§3.7) --------------------------------------------

fn fork_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-fork")
}

/// The S16 on a v1.1 build, in `mode`.
fn s16_leds(mode: InputMode) -> Arc<SimPad> {
    Arc::new(SimPad::new(s16_dir(), mode).with_leds())
}

fn replug(pad: &Arc<SimPad>) {
    for _ in 0..gamepadii_core::sim::OFF_USB_SCANS {
        gamepadii_core::discovery::Scan::scan(&**pad);
    }
}

#[test]
fn a_v1_1_build_has_the_led_commands() {
    let ch = connect(&s16_leds(InputMode::Xinput));
    assert_eq!((ch.hello().major, ch.hello().minor), (1, 1));
    assert!(ch.caps().has(Caps::LEDS));
    assert_eq!(ch.caps().names().last(), Some(&"leds"));
    let l = ch.led_status().unwrap();
    assert!(l.running() && l.custom_theme() && !l.unsaved());
    assert_eq!(
        (
            l.animation,
            l.brightness,
            l.static_color,
            l.button_color,
            l.theme
        ),
        (4, 1, 2, 1, 0)
    );
    assert_eq!(
        (
            l.animations,
            l.brightness_steps,
            l.brightness_maximum,
            l.colors,
            l.themes
        ),
        (5, 5, 100, 16, 20)
    );
}

#[test]
fn a_v1_0_build_has_no_led_commands() {
    let pad = s16(InputMode::Xinput);
    let ch = connect(&pad);
    assert_eq!(ch.hello().minor, 0);
    let e = ch.led_status().unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(e.to_string().contains("no LED_STATUS"), "{e}");
    let e = ch.set_led(&channel::SetLed::default()).unwrap_err();
    assert!(e.to_string().contains("no SET_LED"), "{e}");
    // On the wire: LED_STATUS STALLs, SET_LED is an unknown command.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    let r = sim.control_in(Setup::read(proto::Command::LedStatus, 0), 64);
    assert!(matches!(r, Err(UsbError::Stall)));
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::SetLed),
        &proto::payload(token, 9, &[2, 0, 3, 0, 0, 0, 0, 0]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    let st = sim.status();
    assert_eq!((st.last_seq, st.last_command), (0, 0x13));
    assert_eq!(Outcome::from_byte(st.last_result), Outcome::Unsupported);
}

#[test]
fn a_ram_led_change_is_lost_on_reboot() {
    let pad = s16_leds(InputMode::Xinput);
    let ch = connect(&pad);
    ch.set_led(&channel::SetLed {
        animation: Some(1),
        brightness: Some(4),
        ..Default::default()
    })
    .unwrap();
    let l = ch.led_status().unwrap();
    assert_eq!((l.animation, l.brightness, l.theme), (1, 4, 0));
    assert!(l.unsaved());
    assert_eq!(pad.firmware().saved_leds.brightness, 1);
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let l = connect(&pad).led_status().unwrap();
    assert_eq!((l.animation, l.brightness), (4, 1));
    assert!(!l.unsaved());
}

#[test]
fn a_persisted_led_change_survives_a_reboot() {
    let pad = s16_leds(InputMode::Xinput);
    let ch = connect(&pad);
    ch.set_led(&channel::SetLed {
        static_color: Some(10),
        persist: true,
        ..Default::default()
    })
    .unwrap();
    assert!(!ch.led_status().unwrap().unsaved());
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    assert_eq!(connect(&pad).led_status().unwrap().static_color, 10);
}

#[test]
fn any_save_keeps_a_ram_led_change() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_leds();
    sim.enabled_profiles = 0b11;
    let ch = channel::hello(standalone(sim), 5).found().unwrap();
    ch.set_led(&channel::SetLed {
        brightness: Some(0),
        ..Default::default()
    })
    .unwrap();
    assert!(ch.led_status().unwrap().unsaved());
    ch.set_profile(2, true).unwrap();
    assert!(!ch.led_status().unwrap().unsaved());
}

#[test]
fn an_led_value_out_of_range_changes_nothing() {
    let ch = connect(&s16_leds(InputMode::Xinput));
    for set in [
        channel::SetLed {
            animation: Some(0),
            brightness: Some(6),
            ..Default::default()
        },
        channel::SetLed {
            animation: Some(5),
            ..Default::default()
        },
        channel::SetLed {
            button_color: Some(16),
            ..Default::default()
        },
        channel::SetLed {
            theme: Some(20),
            ..Default::default()
        },
    ] {
        let e = ch.set_led(&set).unwrap_err();
        assert_eq!(e.exit_code(), exit::USAGE, "{set:?}");
        assert!(e.to_string().contains("bad argument"), "{e}");
    }
    let l = ch.led_status().unwrap();
    assert_eq!((l.animation, l.brightness), (4, 1), "all or nothing");
    // The top of the range is fine: brightness goes up to the steps.
    ch.set_led(&channel::SetLed {
        brightness: Some(5),
        ..Default::default()
    })
    .unwrap();
    // Unknown mask bits on the wire: BAD_ARG.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_leds();
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::SetLed),
        &proto::payload(token, 3, &[0x40, 0, 0, 0, 0, 0, 0, 0]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        Outcome::from_byte(sim.status().last_result),
        Outcome::BadArg
    );
    // Short arguments: BAD_LENGTH.
    let r = sim.control_out(
        Setup::command(proto::Command::SetLed),
        &proto::payload(token, 4, &[0x02, 0, 1]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        Outcome::from_byte(sim.status().last_result),
        Outcome::BadLength
    );
}

#[test]
fn an_led_change_is_busy_while_a_reboot_is_pending() {
    let sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_leds();
    let ch = channel::hello(standalone(sim), 12).found().unwrap();
    ch.reboot(BootMode::Webconfig).unwrap();
    let e = ch
        .set_led(&channel::SetLed {
            brightness: Some(2),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(e.exit_code(), exit::TRANSPORT);
    assert!(e.to_string().contains("about to restart"), "{e}");
    // LED_STATUS still answers.
    assert_eq!(ch.led_status().unwrap().brightness, 1);
}

#[test]
fn a_saved_led_change_reaches_the_fork_config() {
    let pad = Arc::new(SimPad::new(fork_dir(), InputMode::Xinput).with_leds());
    let ch = connect(&pad);
    // From the fixture's animationOptions.
    let l = ch.led_status().unwrap();
    assert_eq!((l.animation, l.brightness), (2, 3));
    ch.set_led(&channel::SetLed {
        theme: Some(7),
        persist: true,
        ..Default::default()
    })
    .unwrap();
    let config = pad.mock().current("getConfig").unwrap();
    assert_eq!(config["animationOptions"]["themeIndex"], 7);
    assert_eq!(config["animationOptions"]["brightness"], 3);
}

#[test]
fn the_effect_count_follows_the_custom_theme() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_leds();
    sim.led_limits.custom_theme = false;
    let ch = channel::hello(standalone(sim), 7).found().unwrap();
    let l = ch.led_status().unwrap();
    assert_eq!(l.animations, 4);
    assert!(l.running() && !l.custom_theme());
    assert_eq!(
        ch.string(proto::string::effect_name(0)).unwrap().as_deref(),
        Some("EFFECT_STATIC_COLOR")
    );
    // n at or past the count STALLs: no name.
    assert_eq!(ch.string(proto::string::effect_name(4)).unwrap(), None);
    let e = ch
        .set_led(&channel::SetLed {
            animation: Some(4),
            ..Default::default()
        })
        .unwrap_err();
    assert!(e.to_string().contains("bad argument"), "{e}");
    // LEDs not set up: no capability.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    sim.led_limits.running = false;
    assert!(!sim.with_leds().caps.has(Caps::LEDS));
}

#[test]
fn a_new_node_gets_a_moment_for_its_acl() {
    use gamepadii_core::wait::{Clock, ManualClock};
    let clock = ManualClock::at(0);
    // The ACL comes on the third try.
    let tries = std::cell::Cell::new(0);
    let r = channel::wait_for_acl(&clock, || {
        tries.set(tries.get() + 1);
        if tries.get() < 3 {
            Err(UsbError::NoAccess)
        } else {
            Ok(())
        }
    });
    assert!(r.is_ok());
    assert_eq!(tries.get(), 3);
    assert_eq!(clock.elapsed(), Duration::from_millis(200));

    // It never comes: gamepadii stops after a second.
    let start = clock.elapsed();
    let r = channel::wait_for_acl(&clock, || Err::<(), _>(UsbError::NoAccess));
    assert!(matches!(r, Err(UsbError::NoAccess)));
    assert_eq!(clock.elapsed() - start, channel::ACL_WAIT);

    // Any other error ends the wait at once.
    let start = clock.elapsed();
    let r = channel::wait_for_acl(&clock, || Err::<(), _>(UsbError::Gone));
    assert!(matches!(r, Err(UsbError::Gone)));
    assert_eq!(clock.elapsed(), start);
}

// ---- v1.2: the lights commands on main (§3.8) ---------------------------------

fn main_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-main")
}

/// The S16 on a main-line build, in `mode`.
fn s16_lights(mode: InputMode) -> Arc<SimPad> {
    Arc::new(SimPad::new(main_dir(), mode).with_lights())
}

#[test]
fn a_main_build_has_the_lights_commands_and_not_the_led_ones() {
    let ch = connect(&s16_lights(InputMode::Xinput));
    assert_eq!((ch.hello().major, ch.hello().minor), (1, 2));
    assert!(ch.caps().has(Caps::LIGHTS) && !ch.caps().has(Caps::LEDS));
    assert_eq!(ch.caps().0, 0x22f);
    assert!(ch.hello().boots(InputMode::Switch2Pro));
    let l = ch.lights_status().unwrap();
    assert!(l.running() && !l.unsaved() && !l.preview());
    assert_eq!((l.profile, l.enabled_profiles, l.profiles), (0, 0b1111, 4));
    assert_eq!(
        (l.brightness, l.brightness_steps, l.brightness_maximum),
        (5, 10, 128)
    );
    // Profile 0 of the fixture: rainbow synced, idle and case.
    assert_eq!((l.idle_effect, l.pressed_effect, l.case_effect), (1, 0, 1));
    assert_eq!(l.idle_cycle_time, 0);
    let e = ch.led_status().unwrap_err();
    assert!(e.to_string().contains("no LED_STATUS"), "{e}");
    // On the wire, main doesn't know the v1.1 commands.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights();
    let r = sim.control_in(Setup::read(proto::Command::LedStatus, 0), 64);
    assert!(matches!(r, Err(UsbError::Stall)));
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::SetLed),
        &proto::payload(token, 9, &[2, 0, 3, 0, 0, 0, 0, 0]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(sim.status().last_seq, 0, "rejected in SETUP");
}

#[test]
fn a_v1_1_build_has_no_lights_commands() {
    let ch = connect(&s16_leds(InputMode::Xinput));
    let e = ch.lights_status().unwrap_err();
    assert!(e.to_string().contains("no LIGHTS_STATUS"), "{e}");
    let e = ch.set_lights(&channel::SetLights::default()).unwrap_err();
    assert!(e.to_string().contains("no SET_LIGHTS"), "{e}");
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_leds();
    let r = sim.control_in(Setup::read(proto::Command::LightsStatus, 0), 64);
    assert!(matches!(r, Err(UsbError::Stall)));
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::SetLights),
        &proto::payload(token, 9, &[1, 0, 0, 0]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    let st = sim.status();
    assert_eq!((st.last_seq, st.last_command), (0, 0x14));
}

#[test]
fn main_without_leds_refuses_set_lights_after_the_length_check() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    sim.lights_limits.running = false;
    let mut sim = sim.with_lights();
    assert!(!sim.caps.has(Caps::LIGHTS));
    let r = sim.control_in(Setup::read(proto::Command::LightsStatus, 0), 64);
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        sim.status().last_command,
        0,
        "an IN changes no STATUS field"
    );
    let token = sim.token;
    let out = |sim: &mut SimChannel, token, args: &[u8]| {
        let r = sim.control_out(
            Setup::command(proto::Command::SetLights),
            &proto::payload(token, 7, args),
        );
        assert!(matches!(r, Err(UsbError::Stall)));
        Outcome::from_byte(sim.status().last_result)
    };
    assert_eq!(out(&mut sim, token ^ 1, &[1, 0, 0, 0]), Outcome::BadToken);
    assert_eq!(out(&mut sim, token, &[1, 0, 0]), Outcome::BadLength);
    assert_eq!(out(&mut sim, token, &[1, 0, 0, 0]), Outcome::Unsupported);
    assert_eq!(sim.status().last_seq, 7);
}

#[test]
fn set_lights_checks_in_the_spec_order_and_changes_all_or_nothing() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights();
    sim.lights_limits.profiles[2].enabled = false;
    let token = sim.token;
    let mut out = |args: &[u8]| {
        let r = sim.control_out(
            Setup::command(proto::Command::SetLights),
            &proto::payload(token, 3, args),
        );
        let st = sim.status();
        (r.is_ok(), Outcome::from_byte(st.last_result), sim.lights)
    };
    let before = out(&[0, 0, 0, 0]).2;
    // Mask bits 2-7, a persist byte above 1, a disabled or unknown profile,
    // a brightness past the steps: BAD_ARG, and nothing changes.
    for args in [
        [0x04, 0, 0, 0],
        [0x01, 0, 0, 2],
        [0x03, 2, 3, 0],
        [0x01, 4, 0, 0],
        [0x01, 0xfe, 0, 0],
        [0x03, 1, 11, 0],
    ] {
        let (ok, o, now) = out(&args);
        assert!(!ok, "{args:?}");
        assert_eq!(o, Outcome::BadArg, "{args:?}");
        assert_eq!(now, before, "{args:?}");
    }
    let (ok, _, now) = out(&[0x03, 0xff, 10, 0]);
    assert!(ok);
    assert_eq!((now.profile, now.brightness), (-1, 10));
    // Off: the effects read 255.
    let l = sim.lights_status();
    assert_eq!(l.on(), None);
    assert_eq!(l.idle_effect, proto::LightsStatus::NONE);
    assert!(l.unsaved());
    // A web-config preview: BUSY, after the argument checks.
    sim.lights_limits.preview = true;
    let r = sim.control_out(
        Setup::command(proto::Command::SetLights),
        &proto::payload(token, 4, &[0x01, 0, 0, 0]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(Outcome::from_byte(sim.status().last_result), Outcome::Busy);
    assert!(sim.lights_status().preview());
}

#[test]
fn a_ram_lights_change_is_lost_on_reboot_and_a_saved_one_is_not() {
    let pad = s16_lights(InputMode::Xinput);
    let ch = connect(&pad);
    ch.set_lights(&channel::SetLights {
        profile: Some(3),
        ..Default::default()
    })
    .unwrap();
    let l = ch.lights_status().unwrap();
    assert_eq!((l.profile, l.idle_effect, l.pressed_effect), (3, 14, 4));
    assert!(l.unsaved());
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let ch = connect(&pad);
    assert_eq!(ch.lights_status().unwrap().profile, 0);
    ch.set_lights(&channel::SetLights {
        brightness: Some(2),
        persist: true,
        ..Default::default()
    })
    .unwrap();
    assert!(!ch.lights_status().unwrap().unsaved());
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    assert_eq!(connect(&pad).lights_status().unwrap().brightness, 2);
}

// ---- v1.4: the profile slots (§3.10) --------------------------------------------

/// The S16 (v0.7.12 fixtures: profiles 1 to 5) on a v1.4 build, in `mode`.
fn s16_slots(mode: InputMode) -> Arc<SimPad> {
    Arc::new(
        SimPad::new(s16_dir(), mode)
            .with_channel()
            .channel(|c| *c = c.clone().with_slots()),
    )
}

/// Profile `n` of the S16 with GPIO 6 on B4 and the label `label`.
fn changed(ch: &Channel, n: u8, label: &str, flags: u8) -> proto::Slot {
    let mut s = ch.read_slot(2).unwrap();
    s.profile = n;
    s.flags = flags;
    s.label = label.into();
    s.pins[6].action = 8;
    s
}

#[test]
fn a_1_4_build_reads_every_slot() {
    let ch = connect(&s16_slots(InputMode::Xinput));
    assert!(ch.hello().has_slots());
    assert_eq!(ch.caps().names().last(), Some(&"slots"));
    let base = ch.read_slot(1).unwrap();
    assert_eq!((base.profile, base.pins.len()), (1, 30));
    assert!(base.enabled());
    let silk = ch.read_slot(2).unwrap();
    assert_eq!(silk.label, "SilkSong");
    assert!(silk.enabled() && silk.active());
    assert_eq!(silk.pins[2].action, 5);
    // Profile 6 was never written: zeros.
    let empty = ch.read_slot(6).unwrap();
    assert!(empty.label.is_empty() && !empty.enabled());
    assert!(empty.pins.iter().all(|p| p.action == 0));
    let e = ch.read_slot(7).unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    // On the wire, 0 and 7 STALL.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_slots();
    for n in [0, 7] {
        let r = sim.control_in(Setup::read(proto::Command::ReadSlot, n), 512);
        assert!(matches!(r, Err(UsbError::Stall)), "{n}");
    }
}

#[test]
fn a_1_3_build_never_gets_a_slot_command() {
    let ch = connect(&s16(InputMode::Xinput));
    let e = ch.read_slot(2).unwrap_err();
    assert!(e.to_string().contains("no READ_SLOT"), "{e}");
    let e = ch.write_slot(&proto::Slot::default()).unwrap_err();
    assert!(e.to_string().contains("no WRITE_SLOT"), "{e}");
    // Even with bit 10, a 1.3 HELLO is not enough.
    let sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_board_id("433030383835362E")
        .with_slots();
    let mut sim = sim;
    sim.minor = 3;
    let ch = channel::hello(standalone(sim), 5).found().unwrap();
    assert!(ch.read_slot(2).is_err());
}

#[test]
fn write_slot_checks_everything_before_a_change() {
    let pad = s16_slots(InputMode::Xinput);
    let ch = connect(&pad);
    let before = ch.read_slot(3).unwrap();
    let token = pad.firmware().token;
    let t = open(&pad);
    let out = |data: &[u8]| {
        t.control_out(
            Setup::command(proto::Command::WriteSlot),
            data,
            Duration::from_secs(1),
        )
    };
    let send = |slot: &proto::Slot, extra: &[u8]| {
        let mut args = slot.encode();
        args.extend_from_slice(extra);
        let r = out(&proto::payload(token, 11, &args));
        (
            r.is_ok(),
            Outcome::from_byte(ch.status().unwrap().last_result),
        )
    };
    let good = changed(&ch, 3, "Celeste", proto::Slot::ENABLED);
    let with = |f: &dyn Fn(&mut proto::Slot)| {
        let mut s = good.clone();
        f(&mut s);
        s
    };
    for (slot, extra, want) in [
        (with(&|s| s.profile = 1), vec![], Outcome::BadArg),
        (with(&|s| s.profile = 7), vec![], Outcome::BadArg),
        (with(&|s| s.flags = 0x09), vec![], Outcome::BadArg),
        (with(&|s| s.pins[0].action = 139), vec![], Outcome::BadArg),
        (with(&|s| s.pins[0].action = -11), vec![], Outcome::BadArg),
        (
            with(&|s| s.flags = proto::Slot::ACTIVE),
            vec![],
            Outcome::ProfileDisabled,
        ),
        (good.clone(), vec![0], Outcome::BadLength),
    ] {
        let (ok, o) = send(&slot, &extra);
        assert!(!ok);
        assert_eq!(o, want, "{:?}", slot.flags);
    }
    // One byte short: the pin count says 30, the data has less.
    let mut short = good.encode();
    short.pop();
    assert!(out(&proto::payload(token, 12, &short)).is_err());
    let st = ch.status().unwrap();
    assert_eq!(
        (st.last_seq, Outcome::from_byte(st.last_result)),
        (12, Outcome::BadLength)
    );
    // Disabling the active profile 2.
    let (ok, o) = send(&changed(&ch, 2, "SilkSong", 0), &[]);
    assert!(!ok);
    assert_eq!(o, Outcome::ProfileDisabled);
    assert_eq!(ch.read_slot(3).unwrap(), before, "nothing changed");
    // Too long for any chip: refused in SETUP.
    let long = vec![0u8; 8 + 20 + 480 + 1];
    assert!(out(&long).is_err());
    let st = ch.status().unwrap();
    assert_eq!(
        (st.last_seq, Outcome::from_byte(st.last_result)),
        (0, Outcome::BadLength)
    );
}

#[test]
fn a_ram_slot_write_selects_and_goes_away_at_a_restart() {
    let pad = s16_slots(InputMode::Xinput);
    let ch = connect(&pad);
    let base = ch.read_slot(1).unwrap();
    // Profile 6 on a config with 4 extra slots: the firmware adds it.
    let mut slot = changed(&ch, 6, "Hades", proto::Slot::ENABLED | proto::Slot::ACTIVE);
    slot.pins[0].action = 5; // profile 1 gives GPIO 0 to an add-on
    ch.write_slot(&slot).unwrap();
    let st = ch.status().unwrap();
    assert_eq!(st.profile, 6);
    assert_eq!(st.last_command, proto::Command::WriteSlot as u8);
    assert_eq!(st.last_seq, ch.last_seq());
    assert!(st.profile_enabled(6));
    let after = ch.read_slot(6).unwrap();
    assert!(after.active() && after.enabled());
    assert_eq!(after.label, "Hades");
    assert_eq!(after.pins[6].action, 8);
    assert_eq!(
        after.pins[0].action, base.pins[0].action,
        "profile 1's add-on pin"
    );
    assert_eq!(
        ch.string(proto::string::profile_label(6))
            .unwrap()
            .as_deref(),
        Some("Hades")
    );
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let ch = connect(&pad);
    let st = ch.status().unwrap();
    assert_eq!(st.profile, 2);
    assert!(!st.profile_enabled(6));
    assert!(!ch.read_slot(6).unwrap().enabled());
}

#[test]
fn a_saved_slot_write_survives_a_restart() {
    let pad = s16_slots(InputMode::Xinput);
    let ch = connect(&pad);
    let slot = changed(&ch, 4, "DKC 2", proto::Slot::ENABLED | proto::Slot::SAVE);
    ch.write_slot(&slot).unwrap();
    assert_eq!(ch.status().unwrap().profile, 2, "not selected");
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let after = connect(&pad).read_slot(4).unwrap();
    assert_eq!((after.label.as_str(), after.pins[6].action), ("DKC 2", 8));
}

// ---- v1.5: the lights profiles (§3.11) ------------------------------------------

/// The S16 on a v1.5 main-line build, in `mode`.
fn s16_lights_profiles(mode: InputMode) -> Arc<SimPad> {
    Arc::new(
        SimPad::new(main_dir(), mode)
            .with_lights()
            .channel(|c| *c = c.clone().with_lights_profiles()),
    )
}

fn lights_file(ch: &Channel, n: u8) -> LightsFile {
    LightsFile::from_data(&ch.read_lights(n).unwrap(), None).unwrap()
}

#[test]
fn a_1_5_build_reads_every_lights_profile() {
    let ch = connect(&s16_lights_profiles(InputMode::Xinput));
    assert!(ch.hello().has_lights_profiles());
    let d = ch.read_lights(0).unwrap();
    assert!(d.enabled() && d.active());
    let f = LightsFile::from_data(&d, None).unwrap();
    // The fixture's profile 0: rainbow synced, red when idle.
    assert_eq!(f.values[&2], pb::Value::Varint(1));
    assert_eq!(f.values[&5], pb::Value::Bytes(vec![2; 30]));
    assert_eq!(f.values[&4], pb::Value::Varint(0), "web-config's 1 is 0");
    assert!(!ch.read_lights(3).unwrap().active());
    assert_eq!(ch.read_lights(4).unwrap_err().exit_code(), exit::USAGE);
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights_profiles();
    let r = sim.control_in(Setup::read(proto::Command::ReadLights, 4), 512);
    assert!(matches!(r, Err(UsbError::Stall)));
}

#[test]
fn a_1_4_build_never_gets_a_lights_profile_command() {
    let pad = Arc::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| *c = c.clone().with_slots()),
    );
    let ch = connect(&pad);
    let e = ch.read_lights(0).unwrap_err();
    assert!(e.to_string().contains("no READ_LIGHTS"), "{e}");
    let e = ch.write_lights(&proto::LightsData::default()).unwrap_err();
    assert!(e.to_string().contains("no WRITE_LIGHTS"), "{e}");
    assert!(
        pad.control_log()
            .iter()
            .all(|s| !matches!(s.setup.command_byte(), 0x07 | 0x16))
    );
    // A 1.5 build without LEDs: no bit 11.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput));
    sim.lights_limits.running = false;
    let ch = channel::hello(standalone(sim.with_lights_profiles()), 5)
        .found()
        .unwrap();
    assert!(!ch.hello().has_lights_profiles());
}

#[test]
fn write_lights_checks_everything_before_a_change() {
    let pad = s16_lights_profiles(InputMode::Xinput);
    let ch = connect(&pad);
    let before: Vec<LightsFile> = (0..4).map(|n| lights_file(&ch, n)).collect();
    let token = pad.firmware().token;
    let t = open(&pad);
    let send = |args: &[u8]| {
        let r = t.control_out(
            Setup::command(proto::Command::WriteLights),
            &proto::payload(token, 21, args),
            Duration::from_secs(1),
        );
        (
            r.is_ok(),
            Outcome::from_byte(ch.status().unwrap().last_result),
        )
    };
    let pb = |fields: &[pb::Field]| pb::encode(fields);
    let head = |n: u8, flags: u8| vec![n, flags, 0, 0];
    let with = |mut h: Vec<u8>, body: Vec<u8>| {
        h.extend(body);
        h
    };
    let en = proto::Slot::ENABLED;
    for (args, want) in [
        (vec![1, en, 0], Outcome::BadLength),
        (head(4, en), Outcome::BadArg),
        (head(1, 0x09), Outcome::BadArg),
        (vec![1, en, 1, 0], Outcome::BadArg),
        (with(head(1, en), vec![0xff; 10]), Outcome::BadArg),
        (
            with(head(1, en), pb(&[(2, pb::Value::Varint(20))])),
            Outcome::BadArg,
        ),
        (
            with(head(1, en), pb(&[(4, pb::Value::Varint(10))])),
            Outcome::BadArg,
        ),
        // 14 to 29 are the 16 custom slots: 30 is out of range.
        (
            with(head(1, en), pb(&[(5, pb::Value::Bytes(vec![30]))])),
            Outcome::BadArg,
        ),
        (head(1, proto::Slot::ACTIVE), Outcome::ProfileDisabled),
        (head(0, 0), Outcome::ProfileDisabled),
    ] {
        let (ok, o) = send(&args);
        assert!(!ok, "{args:?}");
        assert_eq!(o, want, "{args:?}");
    }
    let after: Vec<LightsFile> = (0..4).map(|n| lights_file(&ch, n)).collect();
    assert_eq!(after, before, "nothing changed");
    // A web-config preview runs: BUSY.
    let pad = Arc::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| {
                *c = c.clone().with_lights_profiles();
                c.lights_limits.preview = true;
            }),
    );
    let ch = connect(&pad);
    let e = ch.write_lights(&proto::LightsData {
        profile: 1,
        flags: en,
        protobuf: vec![],
    });
    assert!(e.is_err());
    assert_eq!(
        Outcome::from_byte(ch.status().unwrap().last_result),
        Outcome::Busy
    );
}

#[test]
fn a_ram_lights_write_selects_and_goes_away_at_a_restart() {
    let pad = s16_lights_profiles(InputMode::Xinput);
    let ch = connect(&pad);
    let before = lights_file(&ch, 2);
    let sent = LightsFile::parse(r#"{"idle_effect": "rain", "press_hold_ms": 100}"#).unwrap();
    ch.write_lights(&proto::LightsData {
        profile: 2,
        flags: proto::Slot::ENABLED | proto::Slot::ACTIVE,
        protobuf: sent.protobuf(),
    })
    .unwrap();
    let st = ch.status().unwrap();
    assert_eq!(st.last_command, proto::Command::WriteLights as u8);
    let l = ch.lights_status().unwrap();
    assert_eq!((l.profile, l.idle_effect), (2, 19));
    let after = lights_file(&ch, 2);
    assert_eq!(
        after,
        gamepadii_core::lights::expected(&before, &sent, true)
    );
    assert_eq!(after.values[&6], before.values[&6], "the rest stays");
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let ch = connect(&pad);
    assert_eq!(ch.lights_status().unwrap().profile, 0);
    assert_eq!(lights_file(&ch, 2), before);
}

#[test]
fn a_saved_lights_write_survives_a_restart() {
    let pad = s16_lights_profiles(InputMode::Xinput);
    let ch = connect(&pad);
    let sent = LightsFile::parse(r#"{"enabled": false, "case_effect": 3}"#).unwrap();
    ch.write_lights(&proto::LightsData {
        profile: 3,
        flags: proto::Slot::SAVE,
        protobuf: sent.protobuf(),
    })
    .unwrap();
    assert_eq!(ch.lights_status().unwrap().enabled_profiles, 0b0111);
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let ch = connect(&pad);
    let after = lights_file(&ch, 3);
    assert_eq!(
        (after.enabled, after.values[&10].as_u32()),
        (Some(false), Some(3))
    );
}

// ---- v1.7: the input snapshot (§3.12) --------------------------------------------

/// The S16 (v0.7.12 fixtures: profile 2 active) on a v1.7 build, in
/// `mode`, with the inputs from `f`. GPIO 25 has a function, as on the S16.
fn s16_input(mode: InputMode, f: impl FnOnce(&mut SimChannel)) -> Arc<SimPad> {
    Arc::new(SimPad::new(s16_dir(), mode).with_channel().channel(|c| {
        *c = c.clone().with_slots().with_input();
        c.input.functions = 1 << 25;
        f(c);
    }))
}

fn bits(gpios: &[u8]) -> u64 {
    gpios.iter().fold(0, |m, g| m | 1 << g)
}

#[test]
fn a_1_7_build_reads_the_inputs() {
    let pad = s16_input(InputMode::Xinput, |c| {
        c.input.frames = [0, bits(&[27, 3, 6]), 0].into();
    });
    let ch = connect(&pad);
    assert!(ch.hello().has_input());
    assert_eq!(ch.caps().names().last(), Some(&"input"));
    let rest = ch.read_input().unwrap();
    assert!(rest.pressed_gpios().is_empty() && !rest.scan_on());
    // The pins with an action in profile 2; not 15, 22, 25 or 29 (NONE),
    // and not the add-on pins.
    let readable = rest.readable_gpios();
    assert_eq!(readable.len(), 21);
    assert!(readable.contains(&27) && readable.contains(&14));
    assert!(![0, 15, 22, 25, 29].iter().any(|g| readable.contains(g)));
    let held = ch.read_input().unwrap();
    assert_eq!(held.pressed_gpios(), [3, 6, 27]);
    // Up and Down, and L2 on GPIO 6 in SilkSong.
    assert_eq!(held.dpad, 0b0011);
    assert_eq!(held.buttons, 1 << 6);
    assert!(ch.read_input().unwrap().pressed_gpios().is_empty());
    assert!(
        ch.read_input().unwrap().pressed_gpios().is_empty(),
        "the last stays"
    );
    assert_eq!(pad.firmware().input.reads, 4);
}

#[test]
fn a_1_6_build_never_gets_an_input_command() {
    let pad = s16_slots(InputMode::Xinput);
    let ch = connect(&pad);
    let e = ch.read_input().unwrap_err();
    assert!(e.to_string().contains("no READ_INPUT"), "{e}");
    let e = ch.pin_scan(true).unwrap_err();
    assert!(e.to_string().contains("no PIN_SCAN"), "{e}");
    assert!(
        pad.control_log()
            .iter()
            .all(|s| !matches!(s.setup.command_byte(), 0x09 | 0x19))
    );
    // Even with bit 13, a 1.6 HELLO is not enough.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_input();
    sim.minor = 6;
    let ch = channel::hello(standalone(sim), 5).found().unwrap();
    assert!(!ch.hello().has_input());
    assert!(ch.read_input().is_err());
    // On the wire, a build without bit 13 STALLs both.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_slots();
    let r = sim.control_in(Setup::read(proto::Command::ReadInput, 0), 64);
    assert!(matches!(r, Err(UsbError::Stall)));
    let token = sim.token;
    let r = sim.control_out(
        Setup::command(proto::Command::PinScan),
        &proto::payload(token, 1, &[1]),
    );
    assert!(matches!(r, Err(UsbError::Stall)));
    assert_eq!(
        Outcome::from_byte(sim.status().last_result),
        Outcome::Unsupported
    );
}

#[test]
fn pin_scan_checks_its_argument() {
    let pad = s16_input(InputMode::Xinput, |_| {});
    let ch = connect(&pad);
    let token = pad.firmware().token;
    let t = open(&pad);
    let send = |seq: u16, args: &[u8]| {
        let r = t.control_out(
            Setup::command(proto::Command::PinScan),
            &proto::payload(token, seq, args),
            Duration::from_secs(1),
        );
        let st = ch.status().unwrap();
        assert_eq!(st.last_seq, seq);
        (r.is_ok(), Outcome::from_byte(st.last_result))
    };
    assert_eq!(send(1, &[]), (false, Outcome::BadLength));
    assert_eq!(send(2, &[2]), (false, Outcome::BadArg));
    assert_eq!(send(3, &[0xff]), (false, Outcome::BadArg));
    assert!(!pad.firmware().input.scan_on);
    // Off when it is off: OK. Bytes after the argument are ignored.
    assert_eq!(send(4, &[0]), (true, Outcome::Ok));
    assert_eq!(send(5, &[1, 9]), (true, Outcome::Ok));
    assert!(pad.firmware().input.scan_on);
    // The client checks the result as for the other commands.
    let sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_input();
    let ch = channel::hello(standalone(sim), 5).found().unwrap();
    ch.pin_scan(true).unwrap();
    ch.pin_scan(false).unwrap();
}

#[test]
fn the_scan_pins_are_there_only_while_the_scan_is_on() {
    let pad = s16_input(InputMode::Xinput, |c| {
        c.input.frames = [bits(&[15, 22, 25, 27])].into();
    });
    let ch = connect(&pad);
    let before = ch.read_input().unwrap();
    assert_eq!(before.pressed_gpios(), [27], "15 and 22 have no action");
    ch.pin_scan(true).unwrap();
    let on = ch.read_input().unwrap();
    assert!(on.scan_on());
    // GPIO 25 has no action, but a function: the scan leaves it alone.
    assert_eq!(on.pressed_gpios(), [15, 22, 27]);
    assert_eq!(on.readable & !before.readable, bits(&[15, 22, 29]));
    // A press on a scan pin is not a gamepad button.
    assert_eq!((on.dpad, on.buttons), (1, 0));
    ch.pin_scan(true).unwrap();
    assert_eq!(ch.read_input().unwrap().readable, on.readable, "on again");
    ch.pin_scan(false).unwrap();
    let off = ch.read_input().unwrap();
    assert!(!off.scan_on());
    assert_eq!(off.readable, before.readable);
    assert_eq!(off.pressed_gpios(), [27]);
}

#[test]
fn the_scan_stops_after_10_s_with_no_read() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_slots()
        .with_input();
    let token = sim.token;
    sim.control_out(
        Setup::command(proto::Command::PinScan),
        &proto::payload(token, 1, &[1]),
    )
    .unwrap();
    let read = |sim: &mut SimChannel| {
        let r = sim
            .control_in(Setup::read(proto::Command::ReadInput, 0), 64)
            .unwrap();
        assert_eq!(r.len(), proto::INPUT_LEN);
        proto::InputSnapshot::parse(&r).unwrap()
    };
    // Each read keeps it on for 10 s more.
    for _ in 0..3 {
        sim.idle(Duration::from_secs(9));
        assert!(read(&mut sim).scan_on());
    }
    sim.idle(Duration::from_secs(10));
    let late = read(&mut sim);
    assert!(!late.scan_on(), "the read after 10 s stops it");
    assert_eq!(late.readable, 0, "every pin is NONE here, and released");
    // A restart stops it too.
    sim.control_out(
        Setup::command(proto::Command::PinScan),
        &proto::payload(token, 2, &[1]),
    )
    .unwrap();
    sim.boot(Some(InputMode::Xinput), 7);
    assert!(!read(&mut sim).scan_on());
}

// ---- v1.8: the log (§3.13) ----------------------------------------------------------

/// The S16 on a v1.8 main-line build, in `mode`.
fn s16_log(mode: InputMode) -> Arc<SimPad> {
    Arc::new(
        SimPad::new(main_dir(), mode)
            .with_lights()
            .channel(|c| *c = c.clone().with_slots().with_lights_profiles().with_log()),
    )
}

/// The codes of the records, in order.
fn codes(records: &[proto::LogRecord]) -> Vec<u8> {
    records.iter().map(|r| r.code).collect()
}

#[test]
fn a_1_8_build_logs_its_boots_and_commands() {
    use proto::{LogEvent, log_code as c};
    let pad = s16_log(InputMode::Xinput);
    let ch = connect(&pad);
    assert!(ch.hello().has_log());
    assert_eq!(ch.caps().names().last(), Some(&"log"));
    let read = ch.read_log_all(0).unwrap();
    assert_eq!((read.boot, read.next, read.lost), (0, 1, 0));
    assert!(matches!(
        read.records[0].event(),
        LogEvent::Boot {
            kept: false,
            watchdog: false,
            mode: 0,
            ..
        }
    ));
    // IN commands are not in the log; OUT commands are, refused ones too.
    let st = ch.status().unwrap();
    let other = *st.enabled().iter().find(|n| **n != st.profile).unwrap();
    ch.set_profile(other, false).unwrap();
    let off = (1..=6).find(|n| !st.enabled().contains(n)).unwrap();
    assert!(ch.set_profile(off, false).is_err());
    let seq = ch.last_seq().wrapping_add(1);
    ch.reboot(BootMode::Webconfig).unwrap();
    // The restart keeps the log: boot 1, in web-config.
    replug(&pad);
    let ch = connect(&pad);
    let read = ch.read_log_all(read.next).unwrap();
    assert_eq!(
        codes(&read.records),
        [
            c::PROFILE,
            c::COMMAND,
            c::COMMAND,
            c::RESTART,
            c::COMMAND,
            c::BOOT
        ]
    );
    assert_eq!(
        read.records[0].event(),
        LogEvent::Profile {
            old: st.profile,
            new: other
        }
    );
    assert_eq!(
        read.records[4].event(),
        LogEvent::Command {
            cmd: proto::Command::Reboot as u8,
            result: Outcome::Ok,
            seq
        }
    );
    assert!(matches!(
        read.records[2].event(),
        LogEvent::Command {
            result: Outcome::ProfileDisabled,
            ..
        }
    ));
    assert_eq!(read.records[3].event(), LogEvent::Restart { mode: 2 });
    assert_eq!(read.boot, 1);
    assert_eq!(
        read.records[5].event(),
        LogEvent::Boot {
            kept: true,
            watchdog: true,
            mode: 2,
            version: "v0.7.12-119-g8013b41".into()
        }
    );
    assert_eq!((read.records[4].boot, read.records[5].boot), (0, 1));
}

#[test]
fn an_out_refused_in_setup_is_in_the_log_with_seq_0() {
    use proto::LogEvent;
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_log();
    let unknown = Setup {
        index: 0x5030,
        ..Setup::command(proto::Command::Reboot)
    };
    assert!(sim.control_out(unknown, &[0; 8]).is_err());
    let short = Setup::command(proto::Command::Reboot);
    assert!(sim.control_out(short, &[0; 4]).is_err());
    let page = sim.events.as_ref().unwrap().page(0, 1024);
    let events: Vec<LogEvent> = page.records.iter().skip(1).map(|r| r.event()).collect();
    assert_eq!(
        events,
        [
            LogEvent::Command {
                cmd: 0x30,
                result: Outcome::Unsupported,
                seq: 0
            },
            LogEvent::Command {
                cmd: 0x11,
                result: Outcome::BadLength,
                seq: 0
            }
        ]
    );
}

#[test]
fn a_1_7_build_never_gets_read_log() {
    // Bit 14 on a 1.7 build: not the log.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights_profiles();
    sim.minor = 7;
    sim.caps = Caps(sim.caps.0 | Caps::LOG);
    let pad = Arc::new(std::sync::Mutex::new(sim));
    let t = Box::new(Standalone::shared(pad.clone(), "/dev/bus/usb/003/007"));
    let ch = channel::hello(t, 7).found().unwrap();
    let e = ch.read_log(0).unwrap_err();
    assert_eq!(e.exit_code(), exit::USAGE);
    assert!(e.to_string().contains("no READ_LOG"), "{e}");
    assert!(ch.read_log_all(0).is_err());
    assert!(gamepadii_core::log::last_panic(&ch).is_none());
    let sim = pad.lock().unwrap();
    assert!(sim.log.iter().all(|s| s.setup.command_byte() != 0x0a));
    // A build without the log STALLs it.
    let mut old = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights_profiles();
    let r = old.control_in(Setup::read(proto::Command::ReadLog, 0), 1024);
    assert!(matches!(r, Err(UsbError::Stall)));
}

#[test]
fn read_log_needs_a_wlength_of_12_or_more() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_log();
    let setup = Setup::read(proto::Command::ReadLog, 0);
    assert!(matches!(sim.control_in(setup, 11), Err(UsbError::Stall)));
    let head = sim.control_in(setup, 12).unwrap();
    let page = proto::LogPage::parse(&head).unwrap();
    assert_eq!((page.first, page.next, page.records.len()), (0, 1, 0));
    let one = proto::LogPage::parse(&sim.control_in(setup, 44).unwrap()).unwrap();
    assert_eq!(one.records.len(), 1);
}

#[test]
fn the_host_reads_every_page_and_counts_the_lost_records() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_log();
    for i in 0..100u8 {
        sim.add_log(proto::log_code::ERROR, &[b'a' + i % 26]);
    }
    let pad = Arc::new(std::sync::Mutex::new(sim));
    let t = Box::new(Standalone::shared(pad.clone(), "/dev/bus/usb/003/007"));
    let ch = channel::hello(t, 7).found().unwrap();
    let read = ch.read_log_all(0).unwrap();
    assert_eq!((read.next, read.lost, read.records.len()), (101, 37, 64));
    let seqs: Vec<u32> = read.records.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (37..101).collect::<Vec<_>>());
    let reads = |pad: &std::sync::Mutex<SimChannel>| {
        let sim = pad.lock().unwrap();
        sim.log
            .iter()
            .filter(|s| s.setup.command_byte() == 0x0a)
            .count()
    };
    assert_eq!(reads(&pad), 8, "9 records in each reply");
    // From a number in the ring: no loss. From an old one: the rest.
    let read = ch.read_log_all(90).unwrap();
    assert_eq!((read.lost, read.records.len()), (0, 11));
    assert_eq!(ch.read_log_all(10).unwrap().lost, 27);
    // From `next`: nothing new.
    let read = ch.read_log_all(101).unwrap();
    assert_eq!((read.next, read.lost, read.records.len()), (101, 0, 0));
    // A power-on starts a new log: an old number gets the new records.
    pad.lock().unwrap().power_on();
    let read = ch.read_log_all(101).unwrap();
    assert_eq!(
        (read.next, read.lost, codes(&read.records)),
        (1, 0, vec![1])
    );
}

#[test]
fn the_pad_finds_the_full_number_from_16_bits() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_log();
    let log = sim.events.as_mut().unwrap();
    log.records.clear();
    log.next = 0xffc8;
    for _ in 0..64 {
        sim.add_log(proto::log_code::SAVE, &[0, 0]);
    }
    assert_eq!(sim.events.as_ref().unwrap().next, 0x1_0008);
    let pad = Arc::new(std::sync::Mutex::new(sim));
    let t = Box::new(Standalone::shared(pad, "/dev/bus/usb/003/007"));
    let ch = channel::hello(t, 7).found().unwrap();
    // 0xfffc is in the last 65536: the one before the wrap.
    let page = ch.read_log(0xfffc).unwrap();
    assert_eq!((page.first, page.next), (0xfffc, 0x1_0008));
    assert_eq!(ch.read_log(0x1_0002).unwrap().first, 0x1_0002);
    let read = ch.read_log_all(0xfffc).unwrap();
    assert_eq!((read.lost, read.records.len()), (0, 12));
    assert_eq!(read.records.last().unwrap().seq, 0x1_0007);
    // wValue 0 is 0x10000 now; the host still starts at the oldest record.
    let read = ch.read_log_all(0).unwrap();
    assert_eq!(read.records.first().unwrap().seq, 0xffc8);
    assert_eq!((read.lost, read.records.len()), (0xffc8, 64));
}

#[test]
fn a_panic_is_in_the_log_of_the_next_boot() {
    use proto::LogEvent;
    let pad = s16_log(InputMode::Xinput);
    let ch = connect(&pad);
    assert!(gamepadii_core::log::last_panic(&ch).is_none());
    drop(ch);
    let pad = Arc::new(
        SimPad::new(main_dir(), InputMode::Xinput)
            .with_lights()
            .channel(|c| {
                *c = c.clone().with_lights_profiles().with_log();
                c.panic("0123456789abcdefghijklmnopq");
            }),
    );
    let ch = connect(&pad);
    let read = ch.read_log_all(0).unwrap();
    assert_eq!(read.boot, 1);
    assert_eq!(
        read.records[1].event(),
        LogEvent::Panic {
            text: "0123456789abcdefghijklmn".into()
        },
        "cut at 24 bytes"
    );
    let p = gamepadii_core::log::last_panic(&ch).unwrap();
    assert_eq!((p.boot, p.text.as_str()), (0, "0123456789abcdefghijklmn"));
    // Many records after it: still found, in the newest 9.
    for _ in 0..6 {
        ch.set_profile(1, false).unwrap();
    }
    assert!(gamepadii_core::log::last_panic(&ch).is_some());
    // One more restart: the panic is two boots back.
    ch.reboot(BootMode::Gamepad).unwrap();
    replug(&pad);
    let ch = connect(&pad);
    assert!(gamepadii_core::log::last_panic(&ch).is_none());
}

// ---- v1.6: the config blob (§3.14) -------------------------------------------

/// The S16 on a v1.6 main-line build (the log too), in `mode`.
fn s16_config(mode: InputMode, f: impl FnOnce(&mut SimChannel)) -> Arc<SimPad> {
    Arc::new(SimPad::new(main_dir(), mode).with_lights().channel(|c| {
        *c = c
            .clone()
            .with_slots()
            .with_lights_profiles()
            .with_log()
            .with_config();
        f(c);
    }))
}

const BLOB: [u8; 6] = [0x08, 0x01, 0x12, 0x02, 0x08, 0x05];

#[test]
fn a_1_6_build_gives_the_saved_config_in_steps() {
    let pad = s16_config(InputMode::Xinput, |_| {});
    let ch = connect(&pad);
    assert!(ch.hello().has_config());
    assert!(ch.caps().names().contains(&"config"));
    let saved = pad.firmware().config.unwrap().saved;
    assert!(saved.len() > 3 * proto::CONFIG_CHUNK_MAX, "{}", saved.len());
    assert!(pb::decode(&saved).is_some());
    let head = ch.config_head().unwrap();
    assert_eq!(head.size as usize, saved.len());
    assert_eq!(head.crc, proto::crc32(&saved));
    assert_eq!(head.free as usize, 32768 - 12 - saved.len());
    let image = ch.read_config_image().unwrap();
    assert_eq!((image.head, image.data), (head, saved));
    // An offset past the end STALLs; one at the end gives 0 bytes.
    let mut sim = pad.firmware();
    let end = 12 + head.size as u16;
    let at = |v| Setup::read(proto::Command::ReadConfig, v);
    assert!(sim.control_in(at(end), 512).unwrap().is_empty());
    assert!(matches!(
        sim.control_in(at(end + 1), 512),
        Err(UsbError::Stall)
    ));
    assert_eq!(
        sim.control_in(at(0), 4096).unwrap().len(),
        328,
        "the buffer"
    );
}

#[test]
fn a_save_during_a_read_makes_the_host_read_again() {
    let pad = s16_config(InputMode::Xinput, |c| {
        c.config.as_mut().unwrap().save_after_reads = Some(2);
    });
    let ch = connect(&pad);
    let image = ch.read_config_image().unwrap();
    let now = pad.firmware().config.unwrap().saved;
    assert_eq!(image.data, now, "the second read has the new config");
    assert_eq!(image.head.crc, proto::crc32(&now));
}

#[test]
fn a_save_during_each_read_stops_the_host_after_3_reads() {
    let pad = s16_config(InputMode::Xinput, |c| {
        c.config.as_mut().unwrap().save_every_reads = Some(2);
    });
    let ch = connect(&pad);
    let e = ch.read_config_image().unwrap_err();
    assert_eq!(e.exit_code(), exit::TRANSPORT, "{e}");
    assert!(e.to_string().contains("each of 3 reads"), "{e}");
    let reads = pad.firmware().config.unwrap().reads;
    assert!((6..=40).contains(&reads), "{reads}");
}

#[test]
fn a_1_5_build_never_gets_a_config_command() {
    // Bit 12 on a 1.5 build: not the config.
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_config();
    sim.minor = 5;
    let pad = Arc::new(std::sync::Mutex::new(sim));
    let t = Box::new(Standalone::shared(pad.clone(), "/dev/bus/usb/003/007"));
    let ch = channel::hello(t, 7).found().unwrap();
    assert!(!ch.hello().has_config());
    for e in [
        ch.config_head().unwrap_err(),
        ch.read_config_image().unwrap_err(),
        ch.write_config_chunk(0, &[1]).unwrap_err(),
        ch.commit_config(1, 1).map(|_| ()).unwrap_err(),
    ] {
        assert_eq!(e.exit_code(), exit::USAGE);
        assert!(e.to_string().contains("1.6"), "{e}");
    }
    let sim = pad.lock().unwrap();
    for b in [0x08, 0x17, 0x18] {
        assert!(sim.log.iter().all(|s| s.setup.command_byte() != b));
    }
    // A build without bit 12 STALLs an IN and refuses an OUT.
    let mut old = SimChannel::gamepadii(Some(InputMode::Xinput)).with_lights_profiles();
    let r = old.control_in(Setup::read(proto::Command::ReadConfig, 0), 512);
    assert!(matches!(r, Err(UsbError::Stall)));
    let token = old.token;
    let r = old.control_out(
        Setup::command(proto::Command::CommitConfig),
        &proto::payload(token, 3, &[1, 0, 0, 0, 0, 0, 0, 0]),
    );
    assert!(r.is_err());
    assert_eq!(
        Outcome::from_byte(old.status().last_result),
        Outcome::Unsupported
    );
}

#[test]
fn write_and_commit_config_check_in_the_firmware_order() {
    let mut sim = SimChannel::gamepadii(Some(InputMode::Xinput))
        .with_lights_profiles()
        .with_config();
    let token = sim.token;
    let saved = sim.config.as_ref().unwrap().saved.clone();
    let free = sim.config.as_ref().unwrap().free();
    let out = |sim: &mut SimChannel, cmd, args: &[u8]| {
        let r = sim.control_out(Setup::command(cmd), &proto::payload(token, 9, args));
        (r.is_ok(), Outcome::from_byte(sim.status().last_result))
    };
    let chunk = |offset: usize, data: &[u8]| proto::config_chunk(offset as u16, data);
    let w = proto::Command::WriteConfig;
    let c = proto::Command::CommitConfig;
    let commit = proto::config_commit(6, proto::crc32(&BLOB));
    for (cmd, args, want) in [
        (w, vec![0, 0, 0, 0], Outcome::BadLength),
        (w, vec![0, 0, 1, 0, 7], Outcome::BadArg),
        (w, chunk(free, &[1]), Outcome::BadArg),
        (w, chunk(free - 1, &[1, 2]), Outcome::BadArg),
        (c, vec![6, 0, 0, 0, 0, 0, 0], Outcome::BadLength),
        (c, proto::config_commit(0, 0).to_vec(), Outcome::BadArg),
        (
            c,
            proto::config_commit(free as u32 + 1, 0).to_vec(),
            Outcome::BadArg,
        ),
        // Nothing uploaded yet: a CRC mismatch.
        (c, commit.to_vec(), Outcome::BadArg),
    ] {
        assert_eq!(out(&mut sim, cmd, &args), (false, want), "{cmd:?} {args:?}");
    }
    // The CRC agrees, but the bytes are not protobuf (field 0).
    let bad = [0x00, 0x01];
    assert_eq!(out(&mut sim, w, &chunk(0, &bad)), (true, Outcome::Ok));
    let bad_commit = proto::config_commit(2, proto::crc32(&bad));
    assert_eq!(out(&mut sim, c, &bad_commit), (false, Outcome::BadArg));
    assert_eq!(sim.config.as_ref().unwrap().saved, saved, "nothing changed");
    // Forced setup mode: LOCKED, after the length check.
    sim.forced_setup = 2;
    assert_eq!(out(&mut sim, w, &[0; 4]), (false, Outcome::BadLength));
    assert_eq!(out(&mut sim, w, &chunk(0, &BLOB)), (false, Outcome::Locked));
    assert_eq!(out(&mut sim, c, &commit), (false, Outcome::Locked));
    sim.forced_setup = 0;
    // The last byte of the free space is in range.
    assert_eq!(
        out(&mut sim, w, &chunk(free - 1, &[1])),
        (true, Outcome::Ok)
    );
    // Two chunks, then the commit: saved, and a restart.
    assert_eq!(out(&mut sim, w, &chunk(0, &BLOB[..4])), (true, Outcome::Ok));
    assert_eq!(out(&mut sim, w, &chunk(4, &BLOB[4..])), (true, Outcome::Ok));
    assert_eq!(out(&mut sim, c, &commit), (true, Outcome::Ok));
    let cfg = sim.config.as_ref().unwrap();
    assert_eq!((cfg.saved.as_slice(), cfg.commits), (&BLOB[..], 1));
    assert_eq!(cfg.head().free as usize, 32768 - 12 - 6);
    // A restart is pending: BUSY.
    assert_eq!(out(&mut sim, w, &chunk(0, &BLOB)), (false, Outcome::Busy));
}

#[test]
fn a_save_writes_over_the_upload() {
    let pad = s16_config(InputMode::Xinput, |_| {});
    let ch = connect(&pad);
    ch.write_config_chunk(0, &BLOB).unwrap();
    let before = pad.firmware().config.unwrap().saved;
    let st = ch.status().unwrap();
    let other = *st.enabled().iter().find(|n| **n != st.profile).unwrap();
    ch.set_profile(other, true).unwrap();
    let cfg = pad.firmware().config.unwrap();
    assert!(cfg.upload.is_empty());
    assert_ne!(cfg.saved, before, "the save changed the CRC-32");
    assert_eq!(
        ch.commit_config(6, proto::crc32(&BLOB)).unwrap(),
        channel::Commit::BadArg
    );
    assert_eq!(pad.firmware().config.unwrap().commits, 0);
}

#[test]
fn commit_config_saves_and_restarts_the_pad() {
    let pad = s16_config(InputMode::Webconfig, |_| {});
    let ch = connect(&pad);
    assert!(ch.write_config_chunk(0, &[]).is_err());
    assert!(ch.write_config_chunk(0, &[0; 317]).is_err());
    ch.write_config_chunk(0, &BLOB).unwrap();
    let commit = ch.commit_config(6, proto::crc32(&BLOB)).unwrap();
    assert!(matches!(commit, channel::Commit::Accepted(_)));
    replug(&pad);
    let d = gamepadii_core::discovery::Scan::scan(&*pad).remove(0);
    assert_ne!(d.mode, Some(InputMode::Webconfig), "{d:?}");
    let ch = connect(&pad);
    let head = ch.config_head().unwrap();
    assert_eq!((head.size, head.crc), (6, proto::crc32(&BLOB)));
}
