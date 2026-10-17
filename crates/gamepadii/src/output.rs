//! Rendering of command results: text for people, the versioned JSON
//! contract (`"schema": 1`, snake_case keys, explicit `null`s), or one line
//! of waybar custom-module JSON for `status`.

use std::io::IsTerminal;

use gamepadii_core::Error;
use gamepadii_core::commands::Status;
use gamepadii_core::discovery::Device;
use gamepadii_core::mode::{Confidence, InputMode};
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
