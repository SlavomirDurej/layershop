//! A websocket's text messages, read with the standard library: what ComfyUI says while it
//! renders (`/ws`). Only what that needs: plain `ws://`, the server's frames read and a ping
//! answered; nothing is ever sent but the pong and the close.

use crate::{Error, Result};

/// Largest frame kept (ComfyUI's text messages are small; its preview images are skipped).
const MAX_FRAME: u64 = 32 * 1024 * 1024;
/// Largest message put together from its fragments.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// One frame from the server.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// The frame at the start of `buf`, and how many bytes it took: `Ok(None)` until all of it has
/// arrived. A frame larger than [`MAX_FRAME`] is an error, not an allocation.
pub(crate) fn parse_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
    let (Some(&b0), Some(&b1)) = (buf.first(), buf.get(1)) else { return Ok(None) };
    let (fin, opcode, masked) = (b0 & 0x80 != 0, b0 & 0x0F, b1 & 0x80 != 0);
    let mut at = 2usize;
    let len = match b1 & 0x7F {
        126 => {
            let Some(b) = buf.get(2..4).and_then(|b| <[u8; 2]>::try_from(b).ok()) else { return Ok(None) };
            at = 4;
            u64::from(u16::from_be_bytes(b))
        }
        127 => {
            let Some(b) = buf.get(2..10).and_then(|b| <[u8; 8]>::try_from(b).ok()) else { return Ok(None) };
            at = 10;
            u64::from_be_bytes(b)
        }
        n => u64::from(n),
    };
    if len > MAX_FRAME {
        return Err(Error::BadResponse(format!("ComfyUI sent a {len}-byte websocket frame")));
    }
    let mask = if masked {
        let Some(m) = buf.get(at..at + 4).and_then(|m| <[u8; 4]>::try_from(m).ok()) else { return Ok(None) };
        at += 4;
        Some(m)
    } else {
        None
    };
    let end = at.saturating_add(len as usize);
    let Some(body) = buf.get(at..end) else { return Ok(None) };
    let mut payload = body.to_vec();
    if let Some(mask) = mask {
        for (b, m) in payload.iter_mut().zip(mask.iter().cycle()) {
            *b ^= m;
        }
    }
    Ok(Some((Frame { fin, opcode, payload }, end)))
}

/// Bytes from the socket put together into frames, and frames into text messages; what is not
/// whole yet is kept for the next read.
pub(crate) struct Reader {
    buf: Vec<u8>,
    text: Vec<u8>,
    /// The message being put together is text (binary ones, ComfyUI's previews, are dropped).
    in_text: bool,
}

impl Reader {
    pub fn new() -> Self {
        Self { buf: Vec::new(), text: Vec::new(), in_text: false }
    }

    /// Take `bytes` from the socket. Each whole text message goes to `on_text`; pings are
    /// returned to answer. `Ok(true)` once the server closes.
    pub fn take(&mut self, bytes: &[u8], on_text: &mut dyn FnMut(&str), pings: &mut Vec<Vec<u8>>) -> Result<bool> {
        self.buf.extend_from_slice(bytes);
        while let Some((frame, used)) = parse_frame(&self.buf)? {
            self.buf.drain(..used.min(self.buf.len()));
            match frame.opcode {
                0x8 => return Ok(true),
                0x9 => pings.push(frame.payload),
                0xA => {}
                0x0..=0x2 => {
                    if frame.opcode != 0x0 {
                        self.text.clear();
                        self.in_text = frame.opcode == 0x1;
                    }
                    if self.in_text && self.text.len().saturating_add(frame.payload.len()) <= MAX_MESSAGE {
                        self.text.extend_from_slice(&frame.payload);
                    }
                    if frame.fin {
                        if self.in_text {
                            on_text(&String::from_utf8_lossy(&self.text));
                        }
                        self.text.clear();
                        self.in_text = false;
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

/// A frame from the client: masked, as the protocol requires of clients.
pub(crate) fn client_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x80 | (opcode & 0x0F)];
    let len = payload.len().min(125);
    out.push(0x80 | len as u8);
    out.extend_from_slice(&mask);
    out.extend(payload.iter().take(len).zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    out
}

/// `ws://host:port/path` split into the address to connect to and the path to ask for.
pub(crate) fn split_url(url: &str) -> Result<(String, String)> {
    let rest = url.strip_prefix("ws://").ok_or_else(|| Error::Config(format!("{url} is not a plain ws:// address")))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(Error::Config(format!("{url} names no server")));
    }
    Ok((authority.to_string(), path.to_string()))
}

/// Connect to `url` and hand each text message to `on_text` until the server closes, the
/// connection fails, or `stop` says so (checked twice a second).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn listen(url: &str, stop: &dyn Fn() -> bool, on_text: &mut dyn FnMut(&str)) -> Result<()> {
    use base64::Engine;
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let net = |e: std::io::Error| Error::Network(format!("ComfyUI's live updates: {e}"));
    let (authority, path) = split_url(url)?;
    let addr = authority.to_socket_addrs().map_err(net)?.next().ok_or_else(|| Error::Network(format!("{authority} has no address")))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(net)?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).map_err(net)?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(net)?;
    let seed = crate::random_seed().to_le_bytes();
    let key = base64::engine::general_purpose::STANDARD.encode([seed, crate::random_seed().to_le_bytes()].concat());
    let hello = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: PhotoCraft\r\n\r\n"
    );
    stream.write_all(hello.as_bytes()).map_err(net)?;
    // The answer's head; what follows it is already frames.
    let mut head = Vec::new();
    let mut chunk = [0u8; 8192];
    let body_at = loop {
        if stop() {
            return Ok(());
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(Error::Network("ComfyUI closed its live updates before answering".into())),
            Ok(n) => head.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(e) => return Err(net(e)),
        }
        if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if head.len() > 16 * 1024 {
            return Err(Error::BadResponse("ComfyUI's live updates answered with too long a head".into()));
        }
    };
    let status = String::from_utf8_lossy(head.get(..body_at).unwrap_or(&[])).lines().next().unwrap_or("").to_string();
    if !status.split_whitespace().nth(1).is_some_and(|code| code == "101") {
        return Err(Error::Server(format!("ComfyUI did not open its live updates ({status})")));
    }
    let mut reader = Reader::new();
    let mut pings = Vec::new();
    let mut pending = head.get(body_at..).unwrap_or(&[]).to_vec();
    loop {
        let closed = reader.take(&pending, on_text, &mut pings)?;
        for ping in pings.drain(..) {
            stream.write_all(&client_frame(0xA, &ping, [seed[0], seed[1], seed[2], seed[3]])).map_err(net)?;
        }
        if closed {
            return Ok(());
        }
        if stop() {
            let _ = stream.write_all(&client_frame(0x8, &[], [7, 7, 7, 7]));
            return Ok(());
        }
        pending.clear();
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => pending.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(net(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        match payload.len() {
            n if n < 126 => out.push(n as u8),
            n if n <= 0xFFFF => {
                out.push(126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn frames_are_read_whole_whatever_pieces_they_arrive_in() {
        let big = vec![b'x'; 70_000];
        let mut bytes = server_frame(1, true, br#"{"type":"status"}"#);
        bytes.extend(server_frame(2, true, &big)); // a preview: dropped
        bytes.extend(server_frame(1, false, b"{\"type\":"));
        bytes.extend(server_frame(9, true, b"hi")); // a ping between fragments
        bytes.extend(server_frame(0, true, b"\"progress\"}"));
        bytes.extend(server_frame(1, true, &vec![b'y'; 300]));
        bytes.extend(server_frame(8, true, b""));
        for piece in [1, 7, 4096, bytes.len()] {
            let (mut reader, mut texts, mut pings) = (Reader::new(), Vec::new(), Vec::new());
            let mut closed = false;
            for chunk in bytes.chunks(piece) {
                closed = reader.take(chunk, &mut |t| texts.push(t.to_string()), &mut pings).unwrap();
            }
            assert!(closed, "piece {piece}");
            assert_eq!(texts.len(), 3, "piece {piece}");
            assert_eq!((texts[0].as_str(), texts[1].as_str(), texts[2].len()), (r#"{"type":"status"}"#, r#"{"type":"progress"}"#, 300));
            assert_eq!(pings, vec![b"hi".to_vec()]);
        }
    }

    #[test]
    fn hostile_frames_are_errors_or_waits_never_panics() {
        // An eight-exabyte length is refused before anything is allocated.
        let mut huge = vec![0x81, 127];
        huge.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(parse_frame(&huge).is_err());
        // Cut short anywhere: wait for the rest.
        let whole = server_frame(1, true, &[b'a'; 200]);
        for n in 0..whole.len() {
            assert_eq!(parse_frame(&whole[..n]).unwrap(), None, "{n}");
        }
        // A masked frame (servers shouldn't, but it reads), and the client's own pong.
        let pong = client_frame(0xA, b"hello", [1, 2, 3, 4]);
        let (frame, used) = parse_frame(&pong).unwrap().unwrap();
        assert_eq!((frame.opcode, frame.payload.as_slice(), used), (0xA, &b"hello"[..], pong.len()));
        assert!(Reader::new().take(&[0xFF; 3], &mut |_| {}, &mut Vec::new()).is_ok());
    }

    #[test]
    fn addresses_split_into_server_and_path() {
        assert_eq!(split_url("ws://127.0.0.1:8188/ws?clientId=a").unwrap(), ("127.0.0.1:8188".into(), "/ws?clientId=a".into()));
        assert_eq!(split_url("ws://comfy:8188").unwrap(), ("comfy:8188".into(), "/".into()));
        assert!(split_url("wss://comfy/ws").is_err() && split_url("ws:///ws").is_err() && split_url("").is_err());
    }
}
