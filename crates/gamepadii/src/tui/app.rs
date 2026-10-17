//! The TUI's state, with no terminal: keys come in through
//! [`App::on_key`], the worker's replies through [`App::on_reply`], the
//! hotplug tick through [`App::on_tick`], and the jobs for the worker go
//! out through [`App::take_jobs`]. The renderer reads the rest.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use gamepadii_core::Error;
use gamepadii_core::cache::DeviceCache;
use gamepadii_core::channel::Caps;
use gamepadii_core::commands::{ProfileName, Status, StatusDevice};
use gamepadii_core::discovery::Device;
use gamepadii_core::flow;
use gamepadii_core::mode::InputMode;

use super::keys::{Action, Pending};
use super::worker::{Job, Note, Reply};

/// The two lists on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Modes,
    Profiles,
}

/// A list's selected row and first row shown (the renderer keeps the
/// selection in view).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListPos {
    pub sel: usize,
    pub offset: usize,
}

/// What a yes at the prompt does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmed {
    Run(Job),
    /// Quit while an operation runs.
    Quit,
}

/// The yes/no prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    pub then: Confirmed,
}

/// A popup over the screen; it takes the keys until closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Confirm(Confirm),
}

/// The operation running on the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub job: Job,
    /// The latest progress: a wait with its countdown, or the last step.
    pub line: Option<String>,
    /// What to do on the pad, when the flow waits for the user.
    pub guide: Vec<String>,
}

/// How the last operation ended: its report, as the CLI prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub title: String,
    pub text: String,
    pub error: bool,
}

/// A device by what changes when it is replugged or re-enumerates.
type Identity = (u16, u16, Option<u16>, Option<u16>);

fn identity(d: &Device) -> Identity {
    (d.vid, d.pid, d.bus, d.devnum)
}

#[derive(Debug)]
pub struct App {
    /// The last `status`; `None` before the first one and with no pad.
    pub status: Option<Status>,
    /// Why there is no status: no pad on USB, or the error.
    pub status_error: Option<String>,
    /// The device cache, for what a pad without the channel can't say.
    pub cache: Option<Box<DeviceCache>>,
    pub focus: Pane,
    pub modes: ListPos,
    pub profiles: ListPos,
    pub overlay: Option<Overlay>,
    pub busy: Option<Busy>,
    pub outcome: Option<Outcome>,
    /// A one-off message for the status line.
    pub notice: Option<String>,
    pub should_quit: bool,
    /// Spinner frame, advanced by the run loop.
    pub frame: usize,
    keys: Pending,
    jobs: Vec<Job>,
    /// A `Status` or `Scan` is on the worker.
    reading: bool,
    /// What USB showed at the last look; `None` forces a `Status`.
    seen: Option<Vec<Identity>>,
    /// The selections were put on the current mode and profile.
    placed: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// The state before the first `status`, which it asks for.
    pub fn new() -> Self {
        let mut app = Self {
            status: None,
            status_error: None,
            cache: None,
            focus: Pane::Modes,
            modes: ListPos::default(),
            profiles: ListPos::default(),
            overlay: None,
            busy: None,
            outcome: None,
            notice: None,
            should_quit: false,
            frame: 0,
            keys: Pending::default(),
            jobs: Vec::new(),
            reading: false,
            seen: None,
            placed: false,
        };
        app.read();
        app
    }

    /// The jobs to hand the worker, oldest first.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.jobs)
    }

    /// The keys of a sequence typed so far, for the status line.
    pub fn pending_keys(&self) -> &'static str {
        self.keys.text()
    }

    /// The pad the flows act on, with what its channel says.
    pub fn pad(&self) -> Option<&StatusDevice> {
        let s = self.status.as_ref()?;
        let devices: Vec<Device> = s.devices.iter().map(|d| d.device.clone()).collect();
        s.devices.get(flow::picked(&devices)?)
    }

    pub fn in_webconfig(&self) -> bool {
        self.pad()
            .is_some_and(|p| p.device.mode == Some(InputMode::Webconfig))
    }

    /// The modes `mode` switches to, in list order.
    pub fn mode_items(&self) -> Vec<InputMode> {
        flow::targets().collect()
    }

    /// The mode the pad runs, when it is known.
    pub fn current_mode(&self) -> Option<InputMode> {
        self.pad().and_then(|p| p.device.mode)
    }

    /// Whether this build can boot `m`: `None` without the channel.
    pub fn boots(&self, m: InputMode) -> Option<bool> {
        let c = self.pad()?.control_channel.as_ref()?;
        Some(c.input_modes.contains(&m))
    }

    /// The profiles to pick from: the channel's (empty without one).
    pub fn profile_items(&self) -> Vec<ProfileName> {
        self.pad()
            .and_then(|p| p.control_channel.as_ref())
            .map(|c| c.profiles.clone())
            .unwrap_or_default()
    }

    pub fn active_profile(&self) -> Option<u8> {
        self.pad()
            .and_then(|p| p.control_channel.as_ref())
            .map(|c| c.active_profile.number)
    }

    /// Whether `profile switch` can work: the channel with SET_PROFILE.
    pub fn can_switch_profiles(&self) -> bool {
        self.pad()
            .and_then(|p| p.control_channel.as_ref())
            .is_some_and(|c| c.capabilities.has(Caps::SET_PROFILE))
    }

    /// Nothing is on the worker.
    pub fn idle(&self) -> bool {
        self.busy.is_none() && !self.reading
    }

    /// About once a second: look at USB when nothing else is running.
    pub fn on_tick(&mut self) {
        if self.idle() {
            self.reading = true;
            self.jobs.push(Job::Scan);
        }
    }

    /// Asks for a `status` (once: not while one is on its way).
    fn read(&mut self) {
        if !self.jobs.contains(&Job::Status) {
            self.reading = true;
            self.jobs.push(Job::Status);
        }
    }

    /// Starts an operation, unless one is running.
    fn start(&mut self, job: Job) {
        if let Some(b) = &self.busy {
            self.notice = Some(format!("wait: still {}", b.job.title()));
            return;
        }
        self.busy = Some(Busy {
            job: job.clone(),
            line: None,
            guide: Vec::new(),
        });
        self.outcome = None;
        self.notice = None;
        self.jobs.push(job);
    }

    pub fn on_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Scanned(devices) => {
                self.reading = false;
                let now: Vec<Identity> = devices.iter().map(identity).collect();
                if self.seen.as_ref() != Some(&now) {
                    self.read();
                }
            }
            Reply::Status { status, cache } => {
                self.reading = false;
                self.cache = cache;
                match status {
                    Ok(s) => {
                        self.seen = Some(s.devices.iter().map(|d| identity(&d.device)).collect());
                        self.status = Some(s);
                        self.status_error = None;
                    }
                    Err(e) => {
                        // No pad: wait for one. Anything else: try again
                        // at the next look.
                        self.seen = matches!(e, Error::NotFound(_)).then(Vec::new);
                        self.status = None;
                        self.status_error = Some(e.to_string());
                        self.placed = false;
                    }
                }
                self.place();
            }
            Reply::Progress(note) => {
                if let Some(b) = &mut self.busy {
                    match note {
                        Note::Guide(lines) => b.guide = lines,
                        Note::Waiting { text, left } => {
                            b.line = Some(format!("{text}, {left} s left"));
                        }
                        Note::Line(text) => b.line = Some(text),
                    }
                }
            }
            Reply::Done { job, result } => {
                self.busy = None;
                self.outcome = Some(match result {
                    Ok(text) => Outcome {
                        title: "Last result".into(),
                        text: text.trim_end().to_string(),
                        error: false,
                    },
                    Err(e) => Outcome {
                        title: format!("Failed: {}", job.title()),
                        text: e.to_string(),
                        error: true,
                    },
                });
                self.read();
            }
        }
    }

    /// Keeps the selections in range; after the first status of a pad,
    /// puts them on its mode and profile.
    fn place(&mut self) {
        let modes = self.mode_items();
        let profiles = self.profile_items();
        if !self.placed && self.status.is_some() {
            if let Some(i) = self
                .current_mode()
                .and_then(|m| modes.iter().position(|x| *x == m))
            {
                self.modes.sel = i;
            }
            if let Some(i) = self
                .active_profile()
                .and_then(|n| profiles.iter().position(|p| p.number == n))
            {
                self.profiles.sel = i;
            }
            self.placed = true;
        }
        self.modes.sel = self.modes.sel.min(modes.len().saturating_sub(1));
        self.profiles.sel = self.profiles.sel.min(profiles.len().saturating_sub(1));
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if let Some(Overlay::Confirm(c)) = &self.overlay {
            let ctrl_c =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            match key.code {
                _ if ctrl_c => self.should_quit = true,
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    let then = c.then.clone();
                    self.overlay = None;
                    match then {
                        Confirmed::Run(job) => self.start(job),
                        Confirmed::Quit => self.should_quit = true,
                    }
                }
                KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => self.overlay = None,
                _ => {}
            }
            return;
        }
        let action = self.keys.feed(key);
        if self.overlay == Some(Overlay::Help) {
            match action {
                Action::Help | Action::Back | Action::Close => self.overlay = None,
                Action::Quit => self.should_quit = true,
                _ => {}
            }
            return;
        }
        match action {
            Action::Move(n) => self.move_by(n),
            Action::Top => self.list_mut().sel = 0,
            Action::Bottom => {
                let n = self.list_len();
                self.list_mut().sel = n.saturating_sub(1);
            }
            Action::OtherList => {
                self.focus = match self.focus {
                    Pane::Modes => Pane::Profiles,
                    Pane::Profiles => Pane::Modes,
                }
            }
            Action::Select => match self.focus {
                Pane::Modes => self.ask_mode(),
                Pane::Profiles => self.switch_profile(false),
            },
            Action::Save => {
                if self.focus == Pane::Profiles {
                    self.switch_profile(true);
                } else {
                    self.notice = Some("S saves a profile: pick one in the profile list".into());
                }
            }
            Action::Webconfig => match self.pad() {
                None => self.notice = Some("no pad on USB".into()),
                Some(_) if self.in_webconfig() => self.start(Job::LeaveWebconfig),
                Some(_) => self.start(Job::Webconfig),
            },
            Action::Backup => match self.pad() {
                None => self.notice = Some("no pad on USB".into()),
                Some(_) => self.start(Job::Backup),
            },
            Action::Refresh => {
                if self.busy.is_some() {
                    self.notice = Some("the pad is read again when this is done".into());
                } else {
                    self.notice = None;
                    self.read();
                }
            }
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Back => {
                self.outcome = None;
                self.notice = None;
            }
            Action::Close => self.quit(),
            // Ctrl-c and :q quit whatever runs, as Ctrl-c does in the CLI.
            Action::Quit => self.should_quit = true,
            Action::None => {}
        }
    }

    /// Quits, asking first while an operation runs.
    fn quit(&mut self) {
        match &self.busy {
            Some(b) => {
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Quit".into(),
                    lines: vec![
                        format!("gamepadii is still {}.", b.job.title()),
                        "Quitting while it waits is safe: the flows only wait before a write"
                            .into(),
                        "or after one was read back. Check the pad with gamepadii status.".into(),
                    ],
                    then: Confirmed::Quit,
                }));
            }
            None => self.should_quit = true,
        }
    }

    fn list_len(&self) -> usize {
        match self.focus {
            Pane::Modes => self.mode_items().len(),
            Pane::Profiles => self.profile_items().len(),
        }
    }

    fn list_mut(&mut self) -> &mut ListPos {
        match self.focus {
            Pane::Modes => &mut self.modes,
            Pane::Profiles => &mut self.profiles,
        }
    }

    fn move_by(&mut self, n: i64) {
        let len = self.list_len();
        let list = self.list_mut();
        let to = (list.sel as i64 + n).clamp(0, len.saturating_sub(1) as i64);
        list.sel = to as usize;
    }

    /// ⏎ on a mode: the prompt, since `mode` saves it on the pad.
    fn ask_mode(&mut self) {
        let Some(pad) = self.pad() else {
            self.notice = Some("no pad on USB".into());
            return;
        };
        let Some(&target) = self.mode_items().get(self.modes.sel) else {
            return;
        };
        if pad.device.mode == Some(target) {
            self.notice = Some(format!("the pad is in {} already", target.label()));
            return;
        }
        if self.boots(target) == Some(false) {
            self.notice = Some(format!("this firmware build can't boot {}", target.label()));
            return;
        }
        let how = if pad.device.mode == Some(InputMode::Webconfig) {
            "gamepadii backs up the settings, saves inputMode on the pad and reboots it."
        } else if pad.control_channel.is_some() {
            "The pad saves the input mode in flash and restarts (over the control channel)."
        } else {
            "Without the control channel, gamepadii says which buttons to press and waits; \
             through web-config it backs up first, then saves the mode."
        };
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Switch the input mode".into(),
            lines: vec![
                format!("Switch the pad to {}?", target.label()),
                String::new(),
                how.into(),
                "It stays in that mode after a restart.".into(),
            ],
            then: Confirmed::Run(Job::Mode(target)),
        }));
    }

    /// ⏎ (RAM only, straight away) or S (saved, after the prompt) on a
    /// profile.
    fn switch_profile(&mut self, save: bool) {
        if self.pad().is_none() {
            self.notice = Some("no pad on USB".into());
            return;
        }
        if !self.can_switch_profiles() {
            self.notice = Some(
                "switching profiles needs the control channel (the gamepadii firmware)".into(),
            );
            return;
        }
        let Some(p) = self.profile_items().get(self.profiles.sel).cloned() else {
            return;
        };
        let job = Job::Profile {
            number: p.number,
            save,
        };
        if !save {
            self.start(job);
            return;
        }
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Save the profile".into(),
            lines: vec![
                format!("Switch to profile {} and save it in flash?", p.display()),
                String::new(),
                "The pad keeps it after a restart, as the profile hotkeys do.".into(),
            ],
            then: Confirmed::Run(job),
        }));
    }
}
