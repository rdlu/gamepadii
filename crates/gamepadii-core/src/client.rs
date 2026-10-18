//! A connection to a pad in web-config: finds it (or takes `--host`),
//! checks it answers, and remembers its firmware version.

use std::cell::OnceCell;

use crate::api::{self, FirmwareVersion, ForkFixes, Line};
use crate::channel::Hello;
use crate::discovery::{self, Device};
use crate::error::{Error, Result};
use crate::mode::InputMode;
use crate::sysfs::Root;
use crate::transport::Transport;
use crate::transport::http::{self, Http};

/// How to get into web-config, for messages. S2 is Start and B3/B4 are the
/// left face buttons in GP2040-CE's naming, after the active profile's
/// remapping.
pub const WEBCONFIG_HINT: &str = "hold S2 (Start) while plugging the pad in, \
     or hold S2+B3+B4 for 4 seconds while it runs; these are logical buttons, \
     so the active profile's mapping decides which keys they are";

pub struct Client {
    transport: Box<dyn Transport>,
    firmware: FirmwareVersion,
    /// The control channel's HELLO, when the same pad answered one.
    hello: Option<Hello>,
    /// The firmware line, worked out on first use.
    line: OnceCell<Line>,
}

impl Client {
    /// Finds a pad in web-config under `root` and connects to it at
    /// [`http::DEFAULT_HOST`]. A pad in a gamepad mode is
    /// [`Error::NeedsUser`] (it has to be switched by hand); no pad at all
    /// is [`Error::NotFound`].
    pub fn connect(root: &Root) -> Result<Client> {
        require_webconfig(root)?;
        Client::open(Box::new(Http::new(http::DEFAULT_HOST)?))
    }

    /// Connects over `transport` with no USB check (`--host`, tests), and
    /// probes the firmware version.
    pub fn open(transport: Box<dyn Transport>) -> Result<Client> {
        let firmware = api::firmware_version(&*transport)?;
        Ok(Client {
            transport,
            firmware,
            hello: None,
            line: OnceCell::new(),
        })
    }

    pub fn transport(&self) -> &dyn Transport {
        &*self.transport
    }

    /// From `getFirmwareVersion` at connect time.
    pub fn firmware(&self) -> &FirmwareVersion {
        &self.firmware
    }

    /// The pad's control channel answered `hello` (the flows probe the
    /// device they connected to): evidence for [`Client::fork_fixes`].
    pub fn with_hello(mut self, hello: Hello) -> Self {
        self.hello = Some(hello);
        self
    }

    /// Whether this pad runs our fork with the web-config fixes, from the
    /// firmware version and the channel's HELLO (the rule is in
    /// [`api::fork`]). `None` for stock.
    pub fn fork_fixes(&self) -> Option<ForkFixes> {
        ForkFixes::check(&self.firmware, self.hello.as_ref())
    }

    /// Which firmware line the pad is on (the rule is in [`api::line`]):
    /// from the version when that is enough, else from one
    /// `getGamepadOptions` read, the first time it is asked; remembered.
    pub fn line(&self) -> Result<Line> {
        if let Some(l) = self.line.get() {
            return Ok(*l);
        }
        let l = api::line::detect(&*self.transport, &self.firmware)?;
        Ok(*self.line.get_or_init(|| l))
    }

    /// [`Client::line`] when `getGamepadOptions` has been read already
    /// (`options`): no second read.
    pub fn line_with(&self, options: &serde_json::Value) -> Line {
        *self.line.get_or_init(|| {
            api::line::from_version(&self.firmware).unwrap_or(Line {
                line: api::line::from_gamepad_options(options),
                evidence: api::LineEvidence::Shape,
            })
        })
    }

    /// Where it talks to: `http://192.168.7.1`.
    pub fn host(&self) -> String {
        self.transport.describe()
    }
}

/// The pad in web-config under `root`, or why there isn't one.
pub fn require_webconfig(root: &Root) -> Result<Device> {
    let devices = discovery::scan(root);
    if let Some(d) = devices
        .iter()
        .find(|d| d.mode == Some(InputMode::Webconfig))
    {
        return Ok(d.clone());
    }
    match devices.first() {
        None => Err(Error::NotFound(
            "no GP2040-CE device on USB (is it plugged in?)".into(),
        )),
        Some(d) => Err(Error::NeedsUser(not_in_webconfig(d))),
    }
}

/// What to tell someone whose pad `d` isn't in web-config, for a command
/// that needs it.
pub fn not_in_webconfig(d: &Device) -> String {
    format!(
        "the pad is in {} mode, not web-config. To switch: {WEBCONFIG_HINT}. \
         Then run this again",
        mode_name(d)
    )
}

fn mode_name(d: &Device) -> String {
    match d.mode {
        Some(m) => m.label().to_string(),
        None => d
            .candidates
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join(" or "),
    }
}
