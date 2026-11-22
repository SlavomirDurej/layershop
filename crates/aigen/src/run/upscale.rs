//! RGB super-resolution on ComfyUI. The graph and cancellation policy are specific to upscale.

use super::{CONNECTING, Control, Listening, MAX_POLL_FAILURES, POLL, random_seed, with_listener};
use crate::{Error, MAX_RESPONSE_BYTES, Part, Request, Result, Settings, Transport, comfy, live::Live};
use serde_json::{Value, json};
use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU8},
    },
    time::{Duration, Instant},
};

const OUTPUT: &str = "5";
const NATIVE_PREVIEW: &str = "6";
const SCALE_NODE: &str = "4";

#[derive(Clone, Copy, Debug)]
pub struct UpscalePlan {
    output: (u32, u32),
    native: (u32, u32),
}

impl UpscalePlan {
    /// Profile only known model files. Actual native dimensions are also checked against the
    /// server output, including when a size node would otherwise conceal a substituted model.
    pub fn new(name: &str, scale: u32, input: (u32, u32)) -> Result<Self> {
        if !matches!(scale, 2 | 4) {
            return Err(Error::Config("Upscale scale must be 2 or 4".into()));
        }
        let native_scale = match name.rsplit(['/', '\\']).next() {
            Some("RealESRGAN_x2plus.pth") => 2,
            Some("RealESRGAN_x4plus.pth" | "RealESRGAN_x4plus.safetensors") => 4,
            _ => return Err(Error::Config(format!("Unsupported or unverified native scale for upscale model {name:?}"))),
        };
        if scale > native_scale {
            return Err(Error::Config(format!("Upscale model {name:?} is native {native_scale}x and cannot produce {scale}x")));
        }
        let size = |k| {
            input
                .0
                .checked_mul(k)
                .zip(input.1.checked_mul(k))
                .filter(|&(w, h)| w > 0 && h > 0)
                .ok_or_else(|| Error::Config("Upscale image dimensions are empty or overflow".into()))
        };
        let plan = Self { output: size(scale)?, native: size(native_scale)? };
        // Headroom for an incompressible RGB PNG, including filtering and container overhead.
        // Also caps the server's native work when a 4x model is used to request 2x output.
        if u64::from(plan.native.0).saturating_mul(u64::from(plan.native.1)) > MAX_RESPONSE_BYTES / 4 {
            return Err(Error::Config("The native upscale image exceeds the PNG transfer budget".into()));
        }
        Ok(plan)
    }

    pub fn output(self) -> (u32, u32) {
        self.output
    }
    pub fn native(self) -> (u32, u32) {
        self.native
    }
    fn resize(self) -> bool {
        self.native != self.output
    }
}

fn workflow(plan: UpscalePlan, model: &str, image: &str) -> Value {
    let mut graph = json!({
        "1": {"class_type":"LoadImage", "inputs":{"image":image}},
        "2": {"class_type":"UpscaleModelLoader", "inputs":{"model_name":model}},
        "3": {"class_type":"ImageUpscaleWithModel", "inputs":{"image":["1",0], "upscale_model":["2",0]}},
        (OUTPUT): {"class_type":"PreviewImage", "inputs":{"images":[if plan.resize() { SCALE_NODE } else { "3" }, 0]}}
    });
    if plan.resize() {
        graph[SCALE_NODE] = json!({"class_type":"ImageScale", "inputs":{
            "image":["3",0], "upscale_method":"lanczos", "width":plan.output.0, "height":plan.output.1, "crop":"disabled"
        }});
        graph[NATIVE_PREVIEW] = json!({"class_type":"PreviewImage", "inputs":{"images":["3",0]}});
    }
    graph
}

fn request(transport: &dyn Transport, req: &Request) -> Result<crate::Response> {
    transport.send(req).map_err(|e| Error::Network(format!("ComfyUI connection or response limit: {e}")))
}

fn check_cancel(control: &Control<'_>) -> Result<()> {
    if (control.cancelled)() { Err(Error::Cancelled) } else { Ok(()) }
}

fn node_inputs(transport: &dyn Transport, base: &str, class: &str, fields: &[&str], control: &Control<'_>) -> Result<Value> {
    check_cancel(control)?;
    let response = request(transport, &Request::get(format!("{base}/object_info/{class}")).timeout(5))?;
    if response.status == 404 {
        return Err(Error::Config(format!("ComfyUI is missing the required node {class}")));
    }
    if !response.ok() {
        return Err(Error::Server(format!("ComfyUI could not describe node {class} (HTTP {})", response.status)));
    }
    let schema = comfy::json_of(&response.body, "node capabilities")?;
    let inputs = schema.get(class).and_then(|node| node.pointer("/input/required"));
    match inputs {
        Some(required) if fields.iter().all(|field| required.get(field).is_some()) => Ok(required.clone()),
        _ => Err(Error::Config(format!("ComfyUI node {class} is missing or has incompatible inputs ({})", fields.join(", ")))),
    }
}

fn model_inventory(transport: &dyn Transport, base: &str, control: &Control<'_>) -> Result<(Vec<String>, bool)> {
    let loader = node_inputs(transport, base, "UpscaleModelLoader", &["model_name"], control)?;
    let menu = &loader["model_name"];
    let options = menu
        .get(0)
        .and_then(Value::as_array)
        .or_else(|| menu.pointer("/1/options").and_then(Value::as_array))
        .ok_or_else(|| Error::BadResponse("ComfyUI's UpscaleModelLoader did not list model choices".into()))?;
    check_cancel(control)?;
    let inventory = request(transport, &Request::get(format!("{base}/models/upscale_models")).timeout(5))?;
    let listed = match inventory.status {
        404 => None, // Older servers can still list choices through the loader itself.
        200 => Some(comfy::parse_models(&inventory.body)?),
        code => return Err(Error::Server(format!("ComfyUI could not list upscale models (HTTP {code})"))),
    };
    let empty = options.is_empty() || listed.as_ref().is_some_and(Vec::is_empty);
    let models = options
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| listed.as_ref().is_none_or(|files| files.iter().any(|file| file == name)))
        .map(str::to_owned)
        .collect();
    Ok((models, empty))
}

fn common_nodes(transport: &dyn Transport, base: &str, control: &Control<'_>) -> Result<()> {
    node_inputs(transport, base, "LoadImage", &["image"], control)?;
    node_inputs(transport, base, "ImageUpscaleWithModel", &["image", "upscale_model"], control)?;
    node_inputs(transport, base, "PreviewImage", &["images"], control)?;
    Ok(())
}

fn resize_node(transport: &dyn Transport, base: &str, control: &Control<'_>) -> Result<Value> {
    node_inputs(transport, base, "ImageScale", &["image", "upscale_method", "width", "height", "crop"], control)
}

/// Discover supported profiles and scale combinations on the configured server.
pub fn upscale_models(transport: &dyn Transport, url: &str, control: &Control<'_>) -> Result<Vec<(String, Vec<u32>)>> {
    let base = comfy::base_url(url)?;
    let (mut models, _) = model_inventory(transport, &base, control)?;
    models.retain(|model| UpscalePlan::new(model, 2, (1, 1)).is_ok());
    if models.is_empty() {
        return Ok(Vec::new());
    }
    common_nodes(transport, &base, control)?;
    let can_resize = if models.iter().any(|model| UpscalePlan::new(model, 4, (1, 1)).is_ok()) {
        match resize_node(transport, &base, control) {
            Ok(_) => true,
            Err(Error::Config(_)) => false,
            Err(e) => return Err(e),
        }
    } else {
        false
    };
    models.sort();
    models.dedup();
    Ok(models
        .into_iter()
        .map(|model| {
            let scales = match UpscalePlan::new(&model, 4, (1, 1)).is_ok() {
                true if can_resize => vec![2, 4],
                true => vec![4],
                false => vec![2],
            };
            (model, scales)
        })
        .collect())
}

fn capabilities(transport: &dyn Transport, base: &str, plan: UpscalePlan, model: &str, control: &Control<'_>) -> Result<()> {
    let (models, empty) = model_inventory(transport, base, control)?;
    if empty {
        return Err(Error::Config("No upscale models are available on this ComfyUI server".into()));
    }
    if !models.iter().any(|name| name == model) {
        return Err(Error::Config(format!("Upscale model {model:?} is missing from the ComfyUI server's available models")));
    }
    common_nodes(transport, base, control)?;
    if plan.resize() {
        resize_node(transport, base, control)?;
    }
    Ok(())
}

/// Upscale one encoded RGB PNG using the configured ComfyUI. Returns the exact output node's PNG.
pub fn run_upscale(transport: &dyn Transport, settings: &Settings, plan: UpscalePlan, model: &str, png: &[u8], control: &Control<'_>) -> Result<Vec<u8>> {
    let base = comfy::base_url(&settings.comfy_url)?;
    capabilities(transport, &base, plan, model, control)?;
    check_cancel(control)?;
    if png.is_empty() || png.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(Error::Config("Upscale input exceeds the PNG transfer budget or is empty".into()));
    }
    let name = format!("layershop-upscale-{:012x}{:012x}", random_seed(), random_seed());
    let filename = format!("{name}.png");
    (control.progress)(0.08, "Uploading RGB pixels to ComfyUI…");
    let parts = [Part::png("image", &filename, png), Part::text("type", "input")];
    let uploaded = request(transport, &Request::post_multipart(format!("{base}/upload/image"), &parts).timeout(120))?;
    if !uploaded.ok() {
        return Err(Error::Server(format!("ComfyUI rejected the upscale upload (HTTP {})", uploaded.status)));
    }
    let image = comfy::parse_uploaded(uploaded.status, &uploaded.body)?;
    let graph = workflow(plan, model, &image);
    let listening = Listening { live: Mutex::new(Live::new(&graph)), state: AtomicU8::new(CONNECTING), stop: AtomicBool::new(false) };
    let ws = base.strip_prefix("http://").map(|host| format!("ws://{host}/ws?clientId={name}"));
    with_listener(transport, ws.as_deref(), &listening, || {
        let render = Render { transport, base: &base, control, plan, model };
        render.run(&name, &graph, &listening, settings.timeout_seconds.clamp(1, 3600))
    })
}

struct Render<'a, 'b> {
    transport: &'a dyn Transport,
    base: &'a str,
    control: &'a Control<'b>,
    plan: UpscalePlan,
    model: &'a str,
}

impl Render<'_, '_> {
    fn run(&self, client: &str, graph: &Value, listening: &Listening, timeout: u32) -> Result<Vec<u8>> {
        check_cancel(self.control)?;
        let response = request(self.transport, &Request::post_json(format!("{}/prompt", self.base), comfy::queue_body(graph, client)).timeout(15))
            .map_err(|e| Error::Network(format!("ComfyUI may already have accepted the upscale; it was not resubmitted: {e}")))?;
        let id = comfy::parse_queued(response.status, &response.body)?;
        if !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
            return Err(Error::BadResponse("ComfyUI returned an invalid upscale prompt ID".into()));
        }
        if !response.ok() {
            return Err(Error::Server(format!("ComfyUI rejected the upscale workflow (HTTP {})", response.status)));
        }
        let result = self.follow(&id, listening, timeout);
        if result.is_err() {
            self.cancel_own(&id);
        }
        result
    }

    fn follow(&self, id: &str, listening: &Listening, timeout: u32) -> Result<Vec<u8>> {
        let start = Instant::now();
        let deadline = Duration::from_secs(u64::from(timeout));
        let history_url = format!("{}/history/{}", self.base, comfy::encode(id));
        let mut failures = 0;
        while let Some(remaining) = deadline.checked_sub(start.elapsed()) {
            check_cancel(self.control)?;
            let heard = listening.live.lock().unwrap_or_else(PoisonError::into_inner).words(self.model);
            if let Some((fraction, words)) = heard {
                (self.control.progress)(0.1 + 0.72 * fraction, &words);
            } else {
                (self.control.progress)(0.12, "Waiting for ComfyUI to process the queued upscale…");
            }
            let response = self.transport.send(&Request::get(&history_url).timeout(remaining.as_secs().clamp(1, 5)));
            check_cancel(self.control)?;
            match response {
                Ok(response) if response.ok() => {
                    failures = 0;
                    if let Some((output, native)) = read_outputs(&response.body, id, self.plan.resize())? {
                        if let Some(native) = native {
                            self.download(&native, self.plan.native, true)?;
                        }
                        return self.download(&output, self.plan.output, false);
                    }
                }
                _ => {
                    failures += 1;
                    if failures > MAX_POLL_FAILURES {
                        return Err(Error::Network("Lost the ComfyUI connection while following the upscale job".into()));
                    }
                }
            }
            (self.control.sleep)(POLL.min(remaining));
        }
        Err(Error::Server(format!("ComfyUI upscale did not finish within {timeout} seconds")))
    }

    fn download(&self, image: &comfy::ImageRef, size: (u32, u32), header_only: bool) -> Result<Vec<u8>> {
        check_cancel(self.control)?;
        (self.control.progress)(0.84, "Downloading ComfyUI upscale…");
        let mut req = Request::get(comfy::view_url(self.base, image)).timeout(120);
        if header_only {
            req = req.header("Range", "bytes=0-23");
        }
        let response = request(self.transport, &req)?;
        check_cancel(self.control)?;
        if !response.ok() || (!header_only && response.status != 200) || response.body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(Error::BadResponse(format!("ComfyUI could not return a complete upscale PNG (HTTP {})", response.status)));
        }
        let actual = crate::comfy::png_dimensions(&response.body).ok_or_else(|| Error::BadResponse("The upscale output is not a PNG".into()))?;
        if actual != size {
            return Err(Error::BadResponse(format!("Upscale model returned {actual:?}, expected {size:?}; the model may have an unsupported native scale")));
        }
        Ok(response.body)
    }

    fn cancel_own(&self, id: &str) {
        // Only the per-job route promises an atomic ID check for a running prompt. Older
        // servers can safely remove this ID from the pending queue, but may finish it once running.
        let targeted = self.transport.send(&Request::post_json(format!("{}/api/jobs/{}/cancel", self.base, comfy::encode(id)), b"{}".to_vec()).timeout(3));
        if !targeted.is_ok_and(|response| response.ok()) {
            let _ = self.transport.send(&Request::post_json(format!("{}/queue", self.base), json!({"delete":[id]}).to_string().into_bytes()).timeout(3));
        }
    }
}

fn read_outputs(body: &[u8], id: &str, resized: bool) -> Result<Option<(comfy::ImageRef, Option<comfy::ImageRef>)>> {
    let history = comfy::json_of(body, "upscale history")?;
    let Some(entry) = history.get(id) else { return Ok(None) };
    let status = entry.get("status");
    if let Some(error) = comfy::node_failed(status) {
        if error.to_string().contains("UpscaleModelLoader") {
            return Err(Error::Server(format!("ComfyUI could not load the upscale model: {error}")));
        }
        return Err(error);
    }
    if !status.and_then(|s| s.get("completed")).is_some_and(|value| value == true) {
        return Ok(None);
    }
    let find = |node: &str| -> Result<comfy::ImageRef> {
        let invalid = || Error::BadResponse(format!("Upscale output node {node} must return exactly one temporary PNG"));
        let images = entry.get("outputs").and_then(|outputs| outputs.get(node)).and_then(|out| out.get("images")).and_then(Value::as_array);
        let Some([image]) = images.map(Vec::as_slice) else { return Err(invalid()) };
        let filename = image.get("filename").and_then(Value::as_str).filter(|s| s.len() <= 256 && s.ends_with(".png")).ok_or_else(invalid)?;
        let subfolder = image.get("subfolder").and_then(Value::as_str).filter(|s| s.len() <= 256).ok_or_else(invalid)?;
        if image.get("type").and_then(Value::as_str) != Some("temp") {
            return Err(invalid());
        }
        Ok(comfy::ImageRef { filename: filename.into(), subfolder: subfolder.into(), kind: "temp".into() })
    };
    Ok(Some((find(OUTPUT)?, resized.then(|| find(NATIVE_PREVIEW)).transpose()?)))
}

#[cfg(test)]
mod tests;
