//! Shared HTTP/1.1 wire layer for the interpreter and the VM.
//!
//! Phase 5/M4 production shape (`docs/PHASE5_PRODUCTION.md` §2): both
//! runtimes serve from this module so limits, status codes, and byte
//! counts can never diverge — divergence here would be a silent
//! behavioral fork between `run` and `run-vm`. Handler *invocation*
//! stays per-runtime (interpreted vs. VM frames); everything on the
//! wire is shared.
//!
//! Contract, measured on wire bytes:
//! - header block (through the blank line) ≤ 8 KiB, else 413;
//! - request body (Content-Length) ≤ 1 MiB, else 413 with no handler
//!   invocation and `Connection: close`;
//! - malformed request line / headers / lengths → 400, server keeps
//!   serving;
//! - `Transfer-Encoding: chunked` is NOT decoded in M4 (the M4 wire
//!   is close-delimited): such bodies read as empty. Documented,
//!   not silent — full HTTP/1.1 completeness is Phase 6 work.

use std::io::{Read, Write};
use std::net::TcpStream;

/// Header block cap: 8 KiB of wire bytes through the terminator.
pub const MAX_HEADER_BLOCK: usize = 8 * 1024;
/// Request body cap: 1 MiB.
pub const MAX_BODY: usize = 1024 * 1024;
/// Single socket read size for the incremental reader.
const READ_CHUNK: usize = 4096;

/// A fully read request: method, path, version, and body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub version: String,
    pub body: String,
}

/// Why a request was refused without invoking a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Header block exceeded [`MAX_HEADER_BLOCK`].
    OverHeaderCap,
    /// Declared body exceeded [`MAX_BODY`].
    OverBodyCap,
    /// Unparseable request line, headers, or lengths; short body.
    Malformed,
    /// Socket failure mid-read.
    Io(String),
}

/// Read one close-delimited HTTP/1.1 request from `conn`.
/// Returns after the exact Content-Length body (or the bytes already
/// buffered when no length is present); never reads past the request.
/// A 10 s read timeout bounds stalled peers: timeouts surface as
/// [`WireError::Io`] and the caller closes the connection — a peer
/// that never finishes its request is not owed a response.
pub fn read_request(conn: &mut TcpStream) -> Result<ParsedRequest, WireError> {
    conn.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .map_err(|e| WireError::Io(e.to_string()))?;
    let mut buf: Vec<u8> = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; READ_CHUNK];
        match conn.read(&mut chunk) {
            Ok(0) => return Err(WireError::Malformed),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEADER_BLOCK + 4 {
                    return Err(WireError::OverHeaderCap);
                }
                if let Some(pos) = find_header_end(&buf) {
                    if pos > MAX_HEADER_BLOCK {
                        return Err(WireError::OverHeaderCap);
                    }
                    break pos;
                }
                if buf.len() > MAX_HEADER_BLOCK {
                    return Err(WireError::OverHeaderCap);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WireError::Io(e.to_string())),
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(WireError::Malformed)?;
    let (method, path, version) = parse_request_line(&request_line).ok_or(WireError::Malformed)?;
    let mut content_length: Option<u64> = None;
    for header in lines {
        if header.is_empty() {
            continue;
        }
        let colon = header.find(':').ok_or(WireError::Malformed)?;
        let (name, value) = header.split_at(colon);
        if name.trim().eq_ignore_ascii_case("content-length") {
            let digits = value[1..].trim();
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(WireError::Malformed);
            }
            if content_length.is_some() {
                return Err(WireError::Malformed);
            }
            content_length = Some(digits.parse::<u64>().map_err(|_| WireError::Malformed)?);
        } else if header.bytes().any(|b| b == 0) {
            return Err(WireError::Malformed);
        }
    }
    let declared = content_length.unwrap_or(0);
    if declared > MAX_BODY as u64 {
        return Err(WireError::OverBodyCap);
    }
    let mut body = buf.split_off(header_end);
    while (body.len() as u64) < declared {
        let mut chunk = [0u8; READ_CHUNK];
        match conn.read(&mut chunk) {
            Ok(0) => return Err(WireError::Malformed),
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if (body.len() as u64) > MAX_BODY as u64 {
                    return Err(WireError::OverBodyCap);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WireError::Io(e.to_string())),
        }
    }
    body.truncate(declared as usize);
    Ok(ParsedRequest {
        method,
        path,
        version,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// Byte offset just past the `\r\n\r\n` terminator, if present.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

/// `METHOD SP PATH SP VERSION`, all non-empty, no NUL bytes, no
/// trailing tokens.
fn parse_request_line(line: &str) -> Option<(String, String, String)> {
    let line = line.lines().next().unwrap_or("");
    if line.bytes().any(|b| b == 0) {
        return None;
    }
    let mut parts = line.split_whitespace();
    let (method, path, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || method.is_empty()
        || path.is_empty()
        || version.is_empty()
    {
        return None;
    }
    Some((method.to_string(), path.to_string(), version.to_string()))
}

/// Reason phrases served on the wire (mirrors
/// `stdlib/net/http/response.nv::http_reason_phrase` for the codes
/// the server can emit; that file stays the user-visible mapping).
/// Status-aware handlers (Phase 6) may return any code, so every
/// code in the `.nv` table has a wire mapping here — an unlisted
/// code still renders (falling through to `"Unknown"`), never
/// silently remapped.
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// One close-delimited response. The 200 shape keeps
/// `Content-Type: text/plain` (existing clients match on it).
pub fn response(status: u16, body: &str) -> Vec<u8> {
    let head = if status == 200 {
        format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reason_phrase(status),
            body.len()
        )
    } else {
        format!(
            "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reason_phrase(status),
            body.len()
        )
    };
    let mut out = head.into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Write one response, ignoring late write failures (the request
/// was already fully handled — a dead peer is not a server error).
pub fn write_response(conn: &mut TcpStream, status: u16, body: &str) {
    conn.write_all(&response(status, body)).ok();
}

/// Write pre-rendered response bytes, ignoring late write failures
/// (same discipline as [`write_response`]: the request was already
/// fully handled — a dead peer is not a server error).
pub fn write_bytes(conn: &mut TcpStream, bytes: &[u8]) {
    conn.write_all(bytes).ok();
}

/// A successfully rendered status-aware response: the wire bytes
/// plus the names of wire-owned headers the handler supplied (they
/// are skipped, never echoed — the wire layer owns framing; callers
/// log them loudly so the skip is never silent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedStatus {
    pub bytes: Vec<u8>,
    pub skipped_wire_owned: Vec<String>,
}

/// Why a status-aware handler value could not become a response.
/// Every variant answers 500 with a loud server-side log — never a
/// remapped status, never a half-framed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    /// `status` was not in `100..=599`.
    BadStatus(i128),
    /// `reason` contained a framing-breaking byte (`\r`, `\n`, NUL).
    BadReason,
    /// A header name was empty or a name/value contained `\r`, `\n`,
    /// or NUL (a smuggled framing break — never emitted).
    BadHeader,
}

/// Render one status-aware response from handler-supplied parts.
///
/// This is the single source of truth for status validation and
/// response framing: both the interpreter and the VM serve loops
/// extract the plain parts from their own value types (trivial field
/// plumbing, no decisions) and render through here, so the runtimes
/// cannot diverge on what a status means or what bytes it becomes.
///
/// Contract:
/// - `status` must be in `100..=599`, else [`StatusError::BadStatus`]
///   (no silent remap to 200/500 at this layer — the caller answers
///   500 loudly);
/// - an empty `reason` falls back to [`reason_phrase`] (canonical);
///   a reason containing `\r`, `\n`, or NUL is [`StatusError::BadReason`];
/// - headers render verbatim in order, except the wire-owned
///   `Content-Length` and `Connection` (case-insensitive), which are
///   skipped and reported in `skipped_wire_owned` — the wire layer
///   always appends the true length and `close` itself, so echoing a
///   handler-supplied copy would corrupt framing;
/// - `Content-Length` is always the exact body byte length and the
///   response is always `Connection: close` (the M4 wire is
///   close-delimited; no keep-alive, no chunked encoding).
///
/// No `unwrap` on these paths: every input here ultimately comes
/// from handler code or the network, so every failure is a value.
pub fn encode_status_response(
    status: i128,
    reason: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<EncodedStatus, StatusError> {
    if !(100..=599).contains(&status) {
        return Err(StatusError::BadStatus(status));
    }
    let status = status as u16;
    if reason.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(StatusError::BadReason);
    }
    let phrase = if reason.is_empty() {
        reason_phrase(status)
    } else {
        reason
    };
    let mut kept: Vec<(&str, &str)> = Vec::with_capacity(headers.len());
    let mut skipped: Vec<String> = Vec::new();
    for (name, value) in headers {
        if name.is_empty()
            || name.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
            || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        {
            return Err(StatusError::BadHeader);
        }
        if name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("connection") {
            skipped.push(name.clone());
            continue;
        }
        kept.push((name.as_str(), value.as_str()));
    }
    let mut head = format!("HTTP/1.1 {status} {phrase}\r\n");
    for (name, value) in &kept {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut out = head.into_bytes();
    out.extend_from_slice(body.as_bytes());
    Ok(EncodedStatus {
        bytes: out,
        skipped_wire_owned: skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Duration;

    fn roundtrip(raw: &[u8]) -> (Result<ParsedRequest, WireError>, Vec<u8>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let raw = raw.to_vec();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let parsed = read_request(&mut conn);
            let (status, body) = match &parsed {
                Ok(_) => (200u16, "ok"),
                Err(e)
                    if *e == WireError::OverHeaderCap || *e == WireError::OverBodyCap =>
                {
                    (413, "Payload Too Large")
                }
                Err(_) => (400, "Bad Request"),
            };
            write_response(&mut conn, status, body);
            parsed
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(&raw).unwrap();
        let mut echoed = Vec::new();
        client.read_to_end(&mut echoed).ok();
        (server.join().unwrap(), echoed)
    }

    #[test]
    fn reads_get_with_buffered_body() {
        let (parsed, echoed) = roundtrip(b"GET /hi HTTP/1.1\r\nHost: x\r\n\r\n");
        let req = parsed.expect("must parse");
        assert_eq!((req.method, req.path, req.body), ("GET".into(), "/hi".into(), String::new()));
        assert!(echoed.starts_with(b"HTTP/1.1 200 OK"));
    }

    #[test]
    fn reads_exact_content_length_body() {
        let (parsed, _) =
            roundtrip(b"POST /echo HTTP/1.1\r\nContent-Length: 4\r\n\r\npingEXTRA");
        let req = parsed.expect("must parse");
        assert_eq!(req.body, "ping");
    }

    #[test]
    fn refuses_over_header_cap() {
        let mut raw = b"GET /x HTTP/1.1\r\n".to_vec();
        while raw.len() < MAX_HEADER_BLOCK + 100 {
            raw.extend_from_slice(b"X-Pad: abcdefgh\r\n");
        }
        raw.extend_from_slice(b"\r\n");
        let (parsed, echoed) = roundtrip(&raw);
        assert_eq!(parsed, Err(WireError::OverHeaderCap));
        assert!(echoed.starts_with(b"HTTP/1.1 413"));
    }

    #[test]
    fn refuses_over_body_cap() {
        let raw = format!("POST /big HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        let (parsed, echoed) = roundtrip(raw.as_bytes());
        assert_eq!(parsed, Err(WireError::OverBodyCap));
        assert!(echoed.starts_with(b"HTTP/1.1 413"));
    }

    #[test]
    fn refuses_malformed_request_line() {
        let (parsed, echoed) = roundtrip(b"GARBAGE\r\n\r\n");
        assert_eq!(parsed, Err(WireError::Malformed));
        assert!(echoed.starts_with(b"HTTP/1.1 400"));
    }

    #[test]
    fn refuses_bad_content_length() {
        let (parsed, _) = roundtrip(b"POST /x HTTP/1.1\r\nContent-Length: many\r\n\r\n");
        assert_eq!(parsed, Err(WireError::Malformed));
    }

    #[test]
    fn reason_phrase_covers_status_aware_codes() {
        assert_eq!(reason_phrase(201), "Created");
        assert_eq!(reason_phrase(204), "No Content");
        assert_eq!(reason_phrase(503), "Service Unavailable");
        assert_eq!(reason_phrase(299), "Unknown");
    }

    #[test]
    fn encodes_201_with_headers_verbatim() {
        let out = encode_status_response(
            201,
            "Created",
            &[("X-Test".to_string(), "yes".to_string())],
            "made",
        )
        .expect("valid status response must encode");
        assert!(out.skipped_wire_owned.is_empty());
        let text = String::from_utf8(out.bytes).expect("ASCII framing");
        assert_eq!(
            text,
            "HTTP/1.1 201 Created\r\nX-Test: yes\r\nContent-Length: 4\r\nConnection: close\r\n\r\nmade"
        );
    }

    #[test]
    fn empty_reason_falls_back_to_canonical_phrase() {
        let out = encode_status_response(503, "", &[], "busy").expect("must encode");
        let text = String::from_utf8(out.bytes).expect("ASCII framing");
        assert!(text.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    }

    #[test]
    fn rejects_out_of_range_status() {
        for bad in [99, 600, 0, -1, 9999] {
            assert_eq!(
                encode_status_response(bad, "x", &[], "b"),
                Err(StatusError::BadStatus(bad)),
                "status {bad} must not render"
            );
        }
    }

    #[test]
    fn rejects_crlf_in_reason_and_headers() {
        assert_eq!(
            encode_status_response(200, "O\r\nK", &[], "b"),
            Err(StatusError::BadReason)
        );
        assert_eq!(
            encode_status_response(200, "bad\0reason", &[], "b"),
            Err(StatusError::BadReason)
        );
        assert_eq!(
            encode_status_response(
                200,
                "OK",
                &[("X-Evil\r\nInjected: 1".to_string(), "v".to_string())],
                "b"
            ),
            Err(StatusError::BadHeader)
        );
        assert_eq!(
            encode_status_response(
                200,
                "OK",
                &[("X-Ok".to_string(), "a\r\nb".to_string())],
                "b"
            ),
            Err(StatusError::BadHeader)
        );
        assert_eq!(
            encode_status_response(200, "OK", &[("".to_string(), "v".to_string())], "b"),
            Err(StatusError::BadHeader)
        );
    }

    #[test]
    fn skips_wire_owned_headers_and_reports_them() {
        let out = encode_status_response(
            200,
            "OK",
            &[
                ("Content-Length".to_string(), "999".to_string()),
                ("connection".to_string(), "keep-alive".to_string()),
                ("X-Keep".to_string(), "me".to_string()),
            ],
            "hi",
        )
        .expect("must encode");
        assert_eq!(out.skipped_wire_owned, vec!["Content-Length", "connection"]);
        let text = String::from_utf8(out.bytes).expect("ASCII framing");
        assert_eq!(
            text,
            "HTTP/1.1 200 OK\r\nX-Keep: me\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi"
        );
    }
}
