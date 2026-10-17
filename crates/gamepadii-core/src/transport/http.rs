//! The live transport: plain HTTP to the pad's web configurator with ureq.
//! Requests are serialised behind a mutex with a short gap between them,
//! each one has a timeout, and nothing is retried (a hung device should
//! fail fast, not be hammered).

use std::io;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use super::{ApiPath, Method, Reply, Transport};
use crate::error::{Error, Result};

/// Where GP2040-CE serves web-config (its DHCP server gives the host .2).
pub const DEFAULT_HOST: &str = "192.168.7.1";

/// The whole request, connect to last byte.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The pause between the end of one request and the start of the next.
pub const DEFAULT_GAP: Duration = Duration::from_millis(100);

/// No reply from the firmware comes near this (the largest, the splash
/// image, is a few KB).
const MAX_BODY: u64 = 1 << 20;

pub struct Http {
    agent: ureq::Agent,
    base: String,
    gap: Duration,
    /// When the last request finished; held for a whole request, so two
    /// threads never talk to the pad at once.
    last: Mutex<Option<Instant>>,
}

impl Http {
    /// A transport for `host`: `192.168.7.1`, `host:port` or
    /// `http://host[:port]`. No TLS: the firmware only speaks HTTP.
    pub fn new(host: &str) -> Result<Self> {
        Ok(Self::with_timeout(base_url(host)?, DEFAULT_TIMEOUT))
    }

    fn with_timeout(base: String, timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .timeout_connect(Some(CONNECT_TIMEOUT.min(timeout)))
            .http_status_as_error(false)
            .max_redirects(0)
            // The pad is on a point-to-point USB link: never via a proxy.
            .proxy(None)
            .build();
        Self {
            agent: config.new_agent(),
            base,
            gap: DEFAULT_GAP,
            last: Mutex::new(None),
        }
    }

    /// Overrides [`DEFAULT_TIMEOUT`].
    pub fn timeout(self, timeout: Duration) -> Self {
        Self {
            gap: self.gap,
            ..Self::with_timeout(self.base, timeout)
        }
    }
}

/// `http://host[:port]` with no trailing slash, or a usage error.
pub fn base_url(host: &str) -> Result<String> {
    let bad = |why: &str| Error::Usage(format!("--host {host:?}: {why}"));
    let rest = host.trim().trim_end_matches('/');
    let rest = match rest.split_once("://") {
        Some(("http", rest)) => rest,
        Some((scheme, _)) => return Err(bad(&format!("{scheme}:// is not supported, only http"))),
        None => rest,
    };
    if rest.is_empty() {
        return Err(bad("empty host"));
    }
    if rest.contains(['/', '?', '#', ' ', '@']) {
        return Err(bad("expected a host or host:port, with no path"));
    }
    Ok(format!("http://{rest}"))
}

impl Transport for Http {
    fn request(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> Result<Reply> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let since = t.elapsed();
            if since < self.gap {
                thread::sleep(self.gap - since);
            }
        }
        let url = format!("{}{}", self.base, path);
        let result = match method {
            Method::Get => self.agent.get(&url).call(),
            Method::Post => self
                .agent
                .post(&url)
                .content_type("application/json")
                .send(body.unwrap_or_default()),
        };
        let reply = result.and_then(|mut resp| {
            let status = resp.status().as_u16();
            let body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()?;
            Ok(Reply { status, body })
        });
        *last = Some(Instant::now());
        reply.map_err(|e| to_error(format!("{method} {url}"), e))
    }

    fn describe(&self) -> String {
        self.base.clone()
    }
}

fn to_error(context: String, e: ureq::Error) -> Error {
    let kind = match &e {
        // Nothing was sent: not the "dropped mid-request" a reboot causes.
        ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => {
            io::ErrorKind::NotConnected
        }
        ureq::Error::Timeout(_) => io::ErrorKind::TimedOut,
        ureq::Error::ConnectionFailed => io::ErrorKind::ConnectionRefused,
        ureq::Error::HostNotFound => io::ErrorKind::NotFound,
        ureq::Error::Io(err) => err.kind(),
        _ => io::ErrorKind::Other,
    };
    Error::io(context, io::Error::new(kind, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn host_forms() {
        assert_eq!(base_url("192.168.7.1").unwrap(), "http://192.168.7.1");
        assert_eq!(
            base_url("http://192.168.7.1/").unwrap(),
            "http://192.168.7.1"
        );
        assert_eq!(base_url("pad.lan:8080").unwrap(), "http://pad.lan:8080");
        for bad in ["", "https://192.168.7.1", "192.168.7.1/api", "a b"] {
            let err = base_url(bad).unwrap_err();
            assert_eq!(err.exit_code(), crate::error::exit::USAGE, "{bad:?}");
        }
    }

    /// A one-shot server that answers like lwIP's httpd (HTTP/1.0, the
    /// firmware's own headers) and hands back the request it saw.
    fn serve_once(reply: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let mut seen = Vec::new();
            // Headers, then the body if there is a Content-Length.
            loop {
                let n = sock.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if seen.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            sock.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(seen).unwrap()
        });
        (addr, handle)
    }

    const FIRMWARE_REPLY: &str = "HTTP/1.0 200 OK\r\nServer: GP2040-CE\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";

    #[test]
    fn get_against_a_local_server() {
        let (addr, server) = serve_once(FIRMWARE_REPLY);
        let http = Http::new(&addr).unwrap();
        let reply = http
            .request(Method::Get, ApiPath::new("/api/getFirmwareVersion"), None)
            .unwrap();
        assert_eq!(
            reply,
            Reply {
                status: 200,
                body: b"{}".to_vec()
            }
        );
        let seen = server.join().unwrap();
        assert!(
            seen.starts_with("GET /api/getFirmwareVersion HTTP/1.1\r\n"),
            "{seen}"
        );
    }

    #[test]
    fn post_sends_json() {
        let (addr, server) = serve_once(FIRMWARE_REPLY);
        let http = Http::new(&addr).unwrap();
        http.request(
            Method::Post,
            ApiPath::new("/api/reboot"),
            Some(br#"{"bootMode":1}"#),
        )
        .unwrap();
        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /api/reboot HTTP/1.1\r\n"), "{seen}");
        assert!(
            seen.to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(seen.ends_with(r#"{"bootMode":1}"#), "{seen}");
    }

    #[test]
    fn non_200_is_a_reply_not_an_error() {
        let (addr, server) = serve_once("HTTP/1.0 404 File not found\r\nContent-Length: 0\r\n\r\n");
        let reply = Http::new(&addr)
            .unwrap()
            .request(Method::Get, ApiPath::new("/api/nope"), None)
            .unwrap();
        assert_eq!(reply.status, 404);
        server.join().unwrap();
    }

    #[test]
    fn a_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        // Accept and never answer, like the S16 after getConfig.
        let _hold = thread::spawn(move || {
            let conn = listener.accept();
            thread::sleep(Duration::from_secs(2));
            drop(conn);
        });
        let http = Http::new(&addr)
            .unwrap()
            .timeout(Duration::from_millis(300));
        let started = Instant::now();
        let err = http
            .request(Method::Get, ApiPath::new("/api/getFirmwareVersion"), None)
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(err.exit_code(), crate::error::exit::TRANSPORT);
        match err {
            Error::Io { source, .. } => assert_eq!(source.kind(), io::ErrorKind::TimedOut),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn requests_are_spaced_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).unwrap();
                sock.write_all(FIRMWARE_REPLY.as_bytes()).unwrap();
            }
        });
        let http = Http::new(&addr).unwrap();
        let path = ApiPath::new("/api/getFirmwareVersion");
        http.request(Method::Get, path, None).unwrap();
        let between = Instant::now();
        http.request(Method::Get, path, None).unwrap();
        assert!(between.elapsed() >= DEFAULT_GAP);
        server.join().unwrap();
    }
}
