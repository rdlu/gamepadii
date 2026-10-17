//! The TUI without a terminal: keys against the app state, with the jobs
//! it asks for run through the worker's `perform` against a simulated pad
//! (`sim::SimPad`, a manual clock: no real sleeps), and screens rendered
//! through ratatui's `TestBackend`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gamepadii::tui::App;
use gamepadii::tui::app::{Confirmed, Overlay, Pane};
use gamepadii::tui::ui;
use gamepadii::tui::worker::{self, Job, Note, Reply, WithEnv};
use gamepadii_core::cache::{self, DeviceCache};
use gamepadii_core::commands::{self, Firmware};
use gamepadii_core::discovery::{Device, Scan};
use gamepadii_core::flow::{Env, Timeouts};
use gamepadii_core::mode::InputMode;
use gamepadii_core::sim::SimPad;
use gamepadii_core::wait::ManualClock;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::Value;

/// 2026-10-17T16:57:03Z.
const CAPTURED: u64 = 1_792_256_223;

fn s16_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/api/s16-v0.7.12")
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(s16_dir().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A pad, a clock, and the directories the flows write to.
struct Rig {
    pad: Arc<SimPad>,
    clock: Arc<ManualClock>,
    dirs: Arc<tempfile::TempDir>,
}

impl Rig {
    fn new(pad: SimPad) -> Self {
        let dirs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dirs.path().join("cache")).unwrap();
        Self {
            pad: Arc::new(pad),
            clock: Arc::new(ManualClock::at(CAPTURED)),
            dirs: Arc::new(dirs),
        }
    }

    /// The S16 on stock GP2040-CE, in `mode`.
    fn stock(mode: InputMode) -> Self {
        Self::new(SimPad::new(s16_dir(), mode))
    }

    /// The S16 on the gamepadii firmware, with the control channel.
    fn channel(mode: InputMode) -> Self {
        Self::new(SimPad::new(s16_dir(), mode).with_channel())
    }

    /// With the device cache an earlier web-config read left.
    fn with_cache(self) -> Self {
        let fw: gamepadii_core::api::FirmwareVersion =
            serde_json::from_value(fixture("getFirmwareVersion")).unwrap();
        let c = DeviceCache::new(
            Firmware::from(&fw),
            std::time::UNIX_EPOCH + Duration::from_secs(CAPTURED),
            fixture("getGamepadOptions"),
            fixture("getPinMappings"),
            fixture("getProfileOptions"),
        );
        cache::store(&self.cache_dir(), &c).unwrap();
        self
    }

    fn cache_dir(&self) -> PathBuf {
        self.dirs.path().join("cache")
    }

    fn backups_dir(&self) -> PathBuf {
        self.dirs.path().join("backups")
    }

    /// What `main` hands the worker, for this pad.
    fn with_env(&self) -> WithEnv {
        let (pad, clock, dirs) = (self.pad.clone(), self.clock.clone(), self.dirs.clone());
        Box::new(move |_backup, f| {
            let p = pad.clone();
            let open = move || p.open();
            let p = pad.clone();
            let control = move |d: &Device| p.open_control(d);
            let env = Env {
                scan: &*pad,
                clock: &*clock,
                open: &open,
                control: Some(&control),
                cache_dir: Some(dirs.path().join("cache")),
                backups_dir: Some(dirs.path().join("backups")),
                url: "http://192.168.7.1/".into(),
                timeouts: Timeouts::default(),
                poll: Duration::from_millis(200),
            };
            f(&env);
        })
    }

    /// Runs one job as the worker would, collecting its progress.
    fn perform(&self, job: &Job) -> (Vec<Note>, Reply) {
        let mut notes = Vec::new();
        let mut reply = None;
        (self.with_env())(false, &mut |env| {
            reply = Some(worker::perform(env, job, &mut |n| notes.push(n)));
        });
        (notes, reply.unwrap())
    }

    /// Runs every job the app asks for, feeding back the replies, until it
    /// asks for no more.
    fn pump(&self, app: &mut App) {
        for _ in 0..20 {
            let jobs = app.take_jobs();
            if jobs.is_empty() {
                return;
            }
            for job in jobs {
                let (notes, reply) = self.perform(&job);
                for n in notes {
                    app.on_reply(Reply::Progress(n));
                }
                app.on_reply(reply);
            }
        }
        panic!("the app kept asking for jobs");
    }

    /// An app that has read this pad.
    fn app(&self) -> App {
        let mut app = App::new();
        self.pump(&mut app);
        app
    }
}

/// No pad on USB.
struct Nothing;

impl Scan for Nothing {
    fn scan(&self) -> Vec<Device> {
        Vec::new()
    }
}

fn no_pad() -> App {
    let mut app = App::new();
    assert_eq!(app.take_jobs(), [Job::Status]);
    app.on_reply(Reply::Status {
        status: commands::status_with(&Nothing, None, None),
        cache: None,
    });
    app
}

/// The screen as text, one line per row, trailing blanks trimmed.
fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if skip == 0 {
                line.push_str(cell.symbol());
            }
            skip = std::cmp::max(skip, ratatui::text::Span::raw(cell.symbol()).width())
                .saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn enter(app: &mut App) {
    app.on_key(key(KeyCode::Enter));
}

/// Selects `m` in the mode list.
fn select_mode(app: &mut App, m: InputMode) {
    keys(app, "gg");
    let i = app.mode_items().iter().position(|x| *x == m).unwrap();
    for _ in 0..i {
        keys(app, "j");
    }
}

// Screens.

#[test]
fn screen_no_pad() {
    insta::assert_snapshot!(render(&mut no_pad(), 100, 30));
}

#[test]
fn screen_stock_firmware_without_the_channel() {
    let mut app = Rig::stock(InputMode::Xinput).with_cache().app();
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn screen_channel_present() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "l");
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn screen_operation_in_progress() {
    // Stock firmware, no cache: switching to Keyboard waits for the user,
    // with what to press in the result panel.
    let rig = Rig::stock(InputMode::Xinput);
    let mut app = rig.app();
    select_mode(&mut app, InputMode::Keyboard);
    enter(&mut app);
    keys(&mut app, "y");
    let jobs = app.take_jobs();
    assert_eq!(jobs, [Job::Mode(InputMode::Keyboard)]);
    let (notes, _) = rig.perform(&jobs[0]);
    // Up to the first countdown: the flow is still waiting there.
    let first_wait = notes
        .iter()
        .position(|n| matches!(n, Note::Waiting { .. }))
        .unwrap();
    for n in notes.into_iter().take(first_wait + 1) {
        app.on_reply(Reply::Progress(n));
    }
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn screen_help_overlay() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "?");
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn screen_mode_prompt() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    select_mode(&mut app, InputMode::Keyboard);
    enter(&mut app);
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn screen_after_a_profile_switch() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    keys(&mut app, "lj");
    enter(&mut app);
    rig.pump(&mut app);
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

// Keys against the simulated pad.

#[test]
fn starts_by_reading_the_pad_and_selects_its_mode_and_profile() {
    let app = Rig::channel(InputMode::Xinput).app();
    assert_eq!(app.current_mode(), Some(InputMode::Xinput));
    assert_eq!(app.mode_items()[app.modes.sel], InputMode::Xinput);
    assert_eq!(app.active_profile(), Some(2));
    assert_eq!(app.profile_items()[app.profiles.sel].number, 2);
    assert!(app.idle());
}

#[test]
fn enter_on_a_profile_switches_it_ram_only() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    keys(&mut app, "lj");
    enter(&mut app);
    assert!(app.busy.is_some());
    let jobs = app.take_jobs();
    assert_eq!(
        jobs,
        [Job::Profile {
            number: 3,
            save: false
        }]
    );
    let (_, reply) = rig.perform(&jobs[0]);
    app.on_reply(reply);
    rig.pump(&mut app);
    assert!(app.busy.is_none());
    assert_eq!(rig.pad.firmware().active, 3);
    assert_eq!(app.active_profile(), Some(3));
    let o = app.outcome.clone().unwrap();
    assert!(!o.error, "{}", o.text);
    assert!(o.text.contains("saved         no (RAM only)"), "{}", o.text);
}

#[test]
fn capital_s_saves_a_profile_after_the_prompt() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    keys(&mut app, "ljS");
    let Some(Overlay::Confirm(c)) = &app.overlay else {
        panic!("no prompt: {:?}", app.overlay);
    };
    assert_eq!(
        c.then,
        Confirmed::Run(Job::Profile {
            number: 3,
            save: true
        })
    );
    // n says no: nothing runs.
    keys(&mut app, "n");
    assert!(app.overlay.is_none());
    assert!(app.take_jobs().is_empty());
    keys(&mut app, "Sy");
    rig.pump(&mut app);
    let o = app.outcome.clone().unwrap();
    assert!(o.text.contains("saved         yes, in flash"), "{}", o.text);
}

#[test]
fn a_mode_switch_asks_first_then_runs_the_mode_flow() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    select_mode(&mut app, InputMode::Keyboard);
    enter(&mut app);
    assert!(matches!(app.overlay, Some(Overlay::Confirm(_))));
    assert!(app.take_jobs().is_empty());
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none());
    enter(&mut app);
    enter(&mut app);
    rig.pump(&mut app);
    assert_eq!(rig.pad.mode(), Some(InputMode::Keyboard));
    assert_eq!(app.current_mode(), Some(InputMode::Keyboard));
    let o = app.outcome.clone().unwrap();
    assert!(o.text.starts_with("Switched to Keyboard"), "{}", o.text);
}

#[test]
fn enter_on_the_current_mode_does_nothing() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    enter(&mut app);
    assert!(app.overlay.is_none());
    assert!(app.take_jobs().is_empty());
    assert_eq!(app.notice.as_deref(), Some("the pad is in XInput already"));
}

#[test]
fn w_goes_into_web_config_and_back() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    keys(&mut app, "w");
    assert_eq!(app.take_jobs(), [Job::Webconfig]);
    keys(&mut app, "w");
    // Busy: a second operation waits.
    assert!(app.take_jobs().is_empty());
    assert_eq!(
        app.notice.as_deref(),
        Some("wait: still going into web-config")
    );
    // Run the first one.
    let (notes, reply) = rig.perform(&Job::Webconfig);
    for n in notes {
        app.on_reply(Reply::Progress(n));
    }
    app.on_reply(reply);
    rig.pump(&mut app);
    assert!(app.in_webconfig());
    keys(&mut app, "w");
    assert_eq!(app.take_jobs(), [Job::LeaveWebconfig]);
    let (_, reply) = rig.perform(&Job::LeaveWebconfig);
    app.on_reply(reply);
    rig.pump(&mut app);
    assert_eq!(app.current_mode(), Some(InputMode::Xinput));
}

#[test]
fn b_backs_up_into_the_backups_directory() {
    let rig = Rig::stock(InputMode::Webconfig);
    let mut app = rig.app();
    keys(&mut app, "b");
    rig.pump(&mut app);
    let o = app.outcome.clone().unwrap();
    assert!(!o.error, "{}", o.text);
    let files: Vec<_> = std::fs::read_dir(rig.backups_dir()).unwrap().collect();
    assert_eq!(files.len(), 1);
    // The cache was refreshed from the backup.
    assert!(cache::load_latest(&rig.cache_dir()).is_some());
}

#[test]
fn profiles_need_the_channel() {
    let mut app = Rig::stock(InputMode::Xinput).app();
    keys(&mut app, "l");
    enter(&mut app);
    assert!(app.take_jobs().is_empty());
    assert!(app.notice.as_deref().unwrap().contains("control channel"));
}

#[test]
fn hotplug_reads_the_pad_only_when_usb_changes() {
    let rig = Rig::channel(InputMode::Xinput);
    let mut app = rig.app();
    app.on_tick();
    assert_eq!(app.take_jobs(), [Job::Scan]);
    // A second tick while the scan is out does nothing.
    app.on_tick();
    assert!(app.take_jobs().is_empty());
    let (_, reply) = rig.perform(&Job::Scan);
    app.on_reply(reply);
    assert!(app.take_jobs().is_empty(), "nothing changed on USB");
    // The pad re-enumerates (a new device number): read it again.
    app.on_reply(Reply::Scanned(vec![gamepadii_core::sim::device(
        InputMode::Xinput,
        9,
    )]));
    assert_eq!(app.take_jobs(), [Job::Status]);
}

#[test]
fn hotplug_finds_a_pad_plugged_in() {
    let mut app = no_pad();
    assert!(app.status_error.is_some());
    app.on_tick();
    assert_eq!(app.take_jobs(), [Job::Scan]);
    app.on_reply(Reply::Scanned(Vec::new()));
    assert!(app.take_jobs().is_empty());
    app.on_tick();
    app.take_jobs();
    app.on_reply(Reply::Scanned(vec![gamepadii_core::sim::device(
        InputMode::Xinput,
        7,
    )]));
    assert_eq!(app.take_jobs(), [Job::Status]);
}

#[test]
fn no_ticks_while_busy() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "w");
    app.take_jobs();
    app.on_tick();
    assert!(app.take_jobs().is_empty());
}

#[test]
fn quitting_while_busy_asks_first() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "w");
    keys(&mut app, "q");
    assert!(!app.should_quit);
    let Some(Overlay::Confirm(c)) = &app.overlay else {
        panic!("no prompt");
    };
    assert_eq!(c.then, Confirmed::Quit);
    keys(&mut app, "y");
    assert!(app.should_quit);
}

#[test]
fn q_quits_and_ctrl_c_always_does() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "?");
    keys(&mut app, "q");
    assert!(app.overlay.is_none() && !app.should_quit);
    keys(&mut app, "q");
    assert!(app.should_quit);

    let mut app = Rig::channel(InputMode::Xinput).app();
    keys(&mut app, "w");
    app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit);
}

#[test]
fn arrows_work_like_vim_keys() {
    let mut app = Rig::channel(InputMode::Xinput).app();
    let start = app.modes.sel;
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.modes.sel, start + 1);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.modes.sel, start);
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.focus, Pane::Profiles);
    app.on_key(key(KeyCode::Left));
    assert_eq!(app.focus, Pane::Modes);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.modes.sel, app.mode_items().len() - 1);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.modes.sel, 0);
}

#[test]
fn the_worker_thread_answers_over_channels() {
    let rig = Rig::channel(InputMode::Xinput);
    let (jobs, replies) = worker::spawn(rig.with_env());
    jobs.send(Job::Status).unwrap();
    match replies.recv_timeout(Duration::from_secs(10)).unwrap() {
        Reply::Status { status, .. } => {
            let s = status.unwrap();
            assert!(s.devices[0].control_channel.is_some());
        }
        other => panic!("{other:?}"),
    }
    jobs.send(Job::Profile {
        number: 3,
        save: false,
    })
    .unwrap();
    let done = loop {
        match replies.recv_timeout(Duration::from_secs(10)).unwrap() {
            Reply::Progress(_) => continue,
            r => break r,
        }
    };
    assert!(
        matches!(done, Reply::Done { result: Ok(_), .. }),
        "{done:?}"
    );
}
