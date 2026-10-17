//! Rendering of command results: text for people, the versioned JSON
//! contract (`"schema": 1`, snake_case keys, explicit `null`s), or one line
//! of waybar custom-module JSON for `status`.
//!
//! Human output is a bold headline, then `label  value` rows with the
//! labels dimmed and padded to 14 columns; a row with several values
//! continues on lines indented to the same column.

use std::io::IsTerminal;

use gamepadii_core::Error;
use gamepadii_core::api::RebootAck;
use gamepadii_core::commands::{BackupReport, Firmware, Info, ModeSetting, Status};
use gamepadii_core::discovery::Device;
use gamepadii_core::flow::{ModeReport, ModeStatus, RebootReport, Seen, Via, WebconfigReport};
use gamepadii_core::mode::{BootMode, Confidence, InputMode};
use serde::Serialize;

/// Bumped on any breaking change to the JSON output.
pub const SCHEMA: u32 = 1;

/// The address GP2040-CE's web configurator serves on (its own DHCP server
/// hands the host 192.168.7.2).
pub const WEBCONFIG_URL: &str = "http://192.168.7.1/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
    /// waybar `custom` module JSON (one line); only `status` supports it.
    Waybar,
}

/// Output style decided once from the flags and the environment.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub format: Format,
    pub color: bool,
}

impl Style {
    /// Colour only for human output on a TTY, and never with `NO_COLOR` set
    /// (to anything non-empty, per no-color.org).
    pub fn detect(format: Format) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Self {
            format,
            color: format == Format::Human && !no_color && std::io::stdout().is_terminal(),
        }
    }

    /// No colour regardless of the terminal (tooltips, tests).
    pub fn plain(format: Format) -> Self {
        Self {
            format,
            color: false,
        }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    schema: u32,
    #[serde(flatten)]
    data: &'a T,
}

fn json<T: Serialize>(data: &T) -> String {
    serde_json::to_string_pretty(&Envelope {
        schema: SCHEMA,
        data,
    })
    .expect("serialising plain data cannot fail")
}

/// JSON for a failed command, printed to stdout under `--json` so scripts
/// always get a parseable object.
pub fn error_json(err: &Error) -> String {
    #[derive(Serialize)]
    struct ErrorBody {
        kind: &'static str,
        message: String,
        exit_code: u8,
    }
    #[derive(Serialize)]
    struct ErrorEnvelope {
        schema: u32,
        error: ErrorBody,
    }
    serde_json::to_string_pretty(&ErrorEnvelope {
        schema: SCHEMA,
        error: ErrorBody {
            kind: err.kind(),
            message: err.to_string(),
            exit_code: err.exit_code(),
        },
    })
    .expect("serialising plain data cannot fail")
}

/// The mode as people read it: `XInput`, or `PS4 or PS5` when the id is
/// shared between modes.
pub fn mode_label(d: &Device) -> String {
    match d.mode {
        Some(m) => m.label().to_string(),
        None => d
            .candidates
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join(" or "),
    }
}

fn confidence(c: Confidence) -> &'static str {
    match c {
        Confidence::Confirmed => "confirmed",
        Confidence::Likely => "likely",
    }
}

/// Label/value rows shared by the human output and the waybar tooltip.
fn device_rows(d: &Device) -> Vec<(&'static str, String)> {
    let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
    let mut port = match d.bus {
        Some(bus) => format!("bus {bus}, port {}", d.port),
        None => format!("port {}", d.port),
    };
    if let Some(n) = d.devnum {
        port.push_str(&format!(", device {n}"));
    }
    let mut rows = vec![
        ("usb id", d.usb_id()),
        ("product", dash(&d.product)),
        ("manufacturer", dash(&d.manufacturer)),
        ("serial", dash(&d.serial)),
        ("port", port),
    ];
    if d.mode == Some(InputMode::Webconfig) {
        let via = d.netdev.as_deref().unwrap_or("no network interface yet");
        rows.push(("web-config", format!("{WEBCONFIG_URL} (via {via})")));
    }
    if let Some(p) = &d.sysfs_path {
        rows.push(("sysfs", p.display().to_string()));
    }
    if let Some(n) = &d.note {
        rows.push(("note", n.clone()));
    }
    rows
}

pub fn status(style: Style, s: &Status) -> String {
    match style.format {
        Format::Json => return json(s),
        Format::Waybar => return waybar_status(s).to_line(),
        Format::Human => {}
    }
    let mut out = String::new();
    for (i, d) in s.devices.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "{} {}\n",
            style.bold(&mode_label(d)),
            style.dim(&format!("({})", confidence(d.confidence)))
        ));
        for (label, value) in device_rows(d) {
            out.push_str(&format!("{}{value}\n", style.dim(&format!("{label:<14}"))));
        }
    }
    out
}

/// waybar custom-module output (`return-type: json`), always one line.
#[derive(Debug, Serialize)]
pub struct Waybar {
    pub text: String,
    pub tooltip: String,
    pub class: String,
    pub alt: String,
}

impl Waybar {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("serialising plain data cannot fail")
    }
}

/// Nerd Font nf-md-gamepad_variant (U+F0297), the waybar `text` glyph.
pub const GLYPH: &str = "\u{f0297}";

/// The first device in the bar; the tooltip has them all. The class is the
/// mode key (`xinput`, `webconfig`, ...), `ambiguous` when the id fits more
/// than one mode.
pub fn waybar_status(s: &Status) -> Waybar {
    let Some(first) = s.devices.first() else {
        return waybar_error(&Error::NotFound("no device".into()));
    };
    let class = first
        .mode
        .map_or_else(|| "ambiguous".to_string(), |m| m.key().to_string());
    let mut text = format!("{GLYPH} {}", mode_label(first));
    if s.devices.len() > 1 {
        text.push_str(&format!(" +{}", s.devices.len() - 1));
    }
    // waybar renders tooltips in a proportional font: "label: value", no padding.
    let mut parts = Vec::new();
    for d in &s.devices {
        let mut t = format!("{} ({})", mode_label(d), confidence(d.confidence));
        for (label, value) in device_rows(d) {
            t.push_str(&format!("\n{label}: {value}"));
        }
        parts.push(t);
    }
    Waybar {
        text,
        tooltip: parts.join("\n\n"),
        alt: class.clone(),
        class,
    }
}

/// waybar output for a failed `status`, so the bar module never breaks.
pub fn waybar_error(err: &Error) -> Waybar {
    let headline = match err {
        Error::NotFound(_) => "No GP2040-CE gamepad connected",
        _ => "gamepadii: error",
    };
    Waybar {
        text: GLYPH.to_string(),
        tooltip: format!("{headline}\n{err}"),
        class: "disconnected".into(),
        alt: "disconnected".into(),
    }
}

const LABEL: usize = 14;

/// `label  value` rows; a value with several lines is indented to line up.
fn rows(style: Style, out: &mut String, rows: &[(&str, String)]) {
    for (label, value) in rows {
        for (i, line) in value.lines().enumerate() {
            let label = if i == 0 { *label } else { "" };
            out.push_str(&format!(
                "{}{line}\n",
                style.dim(&format!("{label:<LABEL$}"))
            ));
        }
    }
}

/// `1.2 MiB`, `19.3 KiB`, `512 B`.
pub fn bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f >= KIB * KIB {
        format!("{:.1} MiB", f / (KIB * KIB))
    } else if f >= KIB {
        format!("{:.1} KiB", f / KIB)
    } else {
        format!("{n} B")
    }
}

fn opt_bytes(n: Option<u64>) -> String {
    n.map_or_else(|| "?".into(), bytes)
}

/// `Haute42 COSMOX (GP2040-CE v0.7.12)`, as plain text.
fn firmware_headline(f: &Firmware) -> (String, String) {
    let board = f
        .board_label
        .clone()
        .or_else(|| f.board.clone())
        .unwrap_or_else(|| "GP2040-CE pad".into());
    let version = f.version.as_deref().unwrap_or("unknown version");
    (board, format!("(GP2040-CE {version})"))
}

fn setting(s: &ModeSetting) -> String {
    match (s.mode, s.value) {
        (Some(m), _) => m.label().to_string(),
        (None, None | Some(-1)) => "-".into(),
        (None, Some(v)) => format!("unknown ({v})"),
    }
}

pub fn info(style: Style, i: &Info) -> String {
    if style.format != Format::Human {
        return json(i);
    }
    let (board, version) = firmware_headline(&i.firmware);
    let mut out = format!("{} {}\n", style.bold(&board), style.dim(&version));
    let f = &i.firmware;
    let mut build = Vec::new();
    if let Some(commit) = &f.build {
        build.push(format!("build {commit}"));
    }
    build.extend(
        [&f.build_type, &f.architecture]
            .into_iter()
            .flatten()
            .cloned(),
    );
    let mut firmware = f.version.clone().unwrap_or_else(|| "-".into());
    if !build.is_empty() {
        firmware.push_str(&format!(" ({})", build.join(", ")));
    }
    let mut r: Vec<(&str, String)> = vec![
        ("firmware", firmware),
        ("board", f.board.clone().unwrap_or_else(|| "-".into())),
    ];
    if let Some(m) = &i.memory {
        r.push((
            "flash",
            format!(
                "{} of {} used ({} chip)",
                opt_bytes(m.flash_used),
                opt_bytes(m.flash_total),
                opt_bytes(m.flash_physical)
            ),
        ));
        r.push((
            "heap",
            format!(
                "{} of {} used, {} static",
                opt_bytes(m.heap_used),
                opt_bytes(m.heap_total),
                opt_bytes(m.static_allocs)
            ),
        ));
    }
    r.push(("input mode", setting(&i.input_mode)));
    let profiles = i
        .profiles
        .iter()
        .map(|p| {
            let mut line = format!("{} {}", p.number, p.label.as_deref().unwrap_or("-"));
            if p.active {
                line.push_str(" (active)");
            }
            if p.enabled == Some(false) {
                line.push_str(" (disabled)");
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    r.push((
        "profiles",
        if profiles.is_empty() {
            "-".into()
        } else {
            profiles
        },
    ));
    let boot = i
        .boot_buttons
        .iter()
        .map(|b| format!("{:<3}{}", b.button, setting(&b.setting)))
        .collect::<Vec<_>>()
        .join("\n");
    r.push(("boot buttons", boot));
    r.push(("web-config", i.host.clone()));
    rows(style, &mut out, &r);
    out
}

pub fn backup(style: Style, b: &BackupReport) -> String {
    if style.format != Format::Human {
        return json(b);
    }
    let (board, version) = firmware_headline(&b.firmware);
    let mut out = format!(
        "{} {} {}\n",
        style.bold("Backed up"),
        style.bold(&board),
        style.dim(&version)
    );
    let total = b.endpoints.len() + b.unavailable.len();
    let mut r = vec![
        ("file", b.path.display().to_string()),
        ("endpoints", format!("{} of {total}", b.endpoints.len())),
    ];
    if !b.unavailable.is_empty() {
        r.push(("not on this fw", b.unavailable.join("\n")));
    }
    r.push(("size", bytes(b.bytes)));
    r.push(("captured", b.captured_at.clone()));
    rows(style, &mut out, &r);
    out
}

/// `XInput (045e:028e, likely)`.
fn seen(s: &Seen) -> String {
    format!("{} ({}, {})", s.label(), s.usb_id, confidence(s.confidence))
}

fn ack(a: RebootAck) -> &'static str {
    match a {
        RebootAck::Confirmed => "accepted",
        RebootAck::Probable => "no reply (it most likely restarted before answering)",
    }
}

pub fn reboot(style: Style, r: &RebootReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let mut out = match &r.after {
        Some(_) => format!(
            "{} {}\n",
            style.bold("Rebooted into"),
            style.bold(r.mode.label())
        ),
        None => {
            let after = match r.mode {
                BootMode::Gamepad => "it comes back in its saved input mode".to_string(),
                BootMode::Webconfig => format!("it comes back at {}/ in a few seconds", r.host),
                BootMode::Bootsel => {
                    "it comes back as the RPI-RP2 USB drive, for flashing".to_string()
                }
            };
            format!(
                "{} {}\n{}\n",
                style.bold("Rebooting into"),
                style.bold(r.mode.label()),
                style.dim(&after)
            )
        }
    };
    let mut rw: Vec<(&str, String)> = Vec::new();
    if r.ack == RebootAck::Probable {
        rw.push(("request", ack(r.ack).into()));
    }
    if let Some(a) = &r.after {
        let mut line = seen(a);
        if r.verified == Some(true) {
            line.push_str(", as expected");
        }
        rw.push(("came back as", line));
    }
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

/// `inputMode 0 → 3`.
fn change(c: &gamepadii_core::api::FieldChange) -> String {
    format!("{} {} → {}", c.key, c.before, c.after)
}

pub fn mode(style: Style, r: &ModeReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let target = r.target.label();
    let mut out = match r.via {
        Via::Already => format!("{} {}\n", style.bold("Already in"), style.bold(target)),
        _ if r.verified => format!("{} {}\n", style.bold("Switched to"), style.bold(target)),
        _ => format!("{} {}\n", style.bold("Switching to"), style.bold(target)),
    };
    let mut rw: Vec<(&str, String)> = Vec::new();
    match r.via {
        Via::Already => rw.push(("usb id", r.before.usb_id.clone())),
        Via::BootButton => {
            rw.push(("was", seen(&r.before)));
            rw.push(("via", "its boot button; nothing written".into()));
        }
        Via::WebConfig => {
            rw.push(("was", seen(&r.before)));
            rw.push((
                "via",
                if r.waited_for_user {
                    "web-config, once you switched it".into()
                } else {
                    "web-config".into()
                },
            ));
            let saved = match (&r.write, r.saved_before) {
                (Some(w), _) => {
                    let changes = w.changed.iter().map(change).collect::<Vec<_>>().join(", ");
                    format!("{changes} (read back; nothing else changed)")
                }
                (None, Some(s)) => format!("{} already (not written)", setting(&s)),
                (None, None) => "-".into(),
            };
            rw.push(("saved", saved));
            if let Some(b) = &r.backup {
                rw.push(("backup", b.display().to_string()));
            }
            if r.reboot == Some(RebootAck::Probable) {
                rw.push(("reboot", ack(RebootAck::Probable).into()));
            }
        }
    }
    if r.via != Via::Already {
        rw.push((
            "now",
            r.after.as_ref().map_or_else(|| "not verified".into(), seen),
        ));
    }
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn mode_status(style: Style, s: &ModeStatus) -> String {
    if style.format != Format::Human {
        return json(s);
    }
    let mut out = format!(
        "{} {}\n",
        style.bold(&s.current.label()),
        style.dim(&format!("({})", confidence(s.current.confidence)))
    );
    let mut rw: Vec<(&str, String)> = vec![("usb id", s.current.usb_id.clone())];
    let saved = match (&s.saved, s.saved_from, &s.cached_at) {
        (Some(m), Some("cache"), Some(at)) => format!("{} (as of {at})", setting(m)),
        (Some(m), _, _) => setting(m),
        (None, _, _) => "unknown until the pad has been in web-config once".into(),
    };
    rw.push(("saved mode", saved));
    if let Some(f) = &s.firmware {
        rw.push(("firmware", f.clone()));
    }
    let width = s
        .modes
        .iter()
        .map(|c| c.mode.key().len())
        .max()
        .unwrap_or(0)
        + 2;
    let modes = s
        .modes
        .iter()
        .map(|c| {
            let mut extra = Vec::new();
            if c.current {
                extra.push("current".to_string());
            }
            if let Some(b) = c.boot_button {
                extra.push(format!("boot button {}", b.both()));
            }
            if c.supported == Some(false) {
                extra.push("not in this firmware".into());
            }
            let mut line = format!("{:<width$}{}", c.mode.key(), c.label);
            if !extra.is_empty() {
                line.push_str(&format!(" ({})", extra.join(", ")));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    rw.push(("modes", modes));
    rows(style, &mut out, &rw);
    out.push_str(&style.dim("switch with: gamepadii mode <mode>"));
    out.push('\n');
    out
}

pub fn webconfig(style: Style, r: &WebconfigReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let mut out = format!("{} {}\n", style.bold("Web-config at"), style.bold(&r.url));
    let via = if r.waited_for_user {
        "the pad is in web-config now"
    } else {
        "the pad was already in web-config"
    };
    out.push_str(&style.dim(&format!("{via}; leave it with: gamepadii webconfig --exit")));
    out.push('\n');
    out
}
