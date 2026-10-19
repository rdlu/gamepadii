//! The profile library: profiles in KDL 2.0 files on the host, in
//! `$XDG_CONFIG_HOME/gamepadii/profiles/` (`~/.config/gamepadii/profiles/`),
//! and the examples that gamepadii ships. The design is in
//! `docs/design/profile-library.md`.
//!
//! A profile names physical buttons from a board file ([`board`]), never
//! GPIOs. It can have a base profile: what it leaves out comes from the
//! base. In this step a profile holds what the pad can take live: a pin
//! mapping, a label and an input mode. [`Resolved::slot_file`] turns a
//! profile into a slot file for `gamepadii profile write`, and [`pull`]
//! turns the pad's slots into profiles. [`diff`] compares a profile with a
//! slot, for `library diff` and `library apply`.

pub mod board;
pub mod diff;
pub mod parse;
pub mod pull;
pub mod theme;
pub mod write;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

use crate::buttons::NameSystem;
use crate::channel::proto::SLOT_LABEL_LEN;
use crate::error::{Error, Result};
use crate::mode::InputMode;
use crate::slots::{Action, FilePin, SlotFile, action_name};

pub use board::Board;
pub use theme::{DefinedTheme, ResolvedTheme};

/// What a button does in a profile: a `GpioAction`, and the masks of a
/// `CUSTOM_BUTTON_COMBO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pin {
    pub action: i16,
    pub button_mask: u32,
    pub dpad_mask: u32,
}

/// One profile, as its file gives it: before the base is applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    /// The name in the library. Names are unique; case does not matter.
    pub name: String,
    /// The profile that gives what this one leaves out.
    pub base: Option<String>,
    /// The board file name (`haute42-s16`); the base can give it.
    pub board: Option<String>,
    /// The label on the pad (at most 16 bytes); the name when absent. It
    /// does not come from the base.
    pub label: Option<String>,
    /// The input mode that the profile needs; the base can give it.
    pub input_mode: Option<InputMode>,
    /// The name system of the actions in `pins`: the profile's `names`,
    /// else the file's, else GP2040. It does not come from the base.
    pub names: NameSystem,
    /// The theme of the lights; the base can give it.
    pub theme: Option<String>,
    /// The action of each board button that the profile and its bases do
    /// not name; when absent, those buttons follow profile 1 of the pad.
    pub default: Option<Pin>,
    /// Button name → action, in the order of the file.
    pub pins: Vec<(String, Pin)>,
}

/// Where a library file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A file in the library directory.
    File(PathBuf),
    /// A file that gamepadii ships, by its name in `data/`.
    Shipped(&'static str),
}

impl Origin {
    pub fn shipped(&self) -> bool {
        matches!(self, Origin::Shipped(_))
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::File(p) => write!(f, "{}", p.display()),
            Origin::Shipped(name) => write!(f, "(shipped) {name}"),
        }
    }
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A line and a column in a file, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Place {
    pub line: usize,
    pub column: usize,
}

/// Where the parts of a profile are in its file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Places {
    pub node: Place,
    pub base: Option<Place>,
    pub board: Option<Place>,
    pub label: Option<Place>,
    pub input_mode: Option<Place>,
    pub theme: Option<Place>,
    pub default: Option<Place>,
    /// One for each of [`Profile::pins`].
    pub pins: Vec<Place>,
}

/// A profile and the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defined {
    pub profile: Profile,
    pub origin: Origin,
    pub at: Places,
}

/// A problem in a library file, at a line and column when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryError {
    pub file: Origin,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
}

impl LibraryError {
    pub fn at(file: &Origin, place: Option<Place>, msg: &str) -> Self {
        Self {
            file: file.clone(),
            line: place.map(|p| p.line),
            column: place.map(|p| p.column),
            message: msg.to_string(),
        }
    }
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(l) = self.line {
            write!(f, ":{l}")?;
        }
        if let Some(c) = self.column {
            write!(f, ":{c}")?;
        }
        write!(f, ": {}", self.message)
    }
}

impl From<LibraryError> for Error {
    fn from(e: LibraryError) -> Self {
        Error::Usage(e.to_string())
    }
}

/// `$XDG_CONFIG_HOME/gamepadii/profiles` (`~/.config/gamepadii/profiles`);
/// `None` without a home.
pub fn library_dir() -> Option<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?;
    Some(base.join("gamepadii").join("profiles"))
}

/// The example profiles that gamepadii ships.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "examples/haute42-s16.kdl",
        include_str!("../../data/examples/haute42-s16.kdl"),
    ),
    (
        "examples/per-game.kdl",
        include_str!("../../data/examples/per-game.kdl"),
    ),
    (
        "examples/xbox-names.kdl",
        include_str!("../../data/examples/xbox-names.kdl"),
    ),
    (
        "examples/themes.kdl",
        include_str!("../../data/examples/themes.kdl"),
    ),
];

/// Every profile in the library, and the files that have errors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Library {
    pub dir: Option<PathBuf>,
    pub profiles: Vec<Defined>,
    pub themes: Vec<DefinedTheme>,
    pub errors: Vec<LibraryError>,
    /// The board files that profiles can name.
    pub boards: Vec<Board>,
}

impl Library {
    /// The `.kdl` files in `dir` (a missing directory is an empty library)
    /// and the shipped examples.
    pub fn load(dir: Option<&Path>) -> Result<Library> {
        let mut files = Vec::new();
        if let Some(dir) = dir {
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    for e in entries {
                        let e =
                            e.map_err(|e| Error::io(format!("reading {}", dir.display()), e))?;
                        let path = e.path();
                        if path.extension().is_some_and(|x| x == "kdl") && path.is_file() {
                            files.push(path);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(format!("reading {}", dir.display()), e)),
            }
        }
        files.sort();
        let mut sources = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
            sources.push((Origin::File(path), text));
        }
        let mut lib = Library::from_sources(sources, true);
        lib.dir = dir.map(Path::to_path_buf);
        Ok(lib)
    }

    /// The profiles in `sources`, and the shipped examples with `examples`.
    /// A file that does not parse adds one error and no profile. A user
    /// profile hides a shipped one with the same name.
    pub fn from_sources(sources: Vec<(Origin, String)>, examples: bool) -> Library {
        let mut lib = Library {
            boards: board::shipped(),
            ..Library::default()
        };
        for (origin, text) in sources {
            lib.add(origin, &text, false);
        }
        if examples {
            for (name, text) in EXAMPLES {
                lib.add(Origin::Shipped(name), text, true);
            }
        }
        lib
    }

    fn add(&mut self, origin: Origin, text: &str, shipped: bool) {
        let parsed = match parse::file(origin, text) {
            Ok(d) => d,
            Err(e) => return self.errors.push(e),
        };
        // Profiles and themes share the names: `library show NAME` shows
        // either one.
        let taken = |lib: &mut Library, name: &str, origin: &Origin, place: Place| {
            let other = lib
                .get_exact(name)
                .map(|d| ("profile", &d.origin))
                .or_else(|| lib.get_theme(name).map(|d| ("theme", &d.origin)));
            let Some((kind, file)) = other else {
                return false;
            };
            if !shipped {
                let msg = format!("{name:?} is also a {kind} in {file}; names must be unique");
                lib.errors.push(LibraryError::at(origin, Some(place), &msg));
            }
            true
        };
        for d in parsed.profiles {
            if !taken(self, &d.profile.name, &d.origin, d.at.node) {
                self.profiles.push(d);
            }
        }
        for d in parsed.themes {
            if !taken(self, &d.theme.name, &d.origin, d.at.node) {
                self.themes.push(d);
            }
        }
    }

    fn get_exact(&self, name: &str) -> Option<&Defined> {
        self.profiles
            .iter()
            .find(|d| d.profile.name.eq_ignore_ascii_case(name))
    }

    fn get_theme(&self, name: &str) -> Option<&DefinedTheme> {
        self.themes
            .iter()
            .find(|d| d.theme.name.eq_ignore_ascii_case(name))
    }

    /// Whether `name` is a theme, not a profile.
    pub fn is_theme(&self, name: &str) -> bool {
        self.get_theme(name).is_some()
    }

    /// The profile called `name` (case does not matter).
    pub fn get(&self, name: &str) -> Result<&Defined> {
        self.get_exact(name).ok_or_else(|| {
            let also = match self.get_theme(name) {
                Some(t) => format!(" ({:?} is a theme)", t.theme.name),
                None => String::new(),
            };
            Error::Usage(format!(
                "no profile {name:?} in the library{also}; `gamepadii library list` shows them"
            ))
        })
    }

    /// The theme called `name` (case does not matter), checked against its
    /// board file.
    pub fn resolve_theme(&self, name: &str) -> Result<ResolvedTheme> {
        self.resolve_theme_for(name, None).map_err(|e| match e {
            ThemeError::Missing => {
                let also = match self.get_exact(name) {
                    Some(p) => format!(" ({:?} is a profile)", p.profile.name),
                    None => String::new(),
                };
                Error::Usage(format!(
                    "no theme {name:?} in the library{also}; `gamepadii library list` shows them"
                ))
            }
            ThemeError::Bad(e) => e.into(),
        })
    }

    /// The theme of the profile `profile` (with its bases applied), on the
    /// profile's board.
    pub fn profile_theme(&self, profile: &Resolved) -> Result<Option<ResolvedTheme>> {
        let Some(name) = &profile.theme else {
            return Ok(None);
        };
        let board = self.board(&profile.board);
        match self.resolve_theme_for(name, board) {
            Ok(t) => Ok(Some(t)),
            Err(ThemeError::Missing) => {
                Err(Error::Usage(format!("no theme {name:?} in the library")))
            }
            Err(ThemeError::Bad(e)) => Err(e.into()),
        }
    }

    /// The theme called `name`, on its own board, or else on `board` (the
    /// board of a profile that names it).
    fn resolve_theme_for(
        &self,
        name: &str,
        board: Option<&Board>,
    ) -> std::result::Result<ResolvedTheme, ThemeError> {
        let d = self.get_theme(name).ok_or(ThemeError::Missing)?;
        let own = match &d.theme.board {
            Some(b) => Some(self.board(b).ok_or_else(|| {
                let known: Vec<&str> = self.boards.iter().map(|b| b.name.as_str()).collect();
                ThemeError::Bad(LibraryError::at(
                    &d.origin,
                    d.at.board,
                    &format!("no board file {b:?} (known: {})", known.join(", ")),
                ))
            })?),
            None => None,
        };
        if let (Some(own), Some(other)) = (own, board)
            && own.name != other.name
        {
            return Err(ThemeError::Bad(LibraryError::at(
                &d.origin,
                d.at.board,
                &format!(
                    "board conflict: theme {:?} is for {}, the profile for {}",
                    d.theme.name, own.name, other.name
                ),
            )));
        }
        theme::resolve(d, own.or(board)).map_err(ThemeError::Bad)
    }

    fn board(&self, name: &str) -> Option<&Board> {
        self.boards.iter().find(|b| b.name == name)
    }

    /// The profile called `name` with its bases applied, checked against
    /// its board file.
    pub fn resolve(&self, name: &str) -> Result<Resolved> {
        Ok(self.resolve_defined(self.get(name)?)?)
    }

    fn resolve_defined(&self, top: &Defined) -> std::result::Result<Resolved, LibraryError> {
        // The chain: the profile, its base, the base of the base, ...
        let mut chain = vec![top];
        while let Some(base) = &chain.last().expect("not empty").profile.base {
            let at = chain.last().expect("not empty");
            let err = |msg: String| LibraryError::at(&at.origin, at.at.base, &msg);
            let Some(next) = self.get_exact(base) else {
                return Err(err(format!("base profile {base:?} is not in the library")));
            };
            if chain.iter().any(|d| std::ptr::eq(*d, next)) {
                let names: Vec<&str> = chain.iter().map(|d| d.profile.name.as_str()).collect();
                return Err(err(format!(
                    "a loop of bases: {} -> {}",
                    names.join(" -> "),
                    next.profile.name
                )));
            }
            chain.push(next);
        }
        // From the last base to the profile, each one on top of the one
        // before.
        let mut board: Option<(&Board, &Defined)> = None;
        let mut input_mode: Option<InputMode> = None;
        let mut theme: Option<&Defined> = None;
        let mut default: Option<(Pin, &str)> = None;
        for d in chain.iter().rev() {
            let p = &d.profile;
            if let Some(name) = &p.board {
                let Some(b) = self.board(name) else {
                    let known: Vec<&str> = self.boards.iter().map(|b| b.name.as_str()).collect();
                    return Err(LibraryError::at(
                        &d.origin,
                        d.at.board,
                        &format!("no board file {name:?} (known: {})", known.join(", ")),
                    ));
                };
                if let Some((old, from)) = board
                    && old.name != b.name
                {
                    return Err(LibraryError::at(
                        &d.origin,
                        d.at.board,
                        &format!(
                            "board conflict: the base {:?} is for {}",
                            from.profile.name, old.name
                        ),
                    ));
                }
                board = Some((b, d));
            }
            if let Some(m) = p.input_mode {
                if let Some(old) = input_mode
                    && old != m
                {
                    return Err(LibraryError::at(
                        &d.origin,
                        d.at.input_mode,
                        &format!(
                            "input-mode conflict: a base needs {}, this profile {}",
                            old.key(),
                            m.key()
                        ),
                    ));
                }
                input_mode = Some(m);
            }
            if p.theme.is_some() {
                theme = Some(d);
            }
            if let Some(pin) = p.default {
                default = Some((pin, &p.name));
            }
        }
        let Some((board, _)) = board else {
            return Err(LibraryError::at(
                &top.origin,
                Some(top.at.node),
                &format!(
                    "profile {:?} has no board: add `board \"haute42-s16\"` to it or to a base",
                    top.profile.name
                ),
            ));
        };
        // Every button that the chain names must be on the board. An alias
        // names the same button as its name.
        let mut pins: Vec<(String, Pin, &str)> = Vec::new();
        for d in chain.iter().rev() {
            let mut seen: Vec<(&str, &str)> = Vec::new();
            for ((button, pin), place) in d.profile.pins.iter().zip(&d.at.pins) {
                let err = |msg: String| LibraryError::at(&d.origin, Some(*place), &msg);
                let Some(b) = board.button(button) else {
                    if let Some(new) = board.renamed(button) {
                        return Err(err(format!(
                            "button {button:?} is now {new:?} on {}",
                            board.name
                        )));
                    }
                    let known: Vec<&str> = board.buttons.iter().map(|b| b.name.as_str()).collect();
                    return Err(err(format!(
                        "unknown button {button:?}: {} has {}",
                        board.name,
                        known.join(", ")
                    )));
                };
                if let Some((other, _)) = seen.iter().find(|(_, name)| *name == b.name) {
                    return Err(err(format!(
                        "button {} is twice in pins: {other} and {button} are names of one \
                         button",
                        b.name
                    )));
                }
                seen.push((button, &b.name));
                pins.retain(|(n, _, _)| *n != b.name);
                pins.push((b.name.clone(), *pin, &d.profile.name));
            }
        }
        let theme = match theme {
            Some(d) => {
                let name = d.profile.theme.as_deref().expect("a profile with a theme");
                let t = self
                    .resolve_theme_for(name, Some(board))
                    .map_err(|e| match e {
                        ThemeError::Missing => LibraryError::at(
                            &d.origin,
                            d.at.theme,
                            &format!("theme {name:?} is not in the library"),
                        ),
                        ThemeError::Bad(e) => e,
                    })?;
                Some(t.name)
            }
            None => None,
        };
        let label = top
            .profile
            .label
            .clone()
            .unwrap_or_else(|| top.profile.name.clone());
        if label.len() > SLOT_LABEL_LEN {
            let (place, what) = match top.at.label {
                Some(p) => (Some(p), "the label"),
                None => (
                    Some(top.at.node),
                    "the name (the label when there is no label)",
                ),
            };
            return Err(LibraryError::at(
                &top.origin,
                place,
                &format!(
                    "{what} {label:?} is {} bytes; the pad keeps {SLOT_LABEL_LEN}: add a \
                     shorter label",
                    label.len()
                ),
            ));
        }
        let resolved_pins = board
            .buttons
            .iter()
            .map(|b| {
                let set = pins
                    .iter()
                    .find(|(n, _, _)| *n == b.name)
                    .map(|(_, pin, from)| (*pin, *from))
                    .or(default);
                ResolvedPin {
                    button: b.name.clone(),
                    gpio: b.gpio,
                    action: set.map(|(p, _)| p.action),
                    action_name: set.and_then(|(p, _)| action_name(p.action)),
                    button_mask: set.map_or(0, |(p, _)| p.button_mask),
                    dpad_mask: set.map_or(0, |(p, _)| p.dpad_mask),
                    from: set.map(|(_, f)| f.to_string()),
                }
            })
            .collect();
        Ok(Resolved {
            name: top.profile.name.clone(),
            file: top.origin.clone(),
            shipped: top.origin.shipped(),
            line: top.at.node.line,
            bases: chain[1..].iter().map(|d| d.profile.name.clone()).collect(),
            board: board.name.clone(),
            firmware_board: board.firmware_board.clone(),
            label,
            input_mode,
            names: top.profile.names,
            theme,
            pins: resolved_pins,
        })
    }

    /// `gamepadii library list`: every profile, checked, and the errors.
    pub fn list(&self) -> ListReport {
        let mut errors = self.errors.clone();
        let profiles = self
            .profiles
            .iter()
            .map(|d| {
                let checked = self.resolve_defined(d);
                let ok = match checked {
                    Ok(_) => true,
                    Err(e) => {
                        if !errors.contains(&e) {
                            errors.push(e);
                        }
                        false
                    }
                };
                ListEntry {
                    name: d.profile.name.clone(),
                    file: d.origin.clone(),
                    shipped: d.origin.shipped(),
                    line: d.at.node.line,
                    base: d.profile.base.clone(),
                    label: d
                        .profile
                        .label
                        .clone()
                        .unwrap_or_else(|| d.profile.name.clone()),
                    ok,
                }
            })
            .collect();
        let themes = self
            .themes
            .iter()
            .map(|d| {
                let ok = match self.resolve_theme_for(&d.theme.name, None) {
                    Ok(_) => true,
                    Err(ThemeError::Bad(e)) => {
                        if !errors.contains(&e) {
                            errors.push(e);
                        }
                        false
                    }
                    Err(ThemeError::Missing) => false,
                };
                ThemeEntry {
                    name: d.theme.name.clone(),
                    file: d.origin.clone(),
                    shipped: d.origin.shipped(),
                    line: d.at.node.line,
                    board: d.theme.board.clone(),
                    ok,
                }
            })
            .collect();
        ListReport {
            dir: self.dir.clone(),
            profiles,
            themes,
            errors,
        }
    }
}

/// Why a theme does not resolve.
enum ThemeError {
    Missing,
    Bad(LibraryError),
}

/// A theme in `gamepadii library list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeEntry {
    pub name: String,
    pub file: Origin,
    pub shipped: bool,
    pub line: usize,
    /// `null`: for any board.
    pub board: Option<String>,
    /// The theme passes the checks.
    pub ok: bool,
}

/// `gamepadii library list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListReport {
    /// The library directory; `null` without a home.
    pub dir: Option<PathBuf>,
    pub profiles: Vec<ListEntry>,
    pub themes: Vec<ThemeEntry>,
    /// Files that do not parse, and profiles and themes that do not pass
    /// the checks.
    pub errors: Vec<LibraryError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub file: Origin,
    pub shipped: bool,
    pub line: usize,
    pub base: Option<String>,
    pub label: String,
    /// The profile and its bases pass the checks.
    pub ok: bool,
}

/// A profile with its bases applied: `gamepadii library show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub name: String,
    pub file: Origin,
    pub shipped: bool,
    pub line: usize,
    /// The base, the base of the base, ...
    pub bases: Vec<String>,
    pub board: String,
    pub firmware_board: String,
    pub label: String,
    /// `null`: any input mode.
    pub input_mode: Option<InputMode>,
    /// The name system of the profile's file, for the human output. The
    /// JSON gives the GP2040 names in `action_name`.
    pub names: NameSystem,
    /// The theme of the lights; `null`: none.
    pub theme: Option<String>,
    /// Each button of the board, in the order of the board file.
    pub pins: Vec<ResolvedPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedPin {
    pub button: String,
    pub gpio: u8,
    /// `null`: no profile in the chain names the button, so it follows
    /// profile 1 of the pad.
    pub action: Option<i16>,
    pub action_name: Option<String>,
    pub button_mask: u32,
    pub dpad_mask: u32,
    /// The profile that gives the action; `null` with no action.
    pub from: Option<String>,
}

impl Resolved {
    /// The slot file for `gamepadii profile write`: the GPIO of each
    /// button that has an action, in GPIO order. The other pins are not in
    /// the file, so they follow profile 1.
    pub fn slot_file(&self) -> SlotFile {
        let mut file = SlotFile {
            board: Some(self.firmware_board.clone()),
            profile: None,
            label: Some(self.label.clone()),
            enabled: Some(true),
            pins: self
                .pins
                .iter()
                .filter_map(|p| {
                    Some(FilePin {
                        gpio: p.gpio,
                        action: Action(p.action?),
                        button_mask: p.button_mask,
                        dpad_mask: p.dpad_mask,
                    })
                })
                .collect(),
        };
        file.pins.sort_by_key(|p| p.gpio);
        file
    }
}

/// `gamepadii library pull`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullReport {
    /// Always `control`: READ_SLOT.
    pub via: crate::commands::Route,
    pub host: String,
    /// The name system of the actions in the KDL.
    pub names: NameSystem,
    pub board: String,
    pub firmware_board: String,
    /// The names of the profiles, profile 1 first.
    pub profiles: Vec<String>,
    /// The slot numbers that they come from.
    pub slots: Vec<u8>,
    pub skipped: Vec<pull::Skipped>,
    /// The KDL text.
    pub kdl: String,
    /// Where `--write` put it; `null` on standard output.
    pub file: Option<PathBuf>,
}

/// Writes the KDL of `report` to `path`; a bare file name goes into
/// `dir`, the library. It refuses to replace a file without `force`, and
/// refuses a name that another library file has.
pub fn save_pull(
    report: &mut PullReport,
    dir: Option<&Path>,
    path: &Path,
    force: bool,
) -> Result<()> {
    if path.extension().is_none_or(|x| x != "kdl") {
        return Err(Error::Usage(format!(
            "{}: the library reads only .kdl files",
            path.display()
        )));
    }
    let bare = path.parent().is_none_or(|p| p.as_os_str().is_empty());
    let path = match (bare, dir) {
        (true, Some(dir)) => dir.join(path),
        (true, None) => {
            return Err(Error::Usage(
                "no $HOME or $XDG_CONFIG_HOME: give a path for --write".into(),
            ));
        }
        (false, _) => path.to_path_buf(),
    };
    if path.exists() && !force {
        return Err(Error::Usage(format!(
            "{} is there already; --force replaces it",
            path.display()
        )));
    }
    let lib = Library::load(path.parent())?;
    for name in &report.profiles {
        if let Some(d) = lib
            .profiles
            .iter()
            .find(|d| d.profile.name.eq_ignore_ascii_case(name) && !d.origin.shipped())
            && d.origin != Origin::File(path.clone())
        {
            return Err(Error::Usage(format!(
                "profile {name:?} is in {} already; names must be unique",
                d.origin
            )));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
    }
    std::fs::write(&path, &report.kdl)
        .map_err(|e| Error::io(format!("writing {}", path.display()), e))?;
    report.file = Some(path);
    Ok(())
}

#[cfg(test)]
mod tests;
