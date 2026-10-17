//! Small sysfs readers that take a [`Root`]: `/` on a live machine, a
//! fixture directory in tests. Nothing here touches an absolute path itself.
//! A missing or unreadable attribute is `None`: an absent file is
//! information, not an error.

use std::fs;
use std::path::{Path, PathBuf};

/// The filesystem root sysfs paths are resolved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root(PathBuf);

impl Root {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// The running machine's `/`.
    pub fn live() -> Self {
        Self::new("/")
    }

    /// The directory this root stands for.
    pub fn base(&self) -> &Path {
        &self.0
    }

    /// `rel` (e.g. `sys/bus/usb/devices`) under this root. A leading `/` is
    /// ignored, so `/sys/...` and `sys/...` mean the same.
    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        let rel = rel.as_ref();
        self.0.join(rel.strip_prefix("/").unwrap_or(rel))
    }

    /// `path` (under this root) with symlinks resolved, shown as it would be
    /// on a live machine: `/sys/devices/...` even inside a fixture.
    pub fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let real = fs::canonicalize(path).ok()?;
        let base = fs::canonicalize(&self.0).ok()?;
        let rel = real.strip_prefix(&base).ok()?;
        Some(Path::new("/").join(rel))
    }
}

/// A file's contents with trailing whitespace and control characters
/// trimmed; `None` when it can't be read or is empty. Invalid UTF-8 is
/// replaced rather than failing: cheap USB devices put junk in their strings.
pub fn read(path: impl AsRef<Path>) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_end_matches(|c: char| {
        c == char::REPLACEMENT_CHARACTER || c.is_control() || c.is_whitespace()
    });
    (!text.is_empty()).then(|| text.to_owned())
}

/// A decimal attribute such as `busnum`.
pub fn read_dec<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.trim().parse().ok()
}

/// A hex attribute such as `idVendor` (`045e`).
pub fn read_hex(path: impl AsRef<Path>) -> Option<u16> {
    let text = read(path)?;
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// Entry names of a directory that satisfy `keep`, sorted; empty when the
/// directory is missing.
pub fn names(dir: impl AsRef<Path>, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| keep(n))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_path_ignores_leading_slash() {
        let root = Root::new("/tmp/fixture");
        assert_eq!(root.path("/sys/bus"), PathBuf::from("/tmp/fixture/sys/bus"));
        assert_eq!(root.path("sys/bus"), PathBuf::from("/tmp/fixture/sys/bus"));
    }

    #[test]
    fn readers() {
        let dir = tempfile::tempdir().unwrap();
        let f = |name: &str, bytes: &[u8]| {
            let p = dir.path().join(name);
            fs::write(&p, bytes).unwrap();
            p
        };
        assert_eq!(read(f("a", b"Controller\n")).as_deref(), Some("Controller"));
        assert_eq!(read(f("empty", b"\n")), None);
        assert_eq!(read(dir.path().join("missing")), None);
        assert_eq!(
            read(f("latin1", b"\xa9Microsoft\n")).as_deref(),
            Some("\u{fffd}Microsoft")
        );
        assert_eq!(read_hex(f("vid", b"045e\n")), Some(0x045e));
        assert_eq!(read_hex(f("bad", b"xyz\n")), None);
        assert_eq!(read_dec::<u16>(f("bus", b"3\n")), Some(3));
    }

    #[test]
    fn resolve_shows_live_paths() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("sys/devices/usb3/3-1");
        fs::create_dir_all(&real).unwrap();
        let links = dir.path().join("sys/bus/usb/devices");
        fs::create_dir_all(&links).unwrap();
        std::os::unix::fs::symlink("../../../devices/usb3/3-1", links.join("3-1")).unwrap();
        let root = Root::new(dir.path());
        assert_eq!(
            root.resolve(&links.join("3-1")),
            Some(PathBuf::from("/sys/devices/usb3/3-1"))
        );
    }
}
