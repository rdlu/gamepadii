//! Command line: `gamepadii status [--format human|json|waybar] [--json]`,
//! `info`, `backup [-o FILE]`, `reboot <gamepad|webconfig|bootsel>`, and
//! `--host HOST` for the web-config ones. Hidden, for packagers:
//! `gamepadii completions <shell>` and `gamepadii manpages <dir>`; for
//! tests: `--sysroot <dir>` and `--mock-api <dir>`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use gamepadii_core::mode::BootMode;

use crate::output::Format;

/// GP2040-CE gamepads from the command line: status, input modes, profiles.
#[derive(Debug, Parser)]
#[command(name = "gamepadii", version, about, long_about = None)]
pub struct Cli {
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = FormatArg::Human)]
    pub format: FormatArg,

    /// Same as `--format json`.
    #[arg(long, global = true)]
    pub json: bool,

    /// Read sysfs under DIR instead of `/` (fixture trees in tests).
    #[arg(long, global = true, hide = true, value_name = "DIR")]
    pub sysroot: Option<PathBuf>,

    /// Talk to the web configurator at HOST (host, host:port or
    /// http://host) without looking for the pad on USB first. The default
    /// is to find it on USB and use 192.168.7.1.
    #[arg(long, global = true, value_name = "HOST")]
    pub host: Option<String>,

    /// Serve the web-config API from fixture JSON files in DIR (tests).
    #[arg(
        long,
        global = true,
        hide = true,
        value_name = "DIR",
        conflicts_with = "host"
    )]
    pub mock_api: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// Text for people.
    Human,
    /// The versioned JSON contract ("schema": 1).
    Json,
    /// One line of waybar custom-module JSON; `status` only.
    Waybar,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// The pad on USB: input mode (with a confidence), USB ids, bus and port.
    Status,
    /// Firmware, board, memory, input mode, profiles and boot buttons
    /// (needs web-config).
    Info,
    /// Save every readable setting to one JSON file (needs web-config).
    Backup {
        /// Where to write it [default:
        /// ~/.local/share/gamepadii/backups/<time>-<board>.json].
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Restart the pad into gamepad mode, web-config or BOOTSEL (needs
    /// web-config).
    Reboot {
        #[arg(value_enum)]
        mode: RebootArg,
    },
    /// Print a shell completion script, for packagers.
    #[command(hide = true)]
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Write the man pages (gamepadii.1, gamepadii-status.1, ...) into DIR, for packagers.
    #[command(hide = true)]
    Manpages { dir: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RebootArg {
    /// The saved input mode (XInput, Keyboard, ...).
    Gamepad,
    /// The web configurator, at 192.168.7.1.
    #[value(alias = "web-config")]
    Webconfig,
    /// The boot ROM's USB drive, for flashing firmware.
    Bootsel,
}

impl From<RebootArg> for BootMode {
    fn from(a: RebootArg) -> Self {
        match a {
            RebootArg::Gamepad => BootMode::Gamepad,
            RebootArg::Webconfig => BootMode::Webconfig,
            RebootArg::Bootsel => BootMode::Bootsel,
        }
    }
}

impl Cli {
    /// The output format with `--json` folded in; an error for combinations
    /// clap can't express.
    pub fn output_format(&self) -> Result<Format, String> {
        let waybar_ok = matches!(self.command, Some(Command::Status));
        match (self.json, self.format) {
            (true, FormatArg::Waybar) => Err("--json conflicts with --format waybar".into()),
            (true, _) | (false, FormatArg::Json) => Ok(Format::Json),
            (false, FormatArg::Human) => Ok(Format::Human),
            (false, FormatArg::Waybar) if waybar_ok => Ok(Format::Waybar),
            (false, FormatArg::Waybar) => Err("--format waybar is only supported by status".into()),
        }
    }
}

/// Writes the completion script for `shell`.
pub fn completions(shell: Shell, out: &mut dyn Write) {
    clap_complete::generate(shell, &mut Cli::command(), "gamepadii", out);
}

/// Writes gamepadii(1) and a page per visible subcommand into `dir`, which
/// must exist.
pub fn manpages(dir: &Path) -> io::Result<()> {
    clap_mangen::generate_to(Cli::command(), dir)
}
