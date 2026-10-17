//! The TUI's worker thread: it runs the core's commands and guided flows
//! (which poll, sleep and wait for reboots) so the UI never blocks on the
//! pad. [`Job`]s go in over a channel, [`Reply`]s come back, with the
//! flows' progress as [`Note`]s on the way. No async runtime: one std
//! thread, std channels.
//!
//! [`perform`] is the whole of what a job does, against a flow [`Env`];
//! tests call it directly against a simulated pad, and [`spawn`] calls it
//! on the thread with the [`Env`] that [`WithEnv`] builds.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::commands::{self, Status};
use gamepadii_core::discovery::Device;
use gamepadii_core::flow::{self, Env, ProfileArg, Progress, Step};
use gamepadii_core::mode::{BootMode, InputMode};
use gamepadii_core::{Error, Result};

use crate::output::{self, Format, Style};
use crate::progress::waiting_for;

/// What the UI asks the worker for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// `status`, with a HELLO on the control channel (read only), and the
    /// device cache for what a pad without the channel can't say.
    Status,
    /// USB only (sysfs), for hotplug.
    Scan,
    /// `mode <MODE>`: saves the input mode (with a backup first).
    Mode(InputMode),
    /// `profile switch <N> [--save]`.
    Profile { number: u8, save: bool },
    /// `webconfig`: into web-config, waiting for the user without the
    /// channel.
    Webconfig,
    /// `webconfig --exit`: reboot back into the gamepad mode.
    LeaveWebconfig,
    /// `backup`: through web-config and back from a gamepad mode.
    Backup,
}

impl Job {
    /// An operation on the pad (shown as busy), not a read.
    pub fn is_operation(&self) -> bool {
        !matches!(self, Job::Status | Job::Scan)
    }

    /// What it is doing, for the status line: `switching to Keyboard`.
    pub fn title(&self) -> String {
        match self {
            Job::Status => "reading the pad".into(),
            Job::Scan => "looking at USB".into(),
            Job::Mode(m) => format!("switching to {}", m.label()),
            Job::Profile { number, save } => {
                let how = if *save { "saved" } else { "RAM only" };
                format!("switching to profile {number} ({how})")
            }
            Job::Webconfig => "going into web-config".into(),
            Job::LeaveWebconfig => "leaving web-config".into(),
            Job::Backup => "backing up".into(),
        }
    }

    /// Whether its [`Env`] needs the backups directory.
    fn backs_up(&self) -> bool {
        matches!(self, Job::Mode(_) | Job::Backup)
    }
}

/// Progress from a flow, owned, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// The user has to act on the pad: these lines say how.
    Guide(Vec<String>),
    /// Waiting: `waiting for the pad to come back`, and the seconds left.
    Waiting { text: String, left: u64 },
    /// A step done: `backed up to …`, `rebooting into web-config`.
    Line(String),
}

/// What the worker sends back.
#[derive(Debug)]
pub enum Reply {
    Scanned(Vec<Device>),
    Status {
        status: Result<Status>,
        /// The device cache of the pad's board (else the newest one), for
        /// the firmware of a pad without the channel.
        cache: Option<Box<DeviceCache>>,
    },
    Progress(Note),
    /// An operation finished: its report as `gamepadii <command>` prints
    /// it, or the error.
    Done {
        job: Job,
        result: Result<String>,
    },
}

/// Hands `f` the [`Env`] for one job, built on the worker thread (the
/// flows borrow everything in it); `backup` sets its backups directory.
pub type WithEnv = Box<dyn FnMut(bool, &mut dyn FnMut(&Env<'_>)) + Send>;

/// Turns the flows' steps into [`Note`]s.
struct Notes<'a>(&'a mut dyn FnMut(Note));

fn secs(d: std::time::Duration) -> u64 {
    d.as_secs_f64().ceil() as u64
}

impl Progress for Notes<'_> {
    fn step(&mut self, step: Step<'_>) {
        let note = match step {
            Step::Guide(g) => Note::Guide(g.lines()),
            Step::Waiting {
                awaiting,
                elapsed,
                timeout,
            } => Note::Waiting {
                text: waiting_for(awaiting),
                left: secs(timeout.saturating_sub(elapsed)),
            },
            Step::BackedUp(p) => Note::Line(format!("backed up to {}", p.display())),
            Step::Writing(m) => Note::Line(format!("saving input mode {}", m.label())),
            Step::Rebooting(m) => Note::Line(format!("rebooting into {}", m.label())),
            Step::CacheFailed(e) => Note::Line(format!("warning: device cache not saved: {e}")),
            Step::RestoringProfile(n) => {
                Note::Line(format!("switching back to profile {n} (RAM only)"))
            }
            Step::RestoreFailed { profile, error } => Note::Line(format!(
                "warning: profile {profile} not restored after the restart: {error}"
            )),
        };
        (self.0)(note);
    }
}

fn human() -> Style {
    Style::plain(Format::Human)
}

/// Runs `job` against `env`, sending the flows' progress to `note`.
pub fn perform(env: &Env, job: &Job, note: &mut dyn FnMut(Note)) -> Reply {
    let mut progress = Notes(note);
    let progress: &mut dyn Progress = &mut progress;
    let result = match job {
        Job::Scan => return Reply::Scanned(env.scan.scan()),
        Job::Status => return status(env),
        Job::Mode(m) => flow::mode(env, *m, true, progress).map(|r| output::mode(human(), &r)),
        Job::Profile { number, save } => {
            flow::profile_switch(env, &ProfileArg::Number(*number), *save, progress)
                .map(|r| output::profile_switch(human(), &r))
        }
        Job::Webconfig => {
            flow::webconfig(env, true, progress).map(|r| output::webconfig(human(), &r))
        }
        Job::LeaveWebconfig => flow::reboot_pad(env, BootMode::Gamepad, true, progress)
            .map(|r| output::reboot(human(), &r)),
        Job::Backup => backup(env, progress),
    };
    Reply::Done {
        job: job.clone(),
        result,
    }
}

fn status(env: &Env) -> Reply {
    let cache_dir = env.cache_dir.as_deref();
    let status = commands::status_with(env.scan, env.control, cache_dir);
    let board = status.as_ref().ok().and_then(|s| {
        let i = flow::picked(
            &s.devices
                .iter()
                .map(|d| d.device.clone())
                .collect::<Vec<_>>(),
        )?;
        let c = s.devices[i].control_channel.as_ref()?;
        c.firmware.board.clone()
    });
    let cache = cache_dir
        .and_then(|dir| match &board {
            Some(b) => cache::load_for(dir, Some(b)),
            None => cache::load_latest(dir),
        })
        .map(Box::new);
    Reply::Status { status, cache }
}

/// `backup`, into the backups directory, and the device cache refreshed
/// from it (as the CLI does).
fn backup(env: &Env, progress: &mut dyn Progress) -> Result<String> {
    let now = env.clock.wall();
    let work = |c: &gamepadii_core::client::Client| {
        let (report, cache) = match &env.backups_dir {
            Some(dir) => commands::backup_in_dir(c, dir, now)?,
            None => commands::backup_with_cache(c, None, now)?,
        };
        if let Some(cache) = cache {
            // The cache is a convenience: a failure isn't the backup's.
            let _ = cache::store_quietly(env.cache_dir.as_deref(), &cache);
        }
        Ok(report)
    };
    flow::through_webconfig(env, false, progress, work, |b| {
        format!("the backup is saved in {}", b.path.display())
    })
    .map(|(b, trip)| output::backup(human(), &b, trip.as_ref()))
}

/// Starts the worker. It runs one job at a time, in order, and stops when
/// the UI drops its [`Sender`].
pub fn spawn(mut with_env: WithEnv) -> (Sender<Job>, Receiver<Reply>) {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (outbox, replies) = mpsc::channel();
    thread::spawn(move || {
        for job in inbox {
            let progress = outbox.clone();
            let mut reply = None;
            with_env(job.backs_up(), &mut |env| {
                let mut note = |n| {
                    let _ = progress.send(Reply::Progress(n));
                };
                reply = Some(perform(env, &job, &mut note));
            });
            let reply = reply.unwrap_or_else(|| Reply::Done {
                job: job.clone(),
                result: Err(Error::Usage("no environment to run in".into())),
            });
            if outbox.send(reply).is_err() {
                break;
            }
        }
    });
    (jobs, replies)
}
