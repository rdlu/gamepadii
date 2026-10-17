//! The writes gamepadii makes, `setGamepadOptions` and `setDisplayOptions`,
//! each as a read-modify-write with a verify.
//!
//! How the v0.7.12 `setGamepadOptions` (`src/webconfig.cpp`) reads its body:
//!
//! - Every field goes through `readDoc`, which assigns unconditionally: a
//!   missing key becomes 0 (or `false`). A missing `inputModeB1` is not
//!   "none" (-1) but XInput (0). So the body must carry every key.
//! - `usbDescManufacturer`, `usbDescProduct` and `usbDescVersion` are
//!   `strncpy`'d from `doc[key]`, which is a NULL pointer when the key is
//!   missing: they must always be sent, as strings.
//! - `usbVendorID` / `usbProductID` come back from the getter as 4-digit
//!   hex strings (`"10C4"`) but the setter reads integers. ArduinoJson 6.21
//!   parses a string as a decimal number, so echoing `"10C4"` back stores 0.
//!   The web UI converts them with `hexToInt` before posting; so do we.
//! - The hotkeys (`hotkey01`..`hotkey16`) are `{auxMask, buttonsMask,
//!   action}`; the getter folds the dpad mask into `buttonsMask` and the
//!   setter splits it out again, so they round-trip as they are.
//! - `fnButtonPin` is getter-only (worked out from the pin mappings) and
//!   ignored by the setter. Like the web UI, we send it back unchanged.
//! - The reply echoes the posted document. The change is saved to flash on
//!   the firmware's next main-loop pass, before any reboot we ask for.
//!
//! And `setDisplayOptions` (`webconfig.cpp:421`):
//!
//! - The same `readDoc` for all 22 top-level fields ([`DISPLAY_SETTER_KEYS`])
//!   and for the 10 under `buttonLayoutCustomOptions.params` /
//!   `.paramsRight` (`layout`, `startX`, `startY`, `buttonRadius`,
//!   `buttonPadding`): anything missing is 0, a missing nested object
//!   included. All of them are numbers (or booleans the getter prints as
//!   1/0); there are no strings, and the getter and setter have exactly the
//!   same keys, so the getter's object goes back as it is.
//! - The splash image is not part of it (`setSplashImage`).
//! - It saves like `setGamepadOptions` (a `GPStorageSaveEvent`) and echoes
//!   the body. The OLED picks the layout up when the pad next starts.
//!
//! The server ignores the HTTP method, so the setters are only ever POSTed
//! (a GET would replay the previous POST body).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::{DisplayOptions, Endpoint, GamepadOptions, read_raw};
use crate::error::{Error, Result};
use crate::transport::{ApiPath, Method, Transport};

const SET_GAMEPAD_OPTIONS: ApiPath = ApiPath::new("/api/setGamepadOptions");
const SET_DISPLAY_OPTIONS: ApiPath = ApiPath::new("/api/setDisplayOptions");

/// lwIP's POST buffer in the firmware (`LWIP_HTTPD_POST_MAX_PAYLOAD_LEN`).
pub const MAX_POST_BODY: usize = 16 * 1024;

/// Every top-level key the v0.7.12 setter reads. A body without one of
/// these would zero that setting, so [`set_gamepad_options`] refuses to
/// post it.
pub const SETTER_KEYS: &[&str] = &[
    "dpadMode",
    "inputMode",
    "inputDeviceType",
    "socdMode",
    "switchTpShareForDs4",
    "lockHotkeys",
    "fourWayMode",
    "profileNumber",
    "debounceDelay",
    "inputModeB1",
    "inputModeB2",
    "inputModeB3",
    "inputModeB4",
    "inputModeL1",
    "inputModeL2",
    "inputModeR1",
    "inputModeR2",
    "ps4AuthType",
    "ps5AuthType",
    "xinputAuthType",
    "ps4ControllerIDMode",
    "usbDescOverride",
    "miniMenuGamepadInput",
    "usbDescManufacturer",
    "usbDescProduct",
    "usbDescVersion",
    "usbOverrideID",
    "usbVendorID",
    "usbProductID",
    "hotkey01",
    "hotkey02",
    "hotkey03",
    "hotkey04",
    "hotkey05",
    "hotkey06",
    "hotkey07",
    "hotkey08",
    "hotkey09",
    "hotkey10",
    "hotkey11",
    "hotkey12",
    "hotkey13",
    "hotkey14",
    "hotkey15",
    "hotkey16",
    "forcedSetupMode",
];

/// The keys under each `hotkeyNN`.
pub const HOTKEY_KEYS: &[&str] = &["auxMask", "buttonsMask", "action"];

/// Strings the setter `strncpy`s (NULL when missing).
pub const STRING_KEYS: &[&str] = &["usbDescManufacturer", "usbDescProduct", "usbDescVersion"];

/// Hex strings on the way out, integers on the way in.
pub const HEX_ID_KEYS: &[&str] = &["usbVendorID", "usbProductID"];

/// What the getter returns that the setter ignores.
pub const GETTER_ONLY_KEYS: &[&str] = &["fnButtonPin"];

/// Every top-level number the v0.7.12 `setDisplayOptions` reads, in its
/// order. The ones it stores as `bool` are [`DISPLAY_BOOL_KEYS`].
pub const DISPLAY_SETTER_KEYS: &[&str] = &[
    "enabled",
    "flipDisplay",
    "invertDisplay",
    "buttonLayout",
    "buttonLayoutRight",
    "splashMode",
    "splashChoice",
    "splashDuration",
    "displaySaverTimeout",
    "displaySaverMode",
    "buttonLayoutOrientation",
    "turnOffWhenSuspended",
    "inputMode",
    "turboMode",
    "dpadMode",
    "socdMode",
    "macroMode",
    "profileMode",
    "inputHistoryEnabled",
    "inputHistoryLength",
    "inputHistoryCol",
    "inputHistoryRow",
];

/// The display fields that are `bool` in the protobuf (the getter prints
/// them as 1/0).
pub const DISPLAY_BOOL_KEYS: &[&str] = &[
    "enabled",
    "invertDisplay",
    "turnOffWhenSuspended",
    "inputMode",
    "turboMode",
    "dpadMode",
    "socdMode",
    "macroMode",
    "profileMode",
    "inputHistoryEnabled",
];

/// The object holding the custom layout's parameters.
pub const CUSTOM_LAYOUT_KEY: &str = "buttonLayoutCustomOptions";

/// Its two halves (`params` is the left one).
pub const CUSTOM_LAYOUT_SIDES: &[&str] = &["params", "paramsRight"];

/// The numbers the setter reads from each half.
pub const CUSTOM_LAYOUT_KEYS: &[&str] = &[
    "layout",
    "startX",
    "startY",
    "buttonRadius",
    "buttonPadding",
];

/// One setting, before and after (as the getter shows it).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    /// The firmware's key: `inputMode`.
    pub key: String,
    pub before: Value,
    pub after: Value,
}

/// A verified write: what changed, read back from the pad.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OptionsWrite {
    /// The changes asked for, as read back after the write.
    pub changed: Vec<FieldChange>,
    /// Whether anything was posted (`false` when the edit changed nothing).
    pub posted: bool,
}

/// A JSON object, as the getters return it.
type Object = Map<String, Value>;

/// One `get*` / `set*` pair and how to build the setter's body.
struct Setter {
    get: Endpoint,
    set: ApiPath,
    /// `setGamepadOptions`, for messages.
    name: &'static str,
    body: fn(&Object) -> Result<Object>,
}

const GAMEPAD: Setter = Setter {
    get: Endpoint::GetGamepadOptions,
    set: SET_GAMEPAD_OPTIONS,
    name: "setGamepadOptions",
    body: setter_body,
};

const DISPLAY: Setter = Setter {
    get: Endpoint::GetDisplayOptions,
    set: SET_DISPLAY_OPTIONS,
    name: "setDisplayOptions",
    body: display_setter_body,
};

fn refusal(setter: &str, why: String) -> Error {
    Error::Protocol(format!(
        "{setter}: not writing, {why} (the firmware would zero or corrupt it)"
    ))
}

/// The body for the setter, from the getter's object: everything it
/// returned, with the USB ids turned into integers. Fails (before anything
/// is sent) when a key the setter reads is missing or has the wrong type.
pub fn setter_body(options: &Map<String, Value>) -> Result<Map<String, Value>> {
    let refuse = |why: String| refusal(GAMEPAD.name, why);
    let mut body = options.clone();
    for key in SETTER_KEYS {
        let Some(v) = body.get(*key) else {
            return Err(refuse(format!("the pad's options have no {key:?}")));
        };
        if STRING_KEYS.contains(key) && !v.is_string() {
            return Err(refuse(format!("{key:?} is not a string")));
        }
        if key.starts_with("hotkey") {
            for sub in HOTKEY_KEYS {
                if !v.get(*sub).is_some_and(Value::is_number) {
                    return Err(refuse(format!("{key:?} has no numeric {sub:?}")));
                }
            }
        }
    }
    for key in HEX_ID_KEYS {
        let id = match &body[*key] {
            Value::String(s) => u16::from_str_radix(s.trim(), 16)
                .map_err(|_| refuse(format!("{key:?} is {s:?}, not a hex id")))?,
            Value::Number(n) => n
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| refuse(format!("{key:?} is {n}, not a USB id")))?,
            other => return Err(refuse(format!("{key:?} is {other}"))),
        };
        body.insert((*key).to_string(), Value::from(id));
    }
    Ok(body)
}

/// A number, or a boolean (ArduinoJson reads both as either).
fn numeric(v: &Value) -> bool {
    v.is_number() || v.is_boolean()
}

/// The body for `setDisplayOptions`: the getter's object as it is, once
/// every key the setter reads is there and numeric (nested ones included).
pub fn display_setter_body(options: &Map<String, Value>) -> Result<Map<String, Value>> {
    let refuse = |why: String| refusal(DISPLAY.name, why);
    for key in DISPLAY_SETTER_KEYS {
        match options.get(*key) {
            None => return Err(refuse(format!("the pad's display options have no {key:?}"))),
            Some(v) if !numeric(v) => return Err(refuse(format!("{key:?} is {v}, not a number"))),
            Some(_) => {}
        }
    }
    let Some(custom) = options.get(CUSTOM_LAYOUT_KEY).and_then(Value::as_object) else {
        return Err(refuse(format!("there is no {CUSTOM_LAYOUT_KEY:?} object")));
    };
    for side in CUSTOM_LAYOUT_SIDES {
        let Some(half) = custom.get(*side).and_then(Value::as_object) else {
            return Err(refuse(format!("{CUSTOM_LAYOUT_KEY}.{side} is missing")));
        };
        for key in CUSTOM_LAYOUT_KEYS {
            if !half.get(*key).is_some_and(numeric) {
                return Err(refuse(format!(
                    "{CUSTOM_LAYOUT_KEY}.{side} has no numeric {key:?}"
                )));
            }
        }
    }
    Ok(options.clone())
}

/// `before` with the named fields of `edited` laid over it, keeping the
/// pad's key order (serde_json `preserve_order`).
fn overlay<T: Serialize>(before: &Map<String, Value>, edited: &T) -> Map<String, Value> {
    let Value::Object(edited) = serde_json::to_value(edited).expect("plain data serialises") else {
        unreachable!("the options serialise to an object")
    };
    let mut out = before.clone();
    for (k, v) in edited {
        out.insert(k, v);
    }
    out
}

/// Every key whose value differs between `a` and `b` (missing counts as
/// `null`), in `a`'s order, then `b`'s new keys.
pub fn diff(a: &Map<String, Value>, b: &Map<String, Value>) -> Vec<FieldChange> {
    let mut out: Vec<FieldChange> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, v)| FieldChange {
            key: k.clone(),
            before: v.clone(),
            after: b.get(k).cloned().unwrap_or(Value::Null),
        })
        .collect();
    out.extend(
        b.iter()
            .filter(|(k, _)| !a.contains_key(*k))
            .map(|(k, v)| FieldChange {
                key: k.clone(),
                before: Value::Null,
                after: v.clone(),
            }),
    );
    out
}

fn options_object(t: &dyn Transport, ep: Endpoint) -> Result<Map<String, Value>> {
    match read_raw(t, ep)? {
        Value::Object(m) => Ok(m),
        other => Err(Error::Protocol(format!(
            "{ep}: expected an object, got {other}"
        ))),
    }
}

/// The read-modify-write both setters share:
///
/// 1. GET the whole object (unknown fields included);
/// 2. let `edit` change the typed fields;
/// 3. POST the whole object back (see [`setter_body`],
///    [`display_setter_body`]);
/// 4. GET it again and check that exactly the edited fields changed, to
///    the edited values.
///
/// Nothing is posted when `edit` changes nothing. A read-back that differs
/// from what was meant is an error that lists every difference.
fn read_modify_write<T: Serialize + DeserializeOwned>(
    t: &dyn Transport,
    s: &Setter,
    edit: impl FnOnce(&mut T),
) -> Result<OptionsWrite> {
    let before = options_object(t, s.get)?;
    let mut typed: T = serde_json::from_value(Value::Object(before.clone()))
        .map_err(|e| Error::Protocol(format!("{}: unexpected shape: {e}", s.get)))?;
    edit(&mut typed);
    let wanted = overlay(&before, &typed);
    let intended = diff(&before, &wanted);
    if intended.is_empty() {
        return Ok(OptionsWrite {
            changed: Vec::new(),
            posted: false,
        });
    }
    let body = serde_json::to_vec(&(s.body)(&wanted)?).expect("plain data serialises");
    if body.len() > MAX_POST_BODY {
        return Err(Error::Protocol(format!(
            "{}: the body is {} bytes, over the firmware's {MAX_POST_BODY}",
            s.name,
            body.len()
        )));
    }
    let reply = t.request(Method::Post, s.set, Some(&body))?;
    if reply.status != 200 {
        return Err(Error::Protocol(format!(
            "{}: HTTP {}",
            s.name, reply.status
        )));
    }
    let after = options_object(t, s.get)?;
    let surprises = diff(&wanted, &after);
    if !surprises.is_empty() {
        let list = surprises
            .iter()
            .map(|c| format!("{}: wanted {}, the pad has {}", c.key, c.before, c.after))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::CheckFailed(format!(
            "{} was written, but reading it back differs: {list}",
            s.name
        )));
    }
    Ok(OptionsWrite {
        changed: diff(&before, &after),
        posted: true,
    })
}

/// Read-modify-write of `setGamepadOptions`: GET the whole object, let
/// `edit` change the typed fields, POST the whole object back (see
/// [`setter_body`]), then GET it again and check that exactly the edited
/// fields changed. Nothing is posted when `edit` changes nothing.
pub fn set_gamepad_options(
    t: &dyn Transport,
    edit: impl FnOnce(&mut GamepadOptions),
) -> Result<OptionsWrite> {
    read_modify_write(t, &GAMEPAD, edit)
}

/// Read-modify-write of `setDisplayOptions`, the same way (the body:
/// [`display_setter_body`]).
pub fn set_display_options(
    t: &dyn Transport,
    edit: impl FnOnce(&mut DisplayOptions),
) -> Result<OptionsWrite> {
    read_modify_write(t, &DISPLAY, edit)
}
