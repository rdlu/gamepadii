//! Endpoints that only our GP2040-CE fork can answer safely, behind a
//! capability check.
//!
//! On stock v0.7.12, `getConfig` runs the web server out of heap and
//! `getI2CPeripheralMap` races the other core on the I2C bus; either hangs
//! the pad's network stack until it is replugged. The fork
//! (`rdlu/GP2040-CE`, branch `gamepadii/control-channel`) fixes both:
//! replies are built in one exactly sized buffer and kept until lwIP has
//! sent them, a reply that won't fit gets a 503 instead, and I2C access is
//! serialised between the cores.
//!
//! A [`ForkFixes`] is the proof that the pad runs such a build. Only
//! [`ForkFixes::check`] makes one, and the reads here take it, so a stock
//! pad can never be asked:
//!
//! ```compile_fail
//! let _ = gamepadii_core::api::ForkFixes { evidence: gamepadii_core::api::Evidence::KnownBuild };
//! ```
//!
//! The rule (`docs/DESIGN.md`, "Fork firmware"), conservative on purpose,
//! since someone else's dev build `v0.7.12-3-g1234567` isn't necessarily
//! fixed:
//!
//! 1. The build's commit (the `-g<hash>` of the version string, or
//!    `boardBuild`) is one of [`FIXED_BUILDS`]: our builds that were
//!    stress-tested with these endpoints. Or
//! 2. the control channel answered HELLO (only the fork has it, and its
//!    branch sits on top of the fixes) and the version isn't a bare release
//!    tag (`v0.7.12`): a release build with the channel isn't one of ours.
//!
//! Anything else, a bare release tag above all, is stock: no capability.
//! So is upstream main as we know it ([`super::line::STOCK_MAIN_BUILDS`],
//! `v0.7.12-119-g8013b41`): it has upstream's I2C fix, but `getConfig`
//! still runs it out of heap, and our fixes for that are upstream PRs, not
//! merged. Fork builds on main join [`FIXED_BUILDS`] once they pass the
//! same stress test.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::FirmwareVersion;
use crate::channel::Hello;
use crate::error::{Error, Result};
use crate::transport::{ApiPath, Method, Transport};

/// The commits of our builds with the web-config heap/reply and I2C fixes,
/// each stress-tested (`getConfig` and `getI2CPeripheralMap`, 10/10 to
/// 20/20 rounds ok) on the S16. The first seven hex digits, as `git
/// describe` prints them.
pub const FIXED_BUILDS: &[&str] = &[
    "85ca0f6", // freeze-fix-rc: the fixes alone (v0.7.12-5-g85ca0f6)
    "8419a38", // the fixes, test boot into web-config (-testboot)
    "f19c11b", // the fixes and the 503 guard, test boot (-testboot)
    "f507f0b", // cc: the control channel (v0.7.12-6-gf507f0b)
    "5eea8a6", // cc-off: the same with the channel compiled out
    "1008647", // cc-testboot (v0.7.12-7-g1008647-testboot)
    "0a23ea3", // cc-led: control channel v1.1 with the LED commands
];

/// Why gamepadii believes the build has the fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Its commit is in [`FIXED_BUILDS`].
    KnownBuild,
    /// The control channel answered HELLO, on a build that isn't a release.
    ControlChannel,
}

/// Proof that the pad runs a fork build with the fixes: what lets
/// [`get_config`] and [`get_i2c_peripheral_map`] be called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForkFixes {
    evidence: Evidence,
}

impl ForkFixes {
    /// The rule in the module docs, on what `getFirmwareVersion` said and
    /// the HELLO reply, if the channel answered one. `None`: stock, or not
    /// known to be fixed.
    pub fn check(fw: &FirmwareVersion, hello: Option<&Hello>) -> Option<ForkFixes> {
        if super::line::is_stock_main(fw) {
            return None;
        }
        let version = fw.version.as_deref().unwrap_or_default();
        let commits = [described_commit(version), fw.board_build.as_deref()];
        if commits.into_iter().flatten().any(is_fixed_build) {
            return Some(ForkFixes {
                evidence: Evidence::KnownBuild,
            });
        }
        if hello.is_some() && described_commit(version).is_some() {
            return Some(ForkFixes {
                evidence: Evidence::ControlChannel,
            });
        }
        None
    }

    pub fn evidence(&self) -> Evidence {
        self.evidence
    }
}

/// Whether `commit` (an abbreviated hash, 7 or more hex digits) is one of
/// [`FIXED_BUILDS`].
pub fn is_fixed_build(commit: &str) -> bool {
    let commit = commit.to_ascii_lowercase();
    commit.len() >= 7
        && commit.bytes().all(|b| b.is_ascii_hexdigit())
        && FIXED_BUILDS.iter().any(|f| commit.starts_with(f))
}

/// The commit in a `git describe` version past a tag:
/// `v0.7.12-6-gf507f0b` (and `v0.7.12-7-g1008647-testboot`) give
/// `f507f0b`; a bare tag (`v0.7.12`) gives `None`.
pub fn described_commit(version: &str) -> Option<&str> {
    let parts: Vec<&str> = version.split('-').collect();
    parts.windows(2).find_map(|w| {
        let hash = w[1].strip_prefix('g')?;
        let counted = !w[0].is_empty() && w[0].bytes().all(|b| b.is_ascii_digit());
        let hex = hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (counted && hex).then_some(hash)
    })
}

const GET_CONFIG: ApiPath = ApiPath::new("/api/getConfig");
const GET_I2C_PERIPHERAL_MAP: ApiPath = ApiPath::new("/api/getI2CPeripheralMap");

fn read(t: &dyn Transport, path: ApiPath, name: &str) -> Result<Value> {
    let reply = t.request(Method::Get, path, None)?;
    match reply.status {
        200 => {}
        // The fork's guard: the reply wouldn't fit in the heap.
        503 => {
            return Err(Error::Protocol(format!(
                "{name}: HTTP 503 (the pad hadn't the memory for the reply)"
            )));
        }
        s => return Err(Error::Protocol(format!("{name}: HTTP {s}"))),
    }
    serde_json::from_slice(&reply.body)
        .map_err(|e| Error::Protocol(format!("{name}: not JSON: {e}")))
}

/// `getConfig`: the whole configuration in one document (about 40 KB on
/// the S16), every section the per-section getters have and the ones they
/// don't (`animationOptions`, among others).
pub fn get_config(t: &dyn Transport, _: &ForkFixes) -> Result<Value> {
    read(t, GET_CONFIG, "getConfig")
}

/// `getI2CPeripheralMap`: a live scan of the I2C buses, `{"i2c0":
/// {"60": true}}` (the OLED at `0x3C`).
pub fn get_i2c_peripheral_map(t: &dyn Transport, _: &ForkFixes) -> Result<Value> {
    read(t, GET_I2C_PERIPHERAL_MAP, "getI2CPeripheralMap")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Caps;

    fn fw(version: &str, build: &str) -> FirmwareVersion {
        FirmwareVersion {
            version: Some(version.into()),
            board_build: Some(build.into()),
            ..FirmwareVersion::default()
        }
    }

    fn hello() -> Hello {
        Hello {
            major: 1,
            minor: 0,
            caps: Caps(0x2f),
            token: 1,
            modes: 0xffff,
        }
    }

    fn evidence(version: &str, build: &str, hello: Option<&Hello>) -> Option<Evidence> {
        ForkFixes::check(&fw(version, build), hello).map(|c| c.evidence())
    }

    #[test]
    fn described_commits() {
        assert_eq!(described_commit("v0.7.12-6-gf507f0b"), Some("f507f0b"));
        assert_eq!(
            described_commit("v0.7.12-7-g1008647-testboot"),
            Some("1008647")
        );
        assert_eq!(described_commit("v0.7.12"), None);
        assert_eq!(described_commit("v0.7.12-rc1"), None);
        assert_eq!(described_commit("v0.7.12-gf507f0b"), None);
        assert_eq!(described_commit(""), None);
    }

    #[test]
    fn stock_release_never_qualifies() {
        assert_eq!(evidence("v0.7.12", "0014e4a", None), None);
        // Even if something on EP0 passed for the channel.
        assert_eq!(evidence("v0.7.12", "0014e4a", Some(&hello())), None);
    }

    #[test]
    fn known_fork_builds_qualify() {
        let known = Some(Evidence::KnownBuild);
        assert_eq!(evidence("v0.7.12-6-gf507f0b", "f507f0b", None), known);
        assert_eq!(evidence("v0.7.12-5-g85ca0f6", "85ca0f6", None), known);
        assert_eq!(
            evidence("v0.7.12-7-g1008647-testboot", "1008647", None),
            known
        );
        // boardBuild alone is enough, longer hashes and any case too.
        assert_eq!(evidence("", "F507F0BA", None), known);
    }

    #[test]
    fn someone_elses_dev_build_needs_the_channel() {
        assert_eq!(evidence("v0.7.12-3-g1234567", "1234567", None), None);
        assert_eq!(
            evidence("v0.7.12-3-g1234567", "1234567", Some(&hello())),
            Some(Evidence::ControlChannel)
        );
    }

    #[test]
    fn stock_main_never_qualifies() {
        assert_eq!(evidence("v0.7.12-119-g8013b41", "8013b41", None), None);
        assert_eq!(
            evidence("v0.7.12-119-g8013b41", "8013b41", Some(&hello())),
            None
        );
    }

    #[test]
    fn builds_before_the_fixes_are_not_listed() {
        // freeze-investigation: only two of the five fixes.
        assert!(!is_fixed_build("7822dc7"));
        assert!(!is_fixed_build("0014e4a"));
        assert!(!is_fixed_build("f507f0"));
        assert!(!is_fixed_build("f507f0z"));
    }
}
