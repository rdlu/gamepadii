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
