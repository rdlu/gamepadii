use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{CommandFactory, Parser};
use gamepadii::cli::{self, Cli, Command};
use gamepadii::output::{self, Format, Style};
use gamepadii::progress::Reporter;
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::client::{self, Client};
use gamepadii_core::error::exit;
use gamepadii_core::flow::{self, Env, Timeouts};
use gamepadii_core::mode::BootMode;
use gamepadii_core::sysfs::Root;
use gamepadii_core::transport::http::{self, Http};
use gamepadii_core::transport::mock::Mock;
use gamepadii_core::wait::{Poll, SystemClock};
use gamepadii_core::{Error, commands};

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
    match cli.command.clone() {
        None => {
            // No subcommand: usage for now (the TUI will open here on a TTY).
            let _ = Cli::command().print_help();
            ExitCode::from(exit::OK)
        }
        Some(Command::Status) => finish(
            format,
            commands::status(&root).map(|s| output::status(style, &s)),
        ),
        Some(Command::Info) => finish(
            format,
            connect(&cli, &root)
                .and_then(|c| commands::info_with_cache(&c, SystemTime::now()))
                .map(|(i, cache)| {
                    save_cache(&cache);
                    output::info(style, &i)
                }),
        ),
        Some(Command::Backup { output: path }) => finish(
            format,
            connect(&cli, &root)
                .and_then(|c| commands::backup_with_cache(&c, path.as_deref(), SystemTime::now()))
                .map(|(b, cache)| {
                    if let Some(cache) = cache {
                        save_cache(&cache);
                    }
                    output::backup(style, &b)
                }),
        ),
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
            let env = env(&cli, &root, &clock, &open, timeouts, false);
            finish(
                format,
                connect(&cli, &root)
                    .and_then(|c| {
                        flow::reboot(&env, &c, mode.into(), !no_wait, &mut reporter(format))
                    })
                    .map(|r| output::reboot(style, &r)),
            )
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
            let env = env(&cli, &root, &clock, &open, timeouts, !no_backup);
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
            let env = env(&cli, &root, &clock, &open, timeouts, false);
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

/// What the guided flows run against on this machine.
fn env<'a>(
    cli: &Cli,
    root: &'a Root,
    clock: &'a SystemClock,
    open: &'a dyn Fn() -> gamepadii_core::Result<Client>,
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
