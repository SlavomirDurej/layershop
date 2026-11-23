//! Background removal on the local engine (ComfyUI), with nodes that ship with it.
//!
//! Two kinds of model work together. A **matting** model (BiRefNet, Lucida, RMBG 3.0) takes a
//! picture and nothing else, and answers with a soft mask of its main subject: it is good at edges
//! and has no way to be told which object is meant. RMBG 3.0 is a four-step diffusion model on
//! the engine's Fibo 2 nodes that paints the matte, which is what keeps single strands of hair. **SAM 3.1** takes a short description or clicks and
//! answers with a hard mask of that object: it knows which, and its edge is coarse. So SAM says
//! what to keep, and a matting model then cuts its edge (`ai.removeBackground` in the engine).
//!
//! Only weights are installed. The files are pinned with checksums in the Local AI setup's
//! manifest; the workflows use `RemoveBackground`, `TextEncodeFibo2` and `SAM3_Detect` of the
//! pinned engine.

use std::time::Duration;

use serde_json::{Value, json};

use crate::comfy::{self, History};
use crate::{Control, Error, Request, Result, Transport};

/// The engine's folder of matting models.
pub const MATTE_FOLDER: &str = "background_removal";
/// The engine's folder SAM 3.1's checkpoint is in.
pub const TARGET_FOLDER: &str = "checkpoints";
/// The matting model used when none is named.
pub const DEFAULT_MATTE: &str = "birefnet";
/// The longest side of what a matting model is sent. It works at 1024 px; a little more than that
/// keeps the mask it answers with from being scaled up further here.
pub const MATTE_SIDE: u32 = 2048;
/// The longest side of what SAM 3.1 is sent. It works at 1008 px and answers at this size.
pub const TARGET_SIDE: u32 = 1536;
/// What RMBG 3.0 is told. It is the instruction it was distilled with, the same for every picture.
const RMBG_INSTRUCTION: &str = r#"{"edit_instruction": "Generate a detailed grayscale alpha matte. Map the opaque foreground to white and the background to black. Produce soft, anti-aliased grayscale gradients at the edges of the subject to represent fine details and transparency."}"#;
/// RMBG 3.0's id.
const RMBG: &str = "rmbg-3";
/// The pixels RMBG 3.0 renders: the size it was trained at.
const RMBG_PIXELS: f64 = 1_000_000.0;
/// Its sides are multiples of this. Any other size is scaled and centre-cropped by the engine's
/// node, and the matte would no longer line up with the picture.
const RMBG_STEP: u32 = 16;
/// The longest side it is asked for, however long and thin the picture.
const RMBG_MAX_SIDE: u32 = 2048;
/// The longest description SAM 3.1 is given.
pub const MAX_PHRASE: usize = 120;
/// The most clicks a cut-out takes.
pub const MAX_POINTS: usize = 32;

/// How long a cut-out may take, the model's first load from disk included.
const TIMEOUT: Duration = Duration::from_secs(600);
/// How often the engine is asked whether the mask is ready.
const POLL: Duration = Duration::from_millis(250);
/// Failed polls in a row before the engine is taken for lost.
const MAX_POLL_FAILURES: u32 = 20;
/// The id the engine's queue knows these requests by.
const CLIENT: &str = "photocraft-remove-bg";
/// The name the picture is uploaded under (overwritten by the next one).
const UPLOAD: &str = "photocraft_remove_bg.png";

/// A model background removal can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BgModel {
    pub id: &'static str,
    pub name: &'static str,
    /// What it is for, as the settings say it.
    pub role: &'static str,
    /// Its folder among the engine's models.
    pub folder: &'static str,
    pub file: &'static str,
    /// The file's size in bytes.
    pub size: u64,
    pub licence: &'static str,
    pub licence_url: &'static str,
    /// The maker's page for the model.
    pub page: &'static str,
    /// Files it runs with that other models use too.
    pub shared: &'static [Shared],
}

/// A file a model runs with that other models use too: it is downloaded only when missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shared {
    pub folder: &'static str,
    pub file: &'static str,
    /// The file's size in bytes, as the Local AI setup's manifest pins it.
    pub size: u64,
}

impl Shared {
    /// Its file's key in the Local AI setup's manifest.
    pub fn manifest_key(&self) -> String {
        format!("{}/{}", self.folder, self.file)
    }
}

/// RMBG 3.0's text encoder and VAE, which Fibo 2 and FLUX.3 Klein use too.
const RMBG_SHARED: [Shared; 2] = [
    Shared { folder: "text_encoders", file: "qwen3vl_4b_int8_convrot.safetensors", size: 4_858_559_240 },
    Shared { folder: "vae", file: "flux2-vae.safetensors", size: 336_213_556 },
];

impl BgModel {
    /// Its file's key in the Local AI setup's manifest.
    pub fn manifest_key(&self) -> String {
        format!("{}/{}", self.folder, self.file)
    }
}

/// The matting models, the default first.
pub const MATTE_MODELS: [BgModel; 3] = [
    BgModel {
        id: "birefnet",
        name: "BiRefNet General",
        role: "Photos: people, products, animals.",
        folder: MATTE_FOLDER,
        file: "birefnet.safetensors",
        size: 444_473_596,
        licence: "MIT",
        licence_url: "https://huggingface.co/Comfy-Org/BiRefNet",
        page: "https://github.com/ZhengPeng7/BiRefNet",
        shared: &[],
    },
    BgModel {
        id: "lucida",
        name: "Lucida",
        role: "Graphics, logos, text and glass. Some of its training data is research-only.",
        folder: MATTE_FOLDER,
        file: "lucida.safetensors",
        size: 884_878_856,
        licence: "MIT",
        licence_url: "https://github.com/egeorcun/lucida#license",
        page: "https://github.com/egeorcun/lucida",
        shared: &[],
    },
    BgModel {
        id: RMBG,
        name: "RMBG 3.0",
        role: "The finest edges: hair, fur, glass and soft detail. Non-commercial use only, unless licensed from Bria.",
        folder: "diffusion_models",
        file: "rmbg3_int8_convrot.safetensors",
        size: 8_408_562_474,
        licence: "CC BY-NC 4.0",
        licence_url: "https://huggingface.co/briaai/RMBG-3.0-Comfy",
        page: "https://huggingface.co/briaai/RMBG-3.0",
        shared: &RMBG_SHARED,
    },
];

/// SAM 3.1, which finds the object to keep. Meta's own download is a pickle behind manual
/// approval, which the setup does not install; this is Comfy-Org's safetensors of it, under the
/// same licence.
pub const TARGET_MODEL: BgModel = BgModel {
    id: "sam-3-1",
    name: "SAM 3.1",
    role: "Finds what to keep, from a description or clicks.",
    folder: TARGET_FOLDER,
    file: "sam3.1_multiplex_fp16.safetensors",
    size: 1_745_546_848,
    licence: "SAM License",
    licence_url: "https://github.com/facebookresearch/sam3/blob/main/LICENSE",
    page: "https://huggingface.co/facebook/sam3.1",
    shared: &[],
};

/// Every model, in the order the settings list them.
pub fn models() -> [&'static BgModel; 4] {
    [&MATTE_MODELS[0], &MATTE_MODELS[1], &MATTE_MODELS[2], &TARGET_MODEL]
}

/// The size `model` must be sent a picture of `width` × `height` at, when it has one of its own.
/// RMBG 3.0's is about a megapixel in the picture's shape, its sides multiples of 16.
pub fn render_size(model: &BgModel, width: u32, height: u32) -> Option<(u32, u32)> {
    if model.id != RMBG {
        return None;
    }
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let scale = (RMBG_PIXELS / (w * h)).sqrt();
    let side = |v: f64| ((v * scale / f64::from(RMBG_STEP)).round() as u32).saturating_mul(RMBG_STEP).clamp(RMBG_STEP, RMBG_MAX_SIDE);
    Some((side(w), side(h)))
}

/// The model with this id.
pub fn find(id: &str) -> Option<&'static BgModel> {
    models().into_iter().find(|m| m.id == id.trim())
}

/// The matting model with this id.
pub fn find_matte(id: &str) -> Option<&'static BgModel> {
    MATTE_MODELS.iter().find(|m| m.id == id.trim())
}

/// What SAM 3.1 is asked to find.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// A short noun phrase ([`object_phrase`]).
    Text(&'a str),
    /// Clicks, in the pixels of the picture sent: on what to keep, and on what to leave out.
    Points { keep: &'a [[u32; 2]], drop: &'a [[u32; 2]] },
}

/// What to look for in what someone typed. SAM 3.1 takes a short noun phrase, and an instruction
/// finds nothing: "keep the couple, remove everything else" is asked as "couple".
pub fn object_phrase(typed: &str) -> String {
    let flat = typed.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    // What comes after "remove" is what goes, not what to find.
    let cut = [" and remove ", ", remove ", " remove ", " and delete ", ", delete ", " delete ", " without the background"]
        .iter()
        .filter_map(|w| flat.find(w))
        .min()
        .unwrap_or(flat.len());
    let kept = flat.get(..cut).unwrap_or(&flat);
    let mut words: Vec<&str> = kept.split(' ').filter(|w| !w.is_empty()).collect();
    while words.first().is_some_and(|w| matches!(*w, "keep" | "only" | "just" | "select" | "the" | "a" | "an")) {
        words.remove(0);
    }
    let phrase = words.join(" ");
    let phrase = phrase.trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace());
    phrase.chars().take(MAX_PHRASE).collect()
}

/// RMBG 3.0 painting the matte of the uploaded `image`, which is `size`: four steps, no guidance,
/// as its authors and ComfyUI's own template run it.
fn rmbg_workflow(model: &BgModel, image: &str, size: (u32, u32)) -> Value {
    let (encoder, vae) = (RMBG_SHARED[0].file, RMBG_SHARED[1].file);
    json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": image}},
        "2": {"class_type": "UNETLoader", "inputs": {"unet_name": model.file, "weight_dtype": "default"}},
        "3": {"class_type": "CLIPLoader", "inputs": {"clip_name": encoder, "type": "fibo2", "device": "default"}},
        "4": {"class_type": "VAELoader", "inputs": {"vae_name": vae}},
        "5": {"class_type": "TextEncodeFibo2", "inputs": {
            "clip": ["3", 0], "prompt": RMBG_INSTRUCTION, "negative_prompt": "", "images.image_1": ["1", 0],
            "width": size.0, "height": size.1, "batch_size": 1, "vae": ["4", 0],
        }},
        "6": {"class_type": "KSampler", "inputs": {
            "model": ["2", 0], "positive": ["5", 0], "negative": ["5", 1], "latent_image": ["5", 2],
            "seed": 42, "steps": 4, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple", "denoise": 1.0,
        }},
        "7": {"class_type": "VAEDecode", "inputs": {"samples": ["6", 0], "vae": ["4", 0]}},
        "8": {"class_type": "PreviewImage", "inputs": {"images": ["7", 0]}},
    })
}

/// The workflow that masks the main subject of the uploaded `image`, which is `size`, with `model`.
pub fn matte_workflow(model: &BgModel, image: &str, size: (u32, u32)) -> Value {
    if model.id == RMBG {
        return rmbg_workflow(model, image, size);
    }
    json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": image}},
        "2": {"class_type": "LoadBackgroundRemovalModel", "inputs": {"bg_removal_name": model.file}},
        "3": {"class_type": "RemoveBackground", "inputs": {"bg_removal_model": ["2", 0], "image": ["1", 0]}},
        "4": {"class_type": "MaskToImage", "inputs": {"mask": ["3", 0]}},
        "5": {"class_type": "PreviewImage", "inputs": {"images": ["4", 0]}},
    })
}

fn coords(points: &[[u32; 2]]) -> String {
    Value::Array(points.iter().map(|[x, y]| json!({"x": x, "y": y})).collect()).to_string()
}

/// The workflow that masks what `target` says in the uploaded `image` with SAM 3.1. The node
/// joins what text and clicks each find, so a click could not take anything from a description's
/// answer: a cut-out asks one way or the other.
pub fn target_workflow(image: &str, target: &Target<'_>) -> Value {
    let mut detect = json!({"model": ["1", 0], "image": ["2", 0], "threshold": 0.5, "refine_iterations": 2, "individual_masks": false});
    let mut nodes = json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": TARGET_MODEL.file}},
        "2": {"class_type": "LoadImage", "inputs": {"image": image}},
    });
    match target {
        Target::Text(text) => {
            nodes["3"] = json!({"class_type": "CLIPTextEncode", "inputs": {"text": text, "clip": ["1", 1]}});
            detect["conditioning"] = json!(["3", 0]);
        }
        Target::Points { keep, drop } => {
            detect["positive_coords"] = json!(coords(keep));
            if !drop.is_empty() {
                detect["negative_coords"] = json!(coords(drop));
            }
        }
    }
    nodes["4"] = json!({"class_type": "SAM3_Detect", "inputs": detect});
    nodes["5"] = json!({"class_type": "MaskToImage", "inputs": {"mask": ["4", 0]}});
    nodes["6"] = json!({"class_type": "PreviewImage", "inputs": {"images": ["5", 0]}});
    nodes
}

/// `e`, the engine's refusal, in words someone can act on: a file it doesn't have, or a node an
/// older ComfyUI doesn't know.
fn explained(model: &BgModel, e: Error) -> Error {
    match e {
        Error::Server(msg) if msg.contains("does not exist") => Error::Server(format!(
            "This ComfyUI has no {} node: update it (for the one File › Set Up Local AI installed, run that again). ({msg})",
            match model.id {
                id if id == TARGET_MODEL.id => "SAM 3",
                RMBG => "Fibo 2",
                _ => "Remove Background",
            }
        )),
        Error::Server(msg) if ["bg_removal_name", "ckpt_name", "unet_name", "clip_name", "vae_name", model.file].iter().any(|w| msg.contains(w)) => Error::Server(format!(
            "{} is not installed: the engine has no {}. Install it in Preferences › Generative AI › Remove BG ({msg})",
            model.name, model.file
        )),
        e => e,
    }
}

/// Best effort: stop the engine's work on `job` and take it off the queue.
fn stop(transport: &dyn Transport, base: &str, job: &str) {
    let _ = transport.send(&Request::post_json(format!("{base}/interrupt"), b"{}".to_vec()).timeout(5));
    let _ = transport.send(&Request::post_json(format!("{base}/queue"), json!({"delete": [job]}).to_string().into_bytes()).timeout(5));
}

/// Upload `picture`, queue what `workflow` makes of its name, and bring back the image it answers
/// with. `base` must answer: the caller starts the engine.
fn render(transport: &dyn Transport, base: &str, model: &BgModel, picture: &[u8], workflow: impl FnOnce(&str) -> Value, control: &Control<'_>) -> Result<Vec<u8>> {
    let base = comfy::base_url(base)?;
    let unreachable = |e: Error| Error::Network(format!("Could not reach the local engine at {base}. Is it running? ({e})"));
    let sent = transport.send(&comfy::upload_request(&base, UPLOAD, picture)).map_err(unreachable)?;
    let name = comfy::parse_uploaded(sent.status, &sent.body)?;
    if (control.cancelled)() {
        return Err(Error::Cancelled);
    }
    let workflow = workflow(&name);
    let queued = transport.send(&Request::post_json(format!("{base}/prompt"), comfy::queue_body(&workflow, CLIENT)).timeout(30)).map_err(unreachable)?;
    let job = comfy::parse_queued(queued.status, &queued.body).map_err(|e| explained(model, e))?;
    let polls = (TIMEOUT.as_millis() / POLL.as_millis().max(1)).max(1) as u64;
    let mut failures = 0u32;
    for _ in 0..polls {
        if (control.cancelled)() {
            stop(transport, &base, &job);
            return Err(Error::Cancelled);
        }
        (control.sleep)(POLL);
        let history = match transport.send(&Request::get(format!("{base}/history/{job}")).timeout(20)) {
            Ok(r) if r.ok() => comfy::parse_history(&r.body, &job).map_err(|e| explained(model, e))?,
            Ok(_) | Err(_) => {
                failures += 1;
                if failures > MAX_POLL_FAILURES {
                    return Err(Error::Network(format!("Lost the connection to the local engine at {base} while it was working")));
                }
                continue;
            }
        };
        failures = 0;
        if let History::Done(image) = history {
            let r = transport.send(&Request::get(comfy::view_url(&base, &image)).timeout(120)).map_err(|e| Error::Network(format!("Could not download the mask from the engine: {e}")))?;
            if !r.ok() || r.body.is_empty() {
                return Err(Error::Server(format!("The engine could not serve the mask (HTTP {})", r.status)));
            }
            return Ok(r.body);
        }
    }
    stop(transport, &base, &job);
    Err(Error::Server(format!("{} did not finish within {} seconds", model.name, TIMEOUT.as_secs())))
}

/// The soft mask of `picture` (which is `size`)'s main subject from matting `model`, as an image
/// the size of `picture`.
pub fn matte(transport: &dyn Transport, base: &str, model: &BgModel, picture: &[u8], size: (u32, u32), control: &Control<'_>) -> Result<Vec<u8>> {
    render(transport, base, model, picture, |name| matte_workflow(model, name, size), control)
}

/// The mask of what `target` says in `picture` from SAM 3.1, as an image the size of `picture`
/// (black where it found nothing).
pub fn locate(transport: &dyn Transport, base: &str, picture: &[u8], target: &Target<'_>, control: &Control<'_>) -> Result<Vec<u8>> {
    render(transport, base, &TARGET_MODEL, picture, |name| target_workflow(name, target), control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Response;
    use std::sync::Mutex;

    #[test]
    fn an_instruction_is_asked_as_the_object_it_names() {
        for (typed, phrase) in [
            ("keep the couple remove everything else", "couple"),
            ("Keep the couple, remove everything else", "couple"),
            ("the woman in red", "woman in red"),
            ("  a   dog. ", "dog"),
            ("just the car and remove the road", "car"),
            ("red sports car", "red sports car"),
            ("keep the", ""),
            ("", ""),
        ] {
            assert_eq!(object_phrase(typed), phrase, "{typed}");
        }
        assert_eq!(object_phrase(&"é".repeat(MAX_PHRASE * 3)).chars().count(), MAX_PHRASE);
    }

    #[test]
    fn the_catalogue_names_a_model_of_each_kind() {
        assert_eq!(find_matte(DEFAULT_MATTE).map(|m| m.file), Some("birefnet.safetensors"));
        assert!(find_matte(TARGET_MODEL.id).is_none(), "SAM is not a matting model");
        assert_eq!(find(" sam-3-1 "), Some(&TARGET_MODEL));
        assert_eq!(TARGET_MODEL.manifest_key(), "checkpoints/sam3.1_multiplex_fp16.safetensors");
        assert_eq!(MATTE_MODELS[1].manifest_key(), "background_removal/lucida.safetensors");
    }

    #[test]
    fn matting_runs_the_engines_own_nodes() {
        let w = matte_workflow(&MATTE_MODELS[1], "in.png", (64, 64));
        assert_eq!(w["2"]["inputs"]["bg_removal_name"], "lucida.safetensors");
        assert_eq!(w["3"]["class_type"], "RemoveBackground");
        assert_eq!(w["4"]["inputs"]["mask"], json!(["3", 0]));
        assert_eq!(w["5"]["class_type"], "PreviewImage");
    }

    #[test]
    fn rmbg_paints_the_matte_at_the_size_of_the_picture_sent() {
        let rmbg = find_matte("rmbg-3").unwrap();
        let w = matte_workflow(rmbg, "in.png", (1136, 880));
        assert_eq!(w["2"]["inputs"]["unet_name"], "rmbg3_int8_convrot.safetensors");
        assert_eq!((&w["3"]["inputs"]["type"], &w["3"]["inputs"]["clip_name"]), (&json!("fibo2"), &json!("qwen3vl_4b_int8_convrot.safetensors")));
        assert_eq!((&w["5"]["inputs"]["width"], &w["5"]["inputs"]["height"]), (&json!(1136), &json!(880)));
        assert_eq!((&w["6"]["inputs"]["steps"], &w["6"]["inputs"]["cfg"]), (&json!(4), &json!(1.0)));
        assert_eq!(w["8"]["class_type"], "PreviewImage");
    }

    #[test]
    fn only_rmbg_has_a_size_of_its_own_in_steps_of_16() {
        let rmbg = find_matte("rmbg-3").unwrap();
        assert_eq!(render_size(&MATTE_MODELS[0], 2560, 1280), None);
        assert_eq!(render_size(rmbg, 2560, 1280), Some((1408, 704)));
        assert_eq!(render_size(rmbg, 510, 240), Some((1456, 688)));
        for (w, h) in [(1, 1), (0, 0), (100_000, 1), (1, 100_000), (u32::MAX, u32::MAX)] {
            let (a, b) = render_size(rmbg, w, h).unwrap();
            assert!(a % 16 == 0 && b % 16 == 0 && (16..=2048).contains(&a) && (16..=2048).contains(&b), "{w}x{h}: {a}x{b}");
        }
    }

    #[test]
    fn sam_is_asked_by_text_or_by_clicks() {
        let text = target_workflow("in.png", &Target::Text("couple"));
        assert_eq!(text["3"]["inputs"]["text"], "couple");
        assert_eq!(text["4"]["inputs"]["conditioning"], json!(["3", 0]));
        assert!(text["4"]["inputs"].get("positive_coords").is_none());

        let clicks = target_workflow("in.png", &Target::Points { keep: &[[10, 20]], drop: &[[3, 4]] });
        assert!(clicks.get("3").is_none_or(Value::is_null), "no text encoder");
        assert!(clicks["4"]["inputs"].get("conditioning").is_none());
        let read = |key: &str| serde_json::from_str::<Value>(clicks["4"]["inputs"][key].as_str().unwrap()).unwrap();
        assert_eq!(read("positive_coords"), json!([{"x": 10, "y": 20}]));
        assert_eq!(read("negative_coords"), json!([{"x": 3, "y": 4}]));

        let none_left_out = target_workflow("in.png", &Target::Points { keep: &[[1, 1]], drop: &[] });
        assert!(none_left_out["4"]["inputs"].get("negative_coords").is_none());
    }

    /// What the fake engine answers a URL with.
    type Answer = Box<dyn Fn(&str) -> Result<Response> + Send + Sync>;

    /// Answers each request in turn with what `answer` says for its URL.
    struct Fake {
        urls: Mutex<Vec<String>>,
        answer: Answer,
    }

    impl Transport for Fake {
        fn send(&self, r: &Request) -> Result<Response> {
            self.urls.lock().unwrap().push(r.url.clone());
            (self.answer)(&r.url)
        }
    }

    fn body(text: &str) -> Result<Response> {
        Ok(Response { status: 200, body: text.as_bytes().to_vec() })
    }

    fn engine(queue: &'static str) -> Fake {
        Fake {
            urls: Mutex::new(Vec::new()),
            answer: Box::new(move |url| {
                if url.contains("/upload/image") {
                    body(r#"{"name":"in.png","subfolder":""}"#)
                } else if url.ends_with("/prompt") {
                    body(queue)
                } else if url.contains("/history/") {
                    body(r#"{"j":{"status":{"completed":true},"outputs":{"5":{"images":[{"filename":"m.png","subfolder":"","type":"temp"}]}}}}"#)
                } else {
                    body("the mask")
                }
            }),
        }
    }

    fn control<'a>(cancelled: &'a (dyn Fn() -> bool + Sync)) -> Control<'a> {
        Control { cancelled, progress: &|_, _| {}, sleep: &|_| {} }
    }

    #[test]
    fn a_matte_is_uploaded_queued_and_brought_back() {
        let t = engine(r#"{"prompt_id":"j"}"#);
        let got = matte(&t, "http://127.0.0.1:8188", &MATTE_MODELS[0], b"png", (8, 8), &control(&|| false)).unwrap();
        assert_eq!(got, b"the mask");
        let urls = t.urls.lock().unwrap();
        assert!(urls[0].ends_with("/upload/image") && urls[1].ends_with("/prompt") && urls[2].ends_with("/history/j"));
        assert!(urls[3].contains("/view?filename=m.png"));
    }

    #[test]
    fn a_missing_file_says_where_to_install_it() {
        let t = engine(r#"{"error":{"message":"invalid"},"node_errors":{"2":{"class_type":"LoadBackgroundRemovalModel","errors":[{"message":"Value not in list","details":"bg_removal_name: 'birefnet.safetensors' not in []"}]}}}"#);
        let e = matte(&t, "http://127.0.0.1:8188", &MATTE_MODELS[0], b"png", (8, 8), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("BiRefNet General is not installed") && e.contains("Remove BG"), "{e}");
    }

    #[test]
    fn an_older_engine_is_told_to_update() {
        let t = engine(r#"{"error":{"message":"Cannot execute because node SAM3_Detect does not exist."}}"#);
        let e = locate(&t, "http://127.0.0.1:8188", b"png", &Target::Text("dog"), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("no SAM 3 node") && e.contains("update"), "{e}");
    }

    #[test]
    fn cancelling_stops_the_engine() {
        let t = engine(r#"{"prompt_id":"j"}"#);
        let asked = std::sync::atomic::AtomicUsize::new(0);
        // Past the check after the upload, cancelled at the first poll.
        let cancelled = || asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1;
        assert_eq!(matte(&t, "http://127.0.0.1:8188", &MATTE_MODELS[0], b"png", (8, 8), &control(&cancelled)), Err(Error::Cancelled));
        assert!(t.urls.lock().unwrap().iter().any(|u| u.ends_with("/interrupt")));
    }
}
