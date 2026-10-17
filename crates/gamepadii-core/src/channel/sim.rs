//! The firmware side of the control channel, simulated: what a pad answers
//! to vendor requests on EP0, with the channel built in or not.
//!
//! Without the channel it behaves like stock GP2040-CE (§2.3 of the design
//! note): XInput answers any vendor IN with 0 bytes and ACKs any OUT, Xbox
//! One answers an IN with stack garbage and ACKs any OUT, every other mode
//! (and the boot ROM) STALLs. With the channel, our requests get the v1
//! protocol (HELLO, STATUS, STRING, SET_PROFILE, REBOOT, SET_INPUT_MODE,
//! forced-setup locks, the token, STATUS results), and anything else still
//! takes the stock path.
//!
//! [`SimChannel`] is only the protocol engine. `crate::sim::SimPad` wraps it
//! with USB re-enumeration (a REBOOT drops the pad off the bus), and
//! [`Standalone`] wraps it for the CLI's hidden `--mock-channel`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use super::proto::{self, Caps, Command, Hello, Outcome, Setup, Status};
use super::{ControlTransport, UsbError};
use crate::mode::{BootMode, InputMode};
use crate::transport::mock::Mock;

/// What the pad does after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Restarts (500 ms after the ACK on real hardware).
    Reboot(BootMode),
}

/// One request the pad saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub setup: Setup,
    /// The OUT data stage (empty for IN).
    pub data: Vec<u8>,
}

/// The capabilities of our v1 firmware: no BOOTSEL (issue #2).
pub const V1_CAPS: u32 =
    Caps::STATUS | Caps::STRING | Caps::SET_PROFILE | Caps::REBOOT | Caps::SET_INPUT_MODE;

/// `InputMode` 0–15: what v0.7.12 can boot.
pub const V0712_MODES: u32 = 0xffff;

#[derive(Debug, Clone)]
pub struct SimChannel {
    /// The channel is built into this firmware.
    pub enabled: bool,
    pub caps: Caps,
    pub modes: u32,
    /// STRING id → value.
    pub strings: BTreeMap<u16, String>,
    /// Bit 0 is profile 1.
    pub enabled_profiles: u8,
    /// The active profile (RAM).
    pub active: u8,
    /// The profile in flash.
    pub saved_profile: u8,
    pub forced_setup: u8,
    /// The running mode's `InputMode` value (255 web-config); `None` is the
    /// boot ROM.
    pub running: Option<u8>,
    /// The input mode in flash.
    pub saved_mode: u8,
    pub token: u32,
    /// `(seq, command, result)` of the last OUT command.
    pub last: (u16, u8, u8),
    pub pending: bool,
    /// A REBOOT or SET_INPUT_MODE was accepted: every OUT is BUSY until the
    /// pad restarts.
    pub reboot_pending: bool,
    pub uptime_ms: u32,
    /// What an Xbox One IN gets, repeated to `wLength`.
    pub garbage: Vec<u8>,
    pub log: Vec<Seen>,
}

impl SimChannel {
    /// Stock firmware: no channel.
    pub fn stock(running: Option<InputMode>) -> Self {
        let mut strings = BTreeMap::new();
        strings.insert(proto::string::VERSION, "v0.7.12".into());
        strings.insert(proto::string::BOARD, "Pico".into());
        strings.insert(proto::string::BOARD_LABEL, "Pico".into());
        strings.insert(proto::string::BUILD_TYPE, "Release".into());
        strings.insert(proto::string::profile_label(1), "Default".into());
        Self {
            enabled: false,
            caps: Caps(V1_CAPS),
            modes: V0712_MODES,
            strings,
            enabled_profiles: 1,
            active: 1,
            saved_profile: 1,
            forced_setup: 0,
            running: running.map(proto_byte),
            saved_mode: 0,
            token: 0x5eed_0001,
            last: (0, 0, 0),
            pending: false,
            reboot_pending: false,
            uptime_ms: 1000,
            garbage: (0..=255u8).map(|i| i.wrapping_mul(37) ^ 0xa5).collect(),
            log: Vec::new(),
        }
    }

    /// Our firmware with the channel, a single Default profile.
    pub fn gamepadii(running: Option<InputMode>) -> Self {
        Self {
            enabled: true,
            ..Self::stock(running)
        }
    }

    /// Our firmware on the pad behind `mock`: strings, profiles, saved
    /// mode and locks from its web-config fixtures.
    pub fn from_mock(mock: &Mock, running: Option<InputMode>) -> Self {
        let mut s = Self::gamepadii(running);
        s.sync_from(mock);
        if let Some(f) = mock
            .current("getGamepadOptions")
            .and_then(|o| o.get("forcedSetupMode")?.as_u64())
        {
            s.forced_setup = u8::try_from(f).unwrap_or(0);
        }
        if let Some(fw) = mock.current("getFirmwareVersion") {
            for (id, key) in [
                (proto::string::VERSION, "version"),
                (proto::string::BOARD, "boardConfig"),
                (proto::string::BOARD_LABEL, "boardConfigLabel"),
                (proto::string::BUILD_TYPE, "boardBuildType"),
            ] {
                if let Some(v) = fw.get(key).and_then(Value::as_str) {
                    s.strings.insert(id, v.to_string());
                }
            }
        }
        let base = mock.current("getPinMappings");
        let alts = mock
            .current("getProfileOptions")
            .and_then(|p| p.get("alternativePinMappings").cloned())
            .and_then(|a| a.as_array().cloned())
            .unwrap_or_default();
        let label = |v: &Value| {
            v.get("profileLabel")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        s.enabled_profiles = 1;
        if let Some(b) = &base {
            s.strings.insert(proto::string::profile_label(1), label(b));
        }
        for (i, alt) in alts.iter().take(5).enumerate() {
            let n = i as u8 + 2;
            s.strings
                .insert(proto::string::profile_label(n), label(alt));
            if alt.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                s.enabled_profiles |= 1 << (n - 1);
            }
        }
        s.active = s.saved_profile;
        s
    }

    /// Takes the saved input mode and profile from the mock's
    /// `getGamepadOptions` (a web-config write may have changed them).
    pub fn sync_from(&mut self, mock: &Mock) {
        let Some(o) = mock.current("getGamepadOptions") else {
            return;
        };
        let byte = |k: &str| {
            o.get(k)
                .and_then(Value::as_u64)
                .and_then(|v| u8::try_from(v).ok())
        };
        if let Some(m) = byte("inputMode") {
            self.saved_mode = m;
        }
        if let Some(p) = byte("profileNumber") {
            self.saved_profile = p;
        }
    }

    /// Restarts: `running` is the new mode, `token` the new boot's token.
    pub fn boot(&mut self, running: Option<InputMode>, token: u32) {
        self.running = running.map(proto_byte);
        self.token = token;
        self.active = self.saved_profile;
        self.pending = false;
        self.reboot_pending = false;
        self.last = (0, 0, 0);
        self.uptime_ms = 1000;
    }

    /// `None`: not ours, the stock path. `Some(None)`: ours, but a
    /// command this firmware doesn't know (it STALLs).
    fn matched(&self, setup: &Setup) -> Option<Option<Command>> {
        if !self.enabled || self.running.is_none() || !setup.ours() {
            return None;
        }
        // A known command must agree with the direction.
        match Command::from_byte(setup.command_byte()) {
            Some(c) if c.is_in() != setup.is_in() => None,
            known => Some(known),
        }
    }

    /// The stock driver's answer to a vendor IN.
    fn stock_in(&self, len: u16) -> Result<Vec<u8>, UsbError> {
        match self.running {
            Some(0) => Ok(Vec::new()),
            Some(5) => Ok(self
                .garbage
                .iter()
                .copied()
                .cycle()
                .take(usize::from(len))
                .collect()),
            _ => Err(UsbError::Stall),
        }
    }

    fn stock_out(&self) -> Result<(), UsbError> {
        match self.running {
            Some(0 | 5) => Ok(()),
            _ => Err(UsbError::Stall),
        }
    }

    pub fn control_in(&mut self, setup: Setup, len: u16) -> Result<Vec<u8>, UsbError> {
        self.log.push(Seen {
            setup,
            data: Vec::new(),
        });
        self.uptime_ms += 1;
        let Some(cmd) = self.matched(&setup) else {
            return self.stock_in(len);
        };
        let Some(cmd) = cmd.filter(|_| len <= proto::MAX_LEN) else {
            return Err(UsbError::Stall);
        };
        let reply = match cmd {
            Command::Hello => Hello {
                major: proto::MAJOR,
                minor: 0,
                caps: self.caps,
                token: self.token,
                modes: self.modes,
            }
            .encode(setup.value)
            .to_vec(),
            Command::Status => {
                let s = self.status();
                // The main loop applies a profile change on its next pass.
                self.pending = false;
                s.encode().to_vec()
            }
            Command::String => match self.strings.get(&setup.value) {
                Some(v) => v.as_bytes().iter().copied().take(63).collect(),
                None if (0x11..=0x16).contains(&setup.value) => Vec::new(),
                None => return Err(UsbError::Stall),
            },
            _ => return Err(UsbError::Stall),
        };
        Ok(reply.into_iter().take(usize::from(len)).collect())
    }

    pub fn status(&self) -> Status {
        Status {
            major: proto::MAJOR,
            input_mode: self.running.unwrap_or(255),
            profile: self.active,
            enabled_profiles: self.enabled_profiles,
            forced_setup: self.forced_setup,
            flags: if self.pending {
                Status::PROFILE_PENDING
            } else {
                0
            } | if self.reboot_pending {
                Status::REBOOT_PENDING
            } else {
                0
            },
            last_seq: self.last.0,
            last_command: self.last.1,
            last_result: self.last.2,
            uptime_ms: self.uptime_ms,
        }
    }

    pub fn control_out(&mut self, setup: Setup, data: &[u8]) -> Result<Option<Effect>, UsbError> {
        self.log.push(Seen {
            setup,
            data: data.to_vec(),
        });
        self.uptime_ms += 1;
        let Some(cmd) = self.matched(&setup) else {
            return self.stock_out().map(|()| None);
        };
        // Rejected in SETUP (§3.6 item 7): the payload is never read.
        let byte = setup.command_byte();
        let Some(cmd) = cmd else {
            self.last = (0, byte, Outcome::Unsupported.byte());
            return Err(UsbError::Stall);
        };
        if data.len() < proto::HEADER_LEN || data.len() > usize::from(proto::MAX_LEN) {
            self.last = (0, byte, Outcome::BadLength.byte());
            return Err(UsbError::Stall);
        }
        let (token, seq) = proto::header(data).expect("checked the length");
        let result = if token != self.token {
            Err(Outcome::BadToken)
        } else if self.reboot_pending {
            Err(Outcome::Busy)
        } else {
            self.apply(cmd, &data[proto::HEADER_LEN..])
        };
        if matches!(result, Ok(Some(Effect::Reboot(_)))) {
            self.reboot_pending = true;
        }
        let code = match &result {
            Ok(_) => Outcome::Ok,
            Err(o) => *o,
        };
        self.last = (seq, cmd as u8, code.byte());
        result.map_err(|_| UsbError::Stall)
    }

    fn apply(&mut self, cmd: Command, args: &[u8]) -> Result<Option<Effect>, Outcome> {
        // Bytes past the arguments are ignored, for later minors.
        if args.len() < cmd.payload_len().unwrap_or(usize::MAX) {
            return Err(Outcome::BadLength);
        }
        let in_webconfig = self.running == Some(255);
        match cmd {
            Command::SetProfile => {
                let (n, persist) = (args[0], args[1]);
                if !(1..=6).contains(&n) || persist > 1 {
                    return Err(Outcome::BadArg);
                }
                if self.enabled_profiles & (1 << (n - 1)) == 0 {
                    return Err(Outcome::ProfileDisabled);
                }
                if self.active != n {
                    self.active = n;
                    self.pending = !in_webconfig;
                }
                if persist == 1 {
                    self.saved_profile = n;
                }
                Ok(None)
            }
            Command::Reboot => {
                let mode = match args[0] {
                    0 => BootMode::Gamepad,
                    1 if matches!(self.forced_setup, 2 | 3) => return Err(Outcome::Locked),
                    1 => BootMode::Webconfig,
                    2 if self.caps.has(Caps::REBOOT_BOOTSEL) => BootMode::Bootsel,
                    // Not offered by our fork (issue #2, §3.6 item 1).
                    2 => return Err(Outcome::Unsupported),
                    _ => return Err(Outcome::BadArg),
                };
                Ok(Some(Effect::Reboot(mode)))
            }
            Command::SetInputMode => {
                let m = args[0];
                if matches!(self.forced_setup, 1 | 3) {
                    return Err(Outcome::Locked);
                }
                if m >= 32 || self.modes & (1 << m) == 0 {
                    return Err(Outcome::Unsupported);
                }
                self.saved_mode = m;
                Ok(Some(Effect::Reboot(BootMode::Gamepad)))
            }
            _ => Err(Outcome::BadArg),
        }
    }
}

fn proto_byte(m: InputMode) -> u8 {
    match m {
        InputMode::Bootsel => 254,
        m => m
            .proto_value()
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(254),
    }
}

/// A [`SimChannel`] on its own, ignoring reboots: the CLI's hidden
/// `--mock-channel`, for end-to-end tests against static sysfs fixtures.
pub struct Standalone {
    sim: Mutex<SimChannel>,
    node: String,
    no_access: bool,
}

impl Standalone {
    pub fn new(sim: SimChannel, node: impl Into<String>) -> Self {
        Self {
            sim: Mutex::new(sim),
            node: node.into(),
            no_access: false,
        }
    }

    /// Every request fails as on a node without the udev rule.
    pub fn no_access(mut self) -> Self {
        self.no_access = true;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SimChannel> {
        self.sim.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ControlTransport for Standalone {
    fn control_in(&self, setup: Setup, len: u16, _: Duration) -> Result<Vec<u8>, UsbError> {
        if self.no_access {
            return Err(UsbError::NoAccess);
        }
        self.lock().control_in(setup, len)
    }

    fn control_out(&self, setup: Setup, data: &[u8], _: Duration) -> Result<(), UsbError> {
        if self.no_access {
            return Err(UsbError::NoAccess);
        }
        self.lock().control_out(setup, data).map(|_| ())
    }

    fn describe(&self) -> String {
        self.node.clone()
    }
}
