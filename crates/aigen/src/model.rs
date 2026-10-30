//! The models a user has set up, as stored in the preferences (`integrations.ai`).

use serde::{Deserialize, Serialize};

/// Where a model runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    /// A local model run by a ComfyUI server.
    #[default]
    Comfy,
    /// OpenAI's image API (GPT Image); needs an API key.
    Openai,
    /// Google's Gemini image API (Nano Banana); needs an API key.
    Gemini,
    /// xAI's image API (Grok Imagine); needs an API key.
    Xai,
    /// Meta's Model API (Muse Image); needs an API key.
    Meta,
    /// Black Forest Labs' API (FLUX): a job is submitted, then polled; needs an API key.
    Bfl,
}

impl Provider {
    pub const ALL: [Provider; 6] = [Provider::Comfy, Provider::Openai, Provider::Gemini, Provider::Xai, Provider::Meta, Provider::Bfl];

    pub fn key(self) -> &'static str {
        match self {
            Provider::Comfy => "comfy",
            Provider::Openai => "openai",
            Provider::Gemini => "gemini",
            Provider::Xai => "xai",
            Provider::Meta => "meta",
            Provider::Bfl => "bfl",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Comfy => "Local (ComfyUI)",
            Provider::Openai => "OpenAI",
            Provider::Gemini => "Google Gemini",
            Provider::Xai => "xAI",
            Provider::Meta => "Meta",
            Provider::Bfl => "Black Forest Labs",
        }
    }

    /// The cloud providers' keys, as `ai.setKey` takes them: `openai|gemini|…`.
    pub fn cloud_keys() -> String {
        Provider::ALL.into_iter().filter(|p| !p.is_local()).map(Provider::key).collect::<Vec<_>>().join("|")
    }

    /// Runs on this machine, without an API key.
    pub fn is_local(self) -> bool {
        self == Provider::Comfy
    }

    /// The environment variable read when no key is stored.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Provider::Comfy => None,
            Provider::Openai => Some("OPENAI_API_KEY"),
            Provider::Gemini => Some("GEMINI_API_KEY"),
            Provider::Xai => Some("XAI_API_KEY"),
            Provider::Meta => Some("META_API_KEY"),
            Provider::Bfl => Some("BFL_API_KEY"),
        }
    }
}

/// The architecture of a local model: which ComfyUI nodes load and sample it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Family {
    #[default]
    ZImage,
    Krea2,
    /// FLUX.3 Klein: its own text encoder type and a plain sampler (ComfyUI's FLUX 3 Image Klein
    /// templates). Text to image only: see [`Family::edits`].
    Flux3,
    Flux2,
    Flux1,
    Ernie,
    HidreamO1,
    HidreamI1,
    Ideogram4,
    QwenImage,
    /// Qwen Image 2.1: a different text encoder, VAE and prompt node from Qwen Image.
    QwenImage21,
    /// Ming Image (0.1 Design): a model for layouts with text (pages, posters, UI). It generates
    /// and edits with the same weights, and can render a transparent background.
    MingImage,
    /// MOSS Image (SFT, Base): text to image from a layout prompt, JSON that places what it
    /// describes in boxes on the canvas ([`crate::comfy`] writes one for a typed prompt).
    MossImage,
    /// FIBO 2 (Bria): one model that generates and edits, in FLUX.2's latent space, with its own
    /// prompt node. It reads plain prompts and instructions, and Bria's structured captions (VGL).
    Fibo2,
    /// FLUX.1 Fill (Fill Dev, OneReward): BFL's inpainting model. It is given the picture and
    /// the mask as conditioning, so it knows where it paints; it can't generate from text alone.
    Flux1Fill,
    /// Qwen Image steered by InstantX's inpainting ControlNet, which is given the picture and
    /// the mask. Inpainting only.
    QwenInpaint,
    /// Qwen Image Edit (2509, 2511): an edit model. It is given a picture, up to two more for
    /// reference, and an instruction; it can't generate from a prompt alone.
    QwenEdit,
    /// An all-in-one checkpoint (Stable Diffusion, SDXL, …).
    Checkpoint,
    /// The user's own workflow (API format) with `%prompt%`-style placeholders.
    Custom,
}

impl Family {
    pub const ALL: [Family; 19] = [
        Family::ZImage,
        Family::Krea2,
        Family::Flux3,
        Family::Flux2,
        Family::Flux1,
        Family::Ernie,
        Family::HidreamO1,
        Family::HidreamI1,
        Family::Ideogram4,
        Family::QwenImage,
        Family::QwenImage21,
        Family::MingImage,
        Family::MossImage,
        Family::Fibo2,
        Family::Flux1Fill,
        Family::QwenInpaint,
        Family::QwenEdit,
        Family::Checkpoint,
        Family::Custom,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Family::ZImage => "zImage",
            Family::Krea2 => "krea2",
            Family::Flux3 => "flux3",
            Family::Flux2 => "flux2",
            Family::Flux1 => "flux1",
            Family::Ernie => "ernie",
            Family::HidreamO1 => "hidreamO1",
            Family::HidreamI1 => "hidreamI1",
            Family::Ideogram4 => "ideogram4",
            Family::QwenImage => "qwenImage",
            Family::QwenImage21 => "qwenImage21",
            Family::MingImage => "mingImage",
            Family::MossImage => "mossImage",
            Family::Fibo2 => "fibo2",
            Family::Flux1Fill => "flux1Fill",
            Family::QwenInpaint => "qwenInpaint",
            Family::QwenEdit => "qwenEdit",
            Family::Checkpoint => "checkpoint",
            Family::Custom => "custom",
        }
    }

    pub fn from_key(key: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::ZImage => "Z-Image",
            Family::Krea2 => "Krea 2",
            Family::Flux3 => "FLUX.3",
            Family::Flux2 => "FLUX.2",
            Family::Flux1 => "FLUX.1",
            Family::Ernie => "Ernie Image",
            Family::HidreamO1 => "HiDream O1",
            Family::HidreamI1 => "HiDream I1",
            Family::Ideogram4 => "Ideogram 4",
            Family::QwenImage => "Qwen Image",
            Family::QwenImage21 => "Qwen Image 2.1",
            Family::MingImage => "Ming Image",
            Family::MossImage => "MOSS Image",
            Family::Fibo2 => "FIBO 2",
            Family::Flux1Fill => "FLUX.1 Fill (inpainting)",
            Family::QwenInpaint => "Qwen Image + InstantX ControlNet (inpainting)",
            Family::QwenEdit => "Qwen Image Edit (editing)",
            Family::Checkpoint => "Checkpoint (SD / SDXL)",
            Family::Custom => "Custom workflow",
        }
    }

    /// Loads everything from one checkpoint file instead of separate model / encoder / VAE files.
    pub fn uses_checkpoint(self) -> bool {
        matches!(self, Family::HidreamO1 | Family::Checkpoint)
    }

    /// How many text encoder files the family loads (0: they come with the checkpoint).
    pub fn text_encoders(self) -> usize {
        match self {
            Family::HidreamO1 | Family::Checkpoint | Family::Custom => 0,
            Family::Flux1 | Family::Flux1Fill => 2,
            Family::HidreamI1 => 4,
            _ => 1,
        }
    }

    /// Has a "shift" (model sampling) control.
    pub fn has_shift(self) -> bool {
        matches!(self, Family::ZImage | Family::HidreamI1 | Family::Ideogram4 | Family::QwenImage | Family::QwenInpaint | Family::QwenEdit | Family::HidreamO1)
    }

    /// The multiple each edge of the rendered image must be.
    pub fn size_multiple(self) -> u32 {
        // HiDream O1 works on pixels in patches of 32.
        if matches!(self, Family::QwenImage21 | Family::HidreamO1) { 32 } else { 16 }
    }

    /// The size a text-to-image for a `width` × `height` selection is rendered at:
    /// [`crate::size::render_size`], but HiDream O1 only does well at the sizes it was trained
    /// at, so it gets the one closest in shape whatever `megapixels` says (0 still renders at
    /// the selection's own size: the user's to choose).
    pub fn render_size(self, width: u32, height: u32, megapixels: f64) -> (u32, u32) {
        use crate::size::{HIDREAM_O1_SIZES, nearest_ratio, render_size};
        // MOSS Image renders any shape at its own size from 0.25 to 4 megapixels (its authors'
        // range): the selection's size within it, and the nearest end of it outside.
        if self == Family::MossImage && !(megapixels.is_finite() && megapixels > 0.0) {
            let area = f64::from(width.max(1)) * f64::from(height.max(1)) / 1e6;
            let (low, high) = MOSS_MEGAPIXELS;
            let megapixels = if (low..=high).contains(&area) { 0.0 } else { area.clamp(low, high) };
            return render_size(width, height, megapixels, self.size_multiple());
        }
        if self == Family::HidreamO1
            && megapixels.is_finite()
            && megapixels > 0.0
            && let Some(size) = nearest_ratio(width, height, &HIDREAM_O1_SIZES)
        {
            return size;
        }
        render_size(width, height, megapixels, self.size_multiple())
    }

    /// The megapixels a picture this family is given (to edit, restyle or inpaint) is worked on
    /// at, when it renders text to image at `megapixels`. Ming Image can be raised to four, but
    /// a picture doubles what it attends to: on an RTX 4090 an edit at four megapixels took 45 s
    /// against 17 s at two, and one stalled for ten minutes encoding the picture beside the
    /// 18 GB text encoder. 0 (the selection's own size) is the user's to choose.
    ///
    /// FIBO 2 was trained at about one megapixel: above it the model's coordinates stay anchored
    /// top left, and the instruction is followed less (its ComfyUI template's notes).
    pub fn source_megapixels(self, megapixels: f64) -> f64 {
        match self {
            Family::MingImage if megapixels > 2.0 => 2.0,
            Family::Fibo2 if megapixels > 1.0 => 1.0,
            _ => megapixels,
        }
    }

    /// Has a distilled-guidance control (FLUX), separate from CFG.
    pub fn has_guidance(self) -> bool {
        matches!(self, Family::Flux1 | Family::Flux2 | Family::Flux1Fill)
    }

    /// Answers a prompt its training flags with a picture of the words "Image blocked by safety
    /// filter" instead of an image. Ideogram 4 does: the words are drawn by the weights (the
    /// graph has no filter node), and a seed that draws them for a harmless prompt draws a
    /// normal image on the next. The engine recognises the picture and tries another seed.
    pub fn draws_refusal(self) -> bool {
        self == Family::Ideogram4
    }

    /// A dedicated inpainting model: it is given the picture and the mask as conditioning, so it
    /// paints what belongs inside the mask, and it can't generate from a prompt alone. These are
    /// the models of ComfyUI's own inpainting templates; the settings list them on their own page
    /// and the panel offers them on the Edit tab only.
    pub fn inpaint_only(self) -> bool {
        matches!(self, Family::Flux1Fill | Family::QwenInpaint)
    }

    /// Offered for inpainting unless the user says otherwise ([`Model::can_inpaint`]): the
    /// dedicated inpainting models, and FLUX.2, an edit model that is given the picture as a
    /// reference. The built-in workflow runs with every other architecture too (encode the
    /// picture, denoise only the masked part), but those don't know where the mask is: tried
    /// against ComfyUI 0.39 they paint a picture of their own inside it, or leave seams.
    pub fn inpaints(self) -> bool {
        self.inpaint_only() || self == Family::Flux2
    }

    /// A dedicated edit model: it changes a picture it is given, following an instruction, and
    /// can't generate from a prompt alone. The panel offers it on the Edit tab only.
    pub fn edit_only(self) -> bool {
        self == Family::QwenEdit
    }

    /// Edits a picture following an instruction (the panel's Edit tab): the edit models, and
    /// the models that generate and edit with the same weights (Qwen Image 2.1, Ming Image,
    /// FLUX.2, FIBO 2). They are shown the picture whole, as a reference, and render a new one
    /// from it.
    ///
    /// Not FLUX.3 Klein, though its templates edit: asked to add a deer to a meadow (2026-10-16,
    /// ComfyUI 0.39, the template's graph) it drew heads growing from chests, fused bodies and
    /// rocks turned half into animals at CFG 3.5, and at the template's CFG 1 no deer or a
    /// shapeless one.
    pub fn edits(self) -> bool {
        matches!(self, Family::QwenImage21 | Family::MingImage | Family::QwenEdit | Family::Flux2 | Family::Fibo2)
    }

    /// How many pictures an edit takes besides the one it edits. Qwen Image Edit's encoder has
    /// three image inputs in all; the others take more than the panel offers.
    pub fn max_references(self) -> usize {
        if self == Family::QwenEdit { 2 } else { MAX_REFERENCES }
    }

    /// A model of this family with the settings its reference workflow uses. File names are the
    /// ones the ComfyUI templates download; the user picks their own in the settings.
    pub fn preset(self) -> Model {
        let base = Model { provider: Provider::Comfy, family: self, name: self.label().to_string(), ..Model::default() };
        match self {
            // CFG 2, not the template's 1: the setting the comparison of 2026-10-13 was judged at.
            Family::ZImage => Model {
                name: "Z-Image Turbo".into(),
                unet: "z_image_turbo_bf16.safetensors".into(),
                clip: "qwen_3_4b.safetensors".into(),
                vae: "ae.safetensors".into(),
                steps: 8,
                cfg: 2.0,
                sampler: "res_multistep".into(),
                scheduler: "simple".into(),
                shift: 3.0,
                ..base
            },
            Family::Krea2 => Model {
                name: "Krea 2 Turbo".into(),
                unet: "krea2_turbo_fp8_scaled.safetensors".into(),
                clip: "qwen3vl_4b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_vae.safetensors".into(),
                steps: 8,
                cfg: 1.0,
                ..base
            },
            // ComfyUI's FLUX 3 Image Klein text-to-image template, undistilled "base" model. The
            // template can also rewrite the prompt with a 9B prompt upsampler first; that is
            // left out.
            Family::Flux3 => Model {
                name: "FLUX.3 Klein".into(),
                unet: "flux3_image_klein_base_int8_convrot.safetensors".into(),
                clip: "qwen3vl_4b_int8_convrot.safetensors".into(),
                vae: "flux2-vae.safetensors".into(),
                steps: 50,
                cfg: 3.5,
                ..base
            },
            Family::Flux2 => Model {
                name: "FLUX.2 Klein 9B".into(),
                unet: "flux-2-klein-base-9b-fp8.safetensors".into(),
                clip: "qwen_3_8b_fp8mixed.safetensors".into(),
                vae: "flux2-vae.safetensors".into(),
                steps: 20,
                cfg: 5.0,
                guidance: 0.0,
                ..base
            },
            Family::Flux1 => Model {
                name: "FLUX.1 Dev".into(),
                unet: "flux1-dev.safetensors".into(),
                clip: "clip_l.safetensors".into(),
                clip2: "t5xxl_fp8_e4m3fn_scaled.safetensors".into(),
                vae: "ae.safetensors".into(),
                steps: 20,
                cfg: 1.0,
                guidance: 3.5,
                ..base
            },
            Family::Ernie => Model {
                name: "Ernie Image".into(),
                unet: "ernie-image.safetensors".into(),
                clip: "ministral-3-3b.safetensors".into(),
                vae: "flux2-vae.safetensors".into(),
                steps: 40,
                cfg: 4.0,
                ..base
            },
            // ComfyUI's HiDream O1 template, size included: 4.2 megapixels is its 2048 x 2048 for
            // a square. At one megapixel the picture came out soft (2026-10-12).
            Family::HidreamO1 => Model {
                name: "HiDream O1".into(),
                checkpoint: "hidream_o1_image_bf16.safetensors".into(),
                steps: 40,
                cfg: 5.0,
                sampler: "dpmpp_2m_sde_gpu".into(),
                scheduler: "normal".into(),
                shift: 8.0,
                megapixels: 4.2,
                ..base
            },
            Family::HidreamI1 => Model {
                name: "HiDream I1".into(),
                unet: "hidream_i1_full_fp8.safetensors".into(),
                clip: "clip_l_hidream.safetensors".into(),
                clip2: "clip_g_hidream.safetensors".into(),
                clip3: "t5xxl_fp8_e4m3fn_scaled.safetensors".into(),
                clip4: "llama_3.1_8b_instruct_fp8_scaled.safetensors".into(),
                vae: "ae.safetensors".into(),
                steps: 50,
                cfg: 5.0,
                sampler: "uni_pc".into(),
                shift: 3.0,
                ..base
            },
            Family::Ideogram4 => Model {
                name: "Ideogram 4".into(),
                unet: "ideogram4_fp8_scaled.safetensors".into(),
                unet2: "ideogram4_unconditional_fp8_scaled.safetensors".into(),
                clip: "qwen3vl_8b_fp8_scaled.safetensors".into(),
                vae: "flux2-vae.safetensors".into(),
                steps: 28,
                // 5, not the template's 7: the setting the comparison of 2026-10-13 was judged at.
                cfg: 5.0,
                shift: 5.0,
                ..base
            },
            Family::QwenImage => Model {
                name: "Qwen Image".into(),
                unet: "qwen_image_2512_fp8_e4m3fn.safetensors".into(),
                clip: "qwen_2.5_vl_7b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_vae.safetensors".into(),
                steps: 20,
                cfg: 2.5,
                shift: 3.1,
                ..base
            },
            Family::QwenImage21 => Model {
                name: "Qwen Image 2.1".into(),
                unet: "qwen_image_2.1_int8_convrot.safetensors".into(),
                clip: "qwen3vl_8b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_2.1_vae_bf16.safetensors".into(),
                steps: 25,
                cfg: 1.0,
                ..base
            },
            // Two megapixels, as ComfyUI's template renders it: small type is garbled at one
            // megapixel, and a transparent background rarely comes out there (measured: 1 of 7 at
            // 1024 x 1024, 14 of 20 above it). At four (2672 x 1504) a photographic scene came out
            // waxy, with a third person the prompt did not ask for; the same graph and seed at
            // 1928 x 1088 matched the template's result.
            // ComfyUI's MOSS Image templates (SFT): faster than its authors' 50 steps at CFG 3
            // with the Euler sampler, the authors' negative prompt. 0 megapixels: the
            // selection's own size ([`Family::render_size`]).
            Family::MossImage => Model {
                name: "MOSS Image".into(),
                unet: "moss_image_sft_int8_convrot.safetensors".into(),
                clip: "qwen3.5_9b_w4a8_gptq.safetensors".into(),
                vae: "flux2-vae_small.safetensors".into(),
                steps: 25,
                cfg: 2.5,
                sampler: "er_sde".into(),
                scheduler: "simple".into(),
                negative_prompt: MOSS_NEGATIVE.into(),
                megapixels: 0.0,
                ..base
            },
            // ComfyUI's FIBO 2 Edit (Dev) template, without its prompt enhancer: the full,
            // undistilled model at Bria's 50 steps and CFG 5, with the template's structured
            // negative. A plain instruction ("add a red deer in the meadow") was followed without
            // the enhancer's rewriting, the rest of the picture kept pixel for pixel (2026-10-30,
            // RTX 4090: 50 s for an edit at one megapixel, 24 s for a text to image). The int8
            // text encoder, not the template's bf16 one: the same edits came out the same at the
            // same speed, and FLUX.3 Klein downloads it already.
            Family::Fibo2 => Model {
                name: "FIBO 2 Dev".into(),
                unet: "fibo2_dev_int8_convrot.safetensors".into(),
                clip: "qwen3vl_4b_int8_convrot.safetensors".into(),
                vae: "flux2-vae.safetensors".into(),
                steps: 50,
                cfg: 5.0,
                negative_prompt: FIBO2_NEGATIVE.into(),
                ..base
            },
            Family::MingImage => Model {
                name: "Ming Image 0.1 Design".into(),
                unet: "ming_image_0.1_design_int8_convrot.safetensors".into(),
                clip: "ming_image_0.1_ling_mini_2.0_int8_convrot.safetensors".into(),
                vae: "ming_image_vae_bf16.safetensors".into(),
                steps: 12,
                cfg: 1.0,
                megapixels: 2.0,
                alpha: true,
                ..base
            },
            Family::Flux1Fill => Model {
                name: "FLUX.1 Fill Dev".into(),
                unet: "flux1-fill-dev.safetensors".into(),
                clip: "clip_l.safetensors".into(),
                clip2: "t5xxl_fp16.safetensors".into(),
                vae: "ae.safetensors".into(),
                steps: 20,
                cfg: 1.0,
                guidance: 30.0,
                scheduler: "normal".into(),
                ..base
            },
            Family::QwenInpaint => Model {
                name: "Qwen Image InstantX Inpainting".into(),
                unet: "qwen_image_fp8_e4m3fn.safetensors".into(),
                clip: "qwen_2.5_vl_7b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_vae.safetensors".into(),
                controlnet: "Qwen-Image-InstantX-ControlNet-Inpainting.safetensors".into(),
                steps: 20,
                cfg: 2.5,
                shift: 3.1,
                ..base
            },
            Family::QwenEdit => Model {
                name: "Qwen Image Edit 2511".into(),
                unet: "qwen_image_edit_2511_bf16.safetensors".into(),
                clip: "qwen_2.5_vl_7b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_vae.safetensors".into(),
                steps: 20,
                cfg: 2.5,
                shift: 3.0,
                ..base
            },
            Family::Checkpoint => Model { steps: 25, cfg: 7.0, scheduler: "normal".into(), ..base },
            Family::Custom => base,
        }
    }
}

/// The sizes MOSS Image renders well, in megapixels: its authors' range.
pub const MOSS_MEGAPIXELS: (f64, f64) = (0.25, 4.0);
/// MOSS Image's negative prompt, as its authors recommend it.
pub const MOSS_NEGATIVE: &str = "nsfw, worst quality, bad anatomy, watermark, jpeg artifact";
/// FIBO 2 Dev's negative prompt, a structured caption of what to steer away from (ComfyUI's
/// FIBO 2 Edit template). Only CFG above 1 reads it: the distilled Turbo model runs without.
pub const FIBO2_NEGATIVE: &str = r#"{"short_description":"","objects":[{"description":"the main subject","texture":"smooth, featureless, plastic-like, airbrushed, no fine surface detail","appearance_details":"soft blurred edges, simplified shapes, no fine detail","skin_tone_and_texture":"waxy airbrushed skin, poreless, plastic smooth"}],"background_setting":"a plain seamless studio backdrop, perfectly smooth gradient, blurred and featureless, no visible texture","lighting":{},"aesthetics":{"artifacts":["oversaturated colours","neon colour cast","overexposed highlights","lens flare","slight blur","motion blur","haze"]},"photographic_characteristics":{"depth_of_field":"shallow, heavy bokeh obscuring most of the frame","focus":"soft, slightly out of focus, lacking fine texture"},"style_medium":"","context":"","artistic_style":""}"#;

/// The most reference pictures an edit is given besides the one it edits (`ai.generate`'s
/// `references`, the Edit tab's rows).
pub const MAX_REFERENCES: usize = 3;

/// The model an edit runs when the user chose none, among `offered` (in the panel's order): a
/// local Qwen Image 2.1, the recommended edit model (the user's call, 2026-10-16), ahead of the
/// cloud models listed first; else the first offered.
pub fn preferred_edit<'a>(offered: impl IntoIterator<Item = &'a Model>) -> Option<&'a Model> {
    let offered: Vec<&Model> = offered.into_iter().collect();
    offered.iter().copied().find(|m| m.provider.is_local() && m.family == Family::QwenImage21).or_else(|| offered.first().copied())
}

/// One image generator and its rendering parameters. Every field is stored for every model so
/// the settings stay a flat record; a family or provider reads only the ones it needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    /// Stable id (`ai.generate {"model": id}`); unique among the user's models.
    pub id: String,
    /// Shown in the AI Gen panel's model list.
    pub name: String,
    pub provider: Provider,
    /// Listed in the AI Gen panel.
    pub enabled: bool,

    // ----- local (ComfyUI) -----
    pub family: Family,
    /// Diffusion model file (`models/diffusion_models` or `models/unet`).
    pub unet: String,
    /// Second diffusion model: Ideogram 4's unconditional model.
    pub unet2: String,
    /// All-in-one checkpoint file (`models/checkpoints`), for checkpoint families.
    pub checkpoint: String,
    /// Text encoder files (`models/text_encoders`); families use the first one, two or four.
    pub clip: String,
    pub clip2: String,
    pub clip3: String,
    pub clip4: String,
    /// VAE file (`models/vae`).
    pub vae: String,
    /// ControlNet file (`models/controlnet`): the inpainting ControlNet of [`Family::QwenInpaint`].
    pub controlnet: String,
    /// Optional LoRA file (`models/loras`) and its strength.
    pub lora: String,
    pub lora_strength: f64,
    /// HiDream O1, text to image: a text encoder that can write (Gemma 4, `models/text_encoders`).
    /// It rewrites the prompt as the one descriptive paragraph the model was trained on before
    /// the picture is rendered. Empty: the prompt goes in as typed.
    pub enhancer: String,
    /// HiDream O1, text to image: the Z-Image Turbo files (diffusion model, text encoder, VAE)
    /// that go over the result once more at `refine_strength` (a denoise, 0..=1), which gives
    /// skin and fabric their fine texture. An empty `refiner` leaves the pass out.
    pub refiner: String,
    pub refiner_clip: String,
    pub refiner_vae: String,
    pub refine_strength: f64,
    pub steps: u32,
    pub cfg: f64,
    pub sampler: String,
    pub scheduler: String,
    /// Model sampling shift (families with [`Family::has_shift`]); HiDream O1's noise scale.
    pub shift: f64,
    /// Distilled guidance (FLUX); 0 leaves the node out.
    pub guidance: f64,
    /// Fixed seed, or -1 for a new one each time.
    pub seed: i64,
    pub negative_prompt: String,
    /// Custom family: the workflow in ComfyUI's API format (Workflow › Export (API)), with
    /// `%prompt%`, `%negative%`, `%width%`, `%height%`, `%seed%`, `%steps%` and `%cfg%` where
    /// those values go; for inpainting also `%image%` and `%mask%` (the names of the uploaded
    /// image and mask, for Load Image nodes) and `%denoise%`.
    pub workflow: String,

    // ----- both -----
    /// The size the model renders at, in megapixels, with the selection's aspect ratio; the
    /// result is then fitted to the selection. 0 renders at the selection's own size.
    pub megapixels: f64,
    /// The model can return a transparent background (the panel's Alpha Channel box).
    pub alpha: bool,
    /// Offered for inpainting (the panel's Edit tab). `None`: what its architecture or
    /// provider can do ([`Model::can_inpaint`]); the settings' checkbox overrides that.
    pub inpaint: Option<bool>,

    // ----- cloud -----
    /// The provider's model id (`gpt-image-2.5-sunburst`, `gemini-3-pro-image-preview`, …). For
    /// Black Forest Labs it is the endpoint (`flux-3-image`).
    pub api_model: String,
    /// OpenAI: `auto`, `low`, `medium`, `high`, and for GPT Image 2.5 `xhigh` or `max`. xAI:
    /// `auto`, `low` or `medium`.
    pub quality: String,
    /// Gemini and Black Forest Labs: `1K`, `2K` or `4K`. xAI: `1K` or `2K`.
    pub image_size: String,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            provider: Provider::Comfy,
            enabled: true,
            family: Family::ZImage,
            unet: String::new(),
            unet2: String::new(),
            checkpoint: String::new(),
            clip: String::new(),
            clip2: String::new(),
            clip3: String::new(),
            clip4: String::new(),
            vae: String::new(),
            controlnet: String::new(),
            lora: String::new(),
            lora_strength: 1.0,
            enhancer: String::new(),
            refiner: String::new(),
            refiner_clip: String::new(),
            refiner_vae: String::new(),
            refine_strength: 0.35,
            steps: 20,
            cfg: 1.0,
            sampler: "euler".into(),
            scheduler: "simple".into(),
            shift: 3.0,
            guidance: 0.0,
            seed: -1,
            negative_prompt: String::new(),
            workflow: String::new(),
            megapixels: 1.0,
            alpha: false,
            inpaint: None,
            api_model: String::new(),
            quality: "auto".into(),
            image_size: "1K".into(),
        }
    }
}

impl Model {
    /// A cloud model with the provider's defaults.
    pub fn cloud(provider: Provider, name: &str, api_model: &str, alpha: bool) -> Model {
        Model { provider, name: name.into(), api_model: api_model.into(), alpha, ..Model::default() }
    }

    /// Offered for inpainting: the user's choice when they made one, else what the model does
    /// well. Local: [`Family::inpaints`], and a custom workflow that takes an `%image%`. Cloud
    /// models can be sent the picture and a mask, but repaint more than was asked: off unless
    /// the user ticks the box.
    pub fn can_inpaint(&self) -> bool {
        self.inpaint.unwrap_or(match self.provider {
            Provider::Comfy if self.family == Family::Custom => self.workflow.contains("%image%"),
            Provider::Comfy => self.family.inpaints(),
            Provider::Openai | Provider::Gemini | Provider::Xai | Provider::Meta | Provider::Bfl => false,
        })
    }

    /// Generates from a prompt alone (the panel's Create tab): everything but the dedicated
    /// inpainting and edit models.
    pub fn can_generate(&self) -> bool {
        !(self.provider.is_local() && (self.family.inpaint_only() || self.family.edit_only()))
    }

    /// Edits a picture following an instruction (the panel's Edit tab). Local:
    /// [`Family::edits`]. Cloud models are sent the picture and the instruction.
    pub fn can_edit(&self) -> bool {
        match self.provider {
            Provider::Comfy => self.family.edits(),
            Provider::Openai | Provider::Gemini | Provider::Xai | Provider::Meta | Provider::Bfl => true,
        }
    }

    /// How many reference pictures an edit with this model takes ([`MAX_REFERENCES`] at most).
    pub fn max_references(&self) -> usize {
        if self.provider.is_local() { self.family.max_references() } else { MAX_REFERENCES }
    }

    /// The cloud models offered by "Add Model". GPT Image 2 stays offered beside 2.5: some
    /// prefer the look of its pictures.
    pub fn cloud_presets() -> Vec<Model> {
        vec![
            Model::cloud(Provider::Openai, "GPT Image 2.5 Sunburst", "gpt-image-2.5-sunburst", true),
            Model::cloud(Provider::Openai, "GPT Image 2.5 Flare", "gpt-image-2.5-flare", true),
            Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false),
            Model::cloud(Provider::Gemini, "Nano Banana 2.1", "gemini-nano-banana-2.1", false),
            Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false),
            Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false),
            Model::cloud(Provider::Xai, "Grok Imagine Image 2.0", "grok-imagine-image-2.0", false),
            Model::cloud(Provider::Meta, "Muse Image", "muse-image-1.0", false),
            Model::cloud(Provider::Bfl, "FLUX 3 Image", "flux-3-image", false),
        ]
    }

    /// The models a new installation lists, best and newest first: the cloud models (in the
    /// order of Artificial Analysis' text-to-image board of 2026-10-13), then the local ones.
    /// Each has the settings it was judged at: one prompt rendered by every model at
    /// 1920 x 1080 on 2026-10-13. The local entries name the files that comparison ran; Scan
    /// Server points an entry at the file the user's server has of the same model
    /// ([`crate::detect::same_variant`]).
    pub fn defaults() -> Vec<Model> {
        let cloud = |name: &str| Model::cloud_presets().into_iter().find(|m| m.name == name);
        let local = |family: Family, name: &str| Model { name: name.into(), ..family.preset() };
        let models = [
            cloud("GPT Image 2.5 Sunburst"),
            cloud("GPT Image 2.5 Flare"),
            cloud("Grok Imagine Image 2.0"),
            cloud("Nano Banana 2"),
            cloud("FLUX 3 Image"),
            cloud("Muse Image"),
            cloud("Nano Banana Pro"),
            Some(local(Family::QwenImage21, "Qwen Image 2.1")),
            Some(Model { unet: "krea2_turbo_int8_convrot.safetensors".into(), ..local(Family::Krea2, "Krea 2 Turbo") }),
            Some(Model { unet: "z_image_turbo_int8_convrot.safetensors".into(), ..local(Family::ZImage, "Z-Image Turbo") }),
            // In place of FLUX.2 Klein 9B since 2026-10-16 (not in that comparison): tried
            // against it on the same server, faster and as good, at its template's settings.
            Some(local(Family::Flux3, "FLUX.3 Klein")),
            // Added 2026-10-18 (not in that comparison): renders at the selection's own size and
            // places its subject to fill it.
            Some(local(Family::MossImage, "MOSS Image")),
            // Added 2026-10-30 (not in that comparison): edits as well as it generates, beside
            // Qwen Image 2.1, which stays the recommended edit model ([`preferred_edit`]).
            Some(local(Family::Fibo2, "FIBO 2 Dev")),
            Some(local(Family::Ideogram4, "Ideogram 4")),
            // With its prompt written by Gemma 4 and the result refined by Z-Image Turbo.
            Some(Model {
                enhancer: "gemma4_e4b_it_fp8_scaled.safetensors".into(),
                refiner: "z_image_turbo_bf16.safetensors".into(),
                refiner_clip: "qwen_3_4b.safetensors".into(),
                refiner_vae: "ae.safetensors".into(),
                ..local(Family::HidreamO1, "HiDream O1")
            }),
            Some(Model { unet: "ernie-image-turbo.safetensors".into(), steps: 8, cfg: 1.0, megapixels: 2.0, ..local(Family::Ernie, "Ernie Image Turbo") }),
            Some(local(Family::MingImage, "Ming Image 0.1 Design")),
        ];
        let mut settings = Settings::default_without_models();
        for mut model in models.into_iter().flatten() {
            model.id = settings.unique_id(&model.name);
            settings.models.push(model);
        }
        settings.models
    }

    /// Where the model stands in the recommended order: lower is better and newer. Cloud models
    /// by their place on Artificial Analysis' text-to-image board (2026-10-13), local ones by
    /// architecture (the text-to-image ones as the comparison of that day left them, then the
    /// older ones and those that only edit or inpaint). A model this table doesn't know comes
    /// after the ones it does, among its own kind.
    pub fn rank(&self) -> usize {
        /// Model ids by their beginning: `gemini-3.1-flash-image` is also its `-preview`.
        const CLOUD: [&str; 12] = [
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
            "gpt-image-2",
            "gemini-nano-banana-2.1",
            "grok-imagine-image-2",
            "gemini-3.1-flash-image",
            "flux-3",
            "muse-image",
            "gpt-image-1.5",
            "gemini-3-pro-image",
            "flux-2",
            "grok-imagine",
        ];
        const LOCAL: [Family; 19] = [
            Family::QwenImage21,
            Family::Krea2,
            Family::ZImage,
            Family::Flux3,
            Family::MossImage,
            Family::Fibo2,
            Family::Flux2,
            Family::Ideogram4,
            Family::HidreamO1,
            Family::Ernie,
            Family::MingImage,
            Family::QwenImage,
            Family::HidreamI1,
            Family::Flux1,
            Family::Checkpoint,
            Family::QwenEdit,
            Family::Flux1Fill,
            Family::QwenInpaint,
            Family::Custom,
        ];
        if self.provider.is_local() {
            LOCAL.iter().position(|f| *f == self.family).unwrap_or(LOCAL.len())
        } else {
            let id = self.api_model.trim();
            CLOUD.iter().position(|known| id.starts_with(known)).unwrap_or(CLOUD.len())
        }
    }

    /// The file that makes a local model what it is: its checkpoint, else its diffusion model.
    pub fn main_file(&self) -> &str {
        if self.checkpoint.trim().is_empty() { self.unet.trim() } else { self.checkpoint.trim() }
    }
}

/// `integrations.ai` in the preferences. API keys are not part of it: they are kept apart from
/// the preferences (which agents can read with `prefs.get`), see the engine's `ai_cmds`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The ComfyUI server that runs local models.
    pub comfy_url: String,
    /// Start ComfyUI when a local model is used and no server answers at `comfy_url` (and stop it
    /// again when the app quits). A server that is already running is used as it is.
    pub comfy_autostart: bool,
    /// How to start it (see [`crate::launch`]).
    pub comfy_launch: crate::launch::Launch,
    /// Give up on a generation after this long.
    pub timeout_seconds: u32,
    /// Offer cloud models in the AI Gen panel's model lists.
    pub show_cloud: bool,
    /// Offer local models in the AI Gen panel's model lists.
    pub show_local: bool,
    /// The models, in the order the user gave them: the panel lists the cloud ones first, then
    /// the local ones, each kind in this order ([`Settings::listed`]).
    pub models: Vec<Model>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { models: Model::defaults(), ..Self::default_without_models() }
    }
}

impl Settings {
    /// The defaults with an empty model list.
    pub fn default_without_models() -> Self {
        Self {
            comfy_url: "http://127.0.0.1:8188".into(),
            comfy_autostart: false,
            comfy_launch: Default::default(),
            timeout_seconds: 600,
            show_cloud: true,
            show_local: true,
            models: Vec::new(),
        }
    }

    /// The positions in `models` of the models of one kind (`local` or cloud), in their order.
    pub fn kind(&self, local: bool) -> impl Iterator<Item = usize> + '_ {
        self.models.iter().enumerate().filter(move |(_, m)| m.provider.is_local() == local).map(|(i, _)| i)
    }

    /// The models the AI Gen panel offers, in its order: the ticked models of the kinds that are
    /// shown, cloud before local, each kind in the order of `models`.
    pub fn listed(&self) -> Vec<&Model> {
        [(false, self.show_cloud), (true, self.show_local)]
            .into_iter()
            .filter(|(_, shown)| *shown)
            .flat_map(|(local, _)| self.kind(local))
            .filter_map(|i| self.models.get(i))
            .filter(|m| m.enabled)
            .collect()
    }

    /// Put the models of each kind in the recommended order ([`Model::rank`]). Models of the
    /// same rank keep the order they had.
    pub fn sort_recommended(&mut self) {
        // Cloud before local in the list itself too, so its order reads as the panel's.
        self.models.sort_by_key(|m| (m.provider.is_local(), m.rank()));
    }

    /// The model with this id, or for an empty id the first the panel offers ([`Self::listed`]).
    pub fn model(&self, id: &str) -> Option<&Model> {
        if id.is_empty() { self.listed().into_iter().next() } else { self.models.iter().find(|m| m.id == id) }
    }

    /// The model with this id, or for an empty id the edit model the panel offers first
    /// ([`preferred_edit`] of [`Self::listed`]): what an edit runs when none was chosen. With no
    /// model that edits, the first listed, so the edit is refused in words.
    pub fn edit_model(&self, id: &str) -> Option<&Model> {
        if !id.is_empty() {
            return self.model(id);
        }
        preferred_edit(self.listed().into_iter().filter(|m| m.can_edit())).or_else(|| self.model(""))
    }

    /// An id for a new model named `name` that no existing model has.
    pub fn unique_id(&self, name: &str) -> String {
        let mut slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "model" } else { slug };
        let mut id = slug.to_string();
        let mut n = 2u32;
        while self.models.iter().any(|m| m.id == id) {
            id = format!("{slug}-{n}");
            n = n.saturating_add(1);
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_missing_and_unknown_keys() {
        let mut s = Settings::default_without_models();
        s.models.push(Model { id: "z".into(), ..Family::ZImage.preset() });
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["models"][0]["family"], "zImage");
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), s);
        let old: Settings = serde_json::from_str(r#"{"models":[{"id":"a","provider":"openai","future":1}]}"#).unwrap();
        assert_eq!(old.models[0].provider, Provider::Openai);
        assert_eq!(old.models[0].steps, 20);
        assert_eq!(old.comfy_url, "http://127.0.0.1:8188");
    }

    #[test]
    fn ids_are_unique_slugs() {
        let mut s = Settings::default_without_models();
        assert_eq!(s.unique_id("Z-Image Turbo"), "z-image-turbo");
        s.models.push(Model { id: "z-image-turbo".into(), ..Model::default() });
        assert_eq!(s.unique_id("Z-Image Turbo"), "z-image-turbo-2");
        assert_eq!(s.unique_id("***"), "model");
    }

    #[test]
    fn the_first_enabled_model_is_the_default() {
        let mut s = Settings::default_without_models();
        assert!(s.model("").is_none());
        s.models.push(Model { id: "a".into(), enabled: false, ..Model::default() });
        s.models.push(Model { id: "b".into(), ..Model::default() });
        assert_eq!(s.model("").map(|m| m.id.as_str()), Some("b"));
        assert_eq!(s.model("a").map(|m| m.id.as_str()), Some("a"));
        assert!(s.model("zz").is_none());
    }

    #[test]
    fn inpainting_follows_the_architecture_unless_the_user_says_otherwise() {
        for f in Family::ALL {
            let want = matches!(f, Family::Flux2 | Family::Flux1Fill | Family::QwenInpaint);
            assert_eq!(f.preset().can_inpaint(), want, "{f:?}");
            // A dedicated inpainting model does nothing else.
            assert_eq!(f.preset().can_generate(), !matches!(f, Family::Flux1Fill | Family::QwenInpaint | Family::QwenEdit), "{f:?}");
        }
        assert!(Model { inpaint: Some(true), ..Family::HidreamO1.preset() }.can_inpaint());
        assert!(!Model { inpaint: Some(false), ..Family::Flux2.preset() }.can_inpaint());
        let custom = Model { family: Family::Custom, workflow: r#"{"1":{"inputs":{"image":"%image%"}}}"#.into(), ..Model::default() };
        assert!(custom.can_inpaint() && !Family::Custom.preset().can_inpaint());
        // Cloud models generate; they inpaint only when the user says so.
        assert!(Model::cloud_presets().iter().all(|m| m.can_generate() && !m.can_inpaint()));
        assert!(Model { inpaint: Some(true), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) }.can_inpaint());
        // Models saved before the setting existed have none: they get their architecture's.
        let old: Model = serde_json::from_str(r#"{"id":"a","family":"flux2"}"#).unwrap();
        assert_eq!((old.inpaint, old.can_inpaint()), (None, true));
    }

    #[test]
    fn editing_follows_the_architecture() {
        for f in Family::ALL {
            let want = matches!(f, Family::QwenImage21 | Family::MingImage | Family::QwenEdit | Family::Flux2 | Family::Fibo2);
            assert_eq!((f.preset().can_edit(), f.edits()), (want, want), "{f:?}");
            assert!(f.max_references() <= MAX_REFERENCES && (!f.edit_only() || f.edits()), "{f:?}");
        }
        // An edit model does nothing else; Qwen Image Edit has room for two references.
        let edit = Family::QwenEdit.preset();
        assert!(edit.can_edit() && !edit.can_generate() && !edit.can_inpaint() && edit.max_references() == 2);
        // Cloud models are sent the picture and the instruction.
        assert!(Model::cloud_presets().iter().all(|m| m.can_edit() && m.max_references() == MAX_REFERENCES));
        // Ming Image generates and edits, at its reference size, and can render transparency.
        let ming = Family::MingImage.preset();
        assert!(ming.can_generate() && ming.can_edit() && !ming.can_inpaint() && ming.alpha);
        assert_eq!((ming.steps, ming.cfg, ming.megapixels, ming.max_references()), (12, 1.0, 2.0, MAX_REFERENCES));
        // Raised to four, it is still given pictures at two; other models, and a lower size, are left.
        assert_eq!(
            (Family::MingImage.source_megapixels(4.0), Family::MingImage.source_megapixels(1.0), Family::MingImage.source_megapixels(0.0)),
            (2.0, 1.0, 0.0)
        );
        assert_eq!(Family::Flux2.source_megapixels(4.0), 4.0);
        assert!(Family::MingImage.source_megapixels(f64::NAN).is_nan());
        // The other local presets don't claim transparency.
        assert!(Family::ALL.into_iter().all(|f| f == Family::MingImage || !f.preset().alpha));
    }

    #[test]
    fn a_new_installation_lists_the_models_that_were_compared() {
        let s = Settings::default();
        let ids: Vec<&str> = s.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "gpt-image-2-5-sunburst",
                "gpt-image-2-5-flare",
                "grok-imagine-image-2-0",
                "nano-banana-2",
                "flux-3-image",
                "muse-image",
                "nano-banana-pro",
                "qwen-image-2-1",
                "krea-2-turbo",
                "z-image-turbo",
                "flux-3-klein",
                "moss-image",
                "fibo-2-dev",
                "ideogram-4",
                "hidream-o1",
                "ernie-image-turbo",
                "ming-image-0-1-design",
            ]
        );
        // That is the recommended order: cloud first, each kind best and newest first.
        let mut sorted = s.clone();
        sorted.models.reverse();
        sorted.sort_recommended();
        assert_eq!(sorted.models, s.models);
        assert!(s.models.windows(2).all(|w| (w[0].provider.is_local(), w[0].rank()) < (w[1].provider.is_local(), w[1].rank())));
        // Every one generates from a prompt and is listed; the first is what an empty id means.
        assert!(s.models.iter().all(|m| m.enabled && m.can_generate() && !m.name.is_empty()));
        assert_eq!(s.model("").map(|m| m.api_model.as_str()), Some("gpt-image-2.5-sunburst"));
        // An edit without a model is Qwen Image 2.1's, though the cloud models are listed first;
        // without it, the first that edits (a cloud model; FLUX.3 Klein doesn't).
        assert_eq!(s.edit_model("").map(|m| m.id.as_str()), Some("qwen-image-2-1"));
        assert_eq!(s.edit_model("ming-image-0-1-design").map(|m| m.id.as_str()), Some("ming-image-0-1-design"));
        let mut without = s.clone();
        without.models.iter_mut().filter(|m| m.family == Family::QwenImage21).for_each(|m| m.enabled = false);
        assert_eq!(without.edit_model("").map(|m| m.id.as_str()), Some("gpt-image-2-5-sunburst"));
        without.show_cloud = false;
        assert_eq!(without.edit_model("").map(|m| m.id.as_str()), Some("fibo-2-dev"));
        assert_eq!(s.edit_model("fibo-2-dev").map(|m| m.id.as_str()), Some("fibo-2-dev"));
        assert_eq!(preferred_edit([]), None);
        // Nothing that edits: the first listed, which refuses in words.
        without.models.retain(|m| m.provider.is_local() && !m.can_edit());
        assert_eq!(without.edit_model("").map(|m| m.id.as_str()), without.model("").map(|m| m.id.as_str()));
        assert!(without.edit_model("").is_some());
    }

    #[test]
    fn the_listed_models_keep_the_settings_they_were_compared_at() {
        let s = Settings::default();
        // The settings the comparison was judged at.
        let by = |id: &str| s.model(id).unwrap();
        assert_eq!((by("z-image-turbo").steps, by("z-image-turbo").cfg, by("z-image-turbo").sampler.as_str()), (8, 2.0, "res_multistep"));
        assert_eq!((by("ideogram-4").steps, by("ideogram-4").cfg, by("ideogram-4").shift), (28, 5.0, 5.0));
        assert_eq!((by("flux-3-klein").steps, by("flux-3-klein").cfg), (50, 3.5));
        assert_eq!((by("ernie-image-turbo").steps, by("ernie-image-turbo").cfg, by("ernie-image-turbo").megapixels), (8, 1.0, 2.0));
        assert_eq!((by("hidream-o1").steps, by("hidream-o1").refiner.as_str()), (40, "z_image_turbo_bf16.safetensors"));
        assert_eq!((by("ming-image-0-1-design").megapixels, by("ming-image-0-1-design").alpha), (2.0, true));
        assert!(s.models.iter().filter(|m| !m.provider.is_local()).all(|m| m.quality == "auto" && m.image_size == "1K"));
        assert_eq!(by("hidream-o1").main_file(), "hidream_o1_image_bf16.safetensors");
        assert_eq!(by("krea-2-turbo").main_file(), "krea2_turbo_int8_convrot.safetensors");
        // The newer cloud models are offered, not listed until they have been tried.
        let offered: Vec<String> = Model::cloud_presets().into_iter().map(|m| m.api_model).collect();
        assert!(offered.contains(&"gemini-nano-banana-2.1".to_string()) && offered.contains(&"gpt-image-2".to_string()));
        assert!(!s.models.iter().any(|m| m.api_model == "gemini-nano-banana-2.1" || m.api_model == "gpt-image-2"));
        for id in ["grok-imagine-image-2.0", "muse-image-1.0", "flux-3-image"] {
            assert!(offered.contains(&id.to_string()), "{id}");
        }
        // Every provider has a key of its own, a label, and but for the local one an environment variable.
        assert_eq!(Provider::cloud_keys(), "openai|gemini|xai|meta|bfl");
        for p in Provider::ALL {
            assert_eq!(serde_json::to_value(p).unwrap(), p.key());
            assert!(!p.label().is_empty() && p.key_env().is_some() != p.is_local(), "{p:?}");
        }
        assert_eq!(serde_json::from_str::<Settings>(r#"{"models":[]}"#).unwrap().models, []);
    }

    #[test]
    fn the_panel_lists_cloud_models_first_in_the_users_order_and_only_the_kinds_shown() {
        let cloud = |id: &str, api: &str| Model { id: id.into(), ..Model::cloud(Provider::Openai, id, api, false) };
        let local = |id: &str, family: Family| Model { id: id.into(), ..family.preset() };
        let mut s = Settings::default_without_models();
        s.models = vec![
            local("ernie", Family::Ernie),
            cloud("pro", "gemini-3-pro-image-preview"),
            local("qwen", Family::QwenImage21),
            cloud("mine", "my-own-model"),
            Model { enabled: false, ..cloud("off", "gpt-image-2") },
            cloud("sunburst", "gpt-image-2.5-sunburst"),
        ];
        let ids = |s: &Settings| s.listed().into_iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        // Cloud before local, each kind in the order it has in the settings; unticked ones left out.
        assert_eq!(ids(&s), ["pro", "mine", "sunburst", "ernie", "qwen"]);
        assert_eq!(s.model("").map(|m| m.id.as_str()), Some("pro"));
        assert_eq!((s.kind(false).collect::<Vec<_>>(), s.kind(true).collect::<Vec<_>>()), (vec![1, 3, 4, 5], vec![0, 2]));
        // A kind that is switched off is not offered, and is not what an empty id means; a model
        // of it can still be asked for by its id.
        s.show_cloud = false;
        assert_eq!(ids(&s), ["ernie", "qwen"]);
        assert_eq!((s.model("").map(|m| m.id.as_str()), s.model("pro").map(|m| m.id.as_str())), (Some("ernie"), Some("pro")));
        s.show_local = false;
        assert!(ids(&s).is_empty() && s.model("").is_none());
        // The recommended order: by rank within each kind, a model the table doesn't know last
        // of its kind, and the unticked one in its place.
        (s.show_cloud, s.show_local) = (true, true);
        s.sort_recommended();
        assert_eq!(s.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["sunburst", "off", "pro", "mine", "qwen", "ernie"]);
        assert!(
            cloud("x", "").rank() > cloud("x", "grok-imagine-image").rank()
                && cloud("x", "gemini-3.1-flash-image").rank() == cloud("x", "gemini-3.1-flash-image-preview").rank()
        );
        // Settings saved before these switches existed show both kinds.
        let old: Settings = serde_json::from_str(r#"{"models":[]}"#).unwrap();
        assert!(old.show_cloud && old.show_local);
        assert_eq!(serde_json::to_value(&old).unwrap()["showCloud"], true);
    }

    #[test]
    fn every_family_has_a_preset_and_a_key() {
        for f in Family::ALL {
            assert_eq!(Family::from_key(f.key()), Some(f));
            assert_eq!(f.preset().family, f);
        }
    }
}
