//! Wire format v1 of the control channel (`docs/design/control-channel.md`
//! §3, the v1.1 LED commands of §3.7, the v1.2 lights commands of §3.8,
//! and the v1.3 board ID of §3.9):
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
/// Every request and reply fits one 64-byte EP0 packet.
pub const MAX_LEN: u16 = 64;
/// The first bytes of a HELLO reply.
pub const HELLO_MAGIC: [u8; 4] = *b"GPii";
/// The protocol major this host speaks; it refuses any other.
pub const MAJOR: u8 = 1;
/// The newest protocol minor this host knows (v1.3: the board ID). A pad's
/// minor tells which STRING ids it has; what else it can do is in its
/// capabilities.
pub const MINOR: u8 = 3;
/// The first minor with STRING [`string::BOARD_ID`].
pub const BOARD_ID_MINOR: u8 = 3;
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

/// The v1 commands, v1.1's LED ones and v1.2's lights ones.
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
    SetProfile = 0x10,
    Reboot = 0x11,
    SetInputMode = 0x12,
    /// v1.1, behind [`Caps::LEDS`].
    SetLed = 0x13,
    /// v1.2, behind [`Caps::LIGHTS`].
    SetLights = 0x14,
}

impl Command {
    pub const ALL: [Command; 10] = [
        Command::Hello,
        Command::Status,
        Command::String,
        Command::LedStatus,
        Command::LightsStatus,
        Command::SetProfile,
        Command::Reboot,
        Command::SetInputMode,
        Command::SetLed,
        Command::SetLights,
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
        }
    }

    /// The OUT payload length after the header.
    pub fn payload_len(self) -> Option<usize> {
        match self {
            Command::SetProfile | Command::SetInputMode => Some(2),
            Command::Reboot => Some(1),
            Command::SetLed => Some(SET_LED_LEN),
            Command::SetLights => Some(SET_LIGHTS_LEN),
            Command::Hello
            | Command::Status
            | Command::String
            | Command::LedStatus
            | Command::LightsStatus => None,
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

    const NAMES: [(u32, &'static str); 10] = [
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
