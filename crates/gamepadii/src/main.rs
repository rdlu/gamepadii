use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use gamepadii::cli::{self, Cli, Command};
use gamepadii::output::{self, Format, Style};
use gamepadii_core::client::{self, Client};
use gamepadii_core::error::exit;
use gamepadii_core::sysfs::Root;
use gamepadii_core::transport::http::Http;
use gamepadii_core::transport::mock::Mock;
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
                .and_then(|c| commands::info(&c))
                .map(|i| output::info(style, &i)),
        ),
        Some(Command::Backup { output: path }) => finish(
            format,
            connect(&cli, &root)
                .and_then(|c| commands::backup(&c, path.as_deref(), std::time::SystemTime::now()))
                .map(|b| output::backup(style, &b)),
        ),
        Some(Command::Reboot { mode }) => finish(
            format,
            connect(&cli, &root)
                .and_then(|c| commands::reboot(&c, mode.into()))
                .map(|r| output::reboot(style, &r)),
        ),
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
        (Some(dir), _) => {
            client::require_webconfig(root)?;
            Client::open(Box::new(Mock::new(dir)))
        }
        (None, Some(host)) => Client::open(Box::new(Http::new(host)?)),
        (None, None) => Client::connect(root),
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
