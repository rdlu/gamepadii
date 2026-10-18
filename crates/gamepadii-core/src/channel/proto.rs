//! Wire format v1 of the control channel (`docs/design/control-channel.md`
//! §3, the v1.1 LED commands of §3.7, the v1.2 lights commands of §3.8,
//! the v1.3 board ID of §3.9, the v1.4 profile slots of §3.10, the v1.5
//! lights profiles of §3.11, the v1.7 input snapshot of §3.12 and the
//! v1.8 log of §3.13):
//! vendor control requests on EP0, `bRequest` `0x47` (`'G'`), the
//! command in `wIndex` behind the magic high byte `0x50` (`'P'`). Pure
//! encoding and decoding, shared by the host client and the simulator.

use serde::{Serialize, Serializer};

use crate::mode::InputMode;

/// `bmRequestType` of a read: device-to-host, vendor, device recipient.
pub const TYPE_IN: u8 = 0xc0;
/// `bmRequestType` of a command: host-to-device, vendor, device recipient.
pub const TYPE_OUT: u8 = 0x40;
/// `bRequest` (`'G'`).
pub const REQUEST: u8 = 0x47;
/// The high byte of `wIndex` (`'P'`); the low byte is the command.
pub const INDEX_MAGIC: u8 = 0x50;
/// One 64-byte EP0 packet: the limit of every request and reply except
/// the slots ([`Command::max_len`]).
pub const MAX_LEN: u16 = 64;
/// The first bytes of a HELLO reply.
pub const HELLO_MAGIC: [u8; 4] = *b"GPii";
/// The protocol major this host speaks; it refuses any other.
pub const MAJOR: u8 = 1;
/// The newest protocol minor this host knows (v1.8: the log). A
/// pad's minor tells which STRING ids it has; what else it can do is in
/// its capabilities.
pub const MINOR: u8 = 8;
/// The first minor with STRING [`string::BOARD_ID`].
pub const BOARD_ID_MINOR: u8 = 3;
/// The first minor with READ_SLOT and WRITE_SLOT ([`Caps::SLOTS`]).
pub const SLOTS_MINOR: u8 = 4;
/// The first minor with READ_LIGHTS and WRITE_LIGHTS
/// ([`Caps::LIGHTS_PROFILES`]).
pub const LIGHTS_PROFILES_MINOR: u8 = 5;
/// The first minor with READ_INPUT and PIN_SCAN ([`Caps::INPUT`]).
pub const INPUT_MINOR: u8 = 7;
/// The first minor with READ_LOG ([`Caps::LOG`]).
pub const LOG_MINOR: u8 = 8;
pub const HELLO_LEN: usize = 20;
pub const STATUS_LEN: usize = 16;
pub const LED_STATUS_LEN: usize = 12;
pub const LIGHTS_STATUS_LEN: usize = 16;
/// The token and sequence header every OUT payload starts with.
pub const HEADER_LEN: usize = 8;

/// A control request's setup packet, minus `wLength` (the transport adds
/// it from the buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

impl Setup {
    /// One of our reads.
    pub fn read(cmd: Command, value: u16) -> Self {
        Self {
            request_type: TYPE_IN,
            request: REQUEST,
            value,
            index: u16::from_be_bytes([INDEX_MAGIC, cmd as u8]),
        }
    }

    /// One of our commands.
    pub fn command(cmd: Command) -> Self {
        Self {
            request_type: TYPE_OUT,
            request: REQUEST,
            value: 0,
            index: u16::from_be_bytes([INDEX_MAGIC, cmd as u8]),
        }
    }

    /// Whether the firmware's `matches()` takes this request: the type is
    /// `0xC0`/`0x40`, the request `0x47` and the `wIndex` magic `0x50`. The
    /// direction check against the command comes after, in the handler.
    pub fn ours(&self) -> bool {
        matches!(self.request_type, TYPE_IN | TYPE_OUT)
            && self.request == REQUEST
            && self.index >> 8 == u16::from(INDEX_MAGIC)
    }

    /// The command byte (`wIndex` low byte).
    pub fn command_byte(&self) -> u8 {
        (self.index & 0xff) as u8
    }

    pub fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }
}

/// The v1 commands, v1.1's LED ones, v1.2's lights ones, v1.4's slot
/// ones, v1.5's lights profile ones, v1.7's input ones and v1.8's
/// READ_LOG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Command {
    Hello = 0x01,
    Status = 0x02,
    String = 0x03,
    /// v1.1, behind [`Caps::LEDS`].
    LedStatus = 0x04,
    /// v1.2, behind [`Caps::LIGHTS`].
    LightsStatus = 0x05,
    /// v1.4, behind [`Caps::SLOTS`]: `wValue` is the profile (1–6).
    ReadSlot = 0x06,
    /// v1.5, behind [`Caps::LIGHTS_PROFILES`]: `wValue` is the lights
    /// profile (0–3).
    ReadLights = 0x07,
    /// v1.7, behind [`Caps::INPUT`]: a snapshot of the inputs, at once.
    ReadInput = 0x09,
    /// v1.8, behind [`Caps::LOG`]: `wValue` is the low 16 bits of the
    /// first sequence number to read.
    ReadLog = 0x0A,
    SetProfile = 0x10,
    Reboot = 0x11,
    SetInputMode = 0x12,
    /// v1.1, behind [`Caps::LEDS`].
    SetLed = 0x13,
    /// v1.2, behind [`Caps::LIGHTS`].
    SetLights = 0x14,
    /// v1.4, behind [`Caps::SLOTS`].
    WriteSlot = 0x15,
    /// v1.5, behind [`Caps::LIGHTS_PROFILES`].
    WriteLights = 0x16,
    /// v1.7, behind [`Caps::INPUT`]: 1 starts the pin scan, 0 stops it.
    PinScan = 0x19,
}

impl Command {
    pub const ALL: [Command; 17] = [
        Command::Hello,
        Command::Status,
        Command::String,
        Command::LedStatus,
        Command::LightsStatus,
        Command::ReadSlot,
        Command::ReadLights,
        Command::ReadInput,
        Command::ReadLog,
        Command::SetProfile,
        Command::Reboot,
        Command::SetInputMode,
        Command::SetLed,
        Command::SetLights,
        Command::WriteSlot,
        Command::WriteLights,
        Command::PinScan,
    ];

    pub fn from_byte(b: u8) -> Option<Command> {
        Command::ALL.into_iter().find(|c| *c as u8 == b)
    }

    /// Reads are IN; commands are OUT.
    pub fn is_in(self) -> bool {
        matches!(
            self,
            Command::Hello
                | Command::Status
                | Command::String
                | Command::LedStatus
                | Command::LightsStatus
                | Command::ReadSlot
                | Command::ReadLights
                | Command::ReadInput
                | Command::ReadLog
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Command::Hello => "HELLO",
            Command::Status => "STATUS",
            Command::String => "STRING",
            Command::SetProfile => "SET_PROFILE",
            Command::Reboot => "REBOOT",
            Command::SetInputMode => "SET_INPUT_MODE",
            Command::LedStatus => "LED_STATUS",
            Command::SetLed => "SET_LED",
            Command::LightsStatus => "LIGHTS_STATUS",
            Command::SetLights => "SET_LIGHTS",
            Command::ReadSlot => "READ_SLOT",
            Command::WriteSlot => "WRITE_SLOT",
            Command::ReadLights => "READ_LIGHTS",
            Command::WriteLights => "WRITE_LIGHTS",
            Command::ReadInput => "READ_INPUT",
            Command::PinScan => "PIN_SCAN",
            Command::ReadLog => "READ_LOG",
        }
    }

    /// The OUT payload length after the header. WRITE_SLOT and
    /// WRITE_LIGHTS have none: the pin count or the protobuf gives it.
    pub fn payload_len(self) -> Option<usize> {
        match self {
            Command::SetProfile | Command::SetInputMode => Some(2),
            Command::Reboot | Command::PinScan => Some(1),
            Command::SetLed => Some(SET_LED_LEN),
            Command::SetLights => Some(SET_LIGHTS_LEN),
            Command::Hello
            | Command::Status
            | Command::String
            | Command::LedStatus
            | Command::LightsStatus
            | Command::ReadSlot
            | Command::WriteSlot
            | Command::ReadLights
            | Command::WriteLights
            | Command::ReadInput
            | Command::ReadLog => None,
        }
    }

    /// The longest data stage the firmware takes in SETUP: one packet, but
    /// a slot (§3.10), a lights profile (§3.11) and the log (§3.13) are
    /// longer.
    pub fn max_len(self) -> u16 {
        match self {
            Command::ReadSlot | Command::ReadLights | Command::ReadLog => u16::MAX,
            Command::WriteLights => (HEADER_LEN + LIGHTS_HEAD_LEN + ANIMATION_PROFILE_MAX) as u16,
            Command::WriteSlot => (HEADER_LEN + Slot::len(SLOT_MAX_PINS)) as u16,
            _ => MAX_LEN,
        }
    }
}

/// STRING ids.
pub mod string {
    /// `GP2040VERSION`: `v0.7.12-5-g85ca0f6`.
    pub const VERSION: u16 = 1;
    /// `GP2040_BOARDCONFIG`: `Haute42COSMOX`.
    pub const BOARD: u16 = 2;
    /// `BOARD_CONFIG_LABEL`: `Haute42 COSMOX`.
    pub const BOARD_LABEL: u16 = 3;
    /// `GP2040CONFIG`: the build type.
    pub const BUILD_TYPE: u16 = 4;
    /// v1.3: the unique ID of the flash chip, 16 upper-case hex digits
    /// (`pico_get_unique_board_id_string`). Older firmware STALLs it.
    pub const BOARD_ID: u16 = 5;

    /// The label of profile `n` (1–6): `0x11`–`0x16`.
    pub fn profile_label(n: u8) -> u16 {
        0x10 + u16::from(n)
    }

    /// v1.1: the firmware's name for animation effect `n`, `0x20 + n`
    /// (optional; empty when it has none).
    pub fn effect_name(n: u8) -> u16 {
        0x20 + u16::from(n)
    }
}

/// The HELLO capability bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Caps(pub u32);

impl Caps {
    pub const STATUS: u32 = 1 << 0;
    pub const STRING: u32 = 1 << 1;
    pub const SET_PROFILE: u32 = 1 << 2;
    /// REBOOT into gamepad mode or web-config.
    pub const REBOOT: u32 = 1 << 3;
    /// REBOOT into BOOTSEL: reserved, never set by our fork (issue #2);
    /// REBOOT(2) is refused with UNSUPPORTED.
    pub const REBOOT_BOOTSEL: u32 = 1 << 4;
    pub const SET_INPUT_MODE: u32 = 1 << 5;
    pub const CFG_READ: u32 = 1 << 6;
    pub const CFG_WRITE: u32 = 1 << 7;
    /// v1.1: LED_STATUS and SET_LED. Set only when the LEDs run
    /// (AnimationStation is enabled on the build and board).
    pub const LEDS: u32 = 1 << 8;
    /// v1.2: LIGHTS_STATUS and SET_LIGHTS, main's lights profiles. Set only
    /// when the LEDs run (`NeoPicoLEDAddon` is set up).
    pub const LIGHTS: u32 = 1 << 9;
    /// v1.4: READ_SLOT and WRITE_SLOT, the pin mappings of a profile.
    pub const SLOTS: u32 = 1 << 10;
    /// v1.5: READ_LIGHTS and WRITE_LIGHTS, the effects and colours of a
    /// lights profile. Set only when the LEDs run.
    pub const LIGHTS_PROFILES: u32 = 1 << 11;
    /// v1.7: READ_INPUT and PIN_SCAN, the input snapshot.
    pub const INPUT: u32 = 1 << 13;
    /// v1.8: READ_LOG, the log of the pad's last 64 events.
    pub const LOG: u32 = 1 << 14;

    const NAMES: [(u32, &'static str); 14] = [
        (Self::STATUS, "status"),
        (Self::STRING, "string"),
        (Self::SET_PROFILE, "set_profile"),
        (Self::REBOOT, "reboot"),
        (Self::REBOOT_BOOTSEL, "reboot_bootsel"),
        (Self::SET_INPUT_MODE, "set_input_mode"),
        (Self::CFG_READ, "cfg_read"),
        (Self::CFG_WRITE, "cfg_write"),
        (Self::LEDS, "leds"),
        (Self::LIGHTS, "lights"),
        (Self::SLOTS, "slots"),
        (Self::LIGHTS_PROFILES, "lights_profiles"),
        (Self::INPUT, "input"),
        (Self::LOG, "log"),
    ];

    pub fn has(self, bit: u32) -> bool {
        self.0 & bit == bit
    }

    /// The known capabilities, in bit order (`status`, `string`, ...);
    /// reserved bits are left out.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .into_iter()
            .filter(|(bit, _)| self.has(*bit))
            .map(|(_, n)| n)
            .collect()
    }
}

impl Serialize for Caps {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.names().serialize(s)
    }
}

/// A HELLO reply that proved itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    pub major: u8,
    pub minor: u8,
    pub caps: Caps,
    /// Random per boot; every command carries it.
    pub token: u32,
    /// Bit n set: this build can boot `InputMode` n (0–31).
    pub modes: u32,
}

/// Why a HELLO reply isn't the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotHello {
    /// Shorter than 20 bytes; 0 is stock XInput.
    Short(usize),
    /// No `GPii` magic: garbage (stock Xbox One) or something else.
    Magic,
    /// The magic, but not our nonce: a stale or replayed buffer.
    Nonce,
    /// A protocol major this gamepadii doesn't speak.
    Major(u8),
}

impl Hello {
    /// Checks a reply against the nonce that was sent.
    pub fn parse(reply: &[u8], nonce: u16) -> Result<Hello, NotHello> {
        if reply.len() < HELLO_LEN {
            return Err(NotHello::Short(reply.len()));
        }
        if reply[0..4] != HELLO_MAGIC {
            return Err(NotHello::Magic);
        }
        if u16_at(reply, 4) != nonce {
            return Err(NotHello::Nonce);
        }
        if reply[6] != MAJOR {
            return Err(NotHello::Major(reply[6]));
        }
        Ok(Hello {
            major: reply[6],
            minor: reply[7],
            caps: Caps(u32_at(reply, 8)),
            token: u32_at(reply, 12),
            modes: u32_at(reply, 16),
        })
    }

    pub fn encode(&self, nonce: u16) -> [u8; HELLO_LEN] {
        let mut b = [0u8; HELLO_LEN];
        b[0..4].copy_from_slice(&HELLO_MAGIC);
        b[4..6].copy_from_slice(&nonce.to_le_bytes());
        b[6] = self.major;
        b[7] = self.minor;
        b[8..12].copy_from_slice(&self.caps.0.to_le_bytes());
        b[12..16].copy_from_slice(&self.token.to_le_bytes());
        b[16..20].copy_from_slice(&self.modes.to_le_bytes());
        b
    }

    /// Whether the build can boot `mode` (from the mode bitmap).
    pub fn boots(&self, mode: InputMode) -> bool {
        mode.proto_value()
            .filter(|v| *v < 32)
            .is_some_and(|v| self.modes & (1 << v) != 0)
    }

    /// Whether the pad has STRING [`string::BOARD_ID`]: protocol 1.3 or
    /// later, with the STRING capability. Never ask an older pad.
    pub fn has_board_id(&self) -> bool {
        self.major == MAJOR && self.minor >= BOARD_ID_MINOR && self.caps.has(Caps::STRING)
    }

    /// Whether the pad has READ_SLOT and WRITE_SLOT: protocol 1.4 or later,
    /// with [`Caps::SLOTS`]. Never send them to an older pad.
    pub fn has_slots(&self) -> bool {
        self.major == MAJOR && self.minor >= SLOTS_MINOR && self.caps.has(Caps::SLOTS)
    }

    /// Whether the pad has READ_LIGHTS and WRITE_LIGHTS: protocol 1.5 or
    /// later, with [`Caps::LIGHTS_PROFILES`]. Never send them to an older
    /// pad.
    pub fn has_lights_profiles(&self) -> bool {
        self.major == MAJOR
            && self.minor >= LIGHTS_PROFILES_MINOR
            && self.caps.has(Caps::LIGHTS_PROFILES)
    }

    /// Whether the pad has READ_INPUT and PIN_SCAN: protocol 1.7 or later,
    /// with [`Caps::INPUT`]. Never send them to an older pad.
    pub fn has_input(&self) -> bool {
        self.major == MAJOR && self.minor >= INPUT_MINOR && self.caps.has(Caps::INPUT)
    }

    /// Whether the pad has READ_LOG: protocol 1.8 or later, with
    /// [`Caps::LOG`]. Never send it to an older pad.
    pub fn has_log(&self) -> bool {
        self.major == MAJOR && self.minor >= LOG_MINOR && self.caps.has(Caps::LOG)
    }

    /// The modes in the bitmap that gamepadii knows.
    pub fn input_modes(&self) -> Vec<InputMode> {
        (0..32)
            .filter(|v| self.modes & (1 << v) != 0)
            .filter_map(InputMode::from_proto)
            .collect()
    }
}

/// A STATUS reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    pub major: u8,
    /// `DriverManager::getInputMode()`; 255 is web-config.
    pub input_mode: u8,
    /// `profileNumber`, 1–6.
    pub profile: u8,
    /// Bit 0 is profile 1 (always set).
    pub enabled_profiles: u8,
    /// `forcedSetupOptions.mode`.
    pub forced_setup: u8,
    /// Bit 0: a profile change is not applied yet.
    pub flags: u8,
    pub last_seq: u16,
    pub last_command: u8,
    pub last_result: u8,
    pub uptime_ms: u32,
}

impl Status {
    pub const PROFILE_PENDING: u8 = 1 << 0;
    /// A REBOOT or SET_INPUT_MODE was accepted and the restart is pending;
    /// every OUT is BUSY until then (§3.6 item 2).
    pub const REBOOT_PENDING: u8 = 1 << 1;

    pub fn parse(reply: &[u8]) -> Option<Status> {
        (reply.len() >= STATUS_LEN).then(|| Status {
            major: reply[0],
            input_mode: reply[1],
            profile: reply[2],
            enabled_profiles: reply[3],
            forced_setup: reply[4],
            flags: reply[5],
            last_seq: u16_at(reply, 6),
            last_command: reply[8],
            last_result: reply[9],
            uptime_ms: u32_at(reply, 12),
        })
    }

    pub fn encode(&self) -> [u8; STATUS_LEN] {
        let mut b = [0u8; STATUS_LEN];
        b[0] = self.major;
        b[1] = self.input_mode;
        b[2] = self.profile;
        b[3] = self.enabled_profiles;
        b[4] = self.forced_setup;
        b[5] = self.flags;
        b[6..8].copy_from_slice(&self.last_seq.to_le_bytes());
        b[8] = self.last_command;
        b[9] = self.last_result;
        b[12..16].copy_from_slice(&self.uptime_ms.to_le_bytes());
        b
    }

    pub fn profile_enabled(&self, n: u8) -> bool {
        (1..=6).contains(&n) && self.enabled_profiles & (1 << (n - 1)) != 0
    }

    /// The enabled profiles' numbers, from 1.
    pub fn enabled(&self) -> Vec<u8> {
        (1..=6).filter(|n| self.profile_enabled(*n)).collect()
    }

    pub fn pending(&self) -> bool {
        self.flags & Self::PROFILE_PENDING != 0
    }

    pub fn reboot_pending(&self) -> bool {
        self.flags & Self::REBOOT_PENDING != 0
    }
}

/// A LED_STATUS reply (v1.1): AnimationStation's state, as the LED
/// hotkeys see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LedStatus {
    /// [`LedStatus::RUNNING`], [`LedStatus::CUSTOM_THEME`],
    /// [`LedStatus::UNSAVED`].
    pub flags: u8,
    /// `baseAnimationIndex` (the `AnimationEffects` enum).
    pub animation: u8,
    /// How many effects this build cycles through: valid animations are
    /// below it.
    pub animations: u8,
    /// `AnimationOptions.brightness`, 0 to `brightness_steps`.
    pub brightness: u8,
    /// `ledOptions.brightnessSteps`.
    pub brightness_steps: u8,
    /// `ledOptions.brightnessMaximum`, 0-255.
    pub brightness_maximum: u8,
    pub static_color: u8,
    pub button_color: u8,
    pub theme: u8,
    /// How many colours (`AnimationColor`) the colour indexes have.
    pub colors: u8,
    /// How many static themes `theme` indexes.
    pub themes: u8,
}

impl LedStatus {
    /// The LEDs run (AnimationStation is active).
    pub const RUNNING: u8 = 1 << 0;
    /// `hasCustomTheme`.
    pub const CUSTOM_THEME: u8 = 1 << 1;
    /// RAM differs from the saved config: an LED change isn't saved.
    pub const UNSAVED: u8 = 1 << 2;

    pub fn parse(reply: &[u8]) -> Option<LedStatus> {
        (reply.len() >= LED_STATUS_LEN).then(|| LedStatus {
            flags: reply[0],
            animation: reply[1],
            animations: reply[2],
            brightness: reply[3],
            brightness_steps: reply[4],
            brightness_maximum: reply[5],
            static_color: reply[6],
            button_color: reply[7],
            theme: reply[8],
            colors: reply[9],
            themes: reply[10],
        })
    }

    pub fn encode(&self) -> [u8; LED_STATUS_LEN] {
        [
            self.flags,
            self.animation,
            self.animations,
            self.brightness,
            self.brightness_steps,
            self.brightness_maximum,
            self.static_color,
            self.button_color,
            self.theme,
            self.colors,
            self.themes,
            0,
        ]
    }

    pub fn running(&self) -> bool {
        self.flags & Self::RUNNING != 0
    }

    pub fn custom_theme(&self) -> bool {
        self.flags & Self::CUSTOM_THEME != 0
    }

    pub fn unsaved(&self) -> bool {
        self.flags & Self::UNSAVED != 0
    }
}

/// SET_LED's arguments, after the header.
pub const SET_LED_LEN: usize = 8;

/// A SET_LED (v1.1): the fields that are `Some` change, the rest stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetLed {
    pub animation: Option<u8>,
    pub brightness: Option<u8>,
    pub static_color: Option<u8>,
    pub button_color: Option<u8>,
    pub theme: Option<u8>,
    /// Save to flash (the LED hotkeys' save path); else RAM only.
    pub persist: bool,
}

impl SetLed {
    pub const ANIMATION: u8 = 1 << 0;
    pub const BRIGHTNESS: u8 = 1 << 1;
    pub const STATIC_COLOR: u8 = 1 << 2;
    pub const BUTTON_COLOR: u8 = 1 << 3;
    pub const THEME: u8 = 1 << 4;
    /// The mask bits v1.1 defines; any other is BAD_ARG.
    pub const MASK: u8 = 0x1f;

    pub fn mask(&self) -> u8 {
        [
            (self.animation, Self::ANIMATION),
            (self.brightness, Self::BRIGHTNESS),
            (self.static_color, Self::STATIC_COLOR),
            (self.button_color, Self::BUTTON_COLOR),
            (self.theme, Self::THEME),
        ]
        .into_iter()
        .filter(|(v, _)| v.is_some())
        .fold(0, |m, (_, bit)| m | bit)
    }

    pub fn encode(&self) -> [u8; SET_LED_LEN] {
        [
            self.mask(),
            self.animation.unwrap_or(0),
            self.brightness.unwrap_or(0),
            self.static_color.unwrap_or(0),
            self.button_color.unwrap_or(0),
            self.theme.unwrap_or(0),
            u8::from(self.persist),
            0,
        ]
    }

    /// The firmware's reading of `args` (at least [`SET_LED_LEN`] bytes):
    /// `None` for unknown mask bits or a persist byte other than 0/1
    /// (BAD_ARG).
    pub fn parse(args: &[u8]) -> Option<SetLed> {
        let (mask, persist) = (args[0], args[6]);
        if mask & !Self::MASK != 0 || persist > 1 {
            return None;
        }
        let field = |bit: u8, at: usize| (mask & bit != 0).then_some(args[at]);
        Some(SetLed {
            animation: field(Self::ANIMATION, 1),
            brightness: field(Self::BRIGHTNESS, 2),
            static_color: field(Self::STATIC_COLOR, 3),
            button_color: field(Self::BUTTON_COLOR, 4),
            theme: field(Self::THEME, 5),
            persist: persist == 1,
        })
    }

    /// Whether it changes nothing (a bare save, with `persist`).
    pub fn is_empty(&self) -> bool {
        self.mask() == 0
    }
}

/// A LIGHTS_STATUS reply (v1.2): main's lights profile and brightness,
/// and the effects of that profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightsStatus {
    /// [`LightsStatus::RUNNING`], [`LightsStatus::UNSAVED`],
    /// [`LightsStatus::PREVIEW`].
    pub flags: u8,
    /// -1 off, or 0 to `profiles - 1` (`GetSavedMode()`).
    pub profile: i8,
    /// Bit n: profile n is enabled (`bEnabled`).
    pub enabled_profiles: u8,
    /// How many profiles there are: 4.
    pub profiles: u8,
    /// `animationOptions.brightness`, 0 to `brightness_steps`.
    pub brightness: u8,
    pub brightness_steps: u8,
    /// `ledOptions.brightnessMaximum`, 0-255.
    pub brightness_maximum: u8,
    /// The effects and cycle times of `profile`; [`LightsStatus::NONE`]
    /// when the lights are off.
    pub idle_effect: u8,
    pub pressed_effect: u8,
    pub case_effect: u8,
    pub idle_cycle_time: u8,
    pub pressed_cycle_time: u8,
    pub case_cycle_time: u8,
    /// Seconds without input before the lights go dark; 0 is never.
    pub idle_timeout_s: u16,
}

impl LightsStatus {
    /// The lights run (always set in a reply).
    pub const RUNNING: u8 = 1 << 0;
    /// The profile or the brightness differs from the saved settings.
    pub const UNSAVED: u8 = 1 << 1;
    /// A web-config preview runs: SET_LIGHTS is BUSY.
    pub const PREVIEW: u8 = 1 << 2;
    /// v1.5: a SET_LIGHTS (or WRITE_LIGHTS) change waits for core 1, for
    /// about 33 to 57 ms. Older firmware never sets it.
    pub const PENDING: u8 = 1 << 3;
    /// An effect or cycle time when the lights are off.
    pub const NONE: u8 = 255;

    pub fn parse(reply: &[u8]) -> Option<LightsStatus> {
        (reply.len() >= LIGHTS_STATUS_LEN).then(|| LightsStatus {
            flags: reply[0],
            profile: reply[1] as i8,
            enabled_profiles: reply[2],
            profiles: reply[3],
            brightness: reply[4],
            brightness_steps: reply[5],
            brightness_maximum: reply[6],
            idle_effect: reply[7],
            pressed_effect: reply[8],
            case_effect: reply[9],
            idle_cycle_time: reply[10],
            pressed_cycle_time: reply[11],
            case_cycle_time: reply[12],
            idle_timeout_s: u16_at(reply, 14),
        })
    }

    pub fn encode(&self) -> [u8; LIGHTS_STATUS_LEN] {
        let mut b = [0u8; LIGHTS_STATUS_LEN];
        b[0] = self.flags;
        b[1] = self.profile as u8;
        b[2] = self.enabled_profiles;
        b[3] = self.profiles;
        b[4] = self.brightness;
        b[5] = self.brightness_steps;
        b[6] = self.brightness_maximum;
        b[7] = self.idle_effect;
        b[8] = self.pressed_effect;
        b[9] = self.case_effect;
        b[10] = self.idle_cycle_time;
        b[11] = self.pressed_cycle_time;
        b[12] = self.case_cycle_time;
        b[14..16].copy_from_slice(&self.idle_timeout_s.to_le_bytes());
        b
    }

    pub fn running(&self) -> bool {
        self.flags & Self::RUNNING != 0
    }

    pub fn unsaved(&self) -> bool {
        self.flags & Self::UNSAVED != 0
    }

    pub fn preview(&self) -> bool {
        self.flags & Self::PREVIEW != 0
    }

    pub fn pending(&self) -> bool {
        self.flags & Self::PENDING != 0
    }

    /// The lights profile, `None` when the lights are off.
    pub fn on(&self) -> Option<u8> {
        u8::try_from(self.profile).ok()
    }

    pub fn profile_enabled(&self, n: u8) -> bool {
        n < self.profiles.min(8) && self.enabled_profiles & (1 << n) != 0
    }
}

/// SET_LIGHTS's arguments, after the header.
pub const SET_LIGHTS_LEN: usize = 4;

/// A SET_LIGHTS (v1.2): the fields that are `Some` change, the rest stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetLights {
    /// -1 off, or an enabled profile from 0.
    pub profile: Option<i8>,
    pub brightness: Option<u8>,
    /// Save to flash; else RAM only.
    pub persist: bool,
}

impl SetLights {
    pub const PROFILE: u8 = 1 << 0;
    pub const BRIGHTNESS: u8 = 1 << 1;
    /// The mask bits v1.2 defines; any other is BAD_ARG.
    pub const MASK: u8 = 0x03;

    pub fn mask(&self) -> u8 {
        let bit = |on: bool, b: u8| if on { b } else { 0 };
        bit(self.profile.is_some(), Self::PROFILE)
            | bit(self.brightness.is_some(), Self::BRIGHTNESS)
    }

    pub fn encode(&self) -> [u8; SET_LIGHTS_LEN] {
        [
            self.mask(),
            self.profile.unwrap_or(0) as u8,
            self.brightness.unwrap_or(0),
            u8::from(self.persist),
        ]
    }

    /// The firmware's reading of `args` (at least [`SET_LIGHTS_LEN`]
    /// bytes): `None` for unknown mask bits or a persist byte other than
    /// 0/1 (BAD_ARG).
    pub fn parse(args: &[u8]) -> Option<SetLights> {
        let (mask, persist) = (args[0], args[3]);
        if mask & !Self::MASK != 0 || persist > 1 {
            return None;
        }
        Some(SetLights {
            profile: (mask & Self::PROFILE != 0).then_some(args[1] as i8),
            brightness: (mask & Self::BRIGHTNESS != 0).then_some(args[2]),
            persist: persist == 1,
        })
    }

    /// Whether it changes nothing (a bare save, with `persist`).
    pub fn is_empty(&self) -> bool {
        self.mask() == 0
    }
}

/// The bytes before the protobuf of a lights profile: profile, flags, and
/// a `u16` (the protobuf length in a READ_LIGHTS reply, 0 in WRITE_LIGHTS).
pub const LIGHTS_HEAD_LEN: usize = 4;
/// `AnimationProfile_size`: the longest encoded `AnimationProfile`.
pub const ANIMATION_PROFILE_MAX: usize = 227;
/// The `wLength` of a READ_LIGHTS.
pub const READ_LIGHTS_LEN: u16 = 512;

/// A READ_LIGHTS reply, or a WRITE_LIGHTS argument (§3.11): one lights
/// profile, its `AnimationProfile` as protobuf ([`crate::pb`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightsData {
    /// 0–3.
    pub profile: u8,
    /// [`Slot::ENABLED`], [`Slot::ACTIVE`], [`Slot::SAVE`]: the same bits.
    pub flags: u8,
    /// The encoded `AnimationProfile`. In a WRITE_LIGHTS, only the fields
    /// to change: the firmware keeps the others.
    pub protobuf: Vec<u8>,
}

impl LightsData {
    pub fn enabled(&self) -> bool {
        self.flags & Slot::ENABLED != 0
    }

    pub fn active(&self) -> bool {
        self.flags & Slot::ACTIVE != 0
    }

    /// A READ_LIGHTS reply: the head gives the protobuf length.
    pub fn parse(b: &[u8]) -> Option<Self> {
        let head = b.get(..LIGHTS_HEAD_LEN)?;
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        let protobuf = b.get(LIGHTS_HEAD_LEN..LIGHTS_HEAD_LEN + len)?.to_vec();
        Some(Self {
            profile: head[0],
            flags: head[1],
            protobuf,
        })
    }

    /// The WRITE_LIGHTS argument: the `u16` is 0.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![self.profile, self.flags, 0, 0];
        b.extend_from_slice(&self.protobuf);
        b
    }
}

/// The bytes of a READ_INPUT reply.
pub const INPUT_LEN: usize = 36;
/// What the axes are at rest (`GAMEPAD_JOYSTICK_MID`).
pub const AXIS_CENTRE: u16 = 0x7fff;

/// A READ_INPUT reply (v1.7, §3.12): the inputs at the time of the read.
/// The pins are raw, before the debounce. The rest is the gamepad state
/// after the active profile, SOCD and the other processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSnapshot {
    /// Bit n: GPIO n is pressed now (a SIO input that is low).
    pub pressed: u64,
    /// Bit n: the snapshot can read GPIO n (it is a SIO input).
    pub readable: u64,
    /// `GamepadState.dpad`: 1 up, 2 down, 4 left, 8 right.
    pub dpad: u8,
    /// [`InputSnapshot::SCAN_ON`].
    pub flags: u8,
    /// `GamepadState.aux`: bit 15 is the Fn button.
    pub aux: u16,
    /// `GamepadState.buttons` (`GAMEPAD_MASK_*`).
    pub buttons: u32,
    pub lx: u16,
    pub ly: u16,
    pub rx: u16,
    pub ry: u16,
    pub lt: u8,
    pub rt: u8,
}

impl Default for InputSnapshot {
    /// Nothing pressed, the sticks at the centre.
    fn default() -> Self {
        Self {
            pressed: 0,
            readable: 0,
            dpad: 0,
            flags: 0,
            aux: 0,
            buttons: 0,
            lx: AXIS_CENTRE,
            ly: AXIS_CENTRE,
            rx: AXIS_CENTRE,
            ry: AXIS_CENTRE,
            lt: 0,
            rt: 0,
        }
    }
}

impl InputSnapshot {
    /// The pin scan is on (PIN_SCAN).
    pub const SCAN_ON: u8 = 1 << 0;

    /// `None` when the reply is shorter than [`INPUT_LEN`].
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < INPUT_LEN {
            return None;
        }
        let u64_at = |at| u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32;
        Some(Self {
            pressed: u64_at(0),
            readable: u64_at(8),
            dpad: b[16],
            flags: b[17],
            aux: u16_at(b, 18),
            buttons: u32_at(b, 20),
            lx: u16_at(b, 24),
            ly: u16_at(b, 26),
            rx: u16_at(b, 28),
            ry: u16_at(b, 30),
            lt: b[32],
            rt: b[33],
        })
    }

    pub fn encode(&self) -> [u8; INPUT_LEN] {
        let mut b = [0u8; INPUT_LEN];
        b[0..8].copy_from_slice(&self.pressed.to_le_bytes());
        b[8..16].copy_from_slice(&self.readable.to_le_bytes());
        b[16] = self.dpad;
        b[17] = self.flags;
        b[18..20].copy_from_slice(&self.aux.to_le_bytes());
        b[20..24].copy_from_slice(&self.buttons.to_le_bytes());
        for (i, v) in [self.lx, self.ly, self.rx, self.ry].into_iter().enumerate() {
            b[24 + 2 * i..26 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[32] = self.lt;
        b[33] = self.rt;
        b
    }

    pub fn scan_on(&self) -> bool {
        self.flags & Self::SCAN_ON != 0
    }

    pub fn is_pressed(&self, gpio: u8) -> bool {
        gpio < 64 && self.pressed & (1 << gpio) != 0
    }

    /// The GPIOs pressed now, from the lowest.
    pub fn pressed_gpios(&self) -> Vec<u8> {
        gpios(self.pressed)
    }

    /// The GPIOs the snapshot can read, from the lowest.
    pub fn readable_gpios(&self) -> Vec<u8> {
        gpios(self.readable)
    }
}

/// The set bits of `mask`, from the lowest.
pub fn gpios(mask: u64) -> Vec<u8> {
    (0..64).filter(|n| mask & (1 << n) != 0).collect()
}

/// The bytes before the pins of a slot.
pub const SLOT_HEAD_LEN: usize = 20;
/// The bytes of each pin: `i16` action, `u32` button mask, `u32` D-pad mask.
pub const SLOT_PIN_LEN: usize = 10;
/// The label bytes: UTF-8, padded with NUL, no NUL needed.
pub const SLOT_LABEL_LEN: usize = 16;
/// The most pins a chip has (`NUM_BANK0_GPIOS`: 30 on RP2040, 48 on
/// RP2350).
pub const SLOT_MAX_PINS: usize = 48;
/// The `wLength` of a READ_SLOT: the host doesn't know the pin count
/// before the reply.
pub const READ_SLOT_LEN: u16 = 512;
/// The `GpioAction` range of main (cb01f703): `NONE` to
/// `SUSTAIN_PROFILE_SLIDER_POSITION_8`. WRITE_SLOT refuses others.
pub const ACTION_MIN: i16 = -10;
pub const ACTION_MAX: i16 = 138;

/// One pin of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SlotPin {
    /// The `GpioAction`.
    pub action: i16,
    /// `customButtonMask`, for `CUSTOM_BUTTON_COMBO`.
    pub button_mask: u32,
    /// `customDpadMask`.
    pub dpad_mask: u32,
}

/// A READ_SLOT reply, or a WRITE_SLOT argument (§3.10): the pin mappings
/// and label of one profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slot {
    /// 1–6.
    pub profile: u8,
    /// [`Slot::ENABLED`], [`Slot::ACTIVE`], [`Slot::SAVE`].
    pub flags: u8,
    /// At most [`SLOT_LABEL_LEN`] bytes of UTF-8.
    pub label: String,
    /// One for each GPIO, from GPIO 0.
    pub pins: Vec<SlotPin>,
}

impl Slot {
    /// The profile is enabled (READ_SLOT), or WRITE_SLOT enables it.
    pub const ENABLED: u8 = 1 << 0;
    /// The profile is the active one, or WRITE_SLOT selects it.
    pub const ACTIVE: u8 = 1 << 1;
    /// WRITE_SLOT only: save to flash.
    pub const SAVE: u8 = 1 << 2;
    pub const FLAGS: u8 = 0x07;

    /// The bytes of a slot with `pins` pins.
    pub const fn len(pins: usize) -> usize {
        SLOT_HEAD_LEN + SLOT_PIN_LEN * pins
    }

    /// A reply: `None` when it is shorter than its head says, or the pins
    /// have a size other than [`SLOT_PIN_LEN`].
    pub fn parse(reply: &[u8]) -> Option<Slot> {
        if reply.len() < SLOT_HEAD_LEN || usize::from(reply[3]) != SLOT_PIN_LEN {
            return None;
        }
        let count = usize::from(reply[2]);
        let body = reply.get(SLOT_HEAD_LEN..Self::len(count))?;
        let label = &reply[4..4 + SLOT_LABEL_LEN];
        let end = label.iter().position(|b| *b == 0).unwrap_or(SLOT_LABEL_LEN);
        Some(Slot {
            profile: reply[0],
            flags: reply[1],
            label: String::from_utf8_lossy(&label[..end]).into_owned(),
            pins: body
                .as_chunks::<SLOT_PIN_LEN>()
                .0
                .iter()
                .map(|p| SlotPin {
                    action: u16_at(p, 0) as i16,
                    button_mask: u32_at(p, 2),
                    dpad_mask: u32_at(p, 6),
                })
                .collect(),
        })
    }

    /// The wire bytes. The label is cut at [`SLOT_LABEL_LEN`] bytes (the
    /// host checks it before, see [`Slot::check`]).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; Self::len(self.pins.len())];
        b[0] = self.profile;
        b[1] = self.flags;
        b[2] = u8::try_from(self.pins.len()).unwrap_or(u8::MAX);
        b[3] = SLOT_PIN_LEN as u8;
        let label = self.label.as_bytes();
        let n = label.len().min(SLOT_LABEL_LEN);
        b[4..4 + n].copy_from_slice(&label[..n]);
        for (i, p) in self.pins.iter().enumerate() {
            let at = SLOT_HEAD_LEN + i * SLOT_PIN_LEN;
            b[at..at + 2].copy_from_slice(&p.action.to_le_bytes());
            b[at + 2..at + 6].copy_from_slice(&p.button_mask.to_le_bytes());
            b[at + 6..at + 10].copy_from_slice(&p.dpad_mask.to_le_bytes());
        }
        b
    }

    pub fn enabled(&self) -> bool {
        self.flags & Self::ENABLED != 0
    }

    pub fn active(&self) -> bool {
        self.flags & Self::ACTIVE != 0
    }

    /// What WRITE_SLOT refuses with BAD_ARG on a pad with `pins` pins, or
    /// PROFILE_DISABLED when `active_now` is the profile the pad runs; the
    /// reason, for the user. The host checks before it sends.
    pub fn check(&self, pins: usize, active_now: u8) -> std::result::Result<(), String> {
        if !(2..=6).contains(&self.profile) {
            return Err(format!(
                "profile {}: only profiles 2 to 6 can be written (profile 1 is the board's own \
                 mapping)",
                self.profile
            ));
        }
        if self.flags & !Self::FLAGS != 0 {
            return Err(format!("unknown flags {:#04x}", self.flags & !Self::FLAGS));
        }
        if self.pins.len() != pins {
            return Err(format!("{} pins, but this pad has {pins}", self.pins.len()));
        }
        if self.label.len() > SLOT_LABEL_LEN {
            return Err(format!(
                "the label {:?} is {} bytes; the pad keeps {SLOT_LABEL_LEN}",
                self.label,
                self.label.len()
            ));
        }
        if let Some((gpio, p)) = self
            .pins
            .iter()
            .enumerate()
            .find(|(_, p)| !(ACTION_MIN..=ACTION_MAX).contains(&p.action))
        {
            return Err(format!(
                "GPIO {gpio}: action {} is not in {ACTION_MIN} to {ACTION_MAX}",
                p.action
            ));
        }
        if !self.enabled() && (self.active() || self.profile == active_now) {
            return Err(format!(
                "profile {} must stay enabled: it is {}",
                self.profile,
                if self.active() {
                    "the one to select"
                } else {
                    "the active profile"
                }
            ));
        }
        Ok(())
    }
}

/// The result of the last command, in STATUS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    BadToken,
    BadLength,
    BadArg,
    /// Forced setup mode forbids it.
    Locked,
    /// The mode isn't in this build.
    Unsupported,
    ProfileDisabled,
    Busy,
    Other(u8),
}

impl Outcome {
    pub fn from_byte(b: u8) -> Outcome {
        match b {
            0 => Outcome::Ok,
            1 => Outcome::BadToken,
            2 => Outcome::BadLength,
            3 => Outcome::BadArg,
            4 => Outcome::Locked,
            5 => Outcome::Unsupported,
            6 => Outcome::ProfileDisabled,
            7 => Outcome::Busy,
            n => Outcome::Other(n),
        }
    }

    pub fn byte(self) -> u8 {
        match self {
            Outcome::Ok => 0,
            Outcome::BadToken => 1,
            Outcome::BadLength => 2,
            Outcome::BadArg => 3,
            Outcome::Locked => 4,
            Outcome::Unsupported => 5,
            Outcome::ProfileDisabled => 6,
            Outcome::Busy => 7,
            Outcome::Other(n) => n,
        }
    }

    /// The name in the spec: `OK`, `BAD_TOKEN`, ...; `None` for a code that
    /// this host doesn't know.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Outcome::Ok => "OK",
            Outcome::BadToken => "BAD_TOKEN",
            Outcome::BadLength => "BAD_LENGTH",
            Outcome::BadArg => "BAD_ARG",
            Outcome::Locked => "LOCKED",
            Outcome::Unsupported => "UNSUPPORTED",
            Outcome::ProfileDisabled => "PROFILE_DISABLED",
            Outcome::Busy => "BUSY",
            Outcome::Other(_) => return None,
        })
    }
}

/// The head of a READ_LOG reply (§3.13).
pub const LOG_HEAD_LEN: usize = 12;
/// The bytes of each log record.
pub const LOG_RECORD_LEN: usize = 32;
/// The data bytes of a record.
pub const LOG_DATA_LEN: usize = 24;
/// The records the pad keeps: older ones are gone.
pub const LOG_RECORDS: u32 = 64;
/// The `wLength` of a READ_LOG. The firmware's buffer is 328 bytes: 9
/// records in each reply.
pub const READ_LOG_LEN: u16 = 1024;

/// The log codes (§3.13).
pub mod log_code {
    pub const BOOT: u8 = 1;
    pub const PANIC: u8 = 2;
    pub const COMMAND: u8 = 3;
    pub const PROFILE: u8 = 4;
    pub const SAVE: u8 = 5;
    pub const RESTART: u8 = 6;
    pub const ERROR: u8 = 7;
    pub const USB_HOST: u8 = 8;

    /// `BOOT`, `PANIC`, ...; `None` for a code that this host doesn't know.
    pub fn name(code: u8) -> Option<&'static str> {
        Some(match code {
            BOOT => "BOOT",
            PANIC => "PANIC",
            COMMAND => "COMMAND",
            PROFILE => "PROFILE",
            SAVE => "SAVE",
            RESTART => "RESTART",
            ERROR => "ERROR",
            USB_HOST => "USB_HOST",
            _ => return None,
        })
    }
}

/// A boot mode in the log: 0 default, 1 gamepad, 2 web-config, 3 BOOTSEL;
/// the firmware gives 255 for any other.
pub fn log_boot_mode(b: u8) -> &'static str {
    match b {
        0 => "default",
        1 => "gamepad",
        2 => "webconfig",
        3 => "bootsel",
        _ => "other",
    }
}

/// One record of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// Its sequence number: the pad counts every record from 0, from the
    /// start of the log.
    pub seq: u32,
    /// Milliseconds since that boot.
    pub ms: u32,
    /// The boot number: 0 at the start of the log, 1 more at each restart
    /// that keeps the log.
    pub boot: u16,
    pub code: u8,
    /// The bytes that the record uses (at most [`LOG_DATA_LEN`]).
    pub data: Vec<u8>,
}

impl LogRecord {
    /// One record of `LOG_RECORD_LEN` bytes, with its sequence number.
    pub fn parse(b: &[u8], seq: u32) -> Option<LogRecord> {
        let b = b.get(..LOG_RECORD_LEN)?;
        let len = usize::from(b[7]).min(LOG_DATA_LEN);
        Some(LogRecord {
            seq,
            ms: u32_at(b, 0),
            boot: u16_at(b, 4),
            code: b[6],
            data: b[8..8 + len].to_vec(),
        })
    }

    /// The wire bytes: the data is cut at [`LOG_DATA_LEN`] bytes, then
    /// zeros.
    pub fn encode(&self) -> [u8; LOG_RECORD_LEN] {
        let mut b = [0u8; LOG_RECORD_LEN];
        let n = self.data.len().min(LOG_DATA_LEN);
        b[0..4].copy_from_slice(&self.ms.to_le_bytes());
        b[4..6].copy_from_slice(&self.boot.to_le_bytes());
        b[6] = self.code;
        b[7] = n as u8;
        b[8..8 + n].copy_from_slice(&self.data[..n]);
        b
    }

    /// What the record says, by its code.
    pub fn event(&self) -> LogEvent {
        LogEvent::decode(self.code, &self.data)
    }
}

/// A log record, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// The pad started. `kept`: the log was kept from the boot before.
    /// `watchdog`: a watchdog reset (a soft reboot or a panic).
    Boot {
        kept: bool,
        watchdog: bool,
        mode: u8,
        version: String,
    },
    /// The firmware panicked, and then restarted: the message, cut at 24
    /// bytes.
    Panic { text: String },
    /// An OUT command, or an OUT that the pad refused in SETUP (`seq` 0).
    Command { cmd: u8, result: Outcome, seq: u16 },
    /// The profile changed. At boot, `old` can be 254.
    Profile { old: u8, new: u8 },
    /// A save of the settings to flash.
    Save { force: bool, restart: bool },
    /// A restart into the boot mode `mode`.
    Restart { mode: u8 },
    /// A system error, as text.
    Error { text: String },
    /// A device on the USB host port: mounted, or removed.
    UsbHost {
        mounted: bool,
        address: u8,
        vid: u16,
        pid: u16,
    },
    /// A code that this host doesn't know, or data that is too short for
    /// its code.
    Unknown { code: u8, data: Vec<u8> },
}

impl LogEvent {
    pub fn decode(code: u8, data: &[u8]) -> LogEvent {
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let need = match code {
            log_code::BOOT | log_code::PROFILE | log_code::SAVE => 2,
            log_code::COMMAND => 4,
            log_code::RESTART => 1,
            log_code::USB_HOST => 6,
            _ => 0,
        };
        if data.len() < need {
            return LogEvent::Unknown {
                code,
                data: data.to_vec(),
            };
        }
        match code {
            log_code::BOOT => LogEvent::Boot {
                kept: data[0] & 1 != 0,
                watchdog: data[0] & 2 != 0,
                mode: data[1],
                version: text(&data[2..]),
            },
            log_code::PANIC => LogEvent::Panic { text: text(data) },
            log_code::COMMAND => LogEvent::Command {
                cmd: data[0],
                result: Outcome::from_byte(data[1]),
                seq: u16_at(data, 2),
            },
            log_code::PROFILE => LogEvent::Profile {
                old: data[0],
                new: data[1],
            },
            log_code::SAVE => LogEvent::Save {
                force: data[0] != 0,
                restart: data[1] != 0,
            },
            log_code::RESTART => LogEvent::Restart { mode: data[0] },
            log_code::ERROR => LogEvent::Error { text: text(data) },
            log_code::USB_HOST => LogEvent::UsbHost {
                mounted: data[0] != 0,
                address: data[1],
                vid: u16_at(data, 2),
                pid: u16_at(data, 4),
            },
            _ => LogEvent::Unknown {
                code,
                data: data.to_vec(),
            },
        }
    }
}

/// A READ_LOG reply (§3.13): the head, and the records from `first`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPage {
    /// The sequence number of the first record in the reply.
    pub first: u32,
    /// The sequence number of the next record that the pad writes.
    pub next: u32,
    /// The boot number now.
    pub boot: u16,
    pub records: Vec<LogRecord>,
}

impl LogPage {
    /// A reply: `None` when the head is short, the record length is not
    /// [`LOG_RECORD_LEN`], or the records don't fit in the reply.
    pub fn parse(b: &[u8]) -> Option<LogPage> {
        let head = b.get(..LOG_HEAD_LEN)?;
        if usize::from(head[10]) != LOG_RECORD_LEN {
            return None;
        }
        let first = u32_at(head, 0);
        let count = usize::from(head[11]);
        let body = b.get(LOG_HEAD_LEN..LOG_HEAD_LEN + count * LOG_RECORD_LEN)?;
        let records = body
            .chunks(LOG_RECORD_LEN)
            .zip(first..)
            .map(|(r, seq)| LogRecord::parse(r, seq))
            .collect::<Option<Vec<_>>>()?;
        Some(LogPage {
            first,
            next: u32_at(head, 4),
            boot: u16_at(head, 8),
            records,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(LOG_HEAD_LEN + self.records.len() * LOG_RECORD_LEN);
        b.extend_from_slice(&self.first.to_le_bytes());
        b.extend_from_slice(&self.next.to_le_bytes());
        b.extend_from_slice(&self.boot.to_le_bytes());
        b.push(LOG_RECORD_LEN as u8);
        b.push(u8::try_from(self.records.len()).unwrap_or(u8::MAX));
        for r in &self.records {
            b.extend_from_slice(&r.encode());
        }
        b
    }

    /// The sequence number after the last record in the reply.
    pub fn end(&self) -> u32 {
        self.first.wrapping_add(self.records.len() as u32)
    }
}

/// An OUT payload: the header, then `args`.
pub fn payload(token: u32, seq: u16, args: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER_LEN + args.len());
    b.extend_from_slice(&token.to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(args);
    b
}

/// The `(token, seq)` of an OUT payload, when it has a header.
pub fn header(data: &[u8]) -> Option<(u32, u16)> {
    (data.len() >= HEADER_LEN).then(|| (u32_at(data, 0), u16_at(data, 4)))
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_packets_match_the_spec() {
        let s = Setup::read(Command::Hello, 0xbeef);
        assert_eq!(
            (s.request_type, s.request, s.value, s.index),
            (0xc0, 0x47, 0xbeef, 0x5001)
        );
        assert!(s.ours() && s.is_in());
        let s = Setup::command(Command::SetInputMode);
        assert_eq!((s.request_type, s.index), (0x40, 0x5012));
        assert!(s.ours() && !s.is_in());
        // xpad's probe and XSM3 are not ours.
        let xpad = Setup {
            request_type: 0xc1,
            request: 0x01,
            value: 0x0100,
            index: 0,
        };
        assert!(!xpad.ours());
        let wrong_magic = Setup {
            index: 0x4f01,
            ..Setup::read(Command::Hello, 1)
        };
        assert!(!wrong_magic.ours());
    }

    #[test]
    fn hello_round_trips_and_proves_itself() {
        let h = Hello {
            major: 1,
            minor: 0,
            caps: Caps(0b10_1111),
            token: 0xdead_beef,
            modes: 0b1111,
        };
        let bytes = h.encode(0x1234);
        assert_eq!(&bytes[..6], b"GPii\x34\x12");
        assert_eq!(Hello::parse(&bytes, 0x1234), Ok(h));
        assert_eq!(Hello::parse(&bytes, 0x1235), Err(NotHello::Nonce));
        assert_eq!(Hello::parse(&[], 1), Err(NotHello::Short(0)));
        assert_eq!(Hello::parse(&[0xa5; 64], 1), Err(NotHello::Magic));
        let mut v2 = bytes;
        v2[6] = 2;
        assert_eq!(Hello::parse(&v2, 0x1234), Err(NotHello::Major(2)));
        // A longer reply (a later minor) is fine.
        let mut long = bytes.to_vec();
        long.extend_from_slice(&[9; 8]);
        assert_eq!(Hello::parse(&long, 0x1234), Ok(h));
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
        assert_eq!(
            h.input_modes(),
            [
                InputMode::Xinput,
                InputMode::Switch,
                InputMode::Ps3,
                InputMode::Keyboard
            ]
        );
        assert!(h.boots(InputMode::Keyboard));
        assert!(!h.boots(InputMode::Webconfig));
        assert!(!h.boots(InputMode::Bootsel));
    }

    #[test]
    fn status_round_trips() {
        let s = Status {
            major: 1,
            input_mode: 3,
            profile: 2,
            enabled_profiles: 0b1_1111,
            forced_setup: 0,
            flags: 1,
            last_seq: 7,
            last_command: 0x10,
            last_result: 6,
            uptime_ms: 123_456,
        };
        assert_eq!(Status::parse(&s.encode()), Some(s));
        assert_eq!(Status::parse(&[0; 15]), None);
        assert_eq!(s.enabled(), [1, 2, 3, 4, 5]);
        assert!(s.pending());
        assert!(!s.profile_enabled(6) && !s.profile_enabled(0));
        assert_eq!(Outcome::from_byte(s.last_result), Outcome::ProfileDisabled);
        for b in 0..=8 {
            assert_eq!(Outcome::from_byte(b).byte(), b);
        }
    }

    #[test]
    fn payload_has_the_header() {
        let p = payload(0x0403_0201, 0x0605, &[2, 1]);
        assert_eq!(p, [1, 2, 3, 4, 5, 6, 0, 0, 2, 1]);
        assert_eq!(header(&p), Some((0x0403_0201, 0x0605)));
        assert_eq!(header(&p[..7]), None);
        assert_eq!(string::profile_label(1), 0x11);
        assert_eq!(string::profile_label(6), 0x16);
        assert_eq!(string::effect_name(0), 0x20);
        assert_eq!(string::effect_name(4), 0x24);
        assert_eq!(string::BOARD_ID, 5);
    }

    #[test]
    fn only_1_3_has_the_board_id() {
        let h = |minor, caps| Hello {
            major: 1,
            minor,
            caps: Caps(caps),
            token: 1,
            modes: 1,
        };
        assert!(h(3, Caps::STRING).has_board_id());
        assert!(h(4, Caps::STRING | Caps::STATUS).has_board_id());
        assert!(!h(2, Caps::STRING).has_board_id());
        assert!(!h(0, Caps::STRING).has_board_id());
        assert!(!h(3, Caps::STATUS).has_board_id());
    }

    #[test]
    fn v1_1_led_commands() {
        let s = Setup::read(Command::LedStatus, 0);
        assert_eq!((s.request_type, s.index), (0xc0, 0x5004));
        assert!(Command::LedStatus.is_in() && !Command::SetLed.is_in());
        assert_eq!(Setup::command(Command::SetLed).index, 0x5013);
        assert_eq!(Command::from_byte(0x13), Some(Command::SetLed));
        assert_eq!(Command::SetLed.payload_len(), Some(8));
        assert_eq!(Caps(0x12f).names().last(), Some(&"leds"));
        assert!(Caps(0x12f).has(Caps::LEDS) && !Caps(0x2f).has(Caps::LEDS));
    }

    #[test]
    fn v1_5_lights_profile_commands() {
        let s = Setup::read(Command::ReadLights, 3);
        assert_eq!((s.request_type, s.index, s.value), (TYPE_IN, 0x5007, 3));
        assert!(Command::ReadLights.is_in() && !Command::WriteLights.is_in());
        assert_eq!(Command::from_byte(0x16), Some(Command::WriteLights));
        assert_eq!(Command::WriteLights.max_len(), 8 + 4 + 227);
        assert_eq!(Caps(0xe2f).names().last(), Some(&"lights_profiles"));
        let hello = |minor, caps| Hello {
            major: 1,
            minor,
            caps: Caps(caps),
            token: 1,
            modes: 0,
        };
        assert!(hello(5, Caps::LIGHTS_PROFILES).has_lights_profiles());
        assert!(!hello(4, Caps::LIGHTS_PROFILES).has_lights_profiles());
        assert!(!hello(5, Caps::SLOTS).has_lights_profiles());
        // The reply's u16 is the protobuf length; bytes after it are not
        // the profile.
        let d = LightsData::parse(&[2, 3, 2, 0, 0x10, 1, 0xee]).unwrap();
        assert_eq!((d.profile, d.enabled(), d.active()), (2, true, true));
        assert_eq!(d.protobuf, [0x10, 1]);
        assert_eq!(LightsData::parse(&[2, 3, 5, 0, 0x10]), None, "short");
        assert_eq!(d.encode(), [2, 3, 0, 0, 0x10, 1]);
    }

    #[test]
    fn v1_7_input_commands() {
        let s = Setup::read(Command::ReadInput, 0);
        assert_eq!((s.request_type, s.index, s.value), (TYPE_IN, 0x5009, 0));
        assert!(Command::ReadInput.is_in() && !Command::PinScan.is_in());
        assert_eq!(Setup::command(Command::PinScan).index, 0x5019);
        assert_eq!(Command::from_byte(0x19), Some(Command::PinScan));
        assert_eq!(Command::from_byte(0x09), Some(Command::ReadInput));
        assert_eq!(Command::PinScan.payload_len(), Some(1));
        assert_eq!(Command::ReadInput.max_len(), 64);
        assert_eq!(Caps(0x2e2f).names().last(), Some(&"input"));
        let hello = |minor, caps| Hello {
            major: 1,
            minor,
            caps: Caps(caps),
            token: 1,
            modes: 0,
        };
        assert!(hello(7, Caps::INPUT).has_input());
        assert!(!hello(6, Caps::INPUT).has_input(), "1.6 never gets them");
        assert!(!hello(7, Caps::LIGHTS_PROFILES).has_input());
    }

    #[test]
    fn an_input_snapshot_round_trips() {
        let i = InputSnapshot {
            pressed: 1 << 27 | 1 << 3 | 1 << 40,
            readable: 0x3fff_ffff,
            dpad: 0x02,
            flags: InputSnapshot::SCAN_ON,
            aux: 0x8000,
            buttons: 0x0201,
            lx: 0,
            ly: 0xffff,
            rx: AXIS_CENTRE,
            ry: 1,
            lt: 255,
            rt: 7,
        };
        let b = i.encode();
        assert_eq!(&b[0..8], &[8, 0, 0, 8, 0, 1, 0, 0]);
        assert_eq!(&b[16..24], &[2, 1, 0, 0x80, 1, 2, 0, 0]);
        assert_eq!(&b[24..36], &[0, 0, 255, 255, 255, 127, 1, 0, 255, 7, 0, 0]);
        assert_eq!(InputSnapshot::parse(&b), Some(i));
        assert_eq!(InputSnapshot::parse(&b[..35]), None, "short");
        let mut long = b.to_vec();
        long.extend_from_slice(&[9; 4]);
        assert_eq!(InputSnapshot::parse(&long), Some(i), "a later minor");
        assert_eq!(i.pressed_gpios(), [3, 27, 40]);
        assert_eq!(i.readable_gpios().len(), 30);
        assert!(i.scan_on() && i.is_pressed(27) && !i.is_pressed(26) && !i.is_pressed(64));
        let rest = InputSnapshot::default();
        assert!(!rest.scan_on() && rest.pressed_gpios().is_empty());
        assert_eq!(rest.lx, 32767);
    }

    #[test]
    fn v1_8_log_commands() {
        let s = Setup::read(Command::ReadLog, 0x1234);
        assert_eq!(
            (s.request_type, s.index, s.value),
            (TYPE_IN, 0x500a, 0x1234)
        );
        assert!(Command::ReadLog.is_in());
        assert_eq!(Command::from_byte(0x0a), Some(Command::ReadLog));
        assert_eq!(Command::ReadLog.max_len(), u16::MAX);
        assert_eq!(Caps(0x4e2f).names().last(), Some(&"log"));
        let hello = |minor, caps| Hello {
            major: 1,
            minor,
            caps: Caps(caps),
            token: 1,
            modes: 0,
        };
        assert!(hello(8, Caps::LOG).has_log());
        assert!(!hello(7, Caps::LOG).has_log(), "1.7 never gets it");
        assert!(!hello(8, Caps::SLOTS).has_log());
    }

    fn record(seq: u32, code: u8, data: &[u8]) -> LogRecord {
        LogRecord {
            seq,
            ms: 27_054,
            boot: 4,
            code,
            data: data.to_vec(),
        }
    }

    #[test]
    fn a_log_page_round_trips() {
        let page = LogPage {
            first: 85,
            next: 87,
            boot: 5,
            records: vec![
                record(85, log_code::PANIC, b"test 1 22 333 4444 abcde"),
                record(86, log_code::BOOT, b"\x03\x00v0.7.12"),
            ],
        };
        let b = page.encode();
        assert_eq!(b.len(), 12 + 2 * 32);
        assert_eq!(&b[..12], &[85, 0, 0, 0, 87, 0, 0, 0, 5, 0, 32, 2]);
        assert_eq!(&b[12..20], &[0xae, 0x69, 0, 0, 4, 0, 2, 24]);
        assert_eq!(b[44 + 7], 9, "the data length of the BOOT record");
        assert_eq!(&b[44 + 17..76], &[0; 15], "zeros after the data");
        assert_eq!(LogPage::parse(&b), Some(page.clone()));
        assert_eq!(page.end(), 87);
        // A reply that is shorter than its count, or another record length.
        assert_eq!(LogPage::parse(&b[..75]), None);
        assert_eq!(LogPage::parse(&b[..11]), None);
        let mut odd = b.clone();
        odd[10] = 16;
        assert_eq!(LogPage::parse(&odd), None);
        // No records: the head only.
        let empty = LogPage::parse(&[87, 0, 0, 0, 87, 0, 0, 0, 5, 0, 32, 0]).unwrap();
        assert!(empty.records.is_empty() && empty.end() == 87);
        // A data length past 24 is cut.
        let mut long = b.clone();
        long[12 + 7] = 30;
        assert_eq!(LogPage::parse(&long).unwrap().records[0].data.len(), 24);
    }

    #[test]
    fn log_records_decode_by_code() {
        let e = |code, data: &[u8]| record(0, code, data).event();
        assert_eq!(
            e(1, b"\x03\x02v0.7.12-135"),
            LogEvent::Boot {
                kept: true,
                watchdog: true,
                mode: 2,
                version: "v0.7.12-135".into()
            }
        );
        assert_eq!(
            e(2, b"oops"),
            LogEvent::Panic {
                text: "oops".into()
            }
        );
        assert_eq!(
            e(3, &[0x11, 0, 0x8d, 0xb5]),
            LogEvent::Command {
                cmd: 0x11,
                result: Outcome::Ok,
                seq: 46477
            }
        );
        assert_eq!(e(4, &[254, 1]), LogEvent::Profile { old: 254, new: 1 });
        assert_eq!(
            e(5, &[1, 0]),
            LogEvent::Save {
                force: true,
                restart: false
            }
        );
        assert_eq!(e(6, &[2]), LogEvent::Restart { mode: 2 });
        assert_eq!(e(7, b"bad"), LogEvent::Error { text: "bad".into() });
        assert_eq!(
            e(8, &[1, 3, 0x5e, 0x04, 0x8e, 0x02]),
            LogEvent::UsbHost {
                mounted: true,
                address: 3,
                vid: 0x045e,
                pid: 0x028e
            }
        );
        // A code from a later firmware, or data too short for its code.
        assert_eq!(
            e(9, &[1, 2]),
            LogEvent::Unknown {
                code: 9,
                data: vec![1, 2]
            }
        );
        assert!(matches!(
            e(3, &[0x11, 0]),
            LogEvent::Unknown { code: 3, .. }
        ));
        assert_eq!(log_code::name(8), Some("USB_HOST"));
        assert_eq!(log_code::name(0), None);
        assert_eq!(
            (log_boot_mode(3), log_boot_mode(255), log_boot_mode(7)),
            ("bootsel", "other", "other")
        );
        assert_eq!(Outcome::Busy.name(), Some("BUSY"));
        assert_eq!(Outcome::Other(9).name(), None);
    }

    #[test]
    fn v1_4_slot_commands() {
        let s = Setup::read(Command::ReadSlot, 6);
        assert_eq!((s.request_type, s.index, s.value), (TYPE_IN, 0x5006, 6));
        assert!(Command::ReadSlot.is_in() && !Command::WriteSlot.is_in());
        assert_eq!(Setup::command(Command::WriteSlot).index, 0x5015);
        assert_eq!(Command::from_byte(0x15), Some(Command::WriteSlot));
        assert_eq!(Command::WriteSlot.max_len(), 8 + 20 + 480);
        assert_eq!(Command::Status.max_len(), 64);
        assert_eq!(Caps(0x62f).names().last(), Some(&"slots"));
        let hello = |minor, caps| Hello {
            major: 1,
            minor,
            caps: Caps(caps),
            token: 1,
            modes: 0,
        };
        assert!(hello(4, 0x62f).has_slots());
        assert!(!hello(3, 0x62f).has_slots(), "1.3 never gets them");
        assert!(!hello(4, 0x22f).has_slots());
    }

    fn s16_slot() -> Slot {
        let mut pins = vec![SlotPin::default(); 30];
        pins[0] = SlotPin {
            action: 5,
            button_mask: 0,
            dpad_mask: 0,
        };
        pins[1] = SlotPin {
            action: 40,
            button_mask: 0x0003,
            dpad_mask: 0x8,
        };
        pins[29] = SlotPin {
            action: -10,
            ..SlotPin::default()
        };
        Slot {
            profile: 6,
            flags: Slot::ENABLED | Slot::ACTIVE,
            label: "Hollow Knight".into(),
            pins,
        }
    }

    #[test]
    fn a_slot_round_trips() {
        let s = s16_slot();
        let b = s.encode();
        assert_eq!(b.len(), 320, "20 + 10 x 30; 328 with the header");
        assert_eq!(&b[0..4], &[6, 3, 30, 10]);
        assert_eq!(&b[4..17], b"Hollow Knight");
        assert_eq!(b[17], 0);
        assert_eq!(&b[20..22], &[5, 0]);
        assert_eq!(&b[30..40], &[40, 0, 3, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(&b[310..312], &[0xf6, 0xff], "-10");
        assert_eq!(Slot::parse(&b), Some(s.clone()));
        assert_eq!(Slot::parse(&b[..319]), None);
        let mut odd = b.clone();
        odd[3] = 9;
        assert_eq!(Slot::parse(&odd), None);
        // A label of 16 bytes has no NUL.
        let full = Slot {
            label: "0123456789abcdef".into(),
            ..s.clone()
        };
        assert_eq!(Slot::parse(&full.encode()).unwrap().label, full.label);
        // A slot that was never written: zeros after the head.
        let mut empty = vec![0u8; 320];
        empty[..4].copy_from_slice(&[6, 0, 30, 10]);
        let e = Slot::parse(&empty).unwrap();
        assert!(e.label.is_empty() && !e.enabled());
        assert!(e.pins.iter().all(|p| p.action == 0));
    }

    #[test]
    fn the_host_checks_a_slot_as_the_firmware_does() {
        let ok = s16_slot();
        assert_eq!(ok.check(30, 2), Ok(()));
        let bad = |f: &dyn Fn(&mut Slot), active| {
            let mut s = s16_slot();
            f(&mut s);
            s.check(30, active).unwrap_err()
        };
        assert!(bad(&|s| s.profile = 1, 2).contains("profiles 2 to 6"));
        assert!(bad(&|s| s.profile = 7, 2).contains("profiles 2 to 6"));
        assert!(bad(&|s| s.flags |= 0x08, 2).contains("unknown flags 0x08"));
        assert!(bad(&|s| s.pins.pop().map(|_| ()).unwrap(), 2).contains("29 pins"));
        assert!(bad(&|s| s.label = "a label that is too long".into(), 2).contains("24 bytes"));
        assert!(bad(&|s| s.pins[3].action = 139, 2).contains("GPIO 3: action 139"));
        assert!(bad(&|s| s.pins[3].action = -11, 2).contains("action -11"));
        assert!(bad(&|s| s.flags = Slot::ACTIVE, 2).contains("the one to select"));
        assert!(bad(&|s| s.flags = 0, 6).contains("the active profile"));
        // Disabled, and not the active one: allowed.
        let off = Slot {
            flags: 0,
            ..s16_slot()
        };
        assert_eq!(off.check(30, 2), Ok(()));
    }

    #[test]
    fn v1_2_lights_commands() {
        let s = Setup::read(Command::LightsStatus, 0);
        assert_eq!((s.request_type, s.index), (TYPE_IN, 0x5005));
        assert!(Command::LightsStatus.is_in() && !Command::SetLights.is_in());
        assert_eq!(Setup::command(Command::SetLights).index, 0x5014);
        assert_eq!(Command::from_byte(0x14), Some(Command::SetLights));
        assert_eq!(Command::SetLights.payload_len(), Some(4));
        assert_eq!(Caps(0x22f).names().last(), Some(&"lights"));
        assert!(Caps(0x22f).has(Caps::LIGHTS) && !Caps(0x22f).has(Caps::LEDS));
    }

    #[test]
    fn lights_status_round_trips() {
        let s = LightsStatus {
            flags: LightsStatus::RUNNING | LightsStatus::UNSAVED,
            profile: 2,
            enabled_profiles: 0b0111,
            profiles: 4,
            brightness: 7,
            brightness_steps: 10,
            brightness_maximum: 255,
            idle_effect: 3,
            pressed_effect: 4,
            case_effect: 1,
            idle_cycle_time: 5,
            pressed_cycle_time: 0,
            case_cycle_time: 9,
            idle_timeout_s: 300,
        };
        let b = s.encode();
        assert_eq!(b[1], 2);
        assert_eq!(&b[14..16], &[0x2c, 0x01]);
        assert_eq!(LightsStatus::parse(&b), Some(s));
        assert_eq!(LightsStatus::parse(&b[..15]), None);
        assert!(s.unsaved() && !s.preview());
        assert_eq!(s.on(), Some(2));
        assert!(s.profile_enabled(2) && !s.profile_enabled(3) && !s.profile_enabled(4));
        let off = LightsStatus { profile: -1, ..s };
        assert_eq!(off.encode()[1], 0xff);
        assert_eq!(LightsStatus::parse(&off.encode()).unwrap().on(), None);
    }

    #[test]
    fn set_lights_masks_what_it_sets() {
        let off = SetLights {
            profile: Some(-1),
            ..SetLights::default()
        };
        assert_eq!(off.encode(), [0x01, 0xff, 0, 0]);
        assert_eq!(SetLights::parse(&off.encode()), Some(off));
        let both = SetLights {
            profile: Some(1),
            brightness: Some(4),
            persist: true,
        };
        assert_eq!(both.encode(), [0x03, 1, 4, 1]);
        assert_eq!(SetLights::parse(&both.encode()), Some(both));
        assert!(SetLights::default().is_empty());
        assert_eq!(SetLights::parse(&[0x04, 0, 0, 0]), None);
        assert_eq!(SetLights::parse(&[0x01, 0, 0, 2]), None);
    }

    #[test]
    fn led_status_round_trips() {
        let s = LedStatus {
            flags: LedStatus::RUNNING | LedStatus::UNSAVED,
            animation: 4,
            animations: 5,
            brightness: 3,
            brightness_steps: 5,
            brightness_maximum: 100,
            static_color: 2,
            button_color: 1,
            theme: 0,
            colors: 16,
            themes: 20,
        };
        let b = s.encode();
        assert_eq!(b, [5, 4, 5, 3, 5, 100, 2, 1, 0, 16, 20, 0]);
        assert_eq!(LedStatus::parse(&b), Some(s));
        assert_eq!(LedStatus::parse(&b[..11]), None);
        assert!(s.running() && s.unsaved() && !s.custom_theme());
    }

    #[test]
    fn set_led_masks_what_it_sets() {
        let set = SetLed {
            brightness: Some(2),
            theme: Some(7),
            persist: true,
            ..SetLed::default()
        };
        assert_eq!(set.encode(), [0b1_0010, 0, 2, 0, 0, 7, 1, 0]);
        assert_eq!(SetLed::parse(&set.encode()), Some(set));
        let all = SetLed {
            animation: Some(1),
            brightness: Some(2),
            static_color: Some(3),
            button_color: Some(4),
            theme: Some(5),
            persist: false,
        };
        assert_eq!(all.encode(), [0x1f, 1, 2, 3, 4, 5, 0, 0]);
        assert!(SetLed::default().is_empty());
        // Unknown mask bits, or a persist byte of 2: BAD_ARG.
        assert_eq!(SetLed::parse(&[0x20, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(SetLed::parse(&[0x01, 0, 0, 0, 0, 0, 2, 0]), None);
    }
}
