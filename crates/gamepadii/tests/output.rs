//! `status` rendered from the sysfs fixtures (see gamepadii-core's
//! `tests/discovery.rs` for where each tree comes from).

use std::path::PathBuf;

use gamepadii::output::{self, Format, Style};
use gamepadii_core::commands;
use gamepadii_core::sysfs::Root;

fn status(fixture: &str, format: Format) -> String {
    let root = Root::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sysfs")
            .join(fixture),
    );
    output::status(Style::plain(format), &commands::status(&root).unwrap())
}

fn waybar(fixture: &str) -> serde_json::Value {
    let line = status(fixture, Format::Waybar);
    assert!(!line.contains('\n'), "waybar output must be one line");
    serde_json::from_str(&line).unwrap()
}

#[test]
fn human_s16_webconfig() {
    insta::assert_snapshot!(status("s16-webconfig", Format::Human));
}

#[test]
fn human_s16_xinput() {
    insta::assert_snapshot!(status("s16-xinput", Format::Human));
}

#[test]
fn human_two_pads() {
    insta::assert_snapshot!(status("two-pads", Format::Human));
}

#[test]
fn json_s16_webconfig() {
    insta::assert_snapshot!(status("s16-webconfig", Format::Json));
}

#[test]
fn json_s16_xinput() {
    insta::assert_snapshot!(status("s16-xinput", Format::Json));
}

#[test]
fn json_keeps_nulls() {
    let v: serde_json::Value = serde_json::from_str(&status("bootsel", Format::Json)).unwrap();
    assert_eq!(v["schema"], 1);
    let d = &v["devices"][0];
    assert_eq!(d["mode"], "bootsel");
    assert!(d.get("netdev").is_some_and(|n| n.is_null()));
    let v: serde_json::Value = serde_json::from_str(&status("two-pads", Format::Json)).unwrap();
    let d = &v["devices"][1];
    assert!(d["mode"].is_null());
    assert_eq!(d["candidates"], serde_json::json!(["ps4", "ps5"]));
    assert!(d["serial"].is_null());
}

#[test]
fn waybar_lines() {
    let v = waybar("s16-xinput");
    assert_eq!(v["class"], "xinput");
    assert_eq!(v["alt"], "xinput");
    assert_eq!(v["text"], format!("{} XInput", output::GLYPH));
    assert!(
        v["tooltip"]
            .as_str()
            .unwrap()
            .starts_with("XInput (likely)\n")
    );
    let v = waybar("two-pads");
    assert_eq!(v["class"], "webconfig");
    assert_eq!(v["text"], format!("{} web-config +1", output::GLYPH));
    assert!(
        v["tooltip"]
            .as_str()
            .unwrap()
            .contains("PS4 or PS5 (likely)")
    );
}

#[test]
fn waybar_not_found() {
    let err = gamepadii_core::Error::NotFound("no device".into());
    let v: serde_json::Value = serde_json::from_str(&output::waybar_error(&err).to_line()).unwrap();
    assert_eq!(v["class"], "disconnected");
    assert_eq!(v["text"], output::GLYPH);
}
