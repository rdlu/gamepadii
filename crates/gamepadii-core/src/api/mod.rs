//! The web-config API, through an allowlist. [`Endpoint`] lists every
//! endpoint verified safe to read on a firmware line (GP2040-CE v0.7.12,
//! on the S16, and upstream main, from its source); reads take an
//! `Endpoint`, so nothing else can be read. Which ones a pad has depends on
//! its line ([`line::FirmwareLine::endpoints`]; [`line`] tells the lines
//! apart). In particular these are not in it on any line, and cannot be
//! named:
//!
//! - `getConfig`: on stock v0.7.12 it runs the web server out of heap and
//!   hangs the S16's network stack until it is replugged.
//! - `getI2CPeripheralMap`: a live I2C bus scan, which hangs stock the same
//!   way.
//! - `resetSettings`: a destructive GET that wipes the configuration.
//! - `getHeldPins`: blocks until a button is pressed (up to 5 s on main,
//!   which also reconfigures the unused GPIOs; it will come later, behind
//!   its own function, for the button test).
//! - main's `getHETriggerVoltage`, `getJoystickCenter` and
//!   `getJoystickCenter2`: they drive the ADC (and the mux select pins),
//!   and the first reads the previous POST body.
//! - main's `getLightsDataPresets` (a 32 KB document) and
//!   `getLightsPresets/<n>`: the board's built-in light layouts, not
//!   settings.
//! - every `set*`: the server ignores the method, so a GET would replay the
//!   previous POST body. The writes, [`set_gamepad_options`] and
//!   [`set_display_options`], live in `write` and are only ever POSTed.
//!
//! ```compile_fail
//! let _ = gamepadii_core::api::Endpoint::GetConfig;
//! ```
//! ```compile_fail
//! let _ = gamepadii_core::api::Endpoint::GetI2CPeripheralMap;
//! ```
//!
//! Our firmware fork fixes both hangs: [`fork::get_config`] and
//! [`fork::get_i2c_peripheral_map`] read them, and only with a
//! [`ForkFixes`], which only the capability check makes (see [`fork`]).
//!
//! The only other request is [`reboot`].

pub mod fork;
pub mod line;
pub mod model;
pub mod write;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use fork::{Evidence, ForkFixes};
pub use line::{FirmwareLine, Line, LineEvidence};
pub use model::{
    DisplayOptions, FirmwareVersion, GamepadOptions, KeyMappings, MemoryReport, PinMapping,
    PinMappings, ProfileOptions,
};
pub use write::{FieldChange, OptionsWrite, set_display_options, set_gamepad_options};

use crate::error::{Error, Result};
use crate::mode::BootMode;
use crate::transport::{ApiPath, Method, Transport};

macro_rules! endpoints {
    ($($variant:ident => $name:literal,)*) => {
        /// An endpoint verified safe to `GET` (see the module docs).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Endpoint {
            $($variant,)*
        }

        impl Endpoint {
            /// Every line's allowlist together, in alphabetical order
            /// (a pad has [`line::FirmwareLine::endpoints`]).
            pub const ALL: &[Endpoint] = &[$(Endpoint::$variant,)*];

            /// The firmware's name: `getGamepadOptions`.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Endpoint::$variant => $name,)*
                }
            }

            const fn path(self) -> ApiPath {
                ApiPath::new(match self {
                    $(Endpoint::$variant => concat!("/api/", $name),)*
                })
            }
        }
    };
}

endpoints! {
    GetAddonsOptions => "getAddonsOptions",
    GetAnimationProtoOptions => "getAnimationProtoOptions",
    GetBoardDefinition => "getBoardDefinition",
    GetBootModeOptions => "getBootModeOptions",
    GetButtonLayoutDefs => "getButtonLayoutDefs",
    GetButtonLayouts => "getButtonLayouts",
    GetCustomTheme => "getCustomTheme",
    GetDisplayOptions => "getDisplayOptions",
    GetExpansionPins => "getExpansionPins",
    GetFirmwareVersion => "getFirmwareVersion",
    GetGamepadOptions => "getGamepadOptions",
    GetHETriggerCalibration => "getHETriggerCalibration",
    GetHETriggerCalibrations => "getHETriggerCalibrations",
    GetHETriggerOptions => "getHETriggerOptions",
    GetKeyMappings => "getKeyMappings",
    GetLedOptions => "getLedOptions",
    GetLightsDataOptions => "getLightsDataOptions",
    GetMacroAddonOptions => "getMacroAddonOptions",
    GetMemoryReport => "getMemoryReport",
    GetPeripheralOptions => "getPeripheralOptions",
    GetPinMappings => "getPinMappings",
    GetProfileOptions => "getProfileOptions",
    GetReactiveLEDs => "getReactiveLEDs",
    GetSplashImage => "getSplashImage",
    GetUsedPins => "getUsedPins",
    GetWiiControls => "getWiiControls",
}

impl Endpoint {
    /// The allowlisted endpoint with this exact name; `None` for anything
    /// else, `getConfig` included (that one is [`fork::get_config`]).
    pub fn from_name(name: &str) -> Option<Endpoint> {
        Endpoint::ALL.iter().copied().find(|e| e.name() == name)
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for Endpoint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// The endpoint's JSON, or `None` when the firmware doesn't have it (a 404:
/// endpoints come and go between versions).
pub fn try_read_raw(t: &dyn Transport, ep: Endpoint) -> Result<Option<Value>> {
    let reply = t.request(Method::Get, ep.path(), None)?;
    match reply.status {
        200 => {}
        404 => return Ok(None),
        s => return Err(Error::Protocol(format!("{ep}: HTTP {s}"))),
    }
    serde_json::from_slice(&reply.body)
        .map(Some)
        .map_err(|e| Error::Protocol(format!("{ep}: not JSON: {e}")))
}

/// The endpoint's JSON; a 404 is an error.
pub fn read_raw(t: &dyn Transport, ep: Endpoint) -> Result<Value> {
    try_read_raw(t, ep)?.ok_or_else(|| {
        Error::Protocol(format!(
            "{ep}: HTTP 404 (not in this firmware, or not web-config)"
        ))
    })
}

/// The endpoint's JSON as `T`.
pub fn read<T: DeserializeOwned>(t: &dyn Transport, ep: Endpoint) -> Result<T> {
    let value = read_raw(t, ep)?;
    serde_json::from_value(value)
        .map_err(|e| Error::Protocol(format!("{ep}: unexpected shape: {e}")))
}

pub fn firmware_version(t: &dyn Transport) -> Result<FirmwareVersion> {
    read(t, Endpoint::GetFirmwareVersion)
}

pub fn memory_report(t: &dyn Transport) -> Result<MemoryReport> {
    read(t, Endpoint::GetMemoryReport)
}

pub fn gamepad_options(t: &dyn Transport) -> Result<GamepadOptions> {
    read(t, Endpoint::GetGamepadOptions)
}

pub fn display_options(t: &dyn Transport) -> Result<DisplayOptions> {
    read(t, Endpoint::GetDisplayOptions)
}

/// The base profile (profile 1): GPIO pin → action.
pub fn pin_mappings(t: &dyn Transport) -> Result<PinMappings> {
    read(t, Endpoint::GetPinMappings)
}

/// Profiles 2 and up.
pub fn profile_options(t: &dyn Transport) -> Result<ProfileOptions> {
    read(t, Endpoint::GetProfileOptions)
}

pub fn key_mappings(t: &dyn Transport) -> Result<KeyMappings> {
    read(t, Endpoint::GetKeyMappings)
}

const REBOOT: ApiPath = ApiPath::new("/api/reboot");

/// Whether the pad said yes to a reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootAck {
    /// It answered `{"success": true}`.
    Confirmed,
    /// The request went out and the connection then dropped or timed out:
    /// most likely it rebooted before the reply made it back. Only the
    /// re-enumeration can tell.
    Probable,
}

/// `POST /api/reboot {"bootMode": n}`. The firmware answers
/// `{"bootMode": n, "success": true}` and restarts 500 ms later. A
/// connection that drops or times out once the request is out counts as
/// [`RebootAck::Probable`]; a refused connection (nothing sent) is still
/// an error.
pub fn reboot(t: &dyn Transport, mode: BootMode) -> Result<RebootAck> {
    let body = serde_json::json!({ "bootMode": mode.api_value() }).to_string();
    let reply = match t.request(Method::Post, REBOOT, Some(body.as_bytes())) {
        Ok(r) => r,
        Err(Error::Io { source, .. }) if dropped_after_sending(source.kind()) => {
            return Ok(RebootAck::Probable);
        }
        Err(e) => return Err(e),
    };
    if reply.status != 200 {
        return Err(Error::Protocol(format!("reboot: HTTP {}", reply.status)));
    }
    let doc: Value = match serde_json::from_slice(&reply.body) {
        Ok(d) => d,
        // A reply cut short by the reboot.
        Err(_) if reply.body.is_empty() => return Ok(RebootAck::Probable),
        Err(e) => return Err(Error::Protocol(format!("reboot: not JSON: {e}"))),
    };
    if doc.get("success") != Some(&Value::Bool(true)) {
        return Err(Error::Protocol(format!("reboot: the pad said {doc}")));
    }
    Ok(RebootAck::Confirmed)
}

/// I/O errors that mean the connection went away mid-request, rather than
/// never being made.
fn dropped_after_sending(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        kind,
        K::TimedOut | K::ConnectionReset | K::ConnectionAborted | K::UnexpectedEof | K::BrokenPipe
    )
}
