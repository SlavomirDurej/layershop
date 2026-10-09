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

/// The family a diffusion model file belongs to, by name. Edit, video and other non
/// text-to-image models are not recognised.
fn family_of(file: &str) -> Option<Family> {
    let n = file.to_ascii_lowercase();
    if ["edit", "video", "i2v", "t2v", "ltx", "wan", "unconditional", "anima", "controlnet", "inpaint", "kontext"].iter().any(|x| n.contains(x)) {
        return None;
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

/// Models for the text-to-image files in `folders` (as [`crate::comfy_models`] returns them),
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
            _ => {}
        }
        let clip_needles: &[&str] = match family {
            Family::ZImage => &["qwen_3_4b"],
            Family::Krea2 => &["qwen3vl_4b"],
            Family::Flux2 if n.contains("klein") => &["qwen_3_8b"],
            Family::Flux2 => &["mistral_3_small_flux2", "mistral"],
            Family::Flux1 | Family::HidreamI1 => &["clip_l"],
            Family::Ernie => &["ministral"],
            Family::Ideogram4 => &["qwen3vl_8b"],
            Family::QwenImage => &["qwen_2.5_vl"],
            Family::QwenImage21 => &["qwen3vl_8b"],
            _ => &[],
        };
        m.clip = choose(clips, &m.clip, clip_needles);
        match family {
            Family::Flux1 => m.clip2 = choose(clips, &m.clip2, &["t5xxl"]),
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
            Family::Krea2 | Family::QwenImage => &["qwen_image_vae"],
            _ => &["ae.safetensors"],
        };
        m.vae = choose(vaes, &m.vae, vae_needles);
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
    fn recognises_text_to_image_models_only() {
        let found = detect(&folders());
        let names: Vec<&str> = found.iter().map(|m| m.unet.as_str()).filter(|u| !u.is_empty()).collect();
        assert_eq!(names.len(), 12, "{names:?}");
        for skipped in ["edit", "ltx", "hunyuan", "mystery", "unconditional"] {
            assert!(!names.iter().any(|n| n.contains(skipped)), "{skipped} in {names:?}");
        }
        assert_eq!(found.iter().filter(|m| !m.checkpoint.is_empty()).count(), 2);
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
    fn empty_and_odd_listings_detect_nothing() {
        assert!(detect(&BTreeMap::new()).is_empty());
        let odd = BTreeMap::from([("diffusion_models".to_string(), vec![String::new(), ".".into(), "/".into(), "ünïcode.safetensors".into()])]);
        assert!(detect(&odd).is_empty());
        assert_eq!(title("sub/dir\\My_Model-v2.safetensors"), "My Model v2");
        assert_eq!(title(""), "");
    }
}
