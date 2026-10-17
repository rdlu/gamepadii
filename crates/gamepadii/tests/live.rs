//! Smoke tests against this machine: sysfs, and the allowlisted web-config
//! reads when the pad is in web-config. `mise run test-live` runs them;
//! they need a GP2040-CE pad plugged in. The two that change the pad (a
//! reboot, a settings write) also skip unless GAMEPADII_LIVE_REBOOT=1 or
//! GAMEPADII_LIVE_WRITE=1 is set, so by default the run is read-only.

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
    for d in status.devices.iter().map(|d| &d.device) {
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

/// Whether `var` is set to `1`: the extra opt-in for tests that change the
/// pad. Without it they skip, so `mise run test-live` stays read-only.
fn opted_in(var: &str) -> bool {
    let on = std::env::var(var).is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set {var}=1 to run this (it changes the pad)");
    }
    on
}

/// Reboots the pad from web-config back into web-config and checks it
/// re-enumerates. Needs GAMEPADII_LIVE_REBOOT=1.
#[test]
#[ignore = "reboots the pad; needs it in web-config and GAMEPADII_LIVE_REBOOT=1"]
fn reboot_into_web_config_comes_back() {
    use gamepadii_core::flow::{self, Env, Silent, Timeouts};
    use gamepadii_core::mode::BootMode;
    use gamepadii_core::transport::http::{self, Http};
    use gamepadii_core::wait::{Poll, SystemClock};
    if !opted_in("GAMEPADII_LIVE_REBOOT") {
        return;
    }
    let Some(client) = webconfig() else { return };
    let root = Root::live();
    let clock = SystemClock::new();
    let open = || Client::open(Box::new(Http::new(http::DEFAULT_HOST)?));
    let env = Env {
        scan: &root,
        clock: &clock,
        open: &open,
        control: None,
        cache_dir: None,
        backups_dir: None,
        url: "http://192.168.7.1/".into(),
        timeouts: Timeouts::default(),
        poll: Poll::INTERVAL,
    };
    let r = flow::reboot(&env, &client, BootMode::Webconfig, true, &mut Silent).expect("reboot");
    print!("{}", output::reboot(Style::plain(Format::Human), &r));
    assert_eq!(r.verified, Some(true));
}

/// A real `setGamepadOptions` round trip: backs up, bumps `debounceDelay`
/// by one, checks nothing else changed, and puts it back. Needs the pad in
/// web-config and GAMEPADII_LIVE_WRITE=1 (the user's explicit OK).
#[test]
#[ignore = "writes to the pad; needs it in web-config and GAMEPADII_LIVE_WRITE=1"]
fn set_gamepad_options_round_trip() {
    use gamepadii_core::api;
    if !opted_in("GAMEPADII_LIVE_WRITE") {
        return;
    }
    let Some(client) = webconfig() else { return };
    let backup = commands::backup(&client, None, std::time::SystemTime::now()).expect("backup");
    eprintln!("backed up to {}", backup.path.display());
    let t = client.transport();
    let before = api::gamepad_options(t).unwrap();
    let delay = before
        .extra
        .get("debounceDelay")
        .and_then(serde_json::Value::as_i64)
        .expect("debounceDelay");
    let set = |v: i64| {
        api::set_gamepad_options(t, |o| {
            o.extra.insert("debounceDelay".into(), v.into());
        })
    };
    let w = set(delay + 1).expect("write");
    assert_eq!(w.changed.len(), 1, "{w:?}");
    assert_eq!(w.changed[0].key, "debounceDelay");
    set(delay).expect("restore");
    assert_eq!(api::gamepad_options(t).unwrap(), before);
}
