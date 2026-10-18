//! Profiles → KDL 2.0 text, in the form that [`super::parse`] reads.

use super::{Pin, Profile};
use crate::slots::action_name;

/// `profiles` as one KDL file, after the `//` lines of `header`.
pub fn file(header: &[String], profiles: &[Profile]) -> String {
    let mut out = String::new();
    for line in header {
        out.push_str(&format!("// {line}\n"));
    }
    for (i, p) in profiles.iter().enumerate() {
        if i > 0 || !header.is_empty() {
            out.push('\n');
        }
        out.push_str(&profile(p));
    }
    out
}

/// One `profile` node.
pub fn profile(p: &Profile) -> String {
    let mut out = format!("profile {} {{\n", string(&p.name));
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
    if p.default.is_some() || !p.pins.is_empty() {
        out.push_str("    pins {\n");
        if let Some(d) = &p.default {
            out.push_str(&format!("        default {}\n", pin(d)));
        }
        for (button, action) in &p.pins {
            out.push_str(&format!("        {} {}\n", ident(button), pin(action)));
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// `B1`, `none`, or `custom-button-combo button-mask=0x3 dpad-mask=0x0`.
fn pin(p: &Pin) -> String {
    let mut out = action_name(p.action).map_or_else(|| p.action.to_string(), |n| ident(&n));
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
