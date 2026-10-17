//! What gamepadii last read from the pad in web-config, kept so that a
//! later command, with the pad in a gamepad mode (no API), can still say
//! which boot button selects a mode and which physical keys the hotkeys
//! are on in the active profile.
//!
//! One file per board config: `$XDG_CACHE_HOME/gamepadii/<board>.json`
//! (`~/.cache/gamepadii/Haute42COSMOX.json`). Web-config mode reports no
//! unique serial (every pad says `123456`), so two pads with the same
//! board config share a file, and in a gamepad mode, where the board isn't
//! known at all, the newest file is used. The hints say how old they are.
//!
//! The control channel can't read the settings, but its STATUS says which
//! profile the pad runs. Whenever it answers (`status`, the flows, `profile
//! switch`), that is recorded here as the running profile, next to the
//! saved one (`profileNumber` in the options): a RAM-only `profile switch`
//! changes only the running one, `--save` both, and a restart puts the
//! running one back on the saved one.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{self, Endpoint, GamepadOptions, PinMappings, ProfileOptions};
use crate::buttons::Profiles;
use crate::channel::Hello;
use crate::client::Client;
use crate::commands::{self, Bundle, Firmware};
use crate::error::{Error, Result};
use crate::timestamp::Utc;

/// Bumped on any breaking change to the cache file.
pub const CACHE_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCache {
    pub schema: u32,
    /// RFC 3339, UTC.
    pub captured_at: String,
    pub firmware: Firmware,
    /// `getGamepadOptions`, `getPinMappings` and `getProfileOptions`, as
    /// the pad sent them.
    pub gamepad_options: Value,
    pub pin_mappings: Value,
    pub profile_options: Value,
    /// The profile the control channel last reported running, when it
    /// answered after this was captured; `null` means the saved one
    /// (`profileNumber` in `gamepad_options`). Absent in older files.
    #[serde(default)]
    pub running_profile: Option<RunningProfile>,
}

/// A profile the pad was seen running, from the control channel's STATUS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningProfile {
    /// 1 to 6.
    pub number: u8,
    /// When gamepadii first saw it running this profile (RFC 3339, UTC).
    pub seen_at: String,
}

impl DeviceCache {
    /// Reads the three endpoints the hints need.
    pub fn capture(client: &Client, now: SystemTime) -> Result<Self> {
        let t = client.transport();
        Ok(Self::new(
            client.firmware().into(),
            now,
            api::read_raw(t, Endpoint::GetGamepadOptions)?,
            api::read_raw(t, Endpoint::GetPinMappings)?,
            api::read_raw(t, Endpoint::GetProfileOptions)?,
        ))
    }

    pub fn new(
        firmware: Firmware,
        now: SystemTime,
        gamepad_options: Value,
        pin_mappings: Value,
        profile_options: Value,
    ) -> Self {
        Self {
            schema: CACHE_SCHEMA,
            captured_at: Utc::from_system(now).rfc3339(),
            firmware,
            gamepad_options,
            pin_mappings,
            profile_options,
            running_profile: None,
        }
    }

    /// From a backup bundle, which has all three.
    pub fn from_bundle(b: &Bundle) -> Option<Self> {
        let get = |name: &str| b.endpoints.get(name).filter(|v| !v.is_null()).cloned();
        Some(Self {
            schema: CACHE_SCHEMA,
            captured_at: b.captured_at.clone(),
            firmware: b.firmware.clone(),
            gamepad_options: get("getGamepadOptions")?,
            pin_mappings: get("getPinMappings")?,
            profile_options: get("getProfileOptions")?,
            running_profile: None,
        })
    }

    /// The saved profile (`profileNumber`), as last read or saved.
    pub fn saved_profile(&self) -> Option<i64> {
        self.gamepad_options
            .get("profileNumber")
            .and_then(Value::as_i64)
    }

    /// The profile the pad runs, as far as the cache knows: the one the
    /// control channel last reported, else the saved one.
    pub fn active_profile(&self) -> Option<i64> {
        self.running_profile
            .as_ref()
            .map(|r| i64::from(r.number))
            .or_else(|| self.saved_profile())
    }

    /// Records that the pad runs profile `n` (and, with `saved`, that it is
    /// the saved one too); whether anything changed.
    pub fn set_profile(&mut self, n: u8, saved: bool, now: SystemTime) -> bool {
        let mut changed = false;
        if self.running_profile.as_ref().map(|r| r.number) != Some(n) {
            self.running_profile = Some(RunningProfile {
                number: n,
                seen_at: Utc::from_system(now).rfc3339(),
            });
            changed = true;
        }
        if saved
            && self.saved_profile() != Some(i64::from(n))
            && let Value::Object(o) = &mut self.gamepad_options
        {
            o.insert("profileNumber".into(), n.into());
            changed = true;
        }
        changed
    }

    /// The pad restarted: whatever ran from RAM is gone, it runs the saved
    /// profile again. Whether anything changed.
    pub fn restarted(&mut self) -> bool {
        self.running_profile.take().is_some()
    }

    pub fn options(&self) -> Option<GamepadOptions> {
        serde_json::from_value(self.gamepad_options.clone()).ok()
    }

    pub fn profiles(&self) -> Option<Profiles> {
        let base: PinMappings = serde_json::from_value(self.pin_mappings.clone()).ok()?;
        let alts: ProfileOptions = serde_json::from_value(self.profile_options.clone()).ok()?;
        Some(Profiles::new(base, alts))
    }

    /// `Haute42COSMOX.json`.
    pub fn file_name(&self) -> String {
        format!("{}.json", commands::board_slug(&self.firmware))
    }
}

/// `$XDG_CACHE_HOME/gamepadii` (`~/.cache/gamepadii`); `None` without a
/// home.
pub fn cache_dir() -> Option<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))?;
    Some(base.join("gamepadii"))
}

/// Writes the cache into `dir`, replacing that board's last one.
pub fn store(dir: &Path, cache: &DeviceCache) -> Result<PathBuf> {
    let path = dir.join(cache.file_name());
    commands::write_json(cache, &path)?;
    Ok(path)
}

/// `board`'s cache (by [`commands::board_slug`]), if there is one.
pub fn load(dir: &Path, board: &str) -> Option<DeviceCache> {
    let b = fs::read(dir.join(format!("{board}.json"))).ok()?;
    serde_json::from_slice::<DeviceCache>(&b)
        .ok()
        .filter(|c| c.schema == CACHE_SCHEMA)
}

/// `board`'s cache when the board is known, else the newest.
pub fn load_for(dir: &Path, board: Option<&str>) -> Option<DeviceCache> {
    board
        .and_then(|b| load(dir, b))
        .or_else(|| load_latest(dir))
}

/// Applies `f` to `board`'s cache and writes it back when `f` changed it.
/// No cache for that board (it was never seen in web-config) is not an
/// error: there is nothing to keep current.
pub fn update(dir: &Path, board: &str, f: impl FnOnce(&mut DeviceCache) -> bool) -> Result<()> {
    let Some(mut c) = load(dir, board) else {
        return Ok(());
    };
    if f(&mut c) {
        store(dir, &c)?;
    }
    Ok(())
}

/// Records what the control channel said: the pad runs profile `n`
/// (saved too with `saved`, after `profile switch --save`).
pub fn record_profile(dir: &Path, board: &str, n: u8, saved: bool, now: SystemTime) -> Result<()> {
    update(dir, board, |c| c.set_profile(n, saved, now))
}

/// Records that gamepadii restarted the pad (it runs its saved profile).
pub fn record_restart(dir: &Path, board: &str) -> Result<()> {
    update(dir, board, DeviceCache::restarted)
}

/// The newest readable cache in `dir` (by `captured_at`), if any.
pub fn load_latest(dir: &Path) -> Option<DeviceCache> {
    let entries = fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<DeviceCache>(&b).ok())
        .filter(|c| c.schema == CACHE_SCHEMA)
        .max_by(|a, b| a.captured_at.cmp(&b.captured_at))
}

/// Stores `cache` when there is a directory for it. The cache is a
/// convenience: failing to write it is reported, never fatal.
pub fn store_quietly(dir: Option<&Path>, cache: &DeviceCache) -> Option<Error> {
    store(dir?, cache).err()
}

/// Which boards have been seen answering the control channel, so that a
/// later `EACCES` on the device node can say "run `gamepadii setup udev`"
/// for a pad that has the channel, and stay quiet for stock firmware.
/// `$XDG_CACHE_HOME/gamepadii/control-channel.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelsSeen {
    pub schema: u32,
    /// Board config (`Haute42COSMOX`) → what its channel offered.
    pub boards: BTreeMap<String, ChannelRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub major: u8,
    pub minor: u8,
    pub capabilities: Vec<String>,
}

impl ChannelRecord {
    pub fn new(h: &Hello) -> Self {
        Self {
            major: h.major,
            minor: h.minor,
            capabilities: h.caps.names().into_iter().map(String::from).collect(),
        }
    }
}

pub const CHANNELS_FILE: &str = "control-channel.json";

pub fn channels_seen(dir: &Path) -> ChannelsSeen {
    fs::read(dir.join(CHANNELS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<ChannelsSeen>(&b).ok())
        .filter(|c| c.schema == CACHE_SCHEMA)
        .unwrap_or_default()
}

/// Whether any pad has been seen with the channel.
pub fn channel_known(dir: Option<&Path>) -> bool {
    dir.is_some_and(|d| !channels_seen(d).boards.is_empty())
}

/// Records `board`'s channel; writes only when something changed, since
/// `status` (a waybar module, say) probes often.
pub fn record_channel(dir: &Path, board: &str, hello: &Hello) -> Result<()> {
    let mut seen = channels_seen(dir);
    let rec = ChannelRecord::new(hello);
    if seen.boards.get(board) == Some(&rec) {
        return Ok(());
    }
    seen.schema = CACHE_SCHEMA;
    seen.boards.insert(board.to_string(), rec);
    commands::write_json(&seen, &dir.join(CHANNELS_FILE)).map(|_| ())
}
