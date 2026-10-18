use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use clap::{CommandFactory, Parser};
use gamepadii::cli::{
    self, Cli, ColorsCommand, Command, DisplayCommand, InputCommand, LedsCommand, LibraryCommand,
    LightsProfileCommand, MockChannel, ProfileCommand, SetupCommand, ThemeCommand, TripArgs,
};
use gamepadii::output::{self, Format, Style};
use gamepadii::progress::Reporter;
use gamepadii::tui::worker::WithEnv;
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::channel::sim::{SimChannel, SimCustomColors, SimInput, Standalone};
use gamepadii_core::channel::{ControlTransport, OpenControl, Rgb, UsbError};
use gamepadii_core::client::{self, Client};
use gamepadii_core::colors::{ColorsFile, ColorsWrite, ThemeApply, ThemeFile};
use gamepadii_core::discovery::Device;
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Env, Timeouts};
use gamepadii_core::library::{self, Library};
use gamepadii_core::lights::{LightsFile, LightsWrite};
use gamepadii_core::mode::BootMode;
use gamepadii_core::slots::{SlotFile, SlotWrite};
use gamepadii_core::sysfs::Root;
use gamepadii_core::transport::http::{self, Http};
use gamepadii_core::transport::mock::Mock;
use gamepadii_core::wait::{Poll, SystemClock};
use gamepadii_core::{Error, commands, display, udev};

fn main() -> ExitCode {
    cli::complete_env();
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // --help / --version are not errors; everything else is exit 3
            // (clap's own 2 would read as "device not found").
            let code = if e.use_stderr() {
                exit::USAGE
            } else {
                exit::OK
            };
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    let format = match cli.output_format() {
        Ok(f) => f,
        Err(msg) => {
            let _ = Cli::command()
                .error(clap::error::ErrorKind::ArgumentConflict, msg)
                .print();
            return ExitCode::from(exit::USAGE);
        }
    };
    let root = cli.sysroot.clone().map_or_else(Root::live, Root::new);
    let style = Style::detect(format);
    let control = control_opener(&cli, &root);
    let control = control.as_deref();
    match cli.command.clone() {
        None if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() => tui(&cli),
        None => {
            // Not a terminal (a pipe, a script): usage, as before the TUI.
            let _ = Cli::command().print_help();
            ExitCode::from(exit::OK)
        }
        Some(Command::Tui) => tui(&cli),
        Some(Command::Status) => finish(
            format,
            commands::status_with(&root, control, cache::cache_dir().as_deref())
                .map(|s| output::status(style, &s)),
        ),
        Some(Command::Info { trip }) => {
            // --host: the web configurator as it is; otherwise the pad on
            // USB, through web-config and back when it is in a gamepad mode.
            let result = match &cli.host {
                Some(_) => connect(&cli, &root).and_then(|c| {
                    let (info, cache) = commands::info_with_cache(&c, SystemTime::now())?;
                    save_cache(&cache);
                    Ok((info, None))
                }),
                None => on_usb(&cli, &root, control, format, &trip, |env, progress| {
                    flow::info(env, trip.stay, progress)
                }),
            }
            .map(|(i, t)| output::info(style, &i, t.as_ref()));
            finish(format, result)
        }
        Some(Command::Backup {
            output: path,
            channel_only: true,
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result = flow::backup_channel(&env, path.as_deref())
                .map(|b| output::backup(style, &b, None));
            finish(format, result)
        }
        Some(Command::Backup {
            output: path, trip, ..
        }) => {
            let result = match &cli.host {
                Some(_) => connect(&cli, &root).and_then(|c| {
                    let (report, cache) =
                        commands::backup_with_cache(&c, path.as_deref(), SystemTime::now(), None)?;
                    if let Some(cache) = cache {
                        save_cache(&cache);
                    }
                    Ok((report, None))
                }),
                None => on_usb(&cli, &root, control, format, &trip, |env, progress| {
                    flow::backup(env, path.as_deref(), trip.stay, progress)
                }),
            }
            .map(|(b, t)| output::backup(style, &b, t.as_ref()));
            finish(format, result)
        }
        Some(Command::Restore {
            file,
            no_backup,
            timeout,
        }) => {
            let timeouts = Timeouts {
                reboot: Duration::from_secs(timeout),
                ..Timeouts::default()
            };
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(&cli, &root, &clock, &open, control, timeouts, !no_backup);
            let mut progress = reporter(format);
            // A missing backups dir fails here, before any write.
            let result = match no_backup {
                true => Ok(()),
                false => commands::backups_dir().map(|_| ()),
            }
            .and_then(|()| read_file(&file))
            .and_then(|t| commands::parse_backup(&t, &file))
            .and_then(|b| flow::restore(&env, &file, &b, &mut progress))
            .map(|r| output::restore(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Reboot {
            mode,
            no_wait,
            timeout,
        }) => {
            let timeouts = Timeouts {
                reboot: Duration::from_secs(timeout),
                ..Timeouts::default()
            };
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(&cli, &root, &clock, &open, control, timeouts, false);
            let mut progress = reporter(format);
            // --host: the web configurator, wherever it is; otherwise the
            // pad on USB, over HTTP or the control channel.
            let result = match &cli.host {
                Some(_) => connect(&cli, &root)
                    .and_then(|c| flow::reboot(&env, &c, mode.into(), !no_wait, &mut progress)),
                None => flow::reboot_pad(&env, mode.into(), !no_wait, &mut progress),
            }
            .map(|r| output::reboot(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Mode {
            mode,
            no_wait,
            timeout,
            reboot_timeout,
            no_backup,
        }) => {
            let timeouts = Timeouts {
                user: Duration::from_secs(timeout),
                reboot: Duration::from_secs(reboot_timeout),
                ..Timeouts::default()
            };
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(&cli, &root, &clock, &open, control, timeouts, !no_backup);
            let mut progress = reporter(format);
            let result =
                match mode {
                    None => flow::mode_status(&env, &mut progress)
                        .map(|s| output::mode_status(style, &s)),
                    Some(m) => flow::mode(&env, m, !no_wait, &mut progress)
                        .map(|r| output::mode(style, &r)),
                };
            drop(progress);
            finish(format, result)
        }
        Some(Command::Webconfig {
            open: open_browser,
            exit: leave,
            no_wait,
            timeout,
        }) => {
            let mut timeouts = Timeouts::default();
            if let Some(t) = timeout.map(Duration::from_secs) {
                if leave {
                    timeouts.reboot = t;
                } else {
                    timeouts.user = t;
                }
            }
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(&cli, &root, &clock, &open, control, timeouts, false);
            let mut progress = reporter(format);
            let result = if leave {
                connect(&cli, &root)
                    .and_then(|c| {
                        flow::reboot(&env, &c, BootMode::Gamepad, !no_wait, &mut progress)
                    })
                    .map(|r| output::reboot(style, &r))
            } else {
                flow::webconfig(&env, !no_wait, &mut progress).map(|r| {
                    if open_browser {
                        xdg_open(&r.url);
                    }
                    output::webconfig(style, &r)
                })
            };
            drop(progress);
            finish(format, result)
        }
        Some(Command::Profile {
            command: ProfileCommand::Switch { profile, save },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let mut progress = reporter(format);
            let result = flow::profile_switch(&env, &profile, save, &mut progress)
                .map(|r| output::profile_switch(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Profile {
            command: ProfileCommand::Show { profile },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result =
                flow::profile_show(&env, profile.as_ref()).map(|r| output::profile_show(style, &r));
            finish(format, result)
        }
        Some(Command::Profile {
            command:
                ProfileCommand::Write {
                    file,
                    slot,
                    select,
                    save,
                    label,
                    no_backup,
                },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                !no_backup,
            );
            let write = SlotWrite {
                slot,
                select,
                save,
                label,
            };
            let mut progress = reporter(format);
            let result = read_slot_file(&file)
                .and_then(|f| flow::profile_write(&env, &f, &write, &mut progress))
                .map(|r| output::profile_write(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Library {
            command: LibraryCommand::List,
        }) => {
            let dir = library::library_dir();
            let result =
                Library::load(dir.as_deref()).map(|l| output::library_list(style, &l.list()));
            finish(format, result)
        }
        Some(Command::Library {
            command: LibraryCommand::Show { name, slot_file },
        }) => {
            let dir = library::library_dir();
            let result = Library::load(dir.as_deref())
                .and_then(|l| l.resolve(&name))
                .map(|r| match slot_file {
                    // A file: JSON in any format, with an end of line.
                    true if format == Format::Human => format!("{}\n", output::slot_file(&r)),
                    true => output::slot_file(&r),
                    false => output::library_show(style, &r),
                });
            finish(format, result)
        }
        Some(Command::Library {
            command: LibraryCommand::Pull { write, force },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result = flow::library_pull(&env).and_then(|mut r| {
                if let Some(path) = write {
                    let path = path.unwrap_or_else(|| format!("{}.kdl", r.board).into());
                    let dir = library::library_dir();
                    library::save_pull(&mut r, dir.as_deref(), &path, force)?;
                }
                Ok(output::library_pull(style, &r))
            });
            finish(format, result)
        }
        Some(Command::Library {
            command: LibraryCommand::Diff { name, slot },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let dir = library::library_dir();
            let result = Library::load(dir.as_deref())
                .and_then(|l| l.resolve(&name))
                .and_then(|p| flow::library_diff(&env, &p, slot))
                .map(|r| {
                    // As diff(1): 1 when there is a difference.
                    let code = if r.changed {
                        exit::CHECK_FAILED
                    } else {
                        exit::OK
                    };
                    (output::library_diff(style, &r), code)
                });
            finish_with(format, result)
        }
        Some(Command::Library {
            command:
                LibraryCommand::Apply {
                    name,
                    slot,
                    select,
                    save,
                    no_backup,
                },
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                !no_backup,
            );
            let apply = library::diff::Apply { slot, select, save };
            let dir = library::library_dir();
            let mut progress = reporter(format);
            let result = Library::load(dir.as_deref())
                .and_then(|l| l.resolve(&name))
                .and_then(|p| flow::library_apply(&env, &p, &apply, &mut progress))
                .map(|r| output::library_apply(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Leds {
            command: None,
            trip,
        }) => {
            let result = if format == Format::Waybar {
                // A status bar polls: never a trip through web-config.
                let clock = SystemClock::new();
                let open = || open(&cli);
                let env = env(
                    &cli,
                    &root,
                    &clock,
                    &open,
                    control,
                    Timeouts::default(),
                    false,
                );
                flow::leds_now(&env).map(|r| output::waybar_leds(&r).to_line())
            } else {
                match &cli.host {
                    Some(_) => connect(&cli, &root)
                        .and_then(|c| gamepadii_core::leds::read_webconfig(&c))
                        .map(|r| (r, None)),
                    None => on_usb(&cli, &root, control, format, &trip, |env, progress| {
                        flow::leds(env, trip.stay, progress)
                    }),
                }
                .map(|(r, t)| output::leds(style, &r, t.as_ref()))
            };
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Profile {
                    command: LightsProfileCommand::Show { profile },
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result = flow::lights_show(&env, profile).map(|r| output::lights_show(style, &r));
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Profile {
                    command:
                        LightsProfileCommand::Write {
                            file,
                            profile,
                            select,
                            save,
                            no_backup,
                        },
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                !no_backup,
            );
            let write = LightsWrite {
                profile,
                select,
                save,
            };
            let mut progress = reporter(format);
            let result = read_file(&file)
                .and_then(|t| LightsFile::parse(&t))
                .and_then(|f| flow::lights_write(&env, &f, &write, &mut progress))
                .map(|r| output::lights_write(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Colors {
                    command: None | Some(ColorsCommand::Show),
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result = flow::colors_show(&env).map(|r| output::colors_show(style, &r));
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Colors {
                    command:
                        Some(ColorsCommand::Write {
                            file,
                            save,
                            no_backup,
                        }),
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                !no_backup,
            );
            let mut progress = reporter(format);
            let result = read_file(&file)
                .and_then(|t| ColorsFile::parse(&t))
                .and_then(|f| flow::colors_write(&env, &f, &ColorsWrite { save }, &mut progress))
                .map(|r| output::colors_write(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Theme {
                    command: ThemeCommand::Show { profile },
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let result = flow::theme_show(&env, profile).map(|r| output::theme_show(style, &r));
            finish(format, result)
        }
        Some(Command::Leds {
            command:
                Some(LedsCommand::Theme {
                    command:
                        ThemeCommand::Apply {
                            file,
                            profile,
                            save,
                            no_backup,
                        },
                }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                !no_backup,
            );
            let apply = ThemeApply { profile, save };
            let mut progress = reporter(format);
            let result = read_file(&file)
                .and_then(|t| ThemeFile::parse(&t))
                .and_then(|t| flow::theme_apply(&env, &t, &apply, &mut progress))
                .map(|r| output::theme_apply(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Leds {
            command: Some(command),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let change = command.change().unwrap_or_default();
            let result = flow::leds_set(&env, &change).map(|r| output::leds_set(style, &r));
            finish(format, result)
        }
        Some(Command::Input {
            watch,
            reads,
            command: None,
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            if watch {
                return watch_input(&env, style, reads);
            }
            finish(format, flow::input(&env).map(|r| output::input(style, &r)))
        }
        Some(Command::Input {
            command: Some(InputCommand::FindPin { scan, timeout }),
            ..
        }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            let find = flow::FindPin {
                scan,
                timeout: Duration::from_secs(timeout),
                // Ctrl-C must not leave the pin scan on.
                stop: scan.then(ctrl_c::catch),
            };
            let mut progress = reporter(format);
            let result =
                flow::find_pin(&env, &find, &mut progress).map(|r| output::find_pin(style, &r));
            drop(progress);
            finish(format, result)
        }
        Some(Command::Log { from, follow }) => {
            let clock = SystemClock::new();
            let open = || open(&cli);
            let env = env(
                &cli,
                &root,
                &clock,
                &open,
                control,
                Timeouts::default(),
                false,
            );
            if !follow {
                let result = flow::log(&env, from).map(|r| output::log(style, &r));
                return finish(format, result);
            }
            let mut first = true;
            let result = flow::log_follow(&env, from, &mut |r| {
                let text = output::log_follow(style, r, first);
                first = false;
                let mut out = std::io::stdout().lock();
                // A closed pipe (`| head`) ends it.
                out.write_all(text.as_bytes())
                    .and_then(|()| out.flush())
                    .is_ok()
            });
            finish(format, result.map(|()| String::new()))
        }
        Some(Command::Display { command }) => {
            let arg = command.layout_arg();
            let DisplayCommand::Layout {
                no_backup, trip, ..
            } = command;
            let result = match arg {
                None => with_webconfig(
                    &cli,
                    &root,
                    control,
                    format,
                    &trip,
                    display::layout_status,
                    |_| "the layout was read".to_string(),
                )
                .map(|(s, t)| output::display_layout_status(style, &s, t.as_ref())),
                // A typo or an out-of-range value fails here, before any
                // trip into web-config; so does a missing backups dir.
                Some(arg) => display::precheck(&arg)
                    .and_then(|()| match no_backup {
                        true => Ok(None),
                        false => commands::backups_dir().map(Some),
                    })
                    .and_then(|backups| {
                        let work = |c: &Client| {
                            display::set_layout(c, &arg, backups.as_deref(), SystemTime::now())
                        };
                        with_webconfig(&cli, &root, control, format, &trip, work, |r| {
                            if r.write.is_some() {
                                "the layout is saved".to_string()
                            } else {
                                "the layout was already set".to_string()
                            }
                        })
                    })
                    .map(|(r, t)| output::display_layout(style, &r, t.as_ref())),
            };
            finish(format, result)
        }
        Some(Command::Setup {
            command: SetupCommand::Udev { print, install },
        }) => {
            if print {
                print!("{}", udev::rule());
                return ExitCode::from(exit::OK);
            }
            let result = if install {
                install_udev_rule(&cli, &root)
            } else {
                Ok(udev::report(&root))
            };
            finish(format, result.map(|r| output::udev(style, &r)))
        }
        Some(Command::Completions { shell }) => {
            match cli::completions(shell, &mut std::io::stdout().lock()) {
                Ok(()) => ExitCode::from(exit::OK),
                Err(e) => {
                    eprintln!("gamepadii: {e}");
                    ExitCode::from(exit::TRANSPORT)
                }
            }
        }
        Some(Command::Manpages { dir }) => finish(
            Format::Human,
            cli::manpages(&dir)
                .map(|()| String::new())
                .map_err(|e| Error::io(format!("writing man pages into {}", dir.display()), e)),
        ),
    }
}

/// The TUI, with the flows' environment built on its worker thread from
/// the same flags as every other command.
fn tui(cli: &Cli) -> ExitCode {
    if !(std::io::stdout().is_terminal() && std::io::stdin().is_terminal()) {
        eprintln!(
            "gamepadii: the TUI needs a terminal; the commands work anywhere (gamepadii --help)"
        );
        return ExitCode::from(exit::USAGE);
    }
    let cli = cli.clone();
    let with_env: WithEnv = Box::new(move |backup, f| {
        let root = cli.sysroot.clone().map_or_else(Root::live, Root::new);
        let control = control_opener(&cli, &root);
        let clock = SystemClock::new();
        let open = || open(&cli);
        let env = env(
            &cli,
            &root,
            &clock,
            &open,
            control.as_deref(),
            Timeouts::default(),
            backup,
        );
        f(&env);
    });
    match gamepadii::tui::run(with_env) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => {
            eprintln!("gamepadii: the terminal UI failed: {e}");
            ExitCode::from(exit::TRANSPORT)
        }
    }
}

/// `input --watch`: READ_INPUT every 20 ms, and a line on stdout each time
/// the input changes, until Ctrl-C (or `reads` reads, in tests).
fn watch_input(env: &Env, style: Style, reads: Option<u64>) -> ExitCode {
    let reader = match flow::input_reader(env) {
        Ok(r) => r,
        Err(e) => return finish(style.format, Err(e)),
    };
    let mut last = None;
    let mut n = 0;
    loop {
        let report = match reader.read() {
            Ok(r) => r,
            Err(e) => return finish(style.format, Err(e)),
        };
        if last.as_ref() != Some(&report) {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", output::input_line(style, &report));
            let _ = out.flush();
            last = Some(report);
        }
        n += 1;
        if reads.is_some_and(|r| n >= r) {
            return ExitCode::from(exit::OK);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs `work` against the web configurator: at `--host` as it is;
/// otherwise the pad on USB, which the control channel takes into
/// web-config and back when it is in a gamepad mode.
fn with_webconfig<T>(
    cli: &Cli,
    root: &Root,
    control: Option<&OpenControl<'_>>,
    format: Format,
    trip: &TripArgs,
    work: impl FnOnce(&Client) -> gamepadii_core::Result<T>,
    done: impl FnOnce(&T) -> String,
) -> gamepadii_core::Result<(T, Option<flow::Trip>)> {
    if cli.host.is_some() {
        return connect(cli, root).and_then(|c| work(&c)).map(|r| (r, None));
    }
    on_usb(cli, root, control, format, trip, |env, progress| {
        flow::through_webconfig(env, trip.stay, progress, work, done)
    })
}

/// Runs a trip through web-config (`info`, `backup`) against the pad on
/// USB, with the progress on stderr. (`--host` reaches the web
/// configurator as it is, with no trip, and keeps the cache by board: the
/// pad there can't be told apart.)
fn on_usb<T>(
    cli: &Cli,
    root: &Root,
    control: Option<&OpenControl<'_>>,
    format: Format,
    trip: &TripArgs,
    f: impl FnOnce(&Env, &mut dyn flow::Progress) -> gamepadii_core::Result<T>,
) -> gamepadii_core::Result<T> {
    let timeouts = Timeouts {
        reboot: Duration::from_secs(trip.timeout),
        ..Timeouts::default()
    };
    let clock = SystemClock::new();
    let open = || open(cli);
    let env = env(cli, root, &clock, &open, control, timeouts, false);
    let mut progress = reporter(format);
    f(&env, &mut progress)
}

/// The web-config client: `--host` skips the USB check; otherwise the pad
/// has to be on USB in web-config (with `--mock-api`, the fixture stands in
/// for its HTTP server).
fn connect(cli: &Cli, root: &Root) -> gamepadii_core::Result<Client> {
    match (&cli.mock_api, &cli.host) {
        (Some(_), _) => {
            client::require_webconfig(root)?;
            open(cli)
        }
        (None, Some(_)) => open(cli),
        (None, None) => Client::connect(root),
    }
}

/// The client with no USB check: the flows look at USB themselves.
fn open(cli: &Cli) -> gamepadii_core::Result<Client> {
    match (&cli.mock_api, &cli.host) {
        (Some(dir), _) => Client::open(Box::new(Mock::new(dir))),
        (None, Some(host)) => Client::open(Box::new(Http::new(host)?)),
        (None, None) => Client::open(Box::new(Http::new(http::DEFAULT_HOST)?)),
    }
}

/// How to reach a pad's control channel: usbfs under the root (a fixture
/// root has no `dev/`, so tests never reach a real pad), a simulated
/// firmware with `--mock-channel`, or nothing with `--no-channel`.
fn control_opener<'a>(cli: &Cli, root: &'a Root) -> Option<Box<OpenControl<'a>>> {
    if cli.no_channel {
        return None;
    }
    if let Some(kind) = cli.mock_channel {
        let api = cli.mock_api.clone();
        return Some(Box::new(move |d: &Device| {
            mock_control(kind, api.as_deref(), d)
        }));
    }
    #[cfg(target_os = "linux")]
    {
        use gamepadii_core::channel::usbfs::Usbfs;
        let clock = SystemClock::new();
        Some(Box::new(move |d: &Device| {
            Usbfs::open_new(root, d, &clock).map(|u| Box::new(u) as Box<dyn ControlTransport>)
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        None
    }
}

/// A slot file for `profile write`: a path, or `-` for standard input.
/// Ctrl-C while the pad has the pin scan on: the first one sets a flag, so
/// the flow can stop the scan; the second one stops the program.
mod ctrl_c {
    use std::sync::atomic::{AtomicBool, Ordering};

    static STOP: AtomicBool = AtomicBool::new(false);

    #[cfg(target_os = "linux")]
    extern "C" fn on_sigint(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
        // SAFETY: signal() is async-signal-safe; SIG_DFL ends the program at
        // the next Ctrl-C.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
    }

    /// Installs the handler; the flag it sets.
    pub fn catch() -> &'static AtomicBool {
        #[cfg(target_os = "linux")]
        // SAFETY: the handler only stores to an atomic and calls signal().
        unsafe {
            libc::signal(
                libc::SIGINT,
                on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
        &STOP
    }
}

fn read_slot_file(path: &std::path::Path) -> Result<SlotFile, Error> {
    SlotFile::parse(&read_file(path)?)
}

/// A file, or standard input for `-`.
fn read_file(path: &std::path::Path) -> Result<String, Error> {
    Ok(if path == std::path::Path::new("-") {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
            .map_err(|e| Error::io("reading standard input", e))?;
        s
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("reading {}", path.display()), e))?
    })
}

/// `--mock-channel`: the firmware for `d`, in the mode its ids say.
fn mock_control(
    kind: MockChannel,
    api: Option<&std::path::Path>,
    d: &Device,
) -> Result<Box<dyn ControlTransport>, UsbError> {
    let running = d.mode.or_else(|| d.candidates.first().copied());
    let sim = match kind {
        MockChannel::NoAccess => return Err(UsbError::NoAccess),
        MockChannel::Stock => SimChannel::stock(running),
        MockChannel::Channel => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        },
        MockChannel::Leds => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        }
        .with_leds(),
        MockChannel::Lights => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        }
        .with_lights(),
        MockChannel::Slots => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        }
        .with_lights()
        .with_slots(),
        MockChannel::LightsProfiles => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        }
        .with_slots()
        .with_lights_profiles(),
        MockChannel::Input => {
            let mut sim = match api {
                Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
                None => SimChannel::gamepadii(running),
            }
            .with_lights()
            .with_slots()
            .with_input();
            sim.input.frames = SimInput::press(1 << 27, 5, 6);
            // As on the S16: GPIO 25 has no action, but a function.
            sim.input.functions = 1 << 25;
            sim
        }
        MockChannel::Log => {
            let mut sim = match api {
                Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
                None => SimChannel::gamepadii(running),
            }
            .with_slots()
            .with_lights_profiles()
            // A 1.8 build has 1.7 too.
            .with_input()
            .with_log();
            sim.panic("test 1 22 333");
            sim
        }
        MockChannel::Config => match api {
            Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
            None => SimChannel::gamepadii(running),
        }
        .with_slots()
        .with_lights_profiles()
        .with_input()
        .with_log()
        .with_config(),
        MockChannel::CustomColors => {
            let mut sim = match api {
                Some(dir) => SimChannel::from_mock(&Mock::new(dir), running),
                None => SimChannel::gamepadii(running),
            }
            .with_slots()
            .with_custom_colors();
            sim.custom_colors =
                SimCustomColors::new(&[Rgb::from_u32(0xff8800), Rgb::from_u32(0x0044ff)]);
            sim.saved_custom_colors = sim.custom_colors;
            sim.lights_limits.pending_reads = 2;
            sim
        }
    };
    let node = gamepadii_core::channel::node_path(d).unwrap_or_default();
    // One simulated pad per node for the whole process: the TUI opens the
    // channel for each action and must see the changes of the last one.
    static PADS: OnceLock<Mutex<HashMap<String, Arc<Mutex<SimChannel>>>>> = OnceLock::new();
    let mut pads = PADS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let pad = pads
        .entry(node.clone())
        .or_insert_with(|| Arc::new(Mutex::new(sim)))
        .clone();
    Ok(Box::new(Standalone::shared(pad, node)))
}

/// `setup udev --install`: through pkexec on a live system; straight into
/// the tree under `--sysroot` (tests).
fn install_udev_rule(cli: &Cli, root: &Root) -> gamepadii_core::Result<udev::Report> {
    let mut report = udev::report(root);
    if report.installed == udev::Installed::Current {
        report.action = Some(udev::Action::None);
        return Ok(report);
    }
    let rule = udev::rule();
    if cli.sysroot.is_some() {
        let path = root.path(udev::RULE_PATH);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
        }
        std::fs::write(&path, &rule)
            .map_err(|e| Error::io(format!("writing {}", path.display()), e))?;
    } else {
        let tmp = std::env::temp_dir().join(format!("gamepadii-udev-{}.rules", std::process::id()));
        std::fs::write(&tmp, &rule)
            .map_err(|e| Error::io(format!("writing {}", tmp.display()), e))?;
        eprintln!(
            "gamepadii: installing {} (pkexec asks for your password)",
            udev::RULE_PATH
        );
        let status = std::process::Command::new("pkexec")
            .args([
                "/bin/sh",
                "-c",
                "install -m 0644 -- \"$1\" \"$2\" && udevadm control --reload-rules \
                 && udevadm trigger --subsystem-match=usb --action=change",
                "gamepadii-setup-udev",
            ])
            .arg(&tmp)
            .arg(udev::RULE_PATH)
            .status();
        let _ = std::fs::remove_file(&tmp);
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(Error::NeedsUser(format!(
                    "pkexec didn't install the rule ({s}); `gamepadii setup udev --print` \
                     prints it, to install by hand as {}",
                    udev::RULE_PATH
                )));
            }
            Err(e) => return Err(Error::io("running pkexec", e)),
        }
    }
    report.installed = udev::installed(root);
    if report.installed != udev::Installed::Current {
        return Err(Error::CheckFailed(format!(
            "{} doesn't hold the rule after installing it",
            udev::RULE_PATH
        )));
    }
    report.action = Some(udev::Action::Installed);
    Ok(report)
}

/// What the guided flows run against on this machine.
fn env<'a>(
    cli: &Cli,
    root: &'a Root,
    clock: &'a SystemClock,
    open: &'a dyn Fn() -> gamepadii_core::Result<Client>,
    control: Option<&'a OpenControl<'a>>,
    timeouts: Timeouts,
    backup: bool,
) -> Env<'a> {
    let base = match &cli.host {
        Some(h) => http::base_url(h).unwrap_or_else(|_| format!("http://{h}")),
        None => format!("http://{}", http::DEFAULT_HOST),
    };
    Env {
        scan: root,
        clock,
        open,
        control,
        cache_dir: cache::cache_dir(),
        backups_dir: if backup {
            commands::backups_dir().ok()
        } else {
            None
        },
        url: format!("{base}/"),
        timeouts,
        poll: Poll::INTERVAL,
    }
}

/// Progress on stderr, with a spinner for people at a terminal.
fn reporter(format: Format) -> Reporter<std::io::Stderr> {
    let spinner = format == Format::Human && std::io::stderr().is_terminal();
    Reporter::new(std::io::stderr(), spinner)
}

/// The device cache, read through `--host`, is a convenience: a failure is
/// a warning.
fn save_cache(c: &DeviceCache) {
    if let Some(dir) = cache::cache_dir()
        && let Err(e) = cache::store(&dir, c)
    {
        eprintln!("gamepadii: warning: device cache not saved: {e}");
    }
}

fn xdg_open(url: &str) {
    let spawned = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match spawned {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("gamepadii: warning: xdg-open {url} failed ({s})"),
        Err(e) => eprintln!("gamepadii: warning: could not run xdg-open: {e}"),
    }
}

/// Prints a command's output, or its error: on stderr always, and also as a
/// JSON object (or a waybar line) on stdout so machine readers never get
/// nothing.
fn finish(format: Format, result: gamepadii_core::Result<String>) -> ExitCode {
    finish_with(format, result.map(|text| (text, exit::OK)))
}

/// [`finish`] with the exit code of a result that is not an error.
fn finish_with(format: Format, result: gamepadii_core::Result<(String, u8)>) -> ExitCode {
    match result {
        Ok((text, code)) => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(text.as_bytes());
            if format != Format::Human {
                let _ = out.write_all(b"\n");
            }
            ExitCode::from(code)
        }
        Err(err) => {
            eprintln!("gamepadii: {err}");
            match format {
                Format::Json => println!("{}", output::error_json(&err)),
                Format::Waybar => println!("{}", output::waybar_error(&err).to_line()),
                Format::Human => {}
            }
            ExitCode::from(err.exit_code())
        }
    }
}
