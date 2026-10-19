//! Board files: the physical buttons of a pad model and their GPIOs. They
//! are shipped with gamepadii, so profiles name buttons, never GPIOs.

use std::collections::BTreeSet;

use super::parse::{Src, one_string, one_string_with_children};
use super::{LibraryError, Origin};

/// A pad model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// The name that profiles use: `haute42-s16`.
    pub name: String,
    /// `GP2040_BOARDCONFIG` of its firmware: `Haute42COSMOX`. A pull
    /// finds the board file by it.
    pub firmware_board: String,
    /// In the order of the file.
    pub buttons: Vec<BoardButton>,
}

/// One physical button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardButton {
    pub name: String,
    pub gpio: u8,
    /// The text printed on the case, when the button has one.
    pub label: Option<String>,
    /// Other names that a profile can use for the button: the other
    /// printed labels.
    pub aliases: Vec<String>,
}

impl BoardButton {
    fn named(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a == name)
    }
}

/// Buttons that a board file gave another name before a release, for a
/// clear error: (board, old name, new name).
const RENAMED: &[(&str, &str, &str)] = &[
    ("haute42-s16", "fn1", "touchpad"),
    ("haute42-s16", "fn2", "home"),
    ("haute42-s16", "fn3", "share"),
    ("haute42-s16", "fn4", "options"),
];

impl Board {
    /// The button called `name`, by its name or an alias.
    pub fn button(&self, name: &str) -> Option<&BoardButton> {
        self.buttons.iter().find(|b| b.named(name))
    }

    /// The new name of a button that had the name `old`.
    pub fn renamed(&self, old: &str) -> Option<&'static str> {
        RENAMED
            .iter()
            .find(|(b, o, _)| *b == self.name && *o == old)
            .map(|(_, _, new)| *new)
    }

    pub fn by_gpio(&self, gpio: u8) -> Option<&BoardButton> {
        self.buttons.iter().find(|b| b.gpio == gpio)
    }

    /// The board file in `text`; `file` names it in errors.
    pub fn parse(file: Origin, text: &str) -> Result<Board, LibraryError> {
        let src = Src::new(file, text);
        let doc = src.document()?;
        let [node] = doc.nodes() else {
            return Err(src.error_at(0, "a board file has exactly one board node"));
        };
        if node.name().value() != "board" {
            return Err(src.node_error(node, "a board file has exactly one board node"));
        }
        let name = one_string_with_children(&src, node)?;
        let mut firmware_board = None;
        let mut buttons: Vec<BoardButton> = Vec::new();
        let mut gpios = BTreeSet::new();
        for child in node.iter_children() {
            match child.name().value() {
                "firmware-board" => {
                    if firmware_board.is_some() {
                        return Err(src.node_error(child, "firmware-board is twice in the board"));
                    }
                    firmware_board = Some(one_string(&src, child)?);
                }
                "button" => {
                    let b = button(&src, child)?;
                    if buttons.iter().any(|o| o.name == b.name) {
                        return Err(src.node_error(child, &format!("button {} is twice", b.name)));
                    }
                    // A name or an alias names one button only.
                    for name in std::iter::once(&b.name).chain(&b.aliases) {
                        if let Some(o) = buttons.iter().find(|o| o.named(name)) {
                            return Err(src.node_error(
                                child,
                                &format!("{name:?} is a name of button {} already", o.name),
                            ));
                        }
                    }
                    if !gpios.insert(b.gpio) {
                        return Err(
                            src.node_error(child, &format!("GPIO {} has two buttons", b.gpio))
                        );
                    }
                    buttons.push(b);
                }
                other => {
                    return Err(src.node_error(
                        child,
                        &format!("unknown node {other:?} in a board (firmware-board, button)"),
                    ));
                }
            }
        }
        let Some(firmware_board) = firmware_board else {
            return Err(src.node_error(node, "the board has no firmware-board"));
        };
        Ok(Board {
            name,
            firmware_board,
            buttons,
        })
    }
}

/// `button NAME gpio=N [label="TEXT"] [alias="NAME" ...] [size="TEXT"]`.
fn button(src: &Src, node: &kdl::KdlNode) -> Result<BoardButton, LibraryError> {
    let name = one_string(src, node)?;
    if super::parse::is_reserved_pin_name(&name) {
        return Err(src.node_error(node, &format!("{name:?} is not a button name")));
    }
    let mut gpio = None;
    let mut label = None;
    let mut aliases: Vec<String> = Vec::new();
    for e in node.entries().iter().filter(|e| e.name().is_some()) {
        let key = e.name().map(|n| n.value()).unwrap_or_default();
        match key {
            "gpio" => {
                let n = e
                    .value()
                    .as_integer()
                    .and_then(|n| u8::try_from(n).ok())
                    .filter(|n| *n < 48);
                gpio = Some(n.ok_or_else(|| src.entry_error(e, "gpio: a number from 0 to 47"))?);
            }
            "label" => {
                label = Some(
                    e.value()
                        .as_string()
                        .ok_or_else(|| src.entry_error(e, "label: a string"))?
                        .to_string(),
                );
            }
            "alias" => {
                let a = e
                    .value()
                    .as_string()
                    .filter(|a| !a.is_empty())
                    .ok_or_else(|| src.entry_error(e, "alias: a string"))?;
                if a == name
                    || aliases.iter().any(|o| o == a)
                    || super::parse::is_reserved_pin_name(a)
                {
                    return Err(src.entry_error(e, &format!("alias {a:?}: not a new name")));
                }
                aliases.push(a.to_string());
            }
            "size" => {}
            _ => return Err(src.entry_error(e, &format!("unknown property {key:?}"))),
        }
    }
    let gpio = gpio.ok_or_else(|| src.node_error(node, &format!("button {name} has no gpio")))?;
    Ok(BoardButton {
        name,
        gpio,
        label,
        aliases,
    })
}

/// The board files that gamepadii ships.
pub fn shipped() -> Vec<Board> {
    SHIPPED
        .iter()
        .map(|(file, text)| {
            Board::parse(Origin::Shipped(file), text).expect("the shipped board files parse")
        })
        .collect()
}

const SHIPPED: &[(&str, &str)] = &[(
    "boards/haute42-s16.kdl",
    include_str!("../../data/boards/haute42-s16.kdl"),
)];

/// The shipped board called `name`.
pub fn named(name: &str) -> Option<Board> {
    shipped().into_iter().find(|b| b.name == name)
}

/// The shipped board for the firmware's `GP2040_BOARDCONFIG`.
pub fn for_firmware(firmware_board: &str) -> Option<Board> {
    shipped()
        .into_iter()
        .find(|b| b.firmware_board == firmware_board)
}
