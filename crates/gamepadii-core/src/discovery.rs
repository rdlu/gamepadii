//! Finds GP2040-CE devices on USB by reading `sys/bus/usb/devices` under a
//! [`Root`]: the same attributes udev reports, but testable against fixture
//! trees. The guided flows watch for re-enumeration by polling this (see
//! `wait`): simpler than a udev monitor, and testable with fixtures.

use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::mode::{self, Confidence, InputMode};
use crate::sysfs::{self, Root};

/// A USB device whose id GP2040-CE uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    /// The mode, when the id belongs to exactly one (see `candidates`).
    pub mode: Option<InputMode>,
    /// Every mode that produces this id.
    pub candidates: Vec<InputMode>,
    pub confidence: Confidence,
    /// Why the confidence isn't higher; `null` when confirmed.
    pub note: Option<String>,
    #[serde(serialize_with = "hex4")]
    pub vid: u16,
    #[serde(serialize_with = "hex4")]
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    /// USB bus number (`busnum`).
    pub bus: Option<u16>,
    /// Device number on the bus (`devnum`); changes on every re-enumeration.
    pub devnum: Option<u16>,
    /// The kernel's name for the port path, e.g. `3-1` or `6-1.2.3`. Stable
    /// while the pad stays in the same socket.
    pub port: String,
    /// The device's directory, symlinks resolved (`/sys/devices/...`).
    pub sysfs_path: Option<PathBuf>,
    /// The network interface, in web-config mode (e.g. `usb0`).
    pub netdev: Option<String>,
}

impl Device {
    /// `045e:028e`.
    pub fn usb_id(&self) -> String {
        format!("{:04x}:{:04x}", self.vid, self.pid)
    }
}

fn hex4<S: Serializer>(v: &u16, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:04x}"))
}

/// Something that lists the GP2040-CE devices on USB: a [`Root`] (sysfs),
/// or a simulated pad in tests. The guided flows poll it to see the pad
/// re-enumerate.
pub trait Scan {
    fn scan(&self) -> Vec<Device>;
}

impl Scan for Root {
    fn scan(&self) -> Vec<Device> {
        scan(self)
    }
}

/// Every USB device under `root` whose id GP2040-CE uses, sorted by bus and
/// port. A missing `sys/bus/usb` means no devices, not an error.
pub fn scan(root: &Root) -> Vec<Device> {
    let dir = root.path("sys/bus/usb/devices");
    // Interfaces are `3-1:1.0`; devices are `3-1`, root hubs `usb3`.
    let mut out: Vec<Device> = sysfs::names(&dir, |n| !n.contains(':'))
        .into_iter()
        .filter_map(|name| read_device(root, &dir.join(&name), name))
        .collect();
    out.sort_by(|a, b| (a.bus, &a.port).cmp(&(b.bus, &b.port)));
    out
}

fn read_device(root: &Root, path: &std::path::Path, name: String) -> Option<Device> {
    let vid = sysfs::read_hex(path.join("idVendor"))?;
    let pid = sysfs::read_hex(path.join("idProduct"))?;
    let manufacturer = sysfs::read(path.join("manufacturer"));
    let product = sysfs::read(path.join("product"));
    let det = mode::detect(vid, pid, manufacturer.as_deref(), product.as_deref())?;
    let netdev = if det.candidates.contains(&InputMode::Webconfig) {
        netdev(path, &name)
    } else {
        None
    };
    Some(Device {
        mode: det.mode,
        candidates: det.candidates,
        confidence: det.confidence,
        note: det.note,
        vid,
        pid,
        manufacturer,
        product,
        serial: sysfs::read(path.join("serial")),
        bus: sysfs::read_dec(path.join("busnum")),
        devnum: sysfs::read_dec(path.join("devnum")),
        sysfs_path: root.resolve(path),
        port: name,
        netdev,
    })
}

/// The first network interface under any of the device's USB interfaces
/// (`3-1:1.0/net/usb0`).
fn netdev(path: &std::path::Path, name: &str) -> Option<String> {
    let prefix = format!("{name}:");
    sysfs::names(path, |n| n.starts_with(&prefix))
        .into_iter()
        .find_map(|intf| {
            sysfs::names(path.join(intf).join("net"), |_| true)
                .into_iter()
                .next()
        })
}
