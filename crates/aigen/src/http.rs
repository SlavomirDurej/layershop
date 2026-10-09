//! The one network seam: a blocking request/response [`Transport`], and its desktop
//! implementation.

use crate::Result;

/// Largest response body read (a 4K PNG as base64 inside JSON is about 40 MB).
pub const MAX_RESPONSE_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One HTTP request. `Debug` leaves the headers out: they carry API keys.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Sent as the body of a `Post` (JSON).
    pub body: Vec<u8>,
    pub timeout_seconds: u64,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request").field("method", &self.method).field("url", &self.url).field("body_len", &self.body.len()).finish_non_exhaustive()
    }
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: Method::Get, url: url.into(), headers: Vec::new(), body: Vec::new(), timeout_seconds: 30 }
    }

    /// A `POST` of a JSON body.
    pub fn post_json(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self { method: Method::Post, url: url.into(), headers: vec![("Content-Type".into(), "application/json".into())], body, timeout_seconds: 30 }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds.clamp(1, 3600);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends a request and waits for the whole answer. An HTTP error status is a [`Response`], not
/// an `Err`: the body says what went wrong. `Err` is for a request that got no answer.
pub trait Transport: Send + Sync {
    fn send(&self, request: &Request) -> Result<Response>;
}

/// [`Transport`] over `ureq` (rustls). One agent, so connections to ComfyUI are reused while a
/// generation is polled.
#[cfg(not(target_arch = "wasm32"))]
pub struct UreqTransport {
    agent: ureq::Agent,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl UreqTransport {
    pub fn new() -> Self {
        // Error statuses come back as responses so their JSON message can be shown.
        let config = ureq::Agent::config_builder().http_status_as_error(false).user_agent("PhotoCraft").build();
        Self { agent: config.into() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Transport for UreqTransport {
    fn send(&self, request: &Request) -> Result<Response> {
        use crate::Error;
        let timeout = Some(std::time::Duration::from_secs(request.timeout_seconds.clamp(1, 3600)));
        let net = |e: ureq::Error| Error::Network(e.to_string());
        let mut response = match request.method {
            Method::Get => {
                let mut b = self.agent.get(&request.url).config().timeout_global(timeout).build();
                for (k, v) in &request.headers {
                    b = b.header(k.as_str(), v.as_str());
                }
                b.call().map_err(net)?
            }
            Method::Post => {
                let mut b = self.agent.post(&request.url).config().timeout_global(timeout).build();
                for (k, v) in &request.headers {
                    b = b.header(k.as_str(), v.as_str());
                }
                b.send(&request.body[..]).map_err(net)?
            }
        };
        let status = response.status().as_u16();
        let body = response.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_vec().map_err(net)?;
        Ok(Response { status, body })
    }
}
