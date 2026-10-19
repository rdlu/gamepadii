//! KDL 2.0 text → profiles, with the line and column of each part, so
//! that a later check can point at the KDL that it does not accept.

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use super::theme::DefinedTheme;
use super::{Defined, LibraryError, Origin, Pin, Place, Places, Profile};
use crate::buttons::NameSystem;
use crate::mode::InputMode;
use crate::slots::{ACTIONS, action_from_name_in};

/// One file, for the places in its errors.
pub struct Src<'a> {
    pub origin: Origin,
    text: &'a str,
}

impl<'a> Src<'a> {
    pub fn new(origin: Origin, text: &'a str) -> Self {
        Self { origin, text }
    }

    pub fn document(&self) -> Result<KdlDocument, LibraryError> {
        KdlDocument::parse_v2(self.text).map_err(|e| {
            let d = e.diagnostics.first();
            let offset = d.map_or(0, |d| d.span.offset());
            let mut msg = d
                .and_then(|d| d.message.clone())
                .unwrap_or_else(|| "this is not a KDL 2.0 document".into());
            if let Some(help) = d.and_then(|d| d.help.as_deref()) {
                msg.push_str(&format!(" ({help})"));
            }
            self.error_at(offset, &format!("KDL: {msg}"))
        })
    }

    /// The line and column (from 1, in characters) of byte `offset`, after
    /// the white space there.
    pub fn place(&self, offset: usize) -> Place {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let rest = &self.text[offset..];
        offset += rest.len() - rest.trim_start().len();
        let before = &self.text[..offset];
        let line = before.matches('\n').count() + 1;
        let start = before.rfind('\n').map_or(0, |i| i + 1);
        Place {
            line,
            column: before[start..].chars().count() + 1,
        }
    }

    pub fn error_at(&self, offset: usize, msg: &str) -> LibraryError {
        LibraryError::at(&self.origin, Some(self.place(offset)), msg)
    }

    pub fn node_place(&self, node: &KdlNode) -> Place {
        self.place(node.name().span().offset())
    }

    pub fn node_error(&self, node: &KdlNode, msg: &str) -> LibraryError {
        LibraryError::at(&self.origin, Some(self.node_place(node)), msg)
    }

    pub fn entry_error(&self, entry: &KdlEntry, msg: &str) -> LibraryError {
        self.error_at(entry.span().offset(), msg)
    }
}

/// The arguments of `node`, without its properties.
pub fn args(node: &KdlNode) -> impl Iterator<Item = &KdlEntry> {
    node.entries().iter().filter(|e| e.name().is_none())
}

/// The one string argument of `node`, with no properties and no children.
pub fn one_string(src: &Src, node: &KdlNode) -> Result<String, LibraryError> {
    let what = node.name().value();
    let mut args = args(node);
    let value = match (args.next(), args.next()) {
        (Some(e), None) => e
            .value()
            .as_string()
            .ok_or_else(|| src.entry_error(e, &format!("{what}: the value must be a string")))?,
        _ => return Err(src.node_error(node, &format!("{what}: give one value"))),
    };
    if value.is_empty() {
        return Err(src.node_error(node, &format!("{what}: the value is empty")));
    }
    if what != "button"
        && let Some(e) = node.entries().iter().find(|e| e.name().is_some())
    {
        return Err(src.entry_error(e, &format!("{what} has no properties")));
    }
    if node.children().is_some() {
        return Err(src.node_error(node, &format!("{what} has no children")));
    }
    Ok(value.to_string())
}

/// `default` in a `pins` block is not a button: no board has a button
/// with that name.
pub fn is_reserved_pin_name(name: &str) -> bool {
    name == "default"
}

/// What one library file holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub profiles: Vec<Defined>,
    pub themes: Vec<DefinedTheme>,
}

/// The profiles in one library file.
pub fn profiles(origin: Origin, text: &str) -> Result<Vec<Defined>, LibraryError> {
    file(origin, text).map(|p| p.profiles)
}

/// `names SYSTEM`: the name system of a file or a profile.
fn names(src: &Src, node: &KdlNode) -> Result<NameSystem, LibraryError> {
    one_string(src, node)?
        .parse()
        .map_err(|e: String| src.node_error(node, &e))
}

/// The profiles and the themes in one library file.
pub fn file(origin: Origin, text: &str) -> Result<Parsed, LibraryError> {
    let src = Src::new(origin, text);
    let doc = src.document()?;
    // The name system of the file applies to each profile, also to a
    // profile before the names node.
    let mut system = None;
    for node in doc.nodes().iter().filter(|n| n.name().value() == "names") {
        if system.is_some() {
            return Err(src.node_error(node, "names is twice in the file"));
        }
        system = Some(names(&src, node)?);
    }
    let system = system.unwrap_or_default();
    let mut out = Parsed::default();
    for node in doc.nodes() {
        match node.name().value() {
            "profile" => out.profiles.push(profile(&src, node, system)?),
            "theme" => out.themes.push(super::theme::parse(&src, node)?),
            "names" => {}
            n @ ("keymap" | "hotkeys" | "loadout" | "host") => {
                return Err(src.node_error(
                    node,
                    &format!(
                        "gamepadii does not read {n} nodes yet: only profile, theme and names \
                         nodes"
                    ),
                ));
            }
            n => {
                return Err(src.node_error(
                    node,
                    &format!(
                        "unknown node {n:?}: a library file has profile, theme and names nodes"
                    ),
                ));
            }
        }
    }
    Ok(out)
}

fn profile(src: &Src, node: &KdlNode, system: NameSystem) -> Result<Defined, LibraryError> {
    let name = one_string_with_children(src, node)?;
    let mut p = Profile {
        name,
        names: system,
        ..Profile::default()
    };
    let mut at = Places {
        node: src.node_place(node),
        ..Places::default()
    };
    // The name system of the profile applies to its pins, also to pins
    // before the names node.
    if let Some(child) = node.iter_children().find(|c| c.name().value() == "names") {
        p.names = names(src, child)?;
    }
    let mut seen: Vec<(&str, KdlValue)> = Vec::new();
    for child in node.iter_children() {
        let key = child.name().value();
        let value = args(child)
            .next()
            .map(|e| e.value().clone())
            .unwrap_or(KdlValue::Null);
        if let Some((_, before)) = seen.iter().find(|(k, _)| *k == key) {
            let msg = if key == "input-mode" && *before != value {
                "input-mode conflict: the profile gives two input modes".to_string()
            } else {
                format!("{key} is twice in the profile")
            };
            return Err(src.node_error(child, &msg));
        }
        seen.push((key, value));
        let place = Some(src.node_place(child));
        match key {
            "base" => {
                p.base = Some(one_string(src, child)?);
                at.base = place;
            }
            "board" => {
                p.board = Some(one_string(src, child)?);
                at.board = place;
            }
            "label" => {
                p.label = Some(one_string(src, child)?);
                at.label = place;
            }
            "input-mode" => {
                let v = one_string(src, child)?;
                let mode = v
                    .parse::<InputMode>()
                    .ok()
                    .filter(|m| !matches!(m, InputMode::Webconfig | InputMode::Bootsel))
                    .ok_or_else(|| src.node_error(child, &format!("{v:?} is not an input mode")))?;
                p.input_mode = Some(mode);
                at.input_mode = place;
            }
            "theme" => {
                p.theme = Some(one_string(src, child)?);
                at.theme = place;
            }
            "names" => {}
            "pins" => pins(src, child, &mut p, &mut at)?,
            other => {
                return Err(src.node_error(
                    child,
                    &format!(
                        "unknown node {other:?} in a profile (base, board, label, input-mode, \
                         names, theme, pins)"
                    ),
                ));
            }
        }
    }
    Ok(Defined {
        profile: p,
        origin: src.origin.clone(),
        at,
    })
}

/// The one string argument of a node that has children.
pub fn one_string_with_children(src: &Src, node: &KdlNode) -> Result<String, LibraryError> {
    let what = node.name().value();
    let mut a = args(node);
    match (
        a.next(),
        a.next(),
        node.entries().iter().any(|e| e.name().is_some()),
    ) {
        (Some(e), None, false) => match e.value().as_string() {
            Some(v) if !v.is_empty() => Ok(v.to_string()),
            _ => Err(src.entry_error(e, &format!("{what}: the name must be a string"))),
        },
        _ => Err(src.node_error(node, &format!("{what}: give one name, then {{ ... }}"))),
    }
}

/// `pins { BUTTON ACTION [button-mask=N] [dpad-mask=N]; default ACTION }`.
fn pins(src: &Src, node: &KdlNode, p: &mut Profile, at: &mut Places) -> Result<(), LibraryError> {
    if !node.entries().is_empty() {
        return Err(src.node_error(node, "pins: put the buttons in { ... }"));
    }
    for child in node.iter_children() {
        let button = child.name().value();
        let pin = pin(src, child, p.names)?;
        let place = src.node_place(child);
        if is_reserved_pin_name(button) {
            if p.default.is_some() {
                return Err(src.node_error(child, "default is twice in pins"));
            }
            p.default = Some(pin);
            at.default = Some(place);
            continue;
        }
        if p.pins.iter().any(|(b, _)| b == button) {
            return Err(src.node_error(child, &format!("button {button} is twice in pins")));
        }
        p.pins.push((button.to_string(), pin));
        at.pins.push(place);
    }
    Ok(())
}

/// The action of one `pins` line, by name in the name system `names`
/// (`B1`, `A`, `none`, `turbo`) or by number.
fn pin(src: &Src, node: &KdlNode, names: NameSystem) -> Result<Pin, LibraryError> {
    let button = node.name().value();
    let mut a = args(node);
    let (Some(e), None) = (a.next(), a.next()) else {
        return Err(src.node_error(node, &format!("{button}: give one action")));
    };
    let action = match e.value() {
        KdlValue::String(s) => action_from_name_in(names, s).ok_or_else(|| {
            let msg = match names {
                NameSystem::Gp2040 => format!(
                    "unknown action {s:?}: the GP2040 names are {}; the other actions have \
                     names such as none, turbo and macro-1, or a number",
                    names.names()
                ),
                _ => format!(
                    "unknown action {s:?}: the {names} names are {}; the GP2040 names (B1, \
                     Fn, turbo, none, ...) and numbers are also valid",
                    names.names()
                ),
            };
            src.entry_error(e, &msg)
        })?,
        KdlValue::Integer(n) => i16::try_from(*n)
            .ok()
            .filter(|n| ACTIONS.iter().any(|(v, _)| v == n))
            .ok_or_else(|| src.entry_error(e, &format!("action {n}: not a GpioAction")))?,
        _ => return Err(src.entry_error(e, "the action must be a name or a number")),
    };
    let mut pin = Pin {
        action,
        ..Pin::default()
    };
    for e in node.entries().iter().filter(|e| e.name().is_some()) {
        let key = e.name().map(|n| n.value()).unwrap_or_default();
        let mask = || {
            e.value()
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| src.entry_error(e, &format!("{key}: a number from 0")))
        };
        match key {
            "button-mask" => pin.button_mask = mask()?,
            "dpad-mask" => pin.dpad_mask = mask()?,
            _ => return Err(src.entry_error(e, &format!("unknown property {key:?}"))),
        }
    }
    if node.children().is_some() {
        return Err(src.node_error(node, &format!("{button} has no children")));
    }
    Ok(pin)
}
