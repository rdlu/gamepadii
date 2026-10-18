//! The control channel must never send a command to a device that did not
//! prove itself with HELLO. GP2040-CE borrows the ids of real controllers
//! (a real Xbox 360 pad, and an 8BitDo pad in XInput mode, are also
//! `045e:028e`), so a device with our ids can be anything. Each test here
//! gives gamepadii a device with a bad HELLO reply and runs the commands
//! that use the channel: no OUT request may follow, and a connection that
//! answered "no channel" is not asked again.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gamepadii_core::channel::{self, ControlTransport, Probe, Setup, UsbError, proto};
use gamepadii_core::commands;
use gamepadii_core::discovery::{Device, Scan};
use gamepadii_core::flow::{self, Env, Silent, Timeouts};
use gamepadii_core::leds::{LedChange, Level};
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::sim;
use gamepadii_core::wait::ManualClock;

/// What the fake device answers to HELLO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    Stall,
    Empty,
    Garbage,
    WrongNonce,
    WrongMagic,
    NewerMajor,
    Timeout,
}

const STOCK: [Reply; 5] = [
    Reply::Stall,
    Reply::Empty,
    Reply::Garbage,
    Reply::WrongNonce,
    Reply::WrongMagic,
];

/// A device that answers every IN with `reply` and records every request.
#[derive(Clone)]
struct Fake {
    reply: Reply,
    log: Arc<Mutex<Vec<Setup>>>,
}

impl Fake {
    fn new(reply: Reply) -> Self {
        Self {
            reply,
            log: Arc::default(),
        }
    }

    fn requests(&self) -> Vec<Setup> {
        self.log.lock().unwrap().clone()
    }

    fn outs(&self) -> usize {
        self.requests().iter().filter(|s| !s.is_in()).count()
    }
}

impl ControlTransport for Fake {
    fn control_in(&self, setup: Setup, len: u16, _: Duration) -> Result<Vec<u8>, UsbError> {
        self.log.lock().unwrap().push(setup);
        let hello = |nonce: u16, major: u8| {
            let mut b = proto::Hello {
                major,
                minor: 3,
                caps: proto::Caps(0x3ff),
                token: 7,
                modes: 0xffff,
            }
            .encode(nonce)
            .to_vec();
            b.resize(usize::from(len), 0);
            b
        };
        match self.reply {
            Reply::Stall => Err(UsbError::Stall),
            Reply::Empty => Ok(Vec::new()),
            Reply::Garbage => Ok((0..len)
                .map(|i| (i as u8).wrapping_mul(37) ^ 0xa5)
                .collect()),
            Reply::WrongNonce => Ok(hello(setup.value.wrapping_add(1), 1)),
            Reply::WrongMagic => {
                let mut b = hello(setup.value, 1);
                b[0..4].copy_from_slice(b"GPIi");
                Ok(b)
            }
            Reply::NewerMajor => Ok(hello(setup.value, 2)),
            Reply::Timeout => Err(UsbError::Timeout),
        }
    }

    fn control_out(&self, setup: Setup, _: &[u8], _: Duration) -> Result<(), UsbError> {
        self.log.lock().unwrap().push(setup);
        Ok(())
    }

    fn describe(&self) -> String {
        "/dev/bus/usb/003/007".into()
    }
}

/// One device on USB, in XInput mode: a real Xbox 360 pad, as far as
/// gamepadii can see.
struct One(Device);

impl Scan for One {
    fn scan(&self) -> Vec<Device> {
        vec![self.0.clone()]
    }
}

fn xinput() -> One {
    One(sim::device(InputMode::Xinput, 7))
}

fn run<T>(fake: &Fake, dir: &Path, f: impl FnOnce(&Env) -> T) -> T {
    let scan = xinput();
    let clock = ManualClock::at(1_792_256_223);
    let open = || {
        Err(gamepadii_core::error::Error::NotFound(
            "no web-config".into(),
        ))
    };
    let t = fake.clone();
    let control = move |_: &Device| Ok(Box::new(t.clone()) as Box<dyn ControlTransport>);
    let env = Env {
        scan: &scan,
        clock: &clock,
        open: &open,
        control: Some(&control),
        cache_dir: Some(dir.to_path_buf()),
        backups_dir: None,
        url: "http://192.168.7.1/".into(),
        timeouts: Timeouts::default(),
        poll: Duration::from_millis(200),
    };
    f(&env)
}

/// Every command that can use the channel, on the device behind `fake`.
fn every_command(fake: &Fake, dir: &Path) {
    run(fake, dir, |env| {
        let _ = commands::status_with(env.scan, env.control, env.cache_dir.as_deref());
        let _ = flow::profile_switch(env, &"2".parse().unwrap(), true, &mut Silent);
        let _ = flow::mode(env, InputMode::Keyboard, false, &mut Silent);
        let _ = flow::webconfig(env, false, &mut Silent);
        let _ = flow::reboot_pad(env, BootMode::Webconfig, false, &mut Silent);
        let _ = flow::leds_now(env);
        let change = LedChange {
            brightness: Some(Level::To(1)),
            ..LedChange::default()
        };
        let _ = flow::leds_set(env, &change);
        let _ = flow::profile_show(env, None);
        let file = gamepadii_core::slots::SlotFile::parse(r#"{"pins": []}"#).unwrap();
        let write = gamepadii_core::slots::SlotWrite {
            slot: 6,
            select: true,
            ..Default::default()
        };
        let _ = flow::profile_write(env, &file, &write, &mut Silent);
        let _ = flow::lights_show(env, None);
        let file = gamepadii_core::lights::LightsFile::default();
        let write = gamepadii_core::lights::LightsWrite {
            profile: 1,
            select: true,
            ..Default::default()
        };
        let _ = flow::lights_write(env, &file, &write, &mut Silent);
        let _ = flow::input(env);
        let find = flow::FindPin {
            scan: true,
            timeout: Duration::ZERO,
            stop: None,
        };
        let _ = flow::find_pin(env, &find, &mut Silent);
        let _ = flow::log(env, None);
        let _ = flow::log_follow(env, Some(5), &mut |_| false);
        let _ = flow::backup_channel(env, Some(&dir.join("blob.json")));
        let data = [0x08, 0x01];
        let file = dir.join("restore.json");
        let bundle = serde_json::json!({
            "schema": 1,
            "gamepadii_version": "0",
            "captured_at": "2026-10-18T00:00:00Z",
            "firmware": {},
            "endpoints": {},
            "config_blob": {
                "size": 2,
                "crc32": proto::crc32(&data),
                "firmware": null,
                "board": null,
                "board_id": null,
                "hex": gamepadii_core::blob::hex(&data),
            },
        });
        std::fs::write(&file, bundle.to_string()).unwrap();
        let bundle = commands::read_backup(&file).unwrap();
        assert!(bundle.config_blob.is_some());
        let _ = flow::restore(env, &file, &bundle, &mut Silent);
    });
}

#[test]
fn no_command_follows_a_bad_hello() {
    for reply in STOCK.into_iter().chain([Reply::NewerMajor, Reply::Timeout]) {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(reply);
        every_command(&fake, dir.path());
        assert_eq!(fake.outs(), 0, "{reply:?}: an OUT request was sent");
        for s in fake.requests() {
            assert_eq!(
                s.command_byte(),
                proto::Command::Hello as u8,
                "{reply:?}: only HELLO may be sent"
            );
        }
    }
}

#[test]
fn a_bad_hello_is_not_the_channel() {
    for reply in STOCK {
        let p = channel::hello(Box::new(Fake::new(reply)), 0x1234);
        assert!(matches!(p, Probe::Stock(_)), "{reply:?}: {p:?}");
    }
    let p = channel::hello(Box::new(Fake::new(Reply::NewerMajor)), 0x1234);
    assert!(matches!(p, Probe::Newer { major: 2, .. }), "{p:?}");
    let p = channel::hello(Box::new(Fake::new(Reply::Timeout)), 0x1234);
    assert!(matches!(p, Probe::Absent(_)), "{p:?}");
}

#[test]
fn a_connection_without_the_channel_is_asked_one_time() {
    for reply in STOCK {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(reply);
        for _ in 0..3 {
            every_command(&fake, dir.path());
        }
        assert_eq!(fake.requests().len(), 1, "{reply:?}: HELLO was sent again");
    }
}

#[test]
fn a_new_connection_is_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(Reply::Stall);
    let open = |_: &Device| Ok(Box::new(fake.clone()) as Box<dyn ControlTransport>);
    let first = sim::device(InputMode::Xinput, 7);
    commands::probe(&open, &first, Some(dir.path()));
    commands::probe(&open, &first, Some(dir.path()));
    assert_eq!(fake.requests().len(), 1);
    // A replug or a reboot: a new device number on the same port.
    let replugged = sim::device(InputMode::Xinput, 8);
    commands::probe(&open, &replugged, Some(dir.path()));
    assert_eq!(fake.requests().len(), 2);
    commands::probe(&open, &first, Some(dir.path()));
    assert_eq!(
        fake.requests().len(),
        3,
        "the port's old connection is gone"
    );
}

#[test]
fn no_answer_is_asked_again() {
    // A timeout can be a pad that is still starting: it is not "no channel".
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(Reply::Timeout);
    for _ in 0..3 {
        every_command(&fake, dir.path());
    }
    assert!(fake.requests().len() > 3, "{}", fake.requests().len());
    assert_eq!(fake.outs(), 0);
}
