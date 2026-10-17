//! How requests reach the pad's web-config server: [`http::Http`] on a live
//! machine, [`mock::Mock`] (a fixture directory) in tests.
//!
//! A transport only sends what it is given, but what it can be given is
//! narrow: an [`ApiPath`] can only be made inside this crate, by the `api`
//! module, so the endpoint allowlist there is the only way to reach the
//! device. Nothing outside the crate can ask for `/api/getConfig`:
//!
//! ```compile_fail
//! use gamepadii_core::transport::ApiPath;
//! let _ = ApiPath::new("/api/getConfig");
//! ```

pub mod http;
pub mod mock;

use std::fmt;

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Method::Get => "GET",
            Method::Post => "POST",
        })
    }
}

/// A path on the device's web server, such as `/api/getGamepadOptions`.
/// Only the `api` module makes these, from `'static` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApiPath(&'static str);

impl ApiPath {
    pub(crate) const fn new(path: &'static str) -> Self {
        Self(path)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ApiPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// What came back: the HTTP status and the raw body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One request at a time to the pad. Implementations time out rather than
/// block forever, and serialise their requests: the firmware's lwIP server
/// keeps one global POST buffer and does not cope with concurrency.
pub trait Transport {
    fn request(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> Result<Reply>;

    /// Where requests go, for messages and output: `http://192.168.7.1`.
    fn describe(&self) -> String;
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn request(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> Result<Reply> {
        (**self).request(method, path, body)
    }

    fn describe(&self) -> String {
        (**self).describe()
    }
}
