//! Cloud image models: requests for the OpenAI, Gemini, xAI, Meta and Black Forest Labs image
//! APIs, and their answers.

use base64::Engine;
use serde_json::{Value, json};

use crate::comfy::clip;
use crate::http::{Part, Request};
use crate::model::{Model, Provider};
use crate::run::{Job, Source};
use crate::size::{nearest_ratio, render_size};
use crate::{Error, Result};

const OPENAI_URL: &str = "https://api.openai.com/v1/images/generations";
const OPENAI_EDIT_URL: &str = "https://api.openai.com/v1/images/edits";
const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const XAI_URL: &str = "https://api.x.ai/v1/images/generations";
const XAI_EDIT_URL: &str = "https://api.x.ai/v1/images/edits";
const META_URL: &str = "https://api.meta.ai/v1/images/generations";
const META_EDIT_URL: &str = "https://api.meta.ai/v1/images/edits";
const BFL_URL: &str = "https://api.bfl.ai/v1";

/// Aspect ratios xAI's image models accept (it also takes 19.5:9, a phone screen's).
const XAI_RATIOS: [(u32, u32); 13] = [(1, 1), (16, 9), (9, 16), (4, 3), (3, 4), (3, 2), (2, 3), (2, 1), (1, 2), (20, 9), (9, 20), (21, 9), (5, 2)];
/// Aspect ratios FLUX 3 Image accepts.
const BFL_RATIOS: [(u32, u32); 15] =
    [(21, 9), (2, 1), (16, 9), (3, 2), (7, 5), (4, 3), (5, 4), (1, 1), (4, 5), (3, 4), (5, 7), (2, 3), (9, 16), (1, 2), (9, 21)];

/// Aspect ratios the Gemini image models accept.
const GEMINI_RATIOS: [(u32, u32); 10] = [(1, 1), (2, 3), (3, 2), (3, 4), (4, 3), (4, 5), (5, 4), (9, 16), (16, 9), (21, 9)];

fn api_model(model: &Model) -> Result<&str> {
    let id = model.api_model.trim();
    // The id becomes part of a URL (Gemini): keep it to what model ids are made of.
    if id.is_empty() || id.len() > 100 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':')) {
        return Err(Error::Config(format!("{}: `{id}` is not a model id (Preferences › Generative AI)", model.name)));
    }
    Ok(id)
}

fn key_of<'a>(model: &Model, key: &'a str) -> Result<&'a str> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(|c| c.is_control()) {
        return Err(Error::Config(format!("{} needs an API key: add it in Preferences › Generative AI", model.provider.label())));
    }
    Ok(key)
}

/// The `size` asked of an OpenAI model for a `width` × `height` selection.
pub fn openai_size(model: &Model, width: u32, height: u32) -> (u32, u32) {
    let id = model.api_model.trim();
    if id.starts_with("dall-e") {
        return nearest_ratio(width, height, &[(1024, 1024), (1792, 1024), (1024, 1792)]).unwrap_or((1024, 1024));
    }
    if id.starts_with("gpt-image-1") {
        return nearest_ratio(width, height, &[(1024, 1024), (1536, 1024), (1024, 1536)]).unwrap_or((1024, 1024));
    }
    // GPT Image 2 and later take any size with edges in multiples of 16, an aspect ratio up to
    // 3:1 and between about 0.66 and 8.3 megapixels.
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let aspect = (w / h).clamp(1.0 / 3.0, 3.0);
    let (w, h) = ((1000.0 * aspect) as u32, 1000);
    let mp = if model.megapixels.is_finite() && model.megapixels > 0.0 { model.megapixels } else { f64::from(width.max(1)) * f64::from(height.max(1)) / 1e6 };
    let (rw, rh) = render_size(w, h, mp.clamp(0.75, 8.0), 16);
    (rw.min(3840), rh.min(3840))
}

/// The request that renders `job` with an OpenAI image model.
pub fn openai_request(model: &Model, job: &Job, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    let (w, h) = openai_size(model, job.width, job.height);
    let mut body = json!({"model": id, "prompt": job.prompt, "n": 1, "size": format!("{w}x{h}")});
    if id.starts_with("dall-e") {
        body["response_format"] = json!("b64_json");
    } else {
        body["output_format"] = json!("png");
        let quality = model.quality.trim();
        if matches!(quality, "low" | "medium" | "high" | "xhigh" | "max") {
            body["quality"] = json!(quality);
        }
        if job.alpha && model.alpha {
            body["background"] = json!("transparent");
        }
    }
    Ok(Request::post_json(OPENAI_URL, body.to_string().into_bytes()).header("Authorization", &format!("Bearer {key}")).timeout(timeout_seconds))
}

/// The request that repaints the masked part of `source` with an OpenAI image model: the image
/// edit endpoint, which takes the image and a mask that is transparent where to paint.
pub fn openai_edit_request(model: &Model, job: &Job, source: &Source, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    if !id.starts_with("gpt-image") {
        return Err(Error::Config(format!("{}: `{id}` can't inpaint; OpenAI's GPT Image models can", model.name)));
    }
    let size = format!("{}x{}", source.width, source.height);
    let mut parts =
        vec![Part::text("model", id), Part::text("prompt", &job.prompt), Part::text("n", "1"), Part::text("size", &size), Part::text("output_format", "png")];
    // An edit with references sends several pictures: the one to edit first.
    if source.references.is_empty() {
        parts.push(Part::png("image", "image.png", &source.image));
    } else {
        parts.push(Part::png("image[]", "image.png", &source.image));
        parts.extend(source.references.iter().map(|bytes| Part::png("image[]", "reference.png", bytes)));
    }
    if source.edit && job.alpha && model.alpha {
        parts.push(Part::text("background", "transparent"));
    }
    // Without a mask the whole picture is the model's to redraw (image to image).
    if !source.mask.is_empty() {
        parts.push(Part::png("mask", "mask.png", &source.mask));
    }
    let quality = model.quality.trim();
    if matches!(quality, "low" | "medium" | "high" | "xhigh" | "max") {
        parts.push(Part::text("quality", quality));
    }
    Ok(Request::post_multipart(OPENAI_EDIT_URL, &parts).header("Authorization", &format!("Bearer {key}")).timeout(timeout_seconds))
}

/// The supported aspect ratio closest to a `width` × `height` selection.
pub fn gemini_ratio(width: u32, height: u32) -> (u32, u32) {
    nearest_ratio(width, height, &GEMINI_RATIOS).unwrap_or((1, 1))
}

/// The request that renders `job` with a Gemini image model.
pub fn gemini_request(model: &Model, job: &Job, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    let (rw, rh) = gemini_ratio(job.width, job.height);
    // Given an image to edit, the answer takes that image's shape: asking for one of the fixed
    // ratios would move everything in it.
    let mut image = if job.source.is_some() { json!({}) } else { json!({"aspectRatio": format!("{rw}:{rh}")}) };
    let size = model.image_size.trim();
    // The 2.5 models render at one size and reject the field.
    if matches!(size, "1K" | "2K" | "4K") && !id.starts_with("gemini-2") {
        image["imageSize"] = json!(size);
    }
    let parts = match &job.source {
        // Gemini has no mask input. It is shown the image and where to paint (the mask, as a
        // second picture) and told to leave the rest; the layer's mask then hides what it
        // changed outside anyway.
        // Image to image: the picture, and what to make of it.
        Some(source) if source.mask.is_empty() => {
            let picture = |bytes: &Vec<u8>| json!({"inlineData": {"mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(bytes)}});
            // An edit's references follow the picture, in the order the instruction numbers them.
            let mut parts = vec![json!({"text": job.prompt})];
            parts.extend(std::iter::once(&source.image).chain(&source.references).map(picture));
            Value::Array(parts)
        }
        Some(source) => {
            let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
            json!([
                {"text": masked_instruction(&job.prompt)},
                {"inlineData": {"mimeType": "image/png", "data": b64(&source.image)}},
                {"inlineData": {"mimeType": "image/png", "data": b64(&source.mask)}},
            ])
        }
        None => json!([{"text": job.prompt}]),
    };
    let body = json!({
        "contents": [{"parts": parts}],
        "generationConfig": {"responseModalities": ["IMAGE"], "imageConfig": image},
    });
    Ok(Request::post_json(format!("{GEMINI_URL}/{id}:generateContent"), body.to_string().into_bytes()).header("x-goog-api-key", key).timeout(timeout_seconds))
}

/// What a model without a mask input is told when it is given one as a second picture (Gemini,
/// xAI, Meta, FLUX 3): the layer's mask then hides what it changed outside anyway.
fn masked_instruction(prompt: &str) -> String {
    format!(
        "Edit the first image. Change only the region that is transparent in the second image (a mask of the same size), and keep everything else exactly as it is, at the same position and scale. In that region: {prompt}"
    )
}

/// The pictures a request with a source sends, in order, and its prompt: the picture and its
/// mask with [`masked_instruction`], or the picture and its references with the prompt as typed.
fn pictures<'a>(job: &'a Job, source: &'a Source) -> (String, Vec<&'a Vec<u8>>) {
    if source.mask.is_empty() {
        (job.prompt.clone(), std::iter::once(&source.image).chain(&source.references).collect())
    } else {
        (masked_instruction(&job.prompt), vec![&source.image, &source.mask])
    }
}

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64_of(bytes))
}

/// The supported aspect ratio closest to a `width` × `height` selection (xAI).
pub fn xai_ratio(width: u32, height: u32) -> (u32, u32) {
    nearest_ratio(width, height, &XAI_RATIOS).unwrap_or((1, 1))
}

/// xAI renders at `1k` or `2k`.
fn xai_resolution(model: &Model) -> &'static str {
    if model.image_size.trim().eq_ignore_ascii_case("1K") { "1k" } else { "2k" }
}

/// The request that renders `job` with an xAI image model (Grok Imagine). Its images API is
/// OpenAI's in shape, but an edit is JSON with the pictures as data URLs, not a form.
pub fn xai_request(model: &Model, job: &Job, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    let mut body = json!({"model": id, "prompt": job.prompt, "n": 1, "response_format": "b64_json", "resolution": xai_resolution(model)});
    let quality = model.quality.trim();
    if matches!(quality, "low" | "medium") {
        body["quality"] = json!(quality);
    }
    let url = match &job.source {
        Some(source) => {
            let (prompt, sent) = pictures(job, source);
            body["prompt"] = json!(prompt);
            // The answer takes the first picture's shape. One picture goes in `image`, several
            // in `images` (the API takes one field or the other).
            let item = |bytes: &&Vec<u8>| json!({"url": data_url(bytes), "type": "image_url"});
            match sent.as_slice() {
                [one] => body["image"] = item(one),
                many => body["images"] = Value::Array(many.iter().map(item).collect()),
            }
            XAI_EDIT_URL
        }
        None => {
            let (rw, rh) = xai_ratio(job.width, job.height);
            body["aspect_ratio"] = json!(format!("{rw}:{rh}"));
            XAI_URL
        }
    };
    Ok(Request::post_json(url, body.to_string().into_bytes()).header("Authorization", &format!("Bearer {key}")).timeout(timeout_seconds))
}

/// The `size` asked of Muse Image for a `width` × `height` selection. It sets the shape of the
/// picture, not its pixels, and a shape beyond 16:1 is refused.
pub fn meta_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let aspect = (w / h).clamp(1.0 / 16.0, 16.0);
    let (w, h) = if aspect >= 1.0 { ((1024.0 * aspect).round() as u32, 1024) } else { (1024, (1024.0 / aspect).round() as u32) };
    render_size(w, h, 1.0, 8)
}

/// The request that renders `job` with a Meta image model (Muse Image): OpenAI's images API in
/// shape, with an edit sent as JSON (`images`, as data URLs). It takes no mask and no seed.
pub fn meta_request(model: &Model, job: &Job, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    let mut body = json!({"model": id, "prompt": job.prompt, "n": 1, "response_format": "b64_json", "output_format": "png"});
    let url = match &job.source {
        Some(source) => {
            let (prompt, sent) = pictures(job, source);
            body["prompt"] = json!(prompt);
            body["images"] = Value::Array(sent.iter().map(|bytes| json!({"image_url": data_url(bytes)})).collect());
            META_EDIT_URL
        }
        None => {
            let (w, h) = meta_size(job.width, job.height);
            body["size"] = json!(format!("{w}x{h}"));
            META_URL
        }
    };
    Ok(Request::post_json(url, body.to_string().into_bytes()).header("Authorization", &format!("Bearer {key}")).timeout(timeout_seconds))
}

/// The supported aspect ratio closest to a `width` × `height` selection (FLUX 3 Image).
pub fn bfl_ratio(width: u32, height: u32) -> (u32, u32) {
    nearest_ratio(width, height, &BFL_RATIOS).unwrap_or((1, 1))
}

/// The request that submits `job` to Black Forest Labs. The model id is the endpoint. FLUX 3
/// takes an aspect ratio, a resolution tier and its pictures in `images`; the endpoints before
/// it (FLUX.2, Kontext, FLUX 1.1) take a size in pixels and their pictures in `input_image`,
/// `input_image_2`, …. Neither takes a mask.
pub fn bfl_request(model: &Model, job: &Job, key: &str, timeout_seconds: u64) -> Result<Request> {
    let id = api_model(model)?;
    let key = key_of(model, key)?;
    let (prompt, sent) = match &job.source {
        Some(source) => pictures(job, source),
        None => (job.prompt.clone(), Vec::new()),
    };
    let mut body = json!({"prompt": prompt});
    if id.starts_with("flux-3") {
        let size = model.image_size.trim().to_ascii_lowercase();
        body["resolution"] = json!(if matches!(size.as_str(), "2k" | "4k") { size.as_str() } else { "1k" });
        if sent.is_empty() {
            let (rw, rh) = bfl_ratio(job.width, job.height);
            body["aspect_ratio"] = json!(format!("{rw}:{rh}"));
        } else {
            // `auto`, the default, keeps the first picture's framing.
            body["images"] = Value::Array(sent.iter().map(|bytes| json!(base64_of(bytes))).collect());
        }
    } else {
        let (w, h) = match &job.source {
            Some(source) => (source.width, source.height),
            None => render_size(job.width, job.height, 1.0, 16),
        };
        body["width"] = json!(w);
        body["height"] = json!(h);
        for (i, bytes) in sent.iter().enumerate() {
            let name = if i == 0 { "input_image".to_string() } else { format!("input_image_{}", i + 1) };
            body[name] = json!(base64_of(bytes));
        }
    }
    Ok(Request::post_json(format!("{BFL_URL}/{id}"), body.to_string().into_bytes()).header("x-key", key).timeout(timeout_seconds.min(120)))
}

/// `url` is an https address at Black Forest Labs: the only place the key may be sent to (the
/// address to poll comes from the answer to the submitted job).
fn bfl_host(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.contains('@') && {
        let host = authority.split(':').next().unwrap_or("");
        host == "bfl.ai" || host.ends_with(".bfl.ai")
    }
}

/// The request that asks whether a submitted job is done.
pub fn bfl_poll_request(polling_url: &str, key: &str) -> Result<Request> {
    if !bfl_host(polling_url) {
        return Err(Error::BadResponse("Black Forest Labs answered with an address to poll that is not its own".into()));
    }
    Ok(Request::get(polling_url).header("x-key", key.trim()).timeout(30))
}

/// What Black Forest Labs says went wrong: `detail` as text, or as a list of validation errors.
fn bfl_error(status: u16, v: &Value) -> Error {
    let detail = v.get("detail").or_else(|| v.get("error")).or_else(|| v.get("message"));
    let text = match detail {
        Some(Value::String(s)) => Some(clip(s, 400)),
        Some(Value::Array(items)) => {
            let said: Vec<&str> = items.iter().take(4).filter_map(|i| i.get("msg").and_then(Value::as_str)).collect();
            (!said.is_empty()).then(|| clip(&said.join("; "), 400))
        }
        Some(Value::Object(o)) => o.get("message").and_then(Value::as_str).map(|s| clip(s, 400)),
        _ => None,
    };
    let hint = match status {
        401 | 403 => " (check the API key in Preferences › Generative AI)",
        402 => " (the account is out of credits)",
        404 => " (check the model id in Preferences › Generative AI: it is the name of the endpoint, such as flux-3-image)",
        429 => " (too many requests at once)",
        _ => "",
    };
    Error::Server(match text {
        Some(t) => format!("Black Forest Labs: {t}{hint}"),
        None => format!("Black Forest Labs answered HTTP {status}{hint}"),
    })
}

/// A submitted job: where to ask for it, and what it cost in US dollars when the answer says.
pub fn parse_bfl_submitted(status: u16, body: &[u8]) -> Result<(String, Option<f64>)> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let url = v.get("polling_url").and_then(Value::as_str).filter(|u| !u.is_empty() && u.len() <= 2048);
    match url {
        Some(url) if (200..300).contains(&status) => {
            let cost = v.get("cost").and_then(Value::as_f64).and_then(|credits| crate::pricing::reported(credits, crate::pricing::BFL_CREDIT_USD));
            Ok((url.to_string(), cost))
        }
        _ if (200..300).contains(&status) => Err(Error::BadResponse("Black Forest Labs did not say where to ask for the image".into())),
        _ => Err(bfl_error(status, &v)),
    }
}

/// Where a submitted job stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BflJob {
    /// Still working, with its status (`Pending`, `Reasoning`, `Generating`).
    Working(String),
    /// Done: the address the image is downloaded from (signed, for an hour).
    Ready(String),
}

/// What a poll of a submitted job says. A job that was refused or failed is an `Err`.
pub fn parse_bfl_job(status: u16, body: &[u8]) -> Result<BflJob> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    // A failed job can answer with an error status and its state in the body: read that first.
    let Some(state) = v.get("status").and_then(Value::as_str) else { return Err(bfl_error(status, &v)) };
    match state {
        "Ready" => {
            let sample = v.get("result").and_then(|r| r.get("sample")).and_then(Value::as_str).filter(|u| u.starts_with("https://") && u.len() <= 4096);
            sample.map(|u| BflJob::Ready(u.to_string())).ok_or_else(|| Error::BadResponse("Black Forest Labs returned no image".into()))
        }
        "Request Moderated" => Err(Error::Server(
            "Black Forest Labs refused the prompt or a picture sent with it (moderated). Sending it again unchanged will be refused again.".into(),
        )),
        "Content Moderated" => Err(Error::Server("Black Forest Labs withheld the image it generated (moderated). Change the prompt and try again.".into())),
        "Error" | "Failed" => {
            let why = v.get("details").or_else(|| v.get("result")).filter(|d| !d.is_null()).map(|d| format!(": {}", clip(&d.to_string(), 300)));
            Err(Error::Server(format!("Black Forest Labs could not generate the image{}", why.unwrap_or_default())))
        }
        "Task not found" => Err(Error::Server("Black Forest Labs lost track of the request. Generate again.".into())),
        other => Ok(BflJob::Working(clip(other, 40))),
    }
}

fn decode(b64: &str, who: &str) -> Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim().as_bytes())
        .map_err(|_| Error::BadResponse(format!("{who} returned an image that could not be decoded")))?;
    if bytes.is_empty() {
        return Err(Error::BadResponse(format!("{who} returned an empty image")));
    }
    Ok(bytes)
}

/// The provider's error message, with a hint for the statuses users can fix.
fn api_error(provider: Provider, status: u16, v: &Value) -> Error {
    let who = provider.label();
    let raw = v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str()));
    // Google's free tier has no quota at all for the image models ("free_tier… limit: 0"): the
    // key works, but its project has no billing. Waiting, as the message suggests, never helps.
    if status == 429 && provider == Provider::Gemini && raw.is_some_and(|m| m.contains("free_tier")) {
        return Error::Server(format!(
            "{who}: this API key's Google Cloud project is on the free tier, which can't generate images. Set up billing for that project (Google AI Studio › API keys), or use a key from a project that has it. Credit on another project doesn't count."
        ));
    }
    let msg = raw.map(|m| clip(m, 400));
    let hint = match status {
        401 | 403 => " (check the API key in Preferences › Generative AI)",
        404 => " (check the model id in Preferences › Generative AI)",
        429 => " (rate limit or quota reached)",
        _ => "",
    };
    Error::Server(match msg {
        Some(m) => format!("{who}: {m}{hint}"),
        None => format!("{who} answered HTTP {status}{hint}"),
    })
}

fn json_of(provider: Provider, status: u16, body: &[u8]) -> Result<Value> {
    let v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) if (200..300).contains(&status) => return Err(Error::BadResponse(format!("{}'s answer was not JSON", provider.label()))),
        Err(_) => Value::Null,
    };
    if !(200..300).contains(&status) || v.get("error").is_some() {
        return Err(api_error(provider, status, &v));
    }
    Ok(v)
}

/// The image bytes in an OpenAI image answer.
pub fn parse_openai(status: u16, body: &[u8]) -> Result<Vec<u8>> {
    parse_openai_with_usage(status, body).map(|(bytes, _)| bytes)
}

/// [`parse_openai`], with the answer's `usage` object (the tokens billed) when it has one.
pub fn parse_openai_with_usage(status: u16, body: &[u8]) -> Result<(Vec<u8>, Option<Value>)> {
    parse_images(Provider::Openai, status, body)
}

/// The image bytes in an answer shaped like OpenAI's images API (`data[0].b64_json`), which
/// xAI's and Meta's are too, and its `usage` object when it has one.
pub fn parse_images(provider: Provider, status: u16, body: &[u8]) -> Result<(Vec<u8>, Option<Value>)> {
    let who = provider.label();
    let v = json_of(provider, status, body)?;
    let b64 = v
        .get("data")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .and_then(|d| d.get("b64_json"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::BadResponse(format!("{who} returned no image")))?;
    Ok((decode(b64, who)?, v.get("usage").filter(|u| u.is_object()).cloned()))
}

/// What an xAI request cost in US dollars, from the `usage` of its answer.
pub fn xai_cost(usage: &Value) -> Option<f64> {
    let ticks = usage.get("cost_in_usd_ticks").and_then(Value::as_f64)?;
    crate::pricing::reported(ticks, 1.0 / crate::pricing::XAI_TICKS_PER_USD)
}

/// The image bytes in a Gemini `generateContent` answer.
pub fn parse_gemini(status: u16, body: &[u8]) -> Result<Vec<u8>> {
    let v = json_of(Provider::Gemini, status, body)?;
    let candidate = v.get("candidates").and_then(Value::as_array).and_then(|c| c.first());
    let parts = candidate.and_then(|c| c.get("content")).and_then(|c| c.get("parts")).and_then(Value::as_array);
    let data = parts.into_iter().flatten().find_map(|p| p.get("inlineData").or_else(|| p.get("inline_data"))?.get("data")?.as_str());
    if let Some(b64) = data {
        return decode(b64, "Gemini");
    }
    // No image: a safety block, or the model answered in words.
    let reason = v
        .get("promptFeedback")
        .and_then(|f| f.get("blockReason"))
        .or_else(|| candidate.and_then(|c| c.get("finishReason")))
        .and_then(Value::as_str)
        .map(|r| format!(" ({})", clip(r, 80)));
    let said = parts.into_iter().flatten().find_map(|p| p.get("text")?.as_str()).map(|t| format!(": {}", clip(t, 200)));
    Err(Error::Server(format!("Gemini returned no image{}{}", reason.unwrap_or_default(), said.unwrap_or_default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(w: u32, h: u32, alpha: bool) -> Job {
        Job { prompt: "a \"red\" frog".into(), width: w, height: h, seed: 1, alpha, source: None }
    }

    fn body(r: &Request) -> Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn openai_requests() {
        let m = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        let r = openai_request(&m, &job(1920, 1080, true), " sk-test ", 120).unwrap();
        assert_eq!(r.url, OPENAI_URL);
        assert!(r.headers.contains(&("Authorization".into(), "Bearer sk-test".into())));
        assert_eq!(r.timeout_seconds, 120);
        let b = body(&r);
        assert_eq!(b["model"], "gpt-image-2");
        assert_eq!(b["prompt"], "a \"red\" frog");
        assert_eq!(b["output_format"], "png");
        // Asked for alpha, but this model has none: the field would be an API error.
        assert!(b.get("background").is_none());
        assert!(b.get("quality").is_none());
        let (w, h) = b["size"].as_str().unwrap().split_once('x').map(|(w, h)| (w.parse::<u32>().unwrap(), h.parse::<u32>().unwrap())).unwrap();
        assert_eq!((w % 16, h % 16), (0, 0));
        assert!(w > h && w * h >= 655_360, "{w}x{h}");
        // The key never shows up in a debug print of the request.
        assert!(!format!("{r:?}").contains("sk-test"));

        let mut m = Model::cloud(Provider::Openai, "GPT Image 1.5", "gpt-image-1.5", true);
        m.quality = "high".into();
        let b = body(&openai_request(&m, &job(600, 900, true), "k", 60).unwrap());
        assert_eq!(b["size"], "1024x1536");
        assert_eq!(b["background"], "transparent");
        assert_eq!(b["quality"], "high");
        assert!(body(&openai_request(&m, &job(600, 900, false), "k", 60).unwrap()).get("background").is_none());
    }

    #[test]
    fn gpt_image_2_5_takes_any_size_transparency_and_its_quality_levels() {
        let presets = Model::cloud_presets();
        let mut m = presets.iter().find(|m| m.api_model == "gpt-image-2.5-sunburst").unwrap().clone();
        assert!(presets.iter().any(|m| m.api_model == "gpt-image-2.5-flare"));
        m.quality = "xhigh".into();
        let b = body(&openai_request(&m, &job(1600, 900, true), "k", 60).unwrap());
        assert_eq!(b["model"], "gpt-image-2.5-sunburst");
        assert_eq!(b["background"], "transparent");
        assert_eq!(b["quality"], "xhigh");
        // Not one of the three fixed sizes of the GPT Image 1 family: the selection's own shape.
        let (w, h) = openai_size(&m, 1600, 900);
        assert!((f64::from(w) / f64::from(h) - 16.0 / 9.0).abs() < 0.05 && w % 16 == 0 && h % 16 == 0, "{w}x{h}");
        m.quality = "ultra".into();
        assert!(body(&openai_request(&m, &job(64, 64, false), "k", 60).unwrap()).get("quality").is_none());
    }

    #[test]
    fn openai_sizes_stay_within_the_api_limits() {
        let mut m = Model::cloud(Provider::Openai, "x", "gpt-image-2", false);
        for mp in [0.0, 0.01, 1.0, 4.0, 100.0, f64::NAN] {
            m.megapixels = mp;
            for (w, h) in [(1, 1), (16, 16), (5000, 100), (100, 5000), (8000, 8000), (u32::MAX, 1), (0, 0)] {
                let (rw, rh) = openai_size(&m, w, h);
                let ratio = f64::from(rw.max(rh)) / f64::from(rw.min(rh));
                assert!(rw <= 3840 && rh <= 3840 && rw % 16 == 0 && rh % 16 == 0 && ratio <= 3.1, "{w}x{h}@{mp} -> {rw}x{rh}");
                assert!(u64::from(rw) * u64::from(rh) >= 655_360, "{w}x{h}@{mp} -> {rw}x{rh}");
            }
        }
    }

    #[test]
    fn gemini_requests() {
        let m = Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false);
        let r = gemini_request(&m, &job(1920, 1080, false), "g-key", 90).unwrap();
        assert_eq!(r.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-3-pro-image-preview:generateContent");
        assert!(r.headers.contains(&("x-goog-api-key".into(), "g-key".into())));
        assert!(!r.url.contains("g-key"));
        let b = body(&r);
        assert_eq!(b["contents"][0]["parts"][0]["text"], "a \"red\" frog");
        assert_eq!(b["generationConfig"]["imageConfig"], json!({"aspectRatio": "16:9", "imageSize": "1K"}));
        let old = Model::cloud(Provider::Gemini, "Nano Banana", "gemini-2.5-flash-image", false);
        assert_eq!(body(&gemini_request(&old, &job(500, 500, false), "k", 90).unwrap())["generationConfig"]["imageConfig"], json!({"aspectRatio": "1:1"}));
    }

    fn source() -> Source {
        Source { image: b"IMAGEPNG".to_vec(), mask: b"MASKPNG".to_vec(), width: 1024, height: 1536, strength: 0.5, edit: false, references: Vec::new(), shown: Vec::new() }
    }

    #[test]
    fn openai_inpaints_through_the_edit_endpoint_with_the_image_and_its_mask() {
        let mut m = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        m.quality = "high".into();
        let r = openai_edit_request(&m, &job(600, 900, false), &source(), " sk-test ", 120).unwrap();
        assert_eq!(r.url, "https://api.openai.com/v1/images/edits");
        assert!(r.headers.contains(&("Authorization".into(), "Bearer sk-test".into())));
        assert!(r.headers.iter().any(|(k, v)| k == "Content-Type" && v.starts_with("multipart/form-data; boundary=")));
        assert_eq!(r.timeout_seconds, 120);
        let body = String::from_utf8_lossy(&r.body).into_owned();
        for want in [
            "name=\"model\"\r\n\r\ngpt-image-2\r\n",
            "name=\"prompt\"\r\n\r\na \"red\" frog\r\n",
            "name=\"size\"\r\n\r\n1024x1536\r\n",
            "name=\"quality\"\r\n\r\nhigh\r\n",
            "name=\"image\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\nIMAGEPNG\r\n",
            "name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\nMASKPNG\r\n",
        ] {
            assert!(body.contains(want), "{want:?} missing in {body}");
        }
        assert!(!format!("{r:?}").contains("sk-test"));
        // A model without the endpoint, a missing key and a bad id are config errors.
        let dalle = Model::cloud(Provider::Openai, "DALL-E 3", "dall-e-3", false);
        assert!(matches!(openai_edit_request(&dalle, &job(9, 9, false), &source(), "k", 9), Err(Error::Config(s)) if s.contains("can't inpaint")));
        assert!(matches!(openai_edit_request(&m, &job(9, 9, false), &source(), "", 9), Err(Error::Config(s)) if s.contains("API key")));
    }

    #[test]
    fn gemini_inpaints_from_the_image_and_the_mask_in_the_images_own_shape() {
        let m = Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false);
        let j = Job { source: Some(source()), ..job(1920, 1080, false) };
        let b = body(&gemini_request(&m, &j, "g-key", 90).unwrap());
        let parts = b["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        let text = parts[0]["text"].as_str().unwrap();
        assert!(text.contains("a \"red\" frog") && text.contains("transparent in the second image"), "{text}");
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(parts[1]["inlineData"], json!({"mimeType": "image/png", "data": b64(b"IMAGEPNG")}));
        assert_eq!(parts[2]["inlineData"]["data"], b64(b"MASKPNG"));
        // No fixed aspect ratio: the answer follows the image it was given.
        assert_eq!(b["generationConfig"]["imageConfig"], json!({"imageSize": "1K"}));
    }

    #[test]
    fn image_to_image_sends_the_picture_without_a_mask() {
        let unmasked = Source { mask: Vec::new(), ..source() };
        let m = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        let r = openai_edit_request(&m, &job(600, 900, false), &unmasked, "k", 60).unwrap();
        let form = String::from_utf8_lossy(&r.body).into_owned();
        assert!(form.contains("name=\"image\"; filename=\"image.png\"") && !form.contains("name=\"mask\""), "{form}");
        let m = Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false);
        let b = body(&gemini_request(&m, &Job { source: Some(unmasked), ..job(1920, 1080, false) }, "k", 90).unwrap());
        let parts = b["contents"][0]["parts"].as_array().unwrap();
        assert_eq!((parts.len(), parts[0]["text"].as_str()), (2, Some("a \"red\" frog")));
        assert!(parts[1]["inlineData"]["data"].is_string());
    }

    #[test]
    fn bad_keys_and_model_ids_are_config_errors() {
        let m = Model::cloud(Provider::Openai, "x", "gpt-image-2", false);
        for key in ["", "   ", "a\nb"] {
            assert!(matches!(openai_request(&m, &job(9, 9, false), key, 9), Err(Error::Config(s)) if s.contains("API key")));
        }
        for id in ["", "a/b", "x?key=1", "../models", "a b"] {
            let m = Model::cloud(Provider::Gemini, "x", id, false);
            assert!(matches!(gemini_request(&m, &job(9, 9, false), "k", 9), Err(Error::Config(_))), "{id}");
        }
    }

    #[test]
    fn openai_answers() {
        assert_eq!(parse_openai(200, br#"{"data":[{"b64_json":"aGVsbG8="}]}"#).unwrap(), b"hello");
        let e = parse_openai(401, br#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#).unwrap_err().to_string();
        assert!(e.contains("Incorrect API key") && e.contains("check the API key"), "{e}");
        assert!(parse_openai(500, b"<html>oops").unwrap_err().to_string().contains("HTTP 500"));
        assert!(parse_openai(429, b"").unwrap_err().to_string().contains("quota"));
        // Gemini's free tier has no image quota: say that billing is missing, not "retry later".
        let free = br#"{"error":{"code":429,"message":"You exceeded your current quota.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3-pro-image\nPlease retry in 14h53m34s.","status":"RESOURCE_EXHAUSTED"}}"#;
        let e = parse_gemini(429, free).unwrap_err().to_string();
        assert!(e.contains("free tier") && e.contains("billing") && !e.contains("retry"), "{e}");
        let paid = br#"{"error":{"code":429,"message":"Quota exceeded for metric: generate_content_paid_tier_requests"}}"#;
        assert!(parse_gemini(429, paid).unwrap_err().to_string().contains("rate limit or quota reached"));
        for bad in [
            &b"{}"[..],
            br#"{"data":[]}"#,
            br#"{"data":[{"url":"https://x"}]}"#,
            br#"{"data":[{"b64_json":"***"}]}"#,
            br#"{"data":[{"b64_json":""}]}"#,
            b"nope",
            b"[]",
        ] {
            assert!(matches!(parse_openai(200, bad), Err(Error::BadResponse(_))), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn gemini_answers() {
        let ok = br#"{"candidates":[{"content":{"parts":[{"text":"Here you go"},{"inlineData":{"mimeType":"image/png","data":"aGVsbG8="}}]}}]}"#;
        assert_eq!(parse_gemini(200, ok).unwrap(), b"hello");
        let blocked = br#"{"candidates":[{"finishReason":"IMAGE_SAFETY","content":{"parts":[{"text":"I can't make that"}]}}]}"#;
        let e = parse_gemini(200, blocked).unwrap_err().to_string();
        assert!(e.contains("IMAGE_SAFETY") && e.contains("I can't make that"), "{e}");
        let e = parse_gemini(400, br#"{"error":{"code":400,"message":"API key not valid","status":"INVALID_ARGUMENT"}}"#).unwrap_err().to_string();
        assert!(e.contains("API key not valid"), "{e}");
        assert!(parse_gemini(404, b"{}").unwrap_err().to_string().contains("model id"));
        for bad in [&b"{}"[..], br#"{"candidates":[]}"#, br#"{"candidates":[{"content":{"parts":7}}]}"#, b"[]", b"null"] {
            assert!(parse_gemini(200, bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    fn edit_job(mask: &[u8], references: &[&[u8]]) -> Job {
        let source = Source {
            image: b"PICTURE".to_vec(),
            mask: mask.to_vec(),
            width: 1024,
            height: 768,
            strength: 1.0,
            edit: mask.is_empty(),
            references: references.iter().map(|r| r.to_vec()).collect(),
            shown: Vec::new(),
        };
        Job { source: Some(source), ..job(1024, 768, false) }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn xai_requests_are_json_with_a_ratio_and_pictures_as_data_urls() {
        let mut m = Model::cloud_presets().into_iter().find(|m| m.provider == Provider::Xai).unwrap();
        let r = xai_request(&m, &job(1920, 1080, true), " xai-key ", 90).unwrap();
        assert_eq!((r.url.as_str(), r.timeout_seconds), (XAI_URL, 90));
        assert!(r.headers.contains(&("Authorization".into(), "Bearer xai-key".into())) && !format!("{r:?}").contains("xai-key"));
        let b = body(&r);
        assert_eq!((b["model"].as_str(), b["aspect_ratio"].as_str(), b["resolution"].as_str()), (Some("grok-imagine-image-2.0"), Some("16:9"), Some("1k")));
        assert_eq!((b["response_format"].as_str(), b["n"].as_u64()), (Some("b64_json"), Some(1)));
        // `auto` is the API's default: only a chosen quality is sent. No seed, no transparency.
        assert!(b.get("quality").is_none() && b.get("seed").is_none() && b.get("background").is_none() && b.get("image").is_none());
        m.quality = "medium".into();
        m.image_size = "4K".into();
        let b = body(&xai_request(&m, &job(500, 1000, false), "k", 60).unwrap());
        assert_eq!((b["quality"].as_str(), b["resolution"].as_str(), b["aspect_ratio"].as_str()), (Some("medium"), Some("2k"), Some("1:2")));
        m.quality = "max".into();
        assert!(body(&xai_request(&m, &job(64, 64, false), "k", 60).unwrap()).get("quality").is_none());
        // An edit: the picture as a data URL in `image`, the answer in its shape.
        let r = xai_request(&m, &edit_job(b"", &[]), "k", 60).unwrap();
        let b = body(&r);
        assert_eq!(r.url, XAI_EDIT_URL);
        assert_eq!(b["image"], json!({"url": format!("data:image/png;base64,{}", b64(b"PICTURE")), "type": "image_url"}));
        assert!(b.get("images").is_none() && b.get("aspect_ratio").is_none() && b["prompt"] == "a \"red\" frog");
        // With references the pictures go in `images`, the one to edit first.
        let b = body(&xai_request(&m, &edit_job(b"", &[b"SHIRT", b"HAT"]), "k", 60).unwrap());
        let sent: Vec<&str> = b["images"].as_array().unwrap().iter().map(|i| i["url"].as_str().unwrap()).collect();
        assert_eq!(sent.len(), 3);
        assert!(sent[0].ends_with(&b64(b"PICTURE")) && sent[2].ends_with(&b64(b"HAT")) && b.get("image").is_none());
        // It has no mask input: the mask goes as a second picture, with what it means.
        let b = body(&xai_request(&m, &edit_job(b"MASK", &[b"IGNORED"]), "k", 60).unwrap());
        assert_eq!(b["images"].as_array().unwrap().len(), 2);
        assert!(b["prompt"].as_str().unwrap().starts_with("Edit the first image. Change only the region that is transparent"));
        assert!(xai_request(&m, &job(64, 64, false), "", 60).is_err());
    }

    #[test]
    fn meta_requests_give_muse_image_a_shape_and_its_pictures_as_data_urls() {
        let m = Model::cloud_presets().into_iter().find(|m| m.provider == Provider::Meta).unwrap();
        let r = meta_request(&m, &job(1920, 1080, true), "meta-key", 60).unwrap();
        assert_eq!(r.url, META_URL);
        assert!(r.headers.contains(&("Authorization".into(), "Bearer meta-key".into())));
        let b = body(&r);
        assert_eq!((b["model"].as_str(), b["output_format"].as_str(), b["response_format"].as_str()), (Some("muse-image-1.0"), Some("png"), Some("b64_json")));
        let (w, h) = b["size"].as_str().unwrap().split_once('x').map(|(w, h)| (w.parse::<f64>().unwrap(), h.parse::<f64>().unwrap())).unwrap();
        assert!((w / h - 16.0 / 9.0).abs() < 0.03, "{w}x{h}");
        // An unknown field is an HTTP 400 there: no seed, quality or background.
        assert_eq!(b.as_object().unwrap().len(), 6, "{b}");
        // A shape beyond 16:1 is refused: a long strip is asked for at the limit.
        for (w, h) in [(100_000, 10), (10, 100_000), (0, 0), (u32::MAX, 1)] {
            let (rw, rh) = meta_size(w, h);
            let aspect = f64::from(rw) / f64::from(rh);
            assert!(rw > 0 && rh > 0 && (1.0 / 16.6..=16.6).contains(&aspect), "{w}x{h} -> {rw}x{rh}");
        }
        let r = meta_request(&m, &edit_job(b"", &[b"SHIRT"]), "k", 60).unwrap();
        let b = body(&r);
        assert_eq!(r.url, META_EDIT_URL);
        assert_eq!(
            b["images"],
            json!([{"image_url": format!("data:image/png;base64,{}", b64(b"PICTURE"))}, {"image_url": format!("data:image/png;base64,{}", b64(b"SHIRT"))}])
        );
        assert!(b.get("size").is_none() && b.get("mask").is_none());
        // `mask` is an unsupported parameter: the mask is a second picture.
        let b = body(&meta_request(&m, &edit_job(b"MASK", &[]), "k", 60).unwrap());
        assert!(b["images"][1]["image_url"].as_str().unwrap().ends_with(&b64(b"MASK")) && b["prompt"].as_str().unwrap().contains("second image"));
    }

    #[test]
    fn answers_shaped_like_openais_are_read_for_every_provider_that_sends_them() {
        let ok = br#"{"data":[{"b64_json":"aGVsbG8=","mime_type":"image/jpeg"}],"usage":{"cost_in_usd_ticks":600000000}}"#;
        let (bytes, usage) = parse_images(Provider::Xai, 200, ok).unwrap();
        assert_eq!(bytes, b"hello");
        // Six cents, in ticks of a ten-thousand-millionth of a dollar.
        assert!((xai_cost(&usage.unwrap()).unwrap() - 0.06).abs() < 1e-12);
        for junk in [json!({}), json!({"cost_in_usd_ticks": "x"}), json!({"cost_in_usd_ticks": -5}), json!({"cost_in_usd_ticks": 1e300}), json!(null)] {
            assert_eq!(xai_cost(&junk), None, "{junk}");
        }
        assert_eq!(parse_images(Provider::Meta, 200, br#"{"data":[{"b64_json":"aGVsbG8="}],"background":"opaque"}"#).unwrap().0, b"hello");
        // Errors name the provider, with the hint for a bad key.
        let e = parse_images(Provider::Meta, 401, br#"{"error":{"message":"invalid key"}}"#).unwrap_err().to_string();
        assert!(e.contains("Meta: invalid key") && e.contains("check the API key"), "{e}");
        let e = parse_images(Provider::Xai, 200, br#"{"data":[{"url":"https://x"}]}"#).unwrap_err().to_string();
        assert!(e.contains("xAI returned no image"), "{e}");
        for junk in [&b""[..], b"not json", b"[]", br#"{"data":7}"#, br#"{"data":[{"b64_json":"!!"}]}"#] {
            assert!(parse_images(Provider::Xai, 200, junk).is_err());
            assert!(parse_images(Provider::Meta, 500, junk).is_err());
        }
    }

    #[test]
    fn black_forest_labs_jobs_are_submitted_to_the_models_endpoint_and_polled_at_its_own_hosts() {
        let mut m = Model::cloud_presets().into_iter().find(|m| m.provider == Provider::Bfl).unwrap();
        let r = bfl_request(&m, &job(1920, 1080, true), " bfl-key ", 600).unwrap();
        assert_eq!(r.url, "https://api.bfl.ai/v1/flux-3-image");
        assert!(r.headers.contains(&("x-key".into(), "bfl-key".into())) && !format!("{r:?}").contains("bfl-key"));
        // The schema refuses unknown fields: a prompt, a shape and a size tier, nothing else.
        assert_eq!(body(&r), json!({"prompt": "a \"red\" frog", "aspect_ratio": "16:9", "resolution": "1k"}));
        m.image_size = "4K".into();
        let b = body(&bfl_request(&m, &edit_job(b"", &[b"SHIRT"]), "k", 60).unwrap());
        assert_eq!(b, json!({"prompt": "a \"red\" frog", "resolution": "4k", "images": [b64(b"PICTURE"), b64(b"SHIRT")]}));
        let b = body(&bfl_request(&m, &edit_job(b"MASK", &[]), "k", 60).unwrap());
        assert_eq!(b["images"], json!([b64(b"PICTURE"), b64(b"MASK")]));
        // An older endpoint takes pixels and numbered pictures.
        m.api_model = "flux-2-pro".into();
        let r = bfl_request(&m, &edit_job(b"", &[b"SHIRT"]), "k", 60).unwrap();
        assert_eq!(r.url, "https://api.bfl.ai/v1/flux-2-pro");
        assert_eq!(body(&r), json!({"prompt": "a \"red\" frog", "width": 1024, "height": 768, "input_image": b64(b"PICTURE"), "input_image_2": b64(b"SHIRT")}));
        let b = body(&bfl_request(&m, &job(1920, 1080, false), "k", 60).unwrap());
        assert!(b["width"].as_u64().unwrap().is_multiple_of(16) && b["height"].as_u64().unwrap().is_multiple_of(16) && b.get("aspect_ratio").is_none());
        // The id is part of the address: nothing that could lead elsewhere.
        for id in ["", "../x", "flux 3", "a/b", "x?y=1", "x#y"] {
            m.api_model = id.into();
            assert!(bfl_request(&m, &job(64, 64, false), "k", 60).is_err(), "{id}");
        }
    }

    #[test]
    fn black_forest_labs_answers_are_read_and_polled_at_its_own_hosts_only() {
        let (url, cost) =
            parse_bfl_submitted(200, br#"{"id":"t1","polling_url":"https://api.eu2.bfl.ai/v1/get_result?id=t1","cost":4.8,"output_mp":1.0}"#).unwrap();
        assert_eq!(url, "https://api.eu2.bfl.ai/v1/get_result?id=t1");
        assert!((cost.unwrap() - 0.048).abs() < 1e-12);
        assert_eq!(parse_bfl_submitted(200, br#"{"id":"t1","polling_url":"https://api.bfl.ai/x","cost":null}"#).unwrap().1, None);
        // The key is sent to Black Forest Labs' own hosts only, whatever the answer says.
        let poll = bfl_poll_request(&url, " k ").unwrap();
        assert_eq!((poll.method, poll.url.as_str()), (crate::http::Method::Get, url.as_str()));
        assert!(poll.headers.contains(&("x-key".into(), "k".into())));
        for evil in [
            "http://api.bfl.ai/v1/get_result",
            "https://evil.example/get_result?x=.bfl.ai",
            "https://api.bfl.ai.evil.example/x",
            "https://api.bfl.ai@evil.example/x",
            "https://evilbfl.ai/x",
            "ftp://api.bfl.ai/x",
            "",
        ] {
            assert!(bfl_poll_request(evil, "k").is_err(), "{evil}");
        }
        assert!(bfl_poll_request("https://bfl.ai/x", "k").is_ok() && bfl_poll_request("https://api.us1.bfl.ai:443/x?id=1", "k").is_ok());

        // Errors say what the user can do about them.
        let e = parse_bfl_submitted(402, br#"{"detail":"Insufficient credits"}"#).unwrap_err().to_string();
        assert!(e.contains("Insufficient credits") && e.contains("out of credits"), "{e}");
        let e = parse_bfl_submitted(422, br#"{"detail":[{"loc":["body","seed"],"msg":"Extra inputs are not permitted","type":"extra_forbidden"}]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Extra inputs are not permitted"), "{e}");
        assert!(parse_bfl_submitted(404, b"<html>").unwrap_err().to_string().contains("name of the endpoint"));
        assert!(parse_bfl_submitted(200, br#"{"id":"t1"}"#).is_err() && parse_bfl_submitted(200, b"").is_err());

        // Polling: working until Ready, with the address of the image; refusals and failures are errors.
        for state in ["Pending", "Reasoning", "Generating", "Something New"] {
            assert_eq!(parse_bfl_job(200, format!(r#"{{"status":"{state}"}}"#).as_bytes()).unwrap(), BflJob::Working(state.into()));
        }
        let ready = parse_bfl_job(200, br#"{"status":"Ready","result":{"sample":"https://delivery-eu1.bfl.ai/results/a.png?sig=1","duration":4.2}}"#).unwrap();
        assert_eq!(ready, BflJob::Ready("https://delivery-eu1.bfl.ai/results/a.png?sig=1".into()));
        for bad in [r#"{"status":"Ready"}"#, r#"{"status":"Ready","result":{"sample":"http://plain/a.png"}}"#, r#"{"status":"Ready","result":{"sample":7}}"#] {
            assert!(parse_bfl_job(200, bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse_bfl_job(200, br#"{"status":"Request Moderated"}"#).unwrap_err().to_string().contains("refused the prompt"));
        assert!(parse_bfl_job(200, br#"{"status":"Content Moderated"}"#).unwrap_err().to_string().contains("withheld"));
        assert!(parse_bfl_job(503, br#"{"status":"Error","details":{"reason":"overloaded"}}"#).unwrap_err().to_string().contains("overloaded"));
        assert!(parse_bfl_job(200, br#"{"status":"Task not found"}"#).unwrap_err().to_string().contains("Generate again"));
        for junk in [&b""[..], b"null", b"[]", br#"{"status":7}"#, br#"{"detail":"Not authenticated"}"#] {
            assert!(parse_bfl_job(403, junk).is_err());
        }
    }
}
