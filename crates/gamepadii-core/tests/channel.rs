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
use gamepadii_core::mode::{BootMode, InputMode};
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
