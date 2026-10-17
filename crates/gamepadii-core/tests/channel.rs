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
        matches!(p, Probe::Absent(ref r) if r.contains("Short(0)")),
        "{p:?}"
    );
}

#[test]
fn stock_xbox_one_answers_with_garbage_and_is_not_the_channel() {
    let sim = SimChannel::stock(Some(InputMode::Xbone));
    let p = channel::hello(standalone(sim), 7);
    assert!(
        matches!(p, Probe::Absent(ref r) if r.contains("Magic")),
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
        matches!(p, Probe::Absent(ref r) if r.contains("Nonce")),
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
            matches!(p, Probe::Absent(ref r) if r.contains("STALL")),
            "{m:?}: {p:?}"
        );
    }
    // The boot ROM has no firmware at all.
    let pad = Arc::new(SimPad::new(s16_dir(), InputMode::Bootsel).with_channel());
    assert!(matches!(channel::hello(open(&pad), 9), Probe::Absent(_)));
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
