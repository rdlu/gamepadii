//! Rendering of command results: text for people, the versioned JSON
//! contract (`"schema": 1`, snake_case keys, explicit `null`s), or one line
//! of waybar custom-module JSON for `status` and `leds`.
//!
//! Human output is a bold headline, then `label  value` rows with the
//! labels dimmed and padded to 14 columns; a row with several values
//! continues on lines indented to the same column.

use std::io::IsTerminal;

use gamepadii_core::Error;
use gamepadii_core::api::{Evidence, Line, LineEvidence, RebootAck};
use gamepadii_core::buttons::Button;
use gamepadii_core::channel::{LogEvent, proto};
use gamepadii_core::colors::{
    self, ColorEntry, ColorsReport, ColorsWriteReport, ThemeApplyReport, ThemeReport,
};
use gamepadii_core::commands::{
    BackupReport, ChannelReport, Firmware, Info, ModeSetting, Status, StatusDevice,
};
use gamepadii_core::discovery::Device;
use gamepadii_core::display::{BoardLayout, Current, LayoutReport, LayoutStatus};
use gamepadii_core::flow::{
    self, ModeReport, ModeStatus, ProfileReport, RebootReport, RestoreReport, Route, Seen, Trip,
    Via, WebconfigReport, WebconfigVia,
};
use gamepadii_core::input::{FoundPin, InputReport, NamesFrom, PinInfo, Sticks};
use gamepadii_core::leds::{ButtonColor, LedReport, LedSetReport, LedState, Lights, Named};
use gamepadii_core::library::diff::{ApplyReport, Change, DiffReport, PinState};
use gamepadii_core::library::{ListReport, PullReport, Resolved};
use gamepadii_core::lights::{self, Kind, LightsReport, LightsWriteReport};
use gamepadii_core::log::{self, LogEntry, LogReport};
use gamepadii_core::mode::{BootMode, Confidence, InputMode};
use gamepadii_core::slots::{PinView, SlotReport, SlotWriteReport};
use gamepadii_core::udev;
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
    /// waybar `custom` module JSON (one line); `status` and `leds`.
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

pub fn confidence(c: Confidence) -> &'static str {
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

/// The control channel's rows: what the firmware itself says.
fn channel_rows(c: &ChannelReport) -> Vec<(&'static str, String)> {
    let mut firmware = c.firmware.version.clone().unwrap_or_else(|| "-".into());
    let board: Vec<String> = [&c.firmware.board_label, &c.firmware.build_type]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if !board.is_empty() {
        firmware.push_str(&format!(" ({})", board.join(", ")));
    }
    let mut profile = c.active_profile.display();
    if c.profile_pending {
        profile.push_str(" (switching)");
    }
    let mut board = c.firmware.board.clone().unwrap_or_else(|| "-".into());
    if let Some(id) = &c.board_id {
        board.push_str(&format!(" (board ID {id})"));
    }
    vec![
        ("firmware", firmware),
        ("board", board),
        ("input mode", setting(&c.input_mode)),
        ("profile", profile),
        (
            "channel",
            format!("control channel v{} on {}", c.protocol, c.node),
        ),
    ]
}

/// Every row of one device in `status`.
pub fn status_rows(d: &StatusDevice) -> Vec<(&'static str, String)> {
    let mut rows = device_rows(&d.device);
    if let Some(c) = &d.control_channel {
        rows.extend(channel_rows(c));
    }
    if let Some(n) = &d.channel_note {
        rows.push(("channel", n.clone()));
    }
    if let Some(p) = d
        .control_channel
        .as_ref()
        .and_then(|c| c.last_panic.as_ref())
    {
        rows.push((
            "warning",
            format!(
                "the firmware panicked in boot {} at {}.{:03} s: '{}'. `gamepadii log` shows \
                 the log",
                p.boot,
                p.ms / 1000,
                p.ms % 1000,
                p.text
            ),
        ));
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
    for (i, sd) in s.devices.iter().enumerate() {
        let d = &sd.device;
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "{} {}\n",
            style.bold(&mode_label(d)),
            style.dim(&format!("({})", confidence(d.confidence)))
        ));
        for (label, value) in status_rows(sd) {
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
    let Some(first) = s.devices.first().map(|d| &d.device) else {
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
    for sd in &s.devices {
        let d = &sd.device;
        let mut t = format!("{} ({})", mode_label(d), confidence(d.confidence));
        for (label, value) in status_rows(sd) {
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

/// A result that needed web-config, with how the pad got there (`null`
/// when it was there already).
#[derive(Serialize)]
struct WithTrip<'a, T: Serialize> {
    #[serde(flatten)]
    data: &'a T,
    trip: Option<&'a Trip>,
}

/// The rows saying how the pad went to web-config and back.
fn trip_rows(t: &Trip) -> Vec<(&'static str, String)> {
    let mut rw = Vec::new();
    let route = match (t.stayed, t.back_via) {
        (true, _) => "web-config, and left there",
        (false, Some(Route::Control)) => "web-config and back (back over the channel too)",
        (false, _) => "web-config and back",
    };
    rw.push(("via", format!("the control channel: {route} ({})", t.host)));
    rw.push(("was", seen(&t.before)));
    if !t.stayed {
        let now = match &t.after {
            Some(a) if t.verified => format!("{}, as before", seen(a)),
            Some(a) => seen(a),
            None => "not verified".into(),
        };
        rw.push(("now", now));
    }
    if let Some(n) = &t.note {
        rw.push(("note", n.clone()));
    }
    rw
}

/// `main (a known build)`.
fn line_text(l: &Line) -> String {
    let why = match l.evidence {
        LineEvidence::KnownBuild => "a known build",
        LineEvidence::ReleaseTag => "the release tag",
        LineEvidence::Shape => "from what getGamepadOptions has",
    };
    format!("{} ({why})", l.line)
}

pub fn info(style: Style, i: &Info, trip: Option<&Trip>) -> String {
    if style.format != Format::Human {
        return json(&WithTrip { data: i, trip });
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
        ("line", line_text(&i.firmware_line)),
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
            let tags: Vec<&str> = [
                (p.saved, "saved"),
                (p.running == Some(true), "running"),
                (p.enabled == Some(false), "disabled"),
            ]
            .into_iter()
            .filter_map(|(on, tag)| on.then_some(tag))
            .collect();
            if !tags.is_empty() {
                line.push_str(&format!(" ({})", tags.join(", ")));
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
    if let Some(d) = &i.display_layout {
        r.push(("oled layout", layout_text(d)));
    }
    r.push(("web-config", i.host.clone()));
    r.extend(trip.map(trip_rows).unwrap_or_default());
    // A trip went over the channel, so the pad could have said which
    // profile it runs; say so when it didn't, rather than leave it out.
    if trip.is_some() && i.running_profile.is_none() {
        r.push((
            "running",
            "unknown: the control channel didn't report the profile after the trip".into(),
        ));
    }
    for h in &i.hints {
        r.push(("hint", h.clone()));
    }
    rows(style, &mut out, &r);
    out
}

fn values(left: Option<i64>, right: Option<i64>) -> String {
    let n = |v: Option<i64>| v.map_or_else(|| "?".into(), |v| v.to_string());
    format!("{}/{}", n(left), n(right))
}

/// `Board Defined Alternate 2 (36/42): Stickless 16 (1), for G16, S16`.
fn board_layout_text(l: &BoardLayout) -> String {
    let mut s = format!("{} ({}/{}): {}", l.label, l.left, l.right, l.drawing);
    if !l.models.is_empty() {
        s.push_str(&format!(", for {}", l.models.join(", ")));
    }
    s
}

/// What the OLED draws: the board's drawing, or the bare values.
fn layout_text(c: &Current) -> String {
    match &c.layout {
        Some(l) => board_layout_text(l),
        None => format!("{} (left/right)", values(c.left, c.right)),
    }
}

pub fn display_layout_status(style: Style, s: &LayoutStatus, trip: Option<&Trip>) -> String {
    if style.format != Format::Human {
        return json(&WithTrip { data: s, trip });
    }
    let mut out = format!("{}\n", style.bold(&layout_text(&s.current)));
    let mut rw: Vec<(&str, String)> =
        vec![("board", s.board.clone().unwrap_or_else(|| "-".into()))];
    if !s.choices.is_empty() {
        let width = s
            .choices
            .iter()
            .map(|c| c.layout.key.len())
            .max()
            .unwrap_or(0)
            + 2;
        let choices = s
            .choices
            .iter()
            .map(|c| {
                let mut line = format!("{:<width$}{}", c.layout.key, board_layout_text(&c.layout));
                if c.current {
                    line.push_str(" (current)");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        rw.push(("layouts", choices));
    }
    rw.extend(trip.map(trip_rows).unwrap_or_default());
    rows(style, &mut out, &rw);
    let how = if s.choices.is_empty() {
        "set with: gamepadii display layout --left N --right N"
    } else {
        "set with: gamepadii display layout <model|layout> (s16, alt2, ...) or --left N --right N"
    };
    out.push_str(&style.dim(how));
    out.push('\n');
    out
}

pub fn display_layout(style: Style, r: &LayoutReport, trip: Option<&Trip>) -> String {
    if style.format != Format::Human {
        return json(&WithTrip { data: r, trip });
    }
    let head = if r.write.is_some() {
        "OLED layout set to"
    } else {
        "OLED layout already"
    };
    let mut out = format!(
        "{} {}\n",
        style.bold(head),
        style.bold(&layout_text(&r.after))
    );
    let mut rw: Vec<(&str, String)> = Vec::new();
    match &r.write {
        Some(w) => {
            rw.push(("before", layout_text(&r.before)));
            let changes = w.changed.iter().map(change).collect::<Vec<_>>().join(", ");
            rw.push((
                "saved",
                format!("{changes} (read back; nothing else changed)"),
            ));
            if let Some(b) = &r.backup {
                rw.push(("backup", b.display().to_string()));
            }
        }
        None => rw.push(("saved", "nothing written".into())),
    }
    rw.extend(trip.map(trip_rows).unwrap_or_default());
    if r.write.is_some() && (trip.is_none() || trip.is_some_and(|t| t.stayed)) {
        rw.push((
            "note",
            "the OLED shows it once the pad restarts in a gamepad mode \
             (gamepadii webconfig --exit)"
                .into(),
        ));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn backup(style: Style, b: &BackupReport, trip: Option<&Trip>) -> String {
    if style.format != Format::Human {
        return json(&WithTrip { data: b, trip });
    }
    let (board, version) = firmware_headline(&b.firmware);
    let mut out = format!(
        "{} {} {}\n",
        style.bold("Backed up"),
        style.bold(&board),
        style.dim(&version)
    );
    let total = b.endpoints.len() + b.unavailable.len();
    let mut r = vec![("file", b.path.display().to_string())];
    if total > 0 {
        r.push(("endpoints", format!("{} of {total}", b.endpoints.len())));
    } else {
        r.push((
            "endpoints",
            "none: only the config blob, over the control channel".into(),
        ));
    }
    if let Some(l) = &b.firmware_line {
        r.push(("line", line_text(l)));
    }
    if !b.unavailable.is_empty() {
        r.push(("not on this fw", b.unavailable.join("\n")));
    }
    if let Some(evidence) = b.full_config_evidence {
        let why = match evidence {
            Evidence::KnownBuild => "a known fork build",
            Evidence::ControlChannel => "fork firmware, from its control channel",
        };
        let row = match &b.full_config_error {
            Some(e) => format!("getConfig not saved ({why}): {e}"),
            None => format!("getConfig too ({why})"),
        };
        r.push(("full config", row));
    }
    match (&b.config_blob, &b.config_blob_error) {
        (Some(i), _) => r.push((
            "config blob",
            format!(
                "{}, CRC-32 {:08x} (for `gamepadii restore`)",
                bytes(u64::from(i.size)),
                i.crc32
            ),
        )),
        (None, Some(e)) => r.push(("config blob", format!("not saved: {e}"))),
        (None, None) => {}
    }
    r.push(("size", bytes(b.bytes)));
    r.push(("captured", b.captured_at.clone()));
    r.extend(trip.map(trip_rows).unwrap_or_default());
    rows(style, &mut out, &r);
    out
}

pub fn restore(style: Style, r: &RestoreReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let board = r.board.as_deref().unwrap_or("the pad");
    let head = if r.written {
        format!("Restored the config of {board}")
    } else {
        format!("No change to the config of {board}")
    };
    let mut out = format!("{}\n", style.bold(&head));
    let mut rw = vec![
        ("file", r.file.display().to_string()),
        (
            "config blob",
            format!("{}, CRC-32 {:08x}", bytes(u64::from(r.size)), r.crc32),
        ),
        (
            "from",
            r.blob_firmware
                .clone()
                .unwrap_or_else(|| "an unknown firmware".into()),
        ),
    ];
    if r.written {
        let mut sent = format!("{} chunks", r.chunks);
        if r.uploads > 1 {
            sent.push_str(&format!(
                ", in {} uploads (a save on the pad stopped the first)",
                r.uploads
            ));
        }
        rw.push(("sent", sent));
    }
    if let Some(b) = &r.backup {
        rw.push(("backup", b.display().to_string()));
    }
    rw.push(("was", seen(&r.before)));
    if let Some(a) = &r.after {
        rw.push(("now", seen(a)));
    }
    if let Some(a) = r.ack.filter(|a| *a == RebootAck::Probable) {
        rw.push(("restart", ack(a).into()));
    }
    rw.push((
        "verified",
        if r.verified {
            "yes: the saved config has the CRC-32 of the blob"
        } else {
            "no"
        }
        .into(),
    ));
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
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
                BootMode::Webconfig if r.via == Route::Control => {
                    format!("it comes back at {WEBCONFIG_URL} in a few seconds")
                }
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
    if r.via == Route::Control {
        rw.push(("via", format!("the control channel ({})", r.host)));
    }
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
        Via::Control => {
            rw.push(("was", seen(&r.before)));
            rw.push((
                "via",
                "the control channel; no web-config, no button press".into(),
            ));
            rw.push((
                "saved",
                format!("{target} (the pad saved it and restarted)"),
            ));
            if r.reboot == Some(RebootAck::Probable) {
                rw.push(("reboot", ack(RebootAck::Probable).into()));
            }
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
    let via = match r.via {
        WebconfigVia::User => "the pad is in web-config now",
        WebconfigVia::Control => "the pad rebooted into web-config over the control channel",
        WebconfigVia::Already => "the pad was already in web-config",
    };
    out.push_str(&style.dim(&format!("{via}; leave it with: gamepadii webconfig --exit")));
    out.push('\n');
    out
}

pub fn profile_switch(style: Style, r: &ProfileReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let name = format!("profile {}", r.profile.display());
    let headline = match (r.changed, r.saved, r.applied) {
        (true, _, true) => "Switched to",
        (true, _, false) => "Switching to",
        (false, true, _) => "Saved",
        (false, false, _) => "Already on",
    };
    let mut out = format!("{} {}\n", style.bold(headline), style.bold(&name));
    let mut rw: Vec<(&str, String)> = Vec::new();
    if r.changed {
        rw.push(("was", r.before.display()));
    }
    if r.changed || r.saved {
        rw.push((
            "saved",
            if r.saved {
                "yes, in flash".into()
            } else {
                "no (RAM only)".into()
            },
        ));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

/// `2 SilkSong`, or `6` with no label.
fn slot_name(profile: u8, label: &str) -> String {
    if label.is_empty() {
        profile.to_string()
    } else {
        format!("{profile} {label}")
    }
}

/// One pin of `profile show`: the GPIO, the action, and the key printed on
/// the case (profile 1's button there).
fn pin_line(p: &PinView) -> String {
    let action = match (&p.action_name, p.action) {
        (Some(n), 40) => format!("{n} {:#x}/{:#x}", p.button_mask, p.dpad_mask),
        (Some(n), _) => n.clone(),
        (None, a) => a.to_string(),
    };
    let action = if p.from_profile_1 {
        "same as profile 1".to_string()
    } else {
        action
    };
    let key = p.key.map_or_else(|| "-".into(), |b| b.both());
    format!("{:>4}  {action:<24}{key}", p.gpio)
}

pub fn profile_show(style: Style, r: &SlotReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let name = format!("Profile {}", slot_name(r.profile, &r.label));
    let mut out = format!("{}\n", style.bold(&name));
    let state = match (r.enabled, r.active) {
        (true, true) => "enabled, active",
        (true, false) => "enabled",
        (false, true) => "disabled, active",
        (false, false) => "disabled",
    };
    let pins = std::iter::once(format!("{:<6}{:<24}{}", "GPIO", "action", "key"))
        .chain(r.pins.iter().map(pin_line))
        .collect::<Vec<_>>()
        .join("\n");
    let rw = vec![
        ("state", state.to_string()),
        ("board", r.board.clone().unwrap_or_else(|| "-".into())),
        ("pins", pins),
        ("via", format!("the control channel ({})", r.host)),
    ];
    rows(style, &mut out, &rw);
    out
}

pub fn profile_write(style: Style, r: &SlotWriteReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let what = match (r.active, r.saved) {
        (true, true) => "written, active and saved",
        (true, false) => "written and active",
        (false, true) => "written and saved",
        (false, false) => "written",
    };
    let name = format!("Profile {}", slot_name(r.profile, &r.label));
    let mut out = format!("{} {what}\n", style.bold(&name));
    rows(style, &mut out, &slot_write_rows(r));
    out
}

/// The rows of `profile write`, after the headline.
fn slot_write_rows(r: &SlotWriteReport) -> Vec<(&'static str, String)> {
    let was = if r.before_label.is_empty() {
        "no label".into()
    } else {
        r.before_label.clone()
    };
    let mut rw = vec![(
        "was",
        format!("{was}{}", if r.before_enabled { "" } else { ", disabled" }),
    )];
    rw.push((
        "changed",
        match r.changed_pins.len() {
            0 => "no pins".into(),
            n => format!(
                "{n} pin{}: GPIO {}",
                if n == 1 { "" } else { "s" },
                r.changed_pins
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    ));
    if !r.enabled {
        rw.push(("enabled", "no".into()));
    }
    if let Some(a) = r.applied {
        rw.push(("applied", if a { "yes" } else { "not yet" }.into()));
    }
    rw.push((
        "saved",
        if r.saved {
            "yes, in flash".into()
        } else {
            "no (RAM only)".into()
        },
    ));
    if let Some(b) = &r.backup {
        rw.push(("backup", b.display().to_string()));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rw
}

/// `name  base  label  file:line` lines with the columns padded.
fn table(head: &[&str], lines: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..head.len())
        .map(|i| {
            lines
                .iter()
                .map(|l| l[i].chars().count())
                .chain([head[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: Vec<String>| {
        let last = cells.len() - 1;
        cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| match i == last {
                true => c,
                false => format!("{c:<w$}  ", w = widths[i]),
            })
            .collect::<String>()
    };
    std::iter::once(line(head.iter().map(|h| h.to_string()).collect()))
        .chain(lines.iter().map(|l| line(l.clone())))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn library_list(style: Style, r: &ListReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let n = r.profiles.len();
    let head = format!(
        "{n} profile{} in the library",
        if n == 1 { "" } else { "s" }
    );
    let mut out = format!("{}\n", style.bold(&head));
    let mut rw = vec![(
        "directory",
        r.dir
            .as_ref()
            .map_or_else(|| "- (no $HOME)".into(), |d| d.display().to_string()),
    )];
    if !r.profiles.is_empty() {
        let lines: Vec<Vec<String>> = r
            .profiles
            .iter()
            .map(|p| {
                vec![
                    p.name.clone(),
                    p.base.clone().unwrap_or_else(|| "-".into()),
                    p.label.clone(),
                    format!(
                        "{}:{}{}",
                        p.file,
                        p.line,
                        if p.ok { "" } else { " (see the errors)" }
                    ),
                ]
            })
            .collect();
        rw.push((
            "profiles",
            table(&["name", "base", "label", "file"], &lines),
        ));
    }
    if !r.errors.is_empty() {
        let errors = r
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        rw.push(("errors", errors));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn library_show(style: Style, r: &Resolved) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let mut out = format!("{}\n", style.bold(&format!("Profile {}", r.name)));
    let lines: Vec<Vec<String>> = r
        .pins
        .iter()
        .map(|p| {
            let action = match (&p.action_name, p.action) {
                (Some(n), Some(40)) => format!("{n} {:#x}/{:#x}", p.button_mask, p.dpad_mask),
                (Some(n), _) => n.clone(),
                (None, Some(a)) => a.to_string(),
                (None, None) => "same as profile 1".into(),
            };
            vec![
                p.button.clone(),
                format!("{:>4}", p.gpio),
                action,
                p.from.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    let rw = vec![
        ("file", format!("{}:{}", r.file, r.line)),
        (
            "bases",
            match r.bases.is_empty() {
                true => "none".into(),
                false => r.bases.join(" > "),
            },
        ),
        ("board", format!("{} ({})", r.board, r.firmware_board)),
        ("label", r.label.clone()),
        (
            "input mode",
            r.input_mode
                .map_or_else(|| "any".into(), |m| m.label().to_string()),
        ),
        ("pins", table(&["button", "GPIO", "action", "from"], &lines)),
    ];
    rows(style, &mut out, &rw);
    out
}

/// The slot file of a library profile: always JSON, for `profile write`.
pub fn slot_file(r: &Resolved) -> String {
    serde_json::to_string_pretty(&r.slot_file()).expect("serialising plain data cannot fail")
}

/// The KDL on standard output; with `--write`, what went where.
pub fn library_pull(style: Style, r: &PullReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let Some(file) = &r.file else {
        return r.kdl.clone();
    };
    let n = r.profiles.len();
    let head = format!(
        "{n} profile{} pulled into {}",
        if n == 1 { "" } else { "s" },
        file.display()
    );
    let mut out = format!("{}\n", style.bold(&head));
    let names = r
        .profiles
        .iter()
        .zip(&r.slots)
        .map(|(name, n)| format!("{n} {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut rw = vec![
        ("profiles", names),
        ("board", format!("{} ({})", r.board, r.firmware_board)),
    ];
    if !r.skipped.is_empty() {
        let s = r
            .skipped
            .iter()
            .map(|s| {
                format!(
                    "profile {}, GPIO {}: {}",
                    s.profile,
                    s.gpio,
                    s.action_name
                        .clone()
                        .unwrap_or_else(|| s.action.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        rw.push(("not pulled", s));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    rows(style, &mut out, &rw);
    out
}

/// One pin of `library diff`: its action, or profile 1's.
fn pin_state(p: &PinState) -> String {
    let name = |n: &Option<String>, a: i16| match (n, a) {
        (Some(n), 40) => format!("{n} {:#x}/{:#x}", p.button_mask, p.dpad_mask),
        (Some(n), _) => n.clone(),
        (None, a) => a.to_string(),
    };
    if p.from_profile_1 {
        format!(
            "same as profile 1 ({})",
            name(&p.pad_action_name, p.pad_action)
        )
    } else {
        name(&p.action_name, p.action)
    }
}

/// `X -> Y`, or `X` when it does not change.
fn now_after<T: PartialEq>(c: &Change<T>, show: impl Fn(&T) -> String) -> String {
    match c.changed() {
        true => format!("{} -> {}", show(&c.now), show(&c.after)),
        false => show(&c.now),
    }
}

/// The rows of a diff: the slot, the label, the state, the pins.
fn diff_rows(r: &DiffReport) -> Vec<(&'static str, String)> {
    let label = |l: &String| match l.is_empty() {
        true => "no label".to_string(),
        false => l.clone(),
    };
    let state = |e: &bool| match e {
        true => "enabled".to_string(),
        false => "disabled".to_string(),
    };
    let mut rw = vec![
        (
            "slot",
            format!(
                "profile {}{}{}",
                r.slot,
                if r.active { ", active" } else { "" },
                if r.slot_by_label {
                    " (it has the label of the profile)"
                } else {
                    ""
                }
            ),
        ),
        ("label", now_after(&r.label, label)),
        ("state", now_after(&r.enabled, state)),
        ("board", format!("{} ({})", r.board, r.firmware_board)),
    ];
    let lines: Vec<Vec<String>> = r
        .pins
        .iter()
        .filter(|p| !p.same_action)
        .map(|p| {
            vec![
                p.button.clone().unwrap_or_else(|| "-".into()),
                format!("{:>4}", p.gpio),
                pin_state(&p.now),
                pin_state(&p.after),
            ]
        })
        .collect();
    rw.push((
        "pins",
        match lines.is_empty() {
            true => "no change".into(),
            false => table(&["button", "GPIO", "now", "after"], &lines),
        },
    ));
    let same: Vec<String> = r
        .pins
        .iter()
        .filter(|p| p.same_action)
        .map(|p| match &p.button {
            Some(b) => b.clone(),
            None => format!("GPIO {}", p.gpio),
        })
        .collect();
    if !same.is_empty() {
        rw.push((
            "same action",
            format!(
                "{} pin{}: the stored value changes, the action does not{}\n{}",
                same.len(),
                if same.len() == 1 { "" } else { "s" },
                if r.changed {
                    ""
                } else {
                    "; for this alone, apply writes nothing"
                },
                same.join(", ")
            ),
        ));
    }
    rw.push((
        "input mode",
        match (r.input_mode, r.pad_input_mode) {
            (None, _) => "any".into(),
            (Some(m), Some(p)) if m == p => format!("{} (the pad runs it)", m.label()),
            (Some(m), Some(p)) => format!("{} (the pad runs {})", m.label(), p.label()),
            (Some(m), None) => m.label().to_string(),
        },
    ));
    rw
}

pub fn library_diff(style: Style, r: &DiffReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let n = r.pins.iter().filter(|p| !p.same_action).count()
        + usize::from(r.label.changed())
        + usize::from(r.enabled.changed());
    let what = match n {
        0 => "no change".to_string(),
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    };
    let head = format!("Profile {} against profile {}: {what}", r.name, r.slot);
    let mut out = format!("{}\n", style.bold(&head));
    let mut rw = diff_rows(r);
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn library_apply(style: Style, r: &ApplyReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let d = &r.diff;
    let Some(w) = &r.write else {
        let head = format!(
            "Profile {} is in profile {} already: nothing written",
            d.name, d.slot
        );
        let mut out = format!("{}\n", style.bold(&head));
        let mut rw = diff_rows(d);
        rw.push(("via", format!("the control channel ({})", d.host)));
        if let Some(n) = &d.note {
            rw.push(("note", n.clone()));
        }
        rows(style, &mut out, &rw);
        return out;
    };
    let what = match (w.active, w.saved) {
        (true, true) => ", active and saved",
        (true, false) => " and active",
        (false, true) => " and saved",
        (false, false) => "",
    };
    let head = format!(
        "Profile {} written to profile {}{what}",
        d.name,
        slot_name(w.profile, &w.label)
    );
    let mut out = format!("{}\n", style.bold(&head));
    let mut rw = diff_rows(d);
    rw.retain(|(k, _)| !matches!(*k, "slot" | "state"));
    // From the write: what the slot was, the backup, the notes.
    let mut notes = Vec::new();
    for (k, v) in slot_write_rows(w) {
        match k {
            "changed" | "enabled" => {}
            "note" => notes.push(v),
            _ => rw.push((k, v)),
        }
    }
    notes.extend(d.note.clone());
    if !notes.is_empty() {
        rw.push(("note", notes.join("; ")));
    }
    rows(style, &mut out, &rw);
    out
}

/// A list of colour indexes: `red ×30`, the runs in order.
fn color_runs(b: &[u8]) -> String {
    let name = |c: u8| match gamepadii_core::leds::COLORS.get(usize::from(c)) {
        Some(n) if usize::from(c) < lights::BASE_COLORS => (*n).to_string(),
        _ if usize::from(c) >= lights::BASE_COLORS + lights::MAX_CUSTOM_COLORS => {
            format!("colour {c} (out of range)")
        }
        _ => format!(
            "custom {}",
            usize::from(c).saturating_sub(lights::BASE_COLORS)
        ),
    };
    let mut runs: Vec<(u8, usize)> = Vec::new();
    for c in b {
        match runs.last_mut() {
            Some((last, n)) if last == c => *n += 1,
            _ => runs.push((*c, 1)),
        }
    }
    let mut parts: Vec<String> = runs
        .iter()
        .take(4)
        .map(|(c, n)| format!("{} ×{n}", name(*c)))
        .collect();
    if runs.len() > 4 {
        parts.push("...".into());
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// The word for a flag or number field in `leds profile show`.
fn lights_label(key: &str) -> &'static str {
    match key {
        "case_in_special_moves" => "in special moves",
        "case_in_pressed" => "in pressed animations",
        "press_hold_ms" => "hold",
        "press_fade_ms" => "fade",
        "effect_param" => "param",
        k if k.ends_with("special_rainbow") => "special rainbow",
        _ => "",
    }
}

/// The parts of one row of `leds profile show`: the fields whose keys
/// start with one of `prefixes`, as text, in table order.
fn lights_group(file: &lights::LightsFile, prefixes: &[&str]) -> String {
    let mut parts = Vec::new();
    for def in lights::FIELDS
        .iter()
        .filter(|f| prefixes.iter().any(|p| f.key.starts_with(p)))
    {
        let Some(v) = file.values.get(&def.number) else {
            continue;
        };
        let what = lights_label(def.key);
        let n = v.as_u32().unwrap_or_default();
        let text = match def.kind {
            Kind::Effect(table) => table
                .get(n as usize)
                .map_or_else(|| format!("effect {n}"), |e| (*e).to_string()),
            Kind::Cycle => format!("cycle {}", v.as_i32().unwrap_or_default()),
            Kind::Color => format!("special #{n:06x}"),
            Kind::Flag => format!("{what}: {}", if n != 0 { "yes" } else { "no" }),
            Kind::Colors(_) => match v {
                gamepadii_core::pb::Value::Bytes(b) => format!("colours {}", color_runs(b)),
                _ => continue,
            },
            Kind::Number if def.key.ends_with("_ms") => format!("{what} {n} ms"),
            Kind::Number => format!("{what} {n:#x}"),
        };
        parts.push(text);
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(", ")
    }
}

/// The rows of a lights profile: its state, then its fields by group.
fn profile_rows(f: &lights::LightsFile, active: bool) -> Vec<(&'static str, String)> {
    let state = match (f.enabled == Some(true), active) {
        (true, true) => "enabled, runs now",
        (true, false) => "enabled",
        (false, true) => "disabled, runs now",
        (false, false) => "disabled",
    };
    vec![
        ("state", state.to_string()),
        ("board", f.board.clone().unwrap_or_else(|| "-".into())),
        ("idle", lights_group(f, &["idle_"])),
        ("pressed", lights_group(f, &["pressed_", "press_"])),
        ("case", lights_group(f, &["case_"])),
        ("effects", lights_group(f, &["effect_"])),
    ]
}

pub fn lights_show(style: Style, r: &LightsReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let f = &r.file;
    let n = f.profile.unwrap_or_default();
    let mut out = format!("{}\n", style.bold(&format!("Lights profile {n}")));
    let mut rw = profile_rows(f, r.active);
    if !f.other.is_empty() {
        rw.push((
            "other fields",
            format!(
                "{} that gamepadii doesn't know (kept in the JSON)",
                f.other.len()
            ),
        ));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    rows(style, &mut out, &rw);
    out
}

pub fn lights_write(style: Style, r: &LightsWriteReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let what = match (r.active, r.saved) {
        (true, true) => "written, runs now, and saved",
        (true, false) => "written, runs now",
        (false, true) => "written and saved",
        (false, false) => "written",
    };
    let name = format!("Lights profile {}", r.profile);
    let mut out = format!("{} {what}\n", style.bold(&name));
    let mut rw = vec![
        (
            "was",
            if r.before_enabled {
                "enabled"
            } else {
                "disabled"
            }
            .to_string(),
        ),
        (
            "changed",
            if r.changed.is_empty() {
                "nothing".into()
            } else {
                r.changed.join(", ")
            },
        ),
    ];
    if !r.enabled {
        rw.push(("enabled", "no".into()));
    }
    rw.push(("applied", if r.applied { "yes" } else { "not yet" }.into()));
    rw.push((
        "saved",
        if r.saved {
            "yes, in flash".into()
        } else {
            "no (RAM only)".into()
        },
    ));
    if let Some(b) = &r.backup {
        rw.push(("backup", b.display().to_string()));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

/// The custom colours, one line each: `14  #ff8800  custom 0`. The colours
/// after the count that are all black go on one line.
fn custom_color_lines(entries: &[ColorEntry]) -> String {
    let line = |e: &ColorEntry| {
        let not_set = if e.set { "" } else { "  (not set)" };
        format!(
            "{:<3} {}  custom {}{not_set}",
            e.number,
            e.color.hex(),
            e.custom
        )
    };
    let unset: Vec<&ColorEntry> = entries.iter().filter(|e| !e.set).collect();
    let mut lines: Vec<String> = entries.iter().filter(|e| e.set).map(line).collect();
    match unset.as_slice() {
        [] => {}
        all if all.iter().all(|e| e.color == proto::Rgb::BLACK) => {
            let (first, last) = (all[0].number, all[all.len() - 1].number);
            lines.push(if first == last {
                format!("{first}: not set (black)")
            } else {
                format!("{first} to {last}: not set (black)")
            });
        }
        all => lines.extend(all.iter().map(|e| line(e))),
    }
    lines.join("\n")
}

/// `custom 0, 2`, or `nothing`.
fn custom_list(changed: &[u8]) -> String {
    if changed.is_empty() {
        return "nothing".into();
    }
    let list: Vec<String> = changed.iter().map(u8::to_string).collect();
    format!("custom {}", list.join(", "))
}

/// The custom colours of a theme: `14 #ff8800, 15 #0044ff`.
fn custom_colors_text(c: &Option<Vec<proto::Rgb>>) -> String {
    match c {
        None => "-".into(),
        Some(c) if c.is_empty() => "none set".into(),
        Some(c) => c
            .iter()
            .enumerate()
            .map(|(n, c)| format!("{} {}", colors::number(n), c.hex()))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

pub fn colors_show(style: Style, r: &ColorsReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let mut out = format!("{}\n", style.bold("Custom colours"));
    let rw = vec![
        ("set", format!("{} of {}", r.colors.count, r.colors.max)),
        ("colours", custom_color_lines(&r.entries())),
        ("board", r.board.clone().unwrap_or_else(|| "-".into())),
        ("via", format!("the control channel ({})", r.host)),
    ];
    rows(style, &mut out, &rw);
    out
}

/// `written`, `written and saved`.
fn saved_word(saved: bool) -> &'static str {
    if saved {
        "written and saved"
    } else {
        "written"
    }
}

fn saved_row(saved: bool) -> (&'static str, String) {
    (
        "saved",
        if saved {
            "yes, in flash".into()
        } else {
            "no (RAM only)".into()
        },
    )
}

pub fn colors_write(style: Style, r: &ColorsWriteReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let name = style.bold("Custom colours");
    let mut out = format!("{name} {}\n", saved_word(r.saved));
    let mut rw = vec![
        ("was", format!("{} set", r.before_count)),
        ("set", format!("{} of 16", r.count)),
        ("changed", custom_list(&r.changed)),
        ("applied", if r.applied { "yes" } else { "not yet" }.into()),
        saved_row(r.saved),
    ];
    if let Some(b) = &r.backup {
        rw.push(("backup", b.display().to_string()));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn theme_show(style: Style, r: &ThemeReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let f = r.theme.lights.clone().unwrap_or_default();
    let n = f.profile.unwrap_or_default();
    let mut out = format!(
        "{}\n",
        style.bold(&format!("Theme: lights profile {n} and the custom colours"))
    );
    let mut rw = profile_rows(&f, r.active);
    rw.insert(1, ("colours", custom_colors_text(&r.theme.colors)));
    rw.push(("via", format!("the control channel ({})", r.host)));
    rows(style, &mut out, &rw);
    out
}

pub fn theme_apply(style: Style, r: &ThemeApplyReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let what = match (r.active, r.saved) {
        (true, true) => "runs now, and saved",
        (true, false) => "runs now",
        (false, true) => "saved",
        (false, false) => "written",
    };
    let name = format!("Theme on lights profile {}", r.profile);
    let mut out = format!("{} {what}\n", style.bold(&name));
    let colours = match r.colors {
        Some(n) => format!("{n} set, changed {}", custom_list(&r.colors_changed)),
        None => "not in the theme".into(),
    };
    let mut rw = vec![
        ("colours", colours),
        (
            "changed",
            if r.lights_changed.is_empty() {
                "nothing".into()
            } else {
                r.lights_changed.join(", ")
            },
        ),
        ("applied", if r.applied { "yes" } else { "not yet" }.into()),
        saved_row(r.saved),
    ];
    if let Some(b) = &r.backup {
        rw.push(("backup", b.display().to_string()));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

/// `B1 (A)` for a gamepad button, `ddi-up` for the rest.
fn button_label(name: &str) -> String {
    Button::from_name(name).map_or_else(|| name.to_string(), Button::both)
}

/// `2-21, 26, 27`: GPIOs, with runs of three or more as ranges.
fn gpio_ranges(gpios: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < gpios.len() {
        let mut j = i;
        while j + 1 < gpios.len() && gpios[j + 1] == gpios[j] + 1 {
            j += 1;
        }
        if j >= i + 2 {
            parts.push(format!("{}-{}", gpios[i], gpios[j]));
        } else {
            parts.extend(gpios[i..=j].iter().map(u8::to_string));
        }
        i = j + 1;
    }
    parts.join(", ")
}

/// The action of a pin, for people: its name, its number, or `-`.
fn pin_action(p: &PinInfo) -> String {
    match (&p.action_name, p.action) {
        (Some(n), _) => n.clone(),
        (None, Some(a)) => a.to_string(),
        (None, None) => "-".into(),
    }
}

/// Where the names come from, for the `profile` row.
fn names_from(profile: Option<u8>, from: NamesFrom) -> String {
    let p = profile.map_or_else(|| "-".to_string(), |n| n.to_string());
    match from {
        NamesFrom::Slot => p,
        NamesFrom::Cache => format!("{p} (names from the device cache)"),
        NamesFrom::None => format!("{p} (no pin names: gamepadii has no pin mappings)"),
    }
}

fn sticks(s: &Sticks) -> String {
    if s.at_rest() {
        return "at rest".into();
    }
    format!(
        "lx {} ly {} rx {} ry {}, lt {} rt {}",
        s.lx, s.ly, s.rx, s.ry, s.lt, s.rt
    )
}

fn names_or_dash(names: &[&str], label: fn(&str) -> String) -> String {
    if names.is_empty() {
        "-".into()
    } else {
        names
            .iter()
            .map(|n| label(n))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn input(style: Style, r: &InputReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let headline = match r.pressed.len() {
        0 => "Nothing pressed".to_string(),
        1 => "1 pin pressed".to_string(),
        n => format!("{n} pins pressed"),
    };
    let mut out = format!("{}\n", style.bold(&headline));
    let mut rw = Vec::new();
    if !r.pressed.is_empty() {
        let pins = std::iter::once(format!("{:<6}{:<24}{}", "GPIO", "action", "key"))
            .chain(r.pressed.iter().map(|p| {
                let key = p.key.map_or_else(|| "-".into(), |b| b.both());
                format!("{:>4}  {:<24}{key}", p.gpio, pin_action(p))
            }))
            .collect::<Vec<_>>()
            .join("\n");
        rw.push(("pressed", pins));
    }
    rw.push(("dpad", names_or_dash(&r.dpad, str::to_string)));
    rw.push(("buttons", names_or_dash(&r.buttons, button_label)));
    rw.push(("sticks", sticks(&r.sticks)));
    rw.push(("profile", names_from(r.profile, r.names_from)));
    rw.push((
        "readable",
        format!("{} GPIOs: {}", r.readable.len(), gpio_ranges(&r.readable)),
    ));
    if r.scan_on {
        rw.push(("pin scan", "on".into()));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    rows(style, &mut out, &rw);
    out
}

/// One line of `input --watch`: JSON with no line breaks, or text.
pub fn input_line(style: Style, r: &InputReport) -> String {
    if style.format != Format::Human {
        return serde_json::to_string(&Envelope {
            schema: SCHEMA,
            data: r,
        })
        .expect("serialising plain data cannot fail");
    }
    let pins = if r.pressed.is_empty() {
        "-".to_string()
    } else {
        r.pressed
            .iter()
            .map(|p| {
                let action = pin_action(p);
                match p.key.map(Button::both) {
                    Some(k) if k != action && Some(k.as_str()) != p.action_name.as_deref() => {
                        format!("GPIO {} {action} on key {k}", p.gpio)
                    }
                    _ => format!("GPIO {} {action}", p.gpio),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut line = format!(
        "pressed: {pins}; dpad: {}; buttons: {}",
        names_or_dash(&r.dpad, str::to_string),
        names_or_dash(&r.buttons, button_label)
    );
    if !r.sticks.at_rest() {
        line.push_str(&format!("; sticks: {}", sticks(&r.sticks)));
    }
    line
}

pub fn find_pin(style: Style, r: &FoundPin) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let action = pin_action(&r.pin);
    let headline = match r.no_action {
        Some(true) => format!("GPIO {}: no action", r.pin.gpio),
        _ if r.pin.action.is_none() => format!("GPIO {}", r.pin.gpio),
        _ => format!("GPIO {}: {action}", r.pin.gpio),
    };
    let mut out = format!("{}\n", style.bold(&headline));
    let mut rw = vec![
        ("action", action),
        ("key", r.pin.key.map_or_else(|| "-".into(), |b| b.both())),
        ("profile", names_from(r.profile, r.names_from)),
        (
            "pin scan",
            match (r.scan, r.note.is_some()) {
                (false, _) => "not used",
                (true, false) => "used, now off",
                // The note says why: PIN_SCAN 0 failed.
                (true, true) => "used, still on",
            }
            .into(),
        ),
    ];
    if !r.released {
        rw.push((
            "released",
            format!(
                "no: the button was still held after {} s",
                flow::FindPin::RELEASE.as_secs()
            ),
        ));
    }
    rw.push(("via", format!("the control channel ({})", r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn udev(style: Style, r: &udev::Report) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let state = match (r.action, r.installed) {
        (Some(udev::Action::Installed), _) => "installed now",
        (Some(udev::Action::None), _) => "already installed, nothing to do",
        (None, udev::Installed::No) => "not installed",
        (None, udev::Installed::Current) => "installed, up to date",
        (None, udev::Installed::Differs) => "installed, but not this gamepadii's rule",
    };
    let mut out = format!(
        "{} {}\n",
        style.bold("udev rule for the control channel:"),
        style.bold(state)
    );
    let width = r.grants.iter().map(|g| g.usb_id.len()).max().unwrap_or(0) + 2;
    let ids = r
        .grants
        .iter()
        .map(|g| {
            let modes = g.modes.iter().map(|m| m.label()).collect::<Vec<_>>();
            let mut line = format!("{:<width$}{}", g.usb_id, modes.join(", "));
            if let Some(who) = g.also_matches {
                line.push_str(&format!("; also {who}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut rw: Vec<(&str, String)> = vec![
        ("file", r.path.to_string()),
        (
            "grants",
            "read-write access to the USB device node (/dev/bus/usb/BBB/DDD)\n\
             of the ids below, for the user at the seat (uaccess); gamepadii\n\
             sends the control channel's requests there. Input devices\n\
             (evdev, hidraw) are untouched."
                .into(),
        ),
        ("ids", ids),
        (
            "caution",
            "GP2040-CE borrows these ids from real controllers, so a real\n\
             Xbox 360, HORI, DualShock or Switch pad plugged in here gets the\n\
             same access."
                .into(),
        ),
    ];
    match (r.action, r.installed) {
        (Some(_), _) => {}
        (None, udev::Installed::Current) => {
            rw.push(("remove", format!("sudo rm {}", r.path)));
        }
        (None, _) => rw.push((
            "install",
            "gamepadii setup udev --install (asks for your password\n\
             through pkexec), or gamepadii setup udev --print"
                .into(),
        )),
    }
    rows(style, &mut out, &rw);
    out
}

// ---- leds -------------------------------------------------------------------------

/// A level, or `-`.
fn level(n: Option<u8>) -> String {
    n.map_or_else(|| "-".into(), |n| n.to_string())
}

/// What a field shows when the route can't read it.
fn unknown(r: Route) -> String {
    match r {
        Route::WebConfig => "unknown on stock firmware".into(),
        Route::Control => "-".into(),
    }
}

fn named(n: &Option<Named>, via: Route) -> String {
    n.as_ref().map_or_else(|| unknown(via), Named::display)
}

/// `1 of 5 (maximum 100/255)`, or what is known of the limits.
fn brightness(s: &LedState, via: Route) -> String {
    let max = s.brightness_maximum.map(|m| format!("maximum {m}/255"));
    let (level, limits) = match (s.brightness, s.brightness_steps) {
        (Some(b), Some(st)) => (format!("{b} of {st}"), max.into_iter().collect()),
        (Some(b), None) => (b.to_string(), max.into_iter().collect()),
        (None, st) => (
            unknown(via),
            st.map(|st| format!("{st} steps"))
                .into_iter()
                .chain(max)
                .collect::<Vec<_>>(),
        ),
    };
    if limits.is_empty() {
        level
    } else {
        format!("{level} ({})", limits.join(", "))
    }
}

fn on_off(b: Option<bool>) -> String {
    match b {
        Some(true) => "on".into(),
        Some(false) => "off".into(),
        None => "-".into(),
    }
}

/// A lights profile: `profile 2`, or `off`.
fn lights_profile(s: &LedState) -> String {
    match (s.lights_off, s.lights_profile) {
        (Some(true), _) => "off".into(),
        (_, Some(n)) => format!("profile {n}"),
        _ => "-".into(),
    }
}

/// The rows of an LED state.
fn led_rows(s: &LedState, via: Route) -> Vec<(&'static str, String)> {
    if s.lights_off.is_some() {
        // Main firmware: no animation, colours or theme of the v0.7.12 kind.
        let mut rw = vec![
            ("lights", lights_profile(s)),
            ("brightness", brightness(s, via)),
        ];
        if s.unsaved == Some(true) {
            rw.push(("saved", "no: changed since the last save (RAM only)".into()));
        }
        return rw;
    }
    let mut rw = vec![
        ("animation", named(&s.animation, via)),
        ("brightness", brightness(s, via)),
        ("static color", named(&s.static_color, via)),
        ("button color", named(&s.button_color, via)),
        ("theme", named(&s.theme, via)),
        ("custom theme", on_off(s.custom_theme)),
    ];
    if s.unsaved == Some(true) {
        rw.push(("saved", "no: changed since the last save (RAM only)".into()));
    }
    rw
}

fn via_text(via: Route, host: &str) -> String {
    match via {
        Route::Control => format!("the control channel ({host})"),
        Route::WebConfig => format!("web-config ({host})"),
    }
}

/// The custom theme's colours, three buttons a line.
fn theme_colors(c: &[ButtonColor]) -> String {
    let cell = |b: &ButtonColor| {
        format!(
            "{:<6}{}/{}",
            b.button,
            b.normal.as_deref().unwrap_or("-"),
            b.pressed.as_deref().unwrap_or("-")
        )
    };
    c.chunks(3)
        .map(|row| row.iter().map(cell).collect::<Vec<_>>().join("  "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Main firmware's lights profiles: one line each, the one that runs
/// marked.
fn lights_rows(l: &Lights) -> Vec<(&'static str, String)> {
    let name = |n: &Option<Named>| n.as_ref().map_or_else(|| "-".into(), Named::display);
    let profiles = l
        .profiles
        .iter()
        .map(|p| {
            let mark = if l.profile == Some(p.index) { "*" } else { " " };
            let off = if p.enabled == Some(false) {
                ", disabled"
            } else {
                ""
            };
            if p.idle_effect.is_none() && p.pressed_effect.is_none() && p.case_effect.is_none() {
                // The channel gives the effects of the running profile only.
                return format!("{mark}{}{off}", p.index);
            }
            format!(
                "{mark}{} idle {}, pressed {}, case {}{off}",
                p.index,
                name(&p.idle_effect),
                name(&p.pressed_effect),
                name(&p.case_effect)
            )
        })
        .collect::<Vec<_>>();
    let mut rw = vec![(
        "profiles",
        if profiles.is_empty() {
            "-".into()
        } else {
            profiles.join("\n")
        },
    )];
    if let Some(n) = l.light_count {
        rw.push(("light count", n.to_string()));
    }
    if let Some(s) = l.idle_timeout_s {
        let v = if s == 0 {
            "never".into()
        } else {
            format!("{s} s")
        };
        rw.push(("idle timeout", v));
    }
    rw
}

pub fn leds(style: Style, r: &LedReport, trip: Option<&Trip>) -> String {
    if style.format != Format::Human {
        return json(&WithTrip { data: r, trip });
    }
    let head = match r.state.running {
        Some(false) => "LEDs off (not set up)",
        _ => "LEDs",
    };
    let mut out = format!("{}\n", style.bold(head));
    let mut rw = led_rows(&r.state, r.via);
    if r.lights.is_some() && r.state.lights_off.is_none() {
        // Main firmware: no animation, colours or theme of the v0.7.12 kind.
        rw.retain(|(k, _)| *k == "brightness");
    }
    if let Some(c) = &r.custom_theme_colors {
        rw.push(("colours", theme_colors(c)));
    }
    if let Some(l) = &r.lights {
        rw.extend(lights_rows(l));
    }
    if trip.is_none() {
        rw.push(("via", via_text(r.via, &r.host)));
    }
    rw.extend(trip.map(trip_rows).unwrap_or_default());
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

pub fn leds_set(style: Style, r: &LedSetReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let headline = match (r.changed.is_empty(), r.saved) {
        (false, true) => "LEDs changed and saved",
        (false, false) => "LEDs changed",
        (true, true) => "LEDs saved",
        (true, false) => "LEDs unchanged",
    };
    let mut out = format!("{}\n", style.bold(headline));
    let mut rw = Vec::new();
    for field in &r.changed {
        let (label, was, now) = match *field {
            "animation" => (
                "animation",
                named(&r.before.animation, r.via),
                named(&r.leds.animation, r.via),
            ),
            "brightness" => (
                "brightness",
                level(r.before.brightness),
                format!(
                    "{} of {}",
                    level(r.leds.brightness),
                    level(r.leds.brightness_steps)
                ),
            ),
            "static_color" => (
                "static color",
                named(&r.before.static_color, r.via),
                named(&r.leds.static_color, r.via),
            ),
            "button_color" => (
                "button color",
                named(&r.before.button_color, r.via),
                named(&r.leds.button_color, r.via),
            ),
            "lights_profile" => ("lights", lights_profile(&r.before), lights_profile(&r.leds)),
            _ => (
                "theme",
                named(&r.before.theme, r.via),
                named(&r.leds.theme, r.via),
            ),
        };
        rw.push((label, format!("{now}, was {was}")));
    }
    if r.changed.is_empty() {
        rw.extend(led_rows(&r.leds, r.via));
    } else {
        rw.push((
            "saved",
            if r.saved {
                "yes, in flash".into()
            } else {
                "no (RAM only)".into()
            },
        ));
    }
    rw.push(("via", via_text(r.via, &r.host)));
    if let Some(n) = &r.note {
        rw.push(("note", n.clone()));
    }
    rows(style, &mut out, &rw);
    out
}

/// Nerd Font nf-md-lightbulb (U+F0335), the waybar `text` glyph for `leds`.
pub const LED_GLYPH: &str = "\u{f0335}";

/// `leds --format waybar`: the brightness in the bar, the rest in the
/// tooltip; the class is the animation's name (`custom-theme`, ...), or
/// on main the lights profile (`lights-2`, `lights-off`).
pub fn waybar_leds(r: &LedReport) -> Waybar {
    let s = &r.state;
    let text = match (s.brightness, s.brightness_steps) {
        (Some(b), Some(st)) => format!("{LED_GLYPH} {b}/{st}"),
        (Some(b), None) => format!("{LED_GLYPH} {b}"),
        _ => LED_GLYPH.to_string(),
    };
    let class = match (s.lights_off, &s.animation) {
        (Some(true), _) => "lights-off".into(),
        (Some(false), _) => format!("lights-{}", level(s.lights_profile)),
        (None, Some(a)) => a.name.clone().unwrap_or_else(|| "leds".into()),
        (None, None) => "leds".into(),
    };
    let mut tooltip = "LEDs".to_string();
    for (label, value) in led_rows(s, r.via) {
        tooltip.push_str(&format!("\n{label}: {value}"));
    }
    Waybar {
        text,
        tooltip,
        alt: class.clone(),
        class,
    }
}

/// `log`: a headline, then one line for each record.
pub fn log(style: Style, r: &LogReport) -> String {
    if style.format != Format::Human {
        return json(r);
    }
    let board = r.board.as_deref().unwrap_or("The pad's");
    let mut out = format!(
        "{}\n",
        style.bold(&format!(
            "{board} log, boot {} (next record {})",
            r.boot, r.next
        ))
    );
    let mut rw = vec![("via", via_text(r.via, &r.host))];
    if r.records.is_empty() {
        rw.push(("records", "none".into()));
    }
    rows(style, &mut out, &rw);
    out.push_str(&log_lines(style, r));
    out
}

/// `log --follow`: the first read as [`log`], then only the new records.
/// JSON is one line for each read; a later read with no new records gives
/// nothing.
pub fn log_follow(style: Style, r: &LogReport, first: bool) -> String {
    if !first && r.records.is_empty() && r.lost == 0 {
        return String::new();
    }
    match style.format {
        Format::Human if first => log(style, r),
        Format::Human => log_lines(style, r),
        _ => format!(
            "{}\n",
            serde_json::to_string(&Envelope {
                schema: SCHEMA,
                data: r,
            })
            .expect("serialising plain data cannot fail")
        ),
    }
}

/// The record lines, after a note on lost records.
fn log_lines(style: Style, r: &LogReport) -> String {
    let mut out = String::new();
    if r.lost > 0 {
        let first = r.records.first().map_or(r.next, |e| e.seq);
        out.push_str(&style.dim(&format!(
            "{} records before record {first} are gone (the pad keeps the last 64)\n",
            r.lost
        )));
    }
    for e in &r.records {
        let name = e
            .code_name
            .map_or_else(|| format!("code {}", e.code), str::to_string);
        out.push_str(&format!(
            "{:>5}  {} {:>4}.{:03} s  {}{}\n",
            e.seq,
            style.dim(&format!("boot {:<3}", e.boot)),
            e.ms / 1000,
            e.ms % 1000,
            style.bold(&format!("{name:<9}")),
            log_fields(e)
        ));
    }
    out
}

/// The decoded fields of a record, for people.
fn log_fields(e: &LogEntry) -> String {
    let mode = |m: u8| format!("mode {}", proto::log_boot_mode(m));
    match &e.event {
        LogEvent::Boot {
            kept,
            watchdog,
            mode: m,
            version,
        } => {
            let flags = u8::from(*kept) | (u8::from(*watchdog) << 1);
            let mut why = vec![if *kept { "kept" } else { "new" }];
            if *watchdog {
                why.push("watchdog");
            }
            let mut s = format!("flags {flags} ({}) {}", why.join(", "), mode(*m));
            if !version.is_empty() {
                s.push_str(&format!(" version {version}"));
            }
            s
        }
        LogEvent::Panic { text } | LogEvent::Error { text } => format!("'{text}'"),
        LogEvent::Command { cmd, result, seq } => {
            let cmd = log::command_name(*cmd).map_or_else(|| format!("cmd {cmd:#04x}"), Into::into);
            let result = result
                .name()
                .map_or_else(|| format!("result {}", result.byte()), Into::into);
            format!("{cmd} {result} seq {seq}")
        }
        LogEvent::Profile { old, new } => format!("profile {old} to {new}"),
        LogEvent::Save { force, restart } => {
            let yes = |b: bool| if b { "yes" } else { "no" };
            format!("force {}, restart {}", yes(*force), yes(*restart))
        }
        LogEvent::Restart { mode: m } => mode(*m),
        LogEvent::UsbHost {
            mounted,
            address,
            vid,
            pid,
        } => format!(
            "{} {vid:04x}:{pid:04x} at address {address}",
            if *mounted { "mount" } else { "unmount" }
        ),
        LogEvent::Unknown { .. } if e.raw.is_empty() => "no data".into(),
        LogEvent::Unknown { .. } => format!("data {}", e.raw),
    }
}
