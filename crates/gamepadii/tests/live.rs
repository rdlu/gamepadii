//! Smoke tests against this machine (read-only: sysfs only, no HTTP).
//! `mise run test-live` runs them; they need a GP2040-CE pad plugged in.

use gamepadii::output::{self, Format, Style};
use gamepadii_core::commands;
use gamepadii_core::sysfs::Root;

#[test]
#[ignore = "needs a GP2040-CE pad on this machine's USB"]
fn status_finds_a_pad() {
    let status = commands::status(&Root::live()).expect("a GP2040-CE device");
    print!("{}", output::status(Style::plain(Format::Human), &status));
    let json: serde_json::Value =
        serde_json::from_str(&output::status(Style::plain(Format::Json), &status)).unwrap();
    assert_eq!(json["schema"], 1);
    for d in &status.devices {
        assert!(d.sysfs_path.is_some(), "{d:?}");
        assert!(!d.candidates.is_empty());
    }
}
