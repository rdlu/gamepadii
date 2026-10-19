//! Profiles → KDL 2.0 text, in the form that [`super::parse`] reads.

use super::{Pin, Profile};
use crate::buttons::NameSystem;
use crate::slots::action_name_in;

/// `profiles` as one KDL file, after the `//` lines of `header`. When all
/// profiles use one name system other than GP2040, a `names` node at the
/// top gives it; else each profile gives its own.
pub fn file(header: &[String], profiles: &[Profile]) -> String {
    let mut out = String::new();
    for line in header {
        out.push_str(&format!("// {line}\n"));
    }
    let system = match profiles.first() {
        Some(p) if profiles.iter().all(|o| o.names == p.names) => p.names,
        _ => NameSystem::Gp2040,
    };
    let top = system != NameSystem::Gp2040;
    if top {
        if !header.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("names {}\n", system.key()));
    }
    for (i, p) in profiles.iter().enumerate() {
        if i > 0 || !header.is_empty() || top {
            out.push('\n');
        }
        out.push_str(&profile(p, system));
    }
    out
}

/// One `profile` node, in a file with the name system `system`.
pub fn profile(p: &Profile, system: NameSystem) -> String {
    let mut out = format!("profile {} {{\n", string(&p.name));
    if p.names != system {
        out.push_str(&format!("    names {}\n", p.names.key()));
    }
    let mut prop = |key: &str, value: &Option<String>| {
        if let Some(v) = value {
            out.push_str(&format!("    {key} {}\n", string(v)));
        }
    };
    prop("base", &p.base);
    prop("board", &p.board);
    prop("label", &p.label);
    if let Some(m) = p.input_mode {
        out.push_str(&format!("    input-mode {}\n", m.key().replace('_', "-")));
    }
    if let Some(t) = &p.theme {
        out.push_str(&format!("    theme {}\n", string(t)));
    }
    if p.default.is_some() || !p.pins.is_empty() {
        out.push_str("    pins {\n");
        if let Some(d) = &p.default {
            out.push_str(&format!("        default {}\n", pin(d, p.names)));
        }
        for (button, action) in &p.pins {
            out.push_str(&format!(
                "        {} {}\n",
                ident(button),
                pin(action, p.names)
            ));
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// `B1` (`A` in Xbox names), `none`, or `custom-button-combo
/// button-mask=0x3 dpad-mask=0x0`.
fn pin(p: &Pin, names: NameSystem) -> String {
    let mut out =
        action_name_in(names, p.action).map_or_else(|| p.action.to_string(), |n| ident(&n));
    if p.button_mask != 0 || p.dpad_mask != 0 {
        out.push_str(&format!(
            " button-mask={:#x} dpad-mask={:#x}",
            p.button_mask, p.dpad_mask
        ));
    }
    out
}

/// A bare identifier when KDL takes it as one, else a quoted string.
fn ident(s: &str) -> String {
    let mut chars = s.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !matches!(s, "true" | "false" | "null" | "inf" | "nan");
    if plain { s.to_string() } else { string(s) }
}

/// A quoted KDL string.
pub fn string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
