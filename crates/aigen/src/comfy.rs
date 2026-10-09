//! Local models through a ComfyUI server: the workflow for each model family, and the server's
//! queue, history and file answers.
//!
//! The workflows follow ComfyUI's own text-to-image templates for each family and use only nodes
//! that ship with ComfyUI. Anything else (LoRA stacks, prompt enhancers, custom samplers) goes
//! through [`Family::Custom`]: the user's own workflow with placeholders.

use serde_json::{Map, Value, json};

use crate::model::{Family, Model};
use crate::run::Job;
use crate::{Error, Result};

/// The folders of `models/` that hold each kind of file, as `GET /models/<folder>` names them.
pub const FOLDERS: [&str; 5] = ["diffusion_models", "text_encoders", "vae", "checkpoints", "loras"];

/// A workflow under construction, in ComfyUI's API format: `{"<id>": {class_type, inputs}}`.
#[derive(Default)]
struct Graph {
    nodes: Map<String, Value>,
}

/// An output of a node, as an input of another: `[node id, output slot]`.
type Link = Value;

impl Graph {
    fn add(&mut self, class: &str, inputs: Value) -> String {
        let id = (self.nodes.len() + 1).to_string();
        self.nodes.insert(id.clone(), json!({"class_type": class, "inputs": inputs}));
        id
    }

    /// Add a node and return its first output.
    fn node(&mut self, class: &str, inputs: Value) -> Link {
        json!([self.add(class, inputs), 0])
    }
}

fn file<'a>(name: &'a str, what: &str, model: &Model) -> Result<&'a str> {
    let name = name.trim();
    if name.is_empty() {
        let label = if model.name.is_empty() { model.family.label() } else { model.name.as_str() };
        return Err(Error::Config(format!("{label}: choose {what} in Preferences › Integrations")));
    }
    Ok(name)
}

/// The text encoder loader's `type` for families with a single encoder.
fn clip_type(family: Family) -> &'static str {
    match family {
        Family::ZImage => "lumina2",
        Family::Krea2 => "krea2",
        Family::Flux2 | Family::Ernie => "flux2",
        Family::Ideogram4 => "ideogram4",
        Family::QwenImage | Family::QwenImage21 => "qwen_image",
        _ => "stable_diffusion",
    }
}

fn latent_node(family: Family) -> &'static str {
    match family {
        Family::ZImage | Family::Flux1 | Family::HidreamI1 | Family::QwenImage => "EmptySD3LatentImage",
        Family::Flux2 | Family::Ernie | Family::Ideogram4 => "EmptyFlux2LatentImage",
        Family::HidreamO1 => "EmptyHiDreamO1LatentImage",
        _ => "EmptyLatentImage",
    }
}

/// The workflow that renders `job` with `model`, ending in a preview image (kept in ComfyUI's
/// temp folder, not its output folder).
pub fn workflow(model: &Model, job: &Job) -> Result<Value> {
    if model.family == Family::Custom {
        return custom_workflow(model, job);
    }
    let f = model.family;
    let mut g = Graph::default();
    let (mut unet, clip, vae) = if f.uses_checkpoint() {
        let id = g.add("CheckpointLoaderSimple", json!({"ckpt_name": file(&model.checkpoint, "a checkpoint file", model)?}));
        (json!([id, 0]), json!([id, 1]), json!([id, 2]))
    } else {
        let unet = g.node("UNETLoader", json!({"unet_name": file(&model.unet, "a diffusion model file", model)?, "weight_dtype": "default"}));
        let enc = "a text encoder file";
        let clip = match f.text_encoders() {
            2 => g.node(
                "DualCLIPLoader",
                json!({"clip_name1": file(&model.clip, enc, model)?, "clip_name2": file(&model.clip2, "a second text encoder file", model)?, "type": "flux", "device": "default"}),
            ),
            4 => g.node(
                "QuadrupleCLIPLoader",
                json!({
                    "clip_name1": file(&model.clip, enc, model)?,
                    "clip_name2": file(&model.clip2, "a second text encoder file", model)?,
                    "clip_name3": file(&model.clip3, "a third text encoder file", model)?,
                    "clip_name4": file(&model.clip4, "a fourth text encoder file", model)?,
                }),
            ),
            _ => g.node("CLIPLoader", json!({"clip_name": file(&model.clip, enc, model)?, "type": clip_type(f), "device": "default"})),
        };
        let vae = g.node("VAELoader", json!({"vae_name": file(&model.vae, "a VAE file", model)?}));
        (unet, clip, vae)
    };
    if !model.lora.trim().is_empty() {
        unet = g.node("LoraLoaderModelOnly", json!({"model": unet, "lora_name": model.lora.trim(), "strength_model": finite(model.lora_strength, 1.0)}));
    }
    let shift = finite(model.shift, 0.0);
    if f.has_shift() && shift > 0.0 {
        unet = match f {
            Family::HidreamI1 => g.node("ModelSamplingSD3", json!({"model": unet, "shift": shift})),
            Family::HidreamO1 => g.node("ModelNoiseScale", json!({"model": unet, "noise_scale": shift})),
            _ => g.node("ModelSamplingAuraFlow", json!({"model": unet, "shift": shift})),
        };
    }
    let cfg = finite(model.cfg, 1.0);
    let (mut positive, negative) = if f == Family::QwenImage21 {
        // Its own node encodes both prompts (and reference images, of which there are none here).
        let id = g.add(
            "TextEncodeQwenImage21",
            json!({"clip": clip, "prompt": job.prompt, "negative_prompt": model.negative_prompt, "resolution": 1024, "vae": vae}),
        );
        (json!([id, 0]), json!([id, 1]))
    } else {
        let positive = g.node("CLIPTextEncode", json!({"text": job.prompt, "clip": clip}));
        // Without classifier-free guidance the negative prompt is never evaluated: a zeroed
        // conditioning saves encoding it. Ideogram 4's negative model takes one, too.
        let negative = if cfg <= 1.0 || f == Family::Ideogram4 {
            g.node("ConditioningZeroOut", json!({"conditioning": positive}))
        } else {
            g.node("CLIPTextEncode", json!({"text": model.negative_prompt, "clip": clip}))
        };
        (positive, negative)
    };
    let guidance = finite(model.guidance, 0.0);
    if f.has_guidance() && guidance > 0.0 {
        positive = g.node("FluxGuidance", json!({"conditioning": positive, "guidance": guidance}));
    }
    let latent = g.node(latent_node(f), json!({"width": job.width, "height": job.height, "batch_size": 1}));
    let steps = model.steps.clamp(1, 1000);
    let sampler = if model.sampler.trim().is_empty() { "euler" } else { model.sampler.trim() };
    let scheduler = if model.scheduler.trim().is_empty() { "simple" } else { model.scheduler.trim() };
    let samples = match f {
        Family::Flux2 | Family::Ideogram4 => {
            let noise = g.node("RandomNoise", json!({"noise_seed": job.seed}));
            let select = g.node("KSamplerSelect", json!({"sampler_name": sampler}));
            let (sigmas, guider) = if f == Family::Flux2 {
                (
                    g.node("Flux2Scheduler", json!({"steps": steps, "width": job.width, "height": job.height})),
                    g.node("CFGGuider", json!({"model": unet, "positive": positive, "negative": negative, "cfg": cfg})),
                )
            } else {
                let sigmas = g.node("BasicScheduler", json!({"model": unet, "scheduler": scheduler, "steps": steps, "denoise": 1.0}));
                let mut inputs = json!({"model": unet, "positive": positive, "cfg": cfg, "negative": negative});
                if !model.unet2.trim().is_empty() {
                    inputs["model_negative"] = g.node("UNETLoader", json!({"unet_name": model.unet2.trim(), "weight_dtype": "default"}));
                }
                (sigmas, g.node("DualModelGuider", inputs))
            };
            g.node("SamplerCustomAdvanced", json!({"noise": noise, "guider": guider, "sampler": select, "sigmas": sigmas, "latent_image": latent}))
        }
        _ => g.node(
            "KSampler",
            json!({
                "model": unet, "seed": job.seed, "steps": steps, "cfg": cfg, "sampler_name": sampler, "scheduler": scheduler,
                "positive": positive, "negative": negative, "latent_image": latent, "denoise": 1.0,
            }),
        ),
    };
    let image = g.node("VAEDecode", json!({"samples": samples, "vae": vae}));
    g.add("PreviewImage", json!({"images": image}));
    Ok(Value::Object(g.nodes))
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

/// The user's workflow with its placeholders filled in. A string that is exactly a numeric
/// placeholder (`"%width%"`) becomes a number; placeholders inside longer text are substituted.
fn custom_workflow(model: &Model, job: &Job) -> Result<Value> {
    let text = model.workflow.trim();
    if text.is_empty() {
        return Err(Error::Config(format!("{}: paste a workflow (ComfyUI › Workflow › Export (API)) in Preferences › Integrations", model.name)));
    }
    let mut v: Value = serde_json::from_str(text).map_err(|e| Error::Config(format!("{}: the workflow is not valid JSON: {e}", model.name)))?;
    let is_api = v.as_object().is_some_and(|o| !o.is_empty() && o.values().all(|n| n.get("class_type").is_some_and(Value::is_string)));
    if !is_api {
        return Err(Error::Config(format!(
            "{}: the workflow is not in API format. Export it from ComfyUI with Workflow › Export (API); the regular Save format can't be queued",
            model.name
        )));
    }
    if !text.contains("%prompt%") {
        return Err(Error::Config(format!("{}: the workflow has no %prompt% placeholder, so the prompt would be ignored", model.name)));
    }
    let numbers: [(&str, Value); 5] = [
        ("%width%", json!(job.width)),
        ("%height%", json!(job.height)),
        ("%seed%", json!(job.seed)),
        ("%steps%", json!(model.steps.clamp(1, 1000))),
        ("%cfg%", json!(finite(model.cfg, 1.0))),
    ];
    let texts: [(&str, &str); 2] = [("%prompt%", &job.prompt), ("%negative%", &model.negative_prompt)];
    substitute(&mut v, &numbers, &texts, 0);
    Ok(v)
}

fn substitute(v: &mut Value, numbers: &[(&str, Value)], texts: &[(&str, &str)], depth: usize) {
    // Workflows nest three levels (node, inputs, link); anything deeper is not ours to walk.
    if depth > 16 {
        return;
    }
    match v {
        Value::String(s) => {
            if let Some((_, n)) = numbers.iter().find(|(k, _)| s.trim() == *k) {
                *v = n.clone();
                return;
            }
            if s.contains('%') {
                let mut out = s.clone();
                for (k, t) in texts {
                    out = out.replace(k, t);
                }
                for (k, n) in numbers {
                    out = out.replace(k, &n.to_string());
                }
                *s = out;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| substitute(x, numbers, texts, depth + 1)),
        Value::Object(o) => o.values_mut().for_each(|x| substitute(x, numbers, texts, depth + 1)),
        _ => {}
    }
}

/// The server address without a trailing slash; an error unless it is an http(s) URL.
pub fn base_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() < 9 {
        return Err(Error::Config(format!("the ComfyUI address `{url}` is not an http:// URL (Preferences › Integrations)")));
    }
    Ok(url.to_string())
}

/// The body of `POST /prompt`.
pub fn queue_body(workflow: &Value, client_id: &str) -> Vec<u8> {
    json!({"prompt": workflow, "client_id": client_id}).to_string().into_bytes()
}

fn json_of(body: &[u8], what: &str) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Error::BadResponse(format!("ComfyUI's {what} answer was not JSON; is the address a ComfyUI server?")))
}

/// ComfyUI's validation errors for a queued workflow, as one readable line.
fn queue_error(v: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(nodes) = v.get("node_errors").and_then(Value::as_object) {
        for n in nodes.values().take(4) {
            let class = n.get("class_type").and_then(Value::as_str).unwrap_or("node");
            for e in n.get("errors").and_then(Value::as_array).into_iter().flatten().take(2) {
                let msg = e.get("message").and_then(Value::as_str).unwrap_or("invalid input");
                let details = e.get("details").and_then(Value::as_str).unwrap_or("");
                parts.push(clip(&format!("{class}: {msg} {details}"), 300));
            }
        }
    }
    if parts.is_empty() {
        let e = v.get("error")?;
        parts.push(clip(e.get("message").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("the workflow was rejected"), 300));
    }
    Some(parts.join("; "))
}

/// At most `max` characters of `s`, on one line.
pub(crate) fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).chain(['…']).collect() }
}

/// The prompt id from the answer to `POST /prompt`.
pub fn parse_queued(status: u16, body: &[u8]) -> Result<String> {
    let v = json_of(body, "queue")?;
    if let Some(id) = v.get("prompt_id").and_then(Value::as_str).filter(|id| !id.is_empty() && id.len() <= 128) {
        return Ok(id.to_string());
    }
    match queue_error(&v) {
        Some(e) => Err(Error::Server(format!("ComfyUI rejected the workflow: {e}"))),
        None => Err(Error::BadResponse(format!("ComfyUI did not queue the workflow (HTTP {status})"))),
    }
}

/// A rendered image on the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

/// What `GET /history/<id>` says about a queued workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum History {
    /// Still queued or rendering.
    Pending,
    Done(ImageRef),
}

/// Read `GET /history/<id>`: pending while the id is absent, the first image once it finished,
/// an error when a node failed.
pub fn parse_history(body: &[u8], id: &str) -> Result<History> {
    let v = json_of(body, "history")?;
    let Some(entry) = v.get(id) else { return Ok(History::Pending) };
    let status = entry.get("status");
    if status.and_then(|s| s.get("status_str")).and_then(Value::as_str) == Some("error") {
        let detail = status
            .and_then(|s| s.get("messages"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.get(0).and_then(Value::as_str) == Some("execution_error"))
            .find_map(|m| {
                let d = m.get(1)?;
                let node = d.get("node_type").and_then(Value::as_str).unwrap_or("a node");
                Some(format!("{node}: {}", d.get("exception_message").and_then(Value::as_str).unwrap_or("failed")))
            })
            .unwrap_or_else(|| "the workflow failed".into());
        return Err(Error::Server(format!("ComfyUI: {}", clip(&detail, 400))));
    }
    let image = entry.get("outputs").and_then(Value::as_object).into_iter().flat_map(|o| o.values()).find_map(|out| {
        let img = out.get("images")?.as_array()?.first()?;
        Some(ImageRef {
            filename: img.get("filename")?.as_str()?.to_string(),
            subfolder: img.get("subfolder").and_then(Value::as_str).unwrap_or("").to_string(),
            kind: img.get("type").and_then(Value::as_str).unwrap_or("output").to_string(),
        })
    });
    match image {
        Some(i) => Ok(History::Done(i)),
        // Finished without an image: a workflow with no image output node.
        None if status.and_then(|s| s.get("completed")).and_then(Value::as_bool) == Some(true) => {
            Err(Error::Server("ComfyUI finished the workflow but it produced no image (it needs a Save Image or Preview Image node)".into()))
        }
        None => Ok(History::Pending),
    }
}

/// Percent-encode a query value.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `GET` address of a rendered image.
pub fn view_url(base: &str, image: &ImageRef) -> String {
    format!("{base}/view?filename={}&subfolder={}&type={}", encode(&image.filename), encode(&image.subfolder), encode(&image.kind))
}

/// The file names in the answer to `GET /models/<folder>`.
pub fn parse_models(body: &[u8]) -> Result<Vec<String>> {
    let v = json_of(body, "model list")?;
    let list = v.as_array().ok_or_else(|| Error::BadResponse("ComfyUI's model list was not a list".into()))?;
    Ok(list.iter().filter_map(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 512).take(10_000).map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job { prompt: "a red frog".into(), width: 1024, height: 768, seed: 42, alpha: false }
    }

    fn classes(w: &Value) -> Vec<String> {
        w.as_object().unwrap().values().map(|n| n["class_type"].as_str().unwrap().to_string()).collect()
    }

    /// Every link points at a node of the workflow, and exactly one node shows the image.
    fn assert_well_formed(w: &Value) {
        let nodes = w.as_object().unwrap();
        for (id, n) in nodes {
            for (key, input) in n["inputs"].as_object().unwrap() {
                if let Some(link) = input.as_array() {
                    assert!(nodes.contains_key(link[0].as_str().unwrap()), "node {id} input {key} links to a missing node");
                }
            }
        }
        assert_eq!(classes(w).iter().filter(|c| *c == "PreviewImage").count(), 1);
    }

    #[test]
    fn every_family_preset_builds_a_well_formed_workflow() {
        for f in Family::ALL {
            if f == Family::Custom {
                continue;
            }
            let mut m = f.preset();
            if f == Family::Checkpoint {
                m.checkpoint = "sdxl.safetensors".into();
            }
            let w = workflow(&m, &job()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_well_formed(&w);
            let text = w.to_string();
            assert!(text.contains("a red frog") && text.contains("1024") && text.contains("768"), "{f:?}");
        }
    }

    #[test]
    fn z_image_uses_its_reference_nodes() {
        let w = workflow(&Family::ZImage.preset(), &job()).unwrap();
        let c = classes(&w);
        for want in ["UNETLoader", "CLIPLoader", "VAELoader", "ModelSamplingAuraFlow", "ConditioningZeroOut", "EmptySD3LatentImage", "KSampler", "VAEDecode"] {
            assert!(c.iter().any(|x| x == want), "missing {want}: {c:?}");
        }
        let sampler = w.as_object().unwrap().values().find(|n| n["class_type"] == "KSampler").unwrap();
        assert_eq!(sampler["inputs"]["seed"], 42);
        assert_eq!(sampler["inputs"]["steps"], 8);
        assert_eq!(sampler["inputs"]["sampler_name"], "res_multistep");
    }

    #[test]
    fn flux2_samples_with_its_scheduler_and_a_cfg_guider() {
        let mut m = Family::Flux2.preset();
        m.lora = "turbo.safetensors".into();
        let w = workflow(&m, &job()).unwrap();
        let c = classes(&w);
        for want in ["Flux2Scheduler", "CFGGuider", "SamplerCustomAdvanced", "RandomNoise", "EmptyFlux2LatentImage", "LoraLoaderModelOnly"] {
            assert!(c.iter().any(|x| x == want), "missing {want}: {c:?}");
        }
        // CFG above 1 encodes the negative prompt instead of zeroing it.
        assert_eq!(c.iter().filter(|x| *x == "CLIPTextEncode").count(), 2);
        assert!(!c.iter().any(|x| x == "FluxGuidance"));
        m.guidance = 4.0;
        assert!(classes(&workflow(&m, &job()).unwrap()).iter().any(|x| x == "FluxGuidance"));
    }

    #[test]
    fn qwen_image_2_1_encodes_with_its_own_node() {
        let w = workflow(&Family::QwenImage21.preset(), &job()).unwrap();
        let c = classes(&w);
        assert!(c.iter().any(|x| x == "TextEncodeQwenImage21") && !c.iter().any(|x| x == "CLIPTextEncode"), "{c:?}");
        let nodes = w.as_object().unwrap();
        let (id, encode) = nodes.iter().find(|(_, n)| n["class_type"] == "TextEncodeQwenImage21").unwrap();
        assert_eq!(encode["inputs"]["prompt"], "a red frog");
        assert!(encode["inputs"]["vae"].is_array());
        let sampler = nodes.values().find(|n| n["class_type"] == "KSampler").unwrap();
        assert_eq!(sampler["inputs"]["positive"], json!([id, 0]));
        assert_eq!(sampler["inputs"]["negative"], json!([id, 1]));
        // The larger Qwen 3 VL encoder and the 2.1 VAE: the older Qwen Image files don't fit it.
        let text = w.to_string();
        assert!(text.contains("qwen3vl_8b") && text.contains("qwen_image_2.1_vae") && !text.contains("qwen_2.5_vl"), "{text}");
        assert_eq!(Family::QwenImage21.size_multiple(), 32);
    }

    #[test]
    fn ideogram_guides_with_its_unconditional_model() {
        let w = workflow(&Family::Ideogram4.preset(), &job()).unwrap();
        let guider = w.as_object().unwrap().values().find(|n| n["class_type"] == "DualModelGuider").unwrap();
        assert!(guider["inputs"]["model_negative"].is_array());
        assert_eq!(classes(&w).iter().filter(|x| *x == "UNETLoader").count(), 2);
    }

    #[test]
    fn missing_files_are_reported_by_name() {
        let mut m = Family::Krea2.preset();
        m.vae.clear();
        let e = workflow(&m, &job()).unwrap_err();
        assert!(matches!(&e, Error::Config(s) if s.contains("Krea 2 Turbo") && s.contains("VAE")), "{e}");
        let e = workflow(&Family::Checkpoint.preset(), &job()).unwrap_err();
        assert!(e.to_string().contains("checkpoint"));
    }

    #[test]
    fn hostile_numbers_never_reach_the_workflow() {
        let mut m = Family::ZImage.preset();
        (m.cfg, m.shift, m.lora_strength, m.steps) = (f64::NAN, f64::INFINITY, f64::NEG_INFINITY, u32::MAX);
        m.lora = "x.safetensors".into();
        let w = workflow(&m, &job()).unwrap();
        assert!(!w.to_string().contains("null"), "{w}");
        assert!(w.to_string().contains("\"steps\":1000"));
    }

    #[test]
    fn custom_workflows_fill_placeholders() {
        let mut m = Model { family: Family::Custom, name: "Mine".into(), steps: 12, cfg: 2.5, negative_prompt: "blurry".into(), ..Model::default() };
        m.workflow = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"photo of %prompt%, sharp","clip":["2",0]}},
            "3":{"class_type":"EmptyLatentImage","inputs":{"width":"%width%","height":" %height% ","batch_size":1}},
            "4":{"class_type":"KSampler","inputs":{"seed":"%seed%","steps":"%steps%","cfg":"%cfg%","note":"%width%x%height%"}},
            "5":{"class_type":"CLIPTextEncode","inputs":{"text":"%negative%"}}}"#
            .into();
        let w = workflow(&m, &job()).unwrap();
        assert_eq!(w["1"]["inputs"]["text"], "photo of a red frog, sharp");
        assert_eq!(w["3"]["inputs"]["width"], 1024);
        assert_eq!(w["3"]["inputs"]["height"], 768);
        assert_eq!(w["4"]["inputs"]["seed"], 42);
        assert_eq!(w["4"]["inputs"]["steps"], 12);
        assert_eq!(w["4"]["inputs"]["cfg"], 2.5);
        assert_eq!(w["4"]["inputs"]["note"], "1024x768");
        assert_eq!(w["5"]["inputs"]["text"], "blurry");
        assert_eq!(w["1"]["inputs"]["clip"], json!(["2", 0]));
    }

    #[test]
    fn custom_workflows_explain_what_is_wrong() {
        let mut m = Model { family: Family::Custom, name: "Mine".into(), ..Model::default() };
        for (text, want) in [
            ("", "paste a workflow"),
            ("{not json", "not valid JSON"),
            (r#"{"nodes":[],"links":[]}"#, "API format"),
            ("[]", "API format"),
            (r#"{"1":{"class_type":"KSampler","inputs":{}}}"#, "%prompt%"),
        ] {
            m.workflow = text.into();
            let e = workflow(&m, &job()).unwrap_err().to_string();
            assert!(e.contains(want), "{text:?} -> {e}");
        }
        // A deeply nested workflow is walked to a bounded depth, not recursed into forever.
        m.workflow = format!(r#"{{"1":{{"class_type":"X","inputs":{{"a":{}"%prompt%"{}}}}}}}"#, "[".repeat(200), "]".repeat(200));
        // serde_json's own recursion limit rejects it; either way it is an error, not a crash.
        let _ = workflow(&m, &job());
    }

    #[test]
    fn queue_answers() {
        assert_eq!(parse_queued(200, br#"{"prompt_id":"abc","number":3}"#).unwrap(), "abc");
        let rejected = br#"{"error":{"type":"prompt_outputs_failed_validation","message":"Prompt outputs failed validation"},
            "node_errors":{"4":{"class_type":"UNETLoader","errors":[{"message":"Value not in list","details":"unet_name: 'x.safetensors' not in [...]"}]}}}"#;
        let e = parse_queued(400, rejected).unwrap_err().to_string();
        assert!(e.contains("UNETLoader") && e.contains("x.safetensors"), "{e}");
        assert!(parse_queued(400, br#"{"error":"bad"}"#).unwrap_err().to_string().contains("bad"));
        assert!(matches!(parse_queued(500, b"<html>"), Err(Error::BadResponse(_))));
        assert!(matches!(parse_queued(200, b"{}"), Err(Error::BadResponse(_))));
        assert!(matches!(parse_queued(200, br#"{"prompt_id":7}"#), Err(Error::BadResponse(_))));
    }

    #[test]
    fn history_answers() {
        assert_eq!(parse_history(b"{}", "abc").unwrap(), History::Pending);
        let done =
            br#"{"abc":{"outputs":{"9":{"images":[{"filename":"a b.png","subfolder":"","type":"temp"}]}},"status":{"status_str":"success","completed":true}}}"#;
        let History::Done(img) = parse_history(done, "abc").unwrap() else { panic!("not done") };
        assert_eq!(img, ImageRef { filename: "a b.png".into(), subfolder: String::new(), kind: "temp".into() });
        assert_eq!(view_url("http://h:1", &img), "http://h:1/view?filename=a%20b.png&subfolder=&type=temp");
        let failed = br#"{"abc":{"outputs":{},"status":{"status_str":"error","completed":false,"messages":[["execution_start",{}],
            ["execution_error",{"node_type":"KSampler","exception_message":"CUDA out of memory"}]]}}}"#;
        let e = parse_history(failed, "abc").unwrap_err().to_string();
        assert!(e.contains("KSampler") && e.contains("out of memory"), "{e}");
        let empty = br#"{"abc":{"outputs":{},"status":{"status_str":"success","completed":true}}}"#;
        assert!(parse_history(empty, "abc").unwrap_err().to_string().contains("no image"));
        for hostile in [&b"null"[..], b"[]", b"{\"abc\":7}", b"{\"abc\":{\"outputs\":[1,2]}}", b"{\"abc\":{\"outputs\":{\"1\":{\"images\":[{}]}}}}"] {
            assert_eq!(parse_history(hostile, "abc").unwrap(), History::Pending, "{}", String::from_utf8_lossy(hostile));
        }
        assert!(parse_history(b"\xff\xfe", "abc").is_err());
    }

    #[test]
    fn server_addresses_and_model_lists() {
        assert_eq!(base_url(" http://127.0.0.1:8188/ ").unwrap(), "http://127.0.0.1:8188");
        for bad in ["", "127.0.0.1:8188", "ftp://x", "http://"] {
            assert!(base_url(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_models(br#"["a.safetensors","",3,"b.gguf"]"#).unwrap(), vec!["a.safetensors", "b.gguf"]);
        assert!(parse_models(b"{}").is_err());
        assert_eq!(clip("a\n  b   c", 10), "a b c");
        assert_eq!(clip("héllo wörld", 5), "héllo…");
    }
}
