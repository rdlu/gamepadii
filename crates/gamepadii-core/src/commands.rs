//! What the front ends run. Each command returns plain serde data and never
//! prints; `gamepadii`'s `output` module renders it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{self, Endpoint, FirmwareVersion, MemoryReport};
use crate::cache::{self, DeviceCache};
use crate::channel::{self, Caps, Channel, OpenControl, Probe, proto};
use crate::client::Client;
use crate::discovery::{Device, Scan};
use crate::error::{Error, Result};
use crate::mode::Confidence;
use crate::mode::{BootMode, InputMode};
use crate::sysfs::Root;
use crate::timestamp::Utc;

/// `gamepadii status`: every GP2040-CE device on USB, with its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub devices: Vec<StatusDevice>,
}

/// A device, and what its control channel says (when it has one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusDevice {
    /// With the channel, `mode` is what the firmware reports and
    /// `confidence` is `confirmed`.
    #[serde(flatten)]
    pub device: Device,
    /// `null` when the pad has no channel (stock firmware) or it wasn't
    /// probed.
    pub control_channel: Option<ChannelReport>,
    /// Why the channel couldn't be used, when that is worth saying (no
    /// permission on the node of a pad known to have it, a newer protocol).
    pub channel_note: Option<String>,
}

/// [`Status`] for the machine under `root`, from sysfs alone (no channel
/// probe); [`Error::NotFound`] when no device has an id GP2040-CE uses.
pub fn status(root: &Root) -> Result<Status> {
    status_with(root, None, None)
}

/// [`Status`] with a HELLO to each pad through `control` (none for BOOTSEL,
/// the boot ROM has no channel). Pads seen with the channel are recorded
/// in `cache_dir`, so a later permission problem gets the udev hint.
pub fn status_with(
    scan: &dyn Scan,
    control: Option<&OpenControl<'_>>,
    cache_dir: Option<&Path>,
) -> Result<Status> {
    let devices = scan.scan();
    if devices.is_empty() {
        return Err(Error::NotFound(
            "no GP2040-CE device on USB (is it plugged in?)".into(),
        ));
    }
    let devices = devices
        .into_iter()
        .map(|device| match control {
            Some(open) if device.mode != Some(InputMode::Bootsel) => {
                probe_status(open, device, cache_dir)
            }
            _ => StatusDevice {
                device,
                control_channel: None,
                channel_note: None,
            },
        })
        .collect();
    Ok(Status { devices })
}

fn probe_status(
    open: &OpenControl<'_>,
    mut device: Device,
    cache_dir: Option<&Path>,
) -> StatusDevice {
    let (mut report, mut note) = (None, None);
    match channel::probe(open, &device) {
        Probe::Found(ch) => match channel_report(&ch) {
            Ok(r) => {
                confirm(&mut device, &r);
                remember(cache_dir, &ch, &r.firmware);
                let _ = remember_profile(
                    cache_dir,
                    &r.firmware,
                    r.active_profile.number,
                    false,
                    SystemTime::now(),
                );
                report = Some(r);
            }
            Err(e) => note = Some(format!("the control channel answered HELLO, then: {e}")),
        },
        Probe::NoAccess { node } if cache::channel_known(cache_dir) => {
            note = Some(channel::udev_hint(&node));
        }
        Probe::Newer { major, .. } => note = Some(newer_protocol(major)),
        Probe::NoAccess { .. } | Probe::Absent(_) => {}
    }
    StatusDevice {
        device,
        control_channel: report,
        channel_note: note,
    }
}

/// What to say about a channel speaking a protocol major we don't.
pub fn newer_protocol(major: u8) -> String {
    format!(
        "the pad's control channel speaks protocol v{major}; this gamepadii speaks v{}, \
         so it isn't used (update gamepadii)",
        proto::MAJOR
    )
}

/// The firmware said which mode it runs: no more guessing from the VID:PID.
pub fn confirm(device: &mut Device, r: &ChannelReport) {
    device.confidence = Confidence::Confirmed;
    device.note = None;
    if let Some(m) = r.input_mode.mode {
        device.mode = Some(m);
    }
}

/// Records the channel for this board (best effort).
pub fn remember(cache_dir: Option<&Path>, ch: &Channel, fw: &Firmware) {
    if let Some(dir) = cache_dir {
        let _ = fs::create_dir_all(dir);
        let _ = cache::record_channel(dir, &board_slug(fw), ch.hello());
    }
}

/// Keeps the device cache's running profile current: the channel says the
/// pad runs profile `n` (saved too, with `saved`). The cache is a
/// convenience, so a failure is returned for a warning at most.
pub fn remember_profile(
    cache_dir: Option<&Path>,
    fw: &Firmware,
    n: u8,
    saved: bool,
    now: SystemTime,
) -> Option<Error> {
    cache::record_profile(cache_dir?, &board_slug(fw), n, saved, now).err()
}

/// The pad was restarted by gamepadii: the cache's running profile is the
/// saved one again (best effort).
pub fn remember_restart(cache_dir: Option<&Path>, fw: &Firmware) {
    if let Some(dir) = cache_dir {
        let _ = cache::record_restart(dir, &board_slug(fw));
    }
}

/// A profile by number, with its label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileName {
    pub number: u8,
    pub label: Option<String>,
}

impl ProfileName {
    /// `2 SilkSong`, or `2`.
    pub fn display(&self) -> String {
        match &self.label {
            Some(l) => format!("{} {l}", self.number),
            None => self.number.to_string(),
        }
    }
}

/// What the control channel says about the pad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelReport {
    /// The device node it was reached through: `/dev/bus/usb/003/007`.
    pub node: String,
    /// `1.0`.
    pub protocol: String,
    pub capabilities: Caps,
    /// The input modes this build can boot.
    pub input_modes: Vec<InputMode>,
    /// From the build: version, board config, label, build type (the rest
    /// is `null`; web-config has it).
    pub firmware: Firmware,
    /// The running mode: `webconfig` (255) in web-config.
    pub input_mode: ModeSetting,
    pub active_profile: ProfileName,
    pub enabled_profiles: Vec<u8>,
    /// A profile switch the main loop hasn't applied yet.
    pub profile_pending: bool,
    /// `forcedSetupOptions.mode`: 1 locks mode switching, 2 web-config, 3
    /// both.
    pub forced_setup: u8,
    pub uptime_ms: u32,
}

/// STATUS and the strings (version, board, the active profile's label).
pub fn channel_report(ch: &Channel) -> Result<ChannelReport> {
    let st = ch.status()?;
    let has_strings = ch.caps().has(Caps::STRING);
    let string =
        |id| -> Result<Option<String>> { if has_strings { ch.string(id) } else { Ok(None) } };
    let h = ch.hello();
    Ok(ChannelReport {
        node: ch.node(),
        protocol: format!("{}.{}", h.major, h.minor),
        capabilities: h.caps,
        input_modes: h.input_modes(),
        firmware: Firmware {
            version: string(proto::string::VERSION)?,
            board: string(proto::string::BOARD)?,
            board_label: string(proto::string::BOARD_LABEL)?,
            architecture: None,
            build: None,
            build_type: string(proto::string::BUILD_TYPE)?,
            file_name: None,
        },
        input_mode: ModeSetting::new(Some(i64::from(st.input_mode))),
        active_profile: ProfileName {
            number: st.profile,
            label: string(proto::string::profile_label(st.profile))?,
        },
        enabled_profiles: st.enabled(),
        profile_pending: st.pending(),
        forced_setup: st.forced_setup,
        uptime_ms: st.uptime_ms,
    })
}

/// The firmware build, from `getFirmwareVersion` (or the control
/// channel's strings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn new(value: Option<i64>) -> Self {
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
    /// The saved profile (`profileNumber`): what the pad starts in.
    pub saved: bool,
    /// The profile the pad runs, as the control channel's STATUS reported
    /// it after the reads (back in the gamepad mode, after any restore);
    /// `null` when nothing reported it. A RAM-only `profile switch` makes
    /// this differ from `saved`.
    pub running: Option<bool>,
    /// The same as `saved`, kept for older readers: web-config only knows
    /// the saved profile.
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
    /// The saved profile's number (`profileNumber`); the same as
    /// `saved_profile`, kept for older readers.
    pub active_profile: Option<i64>,
    /// The saved profile's number: what the pad starts in.
    pub saved_profile: Option<i64>,
    /// The profile the pad runs, from the control channel's STATUS once
    /// the trip through web-config is over; `null` when nothing reported
    /// it (no channel, no trip, or STATUS failed). Never guessed from the
    /// saved one.
    pub running_profile: Option<u8>,
    pub profiles: Vec<Profile>,
    pub boot_buttons: Vec<BootButton>,
}

impl Info {
    /// Labels the profile the pad runs, as STATUS reported it (`None`:
    /// unknown, and every row's `running` is `null`).
    pub fn set_running(&mut self, n: Option<u8>) {
        self.running_profile = n;
        for p in &mut self.profiles {
            p.running = n.map(|n| p.number == u32::from(n));
        }
    }
}

/// [`Info`] for the pad behind `client` (five reads).
pub fn info(client: &Client) -> Result<Info> {
    info_with_cache(client, SystemTime::now()).map(|(i, _)| i)
}

fn typed<T: serde::de::DeserializeOwned>(ep: Endpoint, v: &Value) -> Result<T> {
    serde_json::from_value(v.clone())
        .map_err(|e| Error::Protocol(format!("{ep}: unexpected shape: {e}")))
}

/// [`info`], and the [`DeviceCache`] from the same reads.
pub fn info_with_cache(client: &Client, now: SystemTime) -> Result<(Info, DeviceCache)> {
    let t = client.transport();
    let memory = api::try_read_raw(t, Endpoint::GetMemoryReport)?
        .map(|v| typed::<MemoryReport>(Endpoint::GetMemoryReport, &v))
        .transpose()?;
    let raw_options = api::read_raw(t, Endpoint::GetGamepadOptions)?;
    let raw_base = api::read_raw(t, Endpoint::GetPinMappings)?;
    let raw_profiles = api::read_raw(t, Endpoint::GetProfileOptions)?;
    let options: api::GamepadOptions = typed(Endpoint::GetGamepadOptions, &raw_options)?;
    let base: api::PinMappings = typed(Endpoint::GetPinMappings, &raw_base)?;
    let alternatives = typed::<api::ProfileOptions>(Endpoint::GetProfileOptions, &raw_profiles)?
        .alternative_pin_mappings;
    let active = options.profile_number;
    let profiles = std::iter::once(&base)
        .chain(&alternatives)
        .zip(1u32..)
        .map(|(p, number)| Profile {
            number,
            label: p.profile_label.clone(),
            enabled: p.enabled,
            saved: active == Some(i64::from(number)),
            running: None,
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
    let info = Info {
        host: client.host(),
        firmware: client.firmware().into(),
        memory: memory.as_ref().map(Memory::from),
        input_mode: ModeSetting::new(options.input_mode),
        active_profile: active,
        saved_profile: active,
        running_profile: None,
        profiles,
        boot_buttons,
    };
    let cache = DeviceCache::new(
        info.firmware.clone(),
        now,
        raw_options,
        raw_base,
        raw_profiles,
    );
    Ok((info, cache))
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
    backup_with_cache(client, output, now).map(|(r, _)| r)
}

/// [`backup`], and the [`DeviceCache`] the bundle holds (no extra reads).
pub fn backup_with_cache(
    client: &Client,
    output: Option<&Path>,
    now: SystemTime,
) -> Result<(BackupReport, Option<DeviceCache>)> {
    let bundle = backup_bundle(client, now)?;
    let cache = DeviceCache::from_bundle(&bundle);
    let path = match output {
        Some(p) => p.to_path_buf(),
        None => default_backup_path(&bundle, now)?,
    };
    let bytes = write_json(&bundle, &path)?;
    let (endpoints, unavailable): (Vec<_>, Vec<_>) =
        bundle.endpoints.iter().partition(|(_, v)| !v.is_null());
    let names = |v: Vec<(&String, &Value)>| v.into_iter().map(|(k, _)| k.clone()).collect();
    let report = BackupReport {
        path,
        captured_at: bundle.captured_at.clone(),
        firmware: bundle.firmware.clone(),
        endpoints: names(endpoints),
        unavailable: names(unavailable),
        bytes,
    };
    Ok((report, cache))
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

/// `20261017-165703-Haute42COSMOX.json`: UTC time, then the
/// [`board_slug`].
pub fn backup_file_name(bundle: &Bundle, now: SystemTime) -> String {
    format!(
        "{}-{}.json",
        Utc::from_system(now).compact(),
        board_slug(&bundle.firmware)
    )
}

/// The board config with anything but `[A-Za-z0-9_-]` dropped
/// (`Haute42COSMOX`), or `gp2040ce` when there is none: safe in a file name.
pub fn board_slug(f: &Firmware) -> String {
    let board: String = f
        .board
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if board.is_empty() {
        "gp2040ce".into()
    } else {
        board
    }
}

/// Writes `value` as pretty JSON through a temporary file and a rename,
/// so a crash never leaves half a file; returns its size.
pub(crate) fn write_json<T: Serialize>(value: &T, path: &Path) -> Result<u64> {
    let mut text = serde_json::to_string_pretty(value).expect("serialising plain data cannot fail");
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
