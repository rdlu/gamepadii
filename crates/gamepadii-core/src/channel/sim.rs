//! The firmware side of the control channel, simulated: what a pad answers
//! to vendor requests on EP0, with the channel built in or not.
//!
//! Without the channel it behaves like stock GP2040-CE (§2.3 of the design
//! note): XInput answers any vendor IN with 0 bytes and ACKs any OUT, Xbox
//! One answers an IN with stack garbage and ACKs any OUT, every other mode
//! (and the boot ROM) STALLs. With the channel, our requests get the v1
//! protocol (HELLO, STATUS, STRING, SET_PROFILE, REBOOT, SET_INPUT_MODE,
//! forced-setup locks, the token, STATUS results), and anything else still
//! takes the stock path. [`SimChannel::with_leds`] makes it a v1.1 build
//! with the LED commands (LED_STATUS, SET_LED: RAM and saved LED state,
//! range checks); without that, they are unknown commands, as on v1.0.
//!
//! [`SimChannel`] is only the protocol engine. `crate::sim::SimPad` wraps it
//! with USB re-enumeration (a REBOOT drops the pad off the bus), and
//! [`Standalone`] wraps it for the CLI's hidden `--mock-channel`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use super::proto::{self, Caps, Command, Hello, LedStatus, Outcome, SetLed, Setup, Status};
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

/// The LED settings SET_LED changes (`AnimationOptions`), in RAM or saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLeds {
    pub animation: u8,
    pub brightness: u8,
    pub static_color: u8,
    pub button_color: u8,
    pub theme: u8,
}

impl Default for SimLeds {
    /// The S16 as read live (issue #19): custom theme, brightness 1 of 5.
    fn default() -> Self {
        Self {
            animation: 4,
            brightness: 1,
            static_color: 2,
            button_color: 1,
            theme: 0,
        }
    }
}

/// What the LED values are checked against, and the fixed flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLedLimits {
    /// The LEDs are set up (`dataPin >= 0`), so AnimationStation runs: the
    /// LED capability is only advertised then.
    pub running: bool,
    /// `hasCustomTheme`: the custom-theme effect (4) is there too.
    pub custom_theme: bool,
    pub brightness_steps: u8,
    pub brightness_maximum: u8,
    pub colors: u8,
    pub themes: u8,
}

impl Default for SimLedLimits {
    /// v0.7.12 on the S16: 5 effects with the custom theme, 5 steps up to
    /// 100, 16 colours, 20 static themes.
    fn default() -> Self {
        Self {
            running: true,
            custom_theme: true,
            brightness_steps: 5,
            brightness_maximum: 100,
            colors: 16,
            themes: 20,
        }
    }
}

impl SimLedLimits {
    /// The effects the firmware cycles through: 4, and the custom theme
    /// when there is one.
    pub fn animations(&self) -> u8 {
        4 + u8::from(self.custom_theme)
    }
}

/// The firmware's names for the effects (STRING `0x20 + n`).
const EFFECT_NAMES: [&str; 5] = [
    "EFFECT_STATIC_COLOR",
    "EFFECT_RAINBOW",
    "EFFECT_CHASE",
    "EFFECT_STATIC_THEME",
    "EFFECT_CUSTOM_THEME",
];

#[derive(Debug, Clone)]
pub struct SimChannel {
    /// The channel is built into this firmware.
    pub enabled: bool,
    /// The protocol minor in HELLO: 0, or 1 with the LED commands.
    pub minor: u8,
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
    /// The LEDs as they run (RAM).
    pub leds: SimLeds,
    /// The LEDs in flash: what a restart goes back to.
    pub saved_leds: SimLeds,
    pub led_limits: SimLedLimits,
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
            minor: 0,
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
            leds: SimLeds::default(),
            saved_leds: SimLeds::default(),
            led_limits: SimLedLimits::default(),
            log: Vec::new(),
        }
    }

    /// A v1.1 build: protocol minor 1, with the LED commands (advertised
    /// only when the LEDs are set up).
    pub fn with_leds(mut self) -> Self {
        self.minor = 1;
        if self.led_limits.running {
            self.caps = Caps(self.caps.0 | Caps::LEDS);
        }
        self
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
        s.leds_from(mock);
        s
    }

    /// The LED state from the mock: `animationOptions` from its `getConfig`
    /// (the fork fixture has one), the brightness limits from
    /// `getLedOptions`, the custom theme from `getCustomTheme`.
    fn leds_from(&mut self, mock: &Mock) {
        let byte = |o: &Value, k: &str| {
            o.get(k)
                .and_then(Value::as_u64)
                .and_then(|v| u8::try_from(v).ok())
        };
        if let Some(a) = mock
            .current("getConfig")
            .and_then(|c| c.get("animationOptions").cloned())
        {
            let l = &mut self.saved_leds;
            for (field, key) in [
                (&mut l.animation, "baseAnimationIndex"),
                (&mut l.brightness, "brightness"),
                (&mut l.static_color, "staticColorIndex"),
                (&mut l.button_color, "buttonColorIndex"),
                (&mut l.theme, "themeIndex"),
            ] {
                if let Some(v) = byte(&a, key) {
                    *field = v;
                }
            }
            self.leds = self.saved_leds;
        }
        if let Some(o) = mock.current("getLedOptions") {
            if let Some(v) = byte(&o, "brightnessSteps") {
                self.led_limits.brightness_steps = v;
            }
            if let Some(v) = byte(&o, "brightnessMaximum") {
                self.led_limits.brightness_maximum = v;
            }
            if let Some(pin) = o.get("dataPin").and_then(Value::as_i64) {
                self.led_limits.running = pin >= 0;
            }
        }
        if let Some(t) = mock.current("getCustomTheme") {
            self.led_limits.custom_theme = t.get("enabled").and_then(Value::as_u64) == Some(1)
                || t.get("enabled").and_then(Value::as_bool) == Some(true);
        }
    }

    /// LED_STATUS as the firmware would answer it now.
    pub fn led_status(&self) -> LedStatus {
        let (l, k) = (&self.leds, &self.led_limits);
        let flag = |on: bool, bit: u8| if on { bit } else { 0 };
        LedStatus {
            // Bit 0 is always set: without the LEDs there is no reply.
            flags: LedStatus::RUNNING
                | flag(k.custom_theme, LedStatus::CUSTOM_THEME)
                | flag(self.leds != self.saved_leds, LedStatus::UNSAVED),
            animation: l.animation,
            animations: k.animations(),
            brightness: l.brightness,
            brightness_steps: k.brightness_steps,
            brightness_maximum: k.brightness_maximum,
            static_color: l.static_color,
            button_color: l.button_color,
            theme: l.theme,
            colors: k.colors,
            themes: k.themes,
        }
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
        self.leds = self.saved_leds;
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
            // A v1.0 build (or one without LEDs) doesn't know them.
            Some(Command::LedStatus | Command::SetLed) if !self.caps.has(Caps::LEDS) => Some(None),
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
                minor: self.minor,
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
                None if self.caps.has(Caps::LEDS)
                    && (0x20..0x20 + u16::from(self.led_limits.animations()))
                        .contains(&setup.value) =>
                {
                    EFFECT_NAMES[usize::from(setup.value - 0x20)]
                        .as_bytes()
                        .to_vec()
                }
                None => return Err(UsbError::Stall),
            },
            Command::LedStatus => self.led_status().encode().to_vec(),
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
                    // Any save writes the whole config, the LEDs' RAM state too.
                    self.saved_leds = self.leds;
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
                self.saved_leds = self.leds;
                Ok(Some(Effect::Reboot(BootMode::Gamepad)))
            }
            Command::SetLed => {
                let set = SetLed::parse(args).ok_or(Outcome::BadArg)?;
                let k = self.led_limits;
                let fits = |v: Option<u8>, below: u16| v.is_none_or(|v| u16::from(v) < below);
                let ok = fits(set.animation, k.animations().into())
                    && fits(set.brightness, u16::from(k.brightness_steps) + 1)
                    && fits(set.static_color, k.colors.into())
                    && fits(set.button_color, k.colors.into())
                    && fits(set.theme, k.themes.into());
                if !ok {
                    return Err(Outcome::BadArg);
                }
                let l = &mut self.leds;
                for (field, v) in [
                    (&mut l.animation, set.animation),
                    (&mut l.brightness, set.brightness),
                    (&mut l.static_color, set.static_color),
                    (&mut l.button_color, set.button_color),
                    (&mut l.theme, set.theme),
                ] {
                    if let Some(v) = v {
                        *field = v;
                    }
                }
                if set.persist {
                    self.saved_leds = self.leds;
                    // The save writes the profile too.
                    self.saved_profile = self.active;
                }
                Ok(None)
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
