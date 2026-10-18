//! [`ControlTransport`] over Linux usbfs: `USBDEVFS_CONTROL` on
//! `/dev/bus/usb/BBB/DDD`, opened read-write (the kernel refuses ioctls on
//! a read-only fd).
//!
//! A vendor request to the device needs no claimed interface:
//! `check_ctrlrecip()` in `drivers/usb/core/devio.c` returns before any
//! interface check for vendor requests, so xpad or usbhid stay bound and
//! the kernel serialises our transfer on EP0 with its own. The node is only
//! `crw-rw-r-- root` by default; `gamepadii setup udev` adds a `uaccess`
//! rule for GP2040-CE's ids.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::time::Duration;

use super::{ControlTransport, Setup, UsbError};
use crate::discovery::Device;
use crate::sysfs::Root;
use crate::udev::{self, Installed};
use crate::wait::Clock;

fn rule_installed(root: &Root) -> bool {
    udev::installed(root) != Installed::No
}

/// `struct usbdevfs_ctrltransfer` from `<linux/usbdevice_fs.h>`.
#[repr(C)]
struct CtrlTransfer {
    b_request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    w_length: u16,
    /// Milliseconds.
    timeout: u32,
    data: *mut libc::c_void,
}

/// `_IOWR('U', 0, struct usbdevfs_ctrltransfer)`, from the struct's size
/// (24 bytes on 64-bit), so it is right on every architecture libc knows.
const USBDEVFS_CONTROL: libc::Ioctl = libc::_IOWR::<CtrlTransfer>(b'U' as u32, 0);

/// An open device node.
pub struct Usbfs {
    file: File,
    node: String,
}

/// Maps a usbfs errno to what the caller does about it.
pub fn classify(e: std::io::Error) -> UsbError {
    match e.raw_os_error() {
        Some(libc::EPIPE) => UsbError::Stall,
        Some(libc::EACCES | libc::EPERM) => UsbError::NoAccess,
        Some(libc::ENODEV | libc::ESHUTDOWN | libc::EPROTO | libc::ENOENT | libc::ENXIO) => {
            UsbError::Gone
        }
        Some(libc::ETIMEDOUT) => UsbError::Timeout,
        _ => UsbError::Io(e),
    }
}

/// A node younger than this is new on USB: its ACL can still be on the
/// way ([`super::wait_for_acl`]).
pub const NEW_NODE: Duration = Duration::from_secs(5);

impl Usbfs {
    /// Opens `d`'s node under `root` (`/` live; a fixture tree in tests,
    /// which has no `dev/`, so tests can never reach a real pad). Never as
    /// root ([`UsbError::AsRoot`]).
    pub fn open(root: &Root, d: &Device) -> Result<Usbfs, UsbError> {
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return Err(UsbError::AsRoot);
        }
        let node = super::node_path(d).ok_or(UsbError::Gone)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path(&node))
            .map_err(classify)?;
        Ok(Usbfs { file, node })
    }

    /// [`Usbfs::open`], and when it fails with `EACCES` on a node that is
    /// new on USB with the udev rule installed, again for a short time
    /// ([`super::wait_for_acl`]): udev can apply the rule's ACL a moment
    /// after it makes the node.
    pub fn open_new(root: &Root, d: &Device, clock: &dyn Clock) -> Result<Usbfs, UsbError> {
        match Self::open(root, d) {
            Err(UsbError::NoAccess) if Self::is_new(root, d) && rule_installed(root) => {
                super::wait_for_acl(clock, || Self::open(root, d))
            }
            r => r,
        }
    }

    /// Whether `d`'s node was made less than [`NEW_NODE`] ago (its change
    /// time: udev makes the node, then sets its ACL).
    fn is_new(root: &Root, d: &Device) -> bool {
        use std::os::unix::fs::MetadataExt;
        let Some(node) = super::node_path(d) else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(root.path(&node)) else {
            return false;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let ctime = u64::try_from(meta.ctime()).unwrap_or(0);
        now.saturating_sub(ctime) < NEW_NODE.as_secs()
    }

    fn transfer(&self, setup: Setup, buf: &mut [u8], timeout: Duration) -> Result<usize, UsbError> {
        let len = u16::try_from(buf.len()).map_err(|_| {
            UsbError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "control transfer longer than 64 KiB",
            ))
        })?;
        let mut xfer = CtrlTransfer {
            b_request_type: setup.request_type,
            b_request: setup.request,
            w_value: setup.value,
            w_index: setup.index,
            w_length: len,
            timeout: u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX),
            data: buf.as_mut_ptr().cast(),
        };
        loop {
            // SAFETY: `xfer` is a correctly laid out usbdevfs_ctrltransfer
            // whose `data` points at `buf`, valid for `w_length` bytes in
            // both directions for the duration of the call; the fd is open.
            let n = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    USBDEVFS_CONTROL,
                    &mut xfer as *mut CtrlTransfer,
                )
            };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(classify(e));
            }
        }
    }
}

impl ControlTransport for Usbfs {
    fn control_in(&self, setup: Setup, len: u16, timeout: Duration) -> Result<Vec<u8>, UsbError> {
        let mut buf = vec![0u8; usize::from(len)];
        let n = self.transfer(setup, &mut buf, timeout)?;
        buf.truncate(n.min(buf.len()));
        Ok(buf)
    }

    fn control_out(&self, setup: Setup, data: &[u8], timeout: Duration) -> Result<(), UsbError> {
        let mut buf = data.to_vec();
        self.transfer(setup, &mut buf, timeout).map(|_| ())
    }

    fn describe(&self) -> String {
        self.node.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ioctl_number_matches_the_kernel_header() {
        // 64-bit x86/arm: _IOWR('U', 0, 24 bytes) = 0xc0185500.
        #[cfg(all(
            target_pointer_width = "64",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        #[allow(clippy::unnecessary_cast)] // c_ulong on glibc, c_int on musl
        let number = USBDEVFS_CONTROL as u32;
        assert_eq!(number, 0xc018_5500);
        assert_eq!(
            std::mem::size_of::<CtrlTransfer>(),
            if cfg!(target_pointer_width = "64") {
                24
            } else {
                16
            }
        );
    }

    #[test]
    fn errnos_map_to_what_the_caller_does() {
        let e = |n| classify(std::io::Error::from_raw_os_error(n));
        assert!(matches!(e(libc::EPIPE), UsbError::Stall));
        assert!(matches!(e(libc::EACCES), UsbError::NoAccess));
        assert!(matches!(e(libc::ENODEV), UsbError::Gone));
        assert!(matches!(e(libc::ESHUTDOWN), UsbError::Gone));
        assert!(matches!(e(libc::EPROTO), UsbError::Gone));
        assert!(matches!(e(libc::ENOENT), UsbError::Gone));
        assert!(matches!(e(libc::ETIMEDOUT), UsbError::Timeout));
        assert!(matches!(e(libc::EIO), UsbError::Io(_)));
    }

    #[test]
    fn a_fixture_root_has_no_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let d = crate::sim::device(crate::mode::InputMode::Xinput, 7);
        assert!(matches!(
            Usbfs::open(&Root::new(dir.path()), &d),
            Err(UsbError::Gone)
        ));
    }
}
