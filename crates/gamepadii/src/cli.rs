//! Command line: `gamepadii status [--format human|json|waybar] [--json]`,
//! `info`, `backup [-o FILE]`, `reboot <gamepad|webconfig|bootsel>`,
//! `mode [MODE]`, `webconfig [--open|--exit]`, `profile switch <N|LABEL>
//! [--save]`, `display layout [LAYOUT | --left N --right N]`, `leds [set
//! ... | brightness up|down|N]`, `tui` (also
//! no command on a terminal), `setup udev [--print|--install]`, `--host HOST`
//! for the
//! web-config ones and `--no-channel` to leave the control channel alone.
//! Hidden, for packagers: `gamepadii completions <shell>` and `gamepadii
//! manpages <dir>`; for tests: `--sysroot <dir>`, `--mock-api <dir>` and
//! `--mock-channel <kind>`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use gamepadii_core::display::LayoutArg;
use gamepadii_core::flow::{self, ProfileArg};
use gamepadii_core::leds::{LedChange, Level, Pick};
use gamepadii_core::mode::{BootMode, InputMode};

use crate::output::Format;

/// GP2040-CE gamepads from the command line: status, input modes, profiles.
/// With no command on a terminal, it opens the TUI.
#[derive(Debug, Clone, Parser)]
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

    /// Don't use the USB control channel (the gamepadii firmware's): only
    /// web-config and button presses.
    #[arg(long, global = true)]
    pub no_channel: bool,

    /// Answer on EP0 from a simulated firmware instead of the device node
    /// (tests).
    #[arg(
        long,
        global = true,
        hide = true,
        value_enum,
        value_name = "KIND",
        conflicts_with = "no_channel"
    )]
    pub mock_channel: Option<MockChannel>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// What `--mock-channel` simulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MockChannel {
    /// Stock GP2040-CE: no channel.
    Stock,
    /// The gamepadii firmware (profiles from `--mock-api`, when given).
    Channel,
    /// The same with control channel v1.1: the LED commands.
    Leds,
    /// A node this user can't open (no udev rule).
    NoAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// Text for people.
    Human,
    /// The versioned JSON contract ("schema": 1).
    Json,
    /// One line of waybar custom-module JSON; `status` and `leds` only.
    Waybar,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// The pad on USB: input mode (with a confidence), USB ids, bus and port.
    Status,
    /// Firmware, board, memory, input mode, profiles and boot buttons.
    /// Needs web-config; in a gamepad mode the control channel takes the
    /// pad there and back.
    Info {
        #[command(flatten)]
        trip: TripArgs,
    },
    /// Save every readable setting to one JSON file. Needs web-config; in a
    /// gamepad mode the control channel takes the pad there and back.
    Backup {
        /// Where to write it [default:
        /// ~/.local/share/gamepadii/backups/<time>-<board>.json].
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        #[command(flatten)]
        trip: TripArgs,
    },
    /// Restart the pad into gamepad mode, web-config or BOOTSEL, then wait
    /// for it on USB and check what came back. Needs web-config, or the
    /// control channel in a gamepad mode.
    Reboot {
        #[arg(value_enum)]
        mode: RebootArg,
        /// Don't wait for the pad to come back.
        #[arg(long)]
        no_wait: bool,
        /// How long to wait for it to come back on USB.
        #[arg(long, value_name = "SECS", default_value_t = 10)]
        timeout: u64,
    },
    /// Switch the input mode, or with no MODE show the current one and the
    /// choices. In a gamepad mode it uses the control channel, or else says
    /// which buttons to press and waits.
    Mode {
        /// The input mode to switch to.
        #[arg(value_parser = mode_parser())]
        mode: Option<InputMode>,
        /// Print the instructions and exit 5 instead of waiting for the
        /// button press.
        #[arg(long)]
        no_wait: bool,
        /// How long to wait for the button press.
        #[arg(long, value_name = "SECS", default_value_t = 60)]
        timeout: u64,
        /// How long to wait for the pad to come back after the reboot.
        #[arg(long, value_name = "SECS", default_value_t = 10)]
        reboot_timeout: u64,
        /// Don't back up the settings before writing.
        #[arg(long)]
        no_backup: bool,
    },
    /// Get the pad into web-config and print its URL; in a gamepad mode it
    /// reboots it over the control channel, or else says which buttons to
    /// press and waits.
    Webconfig {
        /// Open the URL in the browser (xdg-open).
        #[arg(long, conflicts_with = "exit")]
        open: bool,
        /// Leave web-config: reboot into gamepad mode and wait for it.
        #[arg(long)]
        exit: bool,
        /// Print the instructions and exit 5 instead of waiting.
        #[arg(long)]
        no_wait: bool,
        /// How long to wait for the button press (or, with --exit, for the
        /// pad to come back).
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,
    },
    /// Pin-mapping profiles on the pad.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// The per-button RGB LEDs: animation, brightness, colours and the
    /// custom theme. Over the control channel when the firmware has the LED
    /// commands (no reboot); else from web-config, where stock firmware
    /// doesn't show the animation or brightness (in a gamepad mode the
    /// control channel takes the pad there and back).
    Leds {
        #[command(subcommand)]
        command: Option<LedsCommand>,
        #[command(flatten)]
        trip: TripArgs,
    },
    /// The terminal UI: the pad, its input modes and profiles, web-config
    /// and backups. `gamepadii` with no command opens it on a terminal.
    Tui,
    /// The pad's OLED display.
    Display {
        #[command(subcommand)]
        command: DisplayCommand,
    },
    /// One-time setup of this machine.
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
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

/// For `info` and `backup` from a gamepad mode, over the control channel.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct TripArgs {
    /// Leave the pad in web-config afterwards instead of rebooting it back
    /// into its gamepad mode.
    #[arg(long)]
    pub stay: bool,
    /// How long to wait for the pad on USB after each reboot.
    #[arg(long, value_name = "SECS", default_value_t = 10)]
    pub timeout: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ProfileCommand {
    /// Switch the active profile live, with no reboot (needs the control
    /// channel of the gamepadii firmware). RAM only unless --save: the pad
    /// goes back to its saved profile when it restarts.
    Switch {
        /// The profile: its number (1-6) or its label, any case.
        #[arg(value_parser = profile_parser)]
        profile: ProfileArg,
        /// Also save it to flash, as the profile hotkeys do.
        #[arg(long)]
        save: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum LedsCommand {
    /// Change the LEDs live, with no reboot (needs control channel v1.1 of
    /// the gamepadii firmware). RAM only unless --save: the pad goes back
    /// to its saved LED settings when it restarts.
    Set {
        /// The animation: static-color, rainbow, chase, static-theme,
        /// custom-theme, or its number.
        #[arg(long, value_name = "NAME|N", value_parser = pick_parser)]
        animation: Option<Pick>,
        /// The brightness level (0 to the pad's steps), up or down.
        #[arg(long, value_name = "N|up|down", value_parser = level_parser)]
        brightness: Option<Level>,
        /// The static-color animation's colour: black, white, red, orange,
        /// yellow, limegreen, green, seafoam, aqua, skyblue, blue, purple,
        /// pink, magenta, indigo, violet, or its number.
        #[arg(long, value_name = "NAME|N", value_parser = pick_parser)]
        static_color: Option<Pick>,
        /// The colour of a pressed button (same names).
        #[arg(long, value_name = "NAME|N", value_parser = pick_parser)]
        button_color: Option<Pick>,
        /// The static-theme animation's theme: static-rainbow, xbox,
        /// playstation, neogeo, street-fighter-2, tekken, ..., or its number.
        #[arg(long, value_name = "NAME|N", value_parser = pick_parser)]
        theme: Option<Pick>,
        /// Also save it to flash, as the LED hotkeys do (alone: save what
        /// runs now). In PS4/PS5 mode the firmware skips saves while the
        /// controller authentication needs the flash.
        #[arg(long)]
        save: bool,
    },
    /// One brightness step up or down, or a level, live (as `set
    /// --brightness`).
    Brightness {
        #[arg(value_name = "up|down|N", value_parser = level_parser)]
        level: Level,
        /// Also save it to flash.
        #[arg(long)]
        save: bool,
    },
}

impl LedsCommand {
    /// What to change.
    pub fn change(&self) -> LedChange {
        match self.clone() {
            LedsCommand::Set {
                animation,
                brightness,
                static_color,
                button_color,
                theme,
                save,
            } => LedChange {
                animation,
                brightness,
                static_color,
                button_color,
                theme,
                save,
            },
            LedsCommand::Brightness { level, save } => LedChange {
                brightness: Some(level),
                save,
                ..LedChange::default()
            },
        }
    }
}

fn pick_parser(s: &str) -> Result<Pick, String> {
    s.parse()
}

fn level_parser(s: &str) -> Result<Level, String> {
    s.parse()
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DisplayCommand {
    /// Pick the drawing of the buttons on the OLED, or with no LAYOUT show
    /// the current one and the choices. Haute42 pads take their model
    /// (s16, g16, s13, m16, ...) or the layout (default, alt0..alt5); any
    /// board takes the raw values. Needs web-config; in a gamepad mode the
    /// control channel takes the pad there and back, and the restart shows
    /// the new drawing.
    Layout {
        /// The model or layout name, any case.
        #[arg(conflicts_with_all = ["left", "right"])]
        layout: Option<String>,
        /// The `buttonLayout` value (the left half), with --right.
        #[arg(long, value_name = "N", requires = "right")]
        left: Option<i64>,
        /// The `buttonLayoutRight` value (the right half), with --left.
        #[arg(long, value_name = "N", requires = "left")]
        right: Option<i64>,
        /// Don't back up the settings before writing.
        #[arg(long)]
        no_backup: bool,
        #[command(flatten)]
        trip: TripArgs,
    },
}

impl DisplayCommand {
    /// The layout asked for; `None` to show the current one.
    pub fn layout_arg(&self) -> Option<LayoutArg> {
        let DisplayCommand::Layout {
            layout,
            left,
            right,
            ..
        } = self;
        match (layout, left, right) {
            (Some(name), _, _) => Some(LayoutArg::Named(name.clone())),
            (None, Some(left), Some(right)) => Some(LayoutArg::Raw {
                left: *left,
                right: *right,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum SetupCommand {
    /// The udev rule that lets gamepadii use the control channel without
    /// root: what it grants, and whether it is installed.
    Udev {
        /// Print the rule file itself.
        #[arg(long, conflicts_with = "install")]
        print: bool,
        /// Install it as /etc/udev/rules.d/70-gamepadii.rules (through
        /// pkexec) and reload udev.
        #[arg(long)]
        install: bool,
    },
}

fn profile_parser(s: &str) -> Result<ProfileArg, String> {
    s.parse()
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

/// The input modes `mode` takes, with their names as help; `hid` is an
/// alias for `generic`, and `-` works for `_`.
fn mode_parser() -> impl TypedValueParser<Value = InputMode> {
    PossibleValuesParser::new(flow::targets().map(|m| {
        let v = PossibleValue::new(m.key()).help(m.label());
        match m {
            InputMode::Generic => v.alias("hid"),
            InputMode::SwitchPro => v.alias("switch-pro"),
            InputMode::Switch2Pro => v.alias("switch2-pro"),
            _ => v,
        }
    }))
    .map(|s| {
        let s = if s == "hid" { "generic".into() } else { s };
        s.parse::<InputMode>().expect("a listed mode")
    })
}

impl Cli {
    /// The output format with `--json` folded in; an error for combinations
    /// clap can't express.
    pub fn output_format(&self) -> Result<Format, String> {
        let waybar_ok = matches!(
            self.command,
            Some(Command::Status | Command::Leds { command: None, .. })
        );
        match (self.json, self.format) {
            (true, FormatArg::Waybar) => Err("--json conflicts with --format waybar".into()),
            (true, _) | (false, FormatArg::Json) => Ok(Format::Json),
            (false, FormatArg::Human) => Ok(Format::Human),
            (false, FormatArg::Waybar) if waybar_ok => Ok(Format::Waybar),
            (false, FormatArg::Waybar) => {
                Err("--format waybar is only supported by status and leds".into())
            }
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
