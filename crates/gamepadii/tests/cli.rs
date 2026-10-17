//! Argument parsing and `--help`, with no device involved.

use clap::Parser;
use gamepadii::cli::{Cli, Command, FormatArg};
use gamepadii::output::Format;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("gamepadii").chain(args.iter().copied()))
}

fn format(args: &[&str]) -> Result<Format, String> {
    parse(args).unwrap().output_format()
}

#[test]
fn status_formats() {
    assert_eq!(format(&["status"]), Ok(Format::Human));
    assert_eq!(format(&["status", "--json"]), Ok(Format::Json));
    assert_eq!(format(&["--json", "status"]), Ok(Format::Json));
    assert_eq!(format(&["status", "--format", "json"]), Ok(Format::Json));
    assert_eq!(
        format(&["status", "--format", "waybar"]),
        Ok(Format::Waybar)
    );
    assert_eq!(
        format(&["status", "--json", "--format", "json"]),
        Ok(Format::Json)
    );
    assert!(format(&["status", "--json", "--format", "waybar"]).is_err());
    assert!(format(&["--format", "waybar"]).is_err());
}

#[test]
fn parses_status_and_sysroot() {
    let cli = parse(&["--sysroot", "/tmp/x", "status"]).unwrap();
    assert_eq!(cli.command, Some(Command::Status));
    assert_eq!(cli.format, FormatArg::Human);
    assert_eq!(cli.sysroot.as_deref(), Some(std::path::Path::new("/tmp/x")));
    assert_eq!(parse(&[]).unwrap().command, None);
}

#[test]
fn rejects_bad_arguments() {
    assert!(parse(&["status", "--format", "xml"]).is_err());
    assert!(parse(&["bogus"]).is_err());
    assert!(parse(&["status", "extra"]).is_err());
}

#[test]
fn packaging_subcommands() {
    assert!(matches!(
        parse(&["completions", "fish"]).unwrap().command,
        Some(Command::Completions { .. })
    ));
    assert!(matches!(
        parse(&["manpages", "man"]).unwrap().command,
        Some(Command::Manpages { .. })
    ));
}

/// `gamepadii [command] --help` as the user sees it.
fn help(args: &[&str]) -> String {
    let argv: Vec<&str> = args.iter().copied().chain(["--help"]).collect();
    let err = parse(&argv).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    err.render().to_string()
}

#[test]
fn help_gamepadii() {
    insta::assert_snapshot!(help(&[]));
}

#[test]
fn help_status() {
    insta::assert_snapshot!(help(&["status"]));
}
