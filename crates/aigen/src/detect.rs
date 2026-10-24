//! Guess ready-to-use models from the files a ComfyUI server has (Manage Models › Scan Server).
//!
//! File names are the only evidence, so this recognises the names the model publishers and the
//! ComfyUI templates use, and leaves anything else to "Add Local Model".

use std::collections::BTreeMap;

use crate::model::{Family, Model};

/// The first of `files` whose lowercase name contains any of `needles` (in needle order: the
/// earlier needle is the better match).
fn pick<'a>(files: &'a [String], needles: &[&str]) -> Option<&'a str> {
    needles.iter().find_map(|n| files.iter().find(|f| f.to_ascii_lowercase().contains(n))).map(String::as_str)
}

/// `preferred` if the server has it, else the best match for `needles`, else `preferred` (the
/// user then sees which file to download).
fn choose(files: &[String], preferred: &str, needles: &[&str]) -> String {
    if preferred.is_empty() || files.iter().any(|f| f == preferred) {
        return preferred.to_string();
    }
    pick(files, needles).unwrap_or(preferred).to_string()
}

/// A display name from a file name: no folder, no extension, separators as spaces.
fn title(file: &str) -> String {
    let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The family a diffusion model file belongs to, by name. Video models, and edit models other
/// than Qwen Image Edit, are not recognised.
fn family_of(file: &str) -> Option<Family> {
    let n = file.to_ascii_lowercase();
    if n.contains("qwen_image_edit") || n.contains("qwen-image-edit") {
        return Some(Family::QwenEdit);
    }
    if ["edit", "video", "i2v", "t2v", "ltx", "wan", "unconditional", "anima", "controlnet", "inpaint", "kontext"].iter().any(|x| n.contains(x)) {
        return None;
    }
    // BFL's inpainting model and its fine-tunes: not a text-to-image FLUX.1.
    if n.contains("flux") && (n.contains("fill") || n.contains("onereward")) {
        return Some(Family::Flux1Fill);
    }
    Some(if n.contains("z_image") || n.contains("zimage") || n.contains("z-image") {
        Family::ZImage
    } else if n.contains("krea2") || n.contains("krea-2") || n.contains("krea_2") {
        Family::Krea2
    } else if n.contains("flux3") || n.contains("flux-3") || n.contains("flux_3") {
        Family::Flux3
    } else if n.contains("flux2") || n.contains("flux-2") || n.contains("flux_2") {
        Family::Flux2
    } else if n.contains("flux1") || n.contains("flux-1") || n.contains("flux_1") {
        Family::Flux1
    } else if n.contains("ernie") {
        Family::Ernie
    } else if n.contains("hidream_i1") || n.contains("hidream-i1") {
        Family::HidreamI1
    } else if n.contains("ideogram") {
        Family::Ideogram4
    } else if n.contains("ming_image") || n.contains("ming-image") {
        // Its sibling, Ming Image Design Layer, takes a design apart into layers: another task.
        if n.contains("layer") {
            return None;
        }
        Family::MingImage
    } else if n.contains("moss_image") || n.contains("moss-image") {
        Family::MossImage
    } else if n.contains("qwen_image_2.1") || n.contains("qwen-image-2.1") || n.contains("qwen_image_21") {
        Family::QwenImage21
    } else if n.contains("qwen_image") || n.contains("qwen-image") {
        Family::QwenImage
    } else {
        return None;
    })
}

/// A model set up in a way that is known not to run, and what to do: files of one architecture
/// under another's settings. Only the combinations that are certain to fail are reported (the
/// user is free to try anything else); the alternative is ComfyUI's own error, a tensor shape.
pub fn problem(model: &Model) -> Option<String> {
    let name = if model.name.trim().is_empty() { model.family.label() } else { model.name.trim() };
    let fix = "On Preferences › Generative AI › Manage Models, press Scan Server to repair it";
    let unet = model.unet.to_ascii_lowercase();
    let is_21_file = family_of(&model.unet) == Some(Family::QwenImage21);
    match model.family {
        // The model that prompted this: scanned as Qwen Image before 2.1 had its own settings.
        Family::QwenImage if is_21_file => Some(format!(
            "{name}: {} is a Qwen Image 2.1 model, but it is set up as Qwen Image, whose text encoder and VAE it can't use. {fix}, or set its Architecture to Qwen Image 2.1.",
            model.unet.trim()
        )),
        Family::QwenImage21 if !unet.is_empty() && !is_21_file && family_of(&model.unet) == Some(Family::QwenImage) => Some(format!(
            "{name}: {} is a Qwen Image model, but it is set up as Qwen Image 2.1. {fix}, or set its Architecture to Qwen Image.",
            model.unet.trim()
        )),
        Family::QwenImage21 => {
            let clip = model.clip.to_ascii_lowercase();
            let vae = model.vae.to_ascii_lowercase();
            if clip.contains("qwen_2.5") || clip.contains("qwen2.5") {
                Some(format!(
                    "{name}: Qwen Image 2.1 needs the Qwen 3 VL 8B text encoder (qwen3vl_8b…), not {}. {fix}, or choose that text encoder.",
                    model.clip.trim()
                ))
            } else if vae == "qwen_image_vae.safetensors" {
                Some(format!("{name}: Qwen Image 2.1 needs its own VAE (qwen_image_2.1_vae…), not {}. {fix}, or choose that VAE.", model.vae.trim()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `checkpoint` is HiDream O1 Dev, the distilled model (`hidream_o1_image_dev_…`), which is
/// sampled differently from the full one.
pub fn hidream_o1_dev(checkpoint: &str) -> bool {
    let n = checkpoint.to_ascii_lowercase();
    n.contains("_dev") || n.contains("-dev")
}

/// `existing` with the architecture, files and sampling of `detected` (the same model file,
/// detected afresh): what Scan Server does to a model that was set up as the wrong
/// architecture. The user's own choices that still apply are kept: its id, name, whether it is
/// listed, seed, render size, LoRA, negative prompt and transparency.
pub fn repaired(existing: &Model, detected: &Model) -> Model {
    Model {
        id: existing.id.clone(),
        name: existing.name.clone(),
        enabled: existing.enabled,
        seed: existing.seed,
        megapixels: existing.megapixels,
        lora: existing.lora.clone(),
        lora_strength: existing.lora_strength,
        negative_prompt: existing.negative_prompt.clone(),
        alpha: existing.alpha,
        ..detected.clone()
    }
}

/// What a model file is, whatever it is quantised to: its name without the folder, the extension
/// and the words that say how the weights are stored (`z_image_turbo_int8_convrot.safetensors`
/// and `z_image_turbo_bf16.safetensors` are both `z image turbo`).
pub fn variant(file: &str) -> String {
    const STORAGE: [&str; 14] =
        ["bf16", "fp16", "fp8", "fp32", "int8", "mxfp8", "convrot", "scaled", "mixed", "fp8mixed", "e4m3fn", "e5m2", "gguf", "safetensors"];
    let stem = file.rsplit(['/', '\\']).next().unwrap_or(file).to_ascii_lowercase();
    let stem = stem.rsplit_once('.').filter(|(_, ext)| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphabetic())).map_or(stem.as_str(), |(s, _)| s);
    stem.split(['_', '-', ' ']).filter(|word| !word.is_empty() && !STORAGE.contains(word)).collect::<Vec<_>>().join(" ")
}

/// Two local models are the same model, perhaps stored differently: the same architecture and
/// the same [`variant`] of their main file.
pub fn same_variant(a: &Model, b: &Model) -> bool {
    let (x, y) = (variant(a.main_file()), variant(b.main_file()));
    a.provider.is_local() && b.provider.is_local() && a.family == b.family && !x.is_empty() && x == y
}

/// The server lists the file `model` is made of (`folders` as [`crate::comfy_models`] returns
/// them). True for what has no single file to look for: a cloud model, a custom workflow, a
/// model without one chosen yet.
pub fn on_server(folders: &BTreeMap<String, Vec<String>>, model: &Model) -> bool {
    let file = model.main_file();
    if !model.provider.is_local() || model.family == Family::Custom || file.is_empty() {
        return true;
    }
    let folder = if model.checkpoint.trim().is_empty() { "diffusion_models" } else { "checkpoints" };
    folders.get(folder).is_some_and(|files| files.iter().any(|f| f == file))
}

/// `existing` pointed at the files of `detected` (the same model as the server stores it): what
/// Scan Server does to a listed model whose own file the server doesn't have. Everything the
/// user can tune (sampling, size, name, id) is kept.
pub fn adopted(existing: &Model, detected: &Model) -> Model {
    let d = detected;
    Model {
        unet: d.unet.clone(),
        unet2: d.unet2.clone(),
        checkpoint: d.checkpoint.clone(),
        clip: d.clip.clone(),
        clip2: d.clip2.clone(),
        clip3: d.clip3.clone(),
        clip4: d.clip4.clone(),
        vae: d.vae.clone(),
        controlnet: d.controlnet.clone(),
        enhancer: d.enhancer.clone(),
        refiner: d.refiner.clone(),
        refiner_clip: d.refiner_clip.clone(),
        refiner_vae: d.refiner_vae.clone(),
        ..existing.clone()
    }
}

/// Models for the text-to-image, inpainting and edit files in `folders` (as [`crate::comfy_models`] returns them),
/// each with its family's reference settings and the encoder / VAE files the server has.
pub fn detect(folders: &BTreeMap<String, Vec<String>>) -> Vec<Model> {
    let empty = Vec::new();
    let get = |k: &str| folders.get(k).unwrap_or(&empty);
    let (unets, clips, vaes, checkpoints) = (get("diffusion_models"), get("text_encoders"), get("vae"), get("checkpoints"));
    let mut out = Vec::new();
    for file in unets.iter().take(500) {
        let Some(family) = family_of(file) else { continue };
        let n = file.to_ascii_lowercase();
        let turbo = n.contains("turbo") || n.contains("schnell") || n.contains("lightning");
        let mut m = family.preset();
        m.name = title(file);
        m.unet = file.clone();
        match family {
            Family::ZImage if !turbo => (m.steps, m.cfg, m.sampler) = (30, 4.0, "euler".into()),
            // Two megapixels: what the turbo model was judged at (2026-10-13).
            Family::Ernie if turbo => (m.steps, m.cfg, m.megapixels) = (8, 1.0, 2.0),
            // The step-distilled Klein: its templates' 16 steps without guidance. BFL's first names
            // marked it `…_klein_sd_…`, and `flux3_image_klein_…` without "base" was the base model
            // under another name; since 2026-10-17 its repository names files with hyphens, and
            // there every `flux3-image-klein-…` without "-base-" is the distilled one.
            Family::Flux3 if n.contains("klein_sd") || n.contains("klein-sd") || (n.contains("flux3-image-klein-") && !n.contains("-base-")) => {
                (m.steps, m.cfg) = (16, 1.0);
            }
            Family::Flux2 if n.contains("klein") => {
                // The "base" Klein is the undistilled one; the other renders in four steps.
                if !n.contains("base") {
                    (m.steps, m.cfg) = (4, 1.0);
                }
            }
            Family::Flux2 => {
                (m.steps, m.cfg, m.guidance) = (20, 1.0, 4.0);
                m.clip = "mistral_3_small_flux2_bf16.safetensors".into();
            }
            Family::Flux1 if turbo => (m.steps, m.guidance) = (4, 0.0),
            Family::QwenEdit => {
                // With its release's four-step LoRA on the server, the model renders in four steps.
                let release = ["2509", "2511"].into_iter().find(|r| n.contains(r));
                let lightning = |l: &&String| {
                    let l = l.to_ascii_lowercase();
                    l.contains("qwen") && l.contains("edit") && l.contains("lightning") && l.contains("4step") && release.is_some_and(|r| l.contains(r))
                };
                if let Some(lora) = get("loras").iter().find(lightning) {
                    (m.lora, m.steps, m.cfg) = (lora.clone(), 4, 1.0);
                }
            }
            _ => {}
        }
        let clip_needles: &[&str] = match family {
            Family::ZImage => &["qwen_3_4b"],
            Family::Krea2 | Family::Flux3 => &["qwen3vl_4b"],
            Family::Flux2 if n.contains("klein") => &["qwen_3_8b"],
            Family::Flux2 => &["mistral_3_small_flux2", "mistral"],
            Family::Flux1 | Family::Flux1Fill | Family::HidreamI1 => &["clip_l"],
            Family::Ernie => &["ministral"],
            Family::Ideogram4 => &["qwen3vl_8b"],
            Family::QwenImage | Family::QwenEdit => &["qwen_2.5_vl"],
            Family::QwenImage21 => &["qwen3vl_8b"],
            Family::MingImage => &["ling_mini", "ming_image"],
            // Not the other Qwen 3.5 9B files beside it: FLUX.3's and Qwen Image 2.1's prompt writers.
            Family::MossImage => &["qwen3.5_9b_w4a8", "qwen3.5_9b_int8", "qwen3.5_9b_bf16"],
            _ => &[],
        };
        m.clip = choose(clips, &m.clip, clip_needles);
        match family {
            Family::Flux1 | Family::Flux1Fill => m.clip2 = choose(clips, &m.clip2, &["t5xxl"]),
            Family::HidreamI1 => {
                m.clip2 = choose(clips, &m.clip2, &["clip_g"]);
                m.clip3 = choose(clips, &m.clip3, &["t5xxl"]);
                m.clip4 = choose(clips, &m.clip4, &["llama"]);
            }
            Family::Ideogram4 => m.unet2 = choose(unets, &m.unet2, &["ideogram4_unconditional", "unconditional"]),
            _ => {}
        }
        let vae_needles: &[&str] = match family {
            Family::QwenImage21 => &["qwen_image_2.1_vae", "qwen_image_2.1"],
            Family::MingImage => &["ming_image_vae", "ming_image"],
            Family::MossImage => &["flux2-vae_small", "flux2_vae_small", "flux2-vae"],
            Family::Flux3 | Family::Flux2 | Family::Ernie | Family::Ideogram4 => &["flux2-vae", "flux2"],
            Family::Krea2 | Family::QwenImage | Family::QwenEdit => &["qwen_image_vae"],
            _ => &["ae.safetensors"],
        };
        m.vae = choose(vaes, &m.vae, vae_needles);
        out.push(m);
    }
    // Qwen Image becomes an inpainting model with InstantX's ControlNet: one model per such
    // ControlNet, on the Qwen Image file ComfyUI's template names (or the one the server has).
    for file in get("controlnet").iter().take(500) {
        let n = file.to_ascii_lowercase();
        if !(n.contains("qwen") && n.contains("inpaint")) {
            continue;
        }
        let mut m = Family::QwenInpaint.preset();
        m.controlnet = file.clone();
        if !unets.contains(&m.unet) {
            // The original Qwen Image is what the ControlNet was trained on; a later one is a guess.
            let qwen = |u: &&String| family_of(u) == Some(Family::QwenImage);
            let Some(unet) = unets.iter().find(|u| qwen(u) && !u.contains("2512")).or_else(|| unets.iter().find(qwen)) else { continue };
            m.unet = unet.clone();
        }
        m.clip = choose(clips, &m.clip, &["qwen_2.5_vl"]);
        m.vae = choose(vaes, &m.vae, &["qwen_image_vae"]);
        out.push(m);
    }
    for file in checkpoints.iter().take(500) {
        let n = file.to_ascii_lowercase();
        if n.contains("hidream_o1") || n.contains("hidream-o1") {
            let mut m = Family::HidreamO1.preset();
            m.name = title(file);
            m.checkpoint = file.clone();
            if hidream_o1_dev(file) {
                // ComfyUI's HiDream O1 Dev template: distilled, so fewer steps and no guidance.
                (m.steps, m.cfg, m.shift) = (28, 1.0, 7.6);
                m.sampler = "lcm".into();
            }
            // With a Gemma 4 on the server the prompt is rewritten as the model wants it, and
            // with Z-Image Turbo and its files the result is refined: both or neither of the
            // refiner's three files, since it can't run on one.
            let first = |files: &[String], needles: &[&str]| pick(files, needles).unwrap_or("").to_string();
            m.enhancer = first(clips, &["gemma4_e4b_it", "gemma4_e2b_it"]);
            let turbo = unets.iter().find(|u| {
                let u = u.to_ascii_lowercase();
                u.contains("z_image_turbo") || u.contains("z-image-turbo")
            });
            // FLUX.1's VAE by its whole name: every `…vae.safetensors` ends in it.
            let ae = |v: &&String| v.rsplit(['/', '\\']).next().is_some_and(|name| name.eq_ignore_ascii_case("ae.safetensors"));
            let (clip, vae) = (first(clips, &["qwen_3_4b"]), vaes.iter().find(ae).cloned().unwrap_or_default());
            if let Some(turbo) = turbo
                && !clip.is_empty()
                && !vae.is_empty()
            {
                (m.refiner, m.refiner_clip, m.refiner_vae) = (turbo.clone(), clip, vae);
            }
            out.push(m);
        } else if n.contains("sdxl") || n.contains("sd_xl") || n.contains("sd15") || n.contains("v1-5") {
            let mut m = Family::Checkpoint.preset();
            m.name = title(file);
            m.checkpoint = file.clone();
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> BTreeMap<String, Vec<String>> {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        BTreeMap::from([
            (
                "diffusion_models".to_string(),
                v(&[
                    "z_image_turbo_bf16.safetensors",
                    "z_image_bf16.safetensors",
                    "krea2_turbo_fp8_scaled.safetensors",
                    "flux-2-klein-9b-fp8.safetensors",
                    "flux-2-klein-base-9b-fp8.safetensors",
                    "flux2_dev_fp8mixed.safetensors",
                    "flux3_image_klein_base_int8_convrot.safetensors",
                    "flux3_image_klein_sd_int8_convrot.safetensors",
                    "flux1-dev.safetensors",
                    "ernie-image-turbo.safetensors",
                    "hidream_i1_full_fp8.safetensors",
                    "ideogram4_fp8_scaled.safetensors",
                    "ideogram4_unconditional_fp8_scaled.safetensors",
                    "qwen_image_2512_fp8_e4m3fn.safetensors",
                    "qwen_image_2.1_int8_convrot.safetensors",
                    "qwen_image_edit_2511_bf16.safetensors",
                    "ming_image_0.1_design_int8_convrot.safetensors",
                    "ming_image_0.1_design_layer_int8_convrot.safetensors",
                    "moss_image_base_bf16.safetensors",
                    "ltx-2.3-22b-dev_transformer_only_fp8_scaled.safetensors",
                    "hunyuanvideo1.5_720p_i2v_fp16.safetensors",
                    "mystery.safetensors",
                ]),
            ),
            (
                "text_encoders".to_string(),
                v(&[
                    "qwen3vl_8b_int8_convrot.safetensors",
                    "qwen3vl_4b_int8_convrot.safetensors",
                    "qwen_2.5_vl_7b_fp8_scaled.safetensors",
                    "qwen_3_4b.safetensors",
                    "qwen_3_8b_fp8mixed.safetensors",
                    "clip_l.safetensors",
                    "t5xxl_fp16.safetensors",
                    "ministral-3-3b.safetensors",
                    "ming_image_0.1_ling_mini_2.0_fp8_scaled.safetensors",
                    "qwen3.5_9b_flux3_prompt_upsampler_w4a8.safetensors",
                    "qwen3.5_9b_int8_convrot.safetensors",
                ]),
            ),
            (
                "vae".to_string(),
                v(&[
                    "ae.safetensors",
                    "flux2-vae.safetensors",
                    "qwen_image_vae.safetensors",
                    "qwen_image_2.1_vae_bf16.safetensors",
                    "ming_image_vae_fp16.safetensors",
                    "flux2-vae_small.safetensors",
                ]),
            ),
            ("checkpoints".to_string(), v(&["hidream_o1_image_bf16.safetensors", "ltx-2.3-22b-dev-fp8.safetensors", "sdxl_base_1.0.safetensors"])),
        ])
    }

    #[test]
    fn recognises_image_models_only() {
        let found = detect(&folders());
        let names: Vec<&str> = found.iter().map(|m| m.unet.as_str()).filter(|u| !u.is_empty()).collect();
        assert_eq!(names.len(), 17, "{names:?}");
        for skipped in ["kontext", "ltx", "hunyuan", "mystery", "unconditional", "layer"] {
            assert!(!names.iter().any(|n| n.contains(skipped)), "{skipped} in {names:?}");
        }
        assert_eq!(found.iter().filter(|m| !m.checkpoint.is_empty()).count(), 2);
    }

    #[test]
    fn hidream_o1_dev_gets_its_own_templates_settings() {
        let mut f = folders();
        f.get_mut("checkpoints").unwrap().extend(["hidream_o1_image_dev_fp8_scaled.safetensors", "hidream_o1_image_mxfp8.safetensors"].map(String::from));
        let found = detect(&f);
        let by = |file: &str| found.iter().find(|m| m.checkpoint == file).cloned().unwrap();
        let dev = by("hidream_o1_image_dev_fp8_scaled.safetensors");
        assert_eq!((dev.family, dev.steps, dev.cfg, dev.shift, dev.sampler.as_str()), (Family::HidreamO1, 28, 1.0, 7.6, "lcm"));
        // The full model, whatever it is quantised to.
        for file in ["hidream_o1_image_bf16.safetensors", "hidream_o1_image_mxfp8.safetensors"] {
            let full = by(file);
            assert_eq!((full.steps, full.cfg, full.shift, full.sampler.as_str(), full.megapixels), (40, 5.0, 8.0, "dpmpp_2m_sde_gpu", 4.2), "{file}");
        }
        // Z-Image Turbo, its encoder and its VAE are on the server: the result is refined. No
        // Gemma 4 is: the prompt goes in as typed.
        assert_eq!(
            (dev.enhancer.as_str(), dev.refiner.as_str(), dev.refiner_clip.as_str(), dev.refiner_vae.as_str(), dev.refine_strength),
            ("", "z_image_turbo_bf16.safetensors", "qwen_3_4b.safetensors", "ae.safetensors", 0.35)
        );
        f.get_mut("text_encoders").unwrap().extend(["gemma4_e2b_it_int8_convrot.safetensors", "gemma4_e4b_it_fp8_scaled.safetensors"].map(String::from));
        f.get_mut("vae").unwrap().retain(|v| v != "ae.safetensors");
        let found = detect(&f);
        let o1 = found.iter().find(|m| m.family == Family::HidreamO1).unwrap();
        // The larger Gemma writes; a refiner missing one of its files is left out whole.
        assert_eq!((o1.enhancer.as_str(), o1.refiner.as_str(), o1.refiner_clip.as_str()), ("gemma4_e4b_it_fp8_scaled.safetensors", "", ""));
    }

    #[test]
    fn dedicated_inpainting_models_are_detected_as_such() {
        let mut f = folders();
        let unets = f.get_mut("diffusion_models").unwrap();
        unets.extend(
            ["flux1-fill-dev.safetensors", "flux.1-fill-dev-OneReward-transformer_fp8.safetensors", "qwen_image_fp8_e4m3fn.safetensors"].map(String::from),
        );
        f.insert("controlnet".into(), vec!["Qwen-Image-InstantX-ControlNet-Inpainting.safetensors".into(), "flux-canny.safetensors".into()]);
        let found = detect(&f);
        let fill: Vec<&Model> = found.iter().filter(|m| m.family == Family::Flux1Fill).collect();
        assert_eq!(fill.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["flux1 fill dev", "flux.1 fill dev OneReward transformer fp8"]);
        // Not mistaken for a text-to-image FLUX.1, and with the encoders the server has.
        assert!(fill.iter().all(|m| m.can_inpaint() && !m.can_generate() && m.guidance == 30.0 && m.clip2 == "t5xxl_fp16.safetensors"));
        assert_eq!(found.iter().filter(|m| m.family == Family::Flux1).count(), 1);
        let qwen: Vec<&Model> = found.iter().filter(|m| m.family == Family::QwenInpaint).collect();
        assert_eq!(qwen.len(), 1);
        assert_eq!(
            (qwen[0].unet.as_str(), qwen[0].controlnet.as_str()),
            ("qwen_image_fp8_e4m3fn.safetensors", "Qwen-Image-InstantX-ControlNet-Inpainting.safetensors")
        );
        // Without the Qwen Image file the template names, the one the server has; without any, none.
        f.get_mut("diffusion_models").unwrap().retain(|u| u != "qwen_image_fp8_e4m3fn.safetensors");
        let found = detect(&f);
        assert_eq!(found.iter().find(|m| m.family == Family::QwenInpaint).map(|m| m.unet.as_str()), Some("qwen_image_2512_fp8_e4m3fn.safetensors"));
        f.get_mut("diffusion_models").unwrap().retain(|u| !u.starts_with("qwen_image_2512"));
        assert!(!detect(&f).iter().any(|m| m.family == Family::QwenInpaint));
    }

    #[test]
    fn qwen_image_edit_is_an_edit_model_with_its_releases_fast_lora() {
        let mut f = folders();
        f.get_mut("diffusion_models").unwrap().push("qwen_image_edit_2509_fp8_e4m3fn.safetensors".into());
        let by = |found: &[Model], file: &str| found.iter().find(|m| m.unet == file).cloned().unwrap();
        let found = detect(&f);
        let edit = by(&found, "qwen_image_edit_2511_bf16.safetensors");
        assert_eq!(
            (edit.family, edit.clip.as_str(), edit.vae.as_str()),
            (Family::QwenEdit, "qwen_2.5_vl_7b_fp8_scaled.safetensors", "qwen_image_vae.safetensors")
        );
        assert!(edit.can_edit() && !edit.can_generate() && !edit.can_inpaint() && problem(&edit).is_none());
        // Without a LoRA: the full schedule, with guidance.
        assert_eq!((edit.lora.as_str(), edit.steps, edit.cfg), ("", 20, 2.5));
        // Each release takes its own four-step LoRA, not the other's and not Qwen Image's.
        f.insert(
            "loras".into(),
            [
                "Qwen-Image-Lightning-4steps-V1.0.safetensors",
                "Qwen-Image-Edit-2509-Lightning-4steps-V1.0-bf16.safetensors",
                "Qwen-Image-Edit-2511-Lightning-8steps.safetensors",
            ]
            .map(String::from)
            .to_vec(),
        );
        let found = detect(&f);
        let older = by(&found, "qwen_image_edit_2509_fp8_e4m3fn.safetensors");
        assert_eq!((older.lora.as_str(), older.steps, older.cfg), ("Qwen-Image-Edit-2509-Lightning-4steps-V1.0-bf16.safetensors", 4, 1.0));
        assert_eq!(by(&found, "qwen_image_edit_2511_bf16.safetensors").lora, "");
        // The models that generate edit too, with the settings they generate with.
        assert!(by(&found, "qwen_image_2.1_int8_convrot.safetensors").can_edit() && by(&found, "flux-2-klein-9b-fp8.safetensors").can_edit());
        assert!(!by(&found, "z_image_turbo_bf16.safetensors").can_edit());
    }

    #[test]
    fn settings_follow_the_variant() {
        let found = detect(&folders());
        let by = |file: &str| found.iter().find(|m| m.unet == file).unwrap();
        let turbo = by("z_image_turbo_bf16.safetensors");
        assert_eq!((turbo.name.as_str(), turbo.steps, turbo.cfg), ("z image turbo bf16", 8, 2.0));
        assert_eq!(by("z_image_bf16.safetensors").steps, 30);
        assert_eq!(by("flux-2-klein-9b-fp8.safetensors").steps, 4);
        assert_eq!(by("flux-2-klein-base-9b-fp8.safetensors").cfg, 5.0);
        let dev = by("flux2_dev_fp8mixed.safetensors");
        assert_eq!((dev.guidance, dev.clip.as_str()), (4.0, "mistral_3_small_flux2_bf16.safetensors"));
        // FLUX.3 Klein is not FLUX.2: its own architecture and encoder, the FLUX.2 VAE. The base
        // model samples as its template does; the step-distilled one in 16 steps without guidance.
        let base = by("flux3_image_klein_base_int8_convrot.safetensors");
        assert_eq!(
            (base.family, base.clip.as_str(), base.vae.as_str(), base.steps, base.cfg),
            (Family::Flux3, "qwen3vl_4b_int8_convrot.safetensors", "flux2-vae.safetensors", 50, 3.5)
        );
        assert!(base.can_generate() && !base.can_edit() && !base.can_inpaint() && problem(base).is_none());
        let sd = by("flux3_image_klein_sd_int8_convrot.safetensors");
        assert_eq!((sd.family, sd.steps, sd.cfg, sd.name.as_str()), (Family::Flux3, 16, 1.0, "flux3 image klein sd int8 convrot"));
        // Without "base" in its name it is still the base model.
        let mut f = folders();
        f.get_mut("diffusion_models").unwrap().push("flux3_image_klein_nvfp4.safetensors".into());
        let found = detect(&f);
        let plain = found.iter().find(|m| m.unet == "flux3_image_klein_nvfp4.safetensors").unwrap();
        assert_eq!((plain.family, plain.steps, plain.cfg), (Family::Flux3, 50, 3.5));
        // BFL's names since 2026-10-17: hyphens, and only "-base-" is the base model.
        f.get_mut("diffusion_models").unwrap().extend(["flux3-image-klein-int8-convrot.safetensors".into(), "flux3-image-klein-base-int8-convrot.safetensors".into()]);
        let found = detect(&f);
        let named = |unet: &str| found.iter().find(|m| m.unet == unet).map(|m| (m.family, m.steps, m.cfg)).unwrap();
        assert_eq!(named("flux3-image-klein-int8-convrot.safetensors"), (Family::Flux3, 16, 1.0));
        assert_eq!(named("flux3-image-klein-base-int8-convrot.safetensors"), (Family::Flux3, 50, 3.5));
        let ernie = by("ernie-image-turbo.safetensors");
        assert_eq!((ernie.steps, ernie.cfg, ernie.megapixels), (8, 1.0, 2.0));
        assert_eq!(by("ideogram4_fp8_scaled.safetensors").cfg, 5.0);
        // The encoder the server has replaces the preset's when that one is missing.
        assert_eq!(by("flux1-dev.safetensors").clip2, "t5xxl_fp16.safetensors");
        assert_eq!(by("ideogram4_fp8_scaled.safetensors").unet2, "ideogram4_unconditional_fp8_scaled.safetensors");
        assert_eq!(by("krea2_turbo_fp8_scaled.safetensors").vae, "qwen_image_vae.safetensors");
        // Qwen Image 2.1 is not Qwen Image: its own encoder and VAE, which the older one can't use.
        let q21 = by("qwen_image_2.1_int8_convrot.safetensors");
        assert_eq!(
            (q21.family, q21.clip.as_str(), q21.vae.as_str()),
            (Family::QwenImage21, "qwen3vl_8b_int8_convrot.safetensors", "qwen_image_2.1_vae_bf16.safetensors")
        );
        let q = by("qwen_image_2512_fp8_e4m3fn.safetensors");
        assert_eq!((q.family, q.clip.as_str(), q.vae.as_str()), (Family::QwenImage, "qwen_2.5_vl_7b_fp8_scaled.safetensors", "qwen_image_vae.safetensors"));
        // Ming Image: its own encoder and VAE (the ones the server has), its template's settings.
        let ming = by("ming_image_0.1_design_int8_convrot.safetensors");
        assert_eq!(
            (ming.family, ming.name.as_str(), ming.clip.as_str(), ming.vae.as_str()),
            (Family::MingImage, "ming image 0.1 design int8 convrot", "ming_image_0.1_ling_mini_2.0_fp8_scaled.safetensors", "ming_image_vae_fp16.safetensors")
        );
        assert_eq!((ming.steps, ming.cfg, ming.megapixels, ming.alpha), (12, 1.0, 2.0, true));
        assert!(ming.can_generate() && ming.can_edit() && problem(ming).is_none());
        // MOSS Image: its Qwen 3.5 encoder (not the prompt writer of the same size beside it), the
        // small FLUX.2 VAE, its template's settings, at the selection's own size.
        let moss = by("moss_image_base_bf16.safetensors");
        assert_eq!(
            (moss.family, moss.clip.as_str(), moss.vae.as_str()),
            (Family::MossImage, "qwen3.5_9b_int8_convrot.safetensors", "flux2-vae_small.safetensors")
        );
        assert_eq!((moss.steps, moss.cfg, moss.sampler.as_str(), moss.megapixels), (25, 2.5, "er_sde", 0.0));
        assert!(moss.can_generate() && !moss.can_edit() && !moss.can_inpaint());
    }

    #[test]
    fn a_qwen_2_1_file_under_qwen_image_settings_is_caught_and_repaired() {
        let found = detect(&folders());
        let right = found.iter().find(|m| m.unet == "qwen_image_2.1_int8_convrot.safetensors").unwrap();
        assert_eq!(problem(right), None);
        // As an older scan left it: the 2.1 file with Qwen Image's architecture, encoder and VAE.
        let wrong = Model {
            id: "qwen-image-2-1-int8-convrot".into(),
            name: "my qwen".into(),
            enabled: false,
            seed: 42,
            megapixels: 2.0,
            unet: right.unet.clone(),
            ..Family::QwenImage.preset()
        };
        let why = problem(&wrong).unwrap();
        assert!(why.contains("my qwen") && why.contains("Qwen Image 2.1 model") && why.contains("Scan Server"), "{why}");
        // The architecture changed by hand but the files left as they were.
        let half = Model { family: Family::QwenImage21, ..wrong.clone() };
        assert!(problem(&half).unwrap().contains("qwen3vl_8b"), "{:?}", problem(&half));
        let vae_only = Model { clip: right.clip.clone(), ..half.clone() };
        assert!(problem(&vae_only).unwrap().contains("VAE"));
        // The other way round, too.
        let reversed = Model { unet: "qwen_image_2512_fp8_e4m3fn.safetensors".into(), ..right.clone() };
        assert!(problem(&reversed).unwrap().contains("set up as Qwen Image 2.1"));
        // Repairing takes the detected architecture and files and keeps the user's own choices.
        let fixed = repaired(&wrong, right);
        assert_eq!(problem(&fixed), None);
        assert_eq!((fixed.family, fixed.clip.as_str(), fixed.vae.as_str(), fixed.steps), (Family::QwenImage21, right.clip.as_str(), right.vae.as_str(), 25));
        assert_eq!(
            (fixed.id.as_str(), fixed.name.as_str(), fixed.enabled, fixed.seed, fixed.megapixels),
            ("qwen-image-2-1-int8-convrot", "my qwen", false, 42, 2.0)
        );
        // Nothing else is second-guessed: every preset, and a file name that says nothing.
        for f in Family::ALL {
            assert_eq!(problem(&f.preset()), None, "{f:?}");
        }
        assert_eq!(problem(&Model { unet: "mystery.safetensors".into(), ..Family::Flux2.preset() }), None);
    }

    #[test]
    fn a_listed_model_takes_the_files_the_server_has_of_it() {
        assert_eq!(variant("z_image_turbo_int8_convrot.safetensors"), "z image turbo");
        assert_eq!(variant("sub/z_image_turbo_bf16.safetensors"), variant("Z_Image_Turbo_fp8_scaled.safetensors"));
        assert_ne!(variant("flux-2-klein-9b-fp8.safetensors"), variant("flux-2-klein-base-9b-fp8.safetensors"));
        assert_ne!(variant("ernie-image-turbo.safetensors"), variant("ernie-image.safetensors"));
        assert_ne!(variant("qwen_image_2.1_int8_convrot.safetensors"), variant("qwen_image_2512_fp8_e4m3fn.safetensors"));
        assert_eq!((variant(""), variant("..."), variant("fp8.safetensors")), (String::new(), "...".to_string(), String::new()));
        // A default local model finds itself in a scan of a server that stores it another way.
        let found = detect(&folders());
        let defaults = Model::defaults();
        let z = defaults.iter().find(|m| m.id == "z-image-turbo").unwrap();
        let stored = found.iter().find(|m| same_variant(z, m)).unwrap();
        assert_eq!(stored.unet, "z_image_turbo_bf16.safetensors");
        let fixed = adopted(z, stored);
        assert_eq!(
            (fixed.id.as_str(), fixed.name.as_str(), fixed.unet.as_str(), fixed.cfg),
            ("z-image-turbo", "Z-Image Turbo", "z_image_turbo_bf16.safetensors", 2.0)
        );
        // The undistilled Z-Image and a cloud model are other models; a nameless file matches nothing.
        assert!(!same_variant(z, found.iter().find(|m| m.unet == "z_image_bf16.safetensors").unwrap()));
        assert!(!same_variant(&defaults[0], &defaults[0]) && !same_variant(&Model::default(), &Model::default()));
        let o1 = defaults.iter().find(|m| m.id == "hidream-o1").unwrap();
        assert!(found.iter().any(|m| same_variant(o1, m)));
        // The server has HiDream O1's checkpoint and not the int8 Z-Image; a cloud model, a custom
        // workflow and a model without a file have nothing to be missing.
        assert!(on_server(&folders(), o1) && !on_server(&folders(), z) && on_server(&folders(), &fixed));
        assert!(on_server(&folders(), &defaults[0]) && on_server(&BTreeMap::new(), &Family::Custom.preset()));
        assert!(on_server(&BTreeMap::new(), &Model::default()) && !on_server(&BTreeMap::new(), z));
    }

    #[test]
    fn empty_and_odd_listings_detect_nothing() {
        assert!(detect(&BTreeMap::new()).is_empty());
        let odd = BTreeMap::from([("diffusion_models".to_string(), vec![String::new(), ".".into(), "/".into(), "ünïcode.safetensors".into()])]);
        assert!(detect(&odd).is_empty());
        assert_eq!(title("sub/dir\\My_Model-v2.safetensors"), "My Model v2");
        assert_eq!(title(""), "");
    }
}
