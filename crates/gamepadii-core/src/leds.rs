//! `gamepadii leds`: the per-button RGB LEDs (AnimationStation).
//!
//! Three ways to read them, best first:
//!
//! 1. The control channel's LED_STATUS (v1.1, capability `leds`): the
//!    animation, brightness and colours running now, with no reboot.
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
//! Changing them is SET_LED, on the channel only (issue #19).
//!
//! The names are GP2040-CE v0.7.12's: [`EFFECTS`] is `AnimationEffects` in
//! `proto/enums.proto`, [`COLORS`] is `AnimationColor` (`ANIMATION_COLOR_*`)
//! and [`THEMES`] is the order `StaticTheme` adds its themes in
//! (`src/animationstation/effects/statictheme.cpp`).

use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;

use crate::api::{self, Endpoint, FirmwareLine, ForkFixes};
use crate::channel::{Channel, LedStatus, SetLed, proto};
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
}

/// What main firmware's web configurator says about its lights:
/// `getAnimationProtoOptions` and `getLightsDataOptions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lights {
    /// `baseProfileIndex`: the LED profile the pad starts with.
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

/// [`LedReport`] over the channel: LED_STATUS.
pub fn over_channel(ch: &Channel) -> Result<LedReport> {
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
pub const MAIN_NOTE: &str = "main firmware has LED profiles in place of v0.7.12's animation, \
     colours and themes; shown as saved (the control channel's LED commands speak v0.7.12's \
     model and aren't on main yet)";

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

/// A name from `table`, by value.
fn table_name(table: &[&str], v: &Value, key: &str) -> Option<Named> {
    let index = byte(v, key)?;
    Some(Named {
        index,
        name: table.get(usize::from(index)).map(|s| (*s).to_string()),
    })
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
                    })
                    .collect()
            })
            .unwrap_or_default();
        Lights {
            profile: byte(a, "baseProfileIndex"),
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
    }
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
    let steps = before.brightness_steps;
    let brightness = match change.brightness {
        None => None,
        Some(Level::To(n)) if n <= steps => Some(n),
        Some(Level::To(n)) => {
            return Err(Error::Usage(format!(
                "brightness {n}: this pad has levels 0 to {steps}"
            )));
        }
        Some(Level::Up) => Some(before.brightness.saturating_add(1).min(steps)),
        Some(Level::Down) => Some(before.brightness.saturating_sub(1)),
    };
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

/// `gamepadii leds set` and `leds brightness`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedSetReport {
    /// Always `control`: SET_LED.
    pub via: Route,
    pub host: String,
    pub before: LedState,
    pub leds: LedState,
    /// The settings that changed: `animation`, `brightness`,
    /// `static_color`, `button_color`, `theme`.
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
