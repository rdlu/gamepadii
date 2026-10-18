//! Which GP2040-CE line the pad runs: the v0.7.12 release line, or
//! upstream main (unreleased past v0.7.12, checked at 8013b41). Their web
//! APIs differ (`docs/DESIGN.md`, "Firmware lines"): endpoints were removed
//! and added, and `setGamepadOptions` / `setDisplayOptions` read more keys
//! on main, so the allowlist and the setters' key sets are per line.
//!
//! The version string can't tell them apart: a build of main past the tag
//! says `v0.7.12-119-g8013b41`, just as our v0.7.12 fork builds say
//! `v0.7.12-6-gf507f0b`, and a commit count is no line. The rule, in order:
//!
//! 1. A commit we know (the `-g<hash>` of the version, or `boardBuild`) is
//!    in [`KNOWN_BUILDS`] with its line: our fork builds, the S16's stock
//!    build, upstream main 8013b41.
//! 2. The bare release tag `v0.7.12` is the release line.
//! 3. Anything else goes by the shape of `getGamepadOptions`, which both
//!    lines have and which is a plain read on both: main's getter writes the
//!    `switch2Pro*` keys (Switch 2 Pro mode, after v0.7.12), v0.7.12's has
//!    none. No endpoint that one line lacks is ever asked for: an unknown
//!    path gets no reply at all from the firmware's lwIP httpd (no 404 page
//!    in its file system), which reads as a dropped connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fork::described_commit;
use super::model::FirmwareVersion;
use super::{Endpoint, read_raw};
use crate::error::Result;
use crate::transport::Transport;

/// A GP2040-CE firmware line, as far as its web API goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FirmwareLine {
    /// The v0.7.12 release, and builds on top of it (our fork's).
    #[serde(rename = "v0.7.12")]
    V0712,
    /// Upstream main past v0.7.12 (unreleased; checked at 8013b41).
    #[serde(rename = "main")]
    Main,
}

impl FirmwareLine {
    /// `v0.7.12` or `main`.
    pub fn key(self) -> &'static str {
        match self {
            FirmwareLine::V0712 => "v0.7.12",
            FirmwareLine::Main => "main",
        }
    }

    /// The endpoints verified safe to read on this line, in alphabetical
    /// order (the reasons are in `docs/DESIGN.md`).
    pub fn endpoints(self) -> &'static [Endpoint] {
        match self {
            FirmwareLine::V0712 => V0712_ENDPOINTS,
            FirmwareLine::Main => MAIN_ENDPOINTS,
        }
    }

    /// Whether `ep` is in this line's allowlist.
    pub fn allows(self, ep: Endpoint) -> bool {
        self.endpoints().contains(&ep)
    }
}

impl std::fmt::Display for FirmwareLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// The S16 capture's endpoints (`tests/fixtures/api/s16-v0.7.12/`).
const V0712_ENDPOINTS: &[Endpoint] = &[
    Endpoint::GetAddonsOptions,
    Endpoint::GetButtonLayoutDefs,
    Endpoint::GetButtonLayouts,
    Endpoint::GetCustomTheme,
    Endpoint::GetDisplayOptions,
    Endpoint::GetExpansionPins,
    Endpoint::GetFirmwareVersion,
    Endpoint::GetGamepadOptions,
    Endpoint::GetHETriggerCalibration,
    Endpoint::GetHETriggerOptions,
    Endpoint::GetKeyMappings,
    Endpoint::GetLedOptions,
    Endpoint::GetMacroAddonOptions,
    Endpoint::GetMemoryReport,
    Endpoint::GetPeripheralOptions,
    Endpoint::GetPinMappings,
    Endpoint::GetProfileOptions,
    Endpoint::GetReactiveLEDs,
    Endpoint::GetSplashImage,
    Endpoint::GetUsedPins,
    Endpoint::GetWiiControls,
];

/// v0.7.12's, less what main removed (`getCustomTheme`,
/// `getHETriggerCalibration`, `getHETriggerOptions`), plus main's new
/// plain reads. Read from main's `webconfig.cpp`, not yet on a pad.
const MAIN_ENDPOINTS: &[Endpoint] = &[
    Endpoint::GetAddonsOptions,
    Endpoint::GetAnimationProtoOptions,
    Endpoint::GetBoardDefinition,
    Endpoint::GetBootModeOptions,
    Endpoint::GetButtonLayoutDefs,
    Endpoint::GetButtonLayouts,
    Endpoint::GetDisplayOptions,
    Endpoint::GetExpansionPins,
    Endpoint::GetFirmwareVersion,
    Endpoint::GetGamepadOptions,
    Endpoint::GetHETriggerCalibrations,
    Endpoint::GetKeyMappings,
    Endpoint::GetLedOptions,
    Endpoint::GetLightsDataOptions,
    Endpoint::GetMacroAddonOptions,
    Endpoint::GetMemoryReport,
    Endpoint::GetPeripheralOptions,
    Endpoint::GetPinMappings,
    Endpoint::GetProfileOptions,
    Endpoint::GetReactiveLEDs,
    Endpoint::GetSplashImage,
    Endpoint::GetUsedPins,
    Endpoint::GetWiiControls,
];

/// Builds whose line we know, by commit (the first seven hex digits, as
/// `git describe` prints them). Every one of `fork::FIXED_BUILDS` must be
/// here: a new fork build says which line it is on.
pub const KNOWN_BUILDS: &[(&str, FirmwareLine)] = &[
    // The S16's stock firmware (Haute42COSMOX, v0.7.12).
    ("0014e4a", FirmwareLine::V0712),
    // Our fork, on v0.7.12 (see fork::FIXED_BUILDS).
    ("85ca0f6", FirmwareLine::V0712),
    ("8419a38", FirmwareLine::V0712),
    ("f19c11b", FirmwareLine::V0712),
    ("f507f0b", FirmwareLine::V0712),
    ("5eea8a6", FirmwareLine::V0712),
    ("1008647", FirmwareLine::V0712),
    ("0a23ea3", FirmwareLine::V0712),
    // Upstream main, stock (no fixes, no control channel).
    ("8013b41", FirmwareLine::Main),
    // Our fork, on main: control channel v1.2 with the test boot option
    // (on the S16 since 2026-10-18, `v0.7.12-128-ga42b806-dirty`), and
    // v1.3 with the board ID, v1.4 with the profile slots, v1.5 with the
    // LED profiles, v1.6 with the config blob and v1.7 with the input
    // snapshot (on the S16 since 2026-10-18, `v0.7.12-133-g48a9521`).
    ("a42b806", FirmwareLine::Main),
    ("6160a03", FirmwareLine::Main),
    ("cb01f70", FirmwareLine::Main),
    ("77620cd", FirmwareLine::Main),
    ("61ab30f", FirmwareLine::Main),
    ("48a9521", FirmwareLine::Main),
];

/// Upstream main builds with none of our fixes: `getConfig` still hangs
/// them, so they never count as fork builds, whatever else answers.
pub const STOCK_MAIN_BUILDS: &[&str] = &["8013b41"];

/// The release tag of the v0.7.12 line.
pub const RELEASE_TAG: &str = "v0.7.12";

/// Why gamepadii believes the pad is on its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEvidence {
    /// Its commit is in [`KNOWN_BUILDS`].
    KnownBuild,
    /// The version is the bare release tag.
    ReleaseTag,
    /// The shape of `getGamepadOptions` (main's `switch2Pro*` keys).
    Shape,
}

/// The pad's line and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub line: FirmwareLine,
    pub evidence: LineEvidence,
}

/// Whether `a` (an abbreviated hash) is the commit `known` (seven hex
/// digits), in any case.
pub(crate) fn same_commit(a: &str, known: &str) -> bool {
    let a = a.to_ascii_lowercase();
    a.len() >= 7 && a.bytes().all(|b| b.is_ascii_hexdigit()) && a.starts_with(known)
}

/// The commits a build names: the `-g<hash>` of its version, its
/// `boardBuild`.
fn commits(fw: &FirmwareVersion) -> impl Iterator<Item = &str> {
    let version = fw.version.as_deref().unwrap_or_default();
    [described_commit(version), fw.board_build.as_deref()]
        .into_iter()
        .flatten()
}

/// Whether the build is upstream main with none of our fixes.
pub fn is_stock_main(fw: &FirmwareVersion) -> bool {
    commits(fw).any(|c| STOCK_MAIN_BUILDS.iter().any(|k| same_commit(c, k)))
}

/// Rules 1 and 2: the line from what `getFirmwareVersion` said alone, when
/// that is enough.
pub fn from_version(fw: &FirmwareVersion) -> Option<Line> {
    let known = commits(fw).find_map(|c| {
        KNOWN_BUILDS
            .iter()
            .find(|(k, _)| same_commit(c, k))
            .map(|(_, l)| *l)
    });
    if let Some(line) = known {
        return Some(Line {
            line,
            evidence: LineEvidence::KnownBuild,
        });
    }
    (fw.version.as_deref() == Some(RELEASE_TAG)).then_some(Line {
        line: FirmwareLine::V0712,
        evidence: LineEvidence::ReleaseTag,
    })
}

/// Rule 3: the line from `getGamepadOptions`' object.
pub fn from_gamepad_options(options: &Value) -> FirmwareLine {
    if options.get(MAIN_MARKER).is_some() {
        FirmwareLine::Main
    } else {
        FirmwareLine::V0712
    }
}

/// The key only main's `getGamepadOptions` has.
pub const MAIN_MARKER: &str = "switch2ProIdentity";

/// The whole rule (module docs): from the version when it is enough, else
/// one `getGamepadOptions` read.
pub fn detect(t: &dyn Transport, fw: &FirmwareVersion) -> Result<Line> {
    if let Some(l) = from_version(fw) {
        return Ok(l);
    }
    let options = read_raw(t, Endpoint::GetGamepadOptions)?;
    Ok(Line {
        line: from_gamepad_options(&options),
        evidence: LineEvidence::Shape,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::fork::FIXED_BUILDS;

    fn fw(version: &str, build: &str) -> FirmwareVersion {
        FirmwareVersion {
            version: Some(version.into()),
            board_build: Some(build.into()),
            ..FirmwareVersion::default()
        }
    }

    fn line(version: &str, build: &str) -> Option<(FirmwareLine, LineEvidence)> {
        from_version(&fw(version, build)).map(|l| (l.line, l.evidence))
    }

    #[test]
    fn known_builds_and_the_release_tag() {
        use FirmwareLine::*;
        use LineEvidence::*;
        assert_eq!(line("v0.7.12", "0014e4a"), Some((V0712, KnownBuild)));
        assert_eq!(line("v0.7.12", ""), Some((V0712, ReleaseTag)));
        assert_eq!(
            line("v0.7.12-6-gf507f0b", "f507f0b"),
            Some((V0712, KnownBuild))
        );
        assert_eq!(
            line("v0.7.12-119-g8013b41", "8013b41"),
            Some((Main, KnownBuild))
        );
        assert_eq!(line("", "8013B41F"), Some((Main, KnownBuild)));
    }

    #[test]
    fn a_commit_count_is_no_line() {
        // Someone's build past the tag, a later tag: the shape decides.
        assert_eq!(line("v0.7.12-119-g1234567", "1234567"), None);
        assert_eq!(line("v0.7.12-3-g1234567", "1234567"), None);
        assert_eq!(line("v0.7.13", "abcdef0"), None);
    }

    #[test]
    fn every_fork_build_has_a_line() {
        for f in FIXED_BUILDS {
            assert!(KNOWN_BUILDS.iter().any(|(k, _)| k == f), "{f}");
        }
    }

    #[test]
    fn the_shape() {
        let main = serde_json::json!({"inputMode": 0, "switch2ProIdentity": 0});
        let old = serde_json::json!({"inputMode": 0});
        assert_eq!(from_gamepad_options(&main), FirmwareLine::Main);
        assert_eq!(from_gamepad_options(&old), FirmwareLine::V0712);
    }

    #[test]
    fn stock_main() {
        assert!(is_stock_main(&fw("v0.7.12-119-g8013b41", "8013b41")));
        assert!(!is_stock_main(&fw("v0.7.12-6-gf507f0b", "f507f0b")));
        assert!(!is_stock_main(&fw("v0.7.12", "0014e4a")));
    }

    #[test]
    fn the_lists_are_sorted() {
        for line in [FirmwareLine::V0712, FirmwareLine::Main] {
            let names: Vec<_> = line.endpoints().iter().map(|e| e.name()).collect();
            let mut sorted = names.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(names, sorted, "{line}");
        }
    }
}
