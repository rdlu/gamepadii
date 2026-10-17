use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use gamepadii::cli::{self, Cli, Command};
use gamepadii::output::{self, Format, Style};
use gamepadii_core::error::exit;
use gamepadii_core::sysfs::Root;
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
    let root = cli.sysroot.map_or_else(Root::live, Root::new);
    match cli.command {
        None => {
            // No subcommand: usage for now (the TUI will open here on a TTY).
            let _ = Cli::command().print_help();
            ExitCode::from(exit::OK)
        }
        Some(Command::Status) => {
            let style = Style::detect(format);
            finish(
                format,
                commands::status(&root).map(|s| output::status(style, &s)),
            )
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
