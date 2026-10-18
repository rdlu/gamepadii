//! What gamepadii last read from the pad in web-config, kept so that a
//! later command, with the pad in a gamepad mode (no API), can still say
//! which boot button selects a mode and which physical keys the hotkeys
//! are on in the active profile.
//!
//! One file per pad: `$XDG_CACHE_HOME/gamepadii/<board>-<id>.json`
//! (`~/.cache/gamepadii/Haute42COSMOX-433030383835362E.json`). From protocol
//! 1.3, the control channel gives the board ID ([`PadId`]) in every mode,
//! and that is the key. Without it, the key is the USB serial
//! (`Haute42COSMOX-0C0FFEE.json`). Web-config reports no unique serial
//! (every pad says `123456`), but XInput and Xbox One build theirs from
//! the board ID, so the pad is told apart by the serial it had in a
//! gamepad mode. gamepadii learns which pad
//! is in web-config when it sees it go there: it records the serial it saw
//! just before against that web-config enumeration (bus, port and device
//! number, which a replug or a reboot changes), in
//! `webconfig-sessions.json`. A read it can't attribute (the pad was put
//! into web-config by hand, or from a mode with no unique serial) is kept
//! by board config, `<board>.json`, as before, and is attributed when
//! gamepadii sees that same enumeration leave web-config into a mode with
//! a serial. Files from before this (`<board>.json`, no `pad`) still load;
//! the first read attributed to a pad of that board replaces them.
//!
//! Lookups take what is known ([`Key`]): the board (from the control
//! channel or the pad) and the pad. The pad's own file wins, however old;
//! otherwise the newest that can be this pad (a board file, or a pad seen
//! in another mode, whose serial can't be compared), never one of a pad
//! known to be another (another board ID, or the same mode with another
//! serial). A board ID and a serial compare too: the serial comes from the
//! ID ([`PadId::serials_in`]). The hints say how old
//! they are.
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
use crate::discovery::Device;
use crate::error::{Error, Result};
use crate::mode::InputMode;
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
    /// The pad this was read from, when gamepadii knows; `null` keeps it by
    /// board config only. Absent in older files.
    #[serde(default)]
    pub pad: Option<PadId>,
    /// The web-config enumeration a read gamepadii couldn't attribute was
    /// taken in, to attribute it when the pad is seen leaving web-config;
    /// `null` otherwise. Absent in older files.
    #[serde(default)]
    pub read_in: Option<Enumeration>,
}

/// A pad, told apart from others with the same board config.
///
/// The best key is the board ID: the unique ID of the flash chip, which the
/// control channel gives from protocol 1.3 (STRING 0x05) in every mode,
/// web-config too. It is stable, but not guaranteed unique: a flash chip
/// can give a fixed value, so pads with the same chip can share it. When
/// two connected pads give the same one, `port` tells them apart.
///
/// Without it (stock firmware, or a channel older than 1.3), the pad goes by
/// the USB serial that a gamepad mode builds from the same ID. The serial
/// differs between modes, so the mode is part of it. In v0.7.12 and main
/// (`headers/drivers/*/*Descriptors.h`): XInput sends `"0"` and bytes 5 to
/// 7 of the ID in hex (`0C0FFEE`), Xbox One eight letters from all eight
/// bytes and then `8ABCDEFGH`. Every other mode sends none (Keyboard, HID,
/// Switch, PS3, PS4/PS5, ...) or a fixed one (web-config `123456`, Switch
/// Pro `000000000001`, SInput and Switch 2 Pro theirs). `usbDescOverride`
/// replaces XInput's with the user's string, which only counts when it
/// still looks like one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadId {
    /// The board ID, 16 upper-case hex digits (`433030383835362E`); `null`
    /// when the channel did not give it. Absent in older files.
    #[serde(default)]
    pub board_id: Option<String>,
    /// The USB serial in `mode`.
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub mode: Option<InputMode>,
    /// The USB port (`3-2`), only when another connected pad gives the
    /// same board ID; `null` otherwise. Absent in older files.
    #[serde(default)]
    pub port: Option<String>,
}

impl PadId {
    /// `d`'s id from its USB serial, when it is in a mode whose serial is
    /// unique per board.
    pub fn of(d: &Device) -> Option<Self> {
        let serial = d.serial.as_deref()?.trim();
        let mode = d.mode?;
        let unique = match mode {
            InputMode::Xinput => {
                serial.len() == 7
                    && serial.starts_with('0')
                    && serial
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
            }
            InputMode::Xbone => {
                serial.len() == 17
                    && serial.ends_with("8ABCDEFGH")
                    && serial.bytes().take(8).all(|b| b.is_ascii_uppercase())
            }
            _ => false,
        };
        unique.then(|| Self {
            serial: Some(serial.to_string()),
            mode: Some(mode),
            ..Self::default()
        })
    }

    /// A pad known by its board ID only.
    pub fn board(id: &str) -> Self {
        Self {
            board_id: Some(id.to_string()),
            ..Self::default()
        }
    }

    /// `pad` (from [`identify`]) with the board ID, when the channel gave
    /// one.
    pub fn with_board_id(pad: Option<PadId>, id: Option<&str>) -> Option<PadId> {
        match (pad, id) {
            (Some(p), Some(id)) => Some(PadId {
                board_id: Some(id.to_string()),
                ..p
            }),
            (None, Some(id)) => Some(PadId::board(id)),
            (p, None) => p,
        }
    }

    /// The USB serials this pad can send in `mode`: the one it was seen
    /// with, or the ones the firmware makes from the board ID. Xbox One has
    /// two: v0.7.12 takes `'A' + byte % 25`, main `'A' + byte % 26`.
    pub fn serials_in(&self, mode: InputMode) -> Vec<String> {
        if self.mode == Some(mode) {
            return self.serial.iter().cloned().collect();
        }
        let Some(id) = self.board_id.as_deref() else {
            return Vec::new();
        };
        let Some(bytes) = (0..8)
            .map(|i| u8::from_str_radix(id.get(2 * i..2 * i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
        else {
            return Vec::new();
        };
        let letters =
            |m: u8| -> String { bytes.iter().map(|b| char::from(b'A' + b % m)).collect() };
        match mode {
            InputMode::Xinput => vec![format!("0{}", &id[10..16])],
            InputMode::Xbone => [25, 26]
                .map(|m| format!("{}8ABCDEFGH", letters(m)))
                .to_vec(),
            _ => Vec::new(),
        }
    }

    /// Whether `self` and `other` are the same pad: `Some(true)` or
    /// `Some(false)` when the ids can tell, `None` when they can't (two
    /// serials from different modes, say).
    pub fn same(&self, other: &PadId) -> Option<bool> {
        if let (Some(a), Some(b)) = (&self.port, &other.port)
            && a != b
        {
            return Some(false);
        }
        if let (Some(a), Some(b)) = (&self.board_id, &other.board_id) {
            return Some(a == b);
        }
        let mode = self.mode.or(other.mode)?;
        let mode = if self.serial.is_some() && other.serial.is_some() {
            // Two serials compare only in the same mode.
            (self.mode == other.mode).then_some(mode)?
        } else {
            mode
        };
        let (a, b) = (self.serials_in(mode), other.serials_in(mode));
        if a.is_empty() || b.is_empty() {
            return None;
        }
        Some(a.iter().any(|s| b.contains(s)))
    }

    /// The file name part: the board ID (and the port, when it is there),
    /// else the serial; `None` when there is neither.
    fn file_key(&self) -> Option<String> {
        let clean =
            |s: &str| -> String { s.chars().filter(|c| c.is_ascii_alphanumeric()).collect() };
        match (&self.board_id, &self.port, &self.serial) {
            (Some(id), Some(port), _) => Some(format!("{}-{}", clean(id), clean(port))),
            (Some(id), None, _) => Some(clean(id)),
            (None, _, Some(serial)) => Some(clean(serial)),
            (None, _, None) => None,
        }
    }
}

/// One enumeration of a device: the same bus, port and device number. A
/// replug or a reboot gets a new device number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enumeration {
    pub bus: u16,
    pub port: String,
    pub devnum: u16,
}

impl Enumeration {
    pub fn of(d: &Device) -> Option<Self> {
        Some(Self {
            bus: d.bus?,
            port: d.port.clone(),
            devnum: d.devnum?,
        })
    }
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
            pad: None,
            read_in: None,
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
            pad: None,
            read_in: None,
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

    /// Profile `slot.profile` (2–6) is now `slot` in flash: its label,
    /// enabled state and pins, in `profile_options` as `getProfileOptions`
    /// has them. Other keys of each entry stay. Whether anything changed.
    pub fn set_slot(&mut self, slot: &crate::channel::Slot) -> bool {
        let Some(i) = usize::from(slot.profile).checked_sub(2).filter(|i| *i < 5) else {
            return false;
        };
        let Value::Object(o) = &mut self.profile_options else {
            return false;
        };
        let Value::Array(alts) = o
            .entry("alternativePinMappings")
            .or_insert_with(|| Value::Array(Vec::new()))
        else {
            return false;
        };
        while alts.len() <= i {
            alts.push(Value::Object(serde_json::Map::new()));
        }
        let Value::Object(entry) = &mut alts[i] else {
            return false;
        };
        let before = entry.clone();
        entry.insert("profileLabel".into(), slot.label.clone().into());
        entry.insert("enabled".into(), slot.enabled().into());
        for (gpio, p) in slot.pins.iter().enumerate() {
            let pin = entry
                .entry(format!("pin{gpio:02}"))
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(pin) = pin {
                pin.insert("action".into(), p.action.into());
                pin.insert("customButtonMask".into(), p.button_mask.into());
                pin.insert("customDpadMask".into(), p.dpad_mask.into());
            }
        }
        *entry != before
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

    /// Says which pad this was read from: the one gamepadii saw go into
    /// the web-config enumeration `wc` (see [`identify`]), else that
    /// enumeration, to attribute it later.
    pub fn attribute(&mut self, dir: Option<&Path>, wc: &Device) {
        self.pad = identify(dir, wc);
        self.read_in = match self.pad {
            Some(_) => None,
            None => Enumeration::of(wc),
        };
    }

    /// `Haute42COSMOX-433030383835362E.json` for a pad with a board ID,
    /// `Haute42COSMOX-0C0FFEE.json` for one known by its serial, else
    /// `Haute42COSMOX.json`.
    pub fn file_name(&self) -> String {
        let board = self.board();
        match self.pad.as_ref().and_then(PadId::file_key) {
            Some(key) => format!("{board}-{key}.json"),
            None => format!("{board}.json"),
        }
    }

    /// The board config, as [`commands::board_slug`].
    pub fn board(&self) -> String {
        commands::board_slug(&self.firmware)
    }
}

/// What a lookup knows about the pad: its board config (as
/// [`commands::board_slug`]) and which pad it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Key {
    pub board: Option<String>,
    pub pad: Option<PadId>,
}

impl Key {
    /// The board from `fw` (when it names one), the pad as given.
    pub fn new(fw: &Firmware, pad: Option<PadId>) -> Self {
        Self {
            board: fw.board.as_ref().map(|_| commands::board_slug(fw)),
            pad,
        }
    }

    /// Only the board (`Haute42COSMOX`).
    pub fn board(board: &str) -> Self {
        Self {
            board: Some(board.to_string()),
            pad: None,
        }
    }
}

/// Which pad `d` is: from its serial in a gamepad mode that has a unique
/// one, or, in web-config, from the session recorded when gamepadii saw it
/// go there ([`record_session`]).
pub fn identify(dir: Option<&Path>, d: &Device) -> Option<PadId> {
    if d.mode == Some(InputMode::Webconfig) {
        return session_pad(dir?, d);
    }
    PadId::of(d)
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

/// Writes the cache into `dir`, replacing that pad's (or, unattributed,
/// that board's) last one. The first cache attributed to a pad replaces
/// the board's file from before pads were told apart, and the first one
/// with a board ID replaces that pad's file by serial.
pub fn store(dir: &Path, cache: &DeviceCache) -> Result<PathBuf> {
    let path = dir.join(cache.file_name());
    commands::write_json(cache, &path)?;
    if let Some(pad) = &cache.pad {
        let legacy = dir.join(format!("{}.json", cache.board()));
        if read(&legacy).is_some_and(|c| c.pad.is_none() && c.read_in.is_none()) {
            let _ = fs::remove_file(legacy);
        }
        if pad.board_id.is_some() {
            for (old, c) in all(dir) {
                let by_serial = c
                    .pad
                    .as_ref()
                    .is_some_and(|p| p.board_id.is_none() && pad.same(p) == Some(true));
                if old != path && c.board() == cache.board() && by_serial {
                    let _ = fs::remove_file(old);
                }
            }
        }
    }
    Ok(path)
}

fn read(path: &Path) -> Option<DeviceCache> {
    let b = fs::read(path).ok()?;
    serde_json::from_slice::<DeviceCache>(&b)
        .ok()
        .filter(|c| c.schema == CACHE_SCHEMA)
}

/// `board`'s unattributed cache (`<board>.json`, by
/// [`commands::board_slug`]), if there is one.
pub fn load(dir: &Path, board: &str) -> Option<DeviceCache> {
    read(&dir.join(format!("{board}.json")))
}

/// Every readable cache in `dir`, with its file.
fn all(dir: &Path) -> Vec<(PathBuf, DeviceCache)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| read(&p).map(|c| (p, c)))
        .collect();
    // Ties on `captured_at` go the same way on every filesystem.
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn newest<'a>(
    it: impl Iterator<Item = &'a (PathBuf, DeviceCache)>,
) -> Option<&'a (PathBuf, DeviceCache)> {
    it.max_by(|a, b| a.1.captured_at.cmp(&b.1.captured_at))
}

/// The cache for `key`, and its file (see the module doc): the pad's own;
/// else the newest that can be this pad, on its board when the board is
/// known (and, as before pads were told apart, on any board when that
/// board has none).
fn resolve(dir: &Path, key: &Key) -> Option<(PathBuf, DeviceCache)> {
    let all = all(dir);
    let on_board = |c: &DeviceCache| key.board.as_ref().is_none_or(|b| c.board() == *b);
    let same = |c: &DeviceCache| match (&c.pad, &key.pad) {
        (Some(theirs), Some(ours)) => theirs.same(ours),
        _ => None,
    };
    if let Some(own) = newest(
        all.iter()
            .filter(|(_, c)| on_board(c) && same(c) == Some(true)),
    ) {
        return Some(own.clone());
    }
    // Another pad's own cache: another board ID, or the same mode with
    // another serial.
    let other = |c: &DeviceCache| same(c) == Some(false);
    let maybe: Vec<_> = all.iter().filter(|(_, c)| !other(c)).collect();
    newest(maybe.iter().copied().filter(|(_, c)| on_board(c)))
        .or_else(|| newest(maybe.into_iter()))
        .cloned()
}

/// The cache for this pad (see the module doc); `None` when there is none
/// it can be.
pub fn load_for(dir: &Path, key: &Key) -> Option<DeviceCache> {
    resolve(dir, key).map(|(_, c)| c)
}

/// Applies `f` to the cache [`load_for`] gives `key` and writes it back
/// when `f` changed it. No cache (the pad was never seen in web-config) is
/// not an error: there is nothing to keep current. Neither is knowing
/// neither the board nor the pad: nothing is touched then.
pub fn update(dir: &Path, key: &Key, f: impl FnOnce(&mut DeviceCache) -> bool) -> Result<()> {
    if key.board.is_none() && key.pad.is_none() {
        return Ok(());
    }
    let Some((path, mut c)) = resolve(dir, key) else {
        return Ok(());
    };
    if f(&mut c) {
        commands::write_json(&c, &path)?;
    }
    Ok(())
}

/// Records what the control channel said: the pad runs profile `n`
/// (saved too with `saved`, after `profile switch --save`).
pub fn record_profile(dir: &Path, key: &Key, n: u8, saved: bool, now: SystemTime) -> Result<()> {
    update(dir, key, |c| c.set_profile(n, saved, now))
}

/// Records a slot that WRITE_SLOT saved to flash (`profile write --save`).
pub fn record_slot(dir: &Path, key: &Key, slot: &crate::channel::Slot) -> Result<()> {
    update(dir, key, |c| c.set_slot(slot))
}

/// Records that gamepadii restarted the pad (it runs its saved profile).
pub fn record_restart(dir: &Path, key: &Key) -> Result<()> {
    update(dir, key, DeviceCache::restarted)
}

/// The newest readable cache in `dir` (by `captured_at`), if any.
pub fn load_latest(dir: &Path) -> Option<DeviceCache> {
    newest(all(dir).iter()).map(|(_, c)| c.clone())
}

/// Which pad gamepadii saw go into each web-config enumeration, by
/// `bus-port`: `$XDG_CACHE_HOME/gamepadii/webconfig-sessions.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sessions {
    pub schema: u32,
    pub ports: BTreeMap<String, Session>,
}

/// The pad, as it was seen just before it came back as the web-config
/// enumeration `webconfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub pad: PadId,
    pub webconfig: Enumeration,
}

pub const SESSIONS_FILE: &str = "webconfig-sessions.json";

fn sessions(dir: &Path) -> Sessions {
    fs::read(dir.join(SESSIONS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Sessions>(&b).ok())
        .filter(|s| s.schema == CACHE_SCHEMA)
        .unwrap_or_default()
}

/// gamepadii saw `pad` (in a gamepad mode) come back as `wc`, in
/// web-config: until `wc` re-enumerates, it is that pad. Replaces what
/// that port had.
pub fn record_session(dir: &Path, pad: &PadId, wc: &Device) -> Result<()> {
    let Some(at) = Enumeration::of(wc) else {
        return Ok(());
    };
    let mut s = sessions(dir);
    let port = format!("{}-{}", at.bus, at.port);
    let session = Session {
        pad: pad.clone(),
        webconfig: at,
    };
    if s.ports.get(&port) == Some(&session) {
        return Ok(());
    }
    s.schema = CACHE_SCHEMA;
    s.ports.insert(port, session);
    commands::write_json(&s, &dir.join(SESSIONS_FILE)).map(|_| ())
}

/// The pad gamepadii saw go into the web-config enumeration `wc`, if it
/// did (and `wc` hasn't re-enumerated since).
pub fn session_pad(dir: &Path, wc: &Device) -> Option<PadId> {
    let at = Enumeration::of(wc)?;
    sessions(dir)
        .ports
        .into_values()
        .find(|s| s.webconfig == at)
        .map(|s| s.pad)
}

/// gamepadii saw the web-config enumeration `wc` leave as `pad`: the reads
/// it couldn't attribute, taken in `wc`, were that pad's. Moves them to
/// the pad's file.
pub fn adopt(dir: &Path, wc: &Device, pad: &PadId) -> Result<()> {
    let Some(at) = Enumeration::of(wc) else {
        return Ok(());
    };
    for (path, mut c) in all(dir) {
        if c.pad.is_none() && c.read_in.as_ref() == Some(&at) {
            c.pad = Some(pad.clone());
            c.read_in = None;
            let new = store(dir, &c)?;
            if new != path {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(())
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

/// The connections whose HELLO showed no channel ([`crate::channel::Probe::Stock`]),
/// so that `status` in a polling loop (a waybar module, say) does not send
/// vendor requests again to a real Xbox 360 pad, or to any other device
/// that uses one of GP2040-CE's ids. A replug or a reboot makes a new
/// connection, which is asked again.
/// `$XDG_CACHE_HOME/gamepadii/no-channel.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoChannel {
    pub schema: u32,
    /// The newest last.
    pub connections: Vec<Enumeration>,
}

pub const NO_CHANNEL_FILE: &str = "no-channel.json";

/// How many connections [`NoChannel`] keeps.
const NO_CHANNEL_MAX: usize = 32;

fn no_channel(dir: &Path) -> NoChannel {
    fs::read(dir.join(NO_CHANNEL_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<NoChannel>(&b).ok())
        .filter(|c| c.schema == CACHE_SCHEMA)
        .unwrap_or_default()
}

/// Whether `d`'s connection showed no channel before.
pub fn no_channel_seen(dir: &Path, d: &Device) -> bool {
    Enumeration::of(d).is_some_and(|at| no_channel(dir).connections.contains(&at))
}

/// Records that `d`'s connection showed no channel.
pub fn record_no_channel(dir: &Path, d: &Device) -> Result<()> {
    let Some(at) = Enumeration::of(d) else {
        return Ok(());
    };
    let mut seen = no_channel(dir);
    if seen.connections.contains(&at) {
        return Ok(());
    }
    seen.schema = CACHE_SCHEMA;
    // A new connection on a port replaces the old one there.
    seen.connections
        .retain(|c| !(c.bus == at.bus && c.port == at.port));
    seen.connections.push(at);
    let extra = seen.connections.len().saturating_sub(NO_CHANNEL_MAX);
    seen.connections.drain(..extra);
    let _ = fs::create_dir_all(dir);
    commands::write_json(&seen, &dir.join(NO_CHANNEL_FILE)).map(|_| ())
}
