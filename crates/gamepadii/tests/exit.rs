//! The binary end to end against fixtures: stdout, stderr and exit codes.

use std::process::Command;

fn gamepadii(args: &[&str]) -> (u8, String, String) {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/sysfs/");
    let args: Vec<String> = args.iter().map(|a| a.replace("@", fixtures)).collect();
    let out = Command::new(env!("CARGO_BIN_EXE_gamepadii"))
        .args(&args)
        .env("NO_COLOR", "1")
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
