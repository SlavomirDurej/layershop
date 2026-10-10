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
    /// Sent as the body of a `Post` (JSON, or a multipart form).
    pub body: Vec<u8>,
    pub timeout_seconds: u64,
}

/// One field of a multipart form: text, or with `file` (file name, content type) a file.
#[derive(Clone, Copy)]
pub struct Part<'a> {
    pub name: &'a str,
    pub file: Option<(&'a str, &'a str)>,
    pub data: &'a [u8],
}

impl<'a> Part<'a> {
    pub fn text(name: &'a str, value: &'a str) -> Self {
        Self { name, file: None, data: value.as_bytes() }
    }

    pub fn png(name: &'a str, filename: &'a str, data: &'a [u8]) -> Self {
        Self { name, file: Some((filename, "image/png")), data }
    }
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

    /// A `POST` of a multipart form (`multipart/form-data`): how images are uploaded.
    pub fn post_multipart(url: impl Into<String>, parts: &[Part<'_>]) -> Self {
        // The boundary may not occur in what it separates: lengthen it until it doesn't.
        let mut boundary = String::from("----PhotoCraftForm7d9k2mQx4LrT");
        while boundary.len() < 70 && parts.iter().any(|p| p.data.windows(boundary.len()).any(|w| w == boundary.as_bytes())) {
            boundary.push('Z');
        }
        // A quote or line break in a name would end its header early.
        let clean = |s: &str| s.chars().filter(|c| !c.is_control() && *c != '"').collect::<String>();
        let mut body = Vec::with_capacity(parts.iter().map(|p| p.data.len().saturating_add(200)).sum());
        for p in parts {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"", clean(p.name)).as_bytes());
            if let Some((filename, content_type)) = p.file {
                body.extend_from_slice(format!("; filename=\"{}\"\r\nContent-Type: {}", clean(filename), clean(content_type)).as_bytes());
            }
            body.extend_from_slice(b"\r\n\r\n");
            body.extend_from_slice(p.data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let headers = vec![("Content-Type".into(), format!("multipart/form-data; boundary={boundary}"))];
        Self { method: Method::Post, url: url.into(), headers, body, timeout_seconds: 30 }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_forms_frame_each_part_and_never_split_on_their_own_data() {
        let r = Request::post_multipart("http://h/upload", &[Part::text("type", "input"), Part::png("image", "a\"b\r\n.png", b"PNG\0binary")]);
        let boundary = r.headers[0].1.split_once("boundary=").unwrap().1.to_string();
        let body = String::from_utf8_lossy(&r.body).into_owned();
        assert!(body.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"type\"\r\n\r\ninput\r\n")), "{body}");
        assert!(body.contains("name=\"image\"; filename=\"ab.png\"\r\nContent-Type: image/png\r\n\r\nPNG\0binary\r\n"), "{body}");
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
        // Data that contains the boundary gets a different one.
        let hostile = format!("x--{boundary}--y");
        let other = Request::post_multipart("http://h/upload", &[Part::text("a", &hostile)]);
        let boundary2 = other.headers[0].1.split_once("boundary=").unwrap().1.to_string();
        assert!(boundary2 != boundary && !hostile.contains(&boundary2));
        assert!(Request::post_multipart("http://h", &[]).body.ends_with(b"--\r\n"));
    }
}
