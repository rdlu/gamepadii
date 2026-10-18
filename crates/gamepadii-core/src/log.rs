//! The pad's log (control channel v1.8, `docs/design/control-channel.md`
//! §3.13): the report of `gamepadii log`, and the crash check of `status`.

use serde::Serialize;
use serde_json::{Value, json};

use crate::channel::proto::{self, Command, LogEvent, LogRecord, log_code};
use crate::channel::{Channel, LogRead};
use crate::commands::Route;

/// `gamepadii log`: the records from a sequence number, read live with
/// READ_LOG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogReport {
    /// Always `control`.
    pub via: Route,
    pub host: String,
    /// `Haute42COSMOX`; `null` when the pad doesn't say.
    pub board: Option<String>,
    /// The boot number now.
    pub boot: u16,
    /// The sequence number of the next record: `--from` for the next read.
    pub next: u32,
    /// How many records after the asked number were gone.
    pub lost: u32,
    pub records: Vec<LogEntry>,
}

impl LogReport {
    pub fn new(host: String, board: Option<String>, read: LogRead) -> Self {
        Self {
            via: Route::Control,
            host,
            board,
            boot: read.boot,
            next: read.next,
            lost: read.lost,
            records: read.records.iter().map(LogEntry::from).collect(),
        }
    }
}

/// One record, as the JSON shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub seq: u32,
    pub boot: u16,
    /// Milliseconds since that boot.
    pub ms: u32,
    pub code: u8,
    /// `BOOT`, `PANIC`, ...; `null` for a code that gamepadii doesn't know.
    pub code_name: Option<&'static str>,
    /// The fields of the code, as an object; empty for an unknown code, or
    /// data that is too short for its code.
    pub data: Value,
    /// The data bytes, in hex.
    pub raw: String,
    #[serde(skip)]
    pub event: LogEvent,
}

impl From<&LogRecord> for LogEntry {
    fn from(r: &LogRecord) -> Self {
        let event = r.event();
        LogEntry {
            seq: r.seq,
            boot: r.boot,
            ms: r.ms,
            code: r.code,
            code_name: log_code::name(r.code),
            data: fields(&event),
            raw: r.data.iter().map(|b| format!("{b:02x}")).collect(),
            event,
        }
    }
}

/// The name of an OUT command byte: `REBOOT`; `None` for a command that
/// gamepadii doesn't know.
pub fn command_name(b: u8) -> Option<&'static str> {
    Command::from_byte(b).map(Command::name)
}

/// The fields of `e` as a JSON object.
fn fields(e: &LogEvent) -> Value {
    match e {
        LogEvent::Boot {
            kept,
            watchdog,
            mode,
            version,
        } => json!({
            "kept": kept,
            "watchdog": watchdog,
            "mode": proto::log_boot_mode(*mode),
            "version": version,
        }),
        LogEvent::Panic { text } | LogEvent::Error { text } => json!({ "text": text }),
        LogEvent::Command { cmd, result, seq } => json!({
            "command": command_name(*cmd),
            "command_byte": cmd,
            "result": result.name(),
            "result_byte": result.byte(),
            "seq": seq,
        }),
        LogEvent::Profile { old, new } => json!({ "old": old, "new": new }),
        LogEvent::Save { force, restart } => json!({ "force": force, "restart": restart }),
        LogEvent::Restart { mode } => json!({ "mode": proto::log_boot_mode(*mode) }),
        LogEvent::UsbHost {
            mounted,
            address,
            vid,
            pid,
        } => json!({
            "mounted": mounted,
            "address": address,
            "vid": format!("{vid:04x}"),
            "pid": format!("{pid:04x}"),
        }),
        LogEvent::Unknown { .. } => json!({}),
    }
}

/// A firmware panic that ended the boot before this one (or this boot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastPanic {
    pub boot: u16,
    /// Milliseconds after the start of that boot.
    pub ms: u32,
    /// The message, cut at 24 bytes.
    pub text: String,
}

/// The records that [`last_panic`] looks at: one READ_LOG reply.
const TAIL: u32 = 9;

/// The PANIC of the last boot, from the newest 9 records. At most two
/// READ_LOG requests: the first gives the next sequence number, the second
/// the newest records. `None` on a pad without the log, with no such
/// panic, or on any error.
pub fn last_panic(ch: &Channel) -> Option<LastPanic> {
    if !ch.hello().has_log() {
        return None;
    }
    let mut page = ch.read_log(0).ok()?;
    let tail = page.next.saturating_sub(TAIL);
    if page.first < tail {
        page = ch.read_log(tail).ok()?;
    }
    let boot = page.boot;
    page.records.iter().rev().find_map(|r| match r.event() {
        LogEvent::Panic { text } if r.boot == boot || r.boot.wrapping_add(1) == boot => {
            Some(LastPanic {
                boot: r.boot,
                ms: r.ms,
                text,
            })
        }
        _ => None,
    })
}
