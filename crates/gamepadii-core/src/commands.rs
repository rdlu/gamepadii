//! What the front ends run. Each command returns plain serde data and never
//! prints; `gamepadii`'s `output` module renders it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{self, Endpoint, FirmwareVersion, MemoryReport};
use crate::client::Client;
use crate::discovery::{self, Device};
use crate::error::{Error, Result};
use crate::mode::{BootMode, InputMode};
use crate::sysfs::Root;
use crate::timestamp::Utc;

/// `gamepadii status`: every GP2040-CE device on USB, with its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub devices: Vec<Device>,
}

/// [`Status`] for the machine under `root`; [`Error::NotFound`] when no
/// device has an id GP2040-CE uses.
pub fn status(root: &Root) -> Result<Status> {
    let devices = discovery::scan(root);
    if devices.is_empty() {
        return Err(Error::NotFound(
            "no GP2040-CE device on USB (is it plugged in?)".into(),
        ));
    }
    Ok(Status { devices })
}

/// The firmware build, from `getFirmwareVersion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Firmware {
    /// `v0.7.12`.
    pub version: Option<String>,
    /// The board config the build used: `Haute42COSMOX`.
    pub board: Option<String>,
    /// The same, for people: `Haute42 COSMOX`.
    pub board_label: Option<String>,
    /// `rp2040` or `rp2350`.
    pub architecture: Option<String>,
    /// The commit it was built from: `0014e4a`.
    pub build: Option<String>,
    /// `Release` or `Debug`.
    pub build_type: Option<String>,
    pub file_name: Option<String>,
}

impl From<&FirmwareVersion> for Firmware {
    fn from(f: &FirmwareVersion) -> Self {
        Self {
            version: f.version.clone(),
            board: f.board_config.clone(),
            board_label: f.board_config_label.clone(),
            architecture: f.board_architecture.clone(),
            build: f.board_build.clone(),
            build_type: f.board_build_type.clone(),
            file_name: f.board_config_file_name.clone(),
        }
    }
}

/// Flash and RAM use, in bytes, from `getMemoryReport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Memory {
    pub flash_used: Option<u64>,
    /// The flash the firmware may use.
    pub flash_total: Option<u64>,
    /// The flash chip.
    pub flash_physical: Option<u64>,
    pub heap_used: Option<u64>,
    pub heap_total: Option<u64>,
    pub static_allocs: Option<u64>,
}

impl From<&MemoryReport> for Memory {
    fn from(m: &MemoryReport) -> Self {
        Self {
            flash_used: m.used_flash,
            flash_total: m.total_flash,
            flash_physical: m.physical_flash,
            heap_used: m.used_heap,
            heap_total: m.total_heap,
            static_allocs: m.static_allocs,
        }
    }
}

/// A raw `InputMode` setting and what it decodes to (`null` for `-1`,
/// "none", or a value this gamepadii doesn't know).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModeSetting {
    pub value: Option<i64>,
    pub mode: Option<InputMode>,
}

impl ModeSetting {
    fn new(value: Option<i64>) -> Self {
        let mode = value
            .and_then(|v| u32::try_from(v).ok())
            .and_then(InputMode::from_proto);
        Self { value, mode }
    }
}

/// A pin-mapping profile slot on the pad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// From 1, as the firmware and the `LOAD_PROFILE_n` hotkeys count.
    pub number: u32,
    pub label: Option<String>,
    pub enabled: Option<bool>,
    pub active: bool,
}

/// Hold `button` while plugging in to boot into `mode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BootButton {
    /// The logical button: `B1`..`B4`, `L1`, `L2`, `R1`, `R2`.
    pub button: String,
    #[serde(flatten)]
    pub setting: ModeSetting,
}

/// `gamepadii info`: what the pad in web-config says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Info {
    /// Where it was read from: `http://192.168.7.1`.
    pub host: String,
    pub firmware: Firmware,
    /// `null` when the firmware has no `getMemoryReport`.
    pub memory: Option<Memory>,
    /// The persisted input mode: what the pad boots into in gamepad mode.
    pub input_mode: ModeSetting,
    /// The active profile's number.
    pub active_profile: Option<i64>,
    pub profiles: Vec<Profile>,
    pub boot_buttons: Vec<BootButton>,
}

/// [`Info`] for the pad behind `client` (five reads).
pub fn info(client: &Client) -> Result<Info> {
    let t = client.transport();
    let memory = api::try_read_raw(t, Endpoint::GetMemoryReport)?
        .map(|v| {
            serde_json::from_value::<MemoryReport>(v)
                .map_err(|e| Error::Protocol(format!("getMemoryReport: unexpected shape: {e}")))
        })
        .transpose()?;
    let options = api::gamepad_options(t)?;
    let base = api::pin_mappings(t)?;
    let alternatives = api::profile_options(t)?.alternative_pin_mappings;
    let active = options.profile_number;
    let profiles = std::iter::once(&base)
        .chain(&alternatives)
        .zip(1u32..)
        .map(|(p, number)| Profile {
            number,
            label: p.profile_label.clone(),
            enabled: p.enabled,
            active: active == Some(i64::from(number)),
        })
        .collect();
    let boot_buttons = options
        .boot_buttons()
        .into_iter()
        .map(|(button, value)| BootButton {
            button: button.to_string(),
            setting: ModeSetting::new(value),
        })
        .collect();
    Ok(Info {
        host: client.host(),
        firmware: client.firmware().into(),
        memory: memory.as_ref().map(Memory::from),
        input_mode: ModeSetting::new(options.input_mode),
        active_profile: active,
        profiles,
        boot_buttons,
    })
}

/// Bumped on any breaking change to the backup file format.
pub const BACKUP_SCHEMA: u32 = 1;

/// A backup file: every allowlisted endpoint's JSON, as the pad sent it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: u32,
    pub gamepadii_version: String,
    /// RFC 3339, UTC.
    pub captured_at: String,
    pub firmware: Firmware,
    /// Endpoint name → its JSON, verbatim; `null` when the firmware doesn't
    /// have that endpoint (a 404).
    pub endpoints: BTreeMap<String, Value>,
}

/// Reads every endpoint in [`Endpoint::ALL`], one at a time, stopping at the
/// first transport error (a pad that stops answering should be left alone,
/// not asked twenty more times).
pub fn backup_bundle(client: &Client, now: SystemTime) -> Result<Bundle> {
    let mut endpoints = BTreeMap::new();
    for &ep in Endpoint::ALL {
        let value = api::try_read_raw(client.transport(), ep)?.unwrap_or(Value::Null);
        endpoints.insert(ep.name().to_string(), value);
    }
    Ok(Bundle {
        schema: BACKUP_SCHEMA,
        gamepadii_version: env!("CARGO_PKG_VERSION").to_string(),
        captured_at: Utc::from_system(now).rfc3339(),
        firmware: client.firmware().into(),
        endpoints,
    })
}

/// `gamepadii backup`: where the bundle went and what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupReport {
    pub path: PathBuf,
    pub captured_at: String,
    pub firmware: Firmware,
    /// The endpoints saved.
    pub endpoints: Vec<String>,
    /// The endpoints this firmware doesn't have (saved as `null`).
    pub unavailable: Vec<String>,
    pub bytes: u64,
}

/// Reads a [`Bundle`] and writes it to `output`, or to
/// [`default_backup_path`] when that is `None`.
pub fn backup(client: &Client, output: Option<&Path>, now: SystemTime) -> Result<BackupReport> {
    let bundle = backup_bundle(client, now)?;
    let path = match output {
        Some(p) => p.to_path_buf(),
        None => default_backup_path(&bundle, now)?,
    };
    let bytes = write_bundle(&bundle, &path)?;
    let (endpoints, unavailable): (Vec<_>, Vec<_>) =
        bundle.endpoints.iter().partition(|(_, v)| !v.is_null());
    let names = |v: Vec<(&String, &Value)>| v.into_iter().map(|(k, _)| k.clone()).collect();
    Ok(BackupReport {
        path,
        captured_at: bundle.captured_at,
        firmware: bundle.firmware,
        endpoints: names(endpoints),
        unavailable: names(unavailable),
        bytes,
    })
}

/// `$XDG_DATA_HOME/gamepadii/backups` (`~/.local/share/gamepadii/backups`).
pub fn backups_dir() -> Result<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let data = env("XDG_DATA_HOME")
        .or_else(|| env("HOME").map(|h| h.join(".local/share")))
        .ok_or_else(|| Error::Usage("no $HOME or $XDG_DATA_HOME: pass -o FILE".into()))?;
    Ok(data.join("gamepadii").join("backups"))
}

/// `<backups_dir>/20261017-165703-Haute42COSMOX.json`.
pub fn default_backup_path(bundle: &Bundle, now: SystemTime) -> Result<PathBuf> {
    Ok(backups_dir()?.join(backup_file_name(bundle, now)))
}

/// `20261017-165703-Haute42COSMOX.json`: UTC time, then the board config
/// with anything but `[A-Za-z0-9_-]` dropped.
pub fn backup_file_name(bundle: &Bundle, now: SystemTime) -> String {
    let board: String = bundle
        .firmware
        .board
        .as_deref()
        .unwrap_or("gp2040ce")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    let board = if board.is_empty() {
        "gp2040ce".into()
    } else {
        board
    };
    format!("{}-{board}.json", Utc::from_system(now).compact())
}

/// Writes the bundle as pretty JSON through a temporary file and a rename,
/// so a crash never leaves half a backup; returns its size.
fn write_bundle(bundle: &Bundle, path: &Path) -> Result<u64> {
    let mut text =
        serde_json::to_string_pretty(bundle).expect("serialising plain data cannot fail");
    text.push('\n');
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, &text).map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(format!("writing {}", path.display()), e)
    })?;
    Ok(text.len() as u64)
}

/// `gamepadii reboot`: the request was accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reboot {
    pub host: String,
    pub mode: BootMode,
}

/// Asks the pad to restart into `mode`. It does so right after answering;
/// this doesn't wait for it to come back.
pub fn reboot(client: &Client, mode: BootMode) -> Result<Reboot> {
    api::reboot(client.transport(), mode)?;
    Ok(Reboot {
        host: client.host(),
        mode,
    })
}
