//! A simulated pad for tests of the guided flows: one USB device that is
//! [`Scan`]ned like sysfs, and a [`Mock`] web-config server behind it.
//!
//! It re-enumerates like the real thing. A `POST /api/reboot` makes it drop
//! off USB for a few scans, then come back with a new device number: in the
//! saved input mode (what `setGamepadOptions` left in the mock), in
//! web-config, or in BOOTSEL. A scripted "user" can switch it from a gamepad
//! mode into web-config, or to a mode with a boot button, after some scans.
//! The server only answers while the pad is in web-config.
//!
//! On EP0 it runs a [`SimChannel`]: stock firmware by default (XInput
//! answers vendor requests with 0 bytes, Xbox One with garbage, the rest
//! STALL), or with [`SimPad::with_channel`] the gamepadii control channel,
//! whose REBOOT and SET_INPUT_MODE re-enumerate the pad like `/api/reboot`
//! and save into the same mock state the web-config API reads (a saved LED
//! change too, into `getConfig`'s `animationOptions` when the fixture has
//! one). [`SimPad::with_leds`] adds the v1.1 LED commands.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::time::Duration;

use crate::channel::sim::{Effect, Seen, SimChannel};
use crate::channel::{ControlTransport, Setup, UsbError};
use crate::client::Client;
use crate::discovery::{Device, Scan};
use crate::error::{Error, Result};
use crate::mode::{self, BootMode, InputMode};
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
    /// Reboots so far (HTTP or the channel).
    reboots: usize,
    /// Never comes back from this reboot (1 is the first) on.
    away_from: Option<usize>,
    /// The web configurator never answers (the network didn't come up).
    server_down: bool,
    scans: usize,
    /// What answers on EP0.
    channel: SimChannel,
    /// Opening the device node fails with `EACCES` (no udev rule).
    no_access: bool,
    /// The RP2040's unique board id, which some modes put in their USB
    /// serial; `None` shows no serial at all.
    unique_id: Option<[u8; 8]>,
    /// The pad is off USB because the user replugged it.
    replugged: bool,
}

impl State {
    /// Counts a reboot; whether the pad comes back from it.
    fn comes_back(&mut self) -> bool {
        self.reboots += 1;
        self.away_from.is_none_or(|n| self.reboots < n)
    }
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
                reboots: 0,
                away_from: None,
                server_down: false,
                scans: 0,
                channel: SimChannel::stock(Some(mode)),
                no_access: false,
                unique_id: None,
                replugged: false,
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
        self.stays_away_after(0)
    }

    /// Comes back from the first `n` reboots, never from the next.
    pub fn stays_away_after(self, n: usize) -> Self {
        self.lock().away_from = Some(n + 1);
        self
    }

    /// In web-config its server never answers.
    pub fn server_down(self) -> Self {
        self.lock().server_down = true;
        self
    }

    /// Runs the gamepadii firmware, with the control channel, set up from
    /// the mock's fixtures (strings, profiles, saved mode, locks).
    pub fn with_channel(self) -> Self {
        let running = self.lock().mode;
        let ch = SimChannel::from_mock(&self.mock, running);
        self.lock().channel = ch;
        self
    }

    /// The gamepadii firmware with control channel v1.1: the LED commands.
    pub fn with_leds(self) -> Self {
        self.with_channel().channel(|c| *c = c.clone().with_leds())
    }

    /// The fork's main line with control channel v1.2: the lights
    /// commands, from the mock's `getAnimationProtoOptions` when it has one.
    pub fn with_lights(self) -> Self {
        self.with_channel()
            .channel(|c| *c = c.clone().with_lights())
    }

    /// Changes the simulated firmware (locks, capabilities, garbage).
    pub fn channel(self, f: impl FnOnce(&mut SimChannel)) -> Self {
        f(&mut self.lock().channel);
        self
    }

    /// The board's unique id: its USB serial is the firmware's in each mode
    /// ([`serial`]), where without one it has none.
    pub fn unique_id(self, id: [u8; 8]) -> Self {
        self.lock().unique_id = Some(id);
        self
    }

    /// Control channel v1.3: STRING 0x05 gives the unique id (set with
    /// [`SimPad::unique_id`] first) as 16 hex digits.
    pub fn with_board_id(self) -> Self {
        let id = self.lock().unique_id.expect("unique_id first");
        let hex: String = id.iter().map(|b| format!("{b:02X}")).collect();
        self.channel(|c| *c = c.clone().with_board_id(&hex))
    }

    /// On USB with device number `n` (7 by default): a replug gets a new
    /// one.
    pub fn with_devnum(self, n: u16) -> Self {
        self.lock().devnum = n;
        self
    }

    /// The device node can't be opened (no udev rule).
    pub fn no_access(self) -> Self {
        self.lock().no_access = true;
        self
    }

    /// Every vendor request the pad has seen on EP0.
    pub fn control_log(&self) -> Vec<Seen> {
        self.lock().channel.log.clone()
    }

    /// The simulated firmware's state now.
    pub fn firmware(&self) -> SimChannel {
        self.lock().channel.clone()
    }

    /// Changes the running firmware (a log record, a panic) while a flow
    /// uses the pad.
    pub fn change_firmware(&self, f: impl FnOnce(&mut SimChannel)) {
        f(&mut self.lock().channel);
    }

    /// The pad leaves USB, and does not come back.
    pub fn unplug(&self) {
        let mut s = self.lock();
        s.mode = None;
        s.coming_back = None;
    }

    /// Opens the pad's EP0 as found at `d` (a [`crate::channel::OpenControl`]).
    /// The handle goes stale when the pad re-enumerates, like a real fd.
    pub fn open_control(
        self: &Arc<Self>,
        d: &Device,
    ) -> std::result::Result<Box<dyn ControlTransport>, UsbError> {
        let s = self.lock();
        if s.no_access {
            return Err(UsbError::NoAccess);
        }
        if s.mode.is_none() || d.devnum != Some(s.devnum) {
            return Err(UsbError::Gone);
        }
        Ok(Box::new(SimControl {
            pad: self.clone(),
            devnum: s.devnum,
        }))
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
        if self.mode() != Some(InputMode::Webconfig) || self.lock().server_down {
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
            s.coming_back = s.comes_back().then_some((OFF_USB_SCANS, next));
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
                let token = 0x5eed_0000 ^ u32::from(s.devnum).wrapping_mul(0x9e37_79b9);
                s.channel.sync_from(&self.mock);
                s.channel.boot(Some(next), token);
                if std::mem::take(&mut s.replugged) {
                    s.channel.power_on();
                }
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
                // A replug holding a boot button is a power-on: a new log.
                s.replugged = matches!(s.user, Some(UserAction::BootButton { .. }));
                s.user = None;
                s.mode = None;
                s.coming_back = Some((OFF_USB_SCANS, next));
            }
        }
    }
}

/// The USB serial GP2040-CE v0.7.12 reports in `mode` on a board with
/// unique id `id`: XInput `"0"` and bytes 5 to 7 in hex, Xbox One a letter
/// per byte and `8ABCDEFGH`, web-config `123456`, Switch Pro
/// `000000000001`; the other modes have none.
pub fn serial(mode: InputMode, id: [u8; 8]) -> Option<String> {
    match mode {
        InputMode::Xinput => Some(format!("0{:02X}{:02X}{:02X}", id[5], id[6], id[7])),
        InputMode::Xbone => {
            let letters: String = id.iter().map(|b| char::from(b'A' + b % 25)).collect();
            Some(format!("{letters}8ABCDEFGH"))
        }
        InputMode::Webconfig => Some("123456".into()),
        InputMode::SwitchPro => Some("000000000001".into()),
        _ => None,
    }
}

/// A [`Device`] as sysfs would show the pad in `mode`.
pub fn device(mode: InputMode, devnum: u16) -> Device {
    device_with_id(mode, devnum, None)
}

/// [`device`] on a board with unique id `id`, which gives it a serial.
pub fn device_with_id(mode: InputMode, devnum: u16, id: Option<[u8; 8]>) -> Device {
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
        serial: id.and_then(|id| serial(mode, id)),
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
        s.mode
            .map(|m| device_with_id(m, s.devnum, s.unique_id))
            .into_iter()
            .collect()
    }
}

impl Scan for Arc<SimPad> {
    fn scan(&self) -> Vec<Device> {
        (**self).scan()
    }
}

/// What a reboot over the channel comes back as.
fn reboot_target(s: &State, saved: InputMode, mode: BootMode) -> InputMode {
    match mode {
        BootMode::Gamepad => s.come_back_as.unwrap_or(saved),
        BootMode::Webconfig => InputMode::Webconfig,
        BootMode::Bootsel => InputMode::Bootsel,
    }
}

/// EP0 of the pad as it was enumerated at `devnum`.
struct SimControl {
    pad: Arc<SimPad>,
    devnum: u16,
}

impl SimControl {
    fn with<T>(
        &self,
        f: impl FnOnce(&mut State) -> std::result::Result<T, UsbError>,
    ) -> std::result::Result<T, UsbError> {
        let mut s = self.pad.lock();
        if s.mode.is_none() || s.devnum != self.devnum {
            return Err(UsbError::Gone);
        }
        if s.mode == Some(InputMode::Bootsel) {
            // The boot ROM STALLs vendor requests it doesn't know.
            return Err(UsbError::Stall);
        }
        s.channel.sync_from(&self.pad.mock);
        f(&mut s)
    }

    /// Writes what the channel saved into the mock's options, as the
    /// firmware's `Storage::save` would.
    fn save(&self, ch: &SimChannel) {
        let mock = &self.pad.mock;
        let Some(serde_json::Value::Object(mut o)) = mock.current("getGamepadOptions") else {
            return;
        };
        let changed = o.get("inputMode").and_then(|v| v.as_u64()) != Some(ch.saved_mode.into())
            || o.get("profileNumber").and_then(|v| v.as_u64()) != Some(ch.saved_profile.into());
        if changed {
            o.insert("inputMode".into(), ch.saved_mode.into());
            o.insert("profileNumber".into(), ch.saved_profile.into());
            mock.store("getGamepadOptions", serde_json::Value::Object(o));
        }
        let Some(mut config) = mock.current("getConfig") else {
            return;
        };
        let Some(serde_json::Value::Object(a)) = config.get_mut("animationOptions") else {
            return;
        };
        let l = ch.saved_leds;
        let mut changed = false;
        for (key, v) in [
            ("baseAnimationIndex", l.animation),
            ("brightness", l.brightness),
            ("staticColorIndex", l.static_color),
            ("buttonColorIndex", l.button_color),
            ("themeIndex", l.theme),
        ] {
            if a.get(key).and_then(|x| x.as_u64()) != Some(v.into()) {
                a.insert(key.into(), v.into());
                changed = true;
            }
        }
        if changed {
            mock.store("getConfig", config);
        }
    }
}

impl ControlTransport for SimControl {
    fn control_in(
        &self,
        setup: Setup,
        len: u16,
        _: Duration,
    ) -> std::result::Result<Vec<u8>, UsbError> {
        self.with(|s| s.channel.control_in(setup, len))
    }

    fn control_out(
        &self,
        setup: Setup,
        data: &[u8],
        _: Duration,
    ) -> std::result::Result<(), UsbError> {
        self.with(|s| {
            let effect = s.channel.control_out(setup, data)?;
            self.save(&s.channel);
            if let Some(Effect::Reboot(mode)) = effect {
                let saved =
                    InputMode::from_proto(s.channel.saved_mode.into()).unwrap_or(InputMode::Xinput);
                let next = reboot_target(s, saved, mode);
                s.mode = None;
                s.coming_back = s.comes_back().then_some((OFF_USB_SCANS, next));
            }
            Ok(())
        })
    }

    fn describe(&self) -> String {
        format!("/dev/bus/usb/003/{:03}", self.devnum)
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
