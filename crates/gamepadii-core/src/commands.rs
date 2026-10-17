//! What the front ends run. Each command returns plain serde data and never
//! prints; `gamepadii`'s `output` module renders it.

use serde::Serialize;

use crate::discovery::{self, Device};
use crate::error::{Error, Result};
use crate::sysfs::Root;

/// `gamepadii status`: every GP2040-CE device on USB, with its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub devices: Vec<Device>,
}

/// [`Status`] for the machine under `root`; [`Error::NotFound`] when no
/// device has an id GP2040-CE uses.
pub fn status(root: &Root) -> Result<Status> {
    let devices = discovery::scan(root);
    if devices.is_empty() {
        return Err(Error::NotFound(
            "no GP2040-CE device on USB (is it plugged in?)".into(),
        ));
    }
    Ok(Status { devices })
}
