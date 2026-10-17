//! `gamepadii display layout`: which drawing the OLED shows for the
//! buttons (`buttonLayout` / `buttonLayoutRight` in `getDisplayOptions`).
//!
//! Haute42COSMOX is one firmware for the whole Haute42 family, so its
//! default "Board Defined" layout (27/31) is the original 12-button
//! drawing. The other models have their own "Board Defined Alternate"
//! drawings in `configs/Haute42COSMOX/BoardConfig.h` (v0.7.12), named in
//! the comments above each one; [`HAUTE42`] is that table. The S16/G16 one
//! (Alternate 2, 36/42) was checked on an S16.
//!
//! Elsewhere the model names mean nothing, but `--left N --right N` sets
//! the protobuf values directly.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::api::{self, DisplayOptions, OptionsWrite, PinMappings};
use crate::client::Client;
use crate::commands::{self, Firmware};
use crate::error::{Error, Result};

/// The board config the [`HAUTE42`] table is for.
pub const HAUTE42_BOARD: &str = "Haute42COSMOX";

/// The highest `ButtonLayout` (left) value in v0.7.12's `enums.proto`
/// (`BUTTON_LAYOUT_BOARD_DEFINED_ALT7_A`).
pub const MAX_LEFT: i64 = 41;
/// The highest `ButtonLayoutRight` value (`BUTTON_LAYOUT_BOARD_DEFINED_ALT7_B`).
pub const MAX_RIGHT: i64 = 47;

/// One of a board's own drawings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoardLayout {
    /// What `display layout` takes: `default`, `alt0`..`alt5`.
    pub key: &'static str,
    /// As the web configurator names it: `Board Defined Alternate 2`.
    pub label: &'static str,
    /// The `buttonLayout` value.
    pub left: i64,
    /// The `buttonLayoutRight` value.
    pub right: i64,
    /// The models BoardConfig.h names for it (also accepted as the layout).
    pub models: &'static [&'static str],
    /// The drawing, from the comment in BoardConfig.h.
    pub drawing: &'static str,
}

const fn layout(
    key: &'static str,
    label: &'static str,
    left: i64,
    right: i64,
    models: &'static [&'static str],
    drawing: &'static str,
) -> BoardLayout {
    BoardLayout {
        key,
        label,
        left,
        right,
        models,
        drawing,
    }
}

/// The Haute42COSMOX drawings, from `configs/Haute42COSMOX/BoardConfig.h`
/// at v0.7.12 (ALT6 and ALT7 are not defined there).
pub const HAUTE42: &[BoardLayout] = &[
    layout(
        "default",
        "Board Defined",
        27,
        31,
        &[],
        "Stickless, 12 buttons",
    ),
    layout(
        "alt0",
        "Board Defined Alternate 0",
        34,
        40,
        &["G13", "S13"],
        "Stickless 13 (1)",
    ),
    layout(
        "alt1",
        "Board Defined Alternate 1",
        35,
        41,
        &["T13", "M13", "M13+"],
        "Stickless 13 (2)",
    ),
    layout(
        "alt2",
        "Board Defined Alternate 2",
        36,
        42,
        &["G16", "S16"],
        "Stickless 16 (1)",
    ),
    layout(
        "alt3",
        "Board Defined Alternate 3",
        37,
        43,
        &["T16", "M16", "M16+"],
        "Stickless 16 (2)",
    ),
    layout(
        "alt4",
        "Board Defined Alternate 4",
        38,
        44,
        &[],
        "Stickless 16 R",
    ),
    layout(
        "alt5",
        "Board Defined Alternate 5",
        39,
        45,
        &["E16"],
        "Stickless 16 Southpaw",
    ),
];

/// The GPIOs the S16/G16 drawing (`DEFAULT_BOARD_LAYOUT_A_ALT2` and
/// `_B_ALT2`) has a button for. Every one of them mapped to a button looks
/// like a 16-button pad.
pub const SIXTEEN_BUTTON_PINS: &[u8] = &[
    27, 5, 3, 4, 26, 18, 2, 19, 21, 20, 16, 17, 10, 11, 12, 13, 6, 7, 8, 9,
];

/// `GpioAction` values that press a button (`BUTTON_PRESS_UP` ..
/// `BUTTON_PRESS_DDI_RIGHT`).
const BUTTON_PRESS: std::ops::RangeInclusive<i64> = 1..=23;

fn is_haute42(board: Option<&str>) -> bool {
    board == Some(HAUTE42_BOARD)
}

/// The board's drawing with these values, if it is one.
fn board_layout(board: Option<&str>, left: i64, right: i64) -> Option<BoardLayout> {
    if !is_haute42(board) {
        return None;
    }
    HAUTE42
        .iter()
        .copied()
        .find(|l| l.left == left && l.right == right)
}

/// What the OLED is set to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Current {
    /// `buttonLayout`.
    pub left: Option<i64>,
    /// `buttonLayoutRight`.
    pub right: Option<i64>,
    /// The board's drawing with these two values (`null` for anything
    /// else, or a board without a table).
    pub layout: Option<BoardLayout>,
}

impl Current {
    pub fn new(board: Option<&str>, d: &DisplayOptions) -> Self {
        let layout = match (d.button_layout, d.button_layout_right) {
            (Some(l), Some(r)) => board_layout(board, l, r),
            _ => None,
        };
        Self {
            left: d.button_layout,
            right: d.button_layout_right,
            layout,
        }
    }
}

/// What `display layout` was asked to set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutArg {
    /// A model (`s16`) or a layout key (`alt2`), any case.
    Named(String),
    /// `--left N --right N`.
    Raw { left: i64, right: i64 },
}

/// The layout `arg` names on `board`. Model names and keys are Haute42's;
/// raw values work anywhere, within v0.7.12's ranges.
pub fn resolve(arg: &LayoutArg, board: Option<&str>) -> Result<(i64, i64)> {
    match arg {
        LayoutArg::Raw { left, right } => {
            if !(0..=MAX_LEFT).contains(left) || !(0..=MAX_RIGHT).contains(right) {
                return Err(Error::Usage(format!(
                    "--left takes 0-{MAX_LEFT} and --right 0-{MAX_RIGHT} (GP2040-CE \
                     v0.7.12's ButtonLayout and ButtonLayoutRight); got {left} and {right}"
                )));
            }
            Ok((*left, *right))
        }
        LayoutArg::Named(name) => {
            if !is_haute42(board) {
                return Err(Error::Usage(format!(
                    "the layout names are for {HAUTE42_BOARD} boards, and this one is {}; \
                     set the values with --left N --right N",
                    board.unwrap_or("unknown")
                )));
            }
            let wanted = name.trim().to_ascii_lowercase().replace("plus", "+");
            HAUTE42
                .iter()
                .find(|l| {
                    l.key == wanted || l.models.iter().any(|m| m.to_ascii_lowercase() == wanted)
                })
                .map(|l| (l.left, l.right))
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "no {HAUTE42_BOARD} layout called {name:?}; the choices are {} \
                         (`gamepadii display layout` lists them)",
                        names().join(", ")
                    ))
                })
        }
    }
}

/// What [`resolve`] can tell before the board is known: an unknown name or
/// an out-of-range value, so a typo never costs a trip into web-config.
pub fn precheck(arg: &LayoutArg) -> Result<()> {
    resolve(arg, Some(HAUTE42_BOARD)).map(drop)
}

/// Every name [`resolve`] takes on a Haute42, models first.
pub fn names() -> Vec<String> {
    let models = HAUTE42
        .iter()
        .flat_map(|l| l.models.iter().map(|m| m.to_ascii_lowercase()));
    models
        .chain(HAUTE42.iter().map(|l| l.key.to_string()))
        .collect()
}

/// One choice, and whether the pad has it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Choice {
    #[serde(flatten)]
    pub layout: BoardLayout,
    pub current: bool,
}

/// `gamepadii display layout`: what the OLED draws, and the choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutStatus {
    /// The board config: `Haute42COSMOX`.
    pub board: Option<String>,
    pub current: Current,
    /// The board's own drawings; empty for a board gamepadii has no table
    /// for.
    pub choices: Vec<Choice>,
}

fn read_current(client: &Client) -> Result<(Firmware, Current)> {
    let fw = Firmware::from(client.firmware());
    let d = api::display_options(client.transport())?;
    let current = Current::new(fw.board.as_deref(), &d);
    Ok((fw, current))
}

/// [`LayoutStatus`] for the pad behind `client` (one read).
pub fn layout_status(client: &Client) -> Result<LayoutStatus> {
    let (fw, current) = read_current(client)?;
    let choices = if is_haute42(fw.board.as_deref()) {
        HAUTE42
            .iter()
            .map(|&layout| Choice {
                layout,
                current: current.left == Some(layout.left) && current.right == Some(layout.right),
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(LayoutStatus {
        board: fw.board,
        current,
        choices,
    })
}

/// `gamepadii display layout <LAYOUT>`: what was set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutReport {
    pub board: Option<String>,
    pub before: Current,
    /// As read back from the pad.
    pub after: Current,
    /// `null` when it already had that layout (nothing written).
    pub write: Option<OptionsWrite>,
    /// The backup taken before writing (`null` with `--no-backup`, or when
    /// nothing was written).
    pub backup: Option<PathBuf>,
}

/// Sets the layout `arg` names: a backup into `backups_dir` first (unless
/// `None`), then [`api::set_display_options`] with its read-back check.
/// Nothing is backed up or written when the pad has that layout already.
pub fn set_layout(
    client: &Client,
    arg: &LayoutArg,
    backups_dir: Option<&Path>,
    now: SystemTime,
) -> Result<LayoutReport> {
    let (fw, before) = read_current(client)?;
    let board = fw.board.as_deref();
    let (left, right) = resolve(arg, board)?;
    if before.left == Some(left) && before.right == Some(right) {
        return Ok(LayoutReport {
            board: fw.board.clone(),
            before,
            after: before,
            write: None,
            backup: None,
        });
    }
    let backup = match backups_dir {
        Some(dir) => {
            let bundle = commands::backup_bundle(client, now)?;
            let path = dir.join(commands::backup_file_name(&bundle, now));
            commands::write_json(&bundle, &path)?;
            Some(path)
        }
        None => None,
    };
    let write = api::set_display_options(client.transport(), |d| {
        d.button_layout = Some(left);
        d.button_layout_right = Some(right);
    })?;
    let (_, after) = read_current(client)?;
    Ok(LayoutReport {
        board: fw.board.clone(),
        before,
        after,
        write: Some(write),
        backup,
    })
}

/// For `info`: a Haute42COSMOX still on the generic 12-button drawing,
/// whose base profile maps every button of the S16/G16 drawing, most
/// likely wants that drawing. Only a hint: the stock pin mapping is the
/// same for every Haute42 model.
pub fn generic_layout_hint(
    board: Option<&str>,
    display: &DisplayOptions,
    base: &PinMappings,
) -> Option<String> {
    let generic = HAUTE42[0];
    if !is_haute42(board)
        || display.button_layout != Some(generic.left)
        || display.button_layout_right != Some(generic.right)
    {
        return None;
    }
    let pins = base.pins();
    let sixteen = SIXTEEN_BUTTON_PINS.iter().all(|p| {
        pins.get(p)
            .is_some_and(|m| BUTTON_PRESS.contains(&m.action))
    });
    sixteen.then(|| {
        format!(
            "the OLED draws the generic 12-button layout ({}/{}), but the pins map 16 \
             buttons; on an S16 or G16: gamepadii display layout s16",
            generic.left, generic.right
        )
    })
}
