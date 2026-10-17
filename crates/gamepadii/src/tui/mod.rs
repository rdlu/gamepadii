//! `gamepadii` on a terminal (or `gamepadii tui`): the TUI. [`app`] holds
//! the state and takes keys and replies without a terminal; [`ui`] draws
//! it; [`worker`] runs the core's commands and flows on a thread; [`run`]
//! wires them to the terminal.

pub mod app;
pub mod keys;
pub mod ui;
pub mod worker;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyEventKind};

pub use app::App;
use worker::{Job, Reply, WithEnv};

/// How long to wait for a key before looking at the worker again.
const POLL: Duration = Duration::from_millis(100);
/// How often to look at USB for a pad plugged in or out, while idle.
const HOTPLUG: Duration = Duration::from_secs(1);

/// Runs the TUI until the user quits; the terminal is restored on exit and
/// on panic. `with_env` builds the flows' environment on the worker thread.
pub fn run(with_env: WithEnv) -> std::io::Result<()> {
    let (jobs, replies) = worker::spawn(with_env);
    let mut app = App::new();
    let mut terminal = ratatui::try_init()?;
    let result = run_loop(&mut terminal, &mut app, &jobs, &replies);
    ratatui::restore();
    result
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    jobs: &Sender<Job>,
    replies: &Receiver<Reply>,
) -> std::io::Result<()> {
    let mut alive = true;
    let mut dirty = true;
    let mut last_tick = Instant::now();
    while !app.should_quit {
        for job in app.take_jobs() {
            if jobs.send(job).is_err() {
                alive = false;
            }
        }
        if dirty || app.busy.is_some() {
            if app.busy.is_some() {
                app.frame = app.frame.wrapping_add(1);
            }
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        if event::poll(POLL)? {
            match event::read()? {
                TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                    app.on_key(key);
                    dirty = true;
                }
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }
        while alive {
            match replies.try_recv() {
                Ok(reply) => {
                    app.on_reply(reply);
                    dirty = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    alive = false;
                    app.busy = None;
                    app.notice = Some("the worker stopped: restart gamepadii".into());
                    dirty = true;
                }
            }
        }
        if alive && last_tick.elapsed() >= HOTPLUG {
            last_tick = Instant::now();
            app.on_tick();
        }
    }
    Ok(())
}
