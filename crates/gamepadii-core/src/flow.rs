//! The guided flows: `mode`, `webconfig`, `reboot` with a wait, `profile
//! switch`, and the trip through web-config that `info` and `backup` take
//! from a gamepad mode ([`through_webconfig`]). They poll USB through a
//! [`Scan`] and time through a [`Clock`], open the web-config client and
//! the control channel through closures, and report progress through a
//! [`Progress`], so every branch runs in tests against a simulated pad
//! with no real sleeps.
//!
//! In a gamepad mode the flows first send HELLO on the control channel
//! (`crate::channel`). When the firmware answers, they switch the mode or
//! the profile, or reboot, over it: no web-config, no button press. Stock
//! GP2040-CE has no channel: the flows tell the user which buttons to hold
//! (with key-by-key hints from the [`DeviceCache`] when there is one), wait
//! for the pad to re-enumerate, and carry on from there.
//!
//! Whenever the channel answers STATUS, the profile it reports goes into the
//! device cache, so the key hints stay right after a `profile switch`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::api::{self, FirmwareVersion, OptionsWrite, RebootAck};
use crate::buttons::{Button, Combo};
use crate::cache::{self, DeviceCache, Key, PadId};
use crate::channel::{self, Caps, Channel, OpenControl, Probe, proto};
use crate::client::{self, Client};
use crate::commands::{self, Firmware, ModeSetting, ProfileName};
use crate::discovery::{Device, Scan};
use crate::error::{Error, Result};
use crate::mode::{BootMode, Confidence, InputMode};
use crate::wait::{Clock, Poll, wait_for};

/// How long each wait lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// For the user to switch the pad into web-config (or a mode).
    pub user: Duration,
    /// For the pad to come back on USB after a reboot.
    pub reboot: Duration,
    /// For the web server to answer once the pad is in web-config (its
    /// DHCP lease comes first).
    pub ready: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            user: Duration::from_secs(60),
            reboot: Duration::from_secs(10),
            ready: Duration::from_secs(15),
        }
    }
}

/// What a flow runs against.
pub struct Env<'a> {
    pub scan: &'a dyn Scan,
    pub clock: &'a dyn Clock,
    /// Opens the web-config client, with no USB check (the flow has looked).
    pub open: &'a dyn Fn() -> Result<Client>,
    /// Opens a pad's control channel; `None` never tries (`--no-channel`,
    /// or a platform without usbfs).
    pub control: Option<&'a OpenControl<'a>>,
    /// Where the [`DeviceCache`] lives; `None` neither reads nor writes it.
    pub cache_dir: Option<PathBuf>,
    /// Where to back up before a write; `None` skips the backup.
    pub backups_dir: Option<PathBuf>,
    /// The web configurator's address, for people: `http://192.168.7.1/`.
    pub url: String,
    pub timeouts: Timeouts,
    /// How often to look at USB.
    pub poll: Duration,
}

/// What a flow is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaiting {
    /// The user, to switch the pad (to web-config, or to `target`).
    User { target: Option<InputMode> },
    /// The pad, to come back after a reboot.
    Reboot(BootMode),
    /// The web server, once the pad is in web-config.
    Server,
}

/// Progress, for a spinner or a log.
#[derive(Debug)]
pub enum Step<'a> {
    /// The user has to act: show them how. Sent once, before waiting.
    Guide(&'a Guide),
    Waiting {
        awaiting: Awaiting,
        elapsed: Duration,
        timeout: Duration,
    },
    BackedUp(&'a Path),
    /// About to write `inputMode`.
    Writing(InputMode),
    Rebooting(BootMode),
    /// The device cache could not be written (not fatal).
    CacheFailed(&'a Error),
    /// A restart dropped the RAM-only profile it ran: switching back to it
    /// (RAM only).
    RestoringProfile(u8),
    /// That failed (not fatal: the trip's note says so too).
    RestoreFailed {
        profile: u8,
        error: &'a Error,
    },
}

pub trait Progress {
    fn step(&mut self, step: Step<'_>);
}

/// No progress output.
pub struct Silent;

impl Progress for Silent {
    fn step(&mut self, _: Step<'_>) {}
}

/// A device as the reports show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Seen {
    pub mode: Option<InputMode>,
    pub candidates: Vec<InputMode>,
    pub confidence: Confidence,
    /// `045e:028e`.
    pub usb_id: String,
}

impl Seen {
    /// `XInput`, or `PS4 or PS5`.
    pub fn label(&self) -> String {
        match self.mode {
            Some(m) => m.label().to_string(),
            None => self
                .candidates
                .iter()
                .map(|m| m.label())
                .collect::<Vec<_>>()
                .join(" or "),
        }
    }
}

impl From<&Device> for Seen {
    fn from(d: &Device) -> Self {
        Self {
            mode: d.mode,
            candidates: d.candidates.clone(),
            confidence: d.confidence,
            usb_id: d.usb_id(),
        }
    }
}

/// Buttons to hold, and where they are in the active profile when the
/// cache says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hold {
    pub buttons: Vec<Button>,
    /// `null` without a cache.
    pub keys: Option<Combo>,
}

impl Hold {
    fn new(buttons: &[Button], cache: Option<&Cached>) -> Self {
        Self {
            buttons: buttons.to_vec(),
            keys: cache.map(|c| Combo::new(&c.profiles, c.active, buttons)),
        }
    }

    /// `S2+B3+B4 (Start+X+Y)`.
    pub fn label(&self) -> String {
        let join = |f: fn(Button) -> &'static str| {
            self.buttons
                .iter()
                .map(|b| f(*b))
                .collect::<Vec<_>>()
                .join("+")
        };
        let (names, xbox) = (join(Button::name), join(Button::xbox));
        if names == xbox {
            names
        } else {
            format!("{names} ({xbox})")
        }
    }

    /// The key-by-key sentence, capitalised, when the profile moves them.
    fn physical(&self) -> Option<String> {
        let s = self.keys.as_ref()?.physical()?;
        let mut c = s.chars();
        let first = c.next()?.to_uppercase().collect::<String>();
        Some(format!("{first}{}.", c.as_str()))
    }
}

/// How to get the pad from where it is to web-config (or to `target`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Guide {
    pub current: Seen,
    pub target: Option<InputMode>,
    /// The boot button mapped to `target`: hold it while plugging in, and
    /// the pad boots in that mode and saves it. Needs the cache.
    pub boot_button: Option<Hold>,
    /// Hold at plug-in for web-config; `null` when the pad locks it
    /// (forced setup mode).
    pub plug_in: Option<Hold>,
    /// Hold for 4 s while running; `null` in BOOTSEL (the firmware isn't).
    pub hotkey: Option<Hold>,
    /// When the cache the key hints come from was taken.
    pub cached_at: Option<String>,
    /// Where the active profile the key hints are in came from; `null`
    /// without a cache.
    pub profile_from: Option<ProfileFrom>,
    /// When the pad was seen running it, for [`ProfileFrom::Recorded`].
    pub profile_seen_at: Option<String>,
    /// Why the control channel, which would have made this unnecessary,
    /// couldn't be used (no udev rule); `null` for stock firmware.
    pub channel_hint: Option<String>,
}

/// Where the active profile behind the key hints came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFrom {
    /// The control channel's STATUS, just now.
    Channel,
    /// What the channel last reported, kept in the cache.
    Recorded,
    /// The saved profile, as read in web-config (or saved with `--save`).
    Saved,
}

/// The parts of a cache the guide uses.
struct Cached {
    profiles: crate::buttons::Profiles,
    active: i64,
    from: ProfileFrom,
    seen_at: Option<String>,
    options: api::GamepadOptions,
    captured_at: String,
}

impl Cached {
    /// `live`: the profile the control channel says the pad runs now.
    fn from(c: &DeviceCache, live: Option<u8>) -> Option<Self> {
        let options = c.options()?;
        let recorded = c.running_profile.as_ref();
        let (active, from, seen_at) = match (live, recorded) {
            (Some(n), _) => (i64::from(n), ProfileFrom::Channel, None),
            (None, Some(r)) => (
                i64::from(r.number),
                ProfileFrom::Recorded,
                Some(r.seen_at.clone()),
            ),
            (None, None) => (
                options.profile_number.unwrap_or(1),
                ProfileFrom::Saved,
                None,
            ),
        };
        Some(Self {
            profiles: c.profiles()?,
            active,
            from,
            seen_at,
            options,
            captured_at: c.captured_at.clone(),
        })
    }

    fn forced_setup(&self) -> i64 {
        self.options
            .extra
            .get("forcedSetupMode")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    }
}

/// `FORCED_SETUP_MODE_*` (`proto/enums.proto`).
const LOCK_MODE_SWITCH: i64 = 1;
const LOCK_WEB_CONFIG: i64 = 2;
const LOCK_BOTH: i64 = 3;

/// The modes v0.7.12's boot-button switch (`GP2040::getBootAction`) knows;
/// a button mapped to any other does nothing.
fn boot_button_selects(mode: InputMode) -> bool {
    mode.proto_value().is_some_and(|v| v <= 15)
}

impl Guide {
    pub fn new(current: &Device, target: Option<InputMode>, cache: Option<&DeviceCache>) -> Self {
        Self::in_profile(current, target, cache, None)
    }

    /// [`Guide::new`], with the key hints in profile `live` when the control
    /// channel said which one the pad runs (else the cache's).
    pub fn in_profile(
        current: &Device,
        target: Option<InputMode>,
        cache: Option<&DeviceCache>,
        live: Option<u8>,
    ) -> Self {
        let cached = cache.and_then(|c| Cached::from(c, live));
        let c = cached.as_ref();
        let locks = c.map_or(0, Cached::forced_setup);
        let boot_button = target
            .filter(|t| boot_button_selects(*t))
            .filter(|_| !matches!(locks, LOCK_MODE_SWITCH | LOCK_BOTH))
            .and_then(|t| {
                let opts = &c?.options;
                let (name, _) = opts
                    .boot_buttons()
                    .into_iter()
                    .find(|(_, v)| *v == t.proto_value().map(i64::from))?;
                Some(Hold::new(&[Button::from_name(name)?], c))
            });
        let in_bootsel = current.mode == Some(InputMode::Bootsel);
        Self {
            current: current.into(),
            target,
            boot_button,
            plug_in: (!matches!(locks, LOCK_WEB_CONFIG | LOCK_BOTH))
                .then(|| Hold::new(&[Button::S2], c)),
            hotkey: (!in_bootsel).then(|| Hold::new(&[Button::S2, Button::B3, Button::B4], c)),
            cached_at: c.map(|c| c.captured_at.clone()),
            profile_from: c.map(|c| c.from),
            profile_seen_at: c.and_then(|c| c.seen_at.clone()),
            channel_hint: None,
        }
    }

    fn with_hint(mut self, hint: Option<String>) -> Self {
        self.channel_hint = hint;
        self
    }

    /// The instructions, a sentence per line.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.channel_hint.clone());
        if let (Some(hold), Some(target)) = (&self.boot_button, self.target) {
            out.push(format!(
                "No write needed: unplug the pad, hold {} (only that button) and plug it \
                 back in; it starts in {} mode and keeps it.",
                hold.label(),
                target.label()
            ));
            out.extend(hold.physical());
        }
        let mut ways = Vec::new();
        if let Some(h) = &self.plug_in {
            ways.push(format!("hold {} while plugging the pad in", h.label()));
        }
        if let Some(h) = &self.hotkey {
            ways.push(format!(
                "hold {} for 4 seconds while it runs (only those buttons)",
                h.label()
            ));
        }
        let intro = match (self.boot_button.is_some(), self.target) {
            (true, _) => "Or switch it to web-config",
            (false, Some(_)) => "First switch it to web-config",
            (false, None) => "To switch it to web-config",
        };
        if ways.is_empty() {
            out.push(format!(
                "{intro}: this pad locks web-config at plug-in (forced setup mode); \
                 use its display menu."
            ));
        } else {
            out.push(format!("{intro}: {}.", ways.join(", or ")));
        }
        for h in [&self.plug_in, &self.hotkey].into_iter().flatten() {
            out.extend(h.physical());
        }
        out.push(match (&self.cached_at, self.profile_from) {
            (Some(t), Some(ProfileFrom::Channel)) => format!(
                "Key names come from the profiles gamepadii read on {t}, in the profile \
                 the pad reports running now."
            ),
            (Some(t), Some(ProfileFrom::Recorded)) => format!(
                "Key names come from what gamepadii read on {t}, in the profile the pad \
                 last reported running (on {}); if it changed since, they may be off.",
                self.profile_seen_at.as_deref().unwrap_or("an unknown date")
            ),
            (Some(t), _) => format!(
                "Key names come from what gamepadii read on {t}; if the active profile \
                 changed since, they may be off."
            ),
            (None, _) => "These are logical buttons, so on a remapped profile they may be \
                     other keys; once the pad is in web-config, gamepadii remembers its \
                     profiles and names the keys next time."
                .to_string(),
        });
        out
    }

    pub fn text(&self) -> String {
        self.lines().join(" ")
    }
}

/// Which of `devices` the flows act on: the one in web-config, else the
/// first. Front ends show that one.
pub fn picked(devices: &[Device]) -> Option<usize> {
    devices
        .iter()
        .position(|d| d.mode == Some(InputMode::Webconfig))
        .or((!devices.is_empty()).then_some(0))
}

fn pick(devices: Vec<Device>) -> Result<Device> {
    match picked(&devices) {
        Some(i) => Ok(devices.into_iter().nth(i).expect("index from picked")),
        None => Err(Error::NotFound(
            "no GP2040-CE device on USB (is it plugged in?)".into(),
        )),
    }
}

fn in_gamepad_mode(d: &Device) -> bool {
    !matches!(d.mode, Some(InputMode::Webconfig | InputMode::Bootsel))
}

/// What a HELLO to the pad found.
#[derive(Default)]
struct Probed {
    ch: Option<Channel>,
    /// Why the channel couldn't be used, when that is worth saying in the
    /// guide.
    hint: Option<String>,
    /// The pad's device cache: the board config from the channel's
    /// strings, and the pad from its serial (or its web-config session).
    key: Key,
    /// STATUS, when the channel answered it.
    status: Option<channel::Status>,
}

impl Probed {
    /// The profile the pad runs, from STATUS.
    fn profile(&self) -> Option<u8> {
        self.status.as_ref().map(|s| s.profile)
    }
}

/// The pad's control channel, or (when it couldn't be used and that is
/// worth saying) a sentence for the guide. Pads seen with the channel are
/// recorded in the cache dir, so a later `EACCES` gets the udev hint, and
/// the profile STATUS reports is kept in the device cache.
fn channel(env: &Env, d: &Device) -> Probed {
    let pad = cache::identify(env.cache_dir.as_deref(), d);
    let unprobed = Probed {
        key: Key {
            board: None,
            pad: pad.clone(),
        },
        ..Probed::default()
    };
    let Some(open) = env.control else {
        return unprobed;
    };
    if d.mode == Some(InputMode::Bootsel) {
        return unprobed;
    }
    let hint = match channel::probe(open, d) {
        Probe::Found(ch) => {
            let board = ch.string(proto::string::BOARD).ok().flatten();
            let firmware = Firmware {
                board,
                ..Firmware::default()
            };
            commands::remember(env.cache_dir.as_deref(), &ch, &firmware);
            let key = Key::new(&firmware, pad);
            let status = ch.status().ok();
            if let Some(st) = &status {
                let dir = env.cache_dir.as_deref();
                let _ = commands::remember_profile(dir, &key, st.profile, false, env.clock.wall());
            }
            return Probed {
                ch: Some(ch),
                hint: None,
                key,
                status,
            };
        }
        Probe::NoAccess { node } if cache::channel_known(env.cache_dir.as_deref()) => {
            Some(format!(
                "gamepadii could do this over the pad's control channel, but has no \
                 permission to use {node}: run `gamepadii setup udev` once, then replug the pad."
            ))
        }
        Probe::Newer { major, .. } => Some(format!("{}.", commands::newer_protocol(major))),
        Probe::NoAccess { .. } | Probe::Absent(_) => None,
    };
    Probed { hint, ..unprobed }
}

/// The cache for the guide: this pad's (see `cache`).
fn guide_cache(env: &Env, p: &Probed) -> Option<DeviceCache> {
    cache::load_for(env.cache_dir.as_deref()?, &p.key)
}

/// gamepadii saw `from` (in a gamepad mode) come back as `wc`, in
/// web-config: records which pad that is, so what is read there goes into
/// its cache.
fn entered_webconfig(env: &Env, from: &Device, wc: &Device) {
    if let (Some(dir), Some(pad)) = (env.cache_dir.as_deref(), PadId::of(from)) {
        let _ = cache::record_session(dir, &pad, wc);
    }
}

/// gamepadii saw `wc` (in web-config) come back as `got`, in a gamepad
/// mode: what it read there and couldn't attribute was `got`'s.
fn left_webconfig(env: &Env, wc: &Device, got: &Device) {
    if let (Some(dir), Some(pad)) = (env.cache_dir.as_deref(), PadId::of(got)) {
        let _ = cache::adopt(dir, wc, &pad);
    }
}

/// The channel on a pad that has just re-enumerated. udev may need a
/// moment to apply the `uaccess` ACL to the new node, so this retries
/// briefly.
fn channel_after_reboot(env: &Env, d: &Device) -> Option<Channel> {
    let open = env.control?;
    wait_for(
        env.clock,
        Poll {
            interval: Duration::from_millis(200),
            timeout: Duration::from_secs(2),
        },
        || channel::probe(open, d).found(),
        |_| {},
    )
}

/// `d` as the channel reports it: `confirmed`, in the mode it says it runs.
fn confirmed(d: &Device, running: Option<InputMode>) -> Seen {
    Seen {
        mode: running.or(d.mode),
        candidates: d.candidates.clone(),
        confidence: Confidence::Confirmed,
        usb_id: d.usb_id(),
    }
}

fn running(st: &channel::Status) -> Option<InputMode> {
    InputMode::from_proto(st.input_mode.into())
}

/// The options from the pad's cache, for the `usbOverrideID` check.
fn cached_options(env: &Env, key: &Key) -> Option<api::GamepadOptions> {
    env.cache_dir
        .as_deref()
        .and_then(|d| cache::load_for(d, key))
        .and_then(|c| c.options())
}

/// Whether the device shows `target`'s ids.
fn shows(target: InputMode, d: &Device) -> bool {
    d.mode == Some(target) || d.candidates.contains(&target)
}

fn identity(d: &Device) -> (Option<u16>, Option<u16>, String) {
    (d.bus, d.devnum, d.port.clone())
}

/// Polls until a device that `accept`s shows up as a new enumeration: not
/// `before` (same bus, port and device number) unless `before` has been
/// seen to go away first.
fn wait_reenumerated(
    env: &Env,
    before: &Device,
    timeout: Duration,
    awaiting: Awaiting,
    progress: &mut dyn Progress,
    accept: impl Fn(&Device) -> bool,
) -> Option<Device> {
    let old = identity(before);
    let mut gone = false;
    wait_for(
        env.clock,
        Poll {
            interval: env.poll,
            timeout,
        },
        || {
            let devices = env.scan.scan();
            if !devices.iter().any(|d| identity(d) == old) {
                gone = true;
            }
            devices
                .into_iter()
                .find(|d| accept(d) && (gone || identity(d) != old))
        },
        |elapsed| {
            progress.step(Step::Waiting {
                awaiting,
                elapsed,
                timeout,
            })
        },
    )
}

/// Opens the client, retrying until the web server answers.
fn connect_ready(env: &Env, progress: &mut dyn Progress) -> Result<Client> {
    let mut last = None;
    let timeout = env.timeouts.ready;
    wait_for(
        env.clock,
        Poll {
            interval: Duration::from_millis(500),
            timeout,
        },
        || match (env.open)() {
            Ok(c) => Some(c),
            Err(e) => {
                last = Some(e);
                None
            }
        },
        |elapsed| {
            progress.step(Step::Waiting {
                awaiting: Awaiting::Server,
                elapsed,
                timeout,
            })
        },
    )
    .ok_or_else(|| last.expect("wait_for tried at least once"))
}

/// Refreshes the cache from a connected pad (`wc`, in web-config);
/// returns it.
fn refresh_cache(
    env: &Env,
    client: &Client,
    wc: &Device,
    progress: &mut dyn Progress,
) -> Result<DeviceCache> {
    let c = DeviceCache::capture(client, env.clock.wall())?;
    if let Some(e) = store_read(env, c.clone(), wc) {
        progress.step(Step::CacheFailed(&e));
    }
    Ok(c)
}

/// Keeps a cache read from `wc` (in web-config), attributed to its pad
/// when gamepadii knows which one it is; a failure is for a warning.
fn store_read(env: &Env, mut c: DeviceCache, wc: &Device) -> Option<Error> {
    let dir = env.cache_dir.as_deref();
    c.attribute(dir, wc);
    cache::store_quietly(dir, &c)
}

/// `(major, minor, patch)` from `v0.7.12`.
fn version(fw: &FirmwareVersion) -> Option<(u32, u32, u32)> {
    let v = fw.version.as_deref()?.trim().trim_start_matches('v');
    let mut parts = v.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Whether the firmware has `mode`: P5General, SInput and Switch 2 Pro
/// (16 and up) came after v0.7.12. An unknown version gets the benefit of
/// the doubt.
pub fn firmware_has(fw: &FirmwareVersion, mode: InputMode) -> bool {
    match (mode.proto_value(), version(fw)) {
        (Some(v), Some(ver)) if v > 15 && v != 255 => ver > (0, 7, 12),
        _ => true,
    }
}

/// The modes `gamepadii mode` can switch to: every firmware mode but
/// web-config and BOOTSEL.
pub fn targets() -> impl Iterator<Item = InputMode> {
    InputMode::ALL
        .into_iter()
        .filter(|m| !matches!(m, InputMode::Webconfig | InputMode::Bootsel))
}

/// XInput and Generic HID use the custom USB ids when `usbOverrideID` is
/// on (v0.7.12's `XInputDriver` and `HIDDriver`), which gamepadii can't
/// recognise.
fn custom_ids(opts: &api::GamepadOptions, target: InputMode) -> Option<String> {
    let on = opts
        .extra
        .get("usbOverrideID")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|v| v != 0);
    if !on || !matches!(target, InputMode::Xinput | InputMode::Generic) {
        return None;
    }
    let id = |k: &str| {
        opts.extra
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("????")
            .to_ascii_lowercase()
    };
    Some(format!("{}:{}", id("usbVendorID"), id("usbProductID")))
}

/// How `mode` got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// It was in the mode already; nothing done.
    Already,
    /// Through web-config: `inputMode` written (if it differed), rebooted.
    WebConfig,
    /// The user held the target's boot button: nothing written.
    BootButton,
    /// SET_INPUT_MODE on the control channel: the pad saved the mode and
    /// rebooted itself.
    Control,
}

/// Which way a command reached the pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// The web configurator's HTTP API.
    WebConfig,
    /// The USB control channel.
    Control,
}

/// `gamepadii mode <mode>`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModeReport {
    pub target: InputMode,
    pub via: Via,
    pub before: Seen,
    /// How it came back; `null` when it couldn't be seen (custom USB ids).
    pub after: Option<Seen>,
    /// Whether `after` shows the target's ids.
    pub verified: bool,
    /// Whether it waited for the user to switch the pad.
    pub waited_for_user: bool,
    /// The saved input mode the pad had in web-config.
    pub saved_before: Option<ModeSetting>,
    /// The `setGamepadOptions` write; `null` when none was needed.
    pub write: Option<OptionsWrite>,
    /// The backup taken before the write.
    pub backup: Option<PathBuf>,
    pub reboot: Option<RebootAck>,
    pub note: Option<String>,
}

/// Switches the pad to `target`, guiding the user when it is in a gamepad
/// mode. Without `wait`, a pad that needs the user is
/// [`Error::NeedsUser`] with the instructions.
pub fn mode(
    env: &Env,
    target: InputMode,
    wait: bool,
    progress: &mut dyn Progress,
) -> Result<ModeReport> {
    if !targets().any(|m| m == target) {
        return Err(Error::Usage(format!(
            "{} is not an input mode; use `gamepadii webconfig` or `gamepadii reboot`",
            target.label()
        )));
    }
    let pad = pick(env.scan.scan())?;
    if pad.mode == Some(target) {
        return Ok(ModeReport {
            target,
            via: Via::Already,
            before: (&pad).into(),
            after: Some((&pad).into()),
            verified: true,
            waited_for_user: false,
            saved_before: None,
            write: None,
            backup: None,
            reboot: None,
            note: pad.note.clone(),
        });
    }
    let mut probed = Probed::default();
    if in_gamepad_mode(&pad) {
        probed = channel(env, &pad);
        if let Some(ch) = probed
            .ch
            .as_ref()
            .filter(|c| c.caps().has(Caps::SET_INPUT_MODE))
        {
            return via_control(env, target, &pad, ch, &probed.key, progress);
        }
    }
    if pad.mode == Some(InputMode::Webconfig) {
        return via_webconfig(env, target, &pad, (&pad).into(), false, progress);
    }
    let cache = guide_cache(env, &probed);
    let guide = Guide::in_profile(&pad, Some(target), cache.as_ref(), probed.profile())
        .with_hint(probed.hint);
    if !wait {
        return Err(Error::NeedsUser(format!(
            "the pad is in {} mode. {}",
            guide.current.label(),
            guide.text()
        )));
    }
    progress.step(Step::Guide(&guide));
    let got = wait_reenumerated(
        env,
        &pad,
        env.timeouts.user,
        Awaiting::User {
            target: Some(target),
        },
        progress,
        |d| d.mode == Some(InputMode::Webconfig) || shows(target, d),
    )
    .ok_or_else(|| gave_up(env.timeouts.user, &guide))?;
    if got.mode == Some(InputMode::Webconfig) {
        entered_webconfig(env, &pad, &got);
        return via_webconfig(env, target, &got, (&pad).into(), true, progress);
    }
    Ok(ModeReport {
        target,
        via: Via::BootButton,
        before: (&pad).into(),
        after: Some((&got).into()),
        verified: true,
        waited_for_user: true,
        saved_before: None,
        write: None,
        backup: None,
        reboot: None,
        note: got.note.clone(),
    })
}

fn gave_up(after: Duration, guide: &Guide) -> Error {
    Error::NeedsUser(format!(
        "stopped waiting after {} s; the pad is still in {} mode. {}",
        after.as_secs(),
        guide.current.label(),
        guide.text()
    ))
}

/// `mode` over the control channel: SET_INPUT_MODE (the pad saves the mode
/// and reboots into it), then the re-enumeration and STATUS to check.
fn via_control(
    env: &Env,
    target: InputMode,
    pad: &Device,
    ch: &Channel,
    key: &Key,
    progress: &mut dyn Progress,
) -> Result<ModeReport> {
    if !ch.hello().boots(target) {
        let fw = ch.string(proto::string::VERSION).ok().flatten();
        return Err(Error::Usage(format!(
            "this firmware ({}) has no {} mode",
            fw.as_deref().unwrap_or("unknown version"),
            target.label()
        )));
    }
    let st = ch.status()?;
    let before = confirmed(pad, running(&st));
    let mut report = ModeReport {
        target,
        via: Via::Already,
        before: before.clone(),
        after: Some(before),
        verified: true,
        waited_for_user: false,
        saved_before: None,
        write: None,
        backup: None,
        reboot: None,
        note: None,
    };
    if running(&st) == Some(target) {
        return Ok(report);
    }
    progress.step(Step::Writing(target));
    let ack = ch.set_input_mode(target)?;
    commands::remember_restart(env.cache_dir.as_deref(), key);
    progress.step(Step::Rebooting(BootMode::Gamepad));
    report.via = Via::Control;
    report.after = None;
    report.verified = false;
    report.reboot = Some(ack);
    if let Some(ids) = cached_options(env, key).and_then(|o| custom_ids(&o, target)) {
        report.note = Some(format!(
            "usbOverrideID is on, so it comes back as {ids}, which gamepadii can't \
             recognise; not verified"
        ));
        return Ok(report);
    }
    let what = format!("{} is saved", target.label());
    let got = wait_reenumerated(
        env,
        pad,
        env.timeouts.reboot,
        Awaiting::Reboot(BootMode::Gamepad),
        progress,
        in_gamepad_mode,
    )
    .ok_or_else(|| not_back(env.timeouts.reboot, &what))?;
    if !shows(target, &got) {
        return Err(Error::CheckFailed(format!(
            "{what}, but the pad came back as {} ({}), not {}",
            Seen::from(&got).label(),
            got.usb_id(),
            target.label()
        )));
    }
    let after = match channel_after_reboot(env, &got) {
        Some(ch) => {
            let st = ch.status()?;
            let dir = env.cache_dir.as_deref();
            let _ = commands::remember_profile(dir, key, st.profile, false, env.clock.wall());
            let now = running(&st);
            if now != Some(target) {
                return Err(Error::CheckFailed(format!(
                    "{what}, but the pad says it runs {}",
                    now.map_or("an unknown mode", InputMode::label)
                )));
            }
            confirmed(&got, now)
        }
        None => {
            report.note = got.note.clone();
            (&got).into()
        }
    };
    report.after = Some(after);
    report.verified = true;
    Ok(report)
}

/// The web-config half of `mode`: `pad` is the pad in web-config, `before`
/// what it was when the command started.
fn via_webconfig(
    env: &Env,
    target: InputMode,
    pad: &Device,
    before: Seen,
    waited_for_user: bool,
    progress: &mut dyn Progress,
) -> Result<ModeReport> {
    let client = connect_ready(env, progress)?;
    if !firmware_has(client.firmware(), target) {
        return Err(Error::Usage(format!(
            "this firmware ({}) has no {} mode",
            client
                .firmware()
                .version
                .as_deref()
                .unwrap_or("unknown version"),
            target.label()
        )));
    }
    let cached = refresh_cache(env, &client, pad, progress)?;
    let options = cached
        .options()
        .ok_or_else(|| Error::Protocol("getGamepadOptions: unexpected shape".into()))?;
    let saved = ModeSetting::new(options.input_mode);
    let proto = i64::from(target.proto_value().expect("targets have a proto value"));
    let (write, backup) = if options.input_mode == Some(proto) {
        (None, None)
    } else {
        let backup = match &env.backups_dir {
            Some(dir) => {
                let now = env.clock.wall();
                let bundle = commands::backup_bundle(&client, now)?;
                let path = dir.join(commands::backup_file_name(&bundle, now));
                commands::write_json(&bundle, &path)?;
                progress.step(Step::BackedUp(&path));
                Some(path)
            }
            None => None,
        };
        progress.step(Step::Writing(target));
        let w = api::set_gamepad_options(client.transport(), |o| o.input_mode = Some(proto))?;
        (Some(w), backup)
    };
    progress.step(Step::Rebooting(BootMode::Gamepad));
    let ack = api::reboot(client.transport(), BootMode::Gamepad)?;
    let mut report = ModeReport {
        target,
        via: Via::WebConfig,
        before,
        after: None,
        verified: false,
        waited_for_user,
        saved_before: Some(saved),
        write,
        backup,
        reboot: Some(ack),
        note: None,
    };
    if let Some(ids) = custom_ids(&options, target) {
        report.note = Some(format!(
            "usbOverrideID is on, so it comes back as {ids}, which gamepadii can't \
             recognise; not verified"
        ));
        return Ok(report);
    }
    let what = if report.write.is_some() {
        format!("{} is saved", target.label())
    } else {
        format!("{} was already saved", target.label())
    };
    let got = wait_reenumerated(
        env,
        pad,
        env.timeouts.reboot,
        Awaiting::Reboot(BootMode::Gamepad),
        progress,
        in_gamepad_mode,
    )
    .ok_or_else(|| not_back(env.timeouts.reboot, &what))?;
    if !shows(target, &got) {
        return Err(Error::CheckFailed(format!(
            "{what}, but the pad came back as {} ({}), not {}",
            Seen::from(&got).label(),
            got.usb_id(),
            target.label()
        )));
    }
    left_webconfig(env, pad, &got);
    report.after = Some((&got).into());
    report.verified = true;
    report.note = got.note.clone();
    Ok(report)
}

fn not_back(after: Duration, what: &str) -> Error {
    Error::io(
        format!("{what} and the pad was rebooted"),
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("it hasn't come back on USB after {} s", after.as_secs()),
        ),
    )
}

/// One choice in `gamepadii mode` with no argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeChoice {
    pub mode: InputMode,
    pub label: &'static str,
    /// Its boot button, from the pad or the cache.
    pub boot_button: Option<Button>,
    /// Whether the firmware has it; `null` without a firmware version.
    pub supported: Option<bool>,
    pub current: bool,
}

/// `gamepadii mode`: where the pad is and where it can go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeStatus {
    pub current: Seen,
    /// The saved input mode: from the pad in web-config, else the cache.
    pub saved: Option<ModeSetting>,
    /// `pad`, `cache`, or `null` when unknown.
    pub saved_from: Option<&'static str>,
    /// When the cache was taken, when it was used.
    pub cached_at: Option<String>,
    /// The firmware version, from the pad or the cache.
    pub firmware: Option<String>,
    pub modes: Vec<ModeChoice>,
}

pub fn mode_status(env: &Env, progress: &mut dyn Progress) -> Result<ModeStatus> {
    let pad = pick(env.scan.scan())?;
    let mut current: Seen = (&pad).into();
    let mut boots = None;
    let mut probed = Probed::default();
    if in_gamepad_mode(&pad) {
        probed = channel(env, &pad);
        if let Some(ch) = &probed.ch {
            if let Some(st) = &probed.status {
                current = confirmed(&pad, running(st));
            }
            boots = Some(*ch.hello());
        }
    }
    let (cache, from) = if pad.mode == Some(InputMode::Webconfig) {
        let client = connect_ready(env, progress)?;
        (
            Some(refresh_cache(env, &client, &pad, progress)?),
            Some("pad"),
        )
    } else {
        let c = guide_cache(env, &probed);
        let from = c.as_ref().map(|_| "cache");
        (c, from)
    };
    let options = cache.as_ref().and_then(DeviceCache::options);
    let fw = cache.as_ref().map(|c| FirmwareVersion {
        version: c.firmware.version.clone(),
        ..Default::default()
    });
    let modes = targets()
        .map(|m| ModeChoice {
            mode: m,
            label: m.label(),
            boot_button: options.as_ref().and_then(|o| {
                o.boot_buttons()
                    .into_iter()
                    .find(|(_, v)| *v == m.proto_value().map(i64::from))
                    .and_then(|(name, _)| Button::from_name(name))
                    .filter(|_| boot_button_selects(m))
            }),
            supported: match &boots {
                Some(h) => Some(h.boots(m)),
                None => fw
                    .as_ref()
                    .filter(|f| f.version.is_some())
                    .map(|f| firmware_has(f, m)),
            },
            current: current.mode == Some(m),
        })
        .collect();
    Ok(ModeStatus {
        current,
        saved: options.as_ref().map(|o| ModeSetting::new(o.input_mode)),
        saved_from: options.as_ref().and(from),
        cached_at: (from == Some("cache"))
            .then(|| cache.as_ref().map(|c| c.captured_at.clone()))
            .flatten(),
        firmware: cache.and_then(|c| c.firmware.version),
        modes,
    })
}

/// `gamepadii webconfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebconfigReport {
    pub url: String,
    pub device: Seen,
    pub waited_for_user: bool,
    /// How it got there: `already`, `control` (REBOOT on the control
    /// channel) or `user` (the guided button press).
    pub via: WebconfigVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebconfigVia {
    Already,
    Control,
    User,
}

/// Gets the pad into web-config (guiding the user if needed) and returns
/// the URL. Refreshes the cache when the server answers.
pub fn webconfig(env: &Env, wait: bool, progress: &mut dyn Progress) -> Result<WebconfigReport> {
    let pad = pick(env.scan.scan())?;
    let probed = if in_gamepad_mode(&pad) {
        channel(env, &pad)
    } else {
        Probed::default()
    };
    let (pad, via) = if pad.mode == Some(InputMode::Webconfig) {
        (pad, WebconfigVia::Already)
    } else if let Some(ch) = probed.ch.as_ref().filter(|c| c.caps().has(Caps::REBOOT)) {
        progress.step(Step::Rebooting(BootMode::Webconfig));
        ch.reboot(BootMode::Webconfig)?;
        commands::remember_restart(env.cache_dir.as_deref(), &probed.key);
        let got = wait_reenumerated(
            env,
            &pad,
            env.timeouts.reboot,
            Awaiting::Reboot(BootMode::Webconfig),
            progress,
            |d| d.mode == Some(InputMode::Webconfig),
        )
        .ok_or_else(|| not_back(env.timeouts.reboot, "asked for web-config"))?;
        entered_webconfig(env, &pad, &got);
        (got, WebconfigVia::Control)
    } else {
        let cache = guide_cache(env, &probed);
        let guide =
            Guide::in_profile(&pad, None, cache.as_ref(), probed.profile()).with_hint(probed.hint);
        if !wait {
            return Err(Error::NeedsUser(format!(
                "the pad is in {} mode. {}",
                guide.current.label(),
                guide.text()
            )));
        }
        progress.step(Step::Guide(&guide));
        let got = wait_reenumerated(
            env,
            &pad,
            env.timeouts.user,
            Awaiting::User { target: None },
            progress,
            |d| d.mode == Some(InputMode::Webconfig),
        )
        .ok_or_else(|| gave_up(env.timeouts.user, &guide))?;
        entered_webconfig(env, &pad, &got);
        (got, WebconfigVia::User)
    };
    // The cache is a bonus: a pad whose server isn't up yet still has a URL.
    if let Ok(client) = connect_ready(env, progress) {
        let _ = refresh_cache(env, &client, &pad, progress);
    }
    Ok(WebconfigReport {
        url: env.url.clone(),
        device: (&pad).into(),
        waited_for_user: via == WebconfigVia::User,
        via,
    })
}

/// `gamepadii reboot`, waiting for the pad to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebootReport {
    /// Where the request went: `http://192.168.7.1`, or the device node
    /// over the control channel.
    pub host: String,
    pub mode: BootMode,
    /// `web_config` or `control`.
    pub via: Route,
    pub ack: RebootAck,
    /// Whether it watched USB for the pad to come back.
    pub waited: bool,
    /// What it came back as; `null` when it didn't wait.
    pub after: Option<Seen>,
    /// The input mode expected for `gamepad` (the saved one); web-config or
    /// BOOTSEL for those.
    pub expected: Option<InputMode>,
    /// Whether `after` matched `expected`; `null` when either is unknown.
    pub verified: Option<bool>,
    pub note: Option<String>,
}

/// Reboots the pad wherever it is: over HTTP in web-config, over the
/// control channel in a gamepad mode. BOOTSEL isn't offered on the channel
/// (issue #2), so from a gamepad mode it goes through web-config: REBOOT
/// into web-config, then `/api/reboot`. A pad with neither is
/// [`Error::NeedsUser`], with how to get into web-config.
pub fn reboot_pad(
    env: &Env,
    mode: BootMode,
    wait: bool,
    progress: &mut dyn Progress,
) -> Result<RebootReport> {
    let pad = pick(env.scan.scan())?;
    if pad.mode == Some(InputMode::Webconfig) {
        let client = connect_ready(env, progress)?;
        return reboot(env, &client, mode, wait, progress);
    }
    let probed = if in_gamepad_mode(&pad) {
        channel(env, &pad)
    } else {
        Probed::default()
    };
    match probed.ch.as_ref().filter(|c| c.caps().has(Caps::REBOOT)) {
        Some(ch) => reboot_via_control(env, &pad, ch, &probed.key, mode, wait, progress),
        None => Err(not_in_webconfig(&pad, probed.hint)),
    }
}

/// A pad in a gamepad mode (or BOOTSEL) with no way into web-config but the
/// user's: how to switch, after why the channel couldn't do it.
fn not_in_webconfig(pad: &Device, hint: Option<String>) -> Error {
    let msg = client::not_in_webconfig(pad);
    Error::NeedsUser(match hint {
        Some(h) => format!("{h} Or: {msg}"),
        None => msg,
    })
}

fn reboot_via_control(
    env: &Env,
    pad: &Device,
    ch: &Channel,
    key: &Key,
    mode: BootMode,
    wait: bool,
    progress: &mut dyn Progress,
) -> Result<RebootReport> {
    if mode == BootMode::Bootsel && !ch.caps().has(Caps::REBOOT_BOOTSEL) {
        progress.step(Step::Rebooting(BootMode::Webconfig));
        ch.reboot(BootMode::Webconfig)?;
        commands::remember_restart(env.cache_dir.as_deref(), key);
        wait_reenumerated(
            env,
            pad,
            env.timeouts.reboot,
            Awaiting::Reboot(BootMode::Webconfig),
            progress,
            |d| d.mode == Some(InputMode::Webconfig),
        )
        .ok_or_else(|| {
            not_back(
                env.timeouts.reboot,
                "asked for web-config, on the way to BOOTSEL",
            )
        })?;
        let client = connect_ready(env, progress)?;
        let mut r = reboot(env, &client, BootMode::Bootsel, wait, progress)?;
        let via = "BOOTSEL isn't offered over the control channel, so gamepadii went \
                   through web-config";
        r.note = Some(match r.note.take() {
            Some(n) => format!("{via}; {n}"),
            None => via.to_string(),
        });
        return Ok(r);
    }
    let expected = match mode {
        BootMode::Gamepad => ch
            .status()
            .ok()
            .and_then(|st| running(&st))
            .filter(|m| *m != InputMode::Webconfig),
        BootMode::Webconfig => Some(InputMode::Webconfig),
        BootMode::Bootsel => Some(InputMode::Bootsel),
    };
    progress.step(Step::Rebooting(mode));
    let ack = ch.reboot(mode)?;
    commands::remember_restart(env.cache_dir.as_deref(), key);
    let report = RebootReport {
        host: ch.node(),
        mode,
        via: Route::Control,
        ack,
        waited: false,
        after: None,
        expected,
        verified: None,
        note: None,
    };
    let custom = match mode {
        BootMode::Gamepad => cached_options(env, key)
            .zip(expected)
            .and_then(|(o, m)| custom_ids(&o, m)),
        _ => None,
    };
    watch_reboot(env, wait.then_some(pad), custom, report, progress)
}

/// Reboots the pad in web-config (behind `client`) into `mode`; with
/// `wait`, watches USB until it comes back and checks what it came back as.
pub fn reboot(
    env: &Env,
    client: &Client,
    mode: BootMode,
    wait: bool,
    progress: &mut dyn Progress,
) -> Result<RebootReport> {
    let before = env
        .scan
        .scan()
        .into_iter()
        .find(|d| d.mode == Some(InputMode::Webconfig));
    let options = match mode {
        BootMode::Gamepad => api::gamepad_options(client.transport()).ok(),
        _ => None,
    };
    let expected = match mode {
        BootMode::Gamepad => options
            .as_ref()
            .and_then(|o| o.input_mode)
            .and_then(|v| u32::try_from(v).ok())
            .and_then(InputMode::from_proto),
        BootMode::Webconfig => Some(InputMode::Webconfig),
        BootMode::Bootsel => Some(InputMode::Bootsel),
    };
    progress.step(Step::Rebooting(mode));
    let ack = api::reboot(client.transport(), mode)?;
    let dir = env.cache_dir.as_deref();
    let pad = before.as_ref().and_then(|d| cache::identify(dir, d));
    commands::remember_restart(dir, &Key::new(&client.firmware().into(), pad));
    let mut report = RebootReport {
        host: client.host(),
        mode,
        via: Route::WebConfig,
        ack,
        waited: false,
        after: None,
        expected,
        verified: None,
        note: None,
    };
    let custom = options
        .as_ref()
        .zip(expected)
        .and_then(|(o, m)| custom_ids(o, m));
    if wait && before.is_none() {
        report.note = Some(
            "the pad isn't on this machine's USB in web-config, so gamepadii can't watch it come back"
                .into(),
        );
        return Ok(report);
    }
    watch_reboot(
        env,
        before.as_ref().filter(|_| wait),
        custom,
        report,
        progress,
    )
}

/// The second half of a reboot: watch `before` re-enumerate (unless it is
/// `None`: no wait) and check it came back as `report.expected`.
fn watch_reboot(
    env: &Env,
    before: Option<&Device>,
    custom: Option<String>,
    mut report: RebootReport,
    progress: &mut dyn Progress,
) -> Result<RebootReport> {
    let Some(before) = before else {
        return Ok(report);
    };
    if let Some(ids) = custom {
        report.note = Some(format!(
            "usbOverrideID is on, so it comes back as {ids}, which gamepadii can't recognise"
        ));
        return Ok(report);
    }
    let mode = report.mode;
    let accept = |d: &Device| match mode {
        BootMode::Gamepad => in_gamepad_mode(d),
        BootMode::Webconfig => d.mode == Some(InputMode::Webconfig),
        BootMode::Bootsel => d.mode == Some(InputMode::Bootsel),
    };
    let got = wait_reenumerated(
        env,
        before,
        env.timeouts.reboot,
        Awaiting::Reboot(mode),
        progress,
        accept,
    )
    .ok_or_else(|| not_back(env.timeouts.reboot, &format!("asked for {}", mode.label())))?;
    if before.mode == Some(InputMode::Webconfig) && in_gamepad_mode(&got) {
        left_webconfig(env, before, &got);
    } else if in_gamepad_mode(before) && got.mode == Some(InputMode::Webconfig) {
        entered_webconfig(env, before, &got);
    }
    report.waited = true;
    report.after = Some((&got).into());
    report.note = got.note.clone();
    if let Some(m) = report.expected {
        let ok = shows(m, &got);
        report.verified = Some(ok);
        if !ok {
            return Err(Error::CheckFailed(format!(
                "rebooted into {}, expecting {}, but it came back as {} ({})",
                mode.label(),
                m.label(),
                Seen::from(&got).label(),
                got.usb_id()
            )));
        }
    }
    Ok(report)
}

/// How `info` or `backup` got a pad in a gamepad mode into web-config, and
/// back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trip {
    /// Always `control`: REBOOT into web-config on the control channel.
    pub via: Route,
    /// The device node it went through.
    pub host: String,
    /// What the pad was in before.
    pub before: Seen,
    /// The profile it ran before (from STATUS).
    pub profile: Option<u8>,
    /// The profile switched back to (RAM only, over the channel) when the
    /// restart had dropped it: it ran a profile it hadn't saved. `null`
    /// when nothing needed restoring, restoring failed, or with `--stay`.
    pub restored_profile: Option<u8>,
    /// The profile STATUS shows once the trip is over: back in the gamepad
    /// mode (after any restore), or in web-config with `--stay`. `null`
    /// when the channel didn't answer then.
    pub running_profile: Option<u8>,
    /// `--stay`: it was left in web-config.
    pub stayed: bool,
    /// How it was sent back: `web_config` (`/api/reboot {"bootMode":0}`), or
    /// `control` (REBOOT on the channel, when the web configurator didn't
    /// answer); `null` with `--stay`.
    pub back_via: Option<Route>,
    /// How it came back; `null` with `--stay` or custom USB ids.
    pub after: Option<Seen>,
    /// Whether it came back in the mode it was in.
    pub verified: bool,
    pub note: Option<String>,
}

/// `e` with `prefix` in front of its message, same kind and exit code.
fn prefixed(e: Error, prefix: &str) -> Error {
    match e {
        Error::Io { context, source } => Error::io(format!("{prefix}: {context}"), source),
        Error::NotFound(m) => Error::NotFound(format!("{prefix}: {m}")),
        Error::Usage(m) => Error::Usage(format!("{prefix}: {m}")),
        Error::Protocol(m) => Error::Protocol(format!("{prefix}: {m}")),
        Error::NeedsUser(m) => Error::NeedsUser(format!("{prefix}: {m}")),
        Error::CheckFailed(m) => Error::CheckFailed(format!("{prefix}: {m}")),
    }
}

/// `e` with `suffix` after its message, same kind and exit code.
fn suffixed(e: Error, suffix: &str) -> Error {
    match e {
        Error::Io { context, source } => Error::io(
            context,
            std::io::Error::new(source.kind(), format!("{source}{suffix}")),
        ),
        Error::NotFound(m) => Error::NotFound(format!("{m}{suffix}")),
        Error::Usage(m) => Error::Usage(format!("{m}{suffix}")),
        Error::Protocol(m) => Error::Protocol(format!("{m}{suffix}")),
        Error::NeedsUser(m) => Error::NeedsUser(format!("{m}{suffix}")),
        Error::CheckFailed(m) => Error::CheckFailed(format!("{m}{suffix}")),
    }
}

/// Runs `work` against the pad's web configurator: `info` and `backup`.
///
/// In web-config that is all (`null` trip). In a gamepad mode with the
/// control channel, it reboots the pad into web-config first (REBOOT),
/// waits for `cafe:4028` and the HTTP server, runs `work`, then reboots it
/// back (`/api/reboot {"bootMode":0}`) and checks it returns in the mode it
/// was in, unless `stay`. Whatever fails on the way, it still tries to send
/// the pad back, and says how that went. `done` says what `work` achieved,
/// for an error on the way back. Without the channel it is
/// [`Error::NeedsUser`] with how to switch, as before.
pub fn through_webconfig<T>(
    env: &Env,
    stay: bool,
    progress: &mut dyn Progress,
    work: impl FnOnce(&Client) -> Result<T>,
    done: impl FnOnce(&T) -> String,
) -> Result<(T, Option<Trip>)> {
    visit(env, stay, false, progress, |c, _| work(c), done)
}

/// [`through_webconfig`], with the pad as it is in web-config for `work`.
/// With `hello`, a pad found already in web-config gets a HELLO too, for
/// [`Client::fork_fixes`] (a trip has had one on the way in).
fn visit<T>(
    env: &Env,
    stay: bool,
    hello: bool,
    progress: &mut dyn Progress,
    work: impl FnOnce(&Client, &Device) -> Result<T>,
    done: impl FnOnce(&T) -> String,
) -> Result<(T, Option<Trip>)> {
    let pad = pick(env.scan.scan())?;
    if pad.mode == Some(InputMode::Webconfig) {
        let client = (env.open)()?;
        let client = if hello {
            with_channel(env, client, &pad)
        } else {
            client
        };
        return work(&client, &pad).map(|r| (r, None));
    }
    let probed = if in_gamepad_mode(&pad) {
        channel(env, &pad)
    } else {
        Probed::default()
    };
    let Some(ch) = probed.ch.as_ref().filter(|c| c.caps().has(Caps::REBOOT)) else {
        return Err(not_in_webconfig(&pad, probed.hint));
    };
    let before = match &probed.status {
        Some(st) => confirmed(&pad, running(st)),
        None => (&pad).into(),
    };
    let mut trip = Trip {
        via: Route::Control,
        host: ch.node(),
        before,
        profile: probed.profile(),
        restored_profile: None,
        running_profile: None,
        stayed: stay,
        back_via: None,
        after: None,
        verified: false,
        note: None,
    };
    progress.step(Step::Rebooting(BootMode::Webconfig));
    ch.reboot(BootMode::Webconfig)?;
    commands::remember_restart(env.cache_dir.as_deref(), &probed.key);
    let wc = wait_reenumerated(
        env,
        &pad,
        env.timeouts.reboot,
        Awaiting::Reboot(BootMode::Webconfig),
        progress,
        |d| d.mode == Some(InputMode::Webconfig),
    )
    .ok_or_else(|| {
        suffixed(
            not_back(env.timeouts.reboot, "asked for web-config"),
            "; nothing was read. If it shows up in web-config later, \
             `gamepadii reboot gamepad` sends it back",
        )
    })?;
    entered_webconfig(env, &pad, &wc);
    let client = match connect_ready(env, progress) {
        Ok(c) => c.with_hello(*ch.hello()),
        Err(e) => {
            let e = prefixed(
                e,
                "the pad is in web-config, but its web configurator didn't answer",
            );
            return Err(back_after(
                env,
                &wc,
                None,
                &probed.key,
                &mut trip,
                e,
                progress,
            ));
        }
    };
    let result = match work(&client, &wc) {
        Ok(r) => r,
        Err(e) => {
            let e = prefixed(e, "in web-config");
            return Err(back_after(
                env,
                &wc,
                Some(&client),
                &probed.key,
                &mut trip,
                e,
                progress,
            ));
        }
    };
    if stay {
        let mut note = format!(
            "left in web-config at {}; leave it with: gamepadii webconfig --exit",
            env.url
        );
        if let Some(dropped) = dropped_in_webconfig(env, &wc, &probed.key, &mut trip) {
            note.push_str(&format!("; {dropped}"));
        }
        trip.note = Some(note);
        return Ok((result, Some(trip)));
    }
    match go_back(env, &wc, Some(&client), &probed.key, &mut trip, progress) {
        Ok(()) => Ok((result, Some(trip))),
        Err(e) => Err(prefixed(
            e,
            &format!("{}, but on the way back", done(&result)),
        )),
    }
}

/// `client`, with the HELLO of `wc`'s control channel when it answers one
/// (it does in web-config too): evidence for [`Client::fork_fixes`].
fn with_channel(env: &Env, client: Client, wc: &Device) -> Client {
    match env
        .control
        .and_then(|open| channel::probe(open, wc).found())
    {
        Some(ch) => client.with_hello(*ch.hello()),
        None => client,
    }
}

/// `gamepadii info` for the pad on USB: [`through_webconfig`] with the
/// reads, the device cache refreshed from them (the pad's, before the way
/// back records the running profile in it), and the profile rows labelled
/// with the one STATUS shows once the trip is over (`null` without a trip:
/// the channel isn't asked in web-config).
pub fn info(
    env: &Env,
    stay: bool,
    progress: &mut dyn Progress,
) -> Result<(commands::Info, Option<Trip>)> {
    let mut failed = None;
    let work = |c: &Client, wc: &Device| {
        let (info, cache) = commands::info_with_cache(c, env.clock.wall())?;
        failed = store_read(env, cache, wc);
        Ok(info)
    };
    let (mut info, trip) = visit(env, stay, false, progress, work, |_| {
        "the pad was read".into()
    })?;
    if let Some(e) = &failed {
        progress.step(Step::CacheFailed(e));
    }
    info.set_running(trip.as_ref().and_then(|t| t.running_profile));
    Ok((info, trip))
}

/// `gamepadii backup` for the pad on USB: [`through_webconfig`] with the
/// bundle written to `output` (else into `env.backups_dir` when it has one,
/// as the TUI's does, else the default path), and the device cache
/// refreshed from it, as [`info`] does.
pub fn backup(
    env: &Env,
    output: Option<&Path>,
    stay: bool,
    progress: &mut dyn Progress,
) -> Result<(commands::BackupReport, Option<Trip>)> {
    let mut failed = None;
    let work = |c: &Client, wc: &Device| {
        let now = env.clock.wall();
        let (report, cache) = match (output, &env.backups_dir) {
            (None, Some(dir)) => commands::backup_in_dir(c, dir, now)?,
            _ => commands::backup_with_cache(c, output, now)?,
        };
        if let Some(cache) = cache {
            failed = store_read(env, cache, wc);
        }
        Ok(report)
    };
    let (report, trip) = visit(env, stay, true, progress, work, |b| {
        format!("the backup is saved in {}", b.path.display())
    })?;
    if let Some(e) = &failed {
        progress.step(Step::CacheFailed(e));
    }
    Ok((report, trip))
}

/// With `--stay`: the profile STATUS reports in web-config (the channel
/// answers there too) goes into `trip.running_profile`; what the restart
/// did to a RAM-only profile it ran before, if anything.
fn dropped_in_webconfig(env: &Env, wc: &Device, key: &Key, trip: &mut Trip) -> Option<String> {
    let st = channel_after_reboot(env, wc)?.status().ok()?;
    trip.running_profile = Some(st.profile);
    let dir = env.cache_dir.as_deref();
    let _ = commands::remember_profile(dir, key, st.profile, false, env.clock.wall());
    let p = trip.profile?;
    (st.profile != p).then(|| {
        format!(
            "it ran profile {p} before, not saved; the restart put it back on its saved \
             profile {} (`gamepadii profile switch {p}` switches again once it is back in a \
             gamepad mode)",
            st.profile
        )
    })
}

/// After `e` in web-config: sends the pad back and returns `e`, saying
/// whether that worked.
fn back_after(
    env: &Env,
    wc: &Device,
    client: Option<&Client>,
    key: &Key,
    trip: &mut Trip,
    e: Error,
    progress: &mut dyn Progress,
) -> Error {
    let was = trip.before.label();
    match go_back(env, wc, client, key, trip, progress) {
        Ok(()) => suffixed(
            e,
            &format!("; the pad was sent back and is in {was} mode again"),
        ),
        Err(back) => suffixed(
            e,
            &format!(
                "; sending the pad back to {was} failed too ({back}); \
                 `gamepadii reboot gamepad` tries again"
            ),
        ),
    }
}

/// Reboots the pad in web-config (`wc`) back into gamepad mode: over HTTP,
/// or over the channel (which answers in web-config too) when HTTP fails.
/// Then checks it came back as `trip.before`.
fn go_back(
    env: &Env,
    wc: &Device,
    client: Option<&Client>,
    key: &Key,
    trip: &mut Trip,
    progress: &mut dyn Progress,
) -> Result<()> {
    let options = client.and_then(|c| api::gamepad_options(c.transport()).ok());
    progress.step(Step::Rebooting(BootMode::Gamepad));
    let http = client.map(|c| api::reboot(c.transport(), BootMode::Gamepad));
    let via = match http {
        Some(Ok(_)) => Route::WebConfig,
        failed => {
            let http_err = failed.and_then(Result::err);
            match channel_after_reboot(env, wc).filter(|c| c.caps().has(Caps::REBOOT)) {
                Some(ch) => {
                    ch.reboot(BootMode::Gamepad)?;
                    Route::Control
                }
                None => {
                    return Err(http_err.unwrap_or_else(|| {
                        Error::Protocol(
                            "neither the web configurator nor the control channel answers \
                             in web-config"
                                .into(),
                        )
                    }));
                }
            }
        }
    };
    trip.back_via = Some(via);
    commands::remember_restart(env.cache_dir.as_deref(), key);
    let was = trip.before.mode;
    let custom = options
        .or_else(|| cached_options(env, key))
        .zip(was)
        .and_then(|(o, m)| custom_ids(&o, m));
    if let Some(ids) = custom {
        trip.note = Some(format!(
            "usbOverrideID is on, so it comes back as {ids}, which gamepadii can't \
             recognise; not verified"
        ));
        return Ok(());
    }
    let label = trip.before.label();
    let got = wait_reenumerated(
        env,
        wc,
        env.timeouts.reboot,
        Awaiting::Reboot(BootMode::Gamepad),
        progress,
        in_gamepad_mode,
    )
    .ok_or_else(|| not_back(env.timeouts.reboot, &format!("asked for {label}")))?;
    left_webconfig(env, wc, &got);
    trip.after = Some((&got).into());
    if let Some(m) = was
        && !shows(m, &got)
    {
        return Err(Error::CheckFailed(format!(
            "the pad came back as {} ({}), not {label}",
            Seen::from(&got).label(),
            got.usb_id()
        )));
    }
    let back = channel_after_reboot(env, &got);
    let status = back.as_ref().map(Channel::status);
    if let (Some(ch), Some(Ok(st))) = (&back, status.as_ref()) {
        let dir = env.cache_dir.as_deref();
        let _ = commands::remember_profile(dir, key, st.profile, false, env.clock.wall());
        let now = running(st);
        if was.is_some() && now != was {
            return Err(Error::CheckFailed(format!(
                "the pad came back, but says it runs {}, not {label}",
                now.map_or("an unknown mode", InputMode::label)
            )));
        }
        trip.after = Some(confirmed(&got, now));
        trip.running_profile = Some(st.profile);
        if let Some(p) = trip.profile.filter(|p| *p != st.profile) {
            trip.note = Some(restore_profile(env, ch, key, p, st.profile, trip, progress));
        }
    } else if let Some(Err(e)) = status {
        // The VID:PID checked out; which profile it runs is unknown.
        trip.note = Some(format!(
            "the control channel didn't answer STATUS once the pad was back ({e}), so \
             which profile it runs is unknown"
        ));
    } else {
        trip.note = got.note.clone();
    }
    trip.verified = was.is_some();
    Ok(())
}

/// The restart put the pad back on its `saved` profile, dropping the
/// RAM-only `p` it ran before: switches back to `p`, RAM only (flash is
/// untouched), checks STATUS and records it in the cache. The trip's note
/// either way; a failure is also a warning, but not an error: what the trip
/// was for has worked.
fn restore_profile(
    env: &Env,
    ch: &Channel,
    key: &Key,
    p: u8,
    saved: u8,
    trip: &mut Trip,
    progress: &mut dyn Progress,
) -> String {
    progress.step(Step::RestoringProfile(p));
    let dir = env.cache_dir.as_deref();
    let result = ch
        .set_profile(p, false)
        .and_then(|()| match applied(env, ch, p) {
            Some(st) if st.profile == p => Ok(()),
            Some(st) => {
                trip.running_profile = Some(st.profile);
                let _ = commands::remember_profile(dir, key, st.profile, false, env.clock.wall());
                Err(Error::CheckFailed(format!(
                    "the pad accepted profile {p}, but STATUS shows profile {}",
                    st.profile
                )))
            }
            None => {
                trip.running_profile = None;
                Err(Error::Protocol(format!(
                    "the pad took profile {p} but hasn't applied it after 1 s"
                )))
            }
        });
    match result {
        Ok(()) => {
            if let Some(e) = commands::remember_profile(dir, key, p, false, env.clock.wall()) {
                progress.step(Step::CacheFailed(&e));
            }
            trip.restored_profile = Some(p);
            trip.running_profile = Some(p);
            let name = ProfileName {
                number: p,
                label: ch.string(proto::string::profile_label(p)).ok().flatten(),
            };
            format!(
                "restored profile {} (RAM only) after the restart",
                name.display()
            )
        }
        Err(e) => {
            progress.step(Step::RestoreFailed {
                profile: p,
                error: &e,
            });
            format!(
                "it ran profile {p} before, not saved; the restart put it back on its saved \
                 profile {saved}, and switching back failed ({e}); `gamepadii profile switch \
                 {p}` tries again"
            )
        }
    }
}

/// STATUS once the pad has applied a switch to profile `n` (the main loop
/// remaps the pins on its next pass, well under 1 ms), or to another
/// profile; `None` when it still hasn't after 1 s.
fn applied(env: &Env, ch: &Channel, n: u8) -> Option<channel::Status> {
    wait_for(
        env.clock,
        Poll {
            interval: Duration::from_millis(20),
            timeout: Duration::from_secs(1),
        },
        || ch.status().ok().filter(|s| s.profile != n || !s.pending()),
        |_| {},
    )
}

/// Which profile `profile switch` goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileArg {
    Number(u8),
    /// Matched against the labels, ignoring case.
    Label(String),
}

impl std::str::FromStr for ProfileArg {
    type Err = String;

    /// `1`–`6`, or a label.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty profile".into());
        }
        if s.chars().all(|c| c.is_ascii_digit()) {
            return match s.parse::<u8>() {
                Ok(n @ 1..=6) => Ok(ProfileArg::Number(n)),
                _ => Err(format!("profile {s}: profiles are 1 to 6")),
            };
        }
        Ok(ProfileArg::Label(s.to_string()))
    }
}

/// `gamepadii profile switch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileReport {
    /// Always `control`: there is no other way without a button press.
    pub via: Route,
    /// The device node.
    pub host: String,
    pub before: ProfileName,
    pub profile: ProfileName,
    /// Whether the active profile changed.
    pub changed: bool,
    /// Whether it was saved to flash (`--save`).
    pub saved: bool,
    /// Whether STATUS shows the switch applied (the pins remapped).
    pub applied: bool,
    pub note: Option<String>,
}

/// Why `profile switch` can't work on `pad`.
fn no_channel_for_profiles(pad: &Device, why: Option<String>) -> Error {
    let mut msg = format!(
        "the pad (in {} mode) has no control channel, so gamepadii can't switch its \
         profile; that needs the gamepadii build of GP2040-CE. On stock firmware, switch \
         with the pad's profile hotkeys (LOAD_PROFILE_n or NEXT_PROFILE, set up under \
         Hotkeys in web-config), which also save the profile",
        Seen::from(pad).label()
    );
    if let Some(w) = why {
        msg.push_str(&format!(" ({w})"));
    }
    Error::NeedsUser(msg)
}

/// Switches the active profile over the control channel: live, with no
/// reboot; saved to flash only with `save`.
pub fn profile_switch(
    env: &Env,
    which: &ProfileArg,
    save: bool,
    progress: &mut dyn Progress,
) -> Result<ProfileReport> {
    let pad = pick(env.scan.scan())?;
    if pad.mode == Some(InputMode::Bootsel) {
        return Err(Error::NeedsUser(
            "the pad is in BOOTSEL (the boot ROM, for flashing); replug it first".into(),
        ));
    }
    let Some(open) = env.control else {
        return Err(no_channel_for_profiles(
            &pad,
            Some("the control channel is off: --no-channel".into()),
        ));
    };
    let ch = match channel::probe(open, &pad) {
        Probe::Found(ch) if ch.caps().has(Caps::SET_PROFILE) => ch,
        Probe::Found(_) => {
            return Err(no_channel_for_profiles(
                &pad,
                Some("this build's channel has no SET_PROFILE".into()),
            ));
        }
        Probe::NoAccess { node } => {
            return Err(Error::NeedsUser(format!(
                "{}; switching profiles needs the control channel",
                channel::udev_hint(&node)
            )));
        }
        Probe::Newer { major, .. } => return Err(Error::Usage(commands::newer_protocol(major))),
        Probe::Absent(_) => return Err(no_channel_for_profiles(&pad, None)),
    };
    let st = ch.status()?;
    let firmware = Firmware {
        board: ch.string(proto::string::BOARD).ok().flatten(),
        ..Firmware::default()
    };
    let key = Key::new(&firmware, cache::identify(env.cache_dir.as_deref(), &pad));
    let remember = |n: u8, saved: bool, progress: &mut dyn Progress| {
        let dir = env.cache_dir.as_deref();
        if let Some(e) = commands::remember_profile(dir, &key, n, saved, env.clock.wall()) {
            progress.step(Step::CacheFailed(&e));
        }
    };
    remember(st.profile, false, progress);
    let mut labels = std::collections::BTreeMap::new();
    for n in st.enabled() {
        labels.insert(n, ch.string(proto::string::profile_label(n))?);
    }
    let name = |n: u8| ProfileName {
        number: n,
        label: labels.get(&n).cloned().flatten(),
    };
    let choices = || {
        labels
            .keys()
            .map(|n| name(*n).display())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let n = match which {
        ProfileArg::Number(n) if st.profile_enabled(*n) => *n,
        ProfileArg::Number(n) => {
            return Err(Error::Usage(format!(
                "profile {n} isn't enabled on the pad; it has {}",
                choices()
            )));
        }
        ProfileArg::Label(l) => labels
            .iter()
            .find(|(_, v)| v.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(l)))
            .map(|(n, _)| *n)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "no enabled profile is called {l:?}; the pad has {}",
                    choices()
                ))
            })?,
    };
    let before = name(st.profile);
    let mut report = ProfileReport {
        via: Route::Control,
        host: ch.node(),
        before,
        profile: name(n),
        changed: st.profile != n,
        saved: false,
        applied: true,
        note: None,
    };
    if !report.changed && !save {
        return Ok(report);
    }
    ch.set_profile(n, save)?;
    report.saved = save;
    // The hints follow the switch: RAM-only changes the running profile,
    // --save the saved one too.
    remember(n, save, progress);
    let Some(after) = applied(env, &ch, n) else {
        report.applied = false;
        report.note = Some("the pad took the switch but hasn't applied it after 1 s".into());
        return Ok(report);
    };
    if after.profile != n {
        remember(after.profile, false, progress);
        return Err(Error::CheckFailed(format!(
            "the pad accepted profile {}, but STATUS shows profile {}",
            n, after.profile
        )));
    }
    if !save {
        report.note = Some(
            "not saved: the pad goes back to its saved profile when it restarts, unless \
             something saves its settings first (any hotkey, turbo or LED change does); \
             --save keeps it"
                .into(),
        );
    }
    Ok(report)
}
