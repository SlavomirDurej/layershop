//! Guess ready-to-use models from the files a ComfyUI server has ("Detect Local Models").
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
    let fix = "On Preferences › AI Integrations › Local Models, press Scan Server to repair it";
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
            Family::Ernie if turbo => (m.steps, m.cfg) = (8, 1.0),
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
            Family::Krea2 => &["qwen3vl_4b"],
            Family::Flux2 if n.contains("klein") => &["qwen_3_8b"],
            Family::Flux2 => &["mistral_3_small_flux2", "mistral"],
            Family::Flux1 | Family::Flux1Fill | Family::HidreamI1 => &["clip_l"],
            Family::Ernie => &["ministral"],
            Family::Ideogram4 => &["qwen3vl_8b"],
            Family::QwenImage | Family::QwenEdit => &["qwen_2.5_vl"],
            Family::QwenImage21 => &["qwen3vl_8b"],
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
            Family::Flux2 | Family::Ernie | Family::Ideogram4 => &["flux2-vae", "flux2"],
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
                    "flux1-dev.safetensors",
                    "ernie-image-turbo.safetensors",
                    "hidream_i1_full_fp8.safetensors",
                    "ideogram4_fp8_scaled.safetensors",
                    "ideogram4_unconditional_fp8_scaled.safetensors",
                    "qwen_image_2512_fp8_e4m3fn.safetensors",
                    "qwen_image_2.1_int8_convrot.safetensors",
                    "qwen_image_edit_2511_bf16.safetensors",
                    "ltx-2.3-22b-dev_transformer_only_fp8_scaled.safetensors",
                    "hunyuanvideo1.5_720p_i2v_fp16.safetensors",
                    "mystery.safetensors",
                ]),
            ),
            (
                "text_encoders".to_string(),
                v(&[
                    "qwen3vl_8b_int8_convrot.safetensors",
                    "qwen_2.5_vl_7b_fp8_scaled.safetensors",
                    "qwen_3_4b.safetensors",
                    "qwen_3_8b_fp8mixed.safetensors",
                    "clip_l.safetensors",
                    "t5xxl_fp16.safetensors",
                    "ministral-3-3b.safetensors",
                ]),
            ),
            ("vae".to_string(), v(&["ae.safetensors", "flux2-vae.safetensors", "qwen_image_vae.safetensors", "qwen_image_2.1_vae_bf16.safetensors"])),
            ("checkpoints".to_string(), v(&["hidream_o1_image_bf16.safetensors", "ltx-2.3-22b-dev-fp8.safetensors", "sdxl_base_1.0.safetensors"])),
        ])
    }

    #[test]
    fn recognises_image_models_only() {
        let found = detect(&folders());
        let names: Vec<&str> = found.iter().map(|m| m.unet.as_str()).filter(|u| !u.is_empty()).collect();
        assert_eq!(names.len(), 13, "{names:?}");
        for skipped in ["kontext", "ltx", "hunyuan", "mystery", "unconditional"] {
            assert!(!names.iter().any(|n| n.contains(skipped)), "{skipped} in {names:?}");
        }
        assert_eq!(found.iter().filter(|m| !m.checkpoint.is_empty()).count(), 2);
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
        assert_eq!((turbo.name.as_str(), turbo.steps, turbo.cfg), ("z image turbo bf16", 8, 1.0));
        assert_eq!(by("z_image_bf16.safetensors").steps, 30);
        assert_eq!(by("flux-2-klein-9b-fp8.safetensors").steps, 4);
        assert_eq!(by("flux-2-klein-base-9b-fp8.safetensors").cfg, 5.0);
        let dev = by("flux2_dev_fp8mixed.safetensors");
        assert_eq!((dev.guidance, dev.clip.as_str()), (4.0, "mistral_3_small_flux2_bf16.safetensors"));
        assert_eq!(by("ernie-image-turbo.safetensors").steps, 8);
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
    fn empty_and_odd_listings_detect_nothing() {
        assert!(detect(&BTreeMap::new()).is_empty());
        let odd = BTreeMap::from([("diffusion_models".to_string(), vec![String::new(), ".".into(), "/".into(), "ünïcode.safetensors".into()])]);
        assert!(detect(&odd).is_empty());
        assert_eq!(title("sub/dir\\My_Model-v2.safetensors"), "My Model v2");
        assert_eq!(title(""), "");
    }
}
