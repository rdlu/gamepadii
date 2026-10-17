//! A transport that serves a fixture directory: `GET /api/<name>` returns
//! `<dir>/<name>.json`, a missing file is a 404, and `POST /api/reboot`
//! answers as the firmware does (the request echoed with `"success": true`).
//!
//! `POST /api/setGamepadOptions` behaves like the v0.7.12 setter, so tests
//! catch a partial body: every key the getter has is overwritten from the
//! body, a missing number becomes 0, a missing USB string becomes junk (the
//! firmware `strncpy`s from a NULL pointer), and a string sent for a USB id
//! is parsed as a decimal number, as ArduinoJson does (`"10C4"` is 0). The
//! result is what later GETs return. Every request is logged so tests can
//! check what was asked for.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Map, Value};

use super::{ApiPath, Method, Reply, Transport};
use crate::error::{Error, Result};

/// One logged request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub path: &'static str,
    pub body: Option<Vec<u8>>,
}

impl Request {
    /// The body as JSON (`null` when there is none or it isn't JSON).
    pub fn json(&self) -> Value {
        self.body
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or(Value::Null)
    }
}

/// How the simulated setter misbehaves, for tests of the verify step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetFault {
    /// Answers 200 and stores nothing.
    Ignore,
    /// Stores the body, then also changes `key` to `value`.
    Clobber(&'static str, Value),
}

/// The string the simulated firmware stores for a missing USB string.
pub const NULL_STRING_JUNK: &str = "\u{fffd}(strncpy from NULL)";

pub struct Mock {
    dir: PathBuf,
    log: Mutex<Vec<Request>>,
    /// Paths that fail like a hung device (a timeout).
    hang: Vec<&'static str>,
    /// Endpoint name → JSON written by a POST, served instead of the file.
    state: Mutex<BTreeMap<String, Value>>,
    /// Reboots time out after the request is out, as when the pad restarts
    /// before its reply gets back.
    drop_reboot: bool,
    set_fault: Option<SetFault>,
}

impl Mock {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            log: Mutex::new(Vec::new()),
            hang: Vec::new(),
            state: Mutex::new(BTreeMap::new()),
            drop_reboot: false,
            set_fault: None,
        }
    }

    /// Makes requests to `path` (e.g. `/api/getSplashImage`) time out.
    pub fn hang_on(mut self, path: &'static str) -> Self {
        self.hang.push(path);
        self
    }

    /// `POST /api/reboot` is logged, then times out.
    pub fn drop_reboot(mut self) -> Self {
        self.drop_reboot = true;
        self
    }

    pub fn set_fault(mut self, fault: SetFault) -> Self {
        self.set_fault = Some(fault);
        self
    }

    /// Every request so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// What `GET /api/<name>` returns now: written state, or the fixture.
    pub fn current(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.lock_state().get(name) {
            return Some(v.clone());
        }
        let bytes = fs::read(self.dir.join(format!("{name}.json"))).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Value>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_gamepad_options(&self, body: Option<&[u8]>) -> Result<Reply> {
        let doc: Value = body
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or(Value::Null);
        let Some(Value::Object(current)) = self.current("getGamepadOptions") else {
            return Ok(Reply {
                status: 404,
                body: Vec::new(),
            });
        };
        let mut stored = match &self.set_fault {
            Some(SetFault::Ignore) => current,
            _ => firmware_set_gamepad_options(&current, &doc),
        };
        if let Some(SetFault::Clobber(key, value)) = &self.set_fault {
            stored.insert((*key).to_string(), value.clone());
        }
        self.lock_state()
            .insert("getGamepadOptions".into(), Value::Object(stored));
        // The firmware echoes the document it parsed.
        Ok(json_reply(doc.to_string().into_bytes()))
    }
}

fn json_reply(body: Vec<u8>) -> Reply {
    Reply { status: 200, body }
}

/// ArduinoJson 6's integer conversion: numbers as they are, booleans as
/// 0/1, a string parsed as a whole decimal number (else 0), anything else
/// (missing included) 0.
fn as_int(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// What the v0.7.12 `setGamepadOptions` + `getGamepadOptions` pair turns
/// `current` into for the posted `doc`.
pub fn firmware_set_gamepad_options(
    current: &Map<String, Value>,
    doc: &Value,
) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, old) in current {
        let sent = doc.get(key);
        let new = match key.as_str() {
            // Getter-only: worked out from the pin mappings.
            "fnButtonPin" => old.clone(),
            "usbDescManufacturer" | "usbDescProduct" | "usbDescVersion" => {
                let max = if key == "usbDescVersion" { 8 } else { 32 };
                match sent.and_then(Value::as_str) {
                    Some(s) => Value::from(s.chars().take(max).collect::<String>()),
                    None => Value::from(NULL_STRING_JUNK),
                }
            }
            // A uint32 printed back with %04X.
            "usbVendorID" | "usbProductID" => {
                Value::from(format!("{:04X}", as_int(sent) as u32 & 0xffff))
            }
            // bools, printed back as 1/0.
            "switchTpShareForDs4"
            | "lockHotkeys"
            | "fourWayMode"
            | "usbDescOverride"
            | "usbOverrideID" => Value::from(i64::from(as_int(sent) != 0)),
            k if k.starts_with("hotkey") => {
                let field = |name| Value::from(as_int(sent.and_then(|h| h.get(name))));
                serde_json::json!({
                    "auxMask": field("auxMask"),
                    "buttonsMask": field("buttonsMask"),
                    "action": field("action"),
                })
            }
            _ => Value::from(as_int(sent)),
        };
        out.insert(key.clone(), new);
    }
    out
}

impl Transport for Mock {
    fn request(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> Result<Reply> {
        let path = path.as_str();
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Request {
                method,
                path,
                body: body.map(<[u8]>::to_vec),
            });
        let timed_out = || {
            Err(Error::io(
                format!("{method} mock{path}"),
                std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out (mock)"),
            ))
        };
        if self.hang.contains(&path) {
            return timed_out();
        }
        let name = path.strip_prefix("/api/").unwrap_or(path);
        match (method, name) {
            (Method::Post, "reboot") if self.drop_reboot => return timed_out(),
            (Method::Post, "reboot") => {
                let mut doc: Value = body
                    .and_then(|b| serde_json::from_slice(b).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                if let Some(obj) = doc.as_object_mut() {
                    obj.insert("success".into(), true.into());
                }
                return Ok(json_reply(doc.to_string().into_bytes()));
            }
            (Method::Post, "setGamepadOptions") => return self.set_gamepad_options(body),
            (Method::Post, _) => {
                return Ok(Reply {
                    status: 404,
                    body: Vec::new(),
                });
            }
            (Method::Get, _) => {}
        }
        if let Some(v) = self.lock_state().get(name) {
            return Ok(json_reply(v.to_string().into_bytes()));
        }
        match fs::read(self.dir.join(format!("{name}.json"))) {
            Ok(body) => Ok(json_reply(body)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Reply {
                status: 404,
                body: Vec::new(),
            }),
            Err(e) => Err(Error::io(format!("reading {}", self.dir.display()), e)),
        }
    }

    /// `mock:<fixture dir name>`: no absolute path, so it can be snapshotted.
    fn describe(&self) -> String {
        let name = self.dir.file_name().map_or_else(
            || self.dir.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        format!("mock:{name}")
    }
}
