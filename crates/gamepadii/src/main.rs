use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{CommandFactory, Parser};
use gamepadii::cli::{self, Cli, Command, MockChannel, ProfileCommand, SetupCommand, TripArgs};
use gamepadii::output::{self, Format, Style};
use gamepadii::progress::Reporter;
use gamepadii::tui::worker::WithEnv;
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::channel::sim::{SimChannel, Standalone};
use gamepadii_core::channel::{ControlTransport, OpenControl, UsbError};
use gamepadii_core::client::{self, Client};
use gamepadii_core::discovery::Device;
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Env, Timeouts};
use gamepadii_core::mode::BootMode;
use gamepadii_core::sysfs::Root;
use gamepadii_core::transport::http::{self, Http};
use gamepadii_core::transport::mock::Mock;
use gamepadii_core::wait::{Poll, SystemClock};
use gamepadii_core::{Error, commands, udev};

fn main() -> ExitCode {
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
            let work = |c: &Client| {
                let (info, cache) = commands::info_with_cache(c, SystemTime::now())?;
                save_cache(&cache);
                Ok(info)
            };
            let result = with_webconfig(&cli, &root, control, format, &trip, work, |_| {
                "the pad was read".to_string()
            })
            .map(|(i, t)| output::info(style, &i, t.as_ref()));
            finish(format, result)
        }
        Some(Command::Backup { output: path, trip }) => {
            let work = |c: &Client| {
                let (report, cache) =
                    commands::backup_with_cache(c, path.as_deref(), SystemTime::now())?;
                if let Some(cache) = cache {
                    save_cache(&cache);
                }
                Ok(report)
            };
            let result = with_webconfig(&cli, &root, control, format, &trip, work, |b| {
                format!("the backup is saved in {}", b.path.display())
            })
            .map(|(b, t)| output::backup(style, &b, t.as_ref()));
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
            cli::completions(shell, &mut std::io::stdout().lock());
            ExitCode::from(exit::OK)
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
    let timeouts = Timeouts {
        reboot: Duration::from_secs(trip.timeout),
        ..Timeouts::default()
    };
    let clock = SystemClock::new();
    let open = || open(cli);
    let env = env(cli, root, &clock, &open, control, timeouts, false);
    let mut progress = reporter(format);
    flow::through_webconfig(&env, trip.stay, &mut progress, work, done)
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
        Some(Box::new(move |d: &Device| {
            Usbfs::open(root, d).map(|u| Box::new(u) as Box<dyn ControlTransport>)
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        None
    }
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
    };
    let node = gamepadii_core::channel::node_path(d).unwrap_or_default();
    Ok(Box::new(Standalone::new(sim, node)))
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

/// The device cache is a convenience: a failure is a warning.
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
    match result {
        Ok(text) => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(text.as_bytes());
            if format != Format::Human {
                let _ = out.write_all(b"\n");
            }
            ExitCode::from(exit::OK)
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
