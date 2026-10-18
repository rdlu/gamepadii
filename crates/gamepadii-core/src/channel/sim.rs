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
//! [`SimChannel::with_lights`] makes it a main-line v1.2 build with the
//! lights commands (LIGHTS_STATUS, SET_LIGHTS) in place of the LED ones.
//! [`SimChannel::with_board_id`] makes it a v1.3 build with STRING 0x05;
//! before that, the id STALLs. [`SimChannel::with_slots`] makes it a v1.4
//! build with READ_SLOT and WRITE_SLOT: the pin mappings of each profile,
//! in RAM and saved, from the mock's `getPinMappings` and
//! `getProfileOptions`. [`SimChannel::with_lights_profiles`] makes it a
//! v1.5 build with READ_LIGHTS and WRITE_LIGHTS: each lights profile's
//! `AnimationProfile` fields, in RAM and saved, from the mock's
//! `getAnimationProtoOptions`. [`SimChannel::with_input`] makes it a v1.7
//! build with READ_INPUT and PIN_SCAN: the pins a person holds come from a
//! script ([`SimInput::frames`]), one entry for each read.
//! [`SimChannel::with_log`] makes it a v1.8
//! build with READ_LOG: a ring of the last 64 events that the simulator
//! writes (BOOT, COMMAND, PROFILE, SAVE, RESTART), and a PANIC or a
//! power-on when a test asks for one. [`SimChannel::with_config`] makes it
//! a v1.6 build with READ_CONFIG, WRITE_CONFIG and COMMIT_CONFIG: the saved
//! config as one protobuf blob, the free space for an upload, and the
//! restart after COMMIT_CONFIG.
//!
//! [`SimChannel`] is only the protocol engine. `crate::sim::SimPad` wraps it
//! with USB re-enumeration (a REBOOT drops the pad off the bus), and
//! [`Standalone`] wraps it for the CLI's hidden `--mock-channel`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::proto::{
    self, Caps, Command, ConfigHead, Hello, InputSnapshot, LedStatus, LightsData, LightsStatus,
    LogPage, LogRecord, Outcome, SetLed, SetLights, Setup, Slot, SlotPin, Status, log_code,
};
use super::{ControlTransport, UsbError};
use crate::api::{PinMappings, ProfileOptions};
use crate::input;
use crate::lights::LightsFile;
use crate::mode::{BootMode, InputMode};
use crate::pb;
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

/// `InputMode` 0–17 and 19: what main can boot (§3.8).
pub const MAIN_MODES: u32 = 0x000b_ffff;

/// The lights settings SET_LIGHTS changes, in RAM or saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLights {
    /// -1 off, or 0–3.
    pub profile: i8,
    pub brightness: u8,
}

impl Default for SimLights {
    fn default() -> Self {
        Self {
            profile: 0,
            brightness: 5,
        }
    }
}

/// The effects and cycle times of a lights profile, as LIGHTS_STATUS gives
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SimLightsProfile {
    pub enabled: bool,
    pub idle_effect: u8,
    pub pressed_effect: u8,
    pub case_effect: u8,
    pub idle_cycle_time: u8,
    pub pressed_cycle_time: u8,
    pub case_cycle_time: u8,
}

/// What SET_LIGHTS checks against, and what it can't change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLightsLimits {
    /// `NeoPicoLEDAddon` is set up: [`Caps::LIGHTS`] only then.
    pub running: bool,
    pub brightness_steps: u8,
    pub brightness_maximum: u8,
    pub profiles: [SimLightsProfile; 4],
    pub idle_timeout_s: u16,
    /// A web-config preview runs: SET_LIGHTS is BUSY.
    pub preview: bool,
    /// v1.5: how many LIGHTS_STATUS reads after a SET_LIGHTS show it
    /// PENDING (core 1 applies it on its next frame); 0 on older builds.
    pub pending_reads: u8,
}

impl Default for SimLightsLimits {
    /// Main's defaults: 10 steps, all four profiles enabled.
    fn default() -> Self {
        Self {
            running: true,
            brightness_steps: 10,
            brightness_maximum: 128,
            profiles: [SimLightsProfile {
                enabled: true,
                ..SimLightsProfile::default()
            }; 4],
            idle_timeout_s: 0,
            preview: false,
            pending_reads: 0,
        }
    }
}

/// The lights profiles in flash: the fields and the limits' profiles.
pub type SavedLightsProfiles = ([BTreeMap<u32, pb::Value>; 4], [SimLightsProfile; 4]);

/// How long the pin scan stays on with no READ_INPUT (`SCAN_TIMEOUT_MS`).
pub const SCAN_TIMEOUT_MS: u32 = 10_000;

/// v1.7: what READ_INPUT reads, and the pin scan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimInput {
    /// The pins that a person holds down, as a mask: one entry for each
    /// READ_INPUT. Each read takes the next entry; the last one stays.
    /// Empty: no pin is held.
    pub frames: VecDeque<u64>,
    /// The pins with a GPIO function (the LED data pin, a UART): the scan
    /// never changes them.
    pub functions: u64,
    /// The pins that PIN_SCAN made inputs.
    pub scan_pins: u64,
    pub scan_on: bool,
    /// The `uptime_ms` after which the next READ_INPUT stops the scan.
    pub scan_deadline_ms: u32,
    /// How many READ_INPUTs the pad answered.
    pub reads: usize,
}

impl SimInput {
    /// A script: `0` for `before` reads, then `pins` for `held` reads, then
    /// `0`.
    pub fn press(pins: u64, before: usize, held: usize) -> VecDeque<u64> {
        std::iter::repeat_n(0, before)
            .chain(std::iter::repeat_n(pins, held))
            .chain([0])
            .collect()
    }
}

/// The pins of an RP2040 (`NUM_BANK0_GPIOS`).
pub const RP2040_PINS: u8 = 30;

/// One profile's pin mappings, as `GpioMappings` holds them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimSlot {
    pub label: String,
    pub enabled: bool,
    pub pins: Vec<SlotPin>,
}

impl SimSlot {
    /// The slot of a `getPinMappings` or `getProfileOptions` entry.
    fn from_api(p: &PinMappings, count: u8) -> Self {
        let pins = p.pins();
        SimSlot {
            label: p.profile_label.clone().unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            pins: (0..count)
                .map(|gpio| {
                    pins.get(&gpio).map_or(SlotPin::default(), |m| SlotPin {
                        action: i16::try_from(m.action).unwrap_or(-10),
                        button_mask: u32::try_from(m.custom_button_mask).unwrap_or(0),
                        dpad_mask: u32::try_from(m.custom_dpad_mask).unwrap_or(0),
                    })
                })
                .collect(),
        }
    }
}

/// The firmware's buffer for a reply: a READ_LOG gets at most 9 records.
pub const BUFFER_LEN: usize = 328;

/// The log of a v1.8 build (§3.13): the last 64 records, in RAM that a
/// soft reboot keeps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimLog {
    /// The sequence number of the next record.
    pub next: u32,
    /// The boot number: 1 more at each restart that keeps the log.
    pub boot: u16,
    /// The records that are left, oldest first: at most 64.
    pub records: VecDeque<LogRecord>,
    /// The boot mode of the next boot (from the last RESTART), as the
    /// watchdog scratch register keeps it.
    pub restart_mode: u8,
}

impl SimLog {
    fn add(&mut self, ms: u32, code: u8, data: &[u8]) {
        let n = data.len().min(proto::LOG_DATA_LEN);
        self.records.push_back(LogRecord {
            seq: self.next,
            ms,
            boot: self.boot,
            code,
            data: data[..n].to_vec(),
        });
        while self.records.len() > proto::LOG_RECORDS as usize {
            self.records.pop_front();
        }
        self.next = self.next.wrapping_add(1);
    }

    /// READ_LOG as `buildLog` answers it: `from` is the low 16 bits of a
    /// number in the last 65536.
    pub fn page(&self, from: u16, len: u16) -> LogPage {
        let next = self.next;
        let oldest = next.saturating_sub(proto::LOG_RECORDS);
        let mut first = (next & !0xffff) | u32::from(from);
        if first > next {
            first = first.saturating_sub(0x1_0000);
        }
        let first = first.max(oldest);
        let room = (usize::from(len).min(BUFFER_LEN) - proto::LOG_HEAD_LEN) / proto::LOG_RECORD_LEN;
        let count = usize::try_from(next - first)
            .unwrap_or(usize::MAX)
            .min(room);
        LogPage {
            first,
            next,
            boot: self.boot,
            records: self
                .records
                .iter()
                .filter(|r| r.seq >= first)
                .take(count)
                .cloned()
                .collect(),
        }
    }
}

/// The FlashPROM block (`EEPROM_SIZE_BYTES`): the free space, then the
/// saved config, then its 12-byte footer.
pub const CONFIG_FLASH: usize = 32 * 1024;
/// The footer after the saved config: size, CRC-32, magic.
const CONFIG_FOOTER_LEN: usize = 12;

/// v1.6: the saved config, as the FlashPROM cache holds it (§3.14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimConfig {
    /// The saved blob: a `Config` message, protobuf encoded. Empty: no
    /// config is saved.
    pub saved: Vec<u8>,
    /// The free start of the cache, where WRITE_CONFIG puts the chunks.
    /// Bytes after its end are 0.
    pub upload: Vec<u8>,
    /// After this many READ_CONFIG replies, a save changes the saved
    /// config (a hotkey during a read).
    pub save_after_reads: Option<usize>,
    /// After each this many READ_CONFIG replies, a save (a pad that saves
    /// all the time).
    pub save_every_reads: Option<usize>,
    /// The READ_CONFIG replies that the pad gave.
    pub reads: usize,
    /// After this many WRITE_CONFIGs, a save writes over the upload (a
    /// hotkey during the upload).
    pub save_after_chunks: Option<usize>,
    /// The WRITE_CONFIGs that the pad took.
    pub chunks: usize,
    /// The COMMIT_CONFIGs that the pad took.
    pub commits: usize,
}

impl SimConfig {
    pub fn new(saved: Vec<u8>) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    /// The largest blob that WRITE_CONFIG and COMMIT_CONFIG take now
    /// (`configStageMax`).
    pub fn free(&self) -> usize {
        CONFIG_FLASH - CONFIG_FOOTER_LEN - self.saved.len()
    }

    pub fn head(&self) -> ConfigHead {
        ConfigHead {
            size: self.saved.len() as u32,
            crc: if self.saved.is_empty() {
                0
            } else {
                proto::crc32(&self.saved)
            },
            free: self.free() as u32,
        }
    }

    /// The READ_CONFIG image: the head, then the saved config.
    pub fn image(&self) -> Vec<u8> {
        let mut b = self.head().encode().to_vec();
        b.extend_from_slice(&self.saved);
        b
    }

    /// The first `n` bytes of the upload.
    fn uploaded(&self, n: usize) -> Vec<u8> {
        let mut b: Vec<u8> = self.upload.iter().copied().take(n).collect();
        b.resize(n, 0);
        b
    }

    /// A save: `ConfigUtils::save` encodes the config into the start of
    /// the cache, over the upload. The sim puts a field at the start of
    /// the saved blob: all the bytes after it move, and the CRC-32 changes.
    fn save(&mut self) {
        self.upload.clear();
        if !self.saved.is_empty() {
            let mut b = Vec::new();
            pb::encode_field(&mut b, 100, &pb::Value::Varint(1));
            b.append(&mut self.saved);
            self.saved = b;
        }
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
    /// The protocol minor in HELLO: 0, 1 with the LED commands, 2 on the
    /// main line, 3 with the board ID.
    pub minor: u8,
    /// The main line: the lights commands are known (behind
    /// [`Caps::LIGHTS`]) and the v1.1 LED ones are not.
    pub main: bool,
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
    /// Main's lights as they run (RAM).
    pub lights: SimLights,
    /// Main's lights in flash.
    pub saved_lights: SimLights,
    pub lights_limits: SimLightsLimits,
    /// LIGHTS_STATUS reads left that show PENDING.
    pub lights_pending: u8,
    /// `NUM_BANK0_GPIOS`.
    pub pin_count: u8,
    /// The profiles the config has, by number (1 is `getPinMappings`), in
    /// RAM. A number that isn't here was never written: READ_SLOT gives
    /// zeros.
    pub slots: BTreeMap<u8, SimSlot>,
    /// The same in flash.
    pub saved_slots: BTreeMap<u8, SimSlot>,
    /// v1.5: each lights profile's `AnimationProfile` fields in RAM, by
    /// number. `lights_limits.profiles` holds `bEnabled`, the effects and
    /// the cycle times: they win over the fields here.
    pub lights_fields: [BTreeMap<u32, pb::Value>; 4],
    /// The lights profiles in flash, when a RAM-only WRITE_LIGHTS made them
    /// differ; `None`: flash has what RAM has.
    pub saved_lights_profiles: Option<SavedLightsProfiles>,
    /// v1.7: the inputs and the pin scan.
    pub input: SimInput,
    /// v1.8: the pad's log; `None` before 1.8.
    pub events: Option<SimLog>,
    /// v1.6: the saved config as one blob; `None` before 1.6.
    pub config: Option<SimConfig>,
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
            main: false,
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
            lights: SimLights::default(),
            saved_lights: SimLights::default(),
            lights_limits: SimLightsLimits::default(),
            lights_pending: 0,
            pin_count: RP2040_PINS,
            slots: default_slots(),
            saved_slots: default_slots(),
            lights_fields: Default::default(),
            saved_lights_profiles: None,
            input: SimInput::default(),
            events: None,
            config: None,
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

    /// A main-line build: protocol minor 2 (or more), main's modes, the
    /// lights commands (advertised only when the LEDs are set up) and no
    /// v1.1 LED commands.
    pub fn with_lights(mut self) -> Self {
        self.main = true;
        self.minor = self.minor.max(2);
        self.modes = MAIN_MODES;
        let lights = if self.lights_limits.running {
            Caps::LIGHTS
        } else {
            0
        };
        self.caps = Caps(self.caps.0 & !Caps::LEDS | lights);
        self
    }

    /// A v1.4 build: protocol minor 4, with READ_SLOT and WRITE_SLOT.
    pub fn with_slots(mut self) -> Self {
        self.minor = self.minor.max(proto::SLOTS_MINOR);
        self.caps = Caps(self.caps.0 | Caps::SLOTS);
        self
    }

    /// A v1.3 build: protocol minor 3, with the board ID as STRING 0x05.
    pub fn with_board_id(mut self, id: &str) -> Self {
        self.minor = self.minor.max(proto::BOARD_ID_MINOR);
        self.strings.insert(proto::string::BOARD_ID, id.into());
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
        s.lights_from(mock);
        s.slots_from(base.as_ref(), &alts);
        s
    }

    /// The pin mappings from the mock's `getPinMappings` (profile 1) and
    /// `getProfileOptions` (the profiles after it, at most 5).
    fn slots_from(&mut self, base: Option<&Value>, alts: &[Value]) {
        let parse = |v: &Value| serde_json::from_value::<PinMappings>(v.clone()).ok();
        if let Some(b) = base.and_then(parse) {
            let mut slot = SimSlot::from_api(&b, self.pin_count);
            slot.enabled = true;
            self.slots.insert(1, slot);
        }
        let options = ProfileOptions {
            alternative_pin_mappings: alts.iter().filter_map(parse).collect(),
            ..ProfileOptions::default()
        };
        for (p, n) in options.alternative_pin_mappings.iter().take(5).zip(2u8..) {
            self.slots.insert(n, SimSlot::from_api(p, self.pin_count));
        }
        self.saved_slots = self.slots.clone();
    }

    /// READ_SLOT `n` as the firmware answers it (`buildSlot`).
    pub fn slot(&self, n: u8) -> Slot {
        let count = usize::from(self.pin_count);
        let found = self.slots.get(&n);
        let enabled = found.is_some_and(|s| n == 1 || s.enabled);
        let flag = |on: bool, bit: u8| if on { bit } else { 0 };
        Slot {
            profile: n,
            flags: flag(enabled, Slot::ENABLED) | flag(self.active == n, Slot::ACTIVE),
            label: found.map(|s| s.label.clone()).unwrap_or_default(),
            pins: found.map_or_else(|| vec![SlotPin::default(); count], |s| s.pins.clone()),
        }
    }

    /// The enabled bitmap and the label strings, after a change of the
    /// slots `numbers`.
    fn sync_slots(&mut self, numbers: impl IntoIterator<Item = u8>) {
        for n in numbers.into_iter().filter(|n| (2..=6).contains(n)) {
            let bit = 1 << (n - 1);
            match self.slots.get(&n) {
                Some(s) => {
                    if s.enabled {
                        self.enabled_profiles |= bit;
                    } else {
                        self.enabled_profiles &= !bit;
                    }
                    self.strings
                        .insert(proto::string::profile_label(n), s.label.clone());
                }
                None => {
                    self.enabled_profiles &= !bit;
                    self.strings.remove(&proto::string::profile_label(n));
                }
            }
        }
    }

    /// A v1.5 build: protocol minor 5, with READ_LIGHTS and WRITE_LIGHTS
    /// (advertised only when the LEDs run). A main-line build.
    pub fn with_lights_profiles(mut self) -> Self {
        self = self.with_lights();
        self.minor = self.minor.max(proto::LIGHTS_PROFILES_MINOR);
        if self.lights_limits.running {
            self.caps = Caps(self.caps.0 | Caps::LIGHTS_PROFILES);
        }
        self
    }

    /// A v1.7 build: protocol minor 7, with READ_INPUT and PIN_SCAN.
    pub fn with_input(mut self) -> Self {
        self.minor = self.minor.max(proto::INPUT_MINOR);
        self.caps = Caps(self.caps.0 | Caps::INPUT);
        self
    }

    /// The action of each GPIO as the pad runs the active profile: the
    /// pin of profile 1 where the slot has 0 or -5, or where profile 1
    /// reserves the pin or gives it to an add-on.
    pub fn functional_actions(&self) -> Vec<i16> {
        let base = self.slot(1).pins;
        let active = self.slot(self.active).pins;
        base.iter()
            .zip(active.iter().chain(std::iter::repeat(&SlotPin::default())))
            .map(|(b, a)| {
                if self.active == 1
                    || crate::slots::follows_profile_1(a.action)
                    || crate::slots::follows_profile_1(b.action)
                {
                    b.action
                } else {
                    a.action
                }
            })
            .collect()
    }

    /// Stops the pin scan: the pins it made inputs are released.
    fn stop_scan(&mut self) {
        self.input.scan_pins = 0;
        self.input.scan_on = false;
    }

    /// READ_INPUT as `buildInput` answers it: the scan stops when 10 s
    /// passed with no read, else it stays on for 10 s more.
    pub fn read_input(&mut self) -> InputSnapshot {
        if self.input.scan_on {
            if self.uptime_ms >= self.input.scan_deadline_ms {
                self.stop_scan();
            } else {
                self.input.scan_deadline_ms = self.uptime_ms + SCAN_TIMEOUT_MS;
            }
        }
        self.input.reads += 1;
        let held = if self.input.frames.len() > 1 {
            self.input.frames.pop_front().unwrap_or(0)
        } else {
            self.input.frames.front().copied().unwrap_or(0)
        };
        let actions = self.functional_actions();
        let gamepad = actions
            .iter()
            .zip(0u8..)
            .filter(|(a, _)| **a > 0)
            .fold(0u64, |m, (_, gpio)| m | 1 << gpio);
        let readable = gamepad | self.input.scan_pins;
        let pressed = held & readable;
        let mut snap = InputSnapshot {
            pressed,
            readable,
            flags: if self.input.scan_on {
                InputSnapshot::SCAN_ON
            } else {
                0
            },
            ..InputSnapshot::default()
        };
        for (action, gpio) in actions.iter().zip(0u8..) {
            if pressed & (1 << gpio) != 0 {
                let (dpad, buttons, aux) = input::state_bits(*action);
                snap.dpad |= dpad;
                snap.buttons |= buttons;
                snap.aux |= aux;
            }
        }
        snap
    }

    /// The pad runs with no READ_INPUT for `d`.
    pub fn idle(&mut self, d: Duration) {
        let ms = u32::try_from(d.as_millis()).unwrap_or(u32::MAX);
        self.uptime_ms = self.uptime_ms.saturating_add(ms);
    }

    /// A v1.6 build: protocol minor 6, with READ_CONFIG, WRITE_CONFIG and
    /// COMMIT_CONFIG. The saved blob comes from [`SimChannel::config_blob`].
    pub fn with_config(mut self) -> Self {
        self.minor = self.minor.max(proto::CONFIG_MINOR);
        self.caps = Caps(self.caps.0 | Caps::CONFIG);
        self.config = Some(SimConfig::new(self.config_blob()));
        self
    }

    /// A `Config` message as the firmware saves it, from the strings, the
    /// saved mode and profile, and the saved slots: `boardVersion` (1),
    /// `gamepadOptions` (2), `boardConfig` (12) and `gpioMappings` (13),
    /// then `profileOptions` (11) with the other slots.
    pub fn config_blob(&self) -> Vec<u8> {
        let text = |id| {
            pb::Value::Bytes(
                self.strings
                    .get(&id)
                    .map(|s| s.as_bytes().to_vec())
                    .unwrap_or_default(),
            )
        };
        let mapping = |s: &SimSlot| {
            let mut fields: Vec<pb::Field> = s
                .pins
                .iter()
                .map(|p| {
                    let pin = pb::encode(&[
                        (1, pb::Value::int32(p.action.into())),
                        (2, pb::Value::Varint(p.button_mask.into())),
                        (3, pb::Value::Varint(p.dpad_mask.into())),
                    ]);
                    (1, pb::Value::Bytes(pin))
                })
                .collect();
            fields.push((2, pb::Value::Bytes(s.label.as_bytes().to_vec())));
            fields.push((3, pb::Value::Varint(u64::from(s.enabled))));
            pb::Value::Bytes(pb::encode(&fields))
        };
        let gamepad = pb::encode(&[
            (1, pb::Value::Varint(self.saved_mode.into())),
            (9, pb::Value::Varint(self.saved_profile.into())),
        ]);
        let mut fields = vec![
            (1, text(proto::string::VERSION)),
            (2, pb::Value::Bytes(gamepad)),
            (12, text(proto::string::BOARD)),
        ];
        if let Some(s) = self.saved_slots.get(&1) {
            fields.push((13, mapping(s)));
        }
        let alts: Vec<pb::Field> = (2..=6)
            .filter_map(|n| self.saved_slots.get(&n))
            .map(|s| (2, mapping(s)))
            .collect();
        fields.push((11, pb::Value::Bytes(pb::encode(&alts))));
        pb::encode(&fields)
    }

    /// WRITE_CONFIG, with the firmware's checks in its order (§3.14).
    fn write_config(&mut self, args: &[u8]) -> Result<(), Outcome> {
        if args.len() <= proto::CONFIG_CHUNK_HEAD_LEN {
            return Err(Outcome::BadLength);
        }
        if matches!(self.forced_setup, 2 | 3) {
            return Err(Outcome::Locked);
        }
        let c = self.config.as_mut().ok_or(Outcome::Unsupported)?;
        let offset = usize::from(u16::from_le_bytes([args[0], args[1]]));
        let data = &args[proto::CONFIG_CHUNK_HEAD_LEN..];
        if args[2..4] != [0, 0] || offset + data.len() > c.free() {
            return Err(Outcome::BadArg);
        }
        if c.upload.len() < offset + data.len() {
            c.upload.resize(offset + data.len(), 0);
        }
        c.upload[offset..offset + data.len()].copy_from_slice(data);
        c.chunks += 1;
        let save = match &mut c.save_after_chunks {
            Some(0) | None => false,
            Some(n) => {
                *n -= 1;
                *n == 0
            }
        };
        if save {
            c.save_after_chunks = None;
            self.save_all(false);
        }
        Ok(())
    }

    /// COMMIT_CONFIG, with the firmware's checks in its order (§3.14). On
    /// OK the blob is the saved config, and the pad restarts into gamepad
    /// mode.
    fn commit_config(&mut self, args: &[u8]) -> Result<Option<Effect>, Outcome> {
        if args.len() < proto::CONFIG_COMMIT_LEN {
            return Err(Outcome::BadLength);
        }
        if matches!(self.forced_setup, 2 | 3) {
            return Err(Outcome::Locked);
        }
        let c = self.config.as_mut().ok_or(Outcome::Unsupported)?;
        let size = u32::from_le_bytes([args[0], args[1], args[2], args[3]]) as usize;
        let crc = u32::from_le_bytes([args[4], args[5], args[6], args[7]]);
        if size == 0 || size > c.free() {
            return Err(Outcome::BadArg);
        }
        let blob = c.uploaded(size);
        if proto::crc32(&blob) != crc || pb::decode(&blob).is_none() {
            return Err(Outcome::BadArg);
        }
        c.saved = blob;
        c.upload.clear();
        c.commits += 1;
        Ok(Some(Effect::Reboot(BootMode::Gamepad)))
    }

    /// A v1.8 build: protocol minor 8, with READ_LOG. The log starts at
    /// this boot (a power-on).
    pub fn with_log(mut self) -> Self {
        self.minor = self.minor.max(proto::LOG_MINOR);
        self.caps = Caps(self.caps.0 | Caps::LOG);
        self.power_on();
        self
    }

    /// Writes a log record now, when the build has the log.
    pub fn add_log(&mut self, code: u8, data: &[u8]) {
        let ms = self.uptime_ms;
        if let Some(l) = &mut self.events {
            l.add(ms, code, data);
        }
    }

    /// The BOOT record of a boot.
    fn log_boot(&mut self, flags: u8, mode: u8) {
        let version = self
            .strings
            .get(&proto::string::VERSION)
            .map(|v| v.as_bytes().to_vec())
            .unwrap_or_default();
        let mut data = vec![flags, mode];
        data.extend(version.iter().take(proto::LOG_DATA_LEN - 2));
        if let Some(l) = &mut self.events {
            l.add(3, log_code::BOOT, &data);
        }
    }

    /// A power-on: a new log, from boot 0 and sequence number 0. Only a
    /// v1.8 build has one.
    pub fn power_on(&mut self) {
        if !self.caps.has(Caps::LOG) {
            return;
        }
        self.events = Some(SimLog::default());
        self.log_boot(0, 0);
    }

    /// The firmware panics with `text`: a PANIC record, and a restart that
    /// keeps the log (a watchdog reset, into the default boot mode).
    pub fn panic(&mut self, text: &str) {
        self.add_log(log_code::PANIC, text.as_bytes());
        if let Some(l) = &mut self.events {
            l.restart_mode = 0;
        }
        let running = match self.running {
            Some(_) => InputMode::from_proto(self.saved_mode.into()),
            None => None,
        };
        let token = self.token.rotate_left(8) ^ 0x0101_0101;
        self.boot(running, token);
    }

    /// The firmware's record of an OUT command: the result in STATUS, and
    /// a COMMAND record.
    fn set_last(&mut self, seq: u16, cmd: u8, result: Outcome) {
        self.last = (seq, cmd, result.byte());
        let [lo, hi] = seq.to_le_bytes();
        self.add_log(log_code::COMMAND, &[cmd, result.byte(), lo, hi]);
    }

    /// The active profile changes: a PROFILE record when it is new.
    fn select_profile(&mut self, n: u8) {
        if self.active != n {
            self.add_log(log_code::PROFILE, &[self.active, n]);
            self.active = n;
        }
    }

    /// The `AnimationProfile` of lights profile `n` as READ_LIGHTS gives
    /// it: the fields, with `bEnabled`, the effects and the cycle times
    /// from `lights_limits`.
    pub fn lights_profile(&self, n: u8) -> LightsData {
        let i = usize::from(n);
        let p = self.lights_limits.profiles[i];
        let mut fields = self.lights_fields[i].clone();
        fields.insert(1, pb::Value::Varint(u64::from(p.enabled)));
        for (number, v) in [
            (2, p.idle_effect),
            (3, p.pressed_effect),
            (4, p.idle_cycle_time),
            (10, p.case_effect),
            (14, p.pressed_cycle_time),
            (16, p.case_cycle_time),
        ] {
            fields.insert(number, pb::Value::Varint(u64::from(v)));
        }
        let fields: Vec<pb::Field> = fields.into_iter().collect();
        let flag = |on: bool, bit: u8| if on { bit } else { 0 };
        LightsData {
            profile: n,
            flags: flag(p.enabled, Slot::ENABLED)
                | flag(
                    i8::try_from(n).ok() == Some(self.lights.profile),
                    Slot::ACTIVE,
                ),
            protobuf: pb::encode(&fields),
        }
    }

    /// WRITE_LIGHTS, with the firmware's checks in its order (§3.11).
    fn write_lights(&mut self, args: &[u8]) -> Result<(), Outcome> {
        if args.len() < proto::LIGHTS_HEAD_LEN {
            return Err(Outcome::BadLength);
        }
        if !self.caps.has(Caps::LIGHTS_PROFILES) {
            return Err(Outcome::Unsupported);
        }
        let (n, flags) = (args[0], args[1]);
        if n > 3 || flags & !Slot::FLAGS != 0 || args[2..4] != [0, 0] {
            return Err(Outcome::BadArg);
        }
        if self.lights_limits.preview {
            return Err(Outcome::Busy);
        }
        let enable = flags & Slot::ENABLED != 0;
        let running = i8::try_from(n).ok() == Some(self.lights.profile);
        if !enable && (flags & Slot::ACTIVE != 0 || running) {
            return Err(Outcome::ProfileDisabled);
        }
        let sent = LightsData {
            profile: n,
            flags,
            protobuf: args[proto::LIGHTS_HEAD_LEN..].to_vec(),
        };
        let file = LightsFile::from_data(&sent, None).ok_or(Outcome::BadArg)?;
        file.check().map_err(|_| Outcome::BadArg)?;
        if self.saved_lights_profiles.is_none() {
            self.saved_lights_profiles =
                Some((self.lights_fields.clone(), self.lights_limits.profiles));
        }
        let i = usize::from(n);
        let byte = |number, old: u8| {
            file.values
                .get(&number)
                .and_then(pb::Value::as_u32)
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(old)
        };
        let p = &mut self.lights_limits.profiles[i];
        *p = SimLightsProfile {
            enabled: enable,
            idle_effect: byte(2, p.idle_effect),
            pressed_effect: byte(3, p.pressed_effect),
            idle_cycle_time: byte(4, p.idle_cycle_time),
            case_effect: byte(10, p.case_effect),
            pressed_cycle_time: byte(14, p.pressed_cycle_time),
            case_cycle_time: byte(16, p.case_cycle_time),
        };
        self.lights_fields[i].extend(file.values);
        if flags & Slot::ACTIVE != 0 || running {
            self.lights.profile = n as i8;
        }
        self.lights_pending = self.lights_limits.pending_reads;
        if flags & Slot::SAVE != 0 {
            self.save_all(false);
            self.saved_profile = self.active;
        }
        Ok(())
    }

    /// Main's lights from the mock: `getAnimationProtoOptions` and
    /// `getLedOptions`. A v0.7.12 mock has neither, and nothing changes.
    fn lights_from(&mut self, mock: &Mock) {
        let int = |o: &Value, k: &str| o.get(k).and_then(Value::as_i64);
        let byte = |o: &Value, k: &str| int(o, k).and_then(|v| u8::try_from(v).ok());
        if let Some(a) = mock
            .current("getAnimationProtoOptions")
            .and_then(|a| a.get("AnimationOptions").cloned())
        {
            let k = &mut self.lights_limits;
            if let Some(v) = int(&a, "baseProfileIndex").and_then(|v| i8::try_from(v).ok()) {
                self.saved_lights.profile = v;
            }
            if let Some(v) = byte(&a, "brightness") {
                self.saved_lights.brightness = v;
            }
            if let Some(v) = int(&a, "idletimeout").and_then(|v| u16::try_from(v).ok()) {
                k.idle_timeout_s = v;
            }
            let profiles = a.get("profiles").and_then(Value::as_array);
            for (fields, json) in self
                .lights_fields
                .iter_mut()
                .zip(profiles.into_iter().flatten())
            {
                *fields = animation_fields(json);
            }
            for (p, json) in k.profiles.iter_mut().zip(profiles.into_iter().flatten()) {
                // The getter adds 1 to each cycle time.
                let cycle = |key| byte(json, key).map_or(0, |v| v.saturating_sub(1));
                *p = SimLightsProfile {
                    enabled: int(json, "bEnabled").is_some_and(|v| v != 0)
                        || json.get("bEnabled").and_then(Value::as_bool) == Some(true),
                    idle_effect: byte(json, "baseNonPressedEffect").unwrap_or(0),
                    pressed_effect: byte(json, "basePressedEffect").unwrap_or(0),
                    case_effect: byte(json, "baseCaseEffect").unwrap_or(0),
                    idle_cycle_time: cycle("baseCycleTime"),
                    pressed_cycle_time: cycle("basePressedCycleTime"),
                    case_cycle_time: cycle("baseCaseCycleTime"),
                };
            }
            self.lights = self.saved_lights;
            if let Some(o) = mock.current("getLedOptions") {
                if let Some(pin) = int(&o, "dataPin") {
                    self.lights_limits.running = pin >= 0;
                }
                // Main gives a percentage.
                if let Some(pct) = o.get("brightnessMaximum").and_then(Value::as_f64) {
                    self.lights_limits.brightness_maximum =
                        (pct.clamp(0.0, 100.0) as f32 * 2.55f32 + 0.5) as u8;
                }
            }
        }
    }

    /// LIGHTS_STATUS as main would answer it now.
    pub fn lights_status(&self) -> LightsStatus {
        let k = &self.lights_limits;
        let flag = |on: bool, bit: u8| if on { bit } else { 0 };
        let p = usize::try_from(self.lights.profile)
            .ok()
            .and_then(|i| k.profiles.get(i));
        let or_none = |f: fn(&SimLightsProfile) -> u8| p.map_or(LightsStatus::NONE, f);
        LightsStatus {
            flags: LightsStatus::RUNNING
                | flag(self.lights != self.saved_lights, LightsStatus::UNSAVED)
                | flag(k.preview, LightsStatus::PREVIEW)
                | flag(self.lights_pending > 0, LightsStatus::PENDING),
            profile: self.lights.profile,
            enabled_profiles: k
                .profiles
                .iter()
                .zip(0..)
                .filter(|(p, _)| p.enabled)
                .fold(0, |m, (_, n)| m | 1 << n),
            profiles: 4,
            brightness: self.lights.brightness,
            brightness_steps: k.brightness_steps,
            brightness_maximum: k.brightness_maximum,
            idle_effect: or_none(|p| p.idle_effect),
            pressed_effect: or_none(|p| p.pressed_effect),
            case_effect: or_none(|p| p.case_effect),
            idle_cycle_time: or_none(|p| p.idle_cycle_time),
            pressed_cycle_time: or_none(|p| p.pressed_cycle_time),
            case_cycle_time: or_none(|p| p.case_cycle_time),
            idle_timeout_s: k.idle_timeout_s,
        }
    }

    /// Any save writes the whole config: the LEDs', lights' and slots' RAM
    /// state too. `force` goes into the SAVE record.
    fn save_all(&mut self, force: bool) {
        self.add_log(log_code::SAVE, &[u8::from(force), 0]);
        if let Some(c) = &mut self.config {
            c.save();
        }
        self.saved_leds = self.leds;
        self.saved_lights = self.lights;
        self.saved_slots = self.slots.clone();
        self.saved_lights_profiles = None;
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
        self.lights = self.saved_lights;
        // Only the slots a RAM-only write changed go back.
        let numbers: BTreeSet<u8> = self
            .slots
            .keys()
            .chain(self.saved_slots.keys())
            .copied()
            .filter(|n| self.slots.get(n) != self.saved_slots.get(n))
            .collect();
        self.slots = self.saved_slots.clone();
        self.sync_slots(numbers);
        if let Some((fields, profiles)) = self.saved_lights_profiles.take() {
            self.lights_fields = fields;
            self.lights_limits.profiles = profiles;
        }
        self.pending = false;
        self.reboot_pending = false;
        self.stop_scan();
        // The cache comes from flash again: the upload is gone.
        if let Some(c) = &mut self.config {
            c.upload.clear();
        }
        self.last = (0, 0, 0);
        self.uptime_ms = 1000;
        // The boot ROM writes no record; the firmware does at its next boot.
        let mode = match self.running {
            Some(255) => 2,
            Some(254) | None => return,
            Some(_) => self.events.as_ref().map_or(0, |l| l.restart_mode),
        };
        if let Some(l) = &mut self.events {
            l.boot = l.boot.wrapping_add(1);
            l.restart_mode = 0;
        }
        self.log_boot(3, mode);
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
            // Only main knows the lights commands; without the LEDs,
            // LIGHTS_STATUS STALLs and SET_LIGHTS is UNSUPPORTED (§3.8).
            Some(Command::LightsStatus | Command::SetLights) if !self.main => Some(None),
            Some(Command::LightsStatus) if !self.caps.has(Caps::LIGHTS) => Some(None),
            // Before 1.4 there are no slot commands.
            Some(Command::ReadSlot | Command::WriteSlot) if !self.caps.has(Caps::SLOTS) => {
                Some(None)
            }
            // Before 1.5 there are no lights profile commands.
            Some(Command::ReadLights | Command::WriteLights)
                if !self.main || self.minor < proto::LIGHTS_PROFILES_MINOR =>
            {
                Some(None)
            }
            // Before 1.7 there are no input commands.
            Some(Command::ReadInput | Command::PinScan) if !self.caps.has(Caps::INPUT) => {
                Some(None)
            }
            // Before 1.8 there is no READ_LOG.
            Some(Command::ReadLog) if !self.caps.has(Caps::LOG) => Some(None),
            // Before 1.6 there are no config commands.
            Some(Command::ReadConfig | Command::WriteConfig | Command::CommitConfig)
                if !self.caps.has(Caps::CONFIG) || self.config.is_none() =>
            {
                Some(None)
            }
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
        let Some(cmd) = cmd.filter(|c| len <= c.max_len()) else {
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
            Command::LightsStatus => {
                let l = self.lights_status();
                self.lights_pending = self.lights_pending.saturating_sub(1);
                l.encode().to_vec()
            }
            Command::ReadSlot => match u8::try_from(setup.value) {
                Ok(n @ 1..=6) => self.slot(n).encode(),
                _ => return Err(UsbError::Stall),
            },
            // Without the LEDs, buildLights fails: a STALL.
            Command::ReadLights => match u8::try_from(setup.value) {
                Ok(n @ 0..=3) if self.caps.has(Caps::LIGHTS_PROFILES) => {
                    let d = self.lights_profile(n);
                    let mut b = vec![d.profile, d.flags];
                    b.extend_from_slice(&(d.protobuf.len() as u16).to_le_bytes());
                    b.extend_from_slice(&d.protobuf);
                    b
                }
                _ => return Err(UsbError::Stall),
            },
            Command::ReadInput => self.read_input().encode().to_vec(),
            // A wLength shorter than the head STALLs.
            Command::ReadLog => match &self.events {
                Some(l) if usize::from(len) >= proto::LOG_HEAD_LEN => {
                    l.page(setup.value, len).encode()
                }
                _ => return Err(UsbError::Stall),
            },
            // buildConfig: an offset after the end STALLs; one at the end
            // gives 0 bytes. A reply is at most the buffer.
            Command::ReadConfig => {
                let Some(c) = &mut self.config else {
                    return Err(UsbError::Stall);
                };
                let image = c.image();
                let offset = usize::from(setup.value);
                if offset > image.len() {
                    return Err(UsbError::Stall);
                }
                let n = usize::from(len).min(BUFFER_LEN).min(image.len() - offset);
                let reply = image[offset..offset + n].to_vec();
                c.reads += 1;
                if c.save_every_reads
                    .is_some_and(|k| c.reads.is_multiple_of(k))
                {
                    c.save();
                }
                if let Some(left) = &mut c.save_after_reads {
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        c.save_after_reads = None;
                        c.save();
                    }
                }
                reply
            }
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
            self.set_last(0, byte, Outcome::Unsupported);
            return Err(UsbError::Stall);
        };
        if data.len() < proto::HEADER_LEN || data.len() > usize::from(cmd.max_len()) {
            self.set_last(0, byte, Outcome::BadLength);
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
        if let Ok(Some(Effect::Reboot(mode))) = result {
            self.reboot_pending = true;
            let mode = match mode {
                BootMode::Gamepad => 1,
                BootMode::Webconfig => 2,
                BootMode::Bootsel => 3,
            };
            self.add_log(log_code::RESTART, &[mode]);
            if let Some(l) = &mut self.events {
                l.restart_mode = mode;
            }
        }
        let code = match &result {
            Ok(_) => Outcome::Ok,
            Err(o) => *o,
        };
        self.set_last(seq, cmd as u8, code);
        result.map_err(|_| UsbError::Stall)
    }

    fn apply(&mut self, cmd: Command, args: &[u8]) -> Result<Option<Effect>, Outcome> {
        if cmd == Command::WriteSlot {
            return self.write_slot(args).map(|()| None);
        }
        if cmd == Command::WriteLights {
            return self.write_lights(args).map(|()| None);
        }
        if cmd == Command::WriteConfig {
            return self.write_config(args).map(|()| None);
        }
        if cmd == Command::CommitConfig {
            return self.commit_config(args);
        }
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
                    self.select_profile(n);
                    self.pending = !in_webconfig;
                }
                if persist == 1 {
                    self.saved_profile = n;
                    self.save_all(true);
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
                self.save_all(true);
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
                    self.save_all(true);
                    // The save writes the profile too.
                    self.saved_profile = self.active;
                }
                Ok(None)
            }
            // doPinScan: on again keeps the scan on for 10 s more.
            Command::PinScan => {
                match args[0] {
                    0 => self.stop_scan(),
                    1 => {
                        if !self.input.scan_on {
                            let functions = self.input.functions;
                            self.input.scan_pins = self
                                .functional_actions()
                                .iter()
                                .zip(0u8..)
                                .filter(|(a, gpio)| **a == -10 && functions & (1 << gpio) == 0)
                                .fold(0, |m, (_, gpio)| m | 1 << gpio);
                            self.input.scan_on = true;
                        }
                        self.input.scan_deadline_ms = self.uptime_ms + SCAN_TIMEOUT_MS;
                    }
                    _ => return Err(Outcome::BadArg),
                }
                Ok(None)
            }
            Command::SetLights => {
                if !self.caps.has(Caps::LIGHTS) {
                    return Err(Outcome::Unsupported);
                }
                let set = SetLights::parse(args).ok_or(Outcome::BadArg)?;
                if self.lights_limits.preview {
                    return Err(Outcome::Busy);
                }
                let k = self.lights_limits;
                let profile_ok = set.profile.is_none_or(|p| {
                    p == -1
                        || usize::try_from(p)
                            .ok()
                            .and_then(|i| k.profiles.get(i))
                            .is_some_and(|p| p.enabled)
                });
                if !profile_ok || set.brightness.is_some_and(|b| b > k.brightness_steps) {
                    return Err(Outcome::BadArg);
                }
                if let Some(p) = set.profile {
                    self.lights.profile = p;
                }
                if let Some(b) = set.brightness {
                    self.lights.brightness = b;
                }
                self.lights_pending = k.pending_reads;
                if set.persist {
                    self.save_all(false);
                    self.saved_profile = self.active;
                }
                Ok(None)
            }
            _ => Err(Outcome::BadArg),
        }
    }
}

impl SimChannel {
    /// WRITE_SLOT, as `doWriteSlot` does it: every check before any change.
    fn write_slot(&mut self, args: &[u8]) -> Result<(), Outcome> {
        let count = usize::from(self.pin_count);
        if args.len() != Slot::len(count) {
            return Err(Outcome::BadLength);
        }
        let (profile, flags) = (args[0], args[1]);
        if !(2..=6).contains(&profile)
            || flags & !Slot::FLAGS != 0
            || args[2] != self.pin_count
            || usize::from(args[3]) != proto::SLOT_PIN_LEN
        {
            return Err(Outcome::BadArg);
        }
        let slot = Slot::parse(args).ok_or(Outcome::BadArg)?;
        if slot
            .pins
            .iter()
            .any(|p| !(proto::ACTION_MIN..=proto::ACTION_MAX).contains(&p.action))
        {
            return Err(Outcome::BadArg);
        }
        let (enable, select) = (slot.enabled(), slot.active());
        if !enable && (select || self.active == profile) {
            return Err(Outcome::ProfileDisabled);
        }
        // Configs from before profile 6 have only 4 slots: add the missing
        // ones, empty and disabled.
        for n in 2..profile {
            self.slots.entry(n).or_insert_with(|| SimSlot {
                pins: vec![SlotPin::default(); count],
                ..SimSlot::default()
            });
        }
        // Pins that profile 1 reserves or gives to an add-on keep its action.
        let base = self
            .slots
            .get(&1)
            .map(|s| s.pins.clone())
            .unwrap_or_default();
        let pins = slot
            .pins
            .iter()
            .enumerate()
            .map(|(gpio, p)| match base.get(gpio) {
                Some(b) if matches!(b.action, -5 | 0) => SlotPin {
                    action: b.action,
                    ..SlotPin::default()
                },
                _ => *p,
            })
            .collect();
        self.slots.insert(
            profile,
            SimSlot {
                label: slot.label.clone(),
                enabled: enable,
                pins,
            },
        );
        self.sync_slots([profile]);
        // The reinit runs on the next pass of the main loop.
        if select || self.active == profile {
            self.select_profile(profile);
            self.pending = self.running != Some(255);
        }
        if flags & Slot::SAVE != 0 {
            self.save_all(true);
            self.saved_profile = self.active;
        }
        Ok(())
    }
}

/// A pad with only profile 1, every pin NONE.
fn default_slots() -> BTreeMap<u8, SimSlot> {
    let pins = vec![
        SlotPin {
            action: -10,
            ..SlotPin::default()
        };
        usize::from(RP2040_PINS)
    ];
    BTreeMap::from([(
        1,
        SimSlot {
            label: "Default".into(),
            enabled: true,
            pins,
        },
    )])
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
    sim: Arc<Mutex<SimChannel>>,
    node: String,
    no_access: bool,
}

impl Standalone {
    pub fn new(sim: SimChannel, node: impl Into<String>) -> Self {
        Self::shared(Arc::new(Mutex::new(sim)), node)
    }

    /// A transport on a simulated pad that outlives it, so that a later
    /// open sees the changes of an earlier one (as on a real pad).
    pub fn shared(sim: Arc<Mutex<SimChannel>>, node: impl Into<String>) -> Self {
        Self {
            sim,
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

/// The fields of one profile of `getAnimationProtoOptions`, as
/// `AnimationProfile` has them: the getter adds 1 to the cycle times and
/// splits `effectContextParam`.
fn animation_fields(json: &Value) -> BTreeMap<u32, pb::Value> {
    let int = |k: &str| json.get(k).and_then(Value::as_u64);
    let mut f = BTreeMap::new();
    for (number, key) in [
        (2, "baseNonPressedEffect"),
        (3, "basePressedEffect"),
        (7, "buttonPressHoldTimeInMs"),
        (8, "buttonPressFadeOutTimeInMs"),
        (9, "nonPressedSpecialColor"),
        (10, "baseCaseEffect"),
        (12, "pressedSpecialColor"),
        (15, "bUseCaseLightsInPressedAnimations"),
        (17, "bNonPressedSpecialColorIsRainbow"),
        (18, "bPressedSpecialColorIsRainbow"),
        (19, "bCaseSpecialColorIsRainbow"),
        (20, "caseSpecialColor"),
    ] {
        if let Some(v) = int(key) {
            f.insert(number, pb::Value::Varint(v));
        }
    }
    for (number, key) in [
        (4, "baseCycleTime"),
        (14, "basePressedCycleTime"),
        (16, "baseCaseCycleTime"),
    ] {
        if let Some(v) = int(key) {
            f.insert(number, pb::Value::Varint(v.saturating_sub(1)));
        }
    }
    for (number, key) in [
        (5, "notPressedStaticColors"),
        (6, "pressedStaticColors"),
        (11, "nonButtonStaticColors"),
    ] {
        if let Some(list) = json.get(key).and_then(Value::as_array) {
            let bytes = list
                .iter()
                .filter_map(Value::as_u64)
                .map(|b| b as u8)
                .collect();
            f.insert(number, pb::Value::Bytes(bytes));
        }
    }
    let param = [
        "nonPressedContextParam",
        "pressedContextParam",
        "caseContextParam",
    ]
    .iter()
    .zip([0, 8, 16])
    .filter_map(|(k, shift)| int(k).map(|v| (v & 0xff) << shift))
    .fold(None, |a: Option<u64>, v| Some(a.unwrap_or(0) | v));
    if let Some(p) = param {
        f.insert(21, pb::Value::Varint(p));
    }
    f
}
