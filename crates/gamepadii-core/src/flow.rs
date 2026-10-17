//! The guided flows: `mode`, `webconfig`, and `reboot` with a wait. They
//! poll USB through a [`Scan`] and time through a [`Clock`], open the
//! web-config client through a closure, and report progress through a
//! [`Progress`], so every branch runs in tests against a simulated pad with
//! no real sleeps.
//!
//! In a gamepad mode GP2040-CE has no control channel: the flows tell the
//! user which buttons to hold (with key-by-key hints from the
//! [`DeviceCache`] when there is one), wait for the pad to re-enumerate,
//! and carry on from there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::api::{self, FirmwareVersion, OptionsWrite, RebootAck};
use crate::buttons::{Button, Combo};
use crate::cache::{self, DeviceCache};
use crate::client::Client;
use crate::commands::{self, ModeSetting};
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
}

/// The parts of a cache the guide uses.
struct Cached {
    profiles: crate::buttons::Profiles,
    active: i64,
    options: api::GamepadOptions,
    captured_at: String,
}

impl Cached {
    fn from(c: &DeviceCache) -> Option<Self> {
        let options = c.options()?;
        Some(Self {
            profiles: c.profiles()?,
            active: options.profile_number.unwrap_or(1),
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
        let cached = cache.and_then(Cached::from);
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
        }
    }

    /// The instructions, a sentence per line.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
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
        out.push(match &self.cached_at {
            Some(t) => format!(
                "Key names come from what gamepadii read on {t}; if the active profile \
                 changed since, they may be off."
            ),
            None => "These are logical buttons, so on a remapped profile they may be \
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

fn pick(devices: Vec<Device>) -> Result<Device> {
    let webconfig = devices
        .iter()
        .position(|d| d.mode == Some(InputMode::Webconfig));
    match webconfig {
        Some(i) => Ok(devices.into_iter().nth(i).expect("index from position")),
        None => devices.into_iter().next().ok_or_else(|| {
            Error::NotFound("no GP2040-CE device on USB (is it plugged in?)".into())
        }),
    }
}

fn in_gamepad_mode(d: &Device) -> bool {
    !matches!(d.mode, Some(InputMode::Webconfig | InputMode::Bootsel))
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

/// Refreshes the cache from a connected pad; returns it.
fn refresh_cache(env: &Env, client: &Client, progress: &mut dyn Progress) -> Result<DeviceCache> {
    let c = DeviceCache::capture(client, env.clock.wall())?;
    if let Some(e) = cache::store_quietly(env.cache_dir.as_deref(), &c) {
        progress.step(Step::CacheFailed(&e));
    }
    Ok(c)
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
    if pad.mode == Some(InputMode::Webconfig) {
        return via_webconfig(env, target, &pad, (&pad).into(), false, progress);
    }
    let cache = env.cache_dir.as_deref().and_then(cache::load_latest);
    let guide = Guide::new(&pad, Some(target), cache.as_ref());
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
    let cached = refresh_cache(env, &client, progress)?;
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
    let (cache, from) = if pad.mode == Some(InputMode::Webconfig) {
        let client = connect_ready(env, progress)?;
        (Some(refresh_cache(env, &client, progress)?), Some("pad"))
    } else {
        let c = env.cache_dir.as_deref().and_then(cache::load_latest);
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
            supported: fw
                .as_ref()
                .filter(|f| f.version.is_some())
                .map(|f| firmware_has(f, m)),
            current: pad.mode == Some(m),
        })
        .collect();
    Ok(ModeStatus {
        current: (&pad).into(),
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
}

/// Gets the pad into web-config (guiding the user if needed) and returns
/// the URL. Refreshes the cache when the server answers.
pub fn webconfig(env: &Env, wait: bool, progress: &mut dyn Progress) -> Result<WebconfigReport> {
    let pad = pick(env.scan.scan())?;
    let (pad, waited) = if pad.mode == Some(InputMode::Webconfig) {
        (pad, false)
    } else {
        let cache = env.cache_dir.as_deref().and_then(cache::load_latest);
        let guide = Guide::new(&pad, None, cache.as_ref());
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
        (got, true)
    };
    // The cache is a bonus: a pad whose server isn't up yet still has a URL.
    if let Ok(client) = connect_ready(env, progress) {
        let _ = refresh_cache(env, &client, progress);
    }
    Ok(WebconfigReport {
        url: env.url.clone(),
        device: (&pad).into(),
        waited_for_user: waited,
    })
}

/// `gamepadii reboot`, waiting for the pad to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebootReport {
    pub host: String,
    pub mode: BootMode,
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

/// Reboots the pad into `mode`; with `wait`, watches USB until it comes
/// back and checks what it came back as.
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
    let mut report = RebootReport {
        host: client.host(),
        mode,
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
    let Some(before) = before.filter(|_| wait) else {
        if wait {
            report.note = Some("the pad isn't on this machine's USB in web-config, so gamepadii can't watch it come back".into());
        }
        return Ok(report);
    };
    if let Some(ids) = custom {
        report.note = Some(format!(
            "usbOverrideID is on, so it comes back as {ids}, which gamepadii can't recognise"
        ));
        return Ok(report);
    }
    let accept = |d: &Device| match mode {
        BootMode::Gamepad => in_gamepad_mode(d),
        BootMode::Webconfig => d.mode == Some(InputMode::Webconfig),
        BootMode::Bootsel => d.mode == Some(InputMode::Bootsel),
    };
    let got = wait_reenumerated(
        env,
        &before,
        env.timeouts.reboot,
        Awaiting::Reboot(mode),
        progress,
        accept,
    )
    .ok_or_else(|| not_back(env.timeouts.reboot, &format!("asked for {}", mode.label())))?;
    report.waited = true;
    report.after = Some((&got).into());
    report.note = got.note.clone();
    if let Some(m) = expected {
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
