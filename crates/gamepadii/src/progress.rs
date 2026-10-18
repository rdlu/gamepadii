//! Progress for the guided flows, on stderr: the instructions, then a
//! spinner with a countdown while waiting (only for people at a terminal;
//! otherwise one line per wait). stdout stays for the result.
//!
//! Ctrl-C during a wait is safe: the flows only wait before a write (for
//! the user) or after one has been read back (for the reboot), never in
//! the middle of one.

use std::io::Write;
use std::time::Duration;

use gamepadii_core::flow::{Awaiting, Progress, Step};
use gamepadii_core::mode::BootMode;

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub struct Reporter<W: Write> {
    out: W,
    /// Redraw one line with a spinner, rather than print each wait once.
    spinner: bool,
    frame: usize,
    /// A spinner line is on screen.
    drawn: bool,
    /// The last wait announced (without a spinner).
    announced: Option<Awaiting>,
}

impl<W: Write> Reporter<W> {
    pub fn new(out: W, spinner: bool) -> Self {
        Self {
            out,
            spinner,
            frame: 0,
            drawn: false,
            announced: None,
        }
    }

    fn clear(&mut self) {
        if self.drawn {
            let _ = write!(self.out, "\r\x1b[K");
            self.drawn = false;
        }
    }

    fn line(&mut self, text: &str) {
        self.clear();
        let _ = writeln!(self.out, "{text}");
    }
}

/// `waiting for web-config`, for the spinner line.
pub fn waiting_for(a: Awaiting) -> String {
    match a {
        Awaiting::User { target: None } => "waiting for the pad in web-config".into(),
        Awaiting::User { target: Some(m) } => {
            format!(
                "waiting for the pad to show up in web-config or {}",
                m.label()
            )
        }
        Awaiting::Reboot(BootMode::Gamepad) => "waiting for the pad to come back".into(),
        Awaiting::Reboot(m) => format!("waiting for the pad to come back in {}", m.label()),
        Awaiting::Server => "waiting for the web configurator to answer".into(),
        Awaiting::Press => "waiting for a button press".into(),
        Awaiting::Release => "waiting for the release of the button".into(),
    }
}

/// What `input find-pin` tells the user before it waits.
pub fn push_button(scan: bool) -> Vec<String> {
    let mut lines = vec!["Push one button on the pad, then release it.".to_string()];
    if scan {
        lines.push("The pin scan is on: gamepadii also reads the pins with no action.".into());
    }
    lines
}

fn secs(d: Duration) -> u64 {
    d.as_secs_f64().ceil() as u64
}

impl<W: Write> Progress for Reporter<W> {
    fn step(&mut self, step: Step<'_>) {
        match step {
            Step::Guide(g) => {
                self.clear();
                for l in g.lines() {
                    self.line(&l);
                }
                self.announced = None;
            }
            Step::Waiting {
                awaiting,
                elapsed,
                timeout,
            } => {
                let left = secs(timeout.saturating_sub(elapsed));
                if self.spinner {
                    let f = FRAMES[self.frame % FRAMES.len()];
                    self.frame += 1;
                    let _ = write!(
                        self.out,
                        "\r\x1b[K{f} {}, {left} s left (Ctrl-C to stop)",
                        waiting_for(awaiting)
                    );
                    let _ = self.out.flush();
                    self.drawn = true;
                } else if self.announced != Some(awaiting) {
                    self.announced = Some(awaiting);
                    self.line(&format!("{} (up to {} s)", waiting_for(awaiting), left));
                }
            }
            Step::BackedUp(p) => self.line(&format!("backed up to {}", p.display())),
            Step::Writing(m) => self.line(&format!("saving input mode {}", m.label())),
            Step::Rebooting(m) => self.line(&format!("rebooting into {}", m.label())),
            Step::CacheFailed(e) => self.line(&format!("warning: device cache not saved: {e}")),
            Step::RestoringProfile(n) => {
                self.line(&format!("switching back to profile {n} (RAM only)"))
            }
            Step::RestoreFailed { profile, error } => self.line(&format!(
                "warning: profile {profile} not restored after the restart: {error}"
            )),
            Step::PushButton { scan } => {
                for l in push_button(scan) {
                    self.line(&l);
                }
                self.announced = None;
            }
        }
    }
}

impl<W: Write> Drop for Reporter<W> {
    fn drop(&mut self) {
        self.clear();
        let _ = self.out.flush();
    }
}
