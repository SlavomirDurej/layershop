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
        return Err(Error::Config(format!("{label}: choose {what} in Preferences › Generative AI")));
    }
    Ok(name)
}

/// The text encoder loader's `type` for families with a single encoder.
fn clip_type(family: Family) -> &'static str {
    match family {
        Family::ZImage => "lumina2",
        Family::Krea2 => "krea2",
        Family::Flux2 | Family::Ernie => "flux2",
        Family::Flux3 => "flux3",
        Family::MossImage => "moss_image",
        Family::Ideogram4 => "ideogram4",
        Family::Fibo2 => "fibo2",
        // Ming Image's encoder is recognised by its weights; its templates load it as this type.
        Family::QwenImage | Family::QwenImage21 | Family::MingImage | Family::QwenInpaint | Family::QwenEdit => "qwen_image",
        _ => "stable_diffusion",
    }
}

fn latent_node(family: Family) -> &'static str {
    match family {
        Family::ZImage | Family::Flux1 | Family::HidreamI1 | Family::QwenImage => "EmptySD3LatentImage",
        Family::Flux3 | Family::Flux2 | Family::Ernie | Family::Ideogram4 | Family::MossImage | Family::Fibo2 => "EmptyFlux2LatentImage",
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
    /// Where the model may paint (transparent, as an inpainting's mask; it may be soft). `None`:
    /// all of the picture.
    pub mask: Option<&'a str>,
    /// With a mask: what the model is shown as image 1 instead of `image`, which is still what
    /// it paints over ([`crate::Source::shown`]). Ignored without a mask.
    pub shown: Option<&'a str>,
    /// Further pictures, which the instruction calls image 2, image 3, … ([`Family::max_references`]
    /// at most).
    pub references: &'a [String],
}

/// The names the image and mask of an inpainting are uploaded under. They are overwritten by
/// the next one, so ComfyUI's input folder holds two files of ours, not one pair per generation.
pub const UPLOAD_IMAGE: &str = "photocraft_inpaint_image.png";
pub const UPLOAD_MASK: &str = "photocraft_inpaint_mask.png";
/// The name what a masked edit's model is shown of the picture is uploaded under, when that
/// isn't the picture itself ([`Edit::shown`]; overwritten likewise).
pub const UPLOAD_SHOWN: &str = "photocraft_edit_shown.png";
/// The name the `index`th reference picture of an edit is uploaded under (overwritten likewise).
pub fn upload_reference(index: usize) -> String {
    format!("photocraft_edit_reference_{}.png", index.saturating_add(1))
}

/// The phrases Ming Image was trained to answer with a transparent background (its authors'
/// list). One of them goes first in the prompt; they don't combine. None is dependable: the
/// background came out transparent in 14 of 20 tries above one megapixel, by prompt and seed,
/// and no phrase did better than the first.
const MING_TRANSPARENT: [&str; 10] = [
    "RGBA, 4-channel, transparent background",
    "isolated subject, alpha matte, no background",
    "cutout PNG, alpha=0 outside the object",
    "transparent canvas, not white, not checkerboard",
    "production RGBA layer for compositing",
    "带透明通道，4通道RGBA图像",
    "透明背景，alpha通道，无底图",
    "抠图素材，背景alpha=0",
    "孤立主体，透明PNG图层",
    "不要白底，不要棋盘格，只要透明通道",
];

/// Is `job` a transparent background that `model`'s workflow can ask for? Ming Image generating
/// from the prompt alone: given a picture it answers opaque whatever the prompt says (measured
/// with an edit at 1024 and 1536 px).
fn transparent(model: &Model, job: &Job, from_picture: bool) -> bool {
    model.family == Family::MingImage && job.alpha && !from_picture
}

/// `job`'s prompt as a text-to-image with `model` is given it: for MOSS Image, a layout prompt
/// ([`moss_layout`]); for a transparent background from Ming Image, behind the phrase that asks
/// for one, unless the prompt starts with one itself.
fn prompt_for(model: &Model, job: &Job) -> String {
    if model.family == Family::MossImage {
        return moss_layout(&job.prompt, job.width, job.height);
    }
    let prompt = job.prompt.trim_start();
    let lower = prompt.to_lowercase();
    if !transparent(model, job, false) || MING_TRANSPARENT.iter().any(|phrase| lower.starts_with(&phrase.to_lowercase())) {
        return job.prompt.clone();
    }
    format!("{}, {prompt}", MING_TRANSPARENT[0])
}

/// How far in from the canvas's edges the box of a typed prompt's subject is, as a fraction of
/// each edge. At 0 the subject filled the canvas but was cropped by it (a car lost its wheels,
/// 2026-10-18); at 5% it filled it whole. Without a box it sat small in the middle.
const MOSS_INSET: f64 = 0.05;

/// Is `prompt` a MOSS Image layout prompt already: a JSON object with a `summary` or `items`?
fn moss_layout_of(prompt: &str) -> Option<Map<String, Value>> {
    let trimmed = prompt.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(o)) if o.get("summary").is_some_and(Value::is_object) || o.get("items").is_some_and(Value::is_array) => Some(o),
        _ => None,
    }
}

/// `prompt` as MOSS Image's layout prompt for a `width` × `height` canvas (its authors' JSON, as
/// ComfyUI's MOSS-Image Layout Prompt node writes it): the prompt describes the whole picture and
/// one object filling the canvas, in a box [`MOSS_INSET`] in from its edges, so what is asked for
/// fills the selection it is generated for. A prompt that is a layout already goes as written.
pub fn moss_layout(prompt: &str, width: u32, height: u32) -> String {
    if moss_layout_of(prompt).is_some() {
        return prompt.trim().to_string();
    }
    let caption = prompt.trim();
    let (w, h) = (i64::from(width), i64::from(height));
    let (inset_x, inset_y) = ((w as f64 * MOSS_INSET).round() as i64, (h as f64 * MOSS_INSET).round() as i64);
    // Pixels from the canvas's centre, [y0, x0, y1, x1].
    let bbox = [inset_y - h / 2, inset_x - w / 2, h - h / 2 - inset_y, w - w / 2 - inset_x];
    json!({
        "summary": {"caption": caption},
        "items": [{"label": "object", "caption": caption, "bbox": bbox}],
        "bbox_coord_space": "abs_centralized",
        "bbox_format": "yxyx",
    })
    .to_string()
}

/// A layout prompt written for a `from` canvas (the selection, in its pixels) with its boxes
/// scaled to the `to` canvas the model renders, which differs when the selection is outside the
/// sizes MOSS Image renders well or not a multiple of 16. Any other prompt is returned as it is.
pub fn moss_scaled(prompt: &str, from: (u32, u32), to: (u32, u32)) -> String {
    let Some(mut layout) = moss_layout_of(prompt) else { return prompt.to_string() };
    if from == to || from.0 == 0 || from.1 == 0 {
        return prompt.to_string();
    }
    let (sx, sy) = (f64::from(to.0) / f64::from(from.0), f64::from(to.1) / f64::from(from.1));
    let Some(Value::Array(items)) = layout.get_mut("items") else { return prompt.to_string() };
    for item in items.iter_mut() {
        let Some(Value::Array(bbox)) = item.get_mut("bbox") else { continue };
        if bbox.len() != 4 {
            continue;
        }
        // [y0, x0, y1, x1]: even places are rows, odd ones columns.
        for (i, v) in bbox.iter_mut().enumerate() {
            if let Some(n) = v.as_f64().filter(|n| n.is_finite()) {
                let scaled = (n * if i % 2 == 0 { sy } else { sx }).round().clamp(-1e9, 1e9) as i64;
                *v = json!(scaled);
            }
        }
    }
    Value::Object(layout).to_string()
}

/// The nodes that decode `samples` into the image to show. Ming Image's VAE decodes four
/// channels, and an image that was not asked to be transparent still comes back with pixels
/// short of opaque (alpha down to two thirds, measured): its alpha is dropped unless `alpha`.
fn decode(g: &mut Graph, model: &Model, samples: &Link, vae: &Link, alpha: bool) -> Link {
    let image = g.node("VAEDecode", json!({"samples": samples, "vae": vae}));
    if model.family == Family::MingImage && !alpha { g.node("SplitImageWithAlpha", json!({"image": image})) } else { image }
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
/// - Ming Image: its encoder takes the pictures with the prompt, and the model is sampled with
///   the LCM sampler of ComfyUI's Image Edit template.
/// - FLUX.2: each picture is a reference latent on the conditioning.
/// - FIBO 2: its encoder takes the pictures with the prompt and gives the latent to sample, at
///   the size asked for (ComfyUI's FIBO 2 Edit template).
///
/// With a mask the sampler starts from the encoded picture and denoises only the masked part
/// (Set Latent Noise Mask, through Differential Diffusion for a soft mask), as an inpainting
/// does: the model is still shown the picture whole, but what it paints is composed inside the
/// mask instead of anywhere in the picture.
pub fn edit_workflow(model: &Model, job: &Job, edit: &Edit<'_>) -> Result<Value> {
    build_edit(model, job, edit)
}

/// Can `model` edit a picture? The error says what to fix.
pub fn check_edit(model: &Model) -> Result<()> {
    let job = Job { prompt: "x".into(), width: 1024, height: 1024, seed: 0, alpha: false, source: None };
    build_edit(model, &job, &Edit { image: "image", mask: None, shown: None, references: &[] }).map(|_| ())
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
        return Err(Error::Config(format!("{name} is an inpainting model: it repaints a selection (the Edit tab) and can't generate from a prompt alone")));
    }
    if f.edit_only() {
        let name = if model.name.is_empty() { f.label() } else { model.name.as_str() };
        return Err(Error::Config(format!("{name} is an edit model: it changes a picture following an instruction (the Edit tab) and does nothing else")));
    }
    let mut g = Graph::default();
    let (mut unet, clip, vae) = loaders(&mut g, model)?;
    let cfg = finite(model.cfg, 1.0);
    // HiDream O1's prompt writer and refining pass are for a picture made from a prompt alone.
    let polished = f == Family::HidreamO1 && inpaint.is_none();
    let mut text = json!(job.prompt);
    let (mut positive, mut negative) = if f == Family::QwenImage21 {
        // Its own node encodes both prompts (and reference images, of which there are none here).
        let id = g.add(
            "TextEncodeQwenImage21",
            json!({"clip": clip, "prompt": job.prompt, "negative_prompt": model.negative_prompt, "resolution": 1024, "vae": vae}),
        );
        (json!([id, 0]), json!([id, 1]))
    } else if f == Family::Fibo2 {
        // Its own node encodes both prompts; without pictures its size only sizes the latent it
        // gives, which [`latent_node`] gives as well.
        let id = g.add(
            "TextEncodeFibo2",
            json!({"clip": clip, "prompt": job.prompt, "negative_prompt": model.negative_prompt, "width": job.width, "height": job.height, "batch_size": 1}),
        );
        (json!([id, 0]), json!([id, 1]))
    } else {
        let prompt = if inpaint.is_none() { prompt_for(model, job) } else { job.prompt.clone() };
        text = if polished { enhanced(&mut g, model, job, &prompt) } else { json!(prompt) };
        let positive = g.node("CLIPTextEncode", json!({"text": text, "clip": clip}));
        // Without classifier-free guidance the negative prompt is never evaluated: a zeroed
        // conditioning saves encoding it. Ideogram 4's negative model takes one, too.
        let negative = if f == Family::MingImage && cfg <= 1.0 {
            // Its guider then takes the prompt alone ([`sample`]): there is nothing to zero.
            positive.clone()
        } else if cfg <= 1.0 || f == Family::Ideogram4 {
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
                    if matches!(f, Family::Flux3 | Family::Flux2) {
                        positive = g.node("ReferenceLatent", json!({"conditioning": positive, "latent": encoded}));
                        negative = g.node("ReferenceLatent", json!({"conditioning": negative, "latent": encoded}));
                    }
                    g.node("SetLatentNoiseMask", json!({"samples": encoded, "mask": mask}))
                }
            }
        }
        None => g.node(latent_node(f), json!({"width": job.width, "height": job.height, "batch_size": 1})),
    };
    let samples = sample(&mut g, model, job, &Sampling { unet, positive, negative, latent, denoise, from_picture: inpaint.is_some() });
    let mut image = decode(&mut g, model, &samples, &vae, transparent(model, job, inpaint.is_some()));
    if polished && !model.refiner.trim().is_empty() {
        image = refined(&mut g, model, job, &image, &text)?;
    }
    g.add("PreviewImage", json!({"images": image}));
    Ok(Value::Object(g.nodes))
}

/// What HiDream O1's prompt writer is told. The model was trained on one descriptive paragraph
/// per picture (its authors' subject, composition, action, location, style, specs, text order):
/// given a list of instructions with an "avoid" section it draws waxy, posed people, and given
/// the same request as a description, candid ones (2026-10-13).
const ENHANCER_SYSTEM: &str = "You write prompts for a text-to-image model. Rewrite the user's request as one paragraph of plain, fluent English, 80 to 220 words, that describes the finished picture as if it already exists. Cover, in this order: the subject (who or what, their appearance, clothing and expression), the composition (shot size, viewpoint, where each thing is), the action, the location and its details, the image style, and the camera or rendering specifics (lens, depth of field, light, texture). If the request asks for text in the picture, give the exact words in double quotes and say where they are. Describe only what is seen: no instructions such as \"create\", \"should\" or \"make\", no lists, no headings, and no mention of what to avoid; turn anything to avoid into its positive opposite. For a photograph, ask for natural, unretouched skin texture and candid, unposed expressions. Keep every concrete detail the user gave and invent none that contradict it. Answer with the paragraph alone.";

/// `prompt` as `model`'s prompt writer rewrites it ([`Model::enhancer`], ComfyUI's Generate Text
/// node), or as it is without one. The seed makes the rewriting repeat with the picture's.
fn enhanced(g: &mut Graph, model: &Model, job: &Job, prompt: &str) -> Link {
    let file = model.enhancer.trim();
    if file.is_empty() {
        return json!(prompt);
    }
    let writer = g.node("CLIPLoader", json!({"clip_name": file, "type": "stable_diffusion", "device": "default"}));
    let system = g.node("PrimitiveStringMultiline", json!({"value": ENHANCER_SYSTEM}));
    g.node(
        "TextGenerate",
        json!({
            "clip": writer, "prompt": prompt, "system_prompt": system, "max_length": 512, "sampling_mode": "on",
            "sampling_mode.temperature": 0.7, "sampling_mode.top_k": 64, "sampling_mode.top_p": 0.95, "sampling_mode.min_p": 0.05,
            "sampling_mode.repetition_penalty": 1.05, "sampling_mode.seed": job.seed, "thinking": false, "use_default_template": true,
        }),
    )
}

/// `image` gone over by `model`'s refiner ([`Model::refiner`]): Z-Image Turbo, sampled as its
/// template samples it, from the picture and only as far from it as the strength says. `text`
/// is the prompt the picture was rendered from.
fn refined(g: &mut Graph, model: &Model, job: &Job, image: &Link, text: &Value) -> Result<Link> {
    let unet = g.node("UNETLoader", json!({"unet_name": model.refiner.trim(), "weight_dtype": "default"}));
    let unet = g.node("ModelSamplingAuraFlow", json!({"model": unet, "shift": 3.0}));
    let clip = g
        .node("CLIPLoader", json!({"clip_name": file(&model.refiner_clip, "the refiner's text encoder file", model)?, "type": "lumina2", "device": "default"}));
    let vae = g.node("VAELoader", json!({"vae_name": file(&model.refiner_vae, "the refiner's VAE file", model)?}));
    let positive = g.node("CLIPTextEncode", json!({"text": text, "clip": clip}));
    let negative = g.node("ConditioningZeroOut", json!({"conditioning": positive}));
    let latent = g.node("VAEEncode", json!({"pixels": image, "vae": vae}));
    let samples = g.node(
        "KSampler",
        json!({
            "model": unet, "seed": job.seed, "steps": 8, "cfg": 1.0, "sampler_name": "res_multistep", "scheduler": "simple",
            "positive": positive, "negative": negative, "latent_image": latent, "denoise": finite(model.refine_strength, 0.35).clamp(0.05, 1.0),
        }),
    );
    Ok(g.node("VAEDecode", json!({"samples": samples, "vae": vae})))
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
    // ComfyUI's MOSS Image templates: the negative side of the guidance skips layer 22 over the
    // first half of the schedule (its authors' setting), which keeps detail with CFG.
    if f == Family::MossImage {
        unet =
            g.node("SkipLayerGuidanceDiTSimple", json!({"model": unet, "double_layers": "", "single_layers": "22", "start_percent": 0.0, "end_percent": 0.5}));
    }
    // ComfyUI's HiDream O1 template: the model works on pixels in patches, and this blends
    // shifted passes over the last fifth of the schedule so their edges don't show. Its Dev
    // template has none.
    if f == Family::HidreamO1 && !crate::detect::hidream_o1_dev(&model.checkpoint) {
        unet = g.node(
            "HiDreamO1PatchSeamSmoothing",
            json!({"model": unet, "start_percent": 0.8, "end_percent": 1.0, "pattern": "single_shift", "passes": "ramp_2_4", "blend": "median", "strength": 1.0}),
        );
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
    /// The model is given a picture: to start from (inpainting, image to image) or to edit.
    from_picture: bool,
}

/// The nodes that sample `model` (its family's sampler, with the model's steps, CFG, sampler and
/// scheduler), and the latent they give.
fn sample(g: &mut Graph, model: &Model, job: &Job, with: &Sampling) -> Link {
    let f = model.family;
    let Sampling { unet, positive, negative, latent, denoise, from_picture } = with;
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
        // ComfyUI's Ming Image templates: FLUX's shift, which follows the size, and a guider
        // that takes the prompt alone (the model is distilled; CFG above 1 brings the negative
        // prompt back).
        Family::MingImage => {
            let unet = g.node("ModelSamplingFlux", json!({"model": unet, "max_shift": 1.15, "base_shift": 0.5, "width": job.width, "height": job.height}));
            let noise = g.node("RandomNoise", json!({"noise_seed": job.seed}));
            // Given a picture it needs the sampler of its Image Edit template. With the
            // text-to-image one (euler) an edit, an image to image and an inpainting at less
            // than full strength all came out speckled (measured against ComfyUI 0.39).
            let select = if *from_picture {
                g.node("SamplerLCM", json!({"s_noise": 1.02, "s_noise_end": 1.0, "noise_clip_std": 0.0}))
            } else {
                g.node("KSamplerSelect", json!({"sampler_name": sampler}))
            };
            let sigmas = g.node("BasicScheduler", json!({"model": unet, "scheduler": scheduler, "steps": steps, "denoise": denoise}));
            let guider = if cfg > 1.0 {
                g.node("CFGGuider", json!({"model": unet, "positive": positive, "negative": negative, "cfg": cfg}))
            } else {
                g.node("BasicGuider", json!({"model": unet, "conditioning": positive}))
            };
            g.node("SamplerCustomAdvanced", json!({"noise": noise, "guider": guider, "sampler": select, "sigmas": sigmas, "latent_image": latent}))
        }
        // ComfyUI's HiDream O1 Dev template: the LCM sampler with its noise clipped, which the
        // sampler of that name in KSampler does not do.
        Family::HidreamO1 if sampler == "lcm" => {
            let select = g.node("SamplerLCM", json!({"s_noise": 1.0, "s_noise_end": 1.0, "noise_clip_std": 2.5}));
            let sigmas = g.node("BasicScheduler", json!({"model": unet, "scheduler": scheduler, "steps": steps, "denoise": denoise}));
            g.node(
                "SamplerCustom",
                json!({
                    "model": unet, "add_noise": true, "noise_seed": job.seed, "cfg": cfg, "positive": positive, "negative": negative,
                    "sampler": select, "sigmas": sigmas, "latent_image": latent,
                }),
            )
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
            "{name} can't edit a picture from an instruction: choose an edit model (Qwen Image 2.1, FIBO 2, Ming Image, Qwen Image Edit or FLUX.2) on the Edit tab"
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
    // The picture to edit first: it is "image 1" to the model, the references follow. With a
    // mask the model may be shown less of it than it paints over, but not FIBO 2: it scales a
    // single picture to the result's size and lines the result up with it pixel for pixel, so
    // part of the picture would be stretched over the whole.
    let image = g.node("LoadImage", json!({"image": edit.image}));
    let shown = edit.shown.filter(|_| edit.mask.is_some() && f != Family::Fibo2).map(|name| g.node("LoadImage", json!({"image": name})));
    let pictures: Vec<Link> = std::iter::once(shown.unwrap_or_else(|| image.clone()))
        .chain(edit.references.iter().map(|name| g.node("LoadImage", json!({"image": name}))))
        .collect();
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
        Family::Fibo2 => {
            // Its encoder shows the pictures to the text encoder and hands them to the model as
            // reference latents on both sides of the guidance. One picture is scaled and cropped
            // to the result's size, and the size asked for is the picture's, so nothing is
            // cropped; several are each taken at the size the model was trained at.
            let mut inputs = json!({
                "clip": clip, "prompt": job.prompt, "negative_prompt": model.negative_prompt, "vae": vae,
                "width": job.width, "height": job.height, "batch_size": 1,
            });
            for (i, picture) in pictures.iter().enumerate() {
                inputs[format!("images.image_{}", i + 1)] = picture.clone();
            }
            let id = g.add("TextEncodeFibo2", inputs);
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
        Family::MingImage => {
            // Its encoder shows the pictures to the text encoder and hands them to the model as
            // reference latents (ComfyUI's Image Edit template). The latent to sample is the
            // first picture's size, which is the size asked for.
            let encode = |g: &mut Graph, prompt: &str| {
                let mut inputs = json!({"clip": clip, "vae": vae, "prompt": prompt});
                for (i, picture) in pictures.iter().enumerate() {
                    inputs[format!("images.image_{}", i + 1)] = picture.clone();
                }
                g.node("TextEncodeMingImageEdit", inputs)
            };
            let positive = encode(&mut g, &job.prompt);
            // Only CFG above 1 evaluates a negative side ([`sample`]).
            let negative = if cfg > 1.0 { encode(&mut g, &model.negative_prompt) } else { positive.clone() };
            (positive, negative, g.node("EmptyLatentImage", json!({"width": job.width, "height": job.height, "batch_size": 1})))
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
    // Masked: the picture is the latent, and only the mask of it is painted afresh.
    let latent = match edit.mask {
        Some(mask) => {
            unet = g.node("DifferentialDiffusion", json!({"model": unet}));
            let mask = json!([g.add("LoadImage", json!({"image": mask})), 1]);
            let encoded = g.node("VAEEncode", json!({"pixels": image, "vae": vae}));
            g.node("SetLatentNoiseMask", json!({"samples": encoded, "mask": mask}))
        }
        None => latent,
    };
    let samples = sample(&mut g, model, job, &Sampling { unet, positive, negative, latent, denoise: 1.0, from_picture: true });
    let image = decode(&mut g, model, &samples, &vae, transparent(model, job, true));
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
        return Err(Error::Config(format!("{}: paste a workflow (ComfyUI › Workflow › Export (API)) in Preferences › Generative AI", model.name)));
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
        return Err(Error::Config(format!("the ComfyUI address `{url}` is not an http:// URL (Preferences › Generative AI)")));
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

/// What a person can do about a node's failure, for the ones that read as nothing to them.
fn what_to_do(detail: &str) -> Option<&'static str> {
    let lower = detail.to_ascii_lowercase();
    // Windows error 1455: memory and the paging file together ran out while a model loaded. The
    // paging file grows on its drive, and model downloads may just have filled that drive.
    (lower.contains("paging file") || lower.contains("os error 1455")).then_some(
        "Windows ran out of memory loading the model: the computer's memory and its paging file together are too small for it. \
Free some space on the drive that holds the paging file (usually C:), or let Windows manage the paging file size \
(Advanced system settings › Performance › Settings › Advanced › Virtual memory), then try again. A smaller model needs less memory.",
    )
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
        let fix = what_to_do(&detail).map_or_else(String::new, |fix| format!(" {fix}"));
        return Err(Error::Server(format!("ComfyUI: {}{fix}", clip(&detail, 400))));
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
    use crate::model::MOSS_NEGATIVE;

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
        // The template's graph, at its CFG of 1: the negative prompt is zeroed out, not encoded.
        let w = workflow(&Model { cfg: 1.0, ..Family::ZImage.preset() }, &job()).unwrap();
        let c = classes(&w);
        for want in ["UNETLoader", "CLIPLoader", "VAELoader", "ModelSamplingAuraFlow", "ConditioningZeroOut", "EmptySD3LatentImage", "KSampler", "VAEDecode"] {
            assert!(c.iter().any(|x| x == want), "missing {want}: {c:?}");
        }
        // The preset's CFG of 2 gives the sampler a negative prompt to steer away from.
        let preset = workflow(&Family::ZImage.preset(), &job()).unwrap();
        assert!(!classes(&preset).iter().any(|x| x == "ConditioningZeroOut"));
        assert_eq!(preset.as_object().unwrap().values().find(|n| n["class_type"] == "KSampler").unwrap()["inputs"]["cfg"], 2.0);
        let sampler = w.as_object().unwrap().values().find(|n| n["class_type"] == "KSampler").unwrap();
        assert_eq!(sampler["inputs"]["seed"], 42);
        assert_eq!(sampler["inputs"]["steps"], 8);
        assert_eq!(sampler["inputs"]["sampler_name"], "res_multistep");
    }

    fn nodes_with(w: &Value, class: &str) -> Vec<Value> {
        w.as_object().unwrap().values().filter(|n| n["class_type"] == class).cloned().collect()
    }

    /// The id of the refining sampler: the KSampler that starts from an encoded picture.
    fn refine_id(w: &Value) -> Value {
        let nodes = w.as_object().unwrap();
        let (id, _) = nodes
            .iter()
            .find(|(_, n)| n["class_type"] == "KSampler" && nodes[n["inputs"]["latent_image"][0].as_str().unwrap()]["class_type"] == "VAEEncode")
            .unwrap();
        json!(id)
    }

    #[test]
    fn hidream_o1_follows_its_templates() {
        let has = |w: &Value, class: &str| classes(w).iter().any(|x| x == class);
        let node = |w: &Value, class: &str| w.as_object().unwrap().values().find(|n| n["class_type"] == class).cloned().unwrap();

        // The full model: noise scale 8, seams smoothed, KSampler at the template's settings.
        let full = Family::HidreamO1.preset();
        let w = workflow(&full, &job()).unwrap();
        assert_well_formed(&w);
        let sampler = &node(&w, "KSampler")["inputs"];
        assert_eq!(
            (&sampler["steps"], &sampler["cfg"], &sampler["sampler_name"], &sampler["scheduler"]),
            (&json!(40), &json!(5.0), &json!("dpmpp_2m_sde_gpu"), &json!("normal"))
        );
        let smoothing = &w[sampler["model"][0].as_str().unwrap()];
        assert_eq!((&smoothing["class_type"], &smoothing["inputs"]["passes"]), (&json!("HiDreamO1PatchSeamSmoothing"), &json!("ramp_2_4")));
        let scale = &w[smoothing["inputs"]["model"][0].as_str().unwrap()];
        assert_eq!((&scale["class_type"], &scale["inputs"]["noise_scale"]), (&json!("ModelNoiseScale"), &json!(8.0)));
        assert!(has(&w, "EmptyHiDreamO1LatentImage"));
        // Rendered at the trained size closest in shape, whatever render size a saved model has.
        let o1 = Family::HidreamO1;
        assert_eq!((o1.render_size(500, 500, full.megapixels), o1.render_size(1920, 1080, 1.0)), ((2048, 2048), (2560, 1440)));
        assert_eq!((o1.render_size(600, 900, 1.0), o1.render_size(4000, 500, 2.0)), ((1664, 2496), (3104, 1312)));
        // 0 is the selection's own size, and no other family is touched.
        assert_eq!((o1.render_size(1030, 770, 0.0), Family::ZImage.render_size(500, 500, 1.0)), ((1024, 768), (1008, 1008)));
        assert_eq!(o1.render_size(0, 0, f64::NAN), (256, 256));

        // Dev: its template's LCM sampler with clipped noise, and no smoothing.
        let dev =
            Model { checkpoint: "hidream_o1_image_dev_fp8_scaled.safetensors".into(), steps: 28, cfg: 1.0, shift: 7.6, sampler: "lcm".into(), ..full.clone() };
        let w = workflow(&dev, &job()).unwrap();
        assert_well_formed(&w);
        assert!(!has(&w, "HiDreamO1PatchSeamSmoothing") && !has(&w, "KSampler") && has(&w, "ConditioningZeroOut"), "{:?}", classes(&w));
        assert_eq!(node(&w, "SamplerLCM")["inputs"]["noise_clip_std"], json!(2.5));
        let custom = &node(&w, "SamplerCustom")["inputs"];
        assert_eq!((&custom["cfg"], &custom["noise_seed"], &custom["add_noise"]), (&json!(1.0), &json!(42), &json!(true)));
        assert_eq!((&node(&w, "BasicScheduler")["inputs"]["steps"], &node(&w, "ModelNoiseScale")["inputs"]["noise_scale"]), (&json!(28), &json!(7.6)));
        // A prompt writer rewrites the prompt for both models that read it, with the picture's
        // seed; a refiner goes over the decoded picture, and it is the refined one that is shown.
        let polished = Model {
            enhancer: "gemma4_e4b_it_fp8_scaled.safetensors".into(),
            refiner: "z_image_turbo_bf16.safetensors".into(),
            refiner_clip: "qwen_3_4b.safetensors".into(),
            refiner_vae: "ae.safetensors".into(),
            ..full.clone()
        };
        let w = workflow(&polished, &job()).unwrap();
        assert_well_formed(&w);
        let nodes = w.as_object().unwrap();
        let (writer, generate) = nodes.iter().find(|(_, n)| n["class_type"] == "TextGenerate").unwrap();
        assert_eq!((&generate["inputs"]["prompt"], &generate["inputs"]["sampling_mode.seed"]), (&json!("a red frog"), &json!(42)));
        assert!(w[generate["inputs"]["system_prompt"][0].as_str().unwrap()]["inputs"]["value"].as_str().unwrap().contains("one paragraph"));
        let encodes: Vec<&Value> = nodes.values().filter(|n| n["class_type"] == "CLIPTextEncode").collect();
        assert_eq!(encodes.iter().filter(|n| n["inputs"]["text"] == json!([writer, 0])).count(), 2, "{encodes:?}");
        let refine = nodes.values().find(|n| n["class_type"] == "KSampler" && n["inputs"]["denoise"] == json!(0.35)).unwrap();
        assert_eq!((&refine["inputs"]["steps"], &refine["inputs"]["sampler_name"]), (&json!(8), &json!("res_multistep")));
        let encoded = &w[refine["inputs"]["latent_image"][0].as_str().unwrap()];
        assert_eq!((&encoded["class_type"], &w[encoded["inputs"]["pixels"][0].as_str().unwrap()]["class_type"]), (&json!("VAEEncode"), &json!("VAEDecode")));
        assert_eq!(w[node(&w, "PreviewImage")["inputs"]["images"][0].as_str().unwrap()]["inputs"]["samples"][0], refine_id(&w));
        // A refiner without its text encoder is refused in words; neither is used on a picture.
        let e = workflow(&Model { refiner_clip: String::new(), ..polished.clone() }, &job()).unwrap_err().to_string();
        assert!(e.contains("refiner's text encoder"), "{e}");
        let w = inpaint_workflow(&polished, &job(), &INPAINT).unwrap();
        assert!(!has(&w, "TextGenerate") && !has(&w, "UNETLoader"), "{:?}", classes(&w));
        // An absurd strength is brought into range.
        let w = workflow(&Model { refine_strength: f64::NAN, ..polished.clone() }, &job()).unwrap();
        assert!(nodes_with(&w, "KSampler").iter().any(|n| n["inputs"]["denoise"] == json!(0.35)));
        let w = workflow(&Model { refine_strength: 9.0, ..polished }, &job()).unwrap();
        assert!(nodes_with(&w, "KSampler").iter().all(|n| n["inputs"]["denoise"] == json!(1.0)));

        // Another sampler chosen for Dev is sampled as any other model is.
        let w = workflow(&Model { sampler: "euler".into(), ..dev }, &job()).unwrap();
        assert!(has(&w, "KSampler") && !has(&w, "SamplerCustom"));
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
    fn flux3_loads_as_its_template_does_and_does_not_edit() {
        let m = Family::Flux3.preset();
        let w = workflow(&m, &job()).unwrap();
        assert_well_formed(&w);
        let c = classes(&w);
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["type"], "flux3");
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["clip_name"], "qwen3vl_4b_int8_convrot.safetensors");
        assert_eq!(node(&w, "UNETLoader")["inputs"]["unet_name"], "flux3_image_klein_base_int8_convrot.safetensors");
        // The templates' plain sampler: no FLUX.2 scheduler, no distilled guidance, no shift.
        let k = node(&w, "KSampler");
        assert_eq!((&k["inputs"]["steps"], &k["inputs"]["cfg"], &k["inputs"]["sampler_name"]), (&json!(50), &json!(3.5), &json!("euler")));
        assert!(c.iter().any(|x| x == "EmptyFlux2LatentImage"));
        for absent in ["Flux2Scheduler", "FluxGuidance", "ModelSamplingAuraFlow", "ReferenceLatent"] {
            assert!(!c.iter().any(|x| x == absent), "{absent}: {c:?}");
        }
        // CFG above 1: the negative prompt is encoded.
        assert_eq!(c.iter().filter(|x| *x == "CLIPTextEncode").count(), 2);
        // Text to image only: its edits drew malformed animals ([`Family::edits`]).
        let e = edit_workflow(&m, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &[] }).unwrap_err().to_string();
        assert!(e.contains("FLUX.3 Klein can't edit"), "{e}");
        assert!(m.can_generate() && !m.can_edit() && !m.can_inpaint());
        // Inpainting the user turns on by hand still shows it the picture, as FLUX.2 is.
        let w = inpaint_workflow(&Model { inpaint: Some(true), ..m }, &job(), &INPAINT).unwrap();
        assert_eq!(classes(&w).iter().filter(|x| *x == "ReferenceLatent").count(), 2);
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
    fn moss_image_follows_its_templates_with_a_layout_prompt() {
        let m = Family::MossImage.preset();
        let w = workflow(&m, &job()).unwrap();
        assert_well_formed(&w);
        let c = classes(&w);
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["type"], "moss_image");
        assert_eq!(
            (&node(&w, "UNETLoader")["inputs"]["unet_name"], &node(&w, "VAELoader")["inputs"]["vae_name"]),
            (&json!("moss_image_sft_int8_convrot.safetensors"), &json!("flux2-vae_small.safetensors"))
        );
        // Skip layer guidance on layer 22 over the first half, sampled by KSampler at the template's settings.
        let slg = &node(&w, "SkipLayerGuidanceDiTSimple")["inputs"];
        assert_eq!(
            (&slg["double_layers"], &slg["single_layers"], &slg["start_percent"], &slg["end_percent"]),
            (&json!(""), &json!("22"), &json!(0.0), &json!(0.5))
        );
        let k = &node(&w, "KSampler")["inputs"];
        assert_eq!((&k["steps"], &k["cfg"], &k["sampler_name"], &k["scheduler"]), (&json!(25), &json!(2.5), &json!("er_sde"), &json!("simple")));
        assert_eq!(w[k["model"][0].as_str().unwrap()]["class_type"], "SkipLayerGuidanceDiTSimple");
        let latent = &node(&w, "EmptyFlux2LatentImage")["inputs"];
        assert_eq!((&latent["width"], &latent["height"]), (&json!(1024), &json!(768)));
        assert!(!c.iter().any(|x| x == "ModelSamplingAuraFlow" || x == "ConditioningZeroOut"), "{c:?}");
        // The prompt is a layout: the whole picture, and one object in a box 5% in from the edges.
        let texts: Vec<String> = nodes_with(&w, "CLIPTextEncode").iter().map(|n| n["inputs"]["text"].as_str().unwrap().to_string()).collect();
        assert!(texts.contains(&MOSS_NEGATIVE.to_string()), "{texts:?}");
        let layout: Value = serde_json::from_str(texts.iter().find(|t| t.starts_with('{')).unwrap()).unwrap();
        assert_eq!(layout["summary"]["caption"], "a red frog");
        assert_eq!(layout["items"], json!([{"label": "object", "caption": "a red frog", "bbox": [-346, -461, 346, 461]}]));
        assert_eq!((&layout["bbox_coord_space"], &layout["bbox_format"]), (&json!("abs_centralized"), &json!("yxyx")));
        // A layout typed as the prompt goes as written; an odd size still gives a box inside it.
        let own = r#"{"summary":{"caption":"poster"},"items":[{"label":"text","caption":"title","text":"EXPLORE","bbox":[-300,-200,-200,200]}]}"#;
        assert_eq!(
            moss_layout(
                &format!(
                    "  {own}
"
                ),
                1024,
                768
            ),
            own
        );
        let odd: Value = serde_json::from_str(&moss_layout("x", 17, 3)).unwrap();
        assert_eq!(odd["items"][0]["bbox"], json!([-1, -7, 2, 8]));
        // JSON that is not a layout, and text that only starts like it, are a prompt like any other.
        for prompt in [r#"{"a": 1}"#, "{not json", "[1, 2]"] {
            let v: Value = serde_json::from_str(&moss_layout(prompt, 64, 64)).unwrap();
            assert_eq!(v["summary"]["caption"], prompt);
        }
        // Text to image only.
        assert!(m.can_generate() && !m.can_edit() && !m.can_inpaint() && !m.alpha);
        assert!(edit_workflow(&m, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &[] }).unwrap_err().to_string().contains("MOSS Image can't edit"));
        // Image to image is given the prompt as typed: there is a picture to follow, not a canvas to lay out.
        let w = inpaint_workflow(&m, &job(), &Inpaint { image: "src.png", mask: None, denoise: 0.6 }).unwrap();
        assert!(nodes_with(&w, "CLIPTextEncode").iter().any(|n| n["inputs"]["text"] == "a red frog"));
    }

    #[test]
    fn moss_layouts_are_scaled_to_the_render() {
        let own = r#"{"summary":{"caption":"p"},"items":[{"label":"object","caption":"c","bbox":[-100,-200,100,200]},{"label":"text","bbox":[1,2,3]},{"bbox":["a",null,4.6,-8]},{"caption":"no box"}]}"#;
        let scaled: Value = serde_json::from_str(&moss_scaled(own, (400, 200), (800, 300))).unwrap();
        assert_eq!(scaled["items"][0]["bbox"], json!([-150, -400, 150, 400]));
        // What is not four numbers is left as it is.
        assert_eq!(scaled["items"][1]["bbox"], json!([1, 2, 3]));
        assert_eq!(scaled["items"][2]["bbox"], json!(["a", null, 7, -16]));
        assert_eq!(scaled["items"][3]["caption"], "no box");
        assert_eq!(scaled["summary"]["caption"], "p");
        // The same size, an empty one, and a prompt that is not a layout are returned unchanged.
        assert_eq!(moss_scaled(own, (400, 200), (400, 200)), own);
        assert_eq!(moss_scaled(own, (0, 200), (400, 200)), own);
        assert_eq!(moss_scaled("a frog", (400, 200), (800, 300)), "a frog");
        let huge = r#"{"items":[{"bbox":[1e308,-1e308,1,1]}]}"#;
        assert!(moss_scaled(huge, (1, 1), (4096, 4096)).contains("1000000000"));
    }

    #[test]
    fn ming_image_follows_its_templates() {
        let ming = Family::MingImage.preset();
        let has = |w: &Value, class: &str| classes(w).iter().any(|x| x == class);

        // Text to image: FLUX's shift at the size asked for, a guider with the prompt alone, euler.
        let w = workflow(&ming, &job()).unwrap();
        assert_well_formed(&w);
        for want in [
            "ModelSamplingFlux",
            "CLIPTextEncode",
            "EmptyLatentImage",
            "RandomNoise",
            "KSamplerSelect",
            "BasicScheduler",
            "BasicGuider",
            "SamplerCustomAdvanced",
        ] {
            assert!(has(&w, want), "missing {want}: {:?}", classes(&w));
        }
        for absent in ["ConditioningZeroOut", "CFGGuider", "SamplerLCM", "KSampler", "LoadImage"] {
            assert!(!has(&w, absent), "{absent} in {:?}", classes(&w));
        }
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["type"], "qwen_image");
        let shift = &node(&w, "ModelSamplingFlux")["inputs"];
        assert_eq!((&shift["max_shift"], &shift["base_shift"], &shift["width"], &shift["height"]), (&json!(1.15), &json!(0.5), &json!(1024), &json!(768)));
        let scheduler = &node(&w, "BasicScheduler")["inputs"];
        assert_eq!((&scheduler["scheduler"], &scheduler["steps"], &scheduler["denoise"]), (&json!("simple"), &json!(12), &json!(1.0)));
        assert_eq!(node(&w, "KSamplerSelect")["inputs"]["sampler_name"], "euler");
        assert_eq!(node(&w, "CLIPTextEncode")["inputs"]["text"], "a red frog");
        // Its VAE decodes four channels: what is shown is the image without them, opaque.
        let shown = |w: &Value| w[node(w, "PreviewImage")["inputs"]["images"][0].as_str().unwrap()]["class_type"].as_str().unwrap().to_string();
        assert_eq!(shown(&w), "SplitImageWithAlpha");

        // A transparent background is asked for in the model's own words, and its alpha is kept.
        let alpha = Job { alpha: true, ..job() };
        let w = workflow(&ming, &alpha).unwrap();
        assert_eq!(node(&w, "CLIPTextEncode")["inputs"]["text"], "RGBA, 4-channel, transparent background, a red frog");
        assert_eq!(shown(&w), "VAEDecode");
        // A prompt that starts with one of the phrases already is left as it is.
        let own = Job { prompt: "Cutout PNG, alpha=0 outside the object. A frog".into(), ..alpha.clone() };
        assert_eq!(node(&workflow(&ming, &own).unwrap(), "CLIPTextEncode")["inputs"]["text"], "Cutout PNG, alpha=0 outside the object. A frog");
        // The words are Ming Image's: no other model is given them.
        assert_eq!(node(&workflow(&Family::ZImage.preset(), &alpha).unwrap(), "CLIPTextEncode")["inputs"]["text"], "a red frog");
        assert_eq!(shown(&workflow(&Family::ZImage.preset(), &alpha).unwrap()), "VAEDecode");

        // CFG above 1 brings the negative prompt back, through a CFG guider.
        let guided = Model { cfg: 3.0, negative_prompt: "blurry".into(), ..ming.clone() };
        let w = workflow(&guided, &job()).unwrap();
        assert_well_formed(&w);
        let guider = &node(&w, "CFGGuider")["inputs"];
        assert_eq!((&guider["cfg"], &w[guider["negative"][0].as_str().unwrap()]["inputs"]["text"]), (&json!(3.0), &json!("blurry")));
        assert!(!has(&w, "BasicGuider"));
    }

    #[test]
    fn ming_image_follows_its_edit_templates() {
        let ming = Family::MingImage.preset();
        let has = |w: &Value, class: &str| classes(w).iter().any(|x| x == class);
        let count = |w: &Value, class: &str| classes(w).iter().filter(|x| *x == class).count();
        let shown = |w: &Value| w[node(w, "PreviewImage")["inputs"]["images"][0].as_str().unwrap()]["class_type"].as_str().unwrap().to_string();
        let alpha = Job { alpha: true, ..job() };
        let guided = Model { cfg: 3.0, negative_prompt: "blurry".into(), ..ming.clone() };

        // Given a picture it is sampled with the LCM sampler of its edit template, and answers
        // opaque: the phrase is for text to image.
        for source in [&INPAINT, &IMG2IMG] {
            let w = inpaint_workflow(&ming, &alpha, source).unwrap();
            assert_well_formed(&w);
            assert!(has(&w, "SamplerLCM") && !has(&w, "KSamplerSelect") && has(&w, "ModelSamplingFlux"), "{:?}", classes(&w));
            let lcm = &node(&w, "SamplerLCM")["inputs"];
            assert_eq!((&lcm["s_noise"], &lcm["s_noise_end"], &lcm["noise_clip_std"]), (&json!(1.02), &json!(1.0), &json!(0.0)));
            assert_eq!(node(&w, "BasicScheduler")["inputs"]["denoise"], 0.6);
            assert_eq!(node(&w, "CLIPTextEncode")["inputs"]["text"], "a red frog");
            assert_eq!(shown(&w), "SplitImageWithAlpha");
        }

        // An edit, as ComfyUI's Image Edit template: its encoder takes the pictures (and the VAE,
        // to hand them to the model), over an empty latent of the size asked for.
        let references = ["logo.png".to_string()];
        let w = edit_workflow(&ming, &alpha, &Edit { image: "src.png", mask: None, shown: None, references: &references }).unwrap();
        assert_well_formed(&w);
        let nodes = w.as_object().unwrap();
        let (id, encode) = nodes.iter().find(|(_, n)| n["class_type"] == "TextEncodeMingImageEdit").unwrap();
        assert_eq!(encode["inputs"]["prompt"], "a red frog");
        assert!(encode["inputs"]["clip"].is_array() && encode["inputs"]["vae"].is_array());
        let picture = |i: usize| encode["inputs"].get(format!("images.image_{i}")).map(|link| w[link[0].as_str().unwrap()]["inputs"]["image"].clone());
        assert_eq!((picture(1), picture(2), picture(3)), (Some(json!("src.png")), Some(json!("logo.png")), None));
        assert_eq!(node(&w, "BasicGuider")["inputs"]["conditioning"], json!([id, 0]));
        let latent = &node(&w, "EmptyLatentImage")["inputs"];
        assert_eq!((&latent["width"], &latent["height"]), (&json!(1024), &json!(768)));
        assert!(has(&w, "SamplerLCM") && has(&w, "ModelSamplingFlux") && !has(&w, "CLIPTextEncode") && !has(&w, "VAEEncode"), "{:?}", classes(&w));
        assert_eq!((count(&w, "TextEncodeMingImageEdit"), shown(&w).as_str()), (1, "SplitImageWithAlpha"));
        // With CFG both sides of the guidance see the pictures.
        let w = edit_workflow(&guided, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &[] }).unwrap();
        assert_well_formed(&w);
        assert_eq!((count(&w, "TextEncodeMingImageEdit"), has(&w, "CFGGuider")), (2, true));
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
            let references = if matches!(f, Family::Flux3 | Family::Flux2) { 2 } else { 0 };
            assert_eq!(c.iter().filter(|x| *x == "ReferenceLatent").count(), references, "{f:?}");
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
        let edit = Edit { image: "src.png", mask: None, shown: None, references: &references };
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
        let e = edit_workflow(&Family::QwenEdit.preset(), &job(), &Edit { image: "src.png", mask: None, shown: None, references: &many }).unwrap_err().to_string();
        assert!(e.contains("2 reference pictures"), "{e}");
        assert!(edit_workflow(&Family::QwenImage21.preset(), &job(), &Edit { image: "src.png", mask: None, shown: None, references: &many }).is_ok());

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
            match edit_workflow(&m, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &[] }) {
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
    fn a_masked_edit_starts_from_the_picture_and_paints_only_the_mask() {
        let references = ["shirt.png".to_string()];
        let edit = Edit { image: "src.png", mask: Some("mask.png"), shown: None, references: &references };
        for f in [Family::QwenImage21, Family::QwenEdit, Family::MingImage, Family::Flux2, Family::Fibo2] {
            let w = edit_workflow(&f.preset(), &job(), &edit).unwrap();
            assert_well_formed(&w);
            let nodes = w.as_object().unwrap();
            let linked = |v: &Value| &nodes[v[0].as_str().unwrap()];
            // Whichever sampler the family has starts from the encoded picture under the mask.
            let sampler = nodes.values().find(|n| n["inputs"].get("latent_image").is_some()).unwrap();
            let masked = linked(&sampler["inputs"]["latent_image"]);
            assert_eq!(masked["class_type"], "SetLatentNoiseMask", "{f:?}");
            let encoded = linked(&masked["inputs"]["samples"]);
            assert_eq!(
                (encoded["class_type"].as_str(), linked(&encoded["inputs"]["pixels"])["inputs"]["image"].as_str()),
                (Some("VAEEncode"), Some("src.png")),
                "{f:?}"
            );
            // The mask is the second output of its Load Image (where the file is transparent).
            assert_eq!(
                (masked["inputs"]["mask"][1].as_u64(), linked(&masked["inputs"]["mask"])["inputs"]["image"].as_str()),
                (Some(1), Some("mask.png")),
                "{f:?}"
            );
            // A soft mask is honoured, and the model still sees the picture and its reference.
            let c = classes(&w);
            assert!(c.iter().any(|x| x == "DifferentialDiffusion"), "{f:?}: {c:?}");
            assert_eq!(c.iter().filter(|x| *x == "LoadImage").count(), 3, "{f:?}");
            assert!(w.to_string().contains("shirt.png") && !w.to_string().contains("null"), "{f:?}");
        }
        // No mask, no noise mask: the picture is rendered afresh as before.
        let w = edit_workflow(&Family::QwenImage21.preset(), &job(), &Edit { mask: None, ..edit }).unwrap();
        assert!(!classes(&w).iter().any(|x| x == "SetLatentNoiseMask" || x == "DifferentialDiffusion"));
    }

    #[test]
    fn a_masked_edit_can_show_the_model_less_than_it_paints_over() {
        let references = ["shirt.png".to_string()];
        let edit = Edit { image: "src.png", mask: Some("mask.png"), shown: Some("shown.png"), references: &references };
        let w = edit_workflow(&Family::QwenImage21.preset(), &job(), &edit).unwrap();
        assert_well_formed(&w);
        let nodes = w.as_object().unwrap();
        let linked = |v: &Value| &nodes[v[0].as_str().unwrap()];
        let loaded = |v: &Value| linked(v)["inputs"]["image"].as_str().unwrap().to_string();
        // Image 1 is what it is shown, the reference is still image 2…
        let encode = node(&w, "TextEncodeQwenImage21");
        assert_eq!((loaded(&encode["inputs"]["images.image_1"]), loaded(&encode["inputs"]["images.image_2"])), ("shown.png".into(), "shirt.png".into()));
        // …and the picture is what it paints over.
        let masked = node(&w, "SetLatentNoiseMask");
        assert_eq!(loaded(&linked(&masked["inputs"]["samples"])["inputs"]["pixels"]), "src.png");
        // Without a mask there is nothing to paint over: the picture is shown.
        let w = edit_workflow(&Family::QwenImage21.preset(), &job(), &Edit { mask: None, ..edit }).unwrap();
        assert!(!w.to_string().contains("shown.png"), "{w}");
    }

    #[test]
    fn fibo_2_follows_its_templates() {
        let m = Family::Fibo2.preset();
        let loaded = |w: &Value, link: &Value| w[link[0].as_str().unwrap()]["inputs"]["image"].as_str().unwrap().to_string();
        // Its encoder loads as its own type.
        let w = workflow(&m, &job()).unwrap();
        assert_well_formed(&w);
        assert_eq!(node(&w, "CLIPLoader")["inputs"]["type"], "fibo2");
        // Text to image: its node encodes both prompts, the sampler at the template's settings
        // starts from an empty FLUX.2 latent of the size asked for.
        let encode = node(&w, "TextEncodeFibo2");
        assert_eq!((&encode["inputs"]["prompt"], &encode["inputs"]["negative_prompt"]), (&json!("a red frog"), &json!(crate::model::FIBO2_NEGATIVE)));
        assert!(!classes(&w).iter().any(|x| x == "CLIPTextEncode" || x == "LoadImage"), "{:?}", classes(&w));
        let id = w.as_object().unwrap().iter().find(|(_, n)| n["class_type"] == "TextEncodeFibo2").map(|(id, _)| id.clone()).unwrap();
        let sampler = &node(&w, "KSampler")["inputs"];
        assert_eq!((&sampler["positive"], &sampler["negative"]), (&json!([id, 0]), &json!([id, 1])));
        assert_eq!((&sampler["steps"], &sampler["cfg"], &sampler["sampler_name"], &sampler["scheduler"]), (&json!(50), &json!(5.0), &json!("euler"), &json!("simple")));
        let latent = &w[sampler["latent_image"][0].as_str().unwrap()];
        assert_eq!((latent["class_type"].as_str(), latent["inputs"]["width"].as_u64(), latent["inputs"]["height"].as_u64()), (Some("EmptyFlux2LatentImage"), Some(1024), Some(768)));

        // An edit: the pictures go to its encoder, which gives the latent at the picture's size
        // (asked for explicitly, so a single picture isn't cropped to a trained size).
        let references = ["hat.png".to_string()];
        let w = edit_workflow(&m, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &references }).unwrap();
        assert_well_formed(&w);
        let nodes = w.as_object().unwrap();
        let (id, encode) = nodes.iter().find(|(_, n)| n["class_type"] == "TextEncodeFibo2").unwrap();
        let inputs = &encode["inputs"];
        assert_eq!((loaded(&w, &inputs["images.image_1"]), loaded(&w, &inputs["images.image_2"])), ("src.png".into(), "hat.png".into()));
        assert_eq!((inputs["width"].as_u64(), inputs["height"].as_u64(), inputs["batch_size"].as_u64()), (Some(1024), Some(768), Some(1)));
        assert_eq!(nodes[inputs["vae"][0].as_str().unwrap()]["class_type"], "VAELoader");
        let sampler = &node(&w, "KSampler")["inputs"];
        assert_eq!((&sampler["positive"], &sampler["negative"], &sampler["latent_image"]), (&json!([id, 0]), &json!([id, 1]), &json!([id, 2])));
        assert!(!classes(&w).iter().any(|x| x == "VAEEncode" || x == "ReferenceLatent"), "{:?}", classes(&w));
        // Three references are taken, as the other edit models take them.
        let many = ["a.png".to_string(), "b.png".to_string(), "c.png".to_string()];
        assert!(edit_workflow(&m, &job(), &Edit { image: "src.png", mask: None, shown: None, references: &many }).is_ok());

        // A masked edit shows it the picture it paints over, never a part of it: it would stretch
        // that over the whole result.
        let edit = Edit { image: "src.png", mask: Some("mask.png"), shown: Some("shown.png"), references: &references };
        let w = edit_workflow(&m, &job(), &edit).unwrap();
        assert_well_formed(&w);
        assert!(!w.to_string().contains("shown.png"), "{w}");
        assert_eq!(loaded(&w, &node(&w, "TextEncodeFibo2")["inputs"]["images.image_1"]), "src.png");
        let encoded = &w[node(&w, "SetLatentNoiseMask")["inputs"]["samples"][0].as_str().unwrap()];
        assert_eq!((encoded["class_type"].as_str(), loaded(&w, &encoded["inputs"]["pixels"])), (Some("VAEEncode"), "src.png".to_string()));
        // It isn't offered for inpainting, but works on one at a time at about a megapixel.
        assert!(!m.can_inpaint());
        assert_eq!((Family::Fibo2.source_megapixels(4.0), Family::Fibo2.source_megapixels(0.5), Family::Fibo2.source_megapixels(0.0)), (1.0, 0.5, 0.0));
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
        // Windows' paging-file error says what to do about it.
        let paging = br#"{"abc":{"status":{"status_str":"error","messages":[["execution_error",{"node_type":"CLIPLoader","exception_message":"The paging file is too small for this operation to complete. (os error 1455)"}]]}}}"#;
        let e = parse_history(paging, "abc").unwrap_err().to_string();
        assert!(e.contains("CLIPLoader") && e.contains("os error 1455") && e.contains("Virtual memory"), "{e}");
        assert!(!parse_history(failed, "abc").unwrap_err().to_string().contains("paging file"));
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
