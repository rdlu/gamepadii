//! Smoke tests against this machine, read-only: sysfs, and the allowlisted
//! web-config reads when the pad is in web-config (never a reboot or a
//! write). `mise run test-live` runs them; they need a GP2040-CE pad
//! plugged in.

use gamepadii::output::{self, Format, Style};
use gamepadii_core::client::Client;
use gamepadii_core::commands;
use gamepadii_core::error::exit;
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

/// The live client, or `None` (with a note) when the pad isn't in
/// web-config: switching it is the user's call, not a test's.
fn webconfig() -> Option<Client> {
    match Client::connect(&Root::live()) {
        Ok(c) => Some(c),
        Err(e) if e.exit_code() == exit::NEEDS_USER => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
#[ignore = "needs a GP2040-CE pad in web-config"]
fn info_reads_the_pad() {
    let Some(client) = webconfig() else { return };
    let info = commands::info(&client).expect("info");
    print!("{}", output::info(Style::plain(Format::Human), &info));
    assert!(info.firmware.version.is_some());
    assert!(!info.profiles.is_empty());
}

#[test]
#[ignore = "needs a GP2040-CE pad in web-config"]
fn backup_reads_every_allowlisted_endpoint() {
    let Some(client) = webconfig() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.json");
    let report =
        commands::backup(&client, Some(&path), std::time::SystemTime::now()).expect("backup");
    print!("{}", output::backup(Style::plain(Format::Human), &report));
    assert!(path.exists());
    assert!(!report.endpoints.is_empty());
}
