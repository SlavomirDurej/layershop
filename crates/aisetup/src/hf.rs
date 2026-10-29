//! Hugging Face, for the gated models (today FLUX.3 Klein): a model whose maker lets it be
//! downloaded only by someone who accepted its licence, signed in.
//!
//! Two steps, both the user's, in their browser:
//!
//! 1. **Sign in with Hugging Face.** OAuth 2 authorization code with PKCE (S256), as a public app
//!    (no client secret: an app on someone's PC can't keep one). The browser comes back to a
//!    one-shot listener on `127.0.0.1:<any port>` ([`SignIn`]); a `state` that is not the one
//!    sent is refused. The scopes are `openid profile gated-repos`: read the public gated repos
//!    the user was granted, and nothing private. The access token lasts hours; the refresh token
//!    renews it ([`refresh`]).
//! 2. **Accept the licence**, on the model's page. Only Hugging Face can do that.
//!
//! Where a model stands is asked with one `HEAD` of its gated file ([`access`]): `2xx` (or the
//! redirect to the CDN) is ready, `401` is not signed in (or a token Hugging Face doesn't take),
//! `403` is signed in without the licence accepted, `404` is a file that is not there any more (the
//! maker renamed it: only a newer manifest knows where it went). Nothing is downloaded. Signed
//! out, Hugging Face answers 401 before it looks for the file, so a moved file shows only to
//! someone signed in.
//!
//! An access token pasted by hand is the fallback (organisation token policies, or a sign-in
//! that fails), checked with [`whoami`]. A token found on this PC (`HF_TOKEN`, or the
//! `huggingface_hub` token file) is offered, never used unasked ([`found_on_this_pc`]).
//!
//! Every request with a token goes to `https://huggingface.co/` and nowhere else: the ones here
//! through [`send`], the downloads through [`crate::fetch::Policy`], which drops it on the CDN
//! redirect.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{Manifest, ModelEntry};
use crate::{Error, Result};

/// Where every request that carries a token goes.
pub const HOST: &str = "https://huggingface.co/";
pub const AUTHORIZE_URL: &str = "https://huggingface.co/oauth/authorize";
pub const TOKEN_URL: &str = "https://huggingface.co/oauth/token";
pub const USERINFO_URL: &str = "https://huggingface.co/oauth/userinfo";
/// Who a pasted access token belongs to.
pub const WHOAMI_URL: &str = "https://huggingface.co/api/whoami-v2";
/// What the app asks for: who the user is, and the public gated repos they were granted.
pub const SCOPES: &str = "openid profile gated-repos";
/// Where the user sees and revokes the apps they signed in to.
pub const CONNECTED_APPS_URL: &str = "https://huggingface.co/settings/connected-applications";
/// Where an access token is made, for the fallback.
pub const TOKENS_URL: &str = "https://huggingface.co/settings/tokens";
/// "How does this work?"
pub const HELP_URL: &str = "https://layershop.aardvarkstudios.net/hugging-face/";
/// LayerShop as registered with Hugging Face (2026-10-16): a public OAuth app (no secret) whose
/// redirect URIs are `http://127.0.0.1/callback` and `http://localhost/callback`, scopes
/// `openid profile gated-repos`. A client id is public by design: it names the app, it grants
/// nothing. Empty would make Sign in with Hugging Face unavailable, leaving the token.
pub const CLIENT_ID: &str = "da09846f-b9d1-4e60-a51e-618c1e30d707";
/// How long a sign-in waits for the browser.
pub const SIGN_IN_SECONDS: u64 = 300;
/// An access token is renewed when it has less than this left (seconds).
pub const REFRESH_MARGIN: u64 = 300;

/// The OAuth client id: `LAYERSHOP_HF_CLIENT_ID` when set (to try an app before it is built in),
/// else [`CLIENT_ID`].
pub fn client_id() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(id) = std::env::var("LAYERSHOP_HF_CLIENT_ID").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty() && v.len() <= 200) {
        return id;
    }
    CLIENT_ID.to_string()
}

/// How the user signed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    #[default]
    None,
    /// Sign in with Hugging Face: renewable.
    #[serde(rename = "oauth")]
    OAuth,
    /// An access token pasted by hand, or the one found on this PC.
    Token,
}

impl Via {
    pub fn key(self) -> &'static str {
        match self {
            Via::None => "",
            Via::OAuth => "oauth",
            Via::Token => "token",
        }
    }

    pub fn from_key(key: &str) -> Via {
        match key {
            "oauth" => Via::OAuth,
            "token" => Via::Token,
            _ => Via::None,
        }
    }
}

/// What signing in left: kept with the API keys, never in the preferences, never shown to
/// automation. `Debug` leaves the tokens out.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    pub access: String,
    /// Empty for an access token pasted by hand.
    pub refresh: String,
    /// When `access` stops working (Unix seconds); 0: not known to.
    pub expires_at: u64,
    /// The Hugging Face username, once known.
    pub user: String,
    pub via: Via,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access", &!self.access.is_empty())
            .field("refresh", &!self.refresh.is_empty())
            .field("expires_at", &self.expires_at)
            .field("user", &self.user)
            .field("via", &self.via)
            .finish()
    }
}

impl Credentials {
    pub fn signed_in(&self) -> bool {
        !self.access.trim().is_empty()
    }

    /// Has the access token run out at `now` (Unix seconds), with no refresh token to renew it?
    /// Hugging Face would refuse it: it is a sign-in no more.
    pub fn expired(&self, now: u64) -> bool {
        let renewable = self.via == Via::OAuth && !self.refresh.is_empty();
        self.signed_in() && self.expires_at != 0 && now >= self.expires_at && !renewable
    }

    /// Should the access token be renewed before it is used, at `now` (Unix seconds)?
    pub fn needs_refresh(&self, now: u64) -> bool {
        self.via == Via::OAuth && !self.refresh.is_empty() && self.expires_at != 0 && now.saturating_add(REFRESH_MARGIN) >= self.expires_at
    }
}

/// Could `token` be a Hugging Face token (not empty, no spaces or control characters)?
pub fn plausible(token: &str) -> bool {
    let t = token.trim();
    (4..=2048).contains(&t.len()) && !t.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Where a gated model stands for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    /// Not signed in, or the sign-in is not taken any more.
    SignIn,
    /// Signed in; the licence is still to be accepted on the model's page.
    Licence,
    /// It can be downloaded.
    Ready,
    /// Its file is not on Hugging Face where this version of the app looks for it (renamed or
    /// removed by its maker): signing in can't help, a newer version of the app can.
    Unavailable,
}

/// A gated model and where it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAccess {
    pub id: String,
    pub name: String,
    /// Where its licence is accepted.
    pub page: String,
    pub licence: String,
    pub access: Access,
    /// What Hugging Face said, when it says more than the state does.
    pub detail: String,
}

/// The gated models among `models` (ids), in their order: the ones that need a sign-in.
pub fn gated<'a>(manifest: &'a Manifest, models: &[String]) -> Vec<&'a ModelEntry> {
    let mut out: Vec<&ModelEntry> = Vec::new();
    for id in models.iter().take(256) {
        let Some(m) = manifest.model(id) else { continue };
        let any_gated = m.files.iter().any(|k| manifest.files.get(k).is_some_and(|f| f.gated));
        if (m.gated || any_gated) && !out.iter().any(|o| o.id == m.id) {
            out.push(m);
        }
    }
    out
}

/// `bytes` in base64url without padding (RFC 4648 §5): PKCE's verifier, challenge and state.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let take = chunk.len() + 1;
        for i in 0..take {
            let index = ((n >> (18 - 6 * i)) & 63) as usize;
            out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
        }
    }
    out
}

/// `s` percent-encoded for a query string or a form body: only RFC 3986's unreserved characters
/// stay as they are.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The reverse of [`encode`], also reading `+` as a space. Bytes that don't form UTF-8 are
/// replaced, never trusted.
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = |at: usize| bytes.get(at).and_then(|c| char::from(*c).to_digit(16));
        match b {
            b'%' if hex(i + 1).is_some() && hex(i + 2).is_some() => {
                out.push(((hex(i + 1).unwrap_or(0) << 4) | hex(i + 2).unwrap_or(0)) as u8);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A query string as name/value pairs, decoded. At most 64 pairs.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query.split('&').filter(|p| !p.is_empty()).take(64).map(|p| p.split_once('=').map_or((decode(p), String::new()), |(k, v)| (decode(k), decode(v)))).collect()
}

/// The address the browser is sent to, to sign in.
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    format!(
        "{AUTHORIZE_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        encode(client_id),
        encode(redirect_uri),
        encode(SCOPES),
        encode(state),
        encode(challenge)
    )
}

/// `text` cut to `max` characters, for a message.
#[cfg(not(target_arch = "wasm32"))]
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).chain(['…']).collect() }
}

/// What Hugging Face's error JSON says (`{"error", "error_description"}`), for a message.
#[cfg(not(target_arch = "wasm32"))]
fn error_of(body: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(|s| clip(s, 300)).unwrap_or_default();
    (text("error"), text("error_description"))
}

/// The token endpoint's answer as credentials. A refresh that brings no new refresh token keeps
/// `previous_refresh`.
pub fn parse_tokens(body: &[u8], now: u64, previous_refresh: &str) -> Result<Credentials> {
    let v: Value = serde_json::from_slice(body).map_err(|_| Error::Server("Hugging Face answered the sign-in with something that is not JSON.".into()))?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|t| plausible(t)).map(str::to_string);
    let access = text("access_token").ok_or_else(|| Error::Server("Hugging Face's answer to the sign-in has no access token.".into()))?;
    let expires_at = v.get("expires_in").and_then(Value::as_u64).filter(|s| *s > 0 && *s < 10 * 365 * 86_400).map_or(0, |s| now.saturating_add(s));
    Ok(Credentials { access, refresh: text("refresh_token").unwrap_or_else(|| previous_refresh.to_string()), expires_at, user: String::new(), via: Via::OAuth })
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{
        Access, Credentials, Error, HOST, Manifest, ModelAccess, Result, TOKEN_URL, USERINFO_URL, Value, WHOAMI_URL, authorize_url, base64url, clip, encode,
        error_of, gated, parse_query, parse_tokens, plausible,
    };
    use crate::fetch::Fetch;
    use photocraft_aigen::{Request, Response, Transport};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Now, in Unix seconds.
    pub fn unix_now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    /// PKCE's challenge for `verifier`: base64url(sha256(verifier)).
    pub fn challenge(verifier: &str) -> String {
        base64url(&Sha256::digest(verifier.as_bytes()))
    }

    /// Send `request`, which must be for Hugging Face: a token never goes anywhere else.
    pub fn send(transport: &dyn Transport, request: &Request) -> Result<Response> {
        if !request.url.starts_with(HOST) {
            return Err(Error::Config(format!("a Hugging Face request may only go to {HOST}, not `{}`", clip(&request.url, 120))));
        }
        transport.send(request).map_err(|e| Error::Network(format!("Hugging Face could not be reached ({e}). Check the connection, then try again.")))
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
        pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&").into_bytes()
    }

    fn post_form(url: &str, pairs: &[(&str, &str)]) -> Request {
        Request {
            method: photocraft_aigen::Method::Post,
            url: url.to_string(),
            headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into()), ("Accept".into(), "application/json".into())],
            body: form(pairs),
            timeout_seconds: 30,
        }
    }

    /// The code the browser brought back, exchanged for the tokens.
    pub fn exchange(transport: &dyn Transport, client_id: &str, code: &str, verifier: &str, redirect_uri: &str, now: u64) -> Result<Credentials> {
        let request = post_form(
            TOKEN_URL,
            &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect_uri), ("client_id", client_id), ("code_verifier", verifier)],
        );
        let r = send(transport, &request)?;
        if !r.ok() {
            let (error, description) = error_of(&r.body);
            return Err(Error::Access(format!(
                "Hugging Face did not complete the sign-in ({} {error}{}). Press Sign in with Hugging Face to try again.",
                r.status,
                if description.is_empty() { String::new() } else { format!(": {description}") }
            )));
        }
        parse_tokens(&r.body, now, "")
    }

    /// Fresh credentials for ones about to expire. A refresh Hugging Face refuses means signing
    /// in again (`Error::Access`).
    pub fn refresh(transport: &dyn Transport, client_id: &str, credentials: &Credentials, now: u64) -> Result<Credentials> {
        if credentials.refresh.is_empty() {
            return Err(Error::Access("Your Hugging Face sign-in has expired. Sign in again.".into()));
        }
        let request = post_form(TOKEN_URL, &[("grant_type", "refresh_token"), ("refresh_token", &credentials.refresh), ("client_id", client_id)]);
        let r = send(transport, &request)?;
        if matches!(r.status, 400 | 401) {
            return Err(Error::Access("Your Hugging Face sign-in has expired. Sign in again.".into()));
        }
        if !r.ok() {
            return Err(crate::fetch::status_error(r.status, TOKEN_URL));
        }
        let mut fresh = parse_tokens(&r.body, now, &credentials.refresh)?;
        fresh.user.clone_from(&credentials.user);
        Ok(fresh)
    }

    fn bearer(url: &str, token: &str) -> Request {
        Request::get(url).header("Authorization", &format!("Bearer {}", token.trim())).header("Accept", "application/json").timeout(20)
    }

    /// The username of a sign-in (`/oauth/userinfo`).
    pub fn userinfo(transport: &dyn Transport, access: &str) -> Result<String> {
        let r = send(transport, &bearer(USERINFO_URL, access))?;
        if matches!(r.status, 401 | 403) {
            return Err(Error::Access("Hugging Face did not accept the sign-in. Sign in again.".into()));
        }
        if !r.ok() {
            return Err(crate::fetch::status_error(r.status, USERINFO_URL));
        }
        let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        let name = ["preferred_username", "name", "sub"].iter().find_map(|k| v.get(*k).and_then(Value::as_str)).unwrap_or("");
        Ok(clip(name.trim(), 100))
    }

    /// The username of an access token pasted by hand (`/api/whoami-v2`); a token Hugging Face
    /// doesn't take is `Error::Access`.
    pub fn whoami(transport: &dyn Transport, token: &str) -> Result<String> {
        let r = send(transport, &bearer(WHOAMI_URL, token))?;
        if matches!(r.status, 401 | 403) {
            return Err(Error::Access(
                "Hugging Face did not accept that access token. Copy it again from your Hugging Face settings (Access Tokens): a read token is enough.".into(),
            ));
        }
        if !r.ok() {
            return Err(crate::fetch::status_error(r.status, WHOAMI_URL));
        }
        let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        Ok(clip(v.get("name").and_then(Value::as_str).unwrap_or("").trim(), 100))
    }

    /// Where each gated model among `models` stands with `token` (none: all need a sign-in, and
    /// nothing is asked). One `HEAD` per model, of its first gated file.
    pub fn access(manifest: &Manifest, models: &[String], token: Option<&str>, fetch: &dyn Fetch) -> Result<Vec<ModelAccess>> {
        let token = token.map(str::trim).filter(|t| !t.is_empty());
        let mut out = Vec::new();
        for model in gated(manifest, models) {
            let mut entry = ModelAccess {
                id: model.id.clone(),
                name: model.name.clone(),
                page: model.page.clone(),
                licence: model.licence.name.clone(),
                access: Access::SignIn,
                detail: String::new(),
            };
            let file = model.files.iter().filter_map(|k| manifest.files.get(k)).find(|f| f.gated);
            match (token, file) {
                (_, None) => entry.access = Access::Ready,
                (None, Some(_)) => {}
                (Some(token), Some(file)) => {
                    let head = fetch.head(&file.url, Some(token))?;
                    let said = head.error_message.clone().unwrap_or_default();
                    match head.status {
                        200..=399 => entry.access = Access::Ready,
                        401 => entry.detail = "Hugging Face did not accept the sign-in. Sign in again.".into(),
                        403 => {
                            entry.access = Access::Licence;
                            // Not the licence (a token without the permission, say): its own words.
                            if head.error_code.as_deref().is_some_and(|c| c != "GatedRepo") {
                                entry.detail = said;
                            }
                        }
                        404 | 410 => {
                            entry.access = Access::Unavailable;
                            entry.detail = format!(
                                "{} can't be downloaded by this version of the app: its maker moved its file on Hugging Face. Check for a newer version of the app, or leave the model out.",
                                model.name
                            );
                        }
                        status => return Err(crate::fetch::status_error(status, &file.url)),
                    }
                }
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// A token for Hugging Face already on this PC: `HF_TOKEN`, else the file `huggingface_hub`
    /// signs in to (`HF_TOKEN_PATH`, else `%HF_HOME%\token`, else `~/.cache/huggingface/token`).
    /// For the user to choose to use, never used unasked.
    pub fn found_on_this_pc() -> Option<String> {
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if let Some(t) = env("HF_TOKEN").filter(|t| plausible(t)) {
            return Some(t);
        }
        let cache = || env("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| env("USERPROFILE").or_else(|| env("HOME")).map(|h| PathBuf::from(h).join(".cache")));
        let file = env("HF_TOKEN_PATH")
            .map(PathBuf::from)
            .or_else(|| env("HF_HOME").map(|h| PathBuf::from(h).join("token")))
            .or_else(|| cache().map(|c| c.join("huggingface").join("token")))?;
        if std::fs::metadata(&file).ok()?.len() > 4096 {
            return None;
        }
        std::fs::read_to_string(file).ok().map(|t| t.trim().to_string()).filter(|t| plausible(t))
    }

    /// The page the browser shows after coming back.
    fn page(ok: bool, product: &str, message: &str) -> String {
        let title = if ok { "Signed in to Hugging Face" } else { "Not signed in" };
        let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:16px system-ui,sans-serif;background:#2b2b2b;color:#eee;display:grid;place-items:center;height:100vh;margin:0}}main{{max-width:30em;text-align:center}}h1{{font-size:1.4em}}</style></head><body><main><h1>{title}</h1><p>{}</p><p>You can close this tab and go back to {}.</p></main></body></html>",
            escape(message),
            escape(product)
        )
    }

    fn respond(mut stream: TcpStream, status: &str, body: &str) {
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes())).and_then(|()| stream.flush());
    }

    /// One request to the listener: `Ok(Some(code))` when it is the browser coming back with
    /// one; `Ok(None)` for anything else (a favicon), which gets a 404 while the wait goes on. A
    /// `state` that is not the one sent, or an error from Hugging Face, ends the sign-in.
    fn answer(mut stream: TcpStream, state: &str, product: &str) -> Result<Option<String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        while buf.len() < 16 * 1024 && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            }
        }
        let head = String::from_utf8_lossy(&buf);
        let mut words = head.lines().next().unwrap_or("").split(' ');
        let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if method != "GET" || path != "/callback" {
            respond(stream, "404 Not Found", "");
            return Ok(None);
        }
        let pairs = parse_query(query);
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        if get("state") != Some(state) {
            respond(stream, "400 Bad Request", &page(false, product, "This answer does not belong to the sign-in that was started, so it was refused."));
            return Err(Error::Access(
                "The answer from the browser did not match this sign-in, so it was refused. Press Sign in with Hugging Face to try again.".into(),
            ));
        }
        if let Some(error) = get("error") {
            let cancelled = error == "access_denied";
            respond(stream, "200 OK", &page(false, product, if cancelled { "The sign-in was cancelled." } else { "Hugging Face did not sign you in." }));
            let why = get("error_description").map(|d| clip(d, 200)).unwrap_or_default();
            return Err(Error::Access(if cancelled {
                "The sign-in was cancelled in the browser.".into()
            } else {
                format!("Hugging Face did not sign you in ({}{}).", clip(error, 60), if why.is_empty() { String::new() } else { format!(": {why}") })
            }));
        }
        match get("code").map(str::trim).filter(|c| !c.is_empty() && c.len() <= 2048) {
            Some(code) => {
                respond(stream, "200 OK", &page(true, product, "The sign-in worked."));
                Ok(Some(code.to_string()))
            }
            None => {
                respond(stream, "400 Bad Request", &page(false, product, "The answer had no sign-in code."));
                Err(Error::Access("The browser came back without a sign-in code. Press Sign in with Hugging Face to try again.".into()))
            }
        }
    }

    /// Wait on `listener` for the browser to come back with a code for `state`, until `deadline`
    /// or a cancel.
    pub fn wait_for_code(listener: &TcpListener, state: &str, product: &str, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<String> {
        listener.set_nonblocking(true).map_err(|e| Error::Access(format!("The sign-in could not wait for the browser: {e}")))?;
        loop {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::Access("The sign-in timed out: nothing came back from the browser. Press Sign in with Hugging Face to try again.".into()));
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(code) = answer(stream, state, product)? {
                        return Ok(code);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Access(format!("The sign-in could not wait for the browser: {e}"))),
            }
        }
    }

    /// Sign in with Hugging Face, under way: the address to open in the browser, and the worker
    /// that waits for it to come back and exchanges the code.
    pub struct SignIn {
        url: String,
        cancel: Arc<AtomicBool>,
        rx: mpsc::Receiver<Result<Credentials>>,
    }

    impl SignIn {
        /// Listen on `127.0.0.1:<a free port>` and make the address to sign in at. `product`
        /// names the app on the page the browser comes back to.
        pub fn start(transport: Arc<dyn Transport>, client_id: &str, product: &str, timeout: Duration) -> Result<SignIn> {
            let client_id = client_id.trim().to_string();
            if client_id.is_empty() {
                return Err(Error::Config("Sign in with Hugging Face is not set up in this build. Use an access token instead.".into()));
            }
            let listener =
                TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| Error::Access(format!("The sign-in could not wait for the browser: {e}")))?;
            let port = listener.local_addr().map_err(|e| Error::Access(format!("The sign-in could not wait for the browser: {e}")))?.port();
            let redirect = format!("http://127.0.0.1:{port}/callback");
            let mut random = [0u8; 48];
            getrandom::fill(&mut random).map_err(|e| Error::Access(format!("The sign-in could not be started: {e}")))?;
            let verifier = base64url(random.get(..32).unwrap_or(&[]));
            let state = base64url(random.get(32..).unwrap_or(&[]));
            let url = authorize_url(&client_id, &redirect, &state, &challenge(&verifier));
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let (flag, product) = (cancel.clone(), product.to_string());
            let work = move || {
                let deadline = Instant::now() + timeout;
                let result = (|| {
                    let code = wait_for_code(&listener, &state, &product, deadline, &|| flag.load(Ordering::Relaxed))?;
                    let mut credentials = exchange(transport.as_ref(), &client_id, &code, &verifier, &redirect, unix_now())?;
                    // Not knowing the name is no reason to fail a sign-in that worked.
                    credentials.user = userinfo(transport.as_ref(), &credentials.access).unwrap_or_default();
                    Ok(credentials)
                })();
                let _ = tx.send(result);
            };
            std::thread::Builder::new().name("hf-sign-in".into()).spawn(work).map_err(|e| Error::Access(format!("The sign-in could not be started: {e}")))?;
            Ok(SignIn { url, cancel, rx })
        }

        /// Where to send the browser.
        pub fn url(&self) -> &str {
            &self.url
        }

        pub fn cancel(&self) {
            self.cancel.store(true, Ordering::Relaxed);
        }

        /// How it ended, once it has: given once.
        pub fn take(&self) -> Option<Result<Credentials>> {
            match self.rx.try_recv() {
                Ok(r) => Some(r),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(Error::Access("The sign-in stopped unexpectedly. Press Sign in with Hugging Face to try again.".into())))
                }
            }
        }
    }

    impl Drop for SignIn {
        fn drop(&mut self) {
            // Nobody is waiting for it any more: the listener closes.
            self.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
