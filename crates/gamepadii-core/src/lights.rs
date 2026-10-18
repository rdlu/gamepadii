//! The lights profiles of main over the control channel (v1.5, §3.11 of
//! `docs/design/control-channel.md`): READ_LIGHTS and WRITE_LIGHTS.
//!
//! `gamepadii leds profile show` reads one lights profile live. Its JSON is
//! also a lights file: `gamepadii leds profile write FILE --profile N`
//! writes it to lights profile N (0–3) in RAM, and selects and saves it on
//! request. gamepadii backs up the profile first, in the same format, so a
//! write of the backup undoes the write.
//!
//! The firmware sends the profile as an `AnimationProfile` protobuf
//! ([`crate::pb`]). A lights file has one key for each field, by the names
//! in [`FIELDS`], with the values as the firmware keeps them: the cycle
//! times are 0–9 (web-config shows 1–10). A file can leave out fields, or
//! give `null`: WRITE_LIGHTS sends only the others, and the pad keeps the
//! fields it does not get. A colour list goes complete. Fields that
//! gamepadii doesn't know go in `other_fields`, encoded, so that a backup
//! from a newer firmware keeps them.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value as Json;

use crate::channel::LightsData;
use crate::channel::proto::{ANIMATION_PROFILE_MAX, Slot};
use crate::commands::Route;
use crate::error::{Error, Result};
use crate::leds::{MAIN_IDLE_EFFECTS, MAIN_PRESSED_EFFECTS};
use crate::pb::{self, Value};

/// The firmware's colours (`colors` in `animation.h`); the custom colours
/// come after them.
pub const BASE_COLORS: usize = 14;
/// `MAX_CUSTOM_COLORS`.
pub const MAX_CUSTOM_COLORS: usize = 16;
/// `CYCLE_STEPS`: a cycle time is 0 to 9.
pub const CYCLE_STEPS: i32 = 10;

/// What a field holds, and how a lights file writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `bool`: `true` or `false`.
    Flag,
    /// An effect: its number, from the table. The file also gives
    /// `<key>_name`, and takes the name in place of the number.
    Effect(&'static [&'static str]),
    /// A cycle time, 0–9.
    Cycle,
    /// A `uint32`.
    Number,
    /// An RGB colour: `"#rrggbb"` (or a number).
    Color,
    /// Colour indexes, one byte each, at most this many.
    Colors(usize),
}

/// One field of `AnimationProfile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDef {
    /// The protobuf field number.
    pub number: u32,
    /// The key in a lights file.
    pub key: &'static str,
    /// The name in `config.proto`.
    pub proto: &'static str,
    pub kind: Kind,
}

const fn def(number: u32, key: &'static str, proto: &'static str, kind: Kind) -> FieldDef {
    FieldDef {
        number,
        key,
        proto,
        kind,
    }
}

/// `bEnabled`: the head's enabled flag replaces it.
pub const ENABLED_FIELD: u32 = 1;

/// The fields of `AnimationProfile` (77620cdf), except `bEnabled`.
pub const FIELDS: &[FieldDef] = &[
    def(
        2,
        "idle_effect",
        "baseNonPressedEffect",
        Kind::Effect(MAIN_IDLE_EFFECTS),
    ),
    def(
        3,
        "pressed_effect",
        "basePressedEffect",
        Kind::Effect(MAIN_PRESSED_EFFECTS),
    ),
    def(4, "idle_cycle_time", "baseCycleTime", Kind::Cycle),
    def(5, "idle_colors", "notPressedStaticColors", Kind::Colors(48)),
    def(6, "pressed_colors", "pressedStaticColors", Kind::Colors(48)),
    def(7, "press_hold_ms", "buttonPressHoldTimeInMs", Kind::Number),
    def(
        8,
        "press_fade_ms",
        "buttonPressFadeOutTimeInMs",
        Kind::Number,
    ),
    def(
        9,
        "idle_special_color",
        "nonPressedSpecialColor",
        Kind::Color,
    ),
    def(
        10,
        "case_effect",
        "baseCaseEffect",
        Kind::Effect(MAIN_IDLE_EFFECTS),
    ),
    def(11, "case_colors", "nonButtonStaticColors", Kind::Colors(32)),
    def(
        12,
        "pressed_special_color",
        "pressedSpecialColor",
        Kind::Color,
    ),
    def(
        13,
        "case_in_special_moves",
        "bUseCaseLightsInSpecialMoves",
        Kind::Flag,
    ),
    def(
        14,
        "pressed_cycle_time",
        "basePressedCycleTime",
        Kind::Cycle,
    ),
    def(
        15,
        "case_in_pressed",
        "bUseCaseLightsInPressedAnimations",
        Kind::Flag,
    ),
    def(16, "case_cycle_time", "baseCaseCycleTime", Kind::Cycle),
    def(
        17,
        "idle_special_rainbow",
        "bNonPressedSpecialColorIsRainbow",
        Kind::Flag,
    ),
    def(
        18,
        "pressed_special_rainbow",
        "bPressedSpecialColorIsRainbow",
        Kind::Flag,
    ),
    def(
        19,
        "case_special_rainbow",
        "bCaseSpecialColorIsRainbow",
        Kind::Flag,
    ),
    def(20, "case_special_color", "caseSpecialColor", Kind::Color),
    def(21, "effect_param", "effectContextParam", Kind::Number),
];

pub fn field(number: u32) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.number == number)
}

/// The keys of a lights file that are not fields.
const OTHER_KEYS: &[&str] = &[
    "schema",
    "via",
    "host",
    "board",
    "profile",
    "enabled",
    "active",
    "other_fields",
];

impl Kind {
    /// The wire type a field of this kind must have.
    fn takes(self, v: &Value) -> bool {
        match self {
            Kind::Colors(_) => matches!(v, Value::Bytes(_)),
            _ => matches!(v, Value::Varint(_)),
        }
    }
}

/// One lights profile, as a lights file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightsFile {
    /// `GP2040_BOARDCONFIG`: a write refuses another board.
    pub board: Option<String>,
    /// The profile it came from; `--profile` picks where it goes.
    pub profile: Option<u8>,
    /// `null` in a file: enabled.
    pub enabled: Option<bool>,
    /// The known fields, by number.
    pub values: BTreeMap<u32, Value>,
    /// The fields gamepadii doesn't know, each encoded (key and value).
    pub other: Vec<Vec<u8>>,
}

impl LightsFile {
    /// A READ_LIGHTS reply; `None` when the protobuf doesn't decode.
    pub fn from_data(d: &LightsData, board: Option<String>) -> Option<Self> {
        let mut f = LightsFile {
            board,
            profile: Some(d.profile),
            enabled: Some(d.enabled()),
            ..LightsFile::default()
        };
        for (number, value) in pb::decode(&d.protobuf)? {
            match field(number) {
                Some(def) if def.kind.takes(&value) => {
                    f.values.insert(number, value);
                }
                _ if number == ENABLED_FIELD => {}
                _ => {
                    let mut raw = Vec::new();
                    pb::encode_field(&mut raw, number, &value);
                    f.other.push(raw);
                }
            }
        }
        Some(f)
    }

    /// What WRITE_LIGHTS sends: the known fields in order, then the others.
    pub fn protobuf(&self) -> Vec<u8> {
        let fields: Vec<pb::Field> = self.values.iter().map(|(n, v)| (*n, v.clone())).collect();
        let mut b = pb::encode(&fields);
        for raw in &self.other {
            b.extend_from_slice(raw);
        }
        b
    }

    /// What the firmware checks in the fields it gets (§3.11).
    /// `custom_colors`: how many the pad has; `None` allows them all.
    pub fn check(&self, custom_colors: Option<usize>) -> std::result::Result<(), String> {
        let colors = BASE_COLORS + custom_colors.unwrap_or(MAX_CUSTOM_COLORS);
        for (number, value) in &self.values {
            let Some(def) = field(*number) else { continue };
            let n = value.as_i32();
            match (def.kind, value) {
                (Kind::Effect(table), _) => {
                    let ok = n
                        .and_then(|n| usize::try_from(n).ok())
                        .is_some_and(|n| n < table.len());
                    if !ok {
                        return Err(format!(
                            "{}: the effects are 0 to {}",
                            def.key,
                            table.len() - 1
                        ));
                    }
                }
                (Kind::Cycle, _) if !n.is_some_and(|n| (0..CYCLE_STEPS).contains(&n)) => {
                    return Err(format!(
                        "{}: the cycle times are 0 to {}",
                        def.key,
                        CYCLE_STEPS - 1
                    ));
                }
                (Kind::Colors(most), Value::Bytes(b)) => {
                    if b.len() > most {
                        return Err(format!("{}: at most {most} colours", def.key));
                    }
                    if let Some(bad) = b.iter().find(|c| usize::from(**c) >= colors) {
                        return Err(format!(
                            "{}: colour {bad}; the colours are 0 to {}",
                            def.key,
                            colors - 1
                        ));
                    }
                }
                _ => {}
            }
        }
        let len = self.protobuf().len();
        if len > ANIMATION_PROFILE_MAX {
            return Err(format!(
                "{len} bytes of protobuf: the pad takes at most {ANIMATION_PROFILE_MAX}"
            ));
        }
        Ok(())
    }

    pub fn parse(text: &str) -> Result<LightsFile> {
        let bad = |e: String| Error::Usage(format!("not a lights file: {e}"));
        let json: Json = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        let obj = json
            .as_object()
            .ok_or_else(|| bad("not a JSON object".into()))?;
        for key in obj.keys() {
            let known = OTHER_KEYS.contains(&key.as_str())
                || FIELDS.iter().any(|f| {
                    f.key == key
                        || matches!(f.kind, Kind::Effect(_))
                            && key.strip_suffix("_name") == Some(f.key)
                });
            if !known {
                return Err(bad(format!("no field {key}")));
            }
        }
        let get = |k: &str| obj.get(k).filter(|v| !v.is_null());
        let mut f = LightsFile {
            board: match get("board") {
                Some(v) => Some(
                    v.as_str()
                        .ok_or_else(|| bad("board: not text".into()))?
                        .into(),
                ),
                None => None,
            },
            profile: match get("profile") {
                Some(v) => Some(
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| bad("profile: not a number from 0".into()))?,
                ),
                None => None,
            },
            enabled: match get("enabled") {
                Some(v) => Some(
                    v.as_bool()
                        .ok_or_else(|| bad("enabled: not true or false".into()))?,
                ),
                None => None,
            },
            ..LightsFile::default()
        };
        for def in FIELDS {
            if let Some(v) = get(def.key) {
                let value = value_from(def, v).map_err(|e| bad(format!("{}: {e}", def.key)))?;
                f.values.insert(def.number, value);
            }
        }
        for v in get("other_fields")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let raw = v
                .as_str()
                .and_then(unhex)
                .filter(|raw| pb::decode(raw).is_some_and(|f| f.len() == 1))
                .ok_or_else(
                    || bad("other_fields: each must be one encoded field, in hex".into()),
                )?;
            f.other.push(raw);
        }
        Ok(f)
    }

    /// The keys after `schema` (and the report's `via` and `host`).
    fn entries(&self, active: Option<bool>) -> Vec<(String, Json)> {
        let mut e = vec![
            ("board".to_string(), self.board.clone().into()),
            ("profile".to_string(), self.profile.into()),
            ("enabled".to_string(), self.enabled.into()),
        ];
        if let Some(a) = active {
            e.push(("active".into(), a.into()));
        }
        for def in FIELDS {
            let v = self.values.get(&def.number);
            e.push((def.key.into(), v.map_or(Json::Null, |v| value_json(def, v))));
            if let Kind::Effect(table) = def.kind {
                let name = v
                    .and_then(Value::as_i32)
                    .and_then(|n| usize::try_from(n).ok())
                    .and_then(|n| table.get(n));
                e.push((format!("{}_name", def.key), name.copied().into()));
            }
        }
        let other: Vec<Json> = self.other.iter().map(|b| hex(b).into()).collect();
        e.push(("other_fields".into(), other.into()));
        e
    }
}

fn value_from(def: &FieldDef, v: &Json) -> std::result::Result<Value, String> {
    let int = || v.as_i64().ok_or_else(|| "not a whole number".to_string());
    match def.kind {
        Kind::Flag => match v {
            Json::Bool(b) => Ok(Value::Varint(u64::from(*b))),
            _ => Err("not true or false".into()),
        },
        Kind::Effect(table) => match v {
            Json::String(s) => table
                .iter()
                .position(|n| n.eq_ignore_ascii_case(s))
                .map(|n| Value::Varint(n as u64))
                .ok_or_else(|| format!("no effect {s}")),
            _ => Ok(Value::int32(
                i32::try_from(int()?).map_err(|e| e.to_string())?,
            )),
        },
        Kind::Cycle => Ok(Value::int32(
            i32::try_from(int()?).map_err(|e| e.to_string())?,
        )),
        Kind::Number => Ok(Value::Varint(u64::from(
            u32::try_from(int()?).map_err(|_| "not 0 to 4294967295".to_string())?,
        ))),
        Kind::Color => match v {
            Json::String(s) => s
                .strip_prefix('#')
                .filter(|h| (1..=8).contains(&h.len()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .map(|c| Value::Varint(u64::from(c)))
                .ok_or_else(|| format!("{s} is not a colour (#rrggbb)")),
            _ => Ok(Value::Varint(u64::from(
                u32::try_from(int()?).map_err(|_| "not a colour".to_string())?,
            ))),
        },
        Kind::Colors(_) => v
            .as_array()
            .ok_or_else(|| "not a list".to_string())?
            .iter()
            .map(|c| c.as_u64().and_then(|c| u8::try_from(c).ok()))
            .collect::<Option<Vec<u8>>>()
            .map(Value::Bytes)
            .ok_or_else(|| "each colour is a number, 0 to 255".into()),
    }
}

fn value_json(def: &FieldDef, v: &Value) -> Json {
    match (def.kind, v) {
        (Kind::Flag, Value::Varint(n)) => (*n != 0).into(),
        (Kind::Effect(_) | Kind::Cycle, v) => v.as_i32().into(),
        (Kind::Color, Value::Varint(n)) => format!("#{n:06x}").into(),
        (Kind::Colors(_), Value::Bytes(b)) => b.clone().into(),
        (_, Value::Varint(n)) => (*n).into(),
        _ => Json::Null,
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A lights file: `schema` 1, then the keys of [`FIELDS`].
impl Serialize for LightsFile {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let e = self.entries(None);
        let mut m = s.serialize_map(Some(e.len() + 1))?;
        m.serialize_entry("schema", &1)?;
        for (k, v) in &e {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// What `leds profile write` asks for, besides the file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightsWrite {
    /// The lights profile to write, 0–3.
    pub profile: u8,
    /// Select it (make it the lights profile that runs) now.
    pub select: bool,
    /// Save to flash.
    pub save: bool,
}

/// The WRITE_LIGHTS for `file`, on a pad that runs lights profile
/// `running` (`None`: off) and has board config `board`; `Usage` for what
/// the pad would refuse, before anything is sent.
pub fn plan(
    file: &LightsFile,
    write: &LightsWrite,
    running: Option<u8>,
    board: Option<&str>,
) -> Result<LightsData> {
    if let (Some(want), Some(have)) = (file.board.as_deref(), board)
        && want != have
    {
        return Err(Error::Usage(format!(
            "the file is for board {want}, but the pad is {have}"
        )));
    }
    if write.profile > 3 {
        return Err(Error::Usage(format!(
            "lights profile {}: the lights profiles are 0 to 3",
            write.profile
        )));
    }
    let enabled = file.enabled.unwrap_or(true);
    if !enabled && write.select {
        return Err(Error::Usage(
            "a disabled lights profile can't be the one to select".into(),
        ));
    }
    if !enabled && running == Some(write.profile) {
        return Err(Error::Usage(format!(
            "lights profile {} runs now: it must stay enabled",
            write.profile
        )));
    }
    file.check(None).map_err(Error::Usage)?;
    let flag = |on: bool, bit: u8| if on { bit } else { 0 };
    Ok(LightsData {
        profile: write.profile,
        flags: flag(enabled, Slot::ENABLED)
            | flag(write.select, Slot::ACTIVE)
            | flag(write.save, Slot::SAVE),
        protobuf: file.protobuf(),
    })
}

/// What READ_LIGHTS gives after a WRITE_LIGHTS of `sent` on top of
/// `before`: the fields that were sent replace the others.
pub fn expected(before: &LightsFile, sent: &LightsFile, enabled: bool) -> LightsFile {
    let mut f = before.clone();
    f.enabled = Some(enabled);
    f.values
        .extend(sent.values.iter().map(|(n, v)| (*n, v.clone())));
    f
}

/// The keys whose values differ between `a` and `b`.
pub fn changed(a: &LightsFile, b: &LightsFile) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    if a.enabled != b.enabled {
        keys.push("enabled".into());
    }
    keys.extend(
        FIELDS
            .iter()
            .filter(|f| a.values.get(&f.number) != b.values.get(&f.number))
            .map(|f| f.key.to_string()),
    );
    keys
}

/// `gamepadii leds profile show`: one lights profile, read live with
/// READ_LIGHTS. Its JSON is a lights file for `leds profile write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightsReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// It is the lights profile that runs.
    pub active: bool,
    pub file: LightsFile,
}

impl Serialize for LightsReport {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let e = self.file.entries(Some(self.active));
        // The front end adds `schema`, as for every report.
        let mut m = s.serialize_map(Some(e.len() + 2))?;
        m.serialize_entry("via", &self.via)?;
        m.serialize_entry("host", &self.host)?;
        for (k, v) in &e {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// `gamepadii leds profile write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LightsWriteReport {
    /// Always `control`: WRITE_LIGHTS.
    pub via: Route,
    pub host: String,
    pub profile: u8,
    pub before_enabled: bool,
    pub enabled: bool,
    /// The keys whose values changed.
    pub changed: Vec<String>,
    /// It is the lights profile that runs now (`--select`, or it was).
    pub active: bool,
    /// Saved to flash (`--save`).
    pub saved: bool,
    /// LIGHTS_STATUS says the LEDs apply it (PENDING is clear).
    pub applied: bool,
    /// READ_LIGHTS gives what was written.
    pub verified: bool,
    /// The profile as it was, in the lights file format; `null` with
    /// `--no-backup`.
    pub backup: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s16_profile_0() -> LightsData {
        let fields = vec![
            (1, Value::Varint(1)),
            (2, Value::Varint(1)),
            (3, Value::Varint(0)),
            (4, Value::Varint(1)),
            (5, Value::Bytes(vec![2; 30])),
            (9, Value::Varint(0xff0000)),
            (22, Value::Varint(9)),
        ];
        LightsData {
            profile: 0,
            flags: Slot::ENABLED | Slot::ACTIVE,
            protobuf: pb::encode(&fields),
        }
    }

    #[test]
    fn a_lights_file_round_trips() {
        let f = LightsFile::from_data(&s16_profile_0(), Some("Haute42COSMOX".into())).unwrap();
        assert_eq!(f.values.len(), 5, "bEnabled is the flag");
        assert_eq!(f.other, [vec![0xb0, 0x01, 9]]);
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["idle_effect"], 1);
        assert_eq!(json["idle_effect_name"], "rainbow-synced");
        assert_eq!(json["idle_special_color"], "#ff0000");
        assert_eq!(json["case_effect"], Json::Null);
        assert_eq!(json["case_effect_name"], Json::Null);
        assert_eq!(json["other_fields"][0], "b00109");
        let back = LightsFile::parse(&json.to_string()).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.protobuf(), {
            let mut b = s16_profile_0().protobuf;
            b.drain(..2); // without bEnabled
            b
        });
    }

    #[test]
    fn a_file_takes_names_and_leaves_out_fields() {
        let f = LightsFile::parse(
            r##"{"idle_effect": "Rain", "pressed_effect": 4, "case_cycle_time": null,
                "pressed_special_color": "#00ff00", "case_in_pressed": true}"##,
        )
        .unwrap();
        assert_eq!(f.values[&2], Value::Varint(19));
        assert_eq!(f.values[&12], Value::Varint(0x00ff00));
        assert_eq!(f.values.len(), 4);
        for (text, says) in [
            (r#"{"idle_efect": 1}"#, "no field idle_efect"),
            (r#"{"idle_effect": "sparkle"}"#, "no effect sparkle"),
            (r#"{"idle_colors": [1, 300]}"#, "0 to 255"),
            (r#"{"case_special_color": "red"}"#, "#rrggbb"),
            (r#"{"other_fields": ["0801ff"]}"#, "one encoded field"),
            (r#"[1]"#, "JSON object"),
        ] {
            let e = LightsFile::parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }

    #[test]
    fn the_host_checks_as_the_firmware_does() {
        let check = |text: &str| LightsFile::parse(text).unwrap().check(Some(0));
        assert_eq!(
            check(r#"{"idle_effect": 19, "pressed_cycle_time": 9}"#),
            Ok(())
        );
        for (text, says) in [
            (r#"{"idle_effect": 20}"#, "0 to 19"),
            (r#"{"pressed_effect": -1}"#, "0 to 5"),
            (r#"{"idle_cycle_time": 10}"#, "0 to 9"),
            (r#"{"case_colors": [14]}"#, "0 to 13"),
        ] {
            let e = check(text).unwrap_err();
            assert!(e.contains(says), "{text}: {e}");
        }
        let long = format!(r#"{{"case_colors": {:?}}}"#, vec![0u8; 33]);
        assert!(check(&long).unwrap_err().contains("at most 32"));
        let w = |profile, select| LightsWrite {
            profile,
            select,
            save: false,
        };
        let off = LightsFile::parse(r#"{"enabled": false}"#).unwrap();
        assert!(plan(&off, &w(1, false), Some(0), None).is_ok());
        assert!(plan(&off, &w(0, false), Some(0), None).is_err());
        assert!(plan(&off, &w(1, true), Some(0), None).is_err());
        let d = plan(&LightsFile::default(), &w(2, true), None, None).unwrap();
        assert_eq!(
            (d.flags, d.protobuf.len()),
            (Slot::ENABLED | Slot::ACTIVE, 0)
        );
        let pico = LightsFile {
            board: Some("Pico".into()),
            ..LightsFile::default()
        };
        assert!(plan(&pico, &w(1, false), None, Some("Haute42COSMOX")).is_err());
    }

    #[test]
    fn expected_keeps_what_was_not_sent() {
        let before = LightsFile::from_data(&s16_profile_0(), None).unwrap();
        let sent = LightsFile::parse(r#"{"idle_effect": 0, "press_hold_ms": 100}"#).unwrap();
        let after = expected(&before, &sent, true);
        assert_eq!(after.values[&2], Value::Varint(0));
        assert_eq!(after.values[&4], Value::Varint(1));
        assert_eq!(changed(&before, &after), ["idle_effect", "press_hold_ms"]);
    }
}
