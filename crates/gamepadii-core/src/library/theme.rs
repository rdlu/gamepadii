//! Themes: the colours and the effects of a lights profile, in KDL. A
//! theme becomes the theme file of `gamepadii leds theme apply`
//! ([`ThemeFile`]).
//!
//! A colour is a built-in colour (`black` to `magenta`), `custom N` (a
//! custom colour of the pad), or `#rrggbb`. gamepadii puts each different
//! `#rrggbb` into a custom colour, in the order of first use, so a theme
//! has at most 16 of them. The custom colours are global: the 4 lights
//! profiles of the pad share them. The colour lists of the lights profile
//! have one colour for each GPIO: a `button` node gives the colours of one
//! button, by its board name, and the other GPIOs get `idle` and
//! `pressed`.

use kdl::{KdlEntry, KdlNode, KdlValue};
use serde::Serialize;
use serde_json::{Map, Value as Json};

use super::board::Board;
use super::parse::{Src, args, one_string, one_string_with_children};
use super::{LibraryError, Origin, Place};
use crate::channel::proto::{CUSTOM_COLORS_MAX, Rgb};
use crate::colors::{self, ThemeFile};
use crate::leds::COLORS;
use crate::lights::{self, BASE_COLORS, FIELDS, LightsFile};

/// The colour lists of a lights profile have one colour for each GPIO:
/// `NUM_BANK0_GPIOS` of the RP2040, or more for a board with more GPIOs.
const GPIOS: usize = 30;
/// `MAX_NON_BUTTON_LIGHT_COLOR_INDEXES`: the case lights.
const CASE_LIGHTS: usize = 32;

/// The lights-file keys that a theme gives with its own nodes.
const OWN_KEYS: &[(&str, &str)] = &[
    ("idle_colors", "idle and button"),
    ("pressed_colors", "pressed and button"),
    ("case_colors", "case"),
    ("idle_effect", "effect idle="),
    ("pressed_effect", "effect pressed="),
    ("case_effect", "effect case="),
];

/// The colours of one button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeButton {
    /// The name or an alias in the board file.
    pub name: String,
    /// Colour numbers: 0–13 built in, 14–29 custom.
    pub idle: Option<u8>,
    pub pressed: Option<u8>,
}

/// One theme, as its file gives it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Theme {
    pub name: String,
    /// The board file name; it is necessary for `button` nodes.
    pub board: Option<String>,
    /// The custom colours of the `#rrggbb` values, custom colour 0 first.
    /// Empty: the theme does not change the custom colours.
    pub colors: Vec<Rgb>,
    /// The colour numbers of all buttons, when idle and when pressed.
    pub idle: Option<u8>,
    pub pressed: Option<u8>,
    pub buttons: Vec<ThemeButton>,
    /// The colour number of all case lights.
    pub case: Option<u8>,
    /// The other fields of the lights profile: lights-file key → value.
    pub fields: Map<String, Json>,
}

/// Where the parts of a theme are in its file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemePlaces {
    pub node: Place,
    pub board: Option<Place>,
    /// One for each of [`Theme::buttons`].
    pub buttons: Vec<Place>,
}

/// A theme and the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinedTheme {
    pub theme: Theme,
    pub origin: Origin,
    pub at: ThemePlaces,
}

/// The colours that a theme uses, while it is read.
#[derive(Default)]
struct Palette {
    colors: Vec<Rgb>,
    /// The first `custom N`.
    custom: Option<Place>,
}

impl Palette {
    /// The colour number of one value: a name, `custom N` or `#rrggbb`.
    fn number(&mut self, src: &Src, e: &KdlEntry) -> Result<u8, LibraryError> {
        let Some(s) = e.value().as_string() else {
            return Err(
                src.entry_error(e, "a colour is a name, custom N or \"#rrggbb\" (in quotes)")
            );
        };
        if s.starts_with('#') {
            let rgb = Rgb::parse_hex(s).ok_or_else(|| {
                src.entry_error(e, &format!("{s:?} is not a colour: give #rrggbb"))
            })?;
            let slot = match self.colors.iter().position(|c| *c == rgb) {
                Some(n) => n,
                None => {
                    if self.colors.len() == CUSTOM_COLORS_MAX {
                        return Err(src.entry_error(
                            e,
                            &format!(
                                "{s} is colour {} of the theme in #rrggbb: the pad keeps at \
                                 most {CUSTOM_COLORS_MAX} custom colours",
                                CUSTOM_COLORS_MAX + 1
                            ),
                        ));
                    }
                    self.colors.push(rgb);
                    self.colors.len() - 1
                }
            };
            return Ok(colors::number(slot) as u8);
        }
        let n = lights::color_number(s).ok_or_else(|| {
            src.entry_error(
                e,
                &format!(
                    "unknown colour {s:?}: give {}, custom 0 to custom 15, or \"#rrggbb\"",
                    COLORS[..BASE_COLORS].join(", ")
                ),
            )
        })?;
        if usize::from(n) >= BASE_COLORS && self.custom.is_none() {
            self.custom = Some(src.place(e.span().offset()));
        }
        Ok(n)
    }
}

/// The one argument of a node with no properties and no children.
fn one_arg<'a>(src: &Src, node: &'a KdlNode) -> Result<&'a KdlEntry, LibraryError> {
    let what = node.name().value();
    let mut a = args(node);
    match (a.next(), a.next()) {
        (Some(e), None) if node.entries().len() == 1 && node.children().is_none() => Ok(e),
        _ => Err(src.node_error(node, &format!("{what}: give one value"))),
    }
}

/// A KDL value as JSON, for the lights file.
fn json(v: &KdlValue) -> Option<Json> {
    match v {
        KdlValue::String(s) => Some(Json::String(s.clone())),
        KdlValue::Integer(n) => i64::try_from(*n).ok().map(Json::from),
        KdlValue::Bool(b) => Some(Json::Bool(*b)),
        _ => None,
    }
}

/// One lights-file field: `key` and its value, checked as `leds profile
/// write` checks it.
fn field(
    src: &Src,
    e: &KdlEntry,
    key: &str,
    map: &mut Map<String, Json>,
) -> Result<(), LibraryError> {
    let value = json(e.value())
        .ok_or_else(|| src.entry_error(e, &format!("{key}: a string, a number or #true/#false")))?;
    let mut one = Map::new();
    one.insert(key.to_string(), value.clone());
    let checked = LightsFile::parse(&Json::Object(one).to_string())
        .map_err(|e| e.to_string())
        .and_then(|f| f.check());
    if let Err(msg) = checked {
        let msg = msg.trim_start_matches("usage: ");
        let msg = msg.strip_prefix("not a lights file: ").unwrap_or(msg);
        return Err(src.entry_error(e, msg));
    }
    if map.insert(key.to_string(), value).is_some() {
        return Err(src.entry_error(e, &format!("{key} is twice in the theme")));
    }
    Ok(())
}

/// `theme NAME { board; idle; pressed; button; case; effect; lights }`.
pub fn parse(src: &Src, node: &KdlNode) -> Result<DefinedTheme, LibraryError> {
    let mut t = Theme {
        name: one_string_with_children(src, node)?,
        ..Theme::default()
    };
    let mut at = ThemePlaces {
        node: src.node_place(node),
        ..ThemePlaces::default()
    };
    let mut palette = Palette::default();
    let mut seen: Vec<&str> = Vec::new();
    for child in node.iter_children() {
        let key = child.name().value();
        if key != "button" {
            if seen.contains(&key) {
                return Err(src.node_error(child, &format!("{key} is twice in the theme")));
            }
            seen.push(key);
        }
        match key {
            "board" => {
                t.board = Some(one_string(src, child)?);
                at.board = Some(src.node_place(child));
            }
            "idle" => t.idle = Some(palette.number(src, one_arg(src, child)?)?),
            "pressed" => t.pressed = Some(palette.number(src, one_arg(src, child)?)?),
            "case" => t.case = Some(palette.number(src, one_arg(src, child)?)?),
            "button" => {
                let name = one_string(src, child)?;
                if child.children().is_some() {
                    return Err(src.node_error(child, "button has no children"));
                }
                let mut b = ThemeButton {
                    name,
                    idle: None,
                    pressed: None,
                };
                for e in child.entries().iter().filter(|e| e.name().is_some()) {
                    match e.name().map(|n| n.value()).unwrap_or_default() {
                        "idle" if b.idle.is_none() => b.idle = Some(palette.number(src, e)?),
                        "pressed" if b.pressed.is_none() => {
                            b.pressed = Some(palette.number(src, e)?)
                        }
                        k @ ("idle" | "pressed") => {
                            return Err(src.entry_error(e, &format!("{k} is twice")));
                        }
                        k => {
                            return Err(src.entry_error(
                                e,
                                &format!("unknown property {k:?} (idle, pressed)"),
                            ));
                        }
                    }
                }
                if b.idle.is_none() && b.pressed.is_none() {
                    return Err(src.node_error(
                        child,
                        &format!(
                            "button {}: give idle=COLOUR, pressed=COLOUR or both",
                            b.name
                        ),
                    ));
                }
                if t.buttons.iter().any(|o| o.name == b.name) {
                    return Err(
                        src.node_error(child, &format!("button {} is twice in the theme", b.name))
                    );
                }
                t.buttons.push(b);
                at.buttons.push(src.node_place(child));
            }
            "effect" => {
                if args(child).next().is_some() || child.children().is_some() {
                    return Err(src.node_error(child, "effect: give idle=, pressed= or case="));
                }
                for e in child.entries() {
                    let k = e.name().map(|n| n.value()).unwrap_or_default();
                    let key = match k {
                        "idle" => "idle_effect",
                        "pressed" => "pressed_effect",
                        "case" => "case_effect",
                        _ => {
                            return Err(src.entry_error(
                                e,
                                &format!("unknown property {k:?} (idle, pressed, case)"),
                            ));
                        }
                    };
                    field(src, e, key, &mut t.fields)?;
                }
            }
            "lights" => {
                if args(child).next().is_some() || child.children().is_some() {
                    return Err(src.node_error(child, "lights: give KEY=VALUE properties"));
                }
                for e in child.entries() {
                    let k = e.name().map(|n| n.value()).unwrap_or_default();
                    if let Some((_, node)) = OWN_KEYS.iter().find(|(o, _)| *o == k) {
                        return Err(src.entry_error(
                            e,
                            &format!("{k}: a theme gives it with {node}, not with lights"),
                        ));
                    }
                    if !FIELDS.iter().any(|f| f.key == k) {
                        let keys: Vec<&str> = FIELDS
                            .iter()
                            .map(|f| f.key)
                            .filter(|k| !OWN_KEYS.iter().any(|(o, _)| o == k))
                            .collect();
                        return Err(src.entry_error(
                            e,
                            &format!("unknown lights field {k:?} ({})", keys.join(", ")),
                        ));
                    }
                    field(src, e, k, &mut t.fields)?;
                }
            }
            other => {
                return Err(src.node_error(
                    child,
                    &format!(
                        "unknown node {other:?} in a theme (board, idle, pressed, button, case, \
                         effect, lights)"
                    ),
                ));
            }
        }
    }
    if let Some(place) = palette.custom
        && !palette.colors.is_empty()
    {
        return Err(LibraryError::at(
            &src.origin,
            Some(place),
            "a theme with #rrggbb colours sets the custom colours itself: give #rrggbb here \
             too, not custom N",
        ));
    }
    t.colors = palette.colors;
    if t.colors.is_empty()
        && t.idle.is_none()
        && t.pressed.is_none()
        && t.case.is_none()
        && t.buttons.is_empty()
        && t.fields.is_empty()
    {
        return Err(src.node_error(node, &format!("theme {:?} sets nothing", t.name)));
    }
    Ok(DefinedTheme {
        theme: t,
        origin: src.origin.clone(),
        at,
    })
}

/// A colour, as `library show` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColorView {
    /// The colour number in the lights profile: 0–13 built in, 14–29
    /// custom.
    pub number: u8,
    /// `red`, or `custom 2`.
    pub name: String,
    /// The value that the theme gives a custom colour; `null` for a
    /// built-in colour, and for a custom colour that the pad keeps.
    pub rgb: Option<Rgb>,
}

/// The colours of one button, as `library show` gives them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ButtonView {
    pub button: String,
    pub gpio: u8,
    pub idle: Option<ColorView>,
    pub pressed: Option<ColorView>,
}

/// A theme, checked against its board: `gamepadii library show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTheme {
    pub name: String,
    pub file: Origin,
    pub shipped: bool,
    pub line: usize,
    /// `null`: the theme is for any board.
    pub board: Option<String>,
    pub firmware_board: Option<String>,
    /// The custom colours that the theme sets, custom colour 0 first;
    /// `null`: it does not change them.
    pub colors: Option<Vec<Rgb>>,
    pub idle: Option<ColorView>,
    pub pressed: Option<ColorView>,
    pub case: Option<ColorView>,
    /// The buttons with their own colours, in the order of the board file.
    pub buttons: Vec<ButtonView>,
    /// The other lights fields that the theme sets.
    pub fields: Map<String, Json>,
    /// What `leds theme apply` takes.
    pub theme_file: ThemeFile,
}

impl Theme {
    fn view(&self, number: u8) -> ColorView {
        let n = usize::from(number);
        match n.checked_sub(BASE_COLORS) {
            None => ColorView {
                number,
                name: COLORS[n].to_string(),
                rgb: None,
            },
            Some(c) => ColorView {
                number,
                name: format!("custom {c}"),
                rgb: self.colors.get(c).copied(),
            },
        }
    }
}

/// `t` with `board` (the theme's own board, or the board of the profile
/// that names it): the GPIO of each button, and the theme file.
pub fn resolve(d: &DefinedTheme, board: Option<&Board>) -> Result<ResolvedTheme, LibraryError> {
    let t = &d.theme;
    let err = |place: Option<Place>, msg: String| LibraryError::at(&d.origin, place, &msg);
    let mut buttons: Vec<(usize, ButtonView)> = Vec::new();
    for (b, place) in t.buttons.iter().zip(&d.at.buttons) {
        let Some(board) = board else {
            return Err(err(
                Some(*place),
                format!(
                    "theme {:?} has button nodes but no board: add `board \"haute42-s16\"`",
                    t.name
                ),
            ));
        };
        let Some(on) = board.button(&b.name) else {
            if let Some(new) = board.renamed(&b.name) {
                return Err(err(
                    Some(*place),
                    format!("button {:?} is now {new:?} on {}", b.name, board.name),
                ));
            }
            let known: Vec<&str> = board.buttons.iter().map(|b| b.name.as_str()).collect();
            return Err(err(
                Some(*place),
                format!(
                    "unknown button {:?}: {} has {}",
                    b.name,
                    board.name,
                    known.join(", ")
                ),
            ));
        };
        if let Some((_, o)) = buttons.iter().find(|(_, o)| o.button == on.name) {
            return Err(err(
                Some(*place),
                format!(
                    "button {} is twice in the theme: {} names it too",
                    on.name, o.button
                ),
            ));
        }
        let index = board.buttons.iter().position(|o| o.name == on.name);
        buttons.push((
            index.unwrap_or_default(),
            ButtonView {
                button: on.name.clone(),
                gpio: on.gpio,
                idle: b.idle.map(|n| t.view(n)),
                pressed: b.pressed.map(|n| t.view(n)),
            },
        ));
    }
    buttons.sort_by_key(|(i, _)| *i);
    let buttons: Vec<ButtonView> = buttons.into_iter().map(|(_, b)| b).collect();
    let gpios = board
        .and_then(|b| b.buttons.iter().map(|b| usize::from(b.gpio) + 1).max())
        .unwrap_or(0)
        .max(GPIOS);
    // One colour for each GPIO: the button's own, else the colour of all
    // buttons, else black.
    let list = |all: Option<u8>, own: fn(&ButtonView) -> Option<&ColorView>| {
        let mut l = vec![all.unwrap_or(0); gpios];
        for b in &buttons {
            if let Some(c) = own(b) {
                l[usize::from(b.gpio)] = c.number;
            }
        }
        Json::from(l)
    };
    let mut fields = t.fields.clone();
    if t.idle.is_some() || buttons.iter().any(|b| b.idle.is_some()) {
        fields.insert("idle_colors".into(), list(t.idle, |b| b.idle.as_ref()));
    }
    if t.pressed.is_some() || buttons.iter().any(|b| b.pressed.is_some()) {
        fields.insert(
            "pressed_colors".into(),
            list(t.pressed, |b| b.pressed.as_ref()),
        );
    }
    if let Some(c) = t.case {
        fields.insert("case_colors".into(), Json::from(vec![c; CASE_LIGHTS]));
    }
    let firmware_board = board.map(|b| b.firmware_board.clone());
    let mut lights = LightsFile::parse(&Json::Object(fields).to_string())
        .map_err(|e| err(Some(d.at.node), e.to_string()))?;
    lights.check().map_err(|e| err(Some(d.at.node), e))?;
    lights.board = firmware_board.clone();
    let colors = (!t.colors.is_empty()).then(|| t.colors.clone());
    Ok(ResolvedTheme {
        name: t.name.clone(),
        file: d.origin.clone(),
        shipped: d.origin.shipped(),
        line: d.at.node.line,
        board: board.map(|b| b.name.clone()),
        firmware_board: firmware_board.clone(),
        colors: colors.clone(),
        idle: t.idle.map(|n| t.view(n)),
        pressed: t.pressed.map(|n| t.view(n)),
        case: t.case.map(|n| t.view(n)),
        buttons,
        fields: t.fields.clone(),
        theme_file: ThemeFile {
            board: firmware_board,
            colors,
            lights: Some(lights),
        },
    })
}
