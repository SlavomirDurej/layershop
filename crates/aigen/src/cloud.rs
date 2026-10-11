//! Cloud image models: requests for the OpenAI and Gemini image APIs, and their answers.

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

/// Aspect ratios the Gemini image models accept.
const GEMINI_RATIOS: [(u32, u32); 10] = [(1, 1), (2, 3), (3, 2), (3, 4), (4, 3), (4, 5), (5, 4), (9, 16), (16, 9), (21, 9)];

fn api_model(model: &Model) -> Result<&str> {
    let id = model.api_model.trim();
    // The id becomes part of a URL (Gemini): keep it to what model ids are made of.
    if id.is_empty() || id.len() > 100 || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':')) {
        return Err(Error::Config(format!("{}: `{id}` is not a model id (Preferences › Integrations)", model.name)));
    }
    Ok(id)
}

fn key_of<'a>(model: &Model, key: &'a str) -> Result<&'a str> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(|c| c.is_control()) {
        return Err(Error::Config(format!("{} needs an API key: add it in Preferences › Integrations", model.provider.label())));
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
                {"text": format!(
                    "Edit the first image. Change only the region that is transparent in the second image (a mask of the same size), and keep everything else exactly as it is, at the same position and scale. In that region: {}",
                    job.prompt
                )},
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
    let msg = v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str())).map(|m| clip(m, 400));
    let hint = match status {
        401 | 403 => " (check the API key in Preferences › Integrations)",
        404 => " (check the model id in Preferences › Integrations)",
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
    let v = json_of(Provider::Openai, status, body)?;
    let b64 = v
        .get("data")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .and_then(|d| d.get("b64_json"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::BadResponse("OpenAI returned no image".into()))?;
    Ok((decode(b64, "OpenAI")?, v.get("usage").filter(|u| u.is_object()).cloned()))
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
        Source { image: b"IMAGEPNG".to_vec(), mask: b"MASKPNG".to_vec(), width: 1024, height: 1536, strength: 0.5, edit: false, references: Vec::new() }
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
}
