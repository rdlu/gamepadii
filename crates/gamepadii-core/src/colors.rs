//! The custom colours of main over the control channel (v1.9, §3.15 of
//! `docs/design/control-channel.md`): READ_CUSTOM_COLORS and
//! WRITE_CUSTOM_COLORS, and the theme file.
//!
//! The pad keeps 16 custom colours, one RGB value each. A lights profile
//! gives custom colour n as colour number 14 + n (0–13 are the built-in
//! colours). The custom colours are global: the 4 lights profiles share
//! them. `count` tells how many are set; a write makes the colours after
//! them black.
//!
//! `gamepadii leds colors` reads them live. Its JSON is also a colours
//! file: `gamepadii leds colors write FILE` writes it, in RAM, and saves it
//! on request. gamepadii backs up the colours first, in the same format.
//!
//! A theme file holds custom colours and the fields of a lights profile
//! together (`colors` and `lights`, both optional). `gamepadii leds theme
//! apply FILE` writes the colours, then the profile, and makes the profile
//! the one that runs. `gamepadii leds theme show` makes a theme file from
//! the pad.

use std::collections::BTreeSet;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value as Json;

use crate::channel::proto::{CUSTOM_COLORS_MAX, CustomColors, Rgb};
use crate::commands::Route;
use crate::error::{Error, Result};
use crate::lights::{BASE_COLORS, FIELDS, Kind, LightsFile};
use crate::pb::Value;

/// The colour number of custom colour `n` in a lights profile.
pub fn number(n: usize) -> usize {
    BASE_COLORS + n
}

/// The keys of a colours file. `via`, `host`, `count`, `max` and `all` are
/// in the JSON of `leds colors`; a write ignores them.
const COLORS_KEYS: &[&str] = &[
    "schema", "via", "host", "board", "count", "max", "colors", "all",
];

/// The keys of a theme file. `via`, `host` and `active` are in the JSON of
/// `leds theme show`; a write ignores them.
const THEME_KEYS: &[&str] = &[
    "schema", "via", "host", "board", "active", "colors", "lights",
];

/// A list of `#rrggbb` colours, at most 16.
fn colors_from(v: &Json) -> std::result::Result<Vec<Rgb>, String> {
    let list = v.as_array().ok_or("not a list")?;
    if list.len() > CUSTOM_COLORS_MAX {
        return Err(format!(
            "{} colours; the pad keeps at most {CUSTOM_COLORS_MAX}",
            list.len()
        ));
    }
    list.iter()
        .map(|c| {
            c.as_str()
                .and_then(Rgb::parse_hex)
                .ok_or_else(|| format!("{c} is not a colour (#rrggbb)"))
        })
        .collect()
}

/// The text of `board`, or `None` for `null` or no key.
fn board_from(
    obj: &serde_json::Map<String, Json>,
    bad: &dyn Fn(String) -> Error,
) -> Result<Option<String>> {
    match obj.get("board").filter(|v| !v.is_null()) {
        Some(v) => Ok(Some(
            v.as_str()
                .ok_or_else(|| bad("board: not text".into()))?
                .into(),
        )),
        None => Ok(None),
    }
}

/// The JSON object of `text`, with only the keys in `known`.
fn object(
    text: &str,
    known: &[&str],
    bad: &dyn Fn(String) -> Error,
) -> Result<serde_json::Map<String, Json>> {
    let json: Json = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
    let obj = json
        .as_object()
        .cloned()
        .ok_or_else(|| bad("not a JSON object".into()))?;
    if let Some(key) = obj.keys().find(|k| !known.contains(&k.as_str())) {
        return Err(bad(format!("no key {key}")));
    }
    Ok(obj)
}

/// The custom colours, as a colours file holds them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColorsFile {
    /// `GP2040_BOARDCONFIG`: a write refuses another board.
    pub board: Option<String>,
    /// The colours to set, at most 16: custom colour 0 first.
    pub colors: Vec<Rgb>,
}

impl ColorsFile {
    pub fn parse(text: &str) -> Result<ColorsFile> {
        let bad = |e: String| Error::Usage(format!("not a colours file: {e}"));
        let obj = object(text, COLORS_KEYS, &bad)?;
        let colors = obj
            .get("colors")
            .ok_or_else(|| bad("no key colors".into()))?;
        Ok(ColorsFile {
            board: board_from(&obj, &bad)?,
            colors: colors_from(colors).map_err(|e| bad(format!("colors: {e}")))?,
        })
    }
}

/// A colours file: `schema` 1, `board`, `colors`.
impl Serialize for ColorsFile {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(3))?;
        m.serialize_entry("schema", &1)?;
        m.serialize_entry("board", &self.board)?;
        m.serialize_entry("colors", &self.colors)?;
        m.end()
    }
}

/// One of the 16 custom colours, in `leds colors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ColorEntry {
    /// 0–15.
    pub custom: u8,
    /// Its colour number in a lights profile: 14–29.
    pub number: u8,
    pub color: Rgb,
    /// It is below the count. The firmware still uses a colour that is not
    /// set: a write makes it black.
    pub set: bool,
}

/// `gamepadii leds colors`: the custom colours, read live with
/// READ_CUSTOM_COLORS. Its JSON is a colours file for `leds colors write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorsReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    pub board: Option<String>,
    pub colors: CustomColors,
}

impl ColorsReport {
    /// All 16, also the ones after the count.
    pub fn entries(&self) -> Vec<ColorEntry> {
        self.colors
            .colors
            .iter()
            .zip(0u8..)
            .map(|(c, n)| ColorEntry {
                custom: n,
                number: number(n.into()) as u8,
                color: *c,
                set: n < self.colors.count,
            })
            .collect()
    }

    /// The colours file of the report.
    pub fn file(&self) -> ColorsFile {
        ColorsFile {
            board: self.board.clone(),
            colors: self.colors.set().to_vec(),
        }
    }
}

impl Serialize for ColorsReport {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // The front end adds `schema`, as for every report.
        let mut m = s.serialize_map(Some(7))?;
        m.serialize_entry("via", &self.via)?;
        m.serialize_entry("host", &self.host)?;
        m.serialize_entry("board", &self.board)?;
        m.serialize_entry("count", &self.colors.count)?;
        m.serialize_entry("max", &self.colors.max)?;
        m.serialize_entry("colors", self.colors.set())?;
        m.serialize_entry("all", &self.entries())?;
        m.end()
    }
}

/// What `leds colors write` asks for, besides the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorsWrite {
    /// Save to flash.
    pub save: bool,
}

/// `gamepadii leds colors write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColorsWriteReport {
    /// Always `control`: WRITE_CUSTOM_COLORS.
    pub via: Route,
    pub host: String,
    /// How many were set before.
    pub before_count: u8,
    /// How many are set now.
    pub count: u8,
    /// The custom colours (0–15) whose value changed.
    pub changed: Vec<u8>,
    /// Saved to flash (`--save`).
    pub saved: bool,
    /// LIGHTS_STATUS says the LEDs apply them (PENDING is clear).
    pub applied: bool,
    /// READ_CUSTOM_COLORS gives what was written.
    pub verified: bool,
    /// The colours as they were, in the colours file format; `null` with
    /// `--no-backup`.
    pub backup: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

/// The custom colours (0–15) that differ between `a` and `b`.
pub fn changed(a: &[Rgb], b: &[Rgb]) -> Vec<u8> {
    (0..a.len().max(b.len()))
        .filter(|i| a.get(*i) != b.get(*i))
        .map(|i| i as u8)
        .collect()
}

/// The 16 colours after a write of `colors`: the others are black.
pub fn after_write(colors: &[Rgb]) -> Vec<Rgb> {
    let mut all = colors.to_vec();
    all.resize(CUSTOM_COLORS_MAX, Rgb::BLACK);
    all
}

/// The colour numbers in the colour lists of `file` that show black with
/// `count` custom colours set: 14 + `count` and more, from the lowest.
pub fn unset_numbers(file: &LightsFile, count: usize) -> Vec<u8> {
    let first = number(count);
    let numbers: BTreeSet<u8> = FIELDS
        .iter()
        .filter(|f| matches!(f.kind, Kind::Colors(_)))
        .filter_map(|f| match file.values.get(&f.number) {
            Some(Value::Bytes(b)) => Some(b),
            _ => None,
        })
        .flatten()
        .copied()
        .filter(|c| usize::from(*c) >= first)
        .collect();
    numbers.into_iter().collect()
}

/// The note for the colour numbers `unset` of lights profile `profile`.
pub fn unset_note(profile: u8, unset: &[u8], count: usize) -> Option<String> {
    if unset.is_empty() {
        return None;
    }
    let list = unset
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let set = match count {
        0 => "no custom colours are set".to_string(),
        1 => "only custom colour 0 (number 14) is set".to_string(),
        2 => "only custom colours 0 and 1 (numbers 14 and 15) are set".to_string(),
        n => format!(
            "only custom colours 0 to {} (numbers 14 to {}) are set",
            n - 1,
            number(n - 1)
        ),
    };
    Some(format!(
        "lights profile {profile} uses colour number {list}, which shows black: {set}"
    ))
}

/// What to say about the global custom colours.
pub const SHARED: &str = "the custom colours are global: the other lights profiles that use \
     colours 14 to 29 change too";

/// Custom colours and the fields of a lights profile, as a theme file holds
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeFile {
    /// `GP2040_BOARDCONFIG`: an apply refuses another board.
    pub board: Option<String>,
    /// The custom colours to set; `None`: they do not change.
    pub colors: Option<Vec<Rgb>>,
    /// The fields of the lights profile to write (a lights file); `None`:
    /// no field changes, but the profile is still selected.
    pub lights: Option<LightsFile>,
}

impl ThemeFile {
    pub fn parse(text: &str) -> Result<ThemeFile> {
        let bad = |e: String| Error::Usage(format!("not a theme file: {e}"));
        let obj = object(text, THEME_KEYS, &bad)?;
        let get = |k: &str| obj.get(k).filter(|v| !v.is_null());
        let colors = match get("colors") {
            Some(v) => Some(colors_from(v).map_err(|e| bad(format!("colors: {e}")))?),
            None => None,
        };
        let lights = match get("lights") {
            Some(v) if v.is_object() => {
                Some(LightsFile::parse(&v.to_string()).map_err(|e| match e {
                    Error::Usage(m) => bad(format!("lights: {m}")),
                    e => e,
                })?)
            }
            Some(_) => return Err(bad("lights: not a JSON object".into())),
            None => None,
        };
        Ok(ThemeFile {
            board: board_from(&obj, &bad)?,
            colors,
            lights,
        })
    }

    fn entries(&self) -> Vec<(&'static str, Json)> {
        vec![
            ("board", self.board.clone().into()),
            (
                "colors",
                serde_json::to_value(&self.colors).unwrap_or(Json::Null),
            ),
            (
                "lights",
                serde_json::to_value(&self.lights).unwrap_or(Json::Null),
            ),
        ]
    }
}

/// A theme file: `schema` 1, `board`, `colors`, `lights`.
impl Serialize for ThemeFile {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let e = self.entries();
        let mut m = s.serialize_map(Some(e.len() + 1))?;
        m.serialize_entry("schema", &1)?;
        for (k, v) in &e {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// `gamepadii leds theme show`: the custom colours and one lights profile,
/// read live. Its JSON is a theme file for `leds theme apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// The lights profile is the one that runs.
    pub active: bool,
    pub theme: ThemeFile,
}

impl Serialize for ThemeReport {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let e = self.theme.entries();
        // The front end adds `schema`, as for every report.
        let mut m = s.serialize_map(Some(e.len() + 3))?;
        m.serialize_entry("via", &self.via)?;
        m.serialize_entry("host", &self.host)?;
        m.serialize_entry("active", &self.active)?;
        for (k, v) in &e {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// What `leds theme apply` asks for, besides the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThemeApply {
    /// The lights profile to write and select, 0–3; `None`: the one that
    /// runs now.
    pub profile: Option<u8>,
    /// Save to flash.
    pub save: bool,
}

/// `gamepadii leds theme apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeApplyReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// The lights profile that the theme went to.
    pub profile: u8,
    /// How many custom colours are set now; `null` when the theme has no
    /// colours.
    pub colors: Option<u8>,
    /// The custom colours (0–15) whose value changed.
    pub colors_changed: Vec<u8>,
    /// The keys of the lights profile whose values changed.
    pub lights_changed: Vec<String>,
    /// The profile is the one that runs now.
    pub active: bool,
    /// Saved to flash (`--save`).
    pub saved: bool,
    /// LIGHTS_STATUS says the LEDs apply each write (PENDING is clear).
    pub applied: bool,
    /// The pad gives back what was written.
    pub verified: bool,
    /// The colours and the profile as they were, in the theme file format;
    /// `null` with `--no-backup`.
    pub backup: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colours_file_round_trips() {
        let f =
            ColorsFile::parse(r##"{"board": "Haute42COSMOX", "colors": ["#FF00ff", "00ffff"]}"##)
                .unwrap();
        assert_eq!(f.colors, [Rgb::from_u32(0xff00ff), Rgb::from_u32(0x00ffff)]);
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["colors"][0], "#ff00ff");
        assert_eq!(json["schema"], 1);
        assert_eq!(ColorsFile::parse(&json.to_string()).unwrap(), f);
        assert!(
            ColorsFile::parse(r#"{"colors": []}"#)
                .unwrap()
                .colors
                .is_empty()
        );
        let seventeen = format!(r#"{{"colors": {:?}}}"#, vec!["#000000"; 17]);
        for (text, says) in [
            (r#"{"board": "Pico"}"#, "no key colors"),
            (r#"{"colors": ["red"]}"#, "#rrggbb"),
            (r##"{"colors": "#ff0000"}"##, "not a list"),
            (r#"{"colours": []}"#, "no key colours"),
            (seventeen.as_str(), "at most 16"),
            ("[]", "JSON object"),
        ] {
            let e = ColorsFile::parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }

    #[test]
    fn a_report_is_a_colours_file() {
        let mut colors = after_write(&[Rgb::from_u32(0x102030)]);
        colors[5] = Rgb::from_u32(0xffffff);
        let r = ColorsReport {
            via: Route::Control,
            host: "/dev/bus/usb/003/007".into(),
            board: Some("Haute42COSMOX".into()),
            colors: CustomColors {
                count: 1,
                max: 16,
                colors,
            },
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["colors"], serde_json::json!(["#102030"]));
        assert_eq!(json["all"].as_array().unwrap().len(), 16);
        assert_eq!(
            json["all"][1],
            serde_json::json!({"custom": 1, "number": 15, "color": "#000000", "set": false})
        );
        assert_eq!(json["all"][5]["color"], "#ffffff", "after the count too");
        assert_eq!(ColorsFile::parse(&json.to_string()).unwrap(), r.file());
    }

    #[test]
    fn unset_colours_show_black() {
        let f = LightsFile::parse(
            r#"{"idle_colors": [2, 14, 15, 17], "pressed_colors": [29, 16], "case_colors": [32]}"#,
        )
        .unwrap();
        assert_eq!(unset_numbers(&f, 2), [16, 17, 29, 32]);
        assert_eq!(unset_numbers(&f, 16), [32]);
        assert_eq!(unset_numbers(&f, 0), [14, 15, 16, 17, 29, 32]);
        let note = unset_note(1, &[16, 17], 2).unwrap();
        assert!(note.contains("colour number 16, 17"), "{note}");
        assert!(
            note.contains("custom colours 0 and 1 (numbers 14 and 15)"),
            "{note}"
        );
        let note = unset_note(1, &[17], 3).unwrap();
        assert!(
            note.contains("custom colours 0 to 2 (numbers 14 to 16)"),
            "{note}"
        );
        assert!(
            unset_note(1, &[14], 0)
                .unwrap()
                .contains("no custom colours")
        );
        assert_eq!(unset_note(1, &[], 0), None);
        assert_eq!(
            changed(&after_write(&[]), &after_write(&[Rgb::BLACK])),
            Vec::<u8>::new()
        );
        assert_eq!(
            changed(
                &[Rgb::BLACK, Rgb::from_u32(1)],
                &[Rgb::BLACK, Rgb::from_u32(2)]
            ),
            [1]
        );
    }

    #[test]
    fn a_theme_file_has_colours_and_a_lights_profile() {
        let t = ThemeFile::parse(
            r##"{"colors": ["#ff8800"],
                 "lights": {"idle_effect": "static-color", "idle_colors": ["custom 0", "red", 15]}}"##,
        )
        .unwrap();
        assert_eq!(t.colors, Some(vec![Rgb::from_u32(0xff8800)]));
        let l = t.lights.as_ref().unwrap();
        assert_eq!(l.values[&5], Value::Bytes(vec![14, 2, 15]));
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(
            json["lights"]["idle_colors"],
            serde_json::json!([14, 2, 15])
        );
        assert_eq!(ThemeFile::parse(&json.to_string()).unwrap(), t);
        // Both parts are optional.
        let empty = ThemeFile::parse(r#"{"colors": null}"#).unwrap();
        assert!(empty.colors.is_none() && empty.lights.is_none());
        for (text, says) in [
            (r#"{"profile": 2}"#, "no key profile"),
            (r#"{"lights": [1]}"#, "lights: not a JSON object"),
            (
                r#"{"lights": {"idle_effect": 99, "x": 1}}"#,
                "lights: not a lights file: no field x",
            ),
            (
                r#"{"lights": {"idle_colors": ["custom 16"]}}"#,
                "no colour custom 16",
            ),
            (r##"{"colors": ["#ff"]}"##, "colors:"),
        ] {
            let e = ThemeFile::parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
