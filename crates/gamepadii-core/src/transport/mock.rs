//! A transport that serves a fixture directory: `GET /api/<name>` returns
//! `<dir>/<name>.json`, a missing file is a 404, and `POST /api/reboot`
//! answers as the firmware does (the request echoed with `"success": true`).
//! Every request is logged so tests can check what was asked for.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use super::{ApiPath, Method, Reply, Transport};
use crate::error::{Error, Result};

/// One logged request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub path: &'static str,
    pub body: Option<Vec<u8>>,
}

pub struct Mock {
    dir: PathBuf,
    log: Mutex<Vec<Request>>,
    /// Paths that fail like a hung device (a timeout).
    hang: Vec<&'static str>,
}

impl Mock {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            log: Mutex::new(Vec::new()),
            hang: Vec::new(),
        }
    }

    /// Makes requests to `path` (e.g. `/api/getSplashImage`) time out.
    pub fn hang_on(mut self, path: &'static str) -> Self {
        self.hang.push(path);
        self
    }

    /// Every request so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn json_reply(body: Vec<u8>) -> Reply {
    Reply { status: 200, body }
}

impl Transport for Mock {
    fn request(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> Result<Reply> {
        let path = path.as_str();
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Request {
                method,
                path,
                body: body.map(<[u8]>::to_vec),
            });
        if self.hang.contains(&path) {
            return Err(Error::io(
                format!("{method} mock{path}"),
                std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out (mock)"),
            ));
        }
        let name = path.strip_prefix("/api/").unwrap_or(path);
        if method == Method::Post && name == "reboot" {
            let mut doc: serde_json::Value = body
                .and_then(|b| serde_json::from_slice(b).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("success".into(), true.into());
            }
            return Ok(json_reply(doc.to_string().into_bytes()));
        }
        match fs::read(self.dir.join(format!("{name}.json"))) {
            Ok(body) => Ok(json_reply(body)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Reply {
                status: 404,
                body: Vec::new(),
            }),
            Err(e) => Err(Error::io(format!("reading {}", self.dir.display()), e)),
        }
    }

    /// `mock:<fixture dir name>`: no absolute path, so it can be snapshotted.
    fn describe(&self) -> String {
        let name = self.dir.file_name().map_or_else(
            || self.dir.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        format!("mock:{name}")
    }
}
