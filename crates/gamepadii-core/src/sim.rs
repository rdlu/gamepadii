//! A simulated pad for tests of the guided flows: one USB device that is
//! [`Scan`]ned like sysfs, and a [`Mock`] web-config server behind it.
//!
//! It re-enumerates like the real thing. A `POST /api/reboot` makes it drop
//! off USB for a few scans, then come back with a new device number: in the
//! saved input mode (what `setGamepadOptions` left in the mock), in
//! web-config, or in BOOTSEL. A scripted "user" can switch it from a gamepad
//! mode into web-config, or to a mode with a boot button, after some scans.
//! The server only answers while the pad is in web-config.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::client::Client;
use crate::discovery::{Device, Scan};
use crate::error::{Error, Result};
use crate::mode::{self, InputMode};
use crate::transport::mock::Mock;
use crate::transport::{ApiPath, Method, Reply, Transport};

/// What the scripted user does, `after` scans into a gamepad mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    /// Switches to web-config (S2 at plug-in, or the hotkey).
    Webconfig { after: usize },
    /// Replugs holding a boot button: comes back in `mode` and saves it.
    BootButton { after: usize, mode: InputMode },
}

#[derive(Debug)]
struct State {
    /// `None`: off USB.
    mode: Option<InputMode>,
    devnum: u16,
    /// Scans left off USB, and what it comes back as.
    coming_back: Option<(usize, InputMode)>,
    reboots_seen: usize,
    scans_in_mode: usize,
    user: Option<UserAction>,
    /// Comes back from a gamepad reboot as this, whatever is saved.
    come_back_as: Option<InputMode>,
    /// Never comes back after a reboot.
    stays_away: bool,
    scans: usize,
}

/// How many scans a reboot keeps the pad off USB.
pub const OFF_USB_SCANS: usize = 3;

pub struct SimPad {
    mock: Arc<Mock>,
    state: Mutex<State>,
}

impl SimPad {
    /// A pad in `mode`, with the web-config server from fixture `dir`.
    pub fn new(dir: impl Into<PathBuf>, mode: InputMode) -> Self {
        Self::with_mock(Mock::new(dir), mode)
    }

    pub fn with_mock(mock: Mock, mode: InputMode) -> Self {
        Self {
            mock: Arc::new(mock),
            state: Mutex::new(State {
                mode: Some(mode),
                devnum: 7,
                coming_back: None,
                reboots_seen: 0,
                scans_in_mode: 0,
                user: None,
                come_back_as: None,
                stays_away: false,
                scans: 0,
            }),
        }
    }

    pub fn user(self, action: UserAction) -> Self {
        self.lock().user = Some(action);
        self
    }

    /// After a reboot into gamepad mode it shows `mode` (a mismatch).
    pub fn come_back_as(self, mode: InputMode) -> Self {
        self.lock().come_back_as = Some(mode);
        self
    }

    /// After a reboot it never re-enumerates.
    pub fn stays_away(self) -> Self {
        self.lock().stays_away = true;
        self
    }

    pub fn mock(&self) -> &Mock {
        &self.mock
    }

    /// How many times it was scanned.
    pub fn scans(&self) -> usize {
        self.lock().scans
    }

    /// The mode it is in now (`None`: off USB).
    pub fn mode(&self) -> Option<InputMode> {
        self.lock().mode
    }

    /// A client for its server; fails unless it is in web-config, like a
    /// pad whose network interface isn't up.
    pub fn open(self: &Arc<Self>) -> Result<Client> {
        if self.mode() != Some(InputMode::Webconfig) {
            return Err(Error::io(
                "GET http://192.168.7.1/api/getFirmwareVersion",
                std::io::Error::new(std::io::ErrorKind::NotConnected, "no route (sim)"),
            ));
        }
        Client::open(Box::new(SimTransport(self.clone())))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn saved_mode(&self) -> InputMode {
        self.mock
            .current("getGamepadOptions")
            .and_then(|o| o.get("inputMode")?.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .and_then(InputMode::from_proto)
            .unwrap_or(InputMode::Xinput)
    }

    fn step(&self) {
        let reboots: Vec<serde_json::Value> = self
            .mock
            .requests()
            .into_iter()
            .filter(|r| r.method == Method::Post && r.path == "/api/reboot")
            .map(|r| r.json())
            .collect();
        let saved = self.saved_mode();
        let mut s = self.lock();
        s.scans += 1;
        if reboots.len() > s.reboots_seen {
            let body = &reboots[reboots.len() - 1];
            s.reboots_seen = reboots.len();
            let next = match body["bootMode"].as_u64() {
                Some(1) => InputMode::Webconfig,
                Some(2) => InputMode::Bootsel,
                _ => s.come_back_as.unwrap_or(saved),
            };
            s.mode = None;
            s.coming_back = (!s.stays_away).then_some((OFF_USB_SCANS, next));
            return;
        }
        if let Some((left, next)) = s.coming_back {
            if left > 1 {
                s.coming_back = Some((left - 1, next));
            } else {
                s.coming_back = None;
                s.mode = Some(next);
                s.devnum += 1;
                s.scans_in_mode = 0;
            }
            return;
        }
        let in_gamepad = !matches!(
            s.mode,
            None | Some(InputMode::Webconfig) | Some(InputMode::Bootsel)
        );
        if in_gamepad {
            s.scans_in_mode += 1;
            let due = match s.user {
                Some(UserAction::Webconfig { after }) if s.scans_in_mode > after => {
                    Some(InputMode::Webconfig)
                }
                Some(UserAction::BootButton { after, mode }) if s.scans_in_mode > after => {
                    Some(mode)
                }
                _ => None,
            };
            if let Some(next) = due {
                s.user = None;
                s.mode = None;
                s.coming_back = Some((OFF_USB_SCANS, next));
            }
        }
    }
}

/// A [`Device`] as sysfs would show the pad in `mode`.
pub fn device(mode: InputMode, devnum: u16) -> Device {
    let known = mode::KNOWN
        .iter()
        .find(|k| k.modes.contains(&mode))
        .expect("every mode has an id");
    let det = mode::detect(known.vid, known.pid, None, None).expect("a known id");
    Device {
        mode: det.mode,
        candidates: det.candidates,
        confidence: det.confidence,
        note: det.note,
        vid: known.vid,
        pid: known.pid,
        manufacturer: None,
        product: None,
        serial: None,
        bus: Some(3),
        devnum: Some(devnum),
        port: "3-1".into(),
        sysfs_path: None,
        netdev: (mode == InputMode::Webconfig).then(|| "usb0".into()),
    }
}

impl Scan for SimPad {
    fn scan(&self) -> Vec<Device> {
        self.step();
        let s = self.lock();
        s.mode.map(|m| device(m, s.devnum)).into_iter().collect()
    }
}

impl Scan for Arc<SimPad> {
    fn scan(&self) -> Vec<Device> {
        (**self).scan()
    }
}

struct SimTransport(Arc<SimPad>);

impl Transport for SimTransport {
    fn request(&self, m: Method, p: ApiPath, b: Option<&[u8]>) -> Result<Reply> {
        self.0.mock.request(m, p, b)
    }
    fn describe(&self) -> String {
        self.0.mock.describe()
    }
}
