//! The control channel: vendor control requests on EP0 that the gamepadii
//! fork of GP2040-CE answers in every mode (gamepad modes and web-config),
//! with no descriptor change and no kernel driver detached
//! (`docs/design/control-channel.md`).
//!
//! - [`proto`]: the v1 wire format, v1.1's LED commands, v1.2's lights
//!   commands, v1.3's board ID, v1.4's profile slots.
//! - [`ControlTransport`]: one vendor control IN or OUT on EP0. On Linux,
//!   [`usbfs::Usbfs`] (an `ioctl` on `/dev/bus/usb/BBB/DDD`); in tests,
//!   [`sim::SimChannel`] behind `crate::sim::SimPad`.
//! - [`probe`] and [`Channel`]: HELLO (magic, nonce echo, protocol major),
//!   then the commands, each carrying the per-boot token.
//!
//! Stock firmware has no channel, and it doesn't always STALL a request
//! that looks like ours: XInput answers with 0 bytes and Xbox One with
//! whatever is on its stack. Only a reply with the magic *and* our fresh
//! nonce counts.

pub mod proto;
pub mod sim;
#[cfg(target_os = "linux")]
pub mod usbfs;

use std::cell::Cell;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use crate::api::RebootAck;
use crate::discovery::Device;
use crate::error::{Error, Result};
use crate::mode::{BootMode, InputMode};
use crate::wait::Clock;

pub use proto::{
    Caps, Command, Hello, LedStatus, LightsStatus, Outcome, SetLed, SetLights, Setup, Slot,
    SlotPin, Status,
};

/// How long a HELLO may take: short, since most pads have no channel.
pub const HELLO_TIMEOUT: Duration = Duration::from_millis(200);
/// How long any other request may take (a flash save can NAK EP0 for a
/// few ms).
pub const TIMEOUT: Duration = Duration::from_millis(1000);

/// Why a control transfer failed, sorted by what the caller does next.
#[derive(Debug)]
pub enum UsbError {
    /// The device STALLed (`EPIPE`): no such request, or a refused command.
    Stall,
    /// No permission on the device node (`EACCES`): needs the udev rule.
    NoAccess,
    /// The device left the bus (`ENODEV`, `ESHUTDOWN`, `EPROTO`, a missing
    /// node): expected right after a REBOOT.
    Gone,
    /// No reply in time (`ETIMEDOUT`).
    Timeout,
    /// gamepadii runs as root, and does not use the channel then: root
    /// gets past the udev rule's limit, to every device with one of our
    /// ids (a real Xbox 360 pad, a DualShock, ...).
    AsRoot,
    Io(std::io::Error),
}

impl fmt::Display for UsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsbError::Stall => f.write_str("the pad refused the request (STALL)"),
            UsbError::NoAccess => f.write_str("permission denied"),
            UsbError::Gone => f.write_str("the pad is no longer on USB"),
            UsbError::Timeout => f.write_str("no reply in time"),
            UsbError::AsRoot => f.write_str(
                "gamepadii does not use the control channel as root; run it as your user",
            ),
            UsbError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl UsbError {
    /// The crate error for a failed `what` on `node`.
    pub fn into_error(self, node: &str, what: &str) -> Error {
        use std::io::{Error as IoError, ErrorKind};
        let context = format!("{what} on {node}");
        match self {
            UsbError::NoAccess => Error::NeedsUser(udev_hint(node)),
            UsbError::Stall => Error::Protocol(format!("{context}: the pad refused it (STALL)")),
            UsbError::Gone => Error::io(
                context,
                IoError::new(ErrorKind::NotConnected, "the pad is no longer on USB"),
            ),
            UsbError::Timeout => Error::io(
                context,
                IoError::new(ErrorKind::TimedOut, "no reply within 1 s"),
            ),
            UsbError::AsRoot => Error::Usage(format!("{context}: {}", UsbError::AsRoot)),
            UsbError::Io(e) => Error::io(context, e),
        }
    }
}

/// How long to wait for the `uaccess` ACL on a device node that udev has
/// just made: it can come a moment after the node.
pub const ACL_WAIT: Duration = Duration::from_secs(1);

/// `open`, tried again every 100 ms for up to [`ACL_WAIT`] while it fails
/// with `EACCES`. Use it only for a node that is new on USB, with the udev
/// rule installed: otherwise `EACCES` does not go away.
pub fn wait_for_acl<T>(
    clock: &dyn Clock,
    mut open: impl FnMut() -> Result<T, UsbError>,
) -> Result<T, UsbError> {
    let start = clock.elapsed();
    loop {
        match open() {
            Err(UsbError::NoAccess) if clock.elapsed().saturating_sub(start) < ACL_WAIT => {
                clock.sleep(Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// What to tell someone who has no permission on the node.
pub fn udev_hint(node: &str) -> String {
    format!(
        "no permission to use {node}, the pad's USB device node. Run `gamepadii setup udev` \
         once (it explains what it grants), then replug the pad"
    )
}

/// One vendor control transfer on EP0 at a time. Implementations never
/// claim an interface or detach a kernel driver: vendor requests to the
/// device need neither.
pub trait ControlTransport {
    /// A device-to-host request; the reply may be shorter than `len`.
    fn control_in(&self, setup: Setup, len: u16, timeout: Duration) -> Result<Vec<u8>, UsbError>;
    /// A host-to-device request with `data` as its data stage.
    fn control_out(&self, setup: Setup, data: &[u8], timeout: Duration) -> Result<(), UsbError>;
    /// The device node, for messages: `/dev/bus/usb/003/007`.
    fn describe(&self) -> String;
}

/// Opens the channel's transport for a device found on USB.
pub type OpenControl<'a> = dyn Fn(&Device) -> Result<Box<dyn ControlTransport>, UsbError> + 'a;

/// `/dev/bus/usb/003/007` for a device on bus 3 with number 7.
pub fn node_path(d: &Device) -> Option<String> {
    Some(format!("/dev/bus/usb/{:03}/{:03}", d.bus?, d.devnum?))
}

/// A fresh, non-zero nonce for HELLO, from the standard library's
/// randomly seeded hasher (no RNG dependency for 16 bits).
pub fn nonce() -> u16 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    let n = (h.finish() & 0xffff) as u16;
    if n == 0 { 0x4750 } else { n }
}

/// `s` as a board ID: 16 hex digits, in upper case.
pub fn board_id(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_uppercase())
}

/// What a HELLO found.
pub enum Probe {
    Found(Channel),
    /// The device answered HELLO, and the answer is not the channel: a
    /// STALL, a short reply (stock XInput), garbage (stock Xbox One), a
    /// wrong magic or nonce. This connection gives the same answer again,
    /// so it is not asked again ([`crate::commands::probe`]). The reason is
    /// for notes.
    Stock(String),
    /// No answer: no reply in time, the pad left the bus, or no device
    /// node. The reason is for notes.
    Absent(String),
    /// The node exists but this user can't open it read-write.
    NoAccess {
        node: String,
    },
    /// A channel, but with a protocol major this gamepadii doesn't speak.
    Newer {
        node: String,
        major: u8,
    },
}

impl fmt::Debug for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Probe::Found(c) => write!(f, "Found({:?})", c.hello),
            Probe::Stock(r) => write!(f, "Stock({r:?})"),
            Probe::Absent(r) => write!(f, "Absent({r:?})"),
            Probe::NoAccess { node } => write!(f, "NoAccess({node})"),
            Probe::Newer { node, major } => write!(f, "Newer({node}, v{major})"),
        }
    }
}

impl Probe {
    pub fn found(self) -> Option<Channel> {
        match self {
            Probe::Found(c) => Some(c),
            _ => None,
        }
    }
}

/// Opens `d`'s node and sends HELLO with a fresh nonce.
pub fn probe(open: &OpenControl<'_>, d: &Device) -> Probe {
    let node = node_path(d).unwrap_or_else(|| format!("USB device {}", d.port));
    match open(d) {
        Ok(t) => hello(t, nonce()),
        Err(UsbError::NoAccess) => Probe::NoAccess { node },
        Err(e) => Probe::Absent(format!("{node}: {e}")),
    }
}

/// HELLO over `t` with `nonce`: the channel if the reply proves itself.
pub fn hello(t: Box<dyn ControlTransport>, nonce: u16) -> Probe {
    let node = t.describe();
    let reply = match t.control_in(
        Setup::read(Command::Hello, nonce),
        proto::MAX_LEN,
        HELLO_TIMEOUT,
    ) {
        Ok(r) => r,
        Err(UsbError::NoAccess) => return Probe::NoAccess { node },
        Err(e @ UsbError::Stall) => return Probe::Stock(format!("HELLO: {e}")),
        Err(e) => return Probe::Absent(format!("HELLO: {e}")),
    };
    match Hello::parse(&reply, nonce) {
        Ok(hello) => Probe::Found(Channel {
            t,
            hello,
            seq: Cell::new(nonce.rotate_left(8)),
        }),
        Err(proto::NotHello::Major(major)) => Probe::Newer { node, major },
        Err(why) => Probe::Stock(format!("HELLO: not the channel ({why:?})")),
    }
}

/// A pad that answered HELLO: its capabilities, and the commands.
pub struct Channel {
    t: Box<dyn ControlTransport>,
    hello: Hello,
    seq: Cell<u16>,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("node", &self.node())
            .field("hello", &self.hello)
            .finish()
    }
}

impl Channel {
    /// The HELLO reply: version, capabilities, the modes this build boots.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn caps(&self) -> Caps {
        self.hello.caps
    }

    /// `/dev/bus/usb/003/007`.
    pub fn node(&self) -> String {
        self.t.describe()
    }

    fn read(&self, cmd: Command, value: u16) -> Result<Vec<u8>, UsbError> {
        self.read_len(cmd, value, proto::MAX_LEN)
    }

    fn read_len(&self, cmd: Command, value: u16, len: u16) -> Result<Vec<u8>, UsbError> {
        self.t.control_in(Setup::read(cmd, value), len, TIMEOUT)
    }

    fn fail(&self, e: UsbError, cmd: Command) -> Error {
        e.into_error(&self.node(), cmd.name())
    }

    fn need(&self, bit: u32, cmd: Command) -> Result<()> {
        if self.caps().has(bit) {
            Ok(())
        } else {
            Err(Error::Usage(format!(
                "this firmware's control channel has no {}",
                cmd.name()
            )))
        }
    }

    pub fn status(&self) -> Result<Status> {
        self.need(Caps::STATUS, Command::Status)?;
        let reply = self
            .read(Command::Status, 0)
            .map_err(|e| self.fail(e, Command::Status))?;
        Status::parse(&reply).ok_or_else(|| {
            Error::Protocol(format!(
                "STATUS on {}: {} bytes, expected {}",
                self.node(),
                reply.len(),
                proto::STATUS_LEN
            ))
        })
    }

    /// A STRING; `None` when it is empty or the pad has no such id.
    pub fn string(&self, id: u16) -> Result<Option<String>> {
        self.need(Caps::STRING, Command::String)?;
        match self.read(Command::String, id) {
            Ok(b) if b.is_empty() => Ok(None),
            Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
            Err(UsbError::Stall) => Ok(None),
            Err(e) => Err(self.fail(e, Command::String)),
        }
    }

    /// The unique ID of the flash chip (v1.3 STRING [`proto::string::BOARD_ID`]),
    /// in upper case; `None` before protocol 1.3 (the request is never
    /// sent then: an older pad STALLs it) or when the reply is not 16 hex
    /// digits. Pads with the same flash chip can give the same ID.
    pub fn board_id(&self) -> Result<Option<String>> {
        if !self.hello.has_board_id() {
            return Ok(None);
        }
        Ok(self
            .string(proto::string::BOARD_ID)?
            .and_then(|s| board_id(&s)))
    }

    /// Sends an OUT command. A STALL is a refusal: STATUS says why.
    fn send(&self, cmd: Command, args: &[u8]) -> Result<(), UsbError> {
        let seq = self.seq.get().wrapping_add(1);
        self.seq.set(seq);
        let data = proto::payload(self.hello.token, seq, args);
        self.t.control_out(Setup::command(cmd), &data, TIMEOUT)
    }

    /// Why the last command (`seq`) was refused, as an error.
    fn refusal(&self, cmd: Command, what: &str) -> Error {
        let seq = self.seq.get();
        let status = match self.status() {
            Ok(s) => s,
            Err(e) => return e,
        };
        if status.last_seq != seq || status.last_command != cmd as u8 {
            return Error::Protocol(format!(
                "{} on {}: refused, and STATUS doesn't say why",
                cmd.name(),
                self.node()
            ));
        }
        let outcome = Outcome::from_byte(status.last_result);
        let lock = match status.forced_setup {
            1 => "mode switching",
            2 => "web-config",
            3 => "web-config and mode switching",
            _ => "it",
        };
        match outcome {
            Outcome::Locked => Error::Usage(format!(
                "the pad refused {what}: its forced setup mode locks {lock}"
            )),
            Outcome::Unsupported => Error::Usage(format!(
                "the pad refused {what}: not in this firmware build"
            )),
            Outcome::ProfileDisabled => {
                Error::Usage(format!("the pad refused {what}: that profile is disabled"))
            }
            Outcome::BadArg => Error::Usage(format!("the pad refused {what}: bad argument")),
            Outcome::Busy => Error::io(
                format!("{} on {}", cmd.name(), self.node()),
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    if status.reboot_pending() {
                        "the pad is about to restart (a reboot or mode change is pending)"
                    } else {
                        "the pad is busy; try again"
                    },
                ),
            ),
            o => Error::Protocol(format!(
                "{} on {}: refused ({o:?})",
                cmd.name(),
                self.node()
            )),
        }
    }

    fn command(&self, cmd: Command, args: &[u8], what: &str) -> Result<()> {
        match self.send(cmd, args) {
            Ok(()) => Ok(()),
            Err(UsbError::Stall) => Err(self.refusal(cmd, what)),
            Err(e) => Err(self.fail(e, cmd)),
        }
    }

    /// A command after which the pad restarts: losing it from the bus
    /// mid-request counts as a probable success, as for `/api/reboot`.
    fn restarting(&self, cmd: Command, args: &[u8], what: &str) -> Result<RebootAck> {
        match self.send(cmd, args) {
            Ok(()) => Ok(RebootAck::Confirmed),
            Err(UsbError::Gone | UsbError::Timeout) => Ok(RebootAck::Probable),
            Err(UsbError::Stall) => Err(self.refusal(cmd, what)),
            Err(e) => Err(self.fail(e, cmd)),
        }
    }

    /// Switches to profile `n` (1–6), live. `persist` also saves it to
    /// flash; otherwise the pad goes back to its saved profile at the next
    /// boot (or keeps it, if anything else saves the settings first).
    pub fn set_profile(&self, n: u8, persist: bool) -> Result<()> {
        self.need(Caps::SET_PROFILE, Command::SetProfile)?;
        if !(1..=6).contains(&n) {
            return Err(Error::Usage(format!("profile {n}: profiles are 1 to 6")));
        }
        self.command(
            Command::SetProfile,
            &[n, u8::from(persist)],
            &format!("profile {n}"),
        )
    }

    /// Restarts into `mode`. BOOTSEL only when the build offers it.
    pub fn reboot(&self, mode: BootMode) -> Result<RebootAck> {
        let bit = match mode {
            BootMode::Bootsel => Caps::REBOOT_BOOTSEL,
            _ => Caps::REBOOT,
        };
        self.need(bit, Command::Reboot)?;
        self.restarting(
            Command::Reboot,
            &[mode.api_value()],
            &format!("a reboot into {}", mode.label()),
        )
    }

    /// LED_STATUS (v1.1): the animation, brightness and colours running now.
    pub fn led_status(&self) -> Result<LedStatus> {
        self.need(Caps::LEDS, Command::LedStatus)?;
        let reply = self
            .read(Command::LedStatus, 0)
            .map_err(|e| self.fail(e, Command::LedStatus))?;
        LedStatus::parse(&reply).ok_or_else(|| {
            Error::Protocol(format!(
                "LED_STATUS on {}: {} bytes, expected {}",
                self.node(),
                reply.len(),
                proto::LED_STATUS_LEN
            ))
        })
    }

    /// SET_LED (v1.1): changes the fields `set` has, live; saved to flash
    /// only with `set.persist`. All or nothing: a value out of range
    /// changes none of them.
    pub fn set_led(&self, set: &SetLed) -> Result<()> {
        self.need(Caps::LEDS, Command::SetLed)?;
        self.command(Command::SetLed, &set.encode(), "the LED change")
    }

    /// LIGHTS_STATUS (v1.2, main): the lights profile and brightness
    /// running now.
    pub fn lights_status(&self) -> Result<LightsStatus> {
        self.need(Caps::LIGHTS, Command::LightsStatus)?;
        let reply = self
            .read(Command::LightsStatus, 0)
            .map_err(|e| self.fail(e, Command::LightsStatus))?;
        LightsStatus::parse(&reply).ok_or_else(|| {
            Error::Protocol(format!(
                "LIGHTS_STATUS on {}: {} bytes, expected {}",
                self.node(),
                reply.len(),
                proto::LIGHTS_STATUS_LEN
            ))
        })
    }

    /// SET_LIGHTS (v1.2, main): changes the fields `set` has, live; saved
    /// to flash only with `set.persist`. All or nothing.
    pub fn set_lights(&self, set: &SetLights) -> Result<()> {
        self.need(Caps::LIGHTS, Command::SetLights)?;
        self.command(Command::SetLights, &set.encode(), "the lights change")
    }

    /// READ_SLOT and WRITE_SLOT need protocol 1.4 and [`Caps::SLOTS`]: a
    /// 1.3 pad never gets them, whatever its capability bits say.
    fn need_slots(&self, cmd: Command) -> Result<()> {
        if self.hello.has_slots() {
            Ok(())
        } else {
            Err(Error::Usage(format!(
                "this firmware's control channel has no {} (it needs protocol 1.4)",
                cmd.name()
            )))
        }
    }

    /// READ_SLOT (v1.4): the pin mappings and label of profile `n` (1–6),
    /// as the pad has them in RAM.
    pub fn read_slot(&self, n: u8) -> Result<Slot> {
        self.need_slots(Command::ReadSlot)?;
        if !(1..=6).contains(&n) {
            return Err(Error::Usage(format!("profile {n}: profiles are 1 to 6")));
        }
        let reply = self
            .read_len(Command::ReadSlot, n.into(), proto::READ_SLOT_LEN)
            .map_err(|e| self.fail(e, Command::ReadSlot))?;
        Slot::parse(&reply)
            .filter(|s| s.profile == n)
            .ok_or_else(|| {
                Error::Protocol(format!(
                    "READ_SLOT {n} on {}: a reply of {} bytes that is not a slot of profile {n}",
                    self.node(),
                    reply.len()
                ))
            })
    }

    /// WRITE_SLOT (v1.4): writes `slot` in RAM, and enables, selects and
    /// saves it as its flags say. All or nothing. The caller checks it
    /// first ([`Slot::check`]) and then waits for STATUS: the reinit of an
    /// active profile comes after the ACK.
    pub fn write_slot(&self, slot: &Slot) -> Result<()> {
        self.need_slots(Command::WriteSlot)?;
        self.command(
            Command::WriteSlot,
            &slot.encode(),
            &format!("the write of profile {}", slot.profile),
        )
    }

    /// The sequence number of the last OUT command, as STATUS echoes it.
    pub fn last_seq(&self) -> u16 {
        self.seq.get()
    }

    /// Saves `mode` as the input mode and restarts into it.
    pub fn set_input_mode(&self, mode: InputMode) -> Result<RebootAck> {
        self.need(Caps::SET_INPUT_MODE, Command::SetInputMode)?;
        let value = mode
            .proto_value()
            .filter(|v| *v < 255)
            .and_then(|v| u8::try_from(v).ok())
            .ok_or_else(|| Error::Usage(format!("{} is not an input mode", mode.label())))?;
        self.restarting(
            Command::SetInputMode,
            &[value, 0],
            &format!("input mode {}", mode.label()),
        )
    }
}
