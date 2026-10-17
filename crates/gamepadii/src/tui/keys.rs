//! The vim-first keymap: crossterm key events in, [`Action`]s out, with the
//! pending state (`g`, `:`) kept between keys. Arrows always work too.
//! [`BINDINGS`] lists every binding for the help overlay; tests hold it to
//! the keymap.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What a complete key sequence asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `j` / `k` (negative is up).
    Move(i64),
    /// `gg`.
    Top,
    /// `G`.
    Bottom,
    /// `h` / `l` / `Tab`: the other list (modes, profiles).
    OtherList,
    /// `⏎`: switch to the selected mode (after a prompt) or profile (RAM
    /// only).
    Select,
    /// `S`: switch to the selected profile and save it (after a prompt).
    Save,
    /// `w`: into web-config, or back out of it.
    Webconfig,
    /// `b`: back up the settings.
    Backup,
    /// `r`: read the pad again.
    Refresh,
    /// `?`.
    Help,
    /// `Esc`: close a popup, else clear the last result.
    Back,
    /// `q`: close a popup, else quit.
    Close,
    /// `:q⏎`, `Ctrl-c`: quit from anywhere.
    Quit,
    /// Nothing: an unbound key, or a key that only extends a pending
    /// sequence.
    None,
}

/// One entry of the help overlay: key sequences, each with the action it
/// gives, and what they do. The keymap's tests feed every sequence listed
/// here and check its action, and check every action is listed, so the help
/// can't drift from [`Pending::feed`].
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub section: &'static str,
    /// As shown: `j`, `gg`, `⏎`, `:q⏎`. A space separates keys; a token is
    /// one named key (`Home`, `Ctrl-c`, `Esc`, …), else each of its
    /// characters is a key.
    pub keys: &'static [(&'static str, Action)],
    pub help: &'static str,
}

const fn bind(
    section: &'static str,
    keys: &'static [(&'static str, Action)],
    help: &'static str,
) -> Binding {
    Binding {
        section,
        keys,
        help,
    }
}

const MOVE: &str = "Move";
const PAD: &str = "The pad";
const OTHER: &str = "Other";

/// Every binding, in help order.
pub const BINDINGS: &[Binding] = &[
    bind(
        MOVE,
        &[("j", Action::Move(1)), ("↓", Action::Move(1))],
        "down",
    ),
    bind(
        MOVE,
        &[("k", Action::Move(-1)), ("↑", Action::Move(-1))],
        "up",
    ),
    bind(MOVE, &[("gg", Action::Top), ("Home", Action::Top)], "top"),
    bind(
        MOVE,
        &[("G", Action::Bottom), ("End", Action::Bottom)],
        "bottom",
    ),
    bind(
        MOVE,
        &[
            ("h", Action::OtherList),
            ("l", Action::OtherList),
            ("←", Action::OtherList),
            ("→", Action::OtherList),
            ("Tab", Action::OtherList),
        ],
        "the other list (input modes, profiles)",
    ),
    bind(
        PAD,
        &[("⏎", Action::Select)],
        "switch to the selected mode (asks first: it saves) or profile (RAM only)",
    ),
    bind(
        PAD,
        &[("S", Action::Save)],
        "switch to the selected profile and save it (asks first)",
    ),
    bind(
        PAD,
        &[("w", Action::Webconfig)],
        "reboot into web-config, or back to the gamepad mode",
    ),
    bind(PAD, &[("b", Action::Backup)], "back up every setting"),
    bind(PAD, &[("r", Action::Refresh)], "read the pad again"),
    bind(OTHER, &[("?", Action::Help)], "this help"),
    bind(
        OTHER,
        &[("Esc", Action::Back)],
        "close a popup, else clear the last result",
    ),
    bind(OTHER, &[("q", Action::Close)], "close a popup, else quit"),
    bind(
        OTHER,
        &[(":q⏎", Action::Quit), ("Ctrl-c", Action::Quit)],
        "quit",
    ),
];

/// The yes/no prompt's keys: it takes every key.
pub const CONFIRM_KEYS: &[(&str, &str)] = &[("y  ⏎", "yes, do it"), ("n  Esc  q", "no")];

/// Keys typed so far that don't make an action yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    prefix: Prefix,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Prefix {
    #[default]
    None,
    /// `g`, waiting for `g`.
    G,
    /// `:`, waiting for `q`.
    Colon,
    /// `:q`, waiting for `⏎`.
    ColonQ,
}

impl Pending {
    /// The keys typed so far, for the status line: `g`, `:q`.
    pub fn text(&self) -> &'static str {
        match self.prefix {
            Prefix::None => "",
            Prefix::G => "g",
            Prefix::Colon => ":",
            Prefix::ColonQ => ":q",
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prefix == Prefix::None
    }

    /// Feeds one key; returns the action it completes, if any.
    pub fn feed(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            // Ctrl-c quits: in raw mode it would otherwise do nothing at all.
            *self = Self::default();
            return Action::Quit;
        }
        if key.code == KeyCode::Esc {
            // Esc cancels a pending sequence; on its own it goes back.
            let pending = !self.is_empty();
            *self = Self::default();
            return if pending { Action::None } else { Action::Back };
        }
        match std::mem::take(&mut self.prefix) {
            Prefix::G => {
                return match key.code {
                    KeyCode::Char('g') => Action::Top,
                    _ => Action::None,
                };
            }
            Prefix::Colon => {
                if key.code == KeyCode::Char('q') {
                    self.prefix = Prefix::ColonQ;
                }
                return Action::None;
            }
            Prefix::ColonQ => {
                return if key.code == KeyCode::Enter {
                    Action::Quit
                } else {
                    Action::None
                };
            }
            Prefix::None => {}
        }
        if ctrl {
            return Action::None;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::Move(1),
            KeyCode::Char('k') | KeyCode::Up => Action::Move(-1),
            KeyCode::Char('g') => {
                self.prefix = Prefix::G;
                Action::None
            }
            KeyCode::Char('G') | KeyCode::End => Action::Bottom,
            KeyCode::Home => Action::Top,
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                Action::OtherList
            }
            KeyCode::BackTab => Action::OtherList,
            KeyCode::Enter => Action::Select,
            KeyCode::Char('S') => Action::Save,
            KeyCode::Char('w') => Action::Webconfig,
            KeyCode::Char('b') => Action::Backup,
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Char('?') => Action::Help,
            KeyCode::Char('q') => Action::Close,
            KeyCode::Char(':') => {
                self.prefix = Prefix::Colon;
                Action::None
            }
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(keys: &str) -> Vec<Action> {
        let mut p = Pending::default();
        keys.chars()
            .map(|c| p.feed(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            .collect()
    }

    #[test]
    fn g_sequences() {
        assert_eq!(feed("gg").last(), Some(&Action::Top));
        assert_eq!(feed("gx").last(), Some(&Action::None));
        // A stray key after `g` drops it: `j` then moves.
        assert_eq!(feed("gxj").last(), Some(&Action::Move(1)));
    }

    #[test]
    fn colon_q_needs_enter() {
        let mut p = Pending::default();
        for c in [':', 'q'] {
            assert_eq!(
                p.feed(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
                Action::None
            );
        }
        assert_eq!(p.text(), ":q");
        assert_eq!(
            p.feed(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Quit
        );
        assert!(p.is_empty());
    }

    #[test]
    fn esc_cancels_a_sequence_else_goes_back() {
        let mut p = Pending::default();
        p.feed(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(p.text(), "g");
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(p.feed(esc), Action::None);
        assert!(p.is_empty());
        assert_eq!(p.feed(esc), Action::Back);
    }

    #[test]
    fn pad_keys_are_case_sensitive() {
        // `s` does nothing: saving to flash takes the capital.
        assert_eq!(feed("sS"), [Action::None, Action::Save]);
        assert_eq!(
            feed("wbr?q"),
            [
                Action::Webconfig,
                Action::Backup,
                Action::Refresh,
                Action::Help,
                Action::Close
            ]
        );
    }

    /// A help token as key events: a named key, else one key per character.
    fn token_keys(token: &str) -> Vec<KeyEvent> {
        let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
        if let Some(c) = token.strip_prefix("Ctrl-") {
            let c = c.chars().next().unwrap();
            return vec![KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)];
        }
        let named = match token {
            "Home" => Some(KeyCode::Home),
            "End" => Some(KeyCode::End),
            "Tab" => Some(KeyCode::Tab),
            "Esc" => Some(KeyCode::Esc),
            _ => None,
        };
        if let Some(code) = named {
            return vec![plain(code)];
        }
        token
            .chars()
            .map(|c| {
                plain(match c {
                    '⏎' => KeyCode::Enter,
                    '←' => KeyCode::Left,
                    '↓' => KeyCode::Down,
                    '↑' => KeyCode::Up,
                    '→' => KeyCode::Right,
                    c => KeyCode::Char(c),
                })
            })
            .collect()
    }

    #[test]
    fn help_table_matches_the_keymap() {
        for b in BINDINGS {
            for (shown, action) in b.keys {
                let mut p = Pending::default();
                let keys: Vec<KeyEvent> = shown.split(' ').flat_map(token_keys).collect();
                let last = keys.into_iter().map(|k| p.feed(k)).last();
                assert_eq!(last, Some(*action), "{shown}");
                assert!(p.is_empty(), "{shown}");
            }
        }
    }

    #[test]
    fn every_action_is_in_the_help() {
        // A new action fails to compile here until it is sorted below.
        let listed = |a: Action| match a {
            Action::Move(_)
            | Action::Top
            | Action::Bottom
            | Action::OtherList
            | Action::Select
            | Action::Save
            | Action::Webconfig
            | Action::Backup
            | Action::Refresh
            | Action::Help
            | Action::Back
            | Action::Close
            | Action::Quit => true,
            Action::None => false,
        };
        let all = [
            Action::Move(1),
            Action::Move(-1),
            Action::Top,
            Action::Bottom,
            Action::OtherList,
            Action::Select,
            Action::Save,
            Action::Webconfig,
            Action::Backup,
            Action::Refresh,
            Action::Help,
            Action::Back,
            Action::Close,
            Action::Quit,
        ];
        for a in all.into_iter().filter(|&a| listed(a)) {
            assert!(
                BINDINGS.iter().any(|b| b.keys.iter().any(|(_, x)| *x == a)),
                "{a:?} is not in the help"
            );
        }
    }
}
