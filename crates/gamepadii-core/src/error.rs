//! The error type shared by the core, and the stable exit codes.

use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Process exit codes: a stable contract for scripts and status bars.
pub mod exit {
    pub const OK: u8 = 0;
    /// A check ran and failed (no command uses it yet).
    pub const CHECK_FAILED: u8 = 1;
    /// No GP2040-CE device found.
    pub const NOT_FOUND: u8 = 2;
    /// Bad arguments.
    pub const USAGE: u8 = 3;
    /// Talking to the device failed (I/O, HTTP, a reply that makes no sense).
    pub const TRANSPORT: u8 = 4;
    /// The user has to do something on the pad first (a button combo, such
    /// as switching it into web-config).
    pub const NEEDS_USER: u8 = 5;
}

#[derive(Debug)]
pub enum Error {
    /// No device found.
    NotFound(String),
    /// Bad arguments that clap could not catch.
    Usage(String),
    /// I/O failed (sysfs, a socket, a file).
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The device answered, but not in a way we understand.
    Protocol(String),
    /// The user has to act on the pad first: the message says what to do.
    NeedsUser(String),
}

impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::NotFound(_) => exit::NOT_FOUND,
            Error::Usage(_) => exit::USAGE,
            Error::Io { .. } | Error::Protocol(_) => exit::TRANSPORT,
            Error::NeedsUser(_) => exit::NEEDS_USER,
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }

    /// A stable, machine-readable kind for the JSON error object.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Usage(_) => "usage",
            Error::Io { .. } => "io",
            Error::Protocol(_) => "protocol",
            Error::NeedsUser(_) => "needs_user",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(m) => write!(f, "not found: {m}"),
            Error::Usage(m) => write!(f, "usage: {m}"),
            Error::Io { context, source } => write!(f, "{context}: {source}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
            Error::NeedsUser(m) => write!(f, "action needed: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        let io = Error::io("reading x", std::io::Error::other("boom"));
        assert_eq!(Error::NotFound(String::new()).exit_code(), 2);
        assert_eq!(Error::Usage(String::new()).exit_code(), 3);
        assert_eq!(io.exit_code(), 4);
        assert_eq!(Error::Protocol(String::new()).exit_code(), 4);
        assert_eq!(Error::NeedsUser(String::new()).exit_code(), 5);
        assert_eq!(io.to_string(), "reading x: boom");
        assert_eq!(io.kind(), "io");
    }
}
