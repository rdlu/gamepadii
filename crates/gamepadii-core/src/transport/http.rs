//! The live transport: a minimal HTTP/1.0 client over a `TcpStream`, for
//! the pad's web configurator. Requests are serialised behind a mutex with
//! a short gap between them, each one has an overall deadline, and nothing
//! is retried (a hung device should fail fast, not be hammered).
//!
//! Hand-written because lwIP's httpd finds the request body by searching
//! for `Content-Length: ` case-sensitively: an HTTP library that lowercases
//! header names (ureq, via the `http` crate) gets every POST with a body
//! dropped with no reply. So the request is written byte for byte, headers
//! and body in one write, and the server closes the connection to end the
//! reply.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
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
const MAX_BODY: usize = 1 << 20;

/// Room for the status line and headers on top of [`MAX_BODY`].
const MAX_HEAD: usize = 16 << 10;

pub struct Http {
    base: String,
    timeout: Duration,
    gap: Duration,
    /// When the last request finished; held for a whole request, so two
    /// threads never talk to the pad at once.
    last: Mutex<Option<Instant>>,
}

impl Http {
    /// A transport for `host`: `192.168.7.1`, `host:port` or
    /// `http://host[:port]`. No TLS: the firmware only speaks HTTP.
    pub fn new(host: &str) -> Result<Self> {
        Ok(Self {
            base: base_url(host)?,
            timeout: DEFAULT_TIMEOUT,
            gap: DEFAULT_GAP,
            last: Mutex::new(None),
        })
    }

    /// Overrides [`DEFAULT_TIMEOUT`].
    pub fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// `host[:port]`, as it goes in the `Host` header.
    fn authority(&self) -> &str {
        self.base.strip_prefix("http://").unwrap_or(&self.base)
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
        let reply = self.exchange(method, path, body);
        *last = Some(Instant::now());
        reply.map_err(|e| Error::io(format!("{method} {}{path}", self.base), e))
    }

    fn describe(&self) -> String {
        self.base.clone()
    }
}

impl Http {
    /// One request on a fresh connection, all within `self.timeout`.
    fn exchange(&self, method: Method, path: ApiPath, body: Option<&[u8]>) -> io::Result<Reply> {
        let deadline = Instant::now() + self.timeout;
        let mut sock = connect(self.authority(), self.timeout.min(CONNECT_TIMEOUT))?;
        let request = encode(method, self.authority(), path.as_str(), body);
        sock.set_write_timeout(Some(remaining(deadline)?))?;
        sock.write_all(&request).map_err(timed_out)?;
        let raw = read_reply(&mut sock, deadline)?;
        parse(&raw)
    }
}

/// The request bytes, headers and body together so they go out in one
/// write. lwIP looks for the exact `Content-Length: ` casing in the first
/// segment; keep these strings as they are.
fn encode(method: Method, host: &str, path: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\n");
    let body = match method {
        Method::Get => None,
        Method::Post => Some(body.unwrap_or_default()),
    };
    if let Some(body) = body {
        out.push_str("Content-Type: application/json\r\n");
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("Connection: close\r\n\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body.unwrap_or_default());
    out
}

/// Resolves `authority` (port 80 if none is given) and connects. Nothing
/// has been sent if this fails, so its errors never read as "dropped after
/// sending" (see `api::reboot`): a timeout is `NotConnected`.
fn connect(authority: &str, timeout: Duration) -> io::Result<TcpStream> {
    let addrs = resolve(authority)?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(sock) => return Ok(sock),
            Err(e) => last = Some(e),
        }
    }
    let e = last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"));
    Err(match e.kind() {
        io::ErrorKind::ConnectionRefused => e,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => io::Error::new(
            io::ErrorKind::NotConnected,
            format!("connect timed out after {timeout:?}"),
        ),
        _ => io::Error::new(io::ErrorKind::NotConnected, format!("connect failed: {e}")),
    })
}

fn resolve(authority: &str) -> io::Result<Vec<SocketAddr>> {
    let has_port = match authority.strip_prefix('[') {
        Some(v6) => v6.contains("]:"),
        None => authority.contains(':'),
    };
    let found = if has_port {
        authority.to_socket_addrs()
    } else {
        (authority.trim_start_matches('[').trim_end_matches(']'), 80).to_socket_addrs()
    };
    let addrs: Vec<_> = found
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("{authority}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{authority}: no address"),
        ));
    }
    Ok(addrs)
}

/// What is left of the deadline, or a timeout if nothing is.
fn remaining(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"))
    } else {
        Ok(left)
    }
}

/// A socket timeout surfaces as `WouldBlock` on Unix: call it `TimedOut`.
fn timed_out(e: io::Error) -> io::Error {
    match e.kind() {
        io::ErrorKind::WouldBlock => io::Error::new(io::ErrorKind::TimedOut, "timed out"),
        _ => e,
    }
}

/// Reads until the server closes (HTTP/1.0), or until the body is in when
/// the headers give its length, within the deadline and the size cap.
fn read_reply(sock: &mut TcpStream, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(want) = expected_len(&raw)?
            && raw.len() >= want
        {
            return Ok(raw);
        }
        sock.set_read_timeout(Some(remaining(deadline)?))?;
        let n = match sock.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(timed_out(e)),
        };
        if n == 0 {
            return Ok(raw);
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.len() > MAX_HEAD + MAX_BODY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("reply over {} KiB", MAX_BODY >> 10),
            ));
        }
    }
}

/// The full reply length once the headers are in and carry a
/// `Content-Length`.
fn expected_len(raw: &[u8]) -> io::Result<Option<usize>> {
    let Some(end) = header_end(raw) else {
        return Ok(None);
    };
    let (_, headers) = head(&raw[..end])?;
    Ok(content_length(&headers)?.map(|n| end + n))
}

/// The index just past the blank line that ends the headers.
fn header_end(raw: &[u8]) -> Option<usize> {
    if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(i + 4);
    }
    raw.windows(2).position(|w| w == b"\n\n").map(|i| i + 2)
}

fn corrupted(why: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("corrupted reply: {}", why.into()),
    )
}

/// The status code and headers (names lowercased) from the head.
fn head(head: &[u8]) -> io::Result<(u16, Vec<(String, String)>)> {
    let text = std::str::from_utf8(head).map_err(|_| corrupted("headers are not UTF-8"))?;
    let mut lines = text.lines();
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || code.len() != 3
        || !code.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(corrupted(format!("bad status line {status_line:?}")));
    }
    let status = code.parse().map_err(|_| corrupted("bad status code"))?;
    let mut headers = Vec::new();
    for line in lines.take_while(|l| !l.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| corrupted(format!("bad header {line:?}")))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok((status, headers))
}

fn content_length(headers: &[(String, String)]) -> io::Result<Option<usize>> {
    headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| {
            value
                .parse::<usize>()
                .ok()
                .filter(|&n| n <= MAX_BODY)
                .ok_or_else(|| corrupted(format!("bad Content-Length {value:?}")))
        })
        .transpose()
}

/// The reply as read to EOF. An empty or cut-short reply is
/// `UnexpectedEof`: the connection dropped after the request went out,
/// which a reboot does.
fn parse(raw: &[u8]) -> io::Result<Reply> {
    if raw.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed with no reply",
        ));
    }
    let Some(end) = header_end(raw) else {
        if !b"HTTP/".starts_with(&raw[..raw.len().min(5)]) {
            return Err(corrupted("no HTTP status line"));
        }
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed mid-headers",
        ));
    };
    let (status, headers) = head(&raw[..end])?;
    let body = &raw[end..];
    let body = match content_length(&headers)? {
        Some(n) if body.len() < n => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("the connection closed {} of {n} body bytes in", body.len()),
            ));
        }
        Some(n) => &body[..n],
        None if body.len() > MAX_BODY => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("reply over {} KiB", MAX_BODY >> 10),
            ));
        }
        None => body,
    };
    Ok(Reply {
        status,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn default_port_is_80() {
        let addrs = resolve("127.0.0.1").unwrap();
        assert_eq!(addrs[0].port(), 80);
        assert_eq!(resolve("127.0.0.1:8080").unwrap()[0].port(), 8080);
        assert_eq!(resolve("[::1]").unwrap()[0].port(), 80);
        assert_eq!(resolve("[::1]:81").unwrap()[0].port(), 81);
    }

    /// Reads one request the way lwIP's httpd does: the body length comes
    /// only from a header spelled exactly `Content-Length: `.
    fn read_request(sock: &mut TcpStream) -> Vec<u8> {
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        loop {
            let n = sock.read(&mut buf).unwrap();
            seen.extend_from_slice(&buf[..n]);
            if let Some(end) = header_end(&seen) {
                let text = String::from_utf8_lossy(&seen[..end]).to_string();
                let len = text
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                if seen.len() >= end + len {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        seen
    }

    /// A one-shot server that answers like lwIP's httpd (HTTP/1.0, the
    /// firmware's own headers, then a close) and hands back the request.
    fn serve_once(reply: &'static str) -> (String, thread::JoinHandle<String>) {
        serve_with(move |sock| {
            let seen = read_request(sock);
            sock.write_all(reply.as_bytes()).unwrap();
            seen
        })
    }

    fn serve_with(
        f: impl FnOnce(&mut TcpStream) -> Vec<u8> + Send + 'static,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            String::from_utf8(f(&mut sock)).unwrap()
        });
        (addr, handle)
    }

    fn kind(err: Error) -> io::ErrorKind {
        match err {
            Error::Io { source, .. } => source.kind(),
            other => panic!("{other:?}"),
        }
    }

    fn get(addr: &str) -> Result<Reply> {
        Http::new(addr)
            .unwrap()
            .request(Method::Get, ApiPath::new("/api/getFirmwareVersion"), None)
    }

    const FIRMWARE_REPLY: &str = "HTTP/1.0 200 OK\r\nServer: GP2040-CE\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";

    #[test]
    fn get_against_a_local_server() {
        let (addr, server) = serve_once(FIRMWARE_REPLY);
        let reply = get(&addr).unwrap();
        assert_eq!(
            reply,
            Reply {
                status: 200,
                body: b"{}".to_vec()
            }
        );
        let seen = server.join().unwrap();
        assert_eq!(
            seen,
            format!(
                "GET /api/getFirmwareVersion HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
            )
        );
    }

    #[test]
    fn post_sends_exact_bytes_in_one_write() {
        let (addr, server) = serve_with(|sock| {
            // Let the whole request land, then take it in a single read.
            thread::sleep(Duration::from_millis(100));
            let mut buf = vec![0u8; 64 << 10];
            let n = sock.read(&mut buf).unwrap();
            sock.write_all(FIRMWARE_REPLY.as_bytes()).unwrap();
            buf.truncate(n);
            buf
        });
        let body = br#"{"bootMode":1}"#;
        Http::new(&addr)
            .unwrap()
            .request(Method::Post, ApiPath::new("/api/reboot"), Some(body))
            .unwrap();
        let seen = server.join().unwrap();
        assert_eq!(
            seen,
            format!(
                "POST /api/reboot HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{{\"bootMode\":1}}"
            )
        );
        assert!(seen.contains("\r\nContent-Length: 14\r\n"), "{seen}");
    }

    /// The bug this client exists for: lwIP finds the body only through a
    /// case-sensitive `Content-Length: `, and closes without a reply when
    /// it can't (ureq's `content-length` got "Peer disconnected").
    #[test]
    fn a_case_sensitive_server_gets_the_whole_body() {
        let (addr, server) = serve_with(|sock| {
            let seen = read_request(sock);
            let text = String::from_utf8_lossy(&seen).to_string();
            let end = header_end(&seen).unwrap();
            let Some(len) = text
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .map(|v| v.trim().parse::<usize>().unwrap())
            else {
                return seen; // closes with no reply, like the firmware
            };
            let body = seen[end..end + len].to_vec();
            let reply = format!("HTTP/1.0 200 OK\r\nContent-Length: {len}\r\n\r\n");
            sock.write_all(reply.as_bytes()).unwrap();
            sock.write_all(&body).unwrap();
            seen
        });
        // About the size of a real setGamepadOptions body.
        let body = format!("{{\"pad\":\"{}\"}}", "x".repeat(1460));
        let reply = Http::new(&addr)
            .unwrap()
            .request(
                Method::Post,
                ApiPath::new("/api/getFirmwareVersion"),
                Some(body.as_bytes()),
            )
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, body.as_bytes());
        server.join().unwrap();
    }

    #[test]
    fn non_200_is_a_reply_not_an_error() {
        let (addr, server) = serve_once("HTTP/1.0 404 File not found\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(get(&addr).unwrap().status, 404);
        server.join().unwrap();
    }

    #[test]
    fn http_1_1_replies_and_header_case_are_accepted() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK\r\ncontent-LENGTH:  3 \r\n\r\nabcEXTRA");
        assert_eq!(get(&addr).unwrap().body, b"abc");
        server.join().unwrap();
    }

    #[test]
    fn no_content_length_reads_to_eof() {
        let (addr, server) = serve_once("HTTP/1.0 200 OK\r\nServer: lwIP\r\n\r\n{\"a\":1}");
        assert_eq!(get(&addr).unwrap().body, br#"{"a":1}"#);
        server.join().unwrap();
    }

    #[test]
    fn a_truncated_body_is_unexpected_eof() {
        let (addr, server) = serve_once("HTTP/1.0 200 OK\r\nContent-Length: 10\r\n\r\n{}");
        assert_eq!(kind(get(&addr).unwrap_err()), io::ErrorKind::UnexpectedEof);
        server.join().unwrap();
    }

    #[test]
    fn truncated_headers_are_unexpected_eof() {
        let (addr, server) = serve_once("HTTP/1.0 200 OK\r\nContent-Len");
        assert_eq!(kind(get(&addr).unwrap_err()), io::ErrorKind::UnexpectedEof);
        server.join().unwrap();
    }

    /// A reboot drops the connection once the request is out: that must
    /// stay an `UnexpectedEof`, which `api::reboot` counts as probable.
    #[test]
    fn a_close_with_no_reply_is_a_probable_reboot() {
        let (addr, server) = serve_with(read_request);
        let err = get(&addr).unwrap_err();
        assert!(err.to_string().contains("no reply"), "{err}");
        assert_eq!(kind(err), io::ErrorKind::UnexpectedEof);
        server.join().unwrap();

        let (addr, server) = serve_with(read_request);
        let ack =
            crate::api::reboot(&Http::new(&addr).unwrap(), crate::mode::BootMode::Gamepad).unwrap();
        assert_eq!(ack, crate::api::RebootAck::Probable);
        server.join().unwrap();
    }

    #[test]
    fn a_garbage_status_line_is_corrupted() {
        for reply in [
            "SSH-2.0-OpenSSH\r\n\r\n",
            "HTTP/1.0 2x0 OK\r\n\r\n",
            "HTTP/2 200\r\n\r\n",
            "nonsense with no blank line",
        ] {
            let (addr, server) = serve_once(reply);
            let err = get(&addr).unwrap_err();
            assert!(
                err.to_string().contains("corrupted reply"),
                "{reply:?}: {err}"
            );
            assert_eq!(kind(err), io::ErrorKind::InvalidData, "{reply:?}");
            server.join().unwrap();
        }
    }

    #[test]
    fn a_refused_connection_sent_nothing() {
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().to_string()
        };
        let err = get(&addr).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::TRANSPORT);
        assert_eq!(kind(err), io::ErrorKind::ConnectionRefused);
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
        assert_eq!(kind(err), io::ErrorKind::TimedOut);
    }

    /// The timeout covers the whole request: a server that trickles a byte
    /// at a time doesn't reset it.
    #[test]
    fn a_trickling_server_hits_the_overall_deadline() {
        let (addr, _server) = serve_with(|sock| {
            let seen = read_request(sock);
            for b in FIRMWARE_REPLY.bytes().cycle().take(40) {
                if sock.write_all(&[b]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            seen
        });
        let http = Http::new(&addr)
            .unwrap()
            .timeout(Duration::from_millis(300));
        let started = Instant::now();
        let err = http
            .request(Method::Get, ApiPath::new("/api/getFirmwareVersion"), None)
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(kind(err), io::ErrorKind::TimedOut);
    }

    #[test]
    fn requests_are_spaced_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                read_request(&mut sock);
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
