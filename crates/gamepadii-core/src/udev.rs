//! The udev rule that lets the logged-in user open GP2040-CE pads' USB
//! device nodes read-write, which the control channel needs
//! (`/dev/bus/usb/BBB/DDD` is `crw-rw-r-- root` by default). Opt-in, through
//! `gamepadii setup udev` (decided in issue #2): most of the ids are
//! borrowed from real controllers, so the rule covers those too.
//!
//! The file sorts before `73-seat-late.rules`, where systemd-logind applies
//! `uaccess`.

use std::fs;

use serde::Serialize;

use crate::mode::{self, InputMode};
use crate::sysfs::Root;

pub const RULE_PATH: &str = "/etc/udev/rules.d/70-gamepadii.rules";

/// TinyUSB's example vendor id, which GP2040-CE keeps for Keyboard and
/// web-config; matched as a whole vendor.
const TINYUSB_VID: u16 = 0xcafe;

/// One line of the rule: an id and what else carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Grant {
    /// `045e:028e`, or `cafe:*` for a whole vendor.
    pub usb_id: String,
    pub modes: Vec<InputMode>,
    /// Real devices with this id, which the rule also covers; `null` when
    /// only GP2040-CE uses it.
    pub also_matches: Option<&'static str>,
    #[serde(skip)]
    vid: u16,
    #[serde(skip)]
    pid: Option<u16>,
}

impl Grant {
    fn line(&self) -> String {
        let mut l = format!(
            "SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{:04x}\"",
            self.vid
        );
        if let Some(pid) = self.pid {
            l.push_str(&format!(", ATTR{{idProduct}}==\"{pid:04x}\""));
        }
        l.push_str(", TAG+=\"uaccess\"");
        l
    }
}

/// Every id in [`mode::KNOWN`], with TinyUSB's vendor id as one line.
/// BOOTSEL is in (for PICOBOOT later); it is the boot ROM of every RP2040
/// and RP2350.
pub fn grants() -> Vec<Grant> {
    let mut out = vec![Grant {
        usb_id: format!("{TINYUSB_VID:04x}:*"),
        modes: mode::KNOWN
            .iter()
            .filter(|k| k.vid == TINYUSB_VID)
            .flat_map(|k| k.modes.iter().copied())
            .collect(),
        also_matches: Some("other hobby devices that kept TinyUSB's example vendor id"),
        vid: TINYUSB_VID,
        pid: None,
    }];
    out.extend(
        mode::KNOWN
            .iter()
            .filter(|k| k.vid != TINYUSB_VID)
            .map(|k| Grant {
                usb_id: format!("{:04x}:{:04x}", k.vid, k.pid),
                modes: k.modes.to_vec(),
                also_matches: k.shared_with,
                vid: k.vid,
                pid: Some(k.pid),
            }),
    );
    out
}

/// The rule file's text.
pub fn rule() -> String {
    let mut out = String::from(
        "# gamepadii: read-write access for the logged-in user to the USB device nodes\n\
         # (/dev/bus/usb/BBB/DDD) of GP2040-CE pads, for gamepadii's control channel\n\
         # (status, mode, profile switch and reboot with no button press).\n\
         # Written by `gamepadii setup udev`; delete this file to revoke it.\n\
         #\n\
         # GP2040-CE borrows most of its USB ids from real controllers, so this also\n\
         # covers a real Xbox 360 pad, a HORI pad, a DualShock, ... plugged into this\n\
         # machine: their device nodes become writable for the seat user too (as\n\
         # Steam's rules do for hidraw). Input devices (evdev, hidraw) are untouched.\n\
         #\n\
         # Custom USB ids (usbOverrideID): copy a line and put your VID:PID in it.\n",
    );
    for g in grants() {
        let modes = g.modes.iter().map(|m| m.label()).collect::<Vec<_>>();
        out.push_str(&format!("\n# {}", modes.join(", ")));
        if let Some(who) = g.also_matches {
            out.push_str(&format!("; also {who}"));
        }
        out.push('\n');
        out.push_str(&g.line());
        out.push('\n');
    }
    out
}

/// Whether the rule is installed under `root`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Installed {
    No,
    /// Exactly this gamepadii's rule.
    Current,
    /// A file is there, but another version or a hand-made one.
    Differs,
}

pub fn installed(root: &Root) -> Installed {
    match fs::read_to_string(root.path(RULE_PATH)) {
        Err(_) => Installed::No,
        Ok(text) if text == rule() => Installed::Current,
        Ok(_) => Installed::Differs,
    }
}

/// `gamepadii setup udev`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub path: &'static str,
    pub installed: Installed,
    /// What `--install` did: `installed`, or `none` when it already was;
    /// `null` without `--install`.
    pub action: Option<Action>,
    pub grants: Vec<Grant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Installed,
    None,
}

pub fn report(root: &Root) -> Report {
    Report {
        path: RULE_PATH,
        installed: installed(root),
        action: None,
        grants: grants(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_id_once() {
        let g = grants();
        for k in mode::KNOWN {
            let covered = g
                .iter()
                .filter(|g| g.vid == k.vid && g.pid.is_none_or(|p| p == k.pid))
                .count();
            assert_eq!(covered, 1, "{:04x}:{:04x}", k.vid, k.pid);
        }
        assert_eq!(g[0].usb_id, "cafe:*");
        assert_eq!(g[0].modes, [InputMode::Keyboard, InputMode::Webconfig]);
    }

    /// Only TinyUSB's example vendor id is matched as a whole vendor: every
    /// other line names the exact VID:PID that the firmware uses, never a
    /// vendor such as Sony (054c) or Microsoft (045e).
    #[test]
    fn never_matches_a_whole_vendor_but_tinyusb() {
        for g in grants() {
            assert!(
                g.pid.is_some() || g.vid == TINYUSB_VID,
                "{} matches a whole vendor",
                g.usb_id
            );
        }
        let whole: Vec<_> = rule()
            .lines()
            .filter(|l| l.starts_with("SUBSYSTEM") && !l.contains("idProduct"))
            .map(String::from)
            .collect();
        assert_eq!(whole.len(), 1, "{whole:?}");
        assert!(whole[0].contains(r#"ATTR{idVendor}=="cafe""#));
    }

    /// The dev rule on the author's machine (045e:028e, cafe:*, 2e8a:0003)
    /// is a subset of this one, under the same file name.
    #[test]
    fn is_a_superset_of_the_dev_rule() {
        let r = rule();
        for line in [
            r#"SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="045e", ATTR{idProduct}=="028e", TAG+="uaccess""#,
            r#"SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="cafe", TAG+="uaccess""#,
            r#"SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="2e8a", ATTR{idProduct}=="0003", TAG+="uaccess""#,
        ] {
            assert!(r.lines().any(|l| l == line), "{line}");
        }
        assert!(RULE_PATH.ends_with("/70-gamepadii.rules"));
    }

    #[test]
    fn installed_is_read_under_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path());
        assert_eq!(installed(&root), Installed::No);
        let path = root.path(RULE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# old\n").unwrap();
        assert_eq!(installed(&root), Installed::Differs);
        fs::write(&path, rule()).unwrap();
        assert_eq!(installed(&root), Installed::Current);
    }
}
