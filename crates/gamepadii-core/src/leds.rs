//! `gamepadii leds`: the per-button RGB LEDs (AnimationStation).
//!
//! Three ways to read them, best first:
//!
//! 1. The control channel: LED_STATUS (v1.1, capability `leds`) on the
//!    v0.7.12 line, LIGHTS_STATUS (v1.2, capability `lights`) on main.
//!    What runs now, with no reboot.
//! 2. Web-config on a fork build ([`ForkFixes`]): `animationOptions` from
//!    `getConfig` (saved, not RAM), with `getLedOptions` and
//!    `getCustomTheme`.
//! 3. Web-config on stock firmware: only `getLedOptions` and
//!    `getCustomTheme`. v0.7.12 exposes no `AnimationOptions` there, so
//!    the animation, brightness and colours are unknown.
//!
//!
//! Upstream main replaced that model with LED profiles (issue #21): its
//! web configurator has `getAnimationProtoOptions` (the brightness and the
//! profiles' effects) and `getLightsDataOptions` (the light clusters),
//! and no `getCustomTheme`. On main, route 2 and 3 are one:
//! [`read_webconfig_main`] shows what those say, and leaves v0.7.12's
//! animation, colours and theme `null`.
//!
//! Changing them is on the channel only: SET_LED (issue #19) on the
//! v0.7.12 line, SET_LIGHTS (the lights profile and the brightness) on
//! main.
//!
//! The names are GP2040-CE v0.7.12's: [`EFFECTS`] is `AnimationEffects` in
//! `proto/enums.proto`, [`COLORS`] is `AnimationColor` (`ANIMATION_COLOR_*`)
//! and [`THEMES`] is the order `StaticTheme` adds its themes in
//! (`src/animationstation/effects/statictheme.cpp`).

use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;

use crate::api::{self, Endpoint, FirmwareLine, ForkFixes};
use crate::channel::{Channel, LedStatus, LightsStatus, SetLed, SetLights, proto};
use crate::client::Client;
use crate::commands::{self, Route};
use crate::error::{Error, Result};

/// `AnimationEffects`, by value: `EFFECT_STATIC_COLOR` is `static-color`.
pub const EFFECTS: &[&str] = &[
    "static-color",
    "rainbow",
    "chase",
    "static-theme",
    "custom-theme",
];

/// `AnimationColor`, by value: `ANIMATION_COLOR_LIMEGREEN` is `limegreen`.
pub const COLORS: &[&str] = &[
    "black",
    "white",
    "red",
    "orange",
    "yellow",
    "limegreen",
    "green",
    "seafoam",
    "aqua",
    "skyblue",
    "blue",
    "purple",
    "pink",
    "magenta",
    "indigo",
    "violet",
];

/// The static themes, by `themeIndex`.
pub const THEMES: &[&str] = &[
    "static-rainbow",
    "xbox",
    "xbox-all",
    "super-famicom",
    "super-famicom-all",
    "playstation",
    "playstation-all",
    "neogeo",
    "neogeo-curved",
    "neogeo-modern",
    "six-button-fighter",
    "six-button-fighter-plus",
    "street-fighter-2",
    "tekken",
    "guilty-gear-type-a",
    "guilty-gear-type-b",
    "guilty-gear-type-c",
    "guilty-gear-type-d",
    "guilty-gear-type-e",
    "fightboard",
];

/// The buttons `getCustomTheme` has a colour for, in the web
/// configurator's order.
pub const THEME_BUTTONS: &[&str] = &[
    "Up", "Down", "Left", "Right", "B1", "B2", "B3", "B4", "L1", "R1", "L2", "R2", "S1", "S2",
    "L3", "R3", "A1", "A2",
];

/// Main's `AnimationNonPressedEffects` (`proto/enums.proto` at 8013b41),
/// by value: the idle effect of an LED profile, and its case effect.
pub const MAIN_IDLE_EFFECTS: &[&str] = &[
    "static-color",
    "rainbow-synced",
    "rainbow-rotate",
    "chase-index",
    "chase-sequential",
    "chase-circle-clockwise",
    "chase-circle-anticlockwise",
    "chase-left-to-right",
    "chase-right-to-left",
    "chase-top-to-bottom",
    "chase-bottom-to-top",
    "chase-index-pingpong",
    "chase-sequential-pingpong",
    "chase-circle-pingpong",
    "chase-horizontal-pingpong",
    "chase-vertical-pingpong",
    "chase-random",
    "jigglestatic",
    "jiggletwostatics",
    "rain",
];

/// Main's `AnimationPressedEffects`, by value: what a pressed key shows.
pub const MAIN_PRESSED_EFFECTS: &[&str] = &[
    "static-color",
    "random",
    "jigglestatic",
    "jiggletwostatics",
    "burst",
    "burst-small",
];

/// What an index names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Animation,
    Color,
    Theme,
}

impl Kind {
    fn table(self) -> &'static [&'static str] {
        match self {
            Kind::Animation => EFFECTS,
            Kind::Color => COLORS,
            Kind::Theme => THEMES,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Kind::Animation => "animation",
            Kind::Color => "colour",
            Kind::Theme => "theme",
        }
    }
}

/// Names compare ignoring case, `-`, `_` and spaces: `Static_Color`,
/// `static color` and `staticcolor` are `static-color`.
fn normal(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// An animation, colour or theme on the command line: its number or name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Number(u8),
    Name(String),
}

impl FromStr for Pick {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty value".into());
        }
        if s.chars().all(|c| c.is_ascii_digit()) {
            return s
                .parse()
                .map(Pick::Number)
                .map_err(|_| format!("{s}: too big (0 to 255)"));
        }
        Ok(Pick::Name(s.to_string()))
    }
}

/// A lights profile on the command line (main): its number from 0, or
/// `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightsPick {
    Off,
    Number(u8),
}

impl FromStr for LightsPick {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(LightsPick::Off),
            n => n
                .parse()
                .map(LightsPick::Number)
                .map_err(|_| format!("{s}: a lights profile (0, 1, ...) or off")),
        }
    }
}

/// A brightness on the command line: a level, or one step up or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    To(u8),
    Up,
    Down,
}

impl FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" | "+" => Ok(Level::Up),
            "down" | "-" => Ok(Level::Down),
            n => n
                .parse()
                .map(Level::To)
                .map_err(|_| format!("{s}: a brightness level (0, 1, ...), up or down")),
        }
    }
}

/// The name of index `n`: the host's table, else what the firmware called
/// it (`extra`, effects past the table on a newer build).
fn name_of(kind: Kind, n: u8, extra: &[(u8, String)]) -> Option<String> {
    kind.table()
        .get(usize::from(n))
        .map(|s| (*s).to_string())
        .or_else(|| extra.iter().find(|(i, _)| *i == n).map(|(_, s)| s.clone()))
}

/// The choices below `count`, for an error: `0 static-color, 1 rainbow`.
fn choices(kind: Kind, count: u8, extra: &[(u8, String)]) -> String {
    (0..count)
        .map(|n| match name_of(kind, n, extra) {
            Some(name) => format!("{n} {name}"),
            None => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The index `pick` means on a pad with `count` of `kind`.
pub fn resolve(kind: Kind, pick: &Pick, count: u8, extra: &[(u8, String)]) -> Result<u8> {
    let found = match pick {
        Pick::Number(n) => Some(*n).filter(|n| *n < count),
        Pick::Name(name) => {
            let want = normal(name);
            (0..count).find(|n| name_of(kind, *n, extra).is_some_and(|s| normal(&s) == want))
        }
    };
    found.ok_or_else(|| {
        let asked = match pick {
            Pick::Number(n) => n.to_string(),
            Pick::Name(s) => format!("{s:?}"),
        };
        Error::Usage(format!(
            "no {} {asked} on this pad; it has {}",
            kind.what(),
            choices(kind, count, extra)
        ))
    })
}

/// An index and its name (`null` when neither gamepadii nor the firmware
/// has one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Named {
    pub index: u8,
    pub name: Option<String>,
}

impl Named {
    fn new(kind: Kind, index: u8, extra: &[(u8, String)]) -> Self {
        Named {
            index,
            name: name_of(kind, index, extra),
        }
    }

    /// `chase (2)`, or `7`.
    pub fn display(&self) -> String {
        match &self.name {
            Some(n) => format!("{n} ({})", self.index),
            None => self.index.to_string(),
        }
    }
}

/// The LEDs' settings; `null` for what the route can't tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct LedState {
    /// The LEDs are set up and AnimationStation runs.
    pub running: Option<bool>,
    /// `baseAnimationIndex`.
    pub animation: Option<Named>,
    /// 0 to `brightness_steps`.
    pub brightness: Option<u8>,
    pub brightness_steps: Option<u8>,
    /// The brightness at the top step, 0-255.
    pub brightness_maximum: Option<u8>,
    pub static_color: Option<Named>,
    pub button_color: Option<Named>,
    pub theme: Option<Named>,
    /// `hasCustomTheme`: the custom theme is on (it is shown when the
    /// animation is `custom-theme`).
    pub custom_theme: Option<bool>,
    /// The running settings differ from the saved ones (the channel says;
    /// `null` elsewhere).
    pub unsaved: Option<bool>,
    /// Main: the lights profile is off (-1). `null` on v0.7.12.
    pub lights_off: Option<bool>,
    /// Main: the lights profile, from 0; `null` when off, and on v0.7.12.
    pub lights_profile: Option<u8>,
}

impl LedState {
    /// What LED_STATUS says.
    pub fn from_status(l: &LedStatus, extra: &[(u8, String)]) -> Self {
        LedState {
            running: Some(l.running()),
            animation: Some(Named::new(Kind::Animation, l.animation, extra)),
            brightness: Some(l.brightness),
            brightness_steps: Some(l.brightness_steps),
            brightness_maximum: Some(l.brightness_maximum),
            static_color: Some(Named::new(Kind::Color, l.static_color, &[])),
            button_color: Some(Named::new(Kind::Color, l.button_color, &[])),
            theme: Some(Named::new(Kind::Theme, l.theme, &[])),
            custom_theme: Some(l.custom_theme()),
            unsaved: Some(l.unsaved()),
            lights_off: None,
            lights_profile: None,
        }
    }

    /// What LIGHTS_STATUS says. Main has no animation, colours or theme
    /// of the v0.7.12 kind: those stay `null`.
    pub fn from_lights(l: &LightsStatus) -> Self {
        LedState {
            running: Some(l.running()),
            brightness: Some(l.brightness),
            brightness_steps: Some(l.brightness_steps),
            brightness_maximum: Some(l.brightness_maximum),
            unsaved: Some(l.unsaved()),
            lights_off: Some(l.on().is_none()),
            lights_profile: l.on(),
            ..LedState::default()
        }
    }
}

/// A button's custom theme colours, `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ButtonColor {
    pub button: String,
    pub normal: Option<String>,
    pub pressed: Option<String>,
}

/// `gamepadii leds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedReport {
    /// `control` (LED_STATUS) or `web_config`.
    pub via: Route,
    /// The device node, or the web configurator.
    pub host: String,
    #[serde(flatten)]
    pub state: LedState,
    /// From `getCustomTheme`, in web-config; `null` over the channel.
    pub custom_theme_colors: Option<Vec<ButtonColor>>,
    /// Main firmware's LED profiles, from web-config; `null` on v0.7.12
    /// and over the channel.
    pub lights: Option<Lights>,
    pub note: Option<String>,
}

/// One of main's LED profiles (`AnimationOptions.profiles`), as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LightsProfile {
    /// From 0, as the firmware counts (`baseProfileIndex`).
    pub index: u8,
    /// `bEnabled`: in the profile cycle.
    pub enabled: Option<bool>,
    /// `baseNonPressedEffect` ([`MAIN_IDLE_EFFECTS`]).
    pub idle_effect: Option<Named>,
    /// `basePressedEffect` ([`MAIN_PRESSED_EFFECTS`]).
    pub pressed_effect: Option<Named>,
    /// `baseCaseEffect` ([`MAIN_IDLE_EFFECTS`]).
    pub case_effect: Option<Named>,
    /// The cycle times, 0–9 as the firmware counts (web-config shows 1–10).
    pub idle_cycle_time: Option<u8>,
    pub pressed_cycle_time: Option<u8>,
    pub case_cycle_time: Option<u8>,
}

/// What main firmware's web configurator says about its lights:
/// `getAnimationProtoOptions` and `getLightsDataOptions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lights {
    /// `baseProfileIndex`: the lights profile, the same as
    /// [`LedState::lights_profile`]. `null` when off.
    pub profile: Option<u8>,
    pub profiles: Vec<LightsProfile>,
    /// `idletimeout`: seconds without input before the LEDs go dark (0:
    /// never).
    pub idle_timeout_s: Option<u64>,
    /// How many light clusters `getLightsDataOptions` lists (buttons, case
    /// and player LEDs); `null` when it couldn't be read.
    pub light_count: Option<usize>,
}

/// The firmware's names for effects past [`EFFECTS`] (STRING `0x20 + n`).
pub fn extra_effects(ch: &Channel, count: u8) -> Vec<(u8, String)> {
    (u8::try_from(EFFECTS.len()).unwrap_or(u8::MAX)..count)
        .filter_map(|n| {
            let name = ch.string(proto::string::effect_name(n)).ok().flatten()?;
            Some((n, name))
        })
        .collect()
}

/// [`LedReport`] over the channel: LIGHTS_STATUS on main, else
/// LED_STATUS.
pub fn over_channel(ch: &Channel) -> Result<LedReport> {
    if ch.caps().has(proto::Caps::LIGHTS) {
        let l = ch.lights_status()?;
        return Ok(LedReport {
            via: Route::Control,
            host: ch.node(),
            state: LedState::from_lights(&l),
            custom_theme_colors: None,
            lights: Some(Lights::from_status(&l)),
            note: l.preview().then(|| PREVIEW.to_string()),
        });
    }
    let l = ch.led_status()?;
    let extra = extra_effects(ch, l.animations);
    Ok(LedReport {
        via: Route::Control,
        host: ch.node(),
        state: LedState::from_status(&l, &extra),
        custom_theme_colors: None,
        lights: None,
        note: None,
    })
}

/// What to say while web-config shows a preview of the lights.
pub const PREVIEW: &str = "web-config shows a preview of the lights now; the pad refuses a \
     change until the preview stops";

impl Lights {
    /// What LIGHTS_STATUS says: every profile, enabled or not, and the
    /// effects of the one that runs. It has no light count.
    pub fn from_status(l: &LightsStatus) -> Self {
        let value = |v: u8| (v != LightsStatus::NONE).then_some(v);
        let named = |table, v| value(v).map(|index| Named::from_table(table, index));
        Lights {
            profile: l.on(),
            profiles: (0..l.profiles.min(8))
                .map(|index| {
                    // The reply has the effects of the running profile only.
                    let now = l.on() == Some(index);
                    let effect = |table, v| named(table, v).filter(|_| now);
                    let time = |v| value(v).filter(|_| now);
                    LightsProfile {
                        index,
                        enabled: Some(l.profile_enabled(index)),
                        idle_effect: effect(MAIN_IDLE_EFFECTS, l.idle_effect),
                        pressed_effect: effect(MAIN_PRESSED_EFFECTS, l.pressed_effect),
                        case_effect: effect(MAIN_IDLE_EFFECTS, l.case_effect),
                        idle_cycle_time: time(l.idle_cycle_time),
                        pressed_cycle_time: time(l.pressed_cycle_time),
                        case_cycle_time: time(l.case_cycle_time),
                    }
                })
                .collect(),
            idle_timeout_s: Some(u64::from(l.idle_timeout_s)),
            light_count: None,
        }
    }
}

/// What to say where the animation and brightness can't be read.
pub const STOCK_UNKNOWN: &str = "the running animation, brightness and colours are unknown on \
     stock firmware: its web configurator doesn't expose them (the gamepadii firmware's \
     control channel does)";

fn byte(v: &Value, key: &str) -> Option<u8> {
    let n = v.get(key)?.as_i64()?;
    u8::try_from(n.clamp(0, 255)).ok()
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|n| n != 0),
        _ => None,
    }
}

fn hex(v: Option<&Value>) -> Option<String> {
    let n = v?.as_u64()?;
    Some(format!("#{:06x}", n & 0xff_ffff))
}

/// What `leds` says on main firmware, read from web-config.
pub const MAIN_NOTE: &str = "main firmware has lights profiles in place of v0.7.12's \
     animation, colours and themes; shown as saved";

/// [`LedReport`] from the web configurator, by firmware line. On v0.7.12:
/// `getLedOptions` and `getCustomTheme`, and on a fork build `getConfig`'s
/// `animationOptions` (what is saved; a RAM-only change from the channel
/// isn't there). On main: [`read_webconfig_main`].
pub fn read_webconfig(client: &Client) -> Result<LedReport> {
    if client.line()?.line == FirmwareLine::Main {
        return read_webconfig_main(client);
    }
    let t = client.transport();
    let options = api::try_read_raw(t, Endpoint::GetLedOptions)?;
    let theme = api::try_read_raw(t, Endpoint::GetCustomTheme)?;
    let mut state = LedState::default();
    if let Some(o) = &options {
        state.running = o.get("dataPin").and_then(Value::as_i64).map(|p| p >= 0);
        state.brightness_steps = byte(o, "brightnessSteps");
        state.brightness_maximum = byte(o, "brightnessMaximum");
    }
    let colors = theme.as_ref().map(|t| {
        state.custom_theme = flag(t, "enabled");
        THEME_BUTTONS
            .iter()
            .map(|b| ButtonColor {
                button: (*b).to_string(),
                normal: hex(t.get(*b).and_then(|c| c.get("u"))),
                pressed: hex(t.get(*b).and_then(|c| c.get("d"))),
            })
            .collect()
    });
    let note = match client.fork_fixes() {
        Some(cap) => fill_from_config(client, &cap, &mut state).err(),
        None => Some(STOCK_UNKNOWN.to_string()),
    };
    Ok(LedReport {
        via: Route::WebConfig,
        host: client.host(),
        state,
        custom_theme_colors: colors,
        lights: None,
        note,
    })
}

impl Named {
    fn from_table(table: &[&str], index: u8) -> Self {
        Named {
            index,
            name: table.get(usize::from(index)).map(|s| (*s).to_string()),
        }
    }
}

/// A name from `table`, by value.
fn table_name(table: &[&str], v: &Value, key: &str) -> Option<Named> {
    byte(v, key).map(|index| Named::from_table(table, index))
}

/// A cycle time from web-config (1–10) as the firmware counts it (0–9).
fn cycle_time(v: &Value, key: &str) -> Option<u8> {
    byte(v, key).map(|n| n.saturating_sub(1))
}

/// [`LedReport`] from main firmware's web configurator, as saved:
/// `getLedOptions` (set up or not, the maximum brightness, which main
/// prints as a percentage), `getAnimationProtoOptions` (the brightness
/// level and the LED profiles) and `getLightsDataOptions` (the light
/// clusters). Main has no animation, static colour, theme or custom theme
/// as v0.7.12 has them: those stay `null`.
pub fn read_webconfig_main(client: &Client) -> Result<LedReport> {
    let t = client.transport();
    let options = api::try_read_raw(t, Endpoint::GetLedOptions)?;
    let animation = api::try_read_raw(t, Endpoint::GetAnimationProtoOptions)?;
    let lights_data = api::try_read_raw(t, Endpoint::GetLightsDataOptions)?;
    let mut state = LedState::default();
    if let Some(o) = &options {
        state.running = o.get("dataPin").and_then(Value::as_i64).map(|p| p >= 0);
        // Back to 0-255 as the setter does it, in f32: int(percent * 2.55f + 0.5f).
        state.brightness_maximum = o
            .get("brightnessMaximum")
            .and_then(Value::as_f64)
            .map(|p| (p.clamp(0.0, 100.0) as f32 * 2.55f32 + 0.5) as u8);
    }
    let a = animation.as_ref().and_then(|a| a.get("AnimationOptions"));
    let light_count = lights_data
        .as_ref()
        .and_then(|d| d.get("LightData")?.get("Lights")?.as_array().map(Vec::len));
    let lights = a.map(|a| {
        state.brightness = byte(a, "brightness");
        let profiles = a
            .get("profiles")
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .zip(0u8..)
                    .map(|(p, index)| LightsProfile {
                        index,
                        enabled: flag(p, "bEnabled"),
                        idle_effect: table_name(MAIN_IDLE_EFFECTS, p, "baseNonPressedEffect"),
                        pressed_effect: table_name(MAIN_PRESSED_EFFECTS, p, "basePressedEffect"),
                        case_effect: table_name(MAIN_IDLE_EFFECTS, p, "baseCaseEffect"),
                        idle_cycle_time: cycle_time(p, "baseCycleTime"),
                        pressed_cycle_time: cycle_time(p, "basePressedCycleTime"),
                        case_cycle_time: cycle_time(p, "baseCaseCycleTime"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // -1 is off: never clamp it to profile 0.
        let profile = a.get("baseProfileIndex").and_then(Value::as_i64);
        state.lights_off = profile.map(|p| p < 0);
        state.lights_profile = profile.and_then(|p| u8::try_from(p).ok());
        Lights {
            profile: state.lights_profile,
            profiles,
            idle_timeout_s: a.get("idletimeout").and_then(Value::as_u64),
            light_count,
        }
    });
    Ok(LedReport {
        via: Route::WebConfig,
        host: client.host(),
        state,
        custom_theme_colors: None,
        lights,
        note: Some(MAIN_NOTE.to_string()),
    })
}

/// `animationOptions` into `state`; what to say when it can't be read.
fn fill_from_config(
    client: &Client,
    cap: &ForkFixes,
    state: &mut LedState,
) -> std::result::Result<(), String> {
    let config = commands::full_config(client, cap)
        .map_err(|e| format!("the animation and brightness couldn't be read: {e}"))?;
    let a = config
        .get("animationOptions")
        .ok_or("getConfig has no animationOptions")?;
    let named = |kind, key| byte(a, key).map(|n| Named::new(kind, n, &[]));
    state.animation = named(Kind::Animation, "baseAnimationIndex");
    state.brightness = byte(a, "brightness");
    state.static_color = named(Kind::Color, "staticColorIndex");
    state.button_color = named(Kind::Color, "buttonColorIndex");
    state.theme = named(Kind::Theme, "themeIndex");
    if let Some(on) = flag(a, "hasCustomTheme") {
        state.custom_theme = Some(on);
    }
    Ok(())
}

/// What `leds set` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LedChange {
    pub animation: Option<Pick>,
    pub brightness: Option<Level>,
    pub static_color: Option<Pick>,
    pub button_color: Option<Pick>,
    pub theme: Option<Pick>,
    /// Main: the lights profile.
    pub lights_profile: Option<LightsPick>,
    /// Save to flash, as the LED hotkeys do.
    pub save: bool,
}

impl LedChange {
    /// Whether it asks for any setting (else it is a bare `--save`).
    pub fn sets_anything(&self) -> bool {
        self.animation.is_some()
            || self.brightness.is_some()
            || self.static_color.is_some()
            || self.button_color.is_some()
            || self.theme.is_some()
            || self.lights_profile.is_some()
    }

    /// The v0.7.12 settings it asks for, by option name.
    fn v0_7_12_options(&self) -> Vec<&'static str> {
        [
            (self.animation.is_some(), "--animation"),
            (self.static_color.is_some(), "--static-color"),
            (self.button_color.is_some(), "--button-color"),
            (self.theme.is_some(), "--theme"),
        ]
        .into_iter()
        .filter_map(|(on, o)| on.then_some(o))
        .collect()
    }
}

/// The brightness `level` asks for, from `now` on a pad with `steps`.
fn brightness_level(level: Option<Level>, now: u8, steps: u8) -> Result<Option<u8>> {
    Ok(match level {
        None => None,
        Some(Level::To(n)) if n <= steps => Some(n),
        Some(Level::To(n)) => {
            return Err(Error::Usage(format!(
                "brightness {n}: this pad has levels 0 to {steps}"
            )));
        }
        Some(Level::Up) => Some(now.saturating_add(1).min(steps)),
        Some(Level::Down) => Some(now.saturating_sub(1)),
    })
}

/// The SET_LED for `change` on a pad showing `before`, and the settings it
/// changes (`animation`, `brightness`, ...); `Usage` for a value the pad
/// doesn't have.
pub fn plan(
    change: &LedChange,
    before: &LedStatus,
    extra: &[(u8, String)],
) -> Result<(SetLed, Vec<&'static str>)> {
    let pick = |kind, p: &Option<Pick>, count| {
        p.as_ref()
            .map(|p| resolve(kind, p, count, extra))
            .transpose()
    };
    if change.lights_profile.is_some() {
        return Err(Error::Usage(
            "this pad's firmware (the v0.7.12 line) has no lights profiles; use --animation".into(),
        ));
    }
    let brightness = brightness_level(
        change.brightness,
        before.brightness,
        before.brightness_steps,
    )?;
    let set = SetLed {
        animation: pick(Kind::Animation, &change.animation, before.animations)?,
        brightness,
        static_color: pick(Kind::Color, &change.static_color, before.colors)?,
        button_color: pick(Kind::Color, &change.button_color, before.colors)?,
        theme: pick(Kind::Theme, &change.theme, before.themes)?,
        persist: change.save,
    };
    let changed = [
        ("animation", set.animation, before.animation),
        ("brightness", set.brightness, before.brightness),
        ("static_color", set.static_color, before.static_color),
        ("button_color", set.button_color, before.button_color),
        ("theme", set.theme, before.theme),
    ]
    .into_iter()
    .filter(|(_, want, now)| want.is_some_and(|w| w != *now))
    .map(|(name, _, _)| name)
    .collect();
    Ok((set, changed))
}

/// Whether `l` shows what `set` asked for.
pub fn shows(l: &LedStatus, set: &SetLed) -> bool {
    [
        (set.animation, l.animation),
        (set.brightness, l.brightness),
        (set.static_color, l.static_color),
        (set.button_color, l.button_color),
        (set.theme, l.theme),
    ]
    .into_iter()
    .all(|(want, now)| want.is_none_or(|w| w == now))
}

/// The SET_LIGHTS for `change` on a main pad showing `before`, and the
/// settings it changes (`lights_profile`, `brightness`); `Usage` for a
/// v0.7.12 setting or a value the pad doesn't have.
pub fn plan_lights(
    change: &LedChange,
    before: &LightsStatus,
) -> Result<(SetLights, Vec<&'static str>)> {
    let old = change.v0_7_12_options();
    if !old.is_empty() {
        return Err(Error::Usage(format!(
            "{} {}: main firmware has lights profiles in place of v0.7.12's animation, \
             colours and themes; use --lights-profile and --brightness",
            if old.len() == 1 { "option" } else { "options" },
            old.join(", ")
        )));
    }
    let enabled: Vec<u8> = (0..before.profiles)
        .filter(|n| before.profile_enabled(*n))
        .collect();
    let list = || {
        enabled
            .iter()
            .map(u8::to_string)
            .chain(["off".to_string()])
            .collect::<Vec<_>>()
            .join(", ")
    };
    let profile = match change.lights_profile {
        None => None,
        Some(LightsPick::Off) => Some(-1),
        Some(LightsPick::Number(n)) if enabled.contains(&n) => i8::try_from(n).ok(),
        Some(LightsPick::Number(n)) => {
            let why = if n < before.profiles {
                "is disabled"
            } else {
                "is not on this pad"
            };
            return Err(Error::Usage(format!(
                "lights profile {n} {why}; this pad has {}",
                list()
            )));
        }
    };
    let set = SetLights {
        profile,
        brightness: brightness_level(
            change.brightness,
            before.brightness,
            before.brightness_steps,
        )?,
        persist: change.save,
    };
    let mut changed = Vec::new();
    if set.profile.is_some_and(|p| p != before.profile) {
        changed.push("lights_profile");
    }
    if set.brightness.is_some_and(|b| b != before.brightness) {
        changed.push("brightness");
    }
    Ok((set, changed))
}

/// Whether `l` shows what `set` asked for.
pub fn shows_lights(l: &LightsStatus, set: &SetLights) -> bool {
    set.profile.is_none_or(|p| p == l.profile) && set.brightness.is_none_or(|b| b == l.brightness)
}

/// `gamepadii leds set` and `leds brightness`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedSetReport {
    /// Always `control`: SET_LED, or SET_LIGHTS on main.
    pub via: Route,
    pub host: String,
    pub before: LedState,
    pub leds: LedState,
    /// The settings that changed: `animation`, `brightness`,
    /// `static_color`, `button_color`, `theme`, and on main
    /// `lights_profile`.
    pub changed: Vec<&'static str>,
    /// Whether this saved the LED settings to flash (`--save`).
    pub saved: bool,
    /// Whether LED_STATUS shows what was asked for.
    pub verified: bool,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s16() -> LedStatus {
        LedStatus {
            flags: LedStatus::RUNNING | LedStatus::CUSTOM_THEME,
            animation: 4,
            animations: 5,
            brightness: 1,
            brightness_steps: 5,
            brightness_maximum: 100,
            static_color: 2,
            button_color: 1,
            theme: 0,
            colors: 16,
            themes: 20,
        }
    }

    #[test]
    fn the_tables_match_v0_7_12() {
        assert_eq!(EFFECTS.len(), 5);
        assert_eq!(COLORS.len(), 16);
        assert_eq!(COLORS[15], "violet");
        assert_eq!(THEMES.len(), 20);
        assert_eq!(THEMES[19], "fightboard");
        assert_eq!(THEME_BUTTONS.len(), 18);
    }

    #[test]
    fn names_and_numbers_resolve() {
        let r = |kind, s: &str, count| resolve(kind, &s.parse().unwrap(), count, &[]);
        assert_eq!(r(Kind::Animation, "rainbow", 5).unwrap(), 1);
        assert_eq!(r(Kind::Animation, "Static_Color", 5).unwrap(), 0);
        assert_eq!(r(Kind::Animation, "custom theme", 5).unwrap(), 4);
        assert_eq!(r(Kind::Animation, "3", 5).unwrap(), 3);
        assert_eq!(r(Kind::Color, "lime-green", 16).unwrap(), 5);
        assert_eq!(r(Kind::Color, "SkyBlue", 16).unwrap(), 9);
        assert_eq!(r(Kind::Theme, "street-fighter-2", 20).unwrap(), 12);
        let e = r(Kind::Animation, "5", 5).unwrap_err();
        assert_eq!(e.exit_code(), crate::error::exit::USAGE);
        assert!(
            e.to_string()
                .contains("it has 0 static-color, 1 rainbow, 2 chase"),
            "{e}"
        );
        // A name past what the build has.
        assert!(r(Kind::Animation, "custom-theme", 4).is_err());
        let e = r(Kind::Color, "mauve", 16).unwrap_err();
        assert!(e.to_string().contains("no colour \"mauve\""), "{e}");
        // An effect only the firmware names.
        let extra = [(5, "Sparkle".to_string())];
        assert_eq!(
            resolve(Kind::Animation, &Pick::Name("sparkle".into()), 6, &extra).unwrap(),
            5
        );
        assert!("".parse::<Pick>().is_err());
        assert!("300".parse::<Pick>().is_err());
    }

    fn main_s16() -> LightsStatus {
        LightsStatus {
            flags: LightsStatus::RUNNING,
            profile: 0,
            enabled_profiles: 0b1011,
            profiles: 4,
            brightness: 5,
            brightness_steps: 10,
            brightness_maximum: 128,
            idle_effect: 1,
            pressed_effect: 0,
            case_effect: 1,
            idle_cycle_time: 0,
            pressed_cycle_time: 0,
            case_cycle_time: 0,
            idle_timeout_s: 0,
        }
    }

    #[test]
    fn a_lights_plan_takes_enabled_profiles_and_off() {
        let ask = |p, b| LedChange {
            lights_profile: p,
            brightness: b,
            ..LedChange::default()
        };
        let (set, changed) = plan_lights(
            &ask(Some(LightsPick::Number(3)), Some(Level::Up)),
            &main_s16(),
        )
        .unwrap();
        assert_eq!((set.profile, set.brightness), (Some(3), Some(6)));
        assert_eq!(changed, ["lights_profile", "brightness"]);
        let (set, _) = plan_lights(&ask(Some(LightsPick::Off), None), &main_s16()).unwrap();
        assert_eq!(set.encode(), [0x01, 0xff, 0, 0]);
        // Profile 2 is disabled, and 4 doesn't exist.
        let e = plan_lights(&ask(Some(LightsPick::Number(2)), None), &main_s16()).unwrap_err();
        assert!(
            e.to_string()
                .contains("lights profile 2 is disabled; this pad has 0, 1, 3, off"),
            "{e}"
        );
        let e = plan_lights(&ask(Some(LightsPick::Number(4)), None), &main_s16()).unwrap_err();
        assert!(e.to_string().contains("is not on this pad"), "{e}");
        let e = plan_lights(&ask(None, Some(Level::To(11))), &main_s16()).unwrap_err();
        assert!(e.to_string().contains("levels 0 to 10"), "{e}");
        // What runs already changes nothing.
        let (set, changed) = plan_lights(
            &ask(Some(LightsPick::Number(0)), Some(Level::To(5))),
            &main_s16(),
        )
        .unwrap();
        assert!(changed.is_empty() && shows_lights(&main_s16(), &set));
        let v0712 = LedChange {
            animation: Some(Pick::Name("chase".into())),
            theme: Some(Pick::Number(1)),
            ..LedChange::default()
        };
        let e = plan_lights(&v0712, &main_s16()).unwrap_err();
        assert!(
            e.to_string()
                .contains("options --animation, --theme: main firmware"),
            "{e}"
        );
        let e = plan(&ask(Some(LightsPick::Off), None), &s16(), &[]).unwrap_err();
        assert!(e.to_string().contains("no lights profiles"), "{e}");
        assert_eq!("OFF".parse(), Ok(LightsPick::Off));
        assert_eq!("2".parse(), Ok(LightsPick::Number(2)));
        assert!("dim".parse::<LightsPick>().is_err());
    }

    #[test]
    fn lights_status_shows_the_running_profile_only() {
        let l = Lights::from_status(&main_s16());
        assert_eq!(l.profile, Some(0));
        assert_eq!(l.profiles.len(), 4);
        let now = &l.profiles[0];
        assert_eq!(
            now.idle_effect.as_ref().unwrap().name.as_deref(),
            Some("rainbow-synced")
        );
        assert_eq!(now.idle_cycle_time, Some(0));
        assert_eq!(l.profiles[1].idle_effect, None);
        assert_eq!(l.profiles[2].enabled, Some(false));
        let off = LightsStatus {
            profile: -1,
            idle_effect: LightsStatus::NONE,
            ..main_s16()
        };
        let state = LedState::from_lights(&off);
        assert_eq!((state.lights_off, state.lights_profile), (Some(true), None));
        assert!(
            Lights::from_status(&off)
                .profiles
                .iter()
                .all(|p| p.idle_effect.is_none())
        );
    }

    #[test]
    fn levels_parse() {
        assert_eq!("up".parse(), Ok(Level::Up));
        assert_eq!("DOWN".parse(), Ok(Level::Down));
        assert_eq!("3".parse(), Ok(Level::To(3)));
        assert!("bright".parse::<Level>().is_err());
    }

    #[test]
    fn a_plan_masks_only_what_is_asked() {
        let change = LedChange {
            animation: Some(Pick::Name("chase".into())),
            brightness: Some(Level::Up),
            ..LedChange::default()
        };
        let (set, changed) = plan(&change, &s16(), &[]).unwrap();
        assert_eq!(set.mask(), SetLed::ANIMATION | SetLed::BRIGHTNESS);
        assert_eq!((set.animation, set.brightness), (Some(2), Some(2)));
        assert_eq!(changed, ["animation", "brightness"]);
        // Asking for what it has already changes nothing.
        let same = LedChange {
            static_color: Some(Pick::Name("red".into())),
            ..LedChange::default()
        };
        assert!(plan(&same, &s16(), &[]).unwrap().1.is_empty());
        // Up at the top and down at the bottom stay put.
        let top = LedStatus {
            brightness: 5,
            ..s16()
        };
        let up = LedChange {
            brightness: Some(Level::Up),
            ..LedChange::default()
        };
        assert_eq!(plan(&up, &top, &[]).unwrap().0.brightness, Some(5));
        let bottom = LedStatus {
            brightness: 0,
            ..s16()
        };
        let down = LedChange {
            brightness: Some(Level::Down),
            ..LedChange::default()
        };
        assert_eq!(plan(&down, &bottom, &[]).unwrap().0.brightness, Some(0));
        let too_bright = LedChange {
            brightness: Some(Level::To(6)),
            ..LedChange::default()
        };
        let e = plan(&too_bright, &s16(), &[]).unwrap_err();
        assert!(e.to_string().contains("levels 0 to 5"), "{e}");
        let (set, _) = plan(&LedChange::default(), &s16(), &[]).unwrap();
        assert!(set.is_empty());
        assert!(shows(&s16(), &set));
    }
}
