//! Argument parsing and `--help`, with no device involved.

use clap::Parser;
use gamepadii::cli::{Cli, Command, FormatArg, TripArgs};
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
fn parses_web_config_commands() {
    use gamepadii::cli::RebootArg;
    let cli = parse(&["--host", "10.0.0.2", "info"]).unwrap();
    let back = TripArgs {
        stay: false,
        timeout: 10,
    };
    assert_eq!(cli.command, Some(Command::Info { trip: back.clone() }));
    assert_eq!(cli.host.as_deref(), Some("10.0.0.2"));
    assert_eq!(
        parse(&["backup", "-o", "x.json"]).unwrap().command,
        Some(Command::Backup {
            output: Some("x.json".into()),
            trip: back.clone()
        })
    );
    assert_eq!(
        parse(&["backup", "--stay", "--timeout", "30"])
            .unwrap()
            .command,
        Some(Command::Backup {
            output: None,
            trip: TripArgs {
                stay: true,
                timeout: 30
            }
        })
    );
    assert_eq!(
        parse(&["info", "--stay"]).unwrap().command,
        Some(Command::Info {
            trip: TripArgs {
                stay: true,
                timeout: 10
            }
        })
    );
    for (arg, mode) in [
        ("gamepad", RebootArg::Gamepad),
        ("webconfig", RebootArg::Webconfig),
        ("web-config", RebootArg::Webconfig),
        ("bootsel", RebootArg::Bootsel),
    ] {
        assert_eq!(
            parse(&["reboot", arg]).unwrap().command,
            Some(Command::Reboot {
                mode,
                no_wait: false,
                timeout: 10
            })
        );
    }
    assert!(parse(&["reboot"]).is_err());
    assert!(format(&["info", "--format", "waybar"]).is_err());
    assert_eq!(format(&["backup", "--json"]), Ok(Format::Json));
}

#[test]
fn parses_mode_and_webconfig() {
    use gamepadii_core::mode::InputMode;
    let mode = |args: &[&str]| match parse(args).unwrap().command {
        Some(Command::Mode { mode, .. }) => mode,
        other => panic!("{other:?}"),
    };
    assert_eq!(mode(&["mode"]), None);
    assert_eq!(mode(&["mode", "keyboard"]), Some(InputMode::Keyboard));
    assert_eq!(mode(&["mode", "hid"]), Some(InputMode::Generic));
    assert_eq!(mode(&["mode", "switch-pro"]), Some(InputMode::SwitchPro));
    assert_eq!(mode(&["mode", "switch_pro"]), Some(InputMode::SwitchPro));
    assert!(parse(&["mode", "webconfig"]).is_err());
    assert!(parse(&["mode", "bootsel"]).is_err());
    assert!(parse(&["mode", "xbox"]).is_err());
    assert_eq!(
        parse(&["mode", "ps5", "--no-wait", "--timeout", "5", "--no-backup"])
            .unwrap()
            .command,
        Some(Command::Mode {
            mode: Some(InputMode::Ps5),
            no_wait: true,
            timeout: 5,
            reboot_timeout: 10,
            no_backup: true,
        })
    );
    assert_eq!(
        parse(&["webconfig", "--open"]).unwrap().command,
        Some(Command::Webconfig {
            open: true,
            exit: false,
            no_wait: false,
            timeout: None,
        })
    );
    assert!(parse(&["webconfig", "--open", "--exit"]).is_err());
    assert!(format(&["mode", "--format", "waybar"]).is_err());
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

#[test]
fn help_info() {
    insta::assert_snapshot!(help(&["info"]));
}

#[test]
fn help_backup() {
    insta::assert_snapshot!(help(&["backup"]));
}

#[test]
fn help_reboot() {
    insta::assert_snapshot!(help(&["reboot"]));
}

#[test]
fn help_mode() {
    insta::assert_snapshot!(help(&["mode"]));
}

#[test]
fn help_webconfig() {
    insta::assert_snapshot!(help(&["webconfig"]));
}

#[test]
fn help_profile_switch() {
    insta::assert_snapshot!(help(&["profile", "switch"]));
}

#[test]
fn help_leds() {
    insta::assert_snapshot!(help(&["leds"]));
}

#[test]
fn help_leds_set() {
    insta::assert_snapshot!(help(&["leds", "set"]));
}

#[test]
fn parses_leds() {
    use gamepadii::cli::LedsCommand;
    use gamepadii_core::leds::{LedChange, Level, Pick};
    let change = |args: &[&str]| match parse(args).unwrap().command {
        Some(Command::Leds {
            command: Some(c), ..
        }) => c.change().expect("a change"),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        change(&["leds", "set", "--animation", "rainbow", "--brightness", "3"]),
        LedChange {
            animation: Some(Pick::Name("rainbow".into())),
            brightness: Some(Level::To(3)),
            ..LedChange::default()
        }
    );
    assert_eq!(
        change(&[
            "leds",
            "set",
            "--static-color",
            "2",
            "--theme",
            "xbox",
            "--save"
        ]),
        LedChange {
            static_color: Some(Pick::Number(2)),
            theme: Some(Pick::Name("xbox".into())),
            save: true,
            ..LedChange::default()
        }
    );
    assert_eq!(
        change(&["leds", "brightness", "down"]).brightness,
        Some(Level::Down)
    );
    assert!(parse(&["leds", "brightness", "dim"]).is_err());
    assert!(parse(&["leds", "set", "--theme", "999"]).is_err());
    assert!(matches!(
        parse(&["leds", "--stay"]).unwrap().command,
        Some(Command::Leds {
            command: None,
            trip: TripArgs { stay: true, .. }
        })
    ));
    let _ = LedsCommand::Brightness {
        level: Level::Up,
        save: false,
    };
    // waybar: the read only.
    assert_eq!(format(&["leds", "--format", "waybar"]), Ok(Format::Waybar));
    assert!(format(&["leds", "set", "--save", "--format", "waybar"]).is_err());
}

#[test]
fn help_setup_udev() {
    insta::assert_snapshot!(help(&["setup", "udev"]));
}

#[test]
fn help_display_layout() {
    insta::assert_snapshot!(help(&["display", "layout"]));
}

#[test]
fn parses_display_layout() {
    use gamepadii::cli::DisplayCommand;
    use gamepadii_core::display::LayoutArg;
    let arg = |args: &[&str]| match parse(args).unwrap().command {
        Some(Command::Display { command }) => command.layout_arg(),
        other => panic!("{other:?}"),
    };
    assert_eq!(arg(&["display", "layout"]), None);
    assert_eq!(
        arg(&["display", "layout", "S16"]),
        Some(LayoutArg::Named("S16".into()))
    );
    assert_eq!(
        arg(&["display", "layout", "--left", "36", "--right", "42"]),
        Some(LayoutArg::Raw {
            left: 36,
            right: 42
        })
    );
    assert!(parse(&["display", "layout", "--left", "36"]).is_err());
    assert!(parse(&["display", "layout", "s16", "--left", "36", "--right", "42"]).is_err());
    let Some(Command::Display {
        command: DisplayCommand::Layout {
            no_backup, trip, ..
        },
    }) = parse(&["display", "layout", "s16", "--no-backup", "--stay"])
        .unwrap()
        .command
    else {
        panic!()
    };
    assert!(no_backup && trip.stay);
}

#[test]
fn parses_profile_and_setup() {
    use gamepadii::cli::{ProfileCommand, SetupCommand};
    use gamepadii_core::flow::ProfileArg;
    let switch = |args: &[&str]| match parse(args).unwrap().command {
        Some(Command::Profile {
            command: ProfileCommand::Switch { profile, save },
        }) => (profile, save),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        switch(&["profile", "switch", "3"]),
        (ProfileArg::Number(3), false)
    );
    assert_eq!(
        switch(&["profile", "switch", "SilkSong", "--save"]),
        (ProfileArg::Label("SilkSong".into()), true)
    );
    assert!(parse(&["profile", "switch", "7"]).is_err());
    assert!(parse(&["profile", "switch"]).is_err());
    assert_eq!(
        parse(&["setup", "udev", "--print"]).unwrap().command,
        Some(Command::Setup {
            command: SetupCommand::Udev {
                print: true,
                install: false
            }
        })
    );
    assert!(parse(&["setup", "udev", "--print", "--install"]).is_err());
    assert!(parse(&["--no-channel", "status"]).unwrap().no_channel);
    assert!(parse(&["--no-channel", "--mock-channel", "stock", "status"]).is_err());
}
