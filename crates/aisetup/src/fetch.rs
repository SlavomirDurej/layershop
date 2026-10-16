//! The streaming network seam.
//!
//! `photocraft_aigen::Transport` hands back a whole body in memory, which is right for an image
//! and impossible for a 20 GB model. [`Fetch`] streams a body into a [`Sink`] from a byte offset,
//! so a download can be resumed; [`UreqFetch`] is the desktop implementation, and the tests use
//! a fake that serves ranges from memory, drops connections on cue and lies about lengths.
//!
//! Redirects are followed here, not by the HTTP client, because where a download may go is a
//! rule of this app ([`Policy`]): https only, to the hosts Hugging Face and GitHub serve files
//! from, and the user's Hugging Face token is never sent to any host but `huggingface.co`.

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use crate::{Error, Result};

/// What to download, and from where in the file.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub url: &'a str,
    /// A Hugging Face token, sent only to `huggingface.co`.
    pub token: Option<&'a str>,
    /// The first byte wanted.
    pub offset: u64,
    /// The validator (ETag) of the file the bytes before `offset` came from: the server sends the
    /// rest only if the file is still that one, and otherwise the whole file again.
    pub validator: Option<&'a str>,
}

/// What a server says about a file without sending it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub size: Option<u64>,
    /// The sha256 Hugging Face reports for the file (`X-Linked-ETag`), when it does.
    pub sha256: Option<String>,
    /// Why Hugging Face refused it (`X-Error-Code`, e.g. `GatedRepo`), when it did.
    pub error_code: Option<String>,
    /// Its words for it (`X-Error-Message`).
    pub error_message: Option<String>,
}

/// How a download began: told to the [`Sink`] before the first byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Begin {
    /// The offset the bytes that follow start at: the one asked for, or 0 when the server sent
    /// the whole file again (it ignores ranges, or the file changed).
    pub offset: u64,
    /// The length of the whole file, when the server says.
    pub total: Option<u64>,
    /// The file's validator, to resume with.
    pub validator: String,
}

/// Where a download goes.
pub trait Sink {
    /// The response's headers are in. Everything already written from `begin.offset` on is to be
    /// dropped: the bytes that follow replace it.
    fn begin(&mut self, begin: &Begin) -> Result<()>;
    fn write(&mut self, bytes: &[u8]) -> Result<()>;
}

/// Streams files. `cancelled` is asked between chunks, and often enough while nothing arrives.
pub trait Fetch: Send + Sync {
    fn head(&self, url: &str, token: Option<&str>) -> Result<Head>;
    /// Send the body from `request.offset` into `sink`. `Ok` means the body ended as the server
    /// said it would; whether that is the whole file is the caller's to check.
    fn fetch(&self, request: &Request<'_>, sink: &mut dyn Sink, cancelled: &dyn Fn() -> bool) -> Result<()>;
}

/// Where a download may go.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Hosts allowed besides the built-in ones, and plain http to them (the tests' local server).
    pub also: Vec<String>,
}

/// The host a Hugging Face token is for.
const TOKEN_HOST: &str = "huggingface.co";
/// At most this many redirects are followed (Hugging Face needs two: a renamed repository, then
/// its CDN).
const MAX_HOPS: usize = 6;

impl Policy {
    fn extra(&self, host: &str) -> bool {
        self.also.iter().any(|h| h == host)
    }

    /// Hugging Face and its CDN (`*.hf.co`), GitHub and where its release assets are served from.
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        matches!(host.as_str(), "huggingface.co" | "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com")
            || host.ends_with(".hf.co")
            || host.ends_with(".huggingface.co")
            || self.extra(&host)
    }

    /// The host of `url`, when the policy lets a download go there.
    pub fn check(&self, url: &str) -> Result<String> {
        let refuse = |why: &str| Error::Server(format!("the download was sent to `{}`, which {why}", clip(url, 200)));
        let (scheme, rest) = url.split_once("://").ok_or_else(|| refuse("is not a web address"))?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // `user@host` would send the request to `host` whatever comes before the `@`.
        if authority.is_empty() || authority.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(refuse("is not a plain web address"));
        }
        let host = authority.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(authority, |(h, _)| h);
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        if !self.allows_host(&host) {
            return Err(refuse("is not a host model files or ComfyUI are served from"));
        }
        match scheme.to_ascii_lowercase().as_str() {
            "https" => Ok(host),
            "http" if self.extra(&host) => Ok(host),
            _ => Err(refuse("is not an https address")),
        }
    }

    /// Where a redirect from `from` to `location` goes, when the policy allows it.
    pub fn redirect(&self, from: &str, location: &str) -> Result<String> {
        let location = location.trim();
        let next = if location.contains("://") {
            location.to_string()
        } else if let Some(path) = location.strip_prefix("//") {
            format!("{}://{path}", from.split_once("://").map_or("https", |(s, _)| s))
        } else if location.starts_with('/') {
            // The same host: a renamed repository.
            let (scheme, rest) = from.split_once("://").ok_or_else(|| Error::Server("a redirect came from nowhere".into()))?;
            format!("{scheme}://{}{location}", rest.split(['/', '?', '#']).next().unwrap_or(""))
        } else {
            return Err(Error::Server(format!("the server redirected to `{}`, which is not an address", clip(location, 200))));
        };
        self.check(&next)?;
        Ok(next)
    }

    /// The token to send to `url`: only ever to Hugging Face itself, never to a CDN it redirects to.
    pub fn token_for<'a>(&self, url: &str, token: Option<&'a str>) -> Option<&'a str> {
        let host = self.check(url).ok()?;
        token.filter(|t| !t.trim().is_empty() && (host == TOKEN_HOST || self.extra(&host)))
    }
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).chain(['…']).collect() }
}

/// What to say about an error status, and whether trying again could help.
pub fn status_error(status: u16, url: &str) -> Error {
    let host = url.split_once("://").map_or(url, |(_, r)| r).split('/').next().unwrap_or("").to_string();
    match status {
        401 | 403 => Error::Access(format!(
            "{host} refused the file ({status}). It is a gated model: sign in with Hugging Face and accept its licence on the model's page (the Models step shows how), or leave it out."
        )),
        404 | 410 => Error::Server(format!("{host} no longer has the file ({status}). The model may have moved: check for a newer version of the app.")),
        429 => Error::Network(format!("{host} is asking for fewer requests ({status}).")),
        s if s >= 500 => Error::Network(format!("{host} answered with an error ({s}).")),
        s => Error::Server(format!("{host} answered {s} instead of the file.")),
    }
}

/// `Content-Range: bytes 100-199/1000` → (100, Some(1000)).
pub fn content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start_matches([' ', '=']);
    let (range, total) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

/// [`Fetch`] over `ureq` (rustls). Honours the system's proxy settings (the `*_PROXY`
/// environment variables); a proxy that intercepts TLS fails with a message, not a retry loop.
pub struct UreqFetch {
    agent: ureq::Agent,
    policy: Policy,
    /// Give up on a connection that has sent nothing for this long.
    stall: Duration,
}

impl Default for UreqFetch {
    fn default() -> Self {
        Self::new(Policy::default())
    }
}

/// How much is read at a time, and how many such chunks may wait to be written.
const CHUNK: usize = 64 * 1024;
const QUEUE: usize = 64;

enum Piece {
    Begin(Begin),
    Data(Vec<u8>),
    End(Result<()>),
}

impl UreqFetch {
    pub fn new(policy: Policy) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // Redirects are followed here, under the policy.
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(45)))
            .user_agent("PhotoCraft-setup")
            .build();
        Self { agent: config.into(), policy, stall: Duration::from_secs(60) }
    }

    #[cfg(test)]
    fn stalling_after(mut self, stall: Duration) -> Self {
        self.stall = stall;
        self
    }
}

fn network(e: &ureq::Error) -> Error {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("certificate") || lower.contains("tls") && lower.contains("invalid") {
        // Not worth retrying: every attempt meets the same certificate.
        return Error::Server(format!(
            "The download server's certificate could not be verified ({}). A proxy or security product that inspects encrypted traffic may be in the way: allow huggingface.co and github.com through it.",
            clip(&text, 200)
        ));
    }
    Error::Network(clip(&text, 300))
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string())
}

/// Send `request` and follow its redirects under `policy`. The token goes only where the policy
/// sends it, hop by hop.
fn open(agent: &ureq::Agent, policy: &Policy, head: bool, request: &OwnedRequest) -> Result<(String, ureq::http::Response<ureq::Body>)> {
    let mut url = request.url.clone();
    policy.check(&url)?;
    for _ in 0..MAX_HOPS {
        let mut b = if head { agent.head(&url) } else { agent.get(&url) };
        if let Some(token) = policy.token_for(&url, request.token.as_deref()) {
            b = b.header("Authorization", format!("Bearer {}", token.trim()));
        }
        if !head && request.offset > 0 {
            b = b.header("Range", format!("bytes={}-", request.offset));
            if let Some(v) = request.validator.as_deref().filter(|v| !v.is_empty()) {
                b = b.header("If-Range", v);
            }
        }
        // Compressed transfer would make offsets meaningless.
        b = b.header("Accept-Encoding", "identity");
        let response = b.call().map_err(|e| network(&e))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = header(&response, "location").ok_or_else(|| Error::Server("the server redirected without saying where".into()))?;
            url = policy.redirect(&url, &location)?;
            continue;
        }
        return Ok((url, response));
    }
    Err(Error::Server("the download was redirected too many times".into()))
}

struct OwnedRequest {
    url: String,
    token: Option<String>,
    offset: u64,
    validator: Option<String>,
}

impl Fetch for UreqFetch {
    fn head(&self, url: &str, token: Option<&str>) -> Result<Head> {
        // Hugging Face answers a file's own address with its size and sha256 and a redirect to
        // the CDN: read those from the first answer that carries them.
        let mut url = url.to_string();
        self.policy.check(&url)?;
        let mut out = Head::default();
        for _ in 0..MAX_HOPS {
            let mut b = self.agent.head(&url);
            if let Some(token) = self.policy.token_for(&url, token) {
                b = b.header("Authorization", format!("Bearer {}", token.trim()));
            }
            let response = b.call().map_err(|e| network(&e))?;
            out.status = response.status().as_u16();
            let linked = header(&response, "x-linked-size").and_then(|v| v.parse().ok());
            let sha = header(&response, "x-linked-etag").map(|v| v.trim_matches('"').to_ascii_lowercase()).filter(|v| v.len() == 64);
            out.sha256 = out.sha256.take().or(sha);
            out.size = out.size.or(linked);
            if matches!(out.status, 301 | 302 | 303 | 307 | 308) {
                let Some(location) = header(&response, "location") else { break };
                url = self.policy.redirect(&url, &location)?;
                continue;
            }
            out.size = out.size.or_else(|| header(&response, "content-length").and_then(|v| v.parse().ok()));
            out.error_code = header(&response, "x-error-code").map(|v| clip(&v, 80));
            out.error_message = header(&response, "x-error-message").map(|v| clip(&v, 400));
            break;
        }
        Ok(out)
    }

    fn fetch(&self, request: &Request<'_>, sink: &mut dyn Sink, cancelled: &dyn Fn() -> bool) -> Result<()> {
        let owned = OwnedRequest {
            url: request.url.to_string(),
            token: request.token.map(str::to_string),
            offset: request.offset,
            validator: request.validator.map(str::to_string),
        };
        let (agent, policy) = (self.agent.clone(), self.policy.clone());
        // The connection is read on a thread of its own: a read that never returns (a dead
        // connection the OS has not noticed) can't be interrupted, but it can be left behind.
        // This thread then notices the silence, and a cancel, within moments.
        let (tx, rx) = mpsc::sync_channel::<Piece>(QUEUE);
        let reader = move || {
            let end = (|| -> Result<()> {
                let (url, response) = open(&agent, &policy, false, &owned)?;
                let status = response.status().as_u16();
                let offset = match status {
                    206 => header(&response, "content-range").and_then(|v| content_range(&v)).map_or(owned.offset, |(start, _)| start),
                    200 => 0,
                    // Asked for bytes past the end: the file is already whole.
                    416 => return Err(Error::Integrity("the server has fewer bytes than were already downloaded".into())),
                    s => return Err(status_error(s, &url)),
                };
                if offset > owned.offset {
                    return Err(Error::Server("the server skipped part of the file".into()));
                }
                let total = match status {
                    206 => header(&response, "content-range").and_then(|v| content_range(&v)).and_then(|(_, total)| total),
                    _ => response.body().content_length(),
                };
                let validator = header(&response, "etag").unwrap_or_default();
                if tx.send(Piece::Begin(Begin { offset, total, validator })).is_err() {
                    return Ok(());
                }
                let mut body = response.into_body().into_reader();
                let mut chunk = vec![0u8; CHUNK];
                loop {
                    let n = body.read(&mut chunk).map_err(|e| Error::Network(format!("the connection broke: {e}")))?;
                    let Some(read) = chunk.get(..n).filter(|r| !r.is_empty()) else { return Ok(()) };
                    // The receiver is gone: cancelled, or given up on.
                    if tx.send(Piece::Data(read.to_vec())).is_err() {
                        return Ok(());
                    }
                }
            })();
            let _ = tx.send(Piece::End(end));
        };
        std::thread::Builder::new().name("photocraft-download".into()).spawn(reader).map_err(|e| Error::Network(format!("could not start a download: {e}")))?;
        let tick = Duration::from_millis(200);
        let mut silent = Duration::ZERO;
        loop {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            match rx.recv_timeout(tick) {
                Ok(Piece::Begin(begin)) => {
                    silent = Duration::ZERO;
                    sink.begin(&begin)?;
                }
                Ok(Piece::Data(bytes)) => {
                    silent = Duration::ZERO;
                    sink.write(&bytes)?;
                }
                Ok(Piece::End(end)) => return end,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    silent += tick;
                    if silent >= self.stall {
                        return Err(Error::Network(format!("nothing arrived for {} seconds", self.stall.as_secs())));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Network("the download stopped without an answer".into())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn downloads_go_only_to_the_hosts_files_are_served_from() {
        let p = Policy::default();
        for ok in [
            "https://huggingface.co/Comfy-Org/x/resolve/main/a.safetensors",
            "https://us.aws.cdn.hf.co/xet-bridge-us/abc?sig=1",
            "https://cas-bridge.xethub.hf.co/x",
            "https://cdn-lfs.huggingface.co/x",
            "https://github.com/Comfy-Org/ComfyUI/releases/download/v1/a.7z",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1?sp=r",
            "https://objects.githubusercontent.com/x",
            "https://HuggingFace.co:443/x",
        ] {
            assert!(p.check(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://huggingface.co/x",
            "https://evil.example/x",
            "https://huggingface.co.evil.example/x",
            "https://evilhf.co/x",
            "https://huggingface.co@evil.example/x",
            "https://evil.example/huggingface.co/x",
            "ftp://huggingface.co/x",
            "file:///C:/Windows/system32",
            "huggingface.co/x",
            "https:///x",
            "https://huggingface.co/x y",
            "",
        ] {
            assert!(p.check(bad).is_err(), "{bad}");
        }
        assert_eq!(p.check("https://us.aws.cdn.hf.co/x").unwrap(), "us.aws.cdn.hf.co");
    }

    #[test]
    fn redirects_stay_inside_the_policy_and_the_token_stays_on_hugging_face() {
        let p = Policy::default();
        let from = "https://huggingface.co/Comfy-Org/old/resolve/main/a.safetensors";
        assert_eq!(p.redirect(from, "/Comfy-Org/new/resolve/main/a.safetensors").unwrap(), "https://huggingface.co/Comfy-Org/new/resolve/main/a.safetensors");
        assert_eq!(p.redirect(from, "https://us.aws.cdn.hf.co/x?sig=1").unwrap(), "https://us.aws.cdn.hf.co/x?sig=1");
        assert_eq!(p.redirect(from, "//us.aws.cdn.hf.co/x").unwrap(), "https://us.aws.cdn.hf.co/x");
        for bad in ["https://evil.example/a.safetensors", "http://us.aws.cdn.hf.co/x", "//evil.example/x", "elsewhere", "", "javascript:alert(1)"] {
            assert!(p.redirect(from, bad).is_err(), "{bad}");
        }
        // The token is for Hugging Face itself. Its CDN's addresses are signed and need none;
        // GitHub never gets it.
        let token = Some("hf_secret");
        assert_eq!(p.token_for(from, token), token);
        for other in ["https://us.aws.cdn.hf.co/x", "https://github.com/a/b.7z", "https://cdn-lfs.huggingface.co/x", "https://evil.example/x", "nonsense"] {
            assert_eq!(p.token_for(other, token), None, "{other}");
        }
        assert_eq!(p.token_for(from, Some("  ")), None);
        assert_eq!(p.token_for(from, None), None);
    }

    #[test]
    fn statuses_say_what_to_do_and_whether_to_try_again() {
        let url = "https://huggingface.co/black-forest-labs/x/resolve/main/a.safetensors";
        assert!(matches!(status_error(401, url), Error::Access(m) if m.contains("gated") && m.contains("huggingface.co")));
        assert!(matches!(status_error(403, url), Error::Access(_)));
        assert!(matches!(status_error(404, url), Error::Server(m) if m.contains("newer version")));
        assert!(matches!(status_error(429, url), Error::Network(_)));
        assert!(matches!(status_error(503, url), Error::Network(_)));
        assert!(matches!(status_error(418, "nonsense"), Error::Server(_)));
        assert_eq!(content_range("bytes 100-199/1000"), Some((100, Some(1000))));
        assert_eq!(content_range("bytes 0-0/*"), Some((0, None)));
        for junk in ["", "bytes", "bytes */100", "items 1-2/3", "bytes a-b/c", "bytes 5/9"] {
            assert_eq!(content_range(junk), None, "{junk}");
        }
    }

    /// What the local server does with a request for `/file`.
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// Serves ranges, with an ETag; honours If-Range.
        Ranges,
        /// Ignores ranges: always the whole file.
        Whole,
        /// Sends the headers and half the body, then closes.
        Drop,
        /// Sends the headers, then nothing.
        Stall,
        /// Answers 401.
        Gated,
    }

    struct Server {
        base: String,
        host: String,
        /// The `Authorization` headers that reached `/file` and `/start`.
        seen: Arc<std::sync::Mutex<Vec<(String, String)>>>,
        requests: Arc<AtomicUsize>,
    }

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|i| (i % 251) as u8).collect()
    }

    /// A tiny HTTP/1.1 server on a loopback port. `/start` redirects to `/file` (a relative
    /// redirect, as Hugging Face does for a renamed repository); `/away` redirects off the policy.
    fn serve(mode: Mode) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = Arc::new(AtomicUsize::new(0));
        let (seen2, requests2) = (seen.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let (seen, requests) = (seen2.clone(), requests2.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                    let mut headers = std::collections::HashMap::new();
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                        }
                    }
                    requests.fetch_add(1, Ordering::SeqCst);
                    seen.lock().unwrap().push((path.clone(), headers.get("authorization").cloned().unwrap_or_default()));
                    let data = body();
                    let send = |stream: &mut std::net::TcpStream, head: String, payload: &[u8]| {
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(payload);
                        let _ = stream.flush();
                    };
                    match path.as_str() {
                        "/start" => send(&mut stream, "HTTP/1.1 307 Temporary Redirect\r\nLocation: /file\r\nX-Linked-Size: 300000\r\nX-Linked-ETag: \"0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), b""),
                        "/away" => send(&mut stream, "HTTP/1.1 302 Found\r\nLocation: https://evil.example/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), b""),
                        "/loop" => send(&mut stream, "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), b""),
                        _ if mode == Mode::Gated => send(&mut stream, "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), b""),
                        _ => {
                            let etag = "\"v1\"";
                            let range = headers.get("range").and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split('-').next()).and_then(|s| s.parse::<usize>().ok());
                            let fresh = headers.get("if-range").is_none_or(|v| v == etag);
                            let start = if mode == Mode::Whole || !fresh { None } else { range.filter(|s| *s < data.len()) };
                            let payload = &data[start.unwrap_or(0)..];
                            let head = match start {
                                Some(s) => format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{}/{}\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n",
                                    data.len() - 1,
                                    data.len(),
                                    payload.len()
                                ),
                                None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n", payload.len()),
                            };
                            if method == "HEAD" {
                                return send(&mut stream, head, b"");
                            }
                            match mode {
                                Mode::Drop => send(&mut stream, head, &payload[..payload.len() / 2]),
                                Mode::Stall => {
                                    send(&mut stream, head, &payload[..1000]);
                                    std::thread::sleep(Duration::from_secs(4));
                                }
                                _ => send(&mut stream, head, payload),
                            }
                        }
                    }
                });
            }
        });
        Server { base: format!("http://127.0.0.1:{port}"), host: "127.0.0.1".into(), seen, requests }
    }

    #[derive(Default)]
    struct Memory {
        begun: Vec<Begin>,
        data: Vec<u8>,
    }

    impl Sink for Memory {
        fn begin(&mut self, begin: &Begin) -> Result<()> {
            self.data.truncate(begin.offset as usize);
            self.begun.push(begin.clone());
            Ok(())
        }

        fn write(&mut self, bytes: &[u8]) -> Result<()> {
            self.data.extend_from_slice(bytes);
            Ok(())
        }
    }

    fn fetcher(server: &Server) -> UreqFetch {
        UreqFetch::new(Policy { also: vec![server.host.clone()] })
    }

    fn get(server: &Server, path: &str, offset: u64, validator: Option<&str>, sink: &mut Memory) -> Result<()> {
        let url = format!("{}{path}", server.base);
        fetcher(server).fetch(&Request { url: &url, token: Some("hf_secret"), offset, validator }, sink, &|| false)
    }

    #[test]
    fn a_file_streams_whole_through_a_redirect() {
        let server = serve(Mode::Ranges);
        let mut sink = Memory::default();
        get(&server, "/start", 0, None, &mut sink).unwrap();
        assert_eq!(sink.data, body());
        assert_eq!(sink.begun, [Begin { offset: 0, total: Some(300_000), validator: "\"v1\"".into() }]);
        // The test host is in the policy, so it is sent the token on both hops.
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen, [("/start".to_string(), "Bearer hf_secret".to_string()), ("/file".to_string(), "Bearer hf_secret".to_string())]);
        // The same address asked without a body: the size and sha256 of the first answer.
        let head = fetcher(&server).head(&format!("{}/start", server.base), None).unwrap();
        assert_eq!((head.status, head.size, head.sha256.as_deref().map(str::len)), (200, Some(300_000), Some(64)));
    }

    #[test]
    fn a_download_resumes_where_it_stopped() {
        let server = serve(Mode::Ranges);
        let mut sink = Memory { data: body()[..100_000].to_vec(), ..Memory::default() };
        get(&server, "/file", 100_000, Some("\"v1\""), &mut sink).unwrap();
        assert_eq!(sink.data, body());
        assert_eq!(sink.begun[0].offset, 100_000);
        // The file changed upstream: the server sends all of it, and the sink starts over
        // instead of stitching two files together.
        let mut sink = Memory { data: vec![9; 100_000], ..Memory::default() };
        get(&server, "/file", 100_000, Some("\"old\""), &mut sink).unwrap();
        assert_eq!((sink.begun[0].offset, &sink.data), (0, &body()));
        // A server that ignores ranges does the same.
        let whole = serve(Mode::Whole);
        let mut sink = Memory { data: vec![9; 100_000], ..Memory::default() };
        get(&whole, "/file", 100_000, None, &mut sink).unwrap();
        assert_eq!((sink.begun[0].offset, &sink.data), (0, &body()));
    }

    #[test]
    fn a_dropped_connection_is_a_network_error_and_what_arrived_is_kept() {
        let server = serve(Mode::Drop);
        let mut sink = Memory::default();
        let e = get(&server, "/file", 0, None, &mut sink).unwrap_err();
        assert!(matches!(e, Error::Network(_)), "{e:?}");
        assert!(!sink.data.is_empty() && sink.data.len() < body().len() && sink.data[..] == body()[..sink.data.len()]);
    }

    #[test]
    fn silence_is_noticed_and_so_is_a_cancel() {
        let server = serve(Mode::Stall);
        let url = format!("{}/file", server.base);
        let quick = fetcher(&server).stalling_after(Duration::from_millis(600));
        let started = std::time::Instant::now();
        let e = quick.fetch(&Request { url: &url, token: None, offset: 0, validator: None }, &mut Memory::default(), &|| false).unwrap_err();
        assert!(matches!(&e, Error::Network(m) if m.contains("nothing arrived")), "{e:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // Cancelling does not wait for the connection either.
        let started = std::time::Instant::now();
        let asked = AtomicUsize::new(0);
        let cancelled = || asked.fetch_add(1, Ordering::SeqCst) >= 2;
        let e = fetcher(&server).fetch(&Request { url: &url, token: None, offset: 0, validator: None }, &mut Memory::default(), &cancelled).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// The real hosts, with the real policy: the first bytes of the pinned runtime and of a
    /// model file, and what a gated file answers. Needs the network, and downloads about 200 KB:
    /// `cargo test -p photocraft-aisetup --lib real_hosts -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the network"]
    fn real_hosts_serve_ranges_through_their_redirects() {
        let m = crate::Manifest::bundled().unwrap();
        let fetch = UreqFetch::default();
        let runtime = &m.comfy.assets["nvidia"];
        let model = &m.files["vae/ae.safetensors"];
        let gated = m.files.values().find(|f| f.gated).unwrap();
        for (what, url, size, offset) in [("runtime", runtime.url.as_str(), runtime.size, 0u64), ("model", model.url.as_str(), model.size, 1_000_000)] {
            let head = fetch.head(url, None).unwrap();
            println!("{what}: HEAD {} size {:?} sha256 {:?}", head.status, head.size, head.sha256);
            assert_eq!(head.size, Some(size), "{what}");
            // Stop after the first hundred kilobytes.
            let seen = AtomicUsize::new(0);
            struct Count<'a>(&'a AtomicUsize, Vec<Begin>);
            impl Sink for Count<'_> {
                fn begin(&mut self, begin: &Begin) -> Result<()> {
                    self.1.push(begin.clone());
                    Ok(())
                }
                fn write(&mut self, bytes: &[u8]) -> Result<()> {
                    self.0.fetch_add(bytes.len(), Ordering::SeqCst);
                    Ok(())
                }
            }
            let mut sink = Count(&seen, Vec::new());
            let e = fetch.fetch(&Request { url, token: None, offset, validator: None }, &mut sink, &|| seen.load(Ordering::SeqCst) >= 100_000).unwrap_err();
            assert_eq!(e, Error::Cancelled, "{what}");
            println!("{what}: began {:?}, {} bytes before the cancel", sink.1, seen.load(Ordering::SeqCst));
            assert_eq!((sink.1[0].offset, sink.1[0].total), (offset, Some(size)), "{what}");
        }
        assert_eq!(fetch.head(&model.url, None).unwrap().sha256.as_deref(), Some(model.sha256.as_str()));
        // A gated file without a sign-in: refused, and said so.
        let e = fetch.fetch(&Request { url: &gated.url, token: None, offset: 0, validator: None }, &mut Memory::default(), &|| false).unwrap_err();
        println!("gated: {e}");
        assert!(matches!(e, Error::Access(_)));
    }

    #[test]
    fn refusals_and_wandering_redirects_are_not_followed() {
        let gated = serve(Mode::Gated);
        let e = get(&gated, "/file", 0, None, &mut Memory::default()).unwrap_err();
        assert!(matches!(&e, Error::Access(m) if m.contains("gated")), "{e:?}");
        let server = serve(Mode::Ranges);
        let e = get(&server, "/away", 0, None, &mut Memory::default()).unwrap_err();
        assert!(matches!(&e, Error::Server(m) if m.contains("evil.example")), "{e:?}");
        // Nothing was asked of the file after the redirect was refused.
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        let e = get(&server, "/loop", 0, None, &mut Memory::default()).unwrap_err();
        assert!(matches!(&e, Error::Server(m) if m.contains("too many")), "{e:?}");
        // An address outside the policy is refused before any request.
        let before = server.requests.load(Ordering::SeqCst);
        let e = UreqFetch::default().fetch(
            &Request { url: &format!("{}/file", server.base), token: None, offset: 0, validator: None },
            &mut Memory::default(),
            &|| false,
        );
        assert!(e.is_err() && server.requests.load(Ordering::SeqCst) == before);
        // Nothing listening: a network error, promptly.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let dead = UreqFetch::new(Policy { also: vec!["127.0.0.1".into()] });
        let e =
            dead.fetch(&Request { url: &format!("http://127.0.0.1:{closed}/file"), token: None, offset: 0, validator: None }, &mut Memory::default(), &|| {
                false
            });
        assert!(matches!(e, Err(Error::Network(_))), "{e:?}");
    }
}
