//! Local models through a ComfyUI server: the workflow for each model family, and the server's
//! queue, history and file answers.
//!
//! The workflows follow ComfyUI's own text-to-image templates for each family and use only nodes
//! that ship with ComfyUI. Anything else (LoRA stacks, prompt enhancers, custom samplers) goes
//! through [`Family::Custom`]: the user's own workflow with placeholders.

use serde_json::{Map, Value, json};

use crate::http::{Part, Request};
use crate::model::{Family, Model};
use crate::run::Job;
use crate::{Error, Result};

/// The folders of `models/` that hold each kind of file, as `GET /models/<folder>` names them.
pub const FOLDERS: [&str; 6] = ["diffusion_models", "text_encoders", "vae", "checkpoints", "loras", "controlnet"];

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
        Family::QwenImage | Family::QwenImage21 | Family::QwenInpaint | Family::QwenEdit => "qwen_image",
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

/// What an inpainting workflow works on: the image and mask as ComfyUI's input folder names them
/// once uploaded ([`upload_request`]), and how much of the masked part to denoise (0..=1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inpaint<'a> {
    pub image: &'a str,
    /// `None`: image to image. The whole picture is redrawn, as far from it as `denoise` says.
    pub mask: Option<&'a str>,
    pub denoise: f64,
}

/// What an edit workflow works on: the picture to edit and the pictures it may refer to, as
/// ComfyUI's input folder names them once uploaded ([`upload_request`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edit<'a> {
    pub image: &'a str,
    /// Further pictures, which the instruction calls image 2, image 3, … ([`Family::max_references`]
    /// at most).
    pub references: &'a [String],
}

/// The names the image and mask of an inpainting are uploaded under. They are overwritten by
/// the next one, so ComfyUI's input folder holds two files of ours, not one pair per generation.
pub const UPLOAD_IMAGE: &str = "photocraft_inpaint_image.png";
pub const UPLOAD_MASK: &str = "photocraft_inpaint_mask.png";
/// The name the `index`th reference picture of an edit is uploaded under (overwritten likewise).
pub fn upload_reference(index: usize) -> String {
    format!("photocraft_edit_reference_{}.png", index.saturating_add(1))
}

/// The workflow that renders `job` with `model`, ending in a preview image (kept in ComfyUI's
/// temp folder, not its output folder).
pub fn workflow(model: &Model, job: &Job) -> Result<Value> {
    build(model, job, None)
}

/// The workflow that repaints the masked part of an uploaded image with `model`: the image is
/// encoded to a latent and only the masked part of it is denoised (Set Latent Noise Mask), so
/// what surrounds it steers the result and comes back as it was. `job` holds the image's size.
///
/// The mask may be soft: Differential Diffusion denoises each part of it for as long as it is
/// white, so a mask that fades towards its edge blends the new into the old there. FLUX.2 is an
/// edit model and is also given the image as a reference: it then paints into the scene it sees
/// (its light, its scale) instead of composing a picture of its own inside the mask.
pub fn inpaint_workflow(model: &Model, job: &Job, inpaint: &Inpaint<'_>) -> Result<Value> {
    build(model, job, Some(inpaint))
}

/// The workflow that edits an uploaded picture with `model`, following `job`'s prompt as an
/// instruction. The model is shown the picture whole (and the references), as its own reference
/// workflow does, and renders a new one at `job`'s size, which is the picture's:
///
/// - Qwen Image 2.1: its encoder takes the pictures with the prompt and gives the latent to
///   sample (ComfyUI's Image Edit template), at the pictures' own sizes.
/// - Qwen Image Edit: its encoder takes the pictures with the prompt, for both sides of the
///   guidance; 2511 reads them by index (ComfyUI's templates).
/// - FLUX.2: each picture is a reference latent on the conditioning.
pub fn edit_workflow(model: &Model, job: &Job, edit: &Edit<'_>) -> Result<Value> {
    build_edit(model, job, edit)
}

/// Can `model` edit a picture? The error says what to fix.
pub fn check_edit(model: &Model) -> Result<()> {
    let job = Job { prompt: "x".into(), width: 1024, height: 1024, seed: 0, alpha: false, source: None };
    build_edit(model, &job, &Edit { image: "image", references: &[] }).map(|_| ())
}

/// Can `model` run at all (and, with `inpainting`, repaint an image)? The error says what to fix.
pub fn check(model: &Model, inpainting: bool) -> Result<()> {
    let job = Job { prompt: "x".into(), width: 1024, height: 1024, seed: 0, alpha: false, source: None };
    build(model, &job, inpainting.then_some(&Inpaint { image: "image", mask: Some("mask"), denoise: 1.0 })).map(|_| ())
}

/// Can `model` redraw a picture (image to image)? The error says what to fix.
pub fn check_img2img(model: &Model) -> Result<()> {
    let job = Job { prompt: "x".into(), width: 1024, height: 1024, seed: 0, alpha: false, source: None };
    build(model, &job, Some(&Inpaint { image: "image", mask: None, denoise: 0.5 })).map(|_| ())
}

fn build(model: &Model, job: &Job, inpaint: Option<&Inpaint<'_>>) -> Result<Value> {
    if model.family == Family::Custom {
        return custom_workflow(model, job, inpaint);
    }
    // A set-up that can't run: say so in words here, not as a tensor shape from the sampler
    // after the models have loaded.
    if let Some(why) = crate::detect::problem(model) {
        return Err(Error::Config(why));
    }
    let f = model.family;
    if f.inpaint_only() && inpaint.is_none_or(|i| i.mask.is_none()) {
        let name = if model.name.is_empty() { f.label() } else { model.name.as_str() };
        return Err(Error::Config(format!("{name} is an inpainting model: it repaints a selection (the InPaint tab) and can't generate from a prompt alone")));
    }
    if f.edit_only() {
        let name = if model.name.is_empty() { f.label() } else { model.name.as_str() };
        return Err(Error::Config(format!("{name} is an edit model: it changes a picture following an instruction (the ImgEdit tab) and does nothing else")));
    }
    let mut g = Graph::default();
    let (mut unet, clip, vae) = loaders(&mut g, model)?;
    let cfg = finite(model.cfg, 1.0);
    let (mut positive, mut negative) = if f == Family::QwenImage21 {
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
    let denoise = inpaint.map_or(1.0, |i| finite(i.denoise, 1.0).clamp(0.0, 1.0));
    let latent = match inpaint {
        // Image to image: the picture itself is the latent the sampler starts from, and how
        // much of its schedule it runs (the denoise) is how far it gets from it.
        Some(Inpaint { image, mask: None, .. }) => {
            let image = g.node("LoadImage", json!({"image": image}));
            g.node("VAEEncode", json!({"pixels": image, "vae": vae}))
        }
        Some(Inpaint { image, mask: Some(mask), .. }) => {
            let image = g.node("LoadImage", json!({"image": image}));
            // Load Image's second output is the mask: where the file is transparent.
            let mask = json!([g.add("LoadImage", json!({"image": mask})), 1]);
            let encoded = g.node("VAEEncode", json!({"pixels": image, "vae": vae}));
            unet = g.node("DifferentialDiffusion", json!({"model": unet}));
            match f {
                // The dedicated inpainting models take the picture and the mask as conditioning
                // (ComfyUI's own inpainting templates): they know where they paint.
                Family::Flux1Fill => {
                    let id = g.add(
                        "InpaintModelConditioning",
                        json!({"positive": positive, "negative": negative, "vae": vae, "pixels": image, "mask": mask, "noise_mask": true}),
                    );
                    (positive, negative) = (json!([id, 0]), json!([id, 1]));
                    json!([id, 2])
                }
                Family::QwenInpaint => {
                    let control = g.node("ControlNetLoader", json!({"control_net_name": file(&model.controlnet, "an inpainting ControlNet file", model)?}));
                    let id = g.add(
                        "ControlNetInpaintingAliMamaApply",
                        json!({
                            "positive": positive, "negative": negative, "control_net": control, "vae": vae, "image": image, "mask": mask,
                            "strength": 1.0, "start_percent": 0.0, "end_percent": 1.0,
                        }),
                    );
                    (positive, negative) = (json!([id, 0]), json!([id, 1]));
                    g.node("SetLatentNoiseMask", json!({"samples": encoded, "mask": mask}))
                }
                _ => {
                    if f == Family::Flux2 {
                        positive = g.node("ReferenceLatent", json!({"conditioning": positive, "latent": encoded}));
                        negative = g.node("ReferenceLatent", json!({"conditioning": negative, "latent": encoded}));
                    }
                    g.node("SetLatentNoiseMask", json!({"samples": encoded, "mask": mask}))
                }
            }
        }
        None => g.node(latent_node(f), json!({"width": job.width, "height": job.height, "batch_size": 1})),
    };
    let samples = sample(&mut g, model, job, &Sampling { unet, positive, negative, latent, denoise });
    let image = g.node("VAEDecode", json!({"samples": samples, "vae": vae}));
    g.add("PreviewImage", json!({"images": image}));
    Ok(Value::Object(g.nodes))
}

/// The nodes that load `model`'s files: the diffusion model (through its LoRA and its sampling
/// shift), the text encoder and the VAE.
fn loaders(g: &mut Graph, model: &Model) -> Result<(Link, Link, Link)> {
    let f = model.family;
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
    Ok((unet, clip, vae))
}

/// What a sampler is given.
struct Sampling {
    unet: Link,
    positive: Link,
    negative: Link,
    latent: Link,
    denoise: f64,
}

/// The nodes that sample `model` (its family's sampler, with the model's steps, CFG, sampler and
/// scheduler), and the latent they give.
fn sample(g: &mut Graph, model: &Model, job: &Job, with: &Sampling) -> Link {
    let f = model.family;
    let Sampling { unet, positive, negative, latent, denoise } = with;
    let denoise = *denoise;
    let cfg = finite(model.cfg, 1.0);
    let steps = model.steps.clamp(1, 1000);
    let sampler = if model.sampler.trim().is_empty() { "euler" } else { model.sampler.trim() };
    let scheduler = if model.scheduler.trim().is_empty() { "simple" } else { model.scheduler.trim() };
    match f {
        Family::Flux2 | Family::Ideogram4 => {
            let noise = g.node("RandomNoise", json!({"noise_seed": job.seed}));
            let select = g.node("KSamplerSelect", json!({"sampler_name": sampler}));
            let (sigmas, guider) = if f == Family::Flux2 {
                let mut sigmas = g.node("Flux2Scheduler", json!({"steps": steps, "width": job.width, "height": job.height}));
                // Its scheduler has no denoise: keep the tail of the schedule instead.
                if denoise < 1.0 {
                    sigmas = json!([g.add("SplitSigmasDenoise", json!({"sigmas": sigmas, "denoise": denoise})), 1]);
                }
                (sigmas, g.node("CFGGuider", json!({"model": unet, "positive": positive, "negative": negative, "cfg": cfg})))
            } else {
                let sigmas = g.node("BasicScheduler", json!({"model": unet, "scheduler": scheduler, "steps": steps, "denoise": denoise}));
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
                "positive": positive, "negative": negative, "latent_image": latent, "denoise": denoise,
            }),
        ),
    }
}

/// [`edit_workflow`].
fn build_edit(model: &Model, job: &Job, edit: &Edit<'_>) -> Result<Value> {
    let f = model.family;
    let name = if model.name.is_empty() { f.label() } else { model.name.as_str() };
    if !f.edits() {
        return Err(Error::Config(format!(
            "{name} can't edit a picture from an instruction: choose an edit model (Qwen Image 2.1, Qwen Image Edit or FLUX.2) on the ImgEdit tab"
        )));
    }
    if let Some(why) = crate::detect::problem(model) {
        return Err(Error::Config(why));
    }
    if edit.references.len() > f.max_references() {
        return Err(Error::Config(format!("{name} takes {} reference pictures at most, not {}", f.max_references(), edit.references.len())));
    }
    let mut g = Graph::default();
    let (mut unet, clip, vae) = loaders(&mut g, model)?;
    let cfg = finite(model.cfg, 1.0);
    // The picture to edit first: it is "image 1" to the model, the references follow.
    let pictures: Vec<Link> =
        std::iter::once(edit.image).chain(edit.references.iter().map(String::as_str)).map(|name| g.node("LoadImage", json!({"image": name}))).collect();
    let (positive, negative, latent) = match f {
        Family::QwenImage21 => {
            // The pictures are the same at every step: their part of the sequence is cached.
            unet = g.node("QwenImage21Cache", json!({"model": unet, "device": "auto", "dtype": "default"}));
            // Resolution 0: the pictures are taken at the size they were sent, so the latent the
            // node gives (the first picture's size) is the size asked for.
            let mut inputs = json!({"clip": clip, "prompt": job.prompt, "negative_prompt": model.negative_prompt, "resolution": 0, "vae": vae});
            for (i, picture) in pictures.iter().enumerate() {
                inputs[format!("images.image_{}", i + 1)] = picture.clone();
            }
            let id = g.add("TextEncodeQwenImage21", inputs);
            (json!([id, 0]), json!([id, 1]), json!([id, 2]))
        }
        Family::QwenEdit => {
            unet = g.node("CFGNorm", json!({"model": unet, "strength": 1.0}));
            let encode = |g: &mut Graph, prompt: &str| {
                let mut inputs = json!({"clip": clip, "vae": vae, "prompt": prompt});
                for (i, picture) in pictures.iter().enumerate() {
                    inputs[format!("image{}", i + 1)] = picture.clone();
                }
                let cond = g.node("TextEncodeQwenImageEditPlus", inputs);
                // 2511 was trained to tell its pictures apart by index.
                if model.unet.contains("2511") {
                    g.node("FluxKontextMultiReferenceLatentMethod", json!({"conditioning": cond, "reference_latents_method": "index_timestep_zero"}))
                } else {
                    cond
                }
            };
            let positive = encode(&mut g, &job.prompt);
            let negative = encode(&mut g, &model.negative_prompt);
            (positive, negative, g.node("EmptySD3LatentImage", json!({"width": job.width, "height": job.height, "batch_size": 1})))
        }
        _ => {
            let mut positive = g.node("CLIPTextEncode", json!({"text": job.prompt, "clip": clip}));
            let mut negative = if cfg <= 1.0 {
                g.node("ConditioningZeroOut", json!({"conditioning": positive}))
            } else {
                g.node("CLIPTextEncode", json!({"text": model.negative_prompt, "clip": clip}))
            };
            let guidance = finite(model.guidance, 0.0);
            if guidance > 0.0 {
                positive = g.node("FluxGuidance", json!({"conditioning": positive, "guidance": guidance}));
            }
            for picture in &pictures {
                let encoded = g.node("VAEEncode", json!({"pixels": picture, "vae": vae}));
                positive = g.node("ReferenceLatent", json!({"conditioning": positive, "latent": encoded}));
                negative = g.node("ReferenceLatent", json!({"conditioning": negative, "latent": encoded}));
            }
            (positive, negative, g.node("EmptyFlux2LatentImage", json!({"width": job.width, "height": job.height, "batch_size": 1})))
        }
    };
    let samples = sample(&mut g, model, job, &Sampling { unet, positive, negative, latent, denoise: 1.0 });
    let image = g.node("VAEDecode", json!({"samples": samples, "vae": vae}));
    g.add("PreviewImage", json!({"images": image}));
    Ok(Value::Object(g.nodes))
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

/// The user's workflow with its placeholders filled in. A string that is exactly a numeric
/// placeholder (`"%width%"`) becomes a number; placeholders inside longer text are substituted.
fn custom_workflow(model: &Model, job: &Job, inpaint: Option<&Inpaint<'_>>) -> Result<Value> {
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
    if inpaint.is_some() && !text.contains("%image%") {
        return Err(Error::Config(format!(
            "{}: the workflow has no %image% placeholder, so it can't inpaint. Put %image% and %mask% in two Load Image nodes (the mask is the second one's MASK output)",
            model.name
        )));
    }
    let numbers: [(&str, Value); 6] = [
        ("%width%", json!(job.width)),
        ("%height%", json!(job.height)),
        ("%seed%", json!(job.seed)),
        ("%steps%", json!(model.steps.clamp(1, 1000))),
        ("%cfg%", json!(finite(model.cfg, 1.0))),
        ("%denoise%", json!(inpaint.map_or(1.0, |i| finite(i.denoise, 1.0).clamp(0.0, 1.0)))),
    ];
    let texts: [(&str, &str); 4] = [
        ("%prompt%", &job.prompt),
        ("%negative%", &model.negative_prompt),
        ("%image%", inpaint.map_or("", |i| i.image)),
        ("%mask%", inpaint.and_then(|i| i.mask).unwrap_or("")),
    ];
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

/// The request that puts `bytes` (a PNG) in ComfyUI's input folder as `name`, replacing a file of
/// that name, so a Load Image node can read it.
pub fn upload_request(base: &str, name: &str, bytes: &[u8]) -> Request {
    let parts = [Part::png("image", name, bytes), Part::text("type", "input"), Part::text("overwrite", "true")];
    Request::post_multipart(format!("{base}/upload/image"), &parts).timeout(120)
}

/// The name a Load Image node finds an uploaded file under, from the answer to the upload.
pub fn parse_uploaded(status: u16, body: &[u8]) -> Result<String> {
    if !(200..300).contains(&status) {
        return Err(Error::Server(format!("ComfyUI did not take the image to inpaint (HTTP {status}): {}", clip(&String::from_utf8_lossy(body), 200))));
    }
    let v = json_of(body, "upload")?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let (name, folder) = (text("name"), text("subfolder"));
    if name.is_empty() || name.len() > 256 || folder.len() > 256 {
        return Err(Error::BadResponse("ComfyUI took the image to inpaint but did not say under which name".into()));
    }
    Ok(if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") })
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
        Job { prompt: "a red frog".into(), width: 1024, height: 768, seed: 42, alpha: false, source: None }
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
            if f.edit_only() {
                // An edit model has no text-to-image workflow either.
                let e = workflow(&m, &job()).unwrap_err().to_string();
                assert!(e.contains("edit model") && check(&m, false).is_err() && check(&m, true).is_err() && check_edit(&m).is_ok(), "{f:?}: {e}");
                continue;
            }
            if f.inpaint_only() {
                // A dedicated inpainting model has no text-to-image workflow, and says so.
                let e = workflow(&m, &job()).unwrap_err().to_string();
                assert!(e.contains("inpainting model") && check(&m, false).is_err() && check(&m, true).is_ok(), "{f:?}: {e}");
                continue;
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
    fn a_set_up_that_cannot_run_is_refused_in_words() {
        let wrong = Model { unet: "qwen_image_2.1_int8_convrot.safetensors".into(), name: "qwen 2.1".into(), ..Family::QwenImage.preset() };
        let e = workflow(&wrong, &job()).unwrap_err();
        assert!(matches!(&e, Error::Config(s) if s.contains("Qwen Image 2.1 model") && s.contains("Scan Server")), "{e}");
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

    const INPAINT: Inpaint<'static> = Inpaint { image: "src.png", mask: Some("mask.png"), denoise: 0.6 };
    const IMG2IMG: Inpaint<'static> = Inpaint { image: "src.png", mask: None, denoise: 0.6 };

    #[test]
    fn image_to_image_starts_from_the_encoded_picture_with_no_mask() {
        for f in Family::ALL {
            if f == Family::Custom {
                continue;
            }
            let mut m = f.preset();
            if f == Family::Checkpoint {
                m.checkpoint = "sdxl.safetensors".into();
            }
            if f.edit_only() {
                assert!(inpaint_workflow(&m, &job(), &IMG2IMG).unwrap_err().to_string().contains("edit model") && check_img2img(&m).is_err(), "{f:?}");
                continue;
            }
            if f.inpaint_only() {
                // A model made for inpainting needs a mask.
                assert!(inpaint_workflow(&m, &job(), &IMG2IMG).unwrap_err().to_string().contains("inpainting model") && check_img2img(&m).is_err(), "{f:?}");
                continue;
            }
            let w = inpaint_workflow(&m, &job(), &IMG2IMG).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_well_formed(&w);
            let c = classes(&w);
            assert_eq!(c.iter().filter(|x| *x == "LoadImage").count(), 1, "{f:?}");
            for absent in ["SetLatentNoiseMask", "DifferentialDiffusion", "ReferenceLatent", "InpaintModelConditioning"] {
                assert!(!c.iter().any(|x| x == absent) && !c.iter().any(|x| x.starts_with("Empty")), "{f:?}: {c:?}");
            }
            // The sampler's latent is the encoded picture.
            let nodes = w.as_object().unwrap();
            let sampler = nodes.values().find(|n| matches!(n["class_type"].as_str(), Some("KSampler" | "SamplerCustomAdvanced"))).unwrap();
            let latent = &nodes[sampler["inputs"]["latent_image"][0].as_str().unwrap()];
            assert_eq!(latent["class_type"], "VAEEncode", "{f:?}");
            assert_eq!(nodes[latent["inputs"]["pixels"][0].as_str().unwrap()]["inputs"]["image"], "src.png", "{f:?}");
            assert!(check_img2img(&m).is_ok());
        }
        // The strength reaches each kind of sampler.
        assert_eq!(node(&inpaint_workflow(&Family::ZImage.preset(), &job(), &IMG2IMG).unwrap(), "KSampler")["inputs"]["denoise"], 0.6);
        assert_eq!(node(&inpaint_workflow(&Family::Ideogram4.preset(), &job(), &IMG2IMG).unwrap(), "BasicScheduler")["inputs"]["denoise"], 0.6);
        assert_eq!(node(&inpaint_workflow(&Family::Flux2.preset(), &job(), &IMG2IMG).unwrap(), "SplitSigmasDenoise")["inputs"]["denoise"], 0.6);
        // A custom workflow gets the picture's name, and nothing for the mask.
        let mut m = Model { family: Family::Custom, name: "Mine".into(), ..Model::default() };
        m.workflow = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"%prompt%"}},"2":{"class_type":"LoadImage","inputs":{"image":"%image%"}},
            "3":{"class_type":"LoadImage","inputs":{"image":"%mask%"}},"4":{"class_type":"KSampler","inputs":{"denoise":"%denoise%"}}}"#
            .into();
        let w = inpaint_workflow(&m, &job(), &IMG2IMG).unwrap();
        assert_eq!(
            (w["2"]["inputs"]["image"].as_str(), w["3"]["inputs"]["image"].as_str(), w["4"]["inputs"]["denoise"].as_f64()),
            (Some("src.png"), Some(""), Some(0.6))
        );
    }

    fn node<'a>(w: &'a Value, class: &str) -> &'a Value {
        w.as_object().unwrap().values().find(|n| n["class_type"] == class).unwrap_or_else(|| panic!("no {class} in {w}"))
    }

    #[test]
    fn inpainting_encodes_the_image_and_denoises_only_the_mask() {
        for f in Family::ALL {
            if f == Family::Custom {
                continue;
            }
            let mut m = f.preset();
            if f == Family::Checkpoint {
                m.checkpoint = "sdxl.safetensors".into();
            }
            if f.edit_only() {
                assert!(inpaint_workflow(&m, &job(), &INPAINT).unwrap_err().to_string().contains("edit model"), "{f:?}");
                continue;
            }
            let w = inpaint_workflow(&m, &job(), &INPAINT).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_well_formed(&w);
            let c = classes(&w);
            // No empty latent: the sampler starts from the encoded image with a noise mask.
            assert!(!c.iter().any(|x| x.starts_with("Empty")), "{f:?}: {c:?}");
            assert_eq!(c.iter().filter(|x| *x == "LoadImage").count(), 2, "{f:?}");
            // The sampler's model goes through Differential Diffusion, which honours a soft mask.
            assert_eq!(c.iter().filter(|x| *x == "DifferentialDiffusion").count(), 1, "{f:?}");
            if f == Family::Flux1Fill {
                continue;
            }
            let masked = node(&w, "SetLatentNoiseMask");
            let nodes = w.as_object().unwrap();
            let linked = |v: &Value| &nodes[v[0].as_str().unwrap()];
            assert_eq!(linked(&masked["inputs"]["samples"])["class_type"], "VAEEncode", "{f:?}");
            // The mask is the second output of its Load Image, used as it is (it may be soft).
            assert_eq!(masked["inputs"]["mask"][1], 1, "{f:?}");
            assert_eq!(linked(&masked["inputs"]["mask"])["inputs"]["image"], "mask.png");
            assert_eq!(c.iter().filter(|x| *x == "ReferenceLatent").count(), if f == Family::Flux2 { 2 } else { 0 }, "{f:?}");
            assert_eq!(linked(&node(&w, "VAEEncode")["inputs"]["pixels"])["inputs"]["image"], "src.png");
            assert!(check(&m, true).is_ok() && (f.inpaint_only() || check(&m, false).is_ok()));
        }
        // The strength reaches each kind of sampler: KSampler, a scheduler, and FLUX.2's by
        // cutting its schedule.
        assert_eq!(node(&inpaint_workflow(&Family::ZImage.preset(), &job(), &INPAINT).unwrap(), "KSampler")["inputs"]["denoise"], 0.6);
        assert_eq!(node(&inpaint_workflow(&Family::Ideogram4.preset(), &job(), &INPAINT).unwrap(), "BasicScheduler")["inputs"]["denoise"], 0.6);
        let flux = inpaint_workflow(&Family::Flux2.preset(), &job(), &INPAINT).unwrap();
        assert_eq!(node(&flux, "SplitSigmasDenoise")["inputs"]["denoise"], 0.6);
        assert_eq!(node(&flux, "SamplerCustomAdvanced")["inputs"]["sigmas"][1], 1);
        let full = inpaint_workflow(&Family::Flux2.preset(), &job(), &Inpaint { denoise: f64::NAN, ..INPAINT }).unwrap();
        assert!(!classes(&full).iter().any(|x| x == "SplitSigmasDenoise") && !full.to_string().contains("null"));
        // FLUX.2 sees the image as a reference on both sides of its guidance.
        let guider = node(&flux, "CFGGuider");
        let nodes = flux.as_object().unwrap();
        for side in ["positive", "negative"] {
            let reference = &nodes[guider["inputs"][side][0].as_str().unwrap()];
            assert_eq!(reference["class_type"], "ReferenceLatent", "{side}");
            assert_eq!(nodes[reference["inputs"]["latent"][0].as_str().unwrap()]["class_type"], "VAEEncode", "{side}");
        }
        assert_eq!(nodes[guider["inputs"]["model"][0].as_str().unwrap()]["class_type"], "DifferentialDiffusion");
        // Text to image is what it was: an empty latent, fully denoised.
        let plain = workflow(&Family::ZImage.preset(), &job()).unwrap();
        assert!(!classes(&plain).iter().any(|x| x == "LoadImage" || x == "DifferentialDiffusion"));
        assert!(!classes(&workflow(&Family::Flux2.preset(), &job()).unwrap()).iter().any(|x| x == "ReferenceLatent"));
        assert_eq!(node(&plain, "KSampler")["inputs"]["denoise"], 1.0);
    }

    #[test]
    fn dedicated_inpainting_models_are_given_the_picture_and_the_mask() {
        // FLUX.1 Fill, as ComfyUI's template: Inpaint Model Conditioning feeds the sampler its
        // conditioning and its latent.
        let w = inpaint_workflow(&Family::Flux1Fill.preset(), &job(), &INPAINT).unwrap();
        let nodes = w.as_object().unwrap();
        let (id, cond) = nodes.iter().find(|(_, n)| n["class_type"] == "InpaintModelConditioning").unwrap();
        assert_eq!(cond["inputs"]["noise_mask"], true);
        assert_eq!(nodes[cond["inputs"]["pixels"][0].as_str().unwrap()]["inputs"]["image"], "src.png");
        assert_eq!(
            (nodes[cond["inputs"]["mask"][0].as_str().unwrap()]["inputs"]["image"].as_str(), cond["inputs"]["mask"][1].as_u64()),
            (Some("mask.png"), Some(1))
        );
        assert_eq!(nodes[cond["inputs"]["positive"][0].as_str().unwrap()]["class_type"], "FluxGuidance");
        let sampler = node(&w, "KSampler");
        assert_eq!(
            (&sampler["inputs"]["positive"], &sampler["inputs"]["negative"], &sampler["inputs"]["latent_image"]),
            (&json!([id, 0]), &json!([id, 1]), &json!([id, 2]))
        );
        assert_eq!((sampler["inputs"]["denoise"].as_f64(), sampler["inputs"]["scheduler"].as_str()), (Some(0.6), Some("normal")));
        assert_eq!(node(&w, "FluxGuidance")["inputs"]["guidance"], 30.0);
        assert_eq!(node(&w, "DualCLIPLoader")["inputs"]["type"], "flux");
        assert!(!classes(&w).iter().any(|x| x == "SetLatentNoiseMask" || x == "ReferenceLatent"));

        // Qwen Image with the InstantX ControlNet, as ComfyUI's template.
        let w = inpaint_workflow(&Family::QwenInpaint.preset(), &job(), &INPAINT).unwrap();
        let nodes = w.as_object().unwrap();
        let (id, apply) = nodes.iter().find(|(_, n)| n["class_type"] == "ControlNetInpaintingAliMamaApply").unwrap();
        assert_eq!(
            nodes[apply["inputs"]["control_net"][0].as_str().unwrap()]["inputs"]["control_net_name"],
            "Qwen-Image-InstantX-ControlNet-Inpainting.safetensors"
        );
        assert_eq!(nodes[apply["inputs"]["image"][0].as_str().unwrap()]["inputs"]["image"], "src.png");
        let sampler = node(&w, "KSampler");
        assert_eq!((&sampler["inputs"]["positive"], &sampler["inputs"]["negative"]), (&json!([id, 0]), &json!([id, 1])));
        assert_eq!(nodes[sampler["inputs"]["latent_image"][0].as_str().unwrap()]["class_type"], "SetLatentNoiseMask");
        assert_eq!(node(&w, "ModelSamplingAuraFlow")["inputs"]["shift"], 3.1);
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["type"], "qwen_image");
        // Without its ControlNet it is not an inpainting model: say which file is missing.
        let mut m = Family::QwenInpaint.preset();
        m.controlnet.clear();
        assert!(inpaint_workflow(&m, &job(), &INPAINT).unwrap_err().to_string().contains("ControlNet"));
    }

    #[test]
    fn editing_shows_the_model_the_picture_and_its_references() {
        let references = ["shirt.png".to_string(), "hat.png".to_string()];
        let edit = Edit { image: "src.png", references: &references };
        let loaded = |w: &Value, link: &Value| w[link[0].as_str().unwrap()]["inputs"]["image"].as_str().unwrap().to_string();

        // Qwen Image 2.1, as ComfyUI's Image Edit template: its encoder takes the pictures and
        // gives both conditionings and the latent; the model's picture prefix is cached.
        let w = edit_workflow(&Family::QwenImage21.preset(), &job(), &edit).unwrap();
        assert_well_formed(&w);
        let nodes = w.as_object().unwrap();
        let (id, encode) = nodes.iter().find(|(_, n)| n["class_type"] == "TextEncodeQwenImage21").unwrap();
        assert_eq!((encode["inputs"]["prompt"].as_str(), encode["inputs"]["resolution"].as_u64()), (Some("a red frog"), Some(0)));
        let pictures: Vec<String> = (1..=3).map(|i| loaded(&w, &encode["inputs"][format!("images.image_{i}")])).collect();
        assert_eq!(pictures, ["src.png", "shirt.png", "hat.png"]);
        assert!(encode["inputs"].get("images.image_4").is_none());
        let sampler = node(&w, "KSampler");
        assert_eq!(
            (&sampler["inputs"]["positive"], &sampler["inputs"]["negative"], &sampler["inputs"]["latent_image"], sampler["inputs"]["denoise"].as_f64()),
            (&json!([id, 0]), &json!([id, 1]), &json!([id, 2]), Some(1.0))
        );
        assert_eq!(nodes[sampler["inputs"]["model"][0].as_str().unwrap()]["class_type"], "QwenImage21Cache");
        assert!(!classes(&w).iter().any(|c| c.starts_with("Empty") || c == "VAEEncode"));

        // Qwen Image Edit: the pictures go to its encoder on both sides of the guidance, and the
        // picture is rendered afresh at the size asked for.
        let w = edit_workflow(&Family::QwenEdit.preset(), &job(), &edit).unwrap();
        assert_well_formed(&w);
        let encoders: Vec<&Value> = w.as_object().unwrap().values().filter(|n| n["class_type"] == "TextEncodeQwenImageEditPlus").collect();
        assert_eq!(encoders.len(), 2);
        for e in &encoders {
            assert_eq!((1..=3).map(|i| loaded(&w, &e["inputs"][format!("image{i}")])).collect::<Vec<_>>(), ["src.png", "shirt.png", "hat.png"]);
        }
        assert!(encoders.iter().any(|e| e["inputs"]["prompt"] == "a red frog"));
        let c = classes(&w);
        // The preset is 2511, which reads its pictures by index.
        for want in ["CFGNorm", "ModelSamplingAuraFlow", "EmptySD3LatentImage", "KSampler"] {
            assert!(c.iter().any(|x| x == want), "missing {want}: {c:?}");
        }
        assert_eq!(c.iter().filter(|x| *x == "FluxKontextMultiReferenceLatentMethod").count(), 2);
        let older = Model { unet: "qwen_image_edit_2509_fp8_e4m3fn.safetensors".into(), ..Family::QwenEdit.preset() };
        assert!(!classes(&edit_workflow(&older, &job(), &edit).unwrap()).iter().any(|x| x == "FluxKontextMultiReferenceLatentMethod"));
        let latent = node(&w, "EmptySD3LatentImage");
        assert_eq!((latent["inputs"]["width"].as_u64(), latent["inputs"]["height"].as_u64()), (Some(1024), Some(768)));
        // It has three picture inputs in all: a third reference is refused in words.
        let many = ["a.png".to_string(), "b.png".to_string(), "c.png".to_string()];
        let e = edit_workflow(&Family::QwenEdit.preset(), &job(), &Edit { image: "src.png", references: &many }).unwrap_err().to_string();
        assert!(e.contains("2 reference pictures"), "{e}");
        assert!(edit_workflow(&Family::QwenImage21.preset(), &job(), &Edit { image: "src.png", references: &many }).is_ok());

        // FLUX.2: every picture is a reference latent on both sides, over an empty latent.
        let w = edit_workflow(&Family::Flux2.preset(), &job(), &edit).unwrap();
        assert_well_formed(&w);
        let c = classes(&w);
        assert_eq!((c.iter().filter(|x| *x == "ReferenceLatent").count(), c.iter().filter(|x| *x == "VAEEncode").count()), (6, 3));
        assert!(c.iter().any(|x| x == "EmptyFlux2LatentImage") && !c.iter().any(|x| x == "SplitSigmasDenoise" || x == "SetLatentNoiseMask"));
        let guider = node(&w, "CFGGuider");
        for side in ["positive", "negative"] {
            assert_eq!(w[guider["inputs"][side][0].as_str().unwrap()]["class_type"], "ReferenceLatent", "{side}");
        }

        // Every family either edits or says in words that it can't; hostile numbers stay out.
        for f in Family::ALL {
            let mut m = f.preset();
            (m.cfg, m.guidance, m.steps) = (f64::NAN, f64::INFINITY, u32::MAX);
            match edit_workflow(&m, &job(), &Edit { image: "src.png", references: &[] }) {
                Ok(w) => {
                    assert!(f.edits() && check_edit(&f.preset()).is_ok() && !w.to_string().contains("null"), "{f:?}");
                    assert_eq!(classes(&w).iter().filter(|x| *x == "LoadImage").count(), 1, "{f:?}");
                }
                Err(e) => assert!(!f.edits() && e.to_string().contains("can't edit") && check_edit(&f.preset()).is_err(), "{f:?}: {e}"),
            }
        }
        // A missing file is named.
        let mut m = Family::QwenImage21.preset();
        m.vae.clear();
        assert!(check_edit(&m).unwrap_err().to_string().contains("VAE"));
        assert_eq!(upload_reference(0), "photocraft_edit_reference_1.png");
        assert_eq!(upload_reference(usize::MAX), format!("photocraft_edit_reference_{}.png", usize::MAX));
    }

    #[test]
    fn custom_workflows_inpaint_when_they_take_an_image() {
        let mut m = Model { family: Family::Custom, name: "Mine".into(), ..Model::default() };
        m.workflow =
            r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"%prompt%"}},"2":{"class_type":"KSampler","inputs":{"denoise":"%denoise%"}}}"#.into();
        let e = inpaint_workflow(&m, &job(), &INPAINT).unwrap_err().to_string();
        assert!(e.contains("%image%") && check(&m, true).is_err() && check(&m, false).is_ok(), "{e}");
        assert_eq!(workflow(&m, &job()).unwrap()["2"]["inputs"]["denoise"], 1.0);
        m.workflow = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"%prompt%"}},"2":{"class_type":"KSampler","inputs":{"denoise":"%denoise%"}},
            "3":{"class_type":"LoadImage","inputs":{"image":"%image%"}},"4":{"class_type":"LoadImage","inputs":{"image":"%mask%"}}}"#
            .into();
        let w = inpaint_workflow(&m, &job(), &INPAINT).unwrap();
        assert_eq!(
            (w["3"]["inputs"]["image"].as_str(), w["4"]["inputs"]["image"].as_str(), w["2"]["inputs"]["denoise"].as_f64()),
            (Some("src.png"), Some("mask.png"), Some(0.6))
        );
    }

    #[test]
    fn uploads_are_multipart_and_their_answers_name_the_file() {
        let r = upload_request("http://h:1", UPLOAD_IMAGE, b"PNGDATA");
        assert_eq!(r.url, "http://h:1/upload/image");
        let body = String::from_utf8_lossy(&r.body).into_owned();
        assert!(body.contains("name=\"image\"; filename=\"photocraft_inpaint_image.png\"") && body.contains("PNGDATA"), "{body}");
        assert!(body.contains("name=\"overwrite\"\r\n\r\ntrue") && body.contains("name=\"type\"\r\n\r\ninput"), "{body}");
        assert_eq!(parse_uploaded(200, br#"{"name":"a.png","subfolder":"","type":"input"}"#).unwrap(), "a.png");
        assert_eq!(parse_uploaded(200, br#"{"name":"a.png","subfolder":"pc","type":"input"}"#).unwrap(), "pc/a.png");
        assert!(parse_uploaded(400, b"no image").unwrap_err().to_string().contains("HTTP 400"));
        for bad in [&b"{}"[..], br#"{"name":7}"#, br#"{"name":""}"#, b"[]", b"<html>"] {
            assert!(parse_uploaded(200, bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
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
