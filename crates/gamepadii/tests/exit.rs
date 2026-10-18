//! The binary end to end against fixtures: stdout, stderr and exit codes.

use std::process::Command;

fn gamepadii(args: &[&str]) -> (u8, String, String) {
    let home = tempfile::tempdir().unwrap();
    gamepadii_in(home.path(), args)
}

/// With `$XDG_CACHE_HOME` and `$XDG_DATA_HOME` under `home`, so the device
/// cache and backups never touch the real ones.
fn gamepadii_in(home: &std::path::Path, args: &[&str]) -> (u8, String, String) {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/sysfs/");
    let args: Vec<String> = args.iter().map(|a| a.replace("@", fixtures)).collect();
    let out = Command::new(env!("CARGO_BIN_EXE_gamepadii"))
        .args(&args)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .expect("run gamepadii");
    (
        out.status.code().expect("exit code") as u8,
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn status_found_exits_0() {
    let (code, stdout, _) = gamepadii(&["--sysroot", "@s16-xinput", "status"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("XInput (likely)\n"), "{stdout}");
}

#[test]
fn not_found_exits_2_with_an_error_object() {
    let (code, stdout, stderr) = gamepadii(&["--sysroot", "@no-pad", "status", "--json"]);
    assert_eq!(code, 2);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["error"]["kind"], "not_found");
    assert_eq!(v["error"]["exit_code"], 2);
    assert!(stderr.contains("no GP2040-CE device"), "{stderr}");
}

#[test]
fn not_found_human_prints_nothing_on_stdout() {
    let (code, stdout, _) = gamepadii(&["--sysroot", "@no-pad", "status"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
}

#[test]
fn waybar_not_found_is_one_valid_line() {
    let (code, stdout, _) = gamepadii(&["--sysroot", "@no-pad", "status", "--format", "waybar"]);
    assert_eq!(code, 2);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["class"], "disconnected");
}

#[test]
fn usage_errors_exit_3() {
    assert_eq!(gamepadii(&["bogus"]).0, 3);
    assert_eq!(gamepadii(&["status", "--json", "--format", "waybar"]).0, 3);
    assert_eq!(gamepadii(&["--format", "waybar"]).0, 3);
}

#[test]
fn no_command_off_a_terminal_prints_usage() {
    let (code, stdout, _) = gamepadii(&[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage: gamepadii"), "{stdout}");
}

#[test]
fn tui_off_a_terminal_exits_3() {
    let (code, stdout, stderr) = gamepadii(&["tui"]);
    assert_eq!(code, 3);
    assert!(stdout.is_empty());
    assert!(stderr.contains("needs a terminal"), "{stderr}");
}

#[test]
fn help_and_version_exit_0() {
    assert_eq!(gamepadii(&["--help"]).0, 0);
    let (code, stdout, _) = gamepadii(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("gamepadii "));
}

/// `--mock-api` with the S16 capture: `@` expands to the sysfs fixtures,
/// `%` to the API ones.
fn webconfig(args: &[&str]) -> (u8, String, String) {
    let api = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/api/");
    let args: Vec<String> = args.iter().map(|a| a.replace('%', api)).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    gamepadii(&args)
}

#[test]
fn info_in_web_config_exits_0() {
    let (code, stdout, _) = webconfig(&[
        "--sysroot",
        "@s16-webconfig",
        "--mock-api",
        "%s16-v0.7.12",
        "info",
        "--json",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["firmware"]["version"], "v0.7.12");
}

#[test]
fn gamepad_mode_exits_5_with_how_to_switch() {
    for cmd in [&["info"][..], &["backup"], &["reboot", "bootsel"]] {
        let mut args = vec![
            "--sysroot",
            "@s16-xinput",
            "--mock-api",
            "%s16-v0.7.12",
            "--json",
        ];
        args.extend_from_slice(cmd);
        let (code, stdout, stderr) = webconfig(&args);
        assert_eq!(code, 5, "{cmd:?}");
        let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(v["error"]["kind"], "needs_user");
        assert!(stderr.contains("XInput mode, not web-config"), "{stderr}");
        assert!(stderr.contains("S2+B3+B4 for 4 seconds"), "{stderr}");
    }
}

#[test]
fn no_pad_exits_2() {
    let (code, _, _) = webconfig(&["--sysroot", "@no-pad", "--mock-api", "%s16-v0.7.12", "info"]);
    assert_eq!(code, 2);
}

#[test]
fn a_firmware_without_the_api_exits_4() {
    let (code, _, stderr) = webconfig(&[
        "--sysroot",
        "@s16-webconfig",
        "--mock-api",
        "%nothing-here",
        "info",
    ]);
    assert_eq!(code, 4);
    assert!(stderr.contains("getFirmwareVersion: HTTP 404"), "{stderr}");
}

#[test]
fn backup_writes_the_file_it_reports() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("s16.json");
    let (code, stdout, _) = webconfig(&[
        "--sysroot",
        "@s16-webconfig",
        "--mock-api",
        "%s16-v0.7.12",
        "backup",
        "--json",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["path"], out.to_str().unwrap());
    let bundle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(bundle["schema"], 1);
    assert_eq!(bundle["endpoints"].as_object().unwrap().len(), 21);
}

#[test]
fn backup_default_path_is_under_xdg_data_home() {
    let dir = tempfile::tempdir().unwrap();
    let api = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/api/s16-v0.7.12"
    );
    let sysfs = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sysfs/s16-webconfig"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_gamepadii"))
        .args(["--sysroot", sysfs, "--mock-api", api, "backup", "--json"])
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let path = std::path::PathBuf::from(report["path"].as_str().unwrap());
    assert_eq!(path.parent().unwrap(), dir.path().join("gamepadii/backups"));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.ends_with("-Haute42COSMOX.json"), "{name}");
    assert!(path.exists());
}

#[test]
fn web_config_usage_errors_exit_3() {
    assert_eq!(gamepadii(&["--host", "https://192.168.7.1", "info"]).0, 3);
    assert_eq!(gamepadii(&["--host", "x", "--mock-api", "y", "info"]).0, 3);
    assert_eq!(gamepadii(&["reboot"]).0, 3);
    assert_eq!(gamepadii(&["reboot", "usb"]).0, 3);
    assert_eq!(gamepadii(&["info", "--format", "waybar"]).0, 3);
}

const S16_API: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/api/s16-v0.7.12"
);
const SYSFS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/sysfs/");

fn sysfs(name: &str) -> String {
    format!("{SYSFS}{name}")
}

#[test]
fn mode_with_no_argument_lists_the_choices() {
    let (code, stdout, _) = gamepadii(&["--sysroot", "@s16-xinput", "mode"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("XInput (likely)\n"), "{stdout}");
    assert!(stdout.contains("keyboard      Keyboard"), "{stdout}");
}

#[test]
fn mode_already_there_exits_0() {
    let (code, stdout, _) = gamepadii(&["--sysroot", "@s16-xinput", "mode", "xinput"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Already in XInput\n"), "{stdout}");
}

#[test]
fn mode_no_wait_exits_5_with_instructions() {
    let (code, stdout, stderr) = gamepadii(&[
        "--sysroot",
        "@s16-xinput",
        "mode",
        "keyboard",
        "--no-wait",
        "--json",
    ]);
    assert_eq!(code, 5);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["error"]["kind"], "needs_user");
    assert!(
        stderr.contains("S2+B3+B4 (Start+X+Y) for 4 seconds"),
        "{stderr}"
    );
}

#[test]
fn mode_wait_times_out_with_exit_5() {
    let (code, _, stderr) = gamepadii(&[
        "--sysroot",
        "@s16-xinput",
        "mode",
        "keyboard",
        "--timeout",
        "0",
    ]);
    assert_eq!(code, 5);
    assert!(stderr.contains("stopped waiting after 0 s"), "{stderr}");
    assert!(stderr.contains("First switch it to web-config"), "{stderr}");
}

/// In web-config with the mock server: it backs up, writes, reboots, then
/// times out, since a fixture tree never re-enumerates. The cache and the
/// backup land under the test's XDG dirs.
#[test]
fn mode_from_web_config_writes_then_waits_for_usb() {
    let home = tempfile::tempdir().unwrap();
    let (code, _, stderr) = gamepadii_in(
        home.path(),
        &[
            "--sysroot",
            &sysfs("s16-webconfig"),
            "--mock-api",
            S16_API,
            "mode",
            "keyboard",
            "--reboot-timeout",
            "0",
        ],
    );
    assert_eq!(code, 4, "{stderr}");
    assert!(stderr.contains("saving input mode Keyboard"), "{stderr}");
    assert!(
        stderr.contains("Keyboard is saved and the pad was rebooted"),
        "{stderr}"
    );
    assert!(
        home.path()
            .join("cache/gamepadii/Haute42COSMOX.json")
            .exists()
    );
    let backups: Vec<_> = std::fs::read_dir(home.path().join("data/gamepadii/backups"))
        .unwrap()
        .collect();
    assert_eq!(backups.len(), 1);
}

#[test]
fn info_saves_the_device_cache() {
    let home = tempfile::tempdir().unwrap();
    let (code, _, _) = gamepadii_in(
        home.path(),
        &[
            "--sysroot",
            &sysfs("s16-webconfig"),
            "--mock-api",
            S16_API,
            "info",
        ],
    );
    assert_eq!(code, 0);
    let cache = home.path().join("cache/gamepadii/Haute42COSMOX.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cache).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["gamepad_options"]["profileNumber"], 2);
    // Then, in XInput, mode names the boot buttons from it.
    let (code, stdout, _) = gamepadii_in(home.path(), &["--sysroot", &sysfs("s16-xinput"), "mode"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("Keyboard (boot button R2 (RT))"),
        "{stdout}"
    );
    let (code, _, stderr) = gamepadii_in(
        home.path(),
        &[
            "--sysroot",
            &sysfs("s16-xinput"),
            "mode",
            "keyboard",
            "--no-wait",
        ],
    );
    assert_eq!(code, 5);
    assert!(
        stderr.contains("hold R2 (RT) (only that button)"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Default calls S2 (Start), B3 (X) and R1 (RB)"),
        "{stderr}"
    );
}

#[test]
fn webconfig_prints_the_url_or_needs_the_user() {
    let (code, stdout, _) = gamepadii(&[
        "--sysroot",
        &sysfs("s16-webconfig"),
        "--mock-api",
        S16_API,
        "webconfig",
        "--json",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["url"], "http://192.168.7.1/");
    let (code, _, stderr) = gamepadii(&["--sysroot", "@s16-xinput", "webconfig", "--no-wait"]);
    assert_eq!(code, 5);
    assert!(stderr.contains("To switch it to web-config"), "{stderr}");
}

#[test]
fn reboot_waits_for_usb() {
    let args = |extra: &[&str]| {
        let mut a = vec![
            "--sysroot".to_string(),
            sysfs("s16-webconfig"),
            "--mock-api".into(),
            S16_API.into(),
            "reboot".into(),
            "gamepad".into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    };
    let run = |extra: &[&str]| {
        let a = args(extra);
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        gamepadii(&a)
    };
    let (code, stdout, _) = run(&["--no-wait"]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("Rebooting into gamepad mode"),
        "{stdout}"
    );
    // A fixture tree never re-enumerates: the wait times out.
    let (code, _, stderr) = run(&["--timeout", "0"]);
    assert_eq!(code, 4);
    assert!(
        stderr.contains("hasn't come back on USB after 0 s"),
        "{stderr}"
    );
}

// ---- the control channel, against --mock-channel ---------------------------

fn cc(args: &[&str]) -> (u8, String, String) {
    let mut a = vec!["--mock-api", "%s16-v0.7.12"];
    a.extend_from_slice(args);
    webconfig(&a)
}

#[test]
fn status_with_the_channel_is_confirmed() {
    let (code, stdout, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "status",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("XInput (confirmed)\n"), "{stdout}");
    assert!(stdout.contains("profile       2 SilkSong"), "{stdout}");
    let (_, stdout, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "status",
        "--json",
    ]);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["devices"][0]["confidence"], "confirmed");
    assert_eq!(
        v["devices"][0]["control_channel"]["firmware"]["version"],
        "v0.7.12"
    );
    // Stock firmware: as before, with explicit nulls.
    let (code, stdout, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "stock",
        "status",
        "--json",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["devices"][0]["confidence"], "likely");
    assert!(v["devices"][0]["control_channel"].is_null());
}

#[test]
fn a_fixture_sysroot_never_reaches_a_real_node() {
    // No --mock-channel: usbfs under the fixture root finds no dev/.
    let (code, stdout, _) = gamepadii(&["--sysroot", "@s16-xinput", "status", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["devices"][0]["control_channel"].is_null());
}

#[test]
fn profile_switch_exit_codes() {
    let (code, stdout, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "profile",
        "switch",
        "celeste",
    ]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("Switched to profile 3 Celeste\n"),
        "{stdout}"
    );
    let (code, _, stderr) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "stock",
        "profile",
        "switch",
        "3",
    ]);
    assert_eq!(code, 5);
    assert!(stderr.contains("LOAD_PROFILE_n"), "{stderr}");
    let (code, stdout, stderr) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "no-access",
        "--json",
        "profile",
        "switch",
        "3",
    ]);
    assert_eq!(code, 5);
    assert!(stderr.contains("gamepadii setup udev"), "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["error"]["kind"], "needs_user");
    let (code, _, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "profile",
        "switch",
        "Hades",
    ]);
    assert_eq!(code, 3);
    let (code, _, stderr) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--no-channel",
        "profile",
        "switch",
        "2",
    ]);
    assert_eq!(code, 5);
    assert!(stderr.contains("--no-channel"), "{stderr}");
}

#[test]
fn reboot_and_mode_over_the_channel() {
    // A static fixture never re-enumerates, so only --no-wait succeeds.
    let (code, stdout, _) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "reboot",
        "webconfig",
        "--no-wait",
    ]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("via           the control channel (/dev/bus/usb/"),
        "{stdout}"
    );
    let (code, _, stderr) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "mode",
        "keyboard",
        "--reboot-timeout",
        "0",
    ]);
    assert_eq!(
        code, 4,
        "the command went out, the pad didn't come back: {stderr}"
    );
    assert!(stderr.contains("hasn't come back on USB"), "{stderr}");
    let (code, _, stderr) = cc(&[
        "--sysroot",
        "@s16-xinput",
        "--mock-channel",
        "channel",
        "mode",
        "sinput",
    ]);
    assert_eq!(code, 3, "{stderr}");
}

#[test]
fn setup_udev_prints_explains_and_installs_into_a_root() {
    let (code, stdout, _) = gamepadii(&["setup", "udev", "--print"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, gamepadii_core::udev::rule());
    let root = tempfile::tempdir().unwrap();
    let r = root.path().to_str().unwrap();
    let (code, stdout, _) = gamepadii(&["--sysroot", r, "setup", "udev"]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with("udev rule for the control channel: not installed\n"),
        "{stdout}"
    );
    assert!(stdout.contains("045e:028e"), "{stdout}");
    let (code, stdout, _) = gamepadii(&["--sysroot", r, "setup", "udev", "--install", "--json"]);
    assert_eq!(code, 0, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["action"], "installed");
    assert_eq!(v["installed"], "current");
    let file = root.path().join("etc/udev/rules.d/70-gamepadii.rules");
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        gamepadii_core::udev::rule()
    );
    let (_, stdout, _) = gamepadii(&["--sysroot", r, "setup", "udev", "--install", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["action"], "none");
}

#[test]
fn display_layout_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut a = vec!["--mock-api", S16_API, "--sysroot"];
        a.extend_from_slice(args);
        gamepadii_in(home.path(), &a)
    };
    let wc = sysfs("s16-webconfig");
    // No layout: the current one and the choices, nothing written.
    let (code, stdout, _) = run(&[&wc, "display", "layout"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Board Defined (27/31)"), "{stdout}");
    assert!(stdout.contains("alt2"), "{stdout}");
    // s16: backed up, written, read back.
    let (code, stdout, stderr) = run(&[&wc, "display", "layout", "s16", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["after"]["left"], 36);
    assert_eq!(v["after"]["right"], 42);
    assert_eq!(v["write"]["posted"], true);
    let backup = v["backup"].as_str().unwrap();
    assert!(
        backup.starts_with(home.path().to_str().unwrap()),
        "{backup}"
    );
    assert!(std::path::Path::new(backup).exists());
    // --no-backup: none taken.
    let (code, stdout, _) = run(&[&wc, "display", "layout", "g16", "--no-backup", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["backup"].is_null());
    // A typo is a usage error before anything is read.
    let (code, _, stderr) = run(&[
        &sysfs("s16-xinput"),
        "--mock-channel",
        "channel",
        "display",
        "layout",
        "s61",
    ]);
    assert_eq!(code, 3);
    assert!(
        stderr.contains("no Haute42COSMOX layout called \"s61\""),
        "{stderr}"
    );
    assert!(!stderr.contains("rebooting"), "{stderr}");
    let (code, _, _) = run(&[&wc, "display", "layout", "--left", "99", "--right", "1"]);
    assert_eq!(code, 3);
    // In a gamepad mode without the channel: exit 5, as info.
    let (code, _, stderr) = run(&[&sysfs("s16-xinput"), "--no-channel", "display", "layout"]);
    assert_eq!(code, 5);
    assert!(stderr.contains("XInput mode, not web-config"), "{stderr}");
}

#[test]
fn info_and_backup_from_a_gamepad_mode_use_the_channel() {
    // The channel takes REBOOT, but a static fixture never shows up in
    // web-config: nothing is read, and the message says where things are.
    for cmd in ["info", "backup"] {
        let (code, stdout, stderr) = cc(&[
            "--sysroot",
            "@s16-xinput",
            "--mock-channel",
            "channel",
            "--json",
            cmd,
            "--timeout",
            "0",
        ]);
        assert_eq!(code, 4, "{cmd}: {stderr}");
        assert!(stderr.contains("rebooting into web-config"), "{stderr}");
        assert!(stderr.contains("asked for web-config"), "{stderr}");
        assert!(stderr.contains("nothing was read"), "{stderr}");
        let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(v["error"]["kind"], "io");
    }
    // Without the channel: exit 5 with how to switch, as before.
    for kind in [&["--mock-channel", "stock"][..], &["--no-channel"]] {
        let mut args = vec!["--sysroot", "@s16-xinput"];
        args.extend_from_slice(kind);
        args.push("info");
        let (code, _, stderr) = cc(&args);
        assert_eq!(code, 5, "{kind:?}");
        assert!(stderr.contains("XInput mode, not web-config"), "{stderr}");
    }
    // In web-config the JSON says there was no trip.
    let (code, stdout, _) = cc(&["--sysroot", "@s16-webconfig", "info", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["trip"].is_null());
}

#[test]
fn profile_switch_keeps_the_cache_and_the_hints_current() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut a = vec!["--mock-api", S16_API];
        a.extend_from_slice(args);
        gamepadii_in(home.path(), &a)
    };
    let xinput = sysfs("s16-xinput");
    assert_eq!(run(&["--sysroot", &sysfs("s16-webconfig"), "info"]).0, 0);
    let (code, _, stderr) = run(&[
        "--sysroot",
        &xinput,
        "--mock-channel",
        "channel",
        "profile",
        "switch",
        "default",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let cache = home.path().join("cache/gamepadii/Haute42COSMOX.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    assert_eq!(v["running_profile"]["number"], 1);
    assert_eq!(v["gamepad_options"]["profileNumber"], 2, "RAM only");
    // Later, without the channel, the hints are in Default, where nothing
    // is moved (SilkSong, the saved profile, puts B4 on R1).
    let (code, _, stderr) = run(&[
        "--sysroot",
        &xinput,
        "--no-channel",
        "mode",
        "keyboard",
        "--no-wait",
    ]);
    assert_eq!(code, 5);
    assert!(!stderr.contains("R1 (RB)"), "{stderr}");
    assert!(stderr.contains("last reported running"), "{stderr}");
}

#[test]
fn leds_exit_codes() {
    let run = |kind: &str, args: &[&str]| {
        let mut a = vec!["--sysroot", "@s16-xinput", "--mock-channel", kind];
        a.extend_from_slice(args);
        cc(&a)
    };
    let (code, stdout, _) = run("leds", &["leds"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("animation     custom-theme (4)"),
        "{stdout}"
    );
    let (code, stdout, _) = run("leds", &["leds", "--format", "waybar"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["class"], "custom-theme");
    let (code, stdout, _) = run(
        "leds",
        &[
            "--json",
            "leds",
            "set",
            "--animation",
            "chase",
            "--brightness",
            "up",
        ],
    );
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["changed"], serde_json::json!(["animation", "brightness"]));
    assert_eq!(v["leds"]["brightness"], 2);
    assert_eq!(v["saved"], false);
    let (code, stdout, _) = run("leds", &["leds", "brightness", "down", "--save"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("LEDs changed and saved\n"), "{stdout}");
    // A value the pad doesn't have: 3.
    let (code, _, stderr) = run("leds", &["leds", "set", "--static-color", "mauve"]);
    assert_eq!(code, 3);
    assert!(stderr.contains("no colour \"mauve\""), "{stderr}");
    let (code, _, _) = run("leds", &["leds", "brightness", "9"]);
    assert_eq!(code, 3);
    // No LED commands (v1.0, stock), no permission: 5, with the JSON error.
    for kind in ["channel", "stock", "no-access"] {
        let (code, stdout, stderr) = run(kind, &["--json", "leds", "set", "--brightness", "1"]);
        assert_eq!(code, 5, "{kind}: {stderr}");
        let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(v["error"]["kind"], "needs_user");
    }
    // waybar on a v1.0 pad: one valid line, exit 5, no trip.
    let (code, stdout, _) = run("channel", &["leds", "--format", "waybar"]);
    assert_eq!(code, 5);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["class"], "disconnected");
    // Stock in a gamepad mode: switch to web-config first.
    let (code, _, stderr) = run("stock", &["leds"]);
    assert_eq!(code, 5);
    assert!(stderr.contains("not web-config"), "{stderr}");
    // In web-config on stock: what it can tell.
    let (code, stdout, _) = webconfig(&[
        "--sysroot",
        "@s16-webconfig",
        "--mock-channel",
        "stock",
        "--mock-api",
        "%s16-v0.7.12",
        "leds",
        "--json",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["animation"].is_null() && v["brightness"].is_null());
    assert_eq!(v["brightness_steps"], 5);
    assert_eq!(v["via"], "web_config");
    assert_eq!(v["custom_theme_colors"][4]["button"], "B1");
}

#[test]
fn lights_exit_codes() {
    // The fork's main line (v1.2): the lights commands.
    let run = |args: &[&str]| {
        let mut a = vec![
            "--sysroot",
            "@s16-xinput",
            "--mock-channel",
            "lights",
            "--mock-api",
            "%s16-main",
        ];
        a.extend_from_slice(args);
        webconfig(&a)
    };
    let (code, stdout, _) = run(&["leds"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("lights        profile 0"), "{stdout}");
    let (code, stdout, _) = run(&[
        "--json",
        "leds",
        "set",
        "--lights-profile",
        "off",
        "--brightness",
        "up",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        v["changed"],
        serde_json::json!(["lights_profile", "brightness"])
    );
    assert_eq!(v["leds"]["lights_off"], true);
    assert_eq!(v["leds"]["brightness"], 6);
    // The pad keeps its state for the whole process only, so a new run
    // starts from the fixture again.
    let (code, stdout, _) = run(&["leds", "--format", "waybar"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["class"], "lights-0");
    // A v0.7.12 option on main, an unknown lights profile: 3.
    let (code, _, stderr) = run(&["leds", "set", "--animation", "chase"]);
    assert_eq!(code, 3);
    assert!(
        stderr.contains("main firmware has lights profiles"),
        "{stderr}"
    );
    let (code, _, stderr) = run(&["leds", "set", "--lights-profile", "7"]);
    assert_eq!(code, 3);
    assert!(stderr.contains("is not on this pad"), "{stderr}");
    let (code, _, stderr) = run(&["leds", "set", "--lights-profile", "dim"]);
    assert_eq!(code, 3, "{stderr}");
}
