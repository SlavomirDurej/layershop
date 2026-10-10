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
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Comfy, Provider::Openai, Provider::Gemini];

    pub fn key(self) -> &'static str {
        match self {
            Provider::Comfy => "comfy",
            Provider::Openai => "openai",
            Provider::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Comfy => "Local (ComfyUI)",
            Provider::Openai => "OpenAI",
            Provider::Gemini => "Google Gemini",
        }
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
    Flux2,
    Flux1,
    Ernie,
    HidreamO1,
    HidreamI1,
    Ideogram4,
    QwenImage,
    /// Qwen Image 2.1: a different text encoder, VAE and prompt node from Qwen Image.
    QwenImage21,
    /// FLUX.1 Fill (Fill Dev, OneReward): BFL's inpainting model. It is given the picture and
    /// the mask as conditioning, so it knows where it paints; it can't generate from text alone.
    Flux1Fill,
    /// Qwen Image steered by InstantX's inpainting ControlNet, which is given the picture and
    /// the mask. Inpainting only.
    QwenInpaint,
    /// An all-in-one checkpoint (Stable Diffusion, SDXL, …).
    Checkpoint,
    /// The user's own workflow (API format) with `%prompt%`-style placeholders.
    Custom,
}

impl Family {
    pub const ALL: [Family; 14] = [
        Family::ZImage,
        Family::Krea2,
        Family::Flux2,
        Family::Flux1,
        Family::Ernie,
        Family::HidreamO1,
        Family::HidreamI1,
        Family::Ideogram4,
        Family::QwenImage,
        Family::QwenImage21,
        Family::Flux1Fill,
        Family::QwenInpaint,
        Family::Checkpoint,
        Family::Custom,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Family::ZImage => "zImage",
            Family::Krea2 => "krea2",
            Family::Flux2 => "flux2",
            Family::Flux1 => "flux1",
            Family::Ernie => "ernie",
            Family::HidreamO1 => "hidreamO1",
            Family::HidreamI1 => "hidreamI1",
            Family::Ideogram4 => "ideogram4",
            Family::QwenImage => "qwenImage",
            Family::QwenImage21 => "qwenImage21",
            Family::Flux1Fill => "flux1Fill",
            Family::QwenInpaint => "qwenInpaint",
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
            Family::Flux2 => "FLUX.2",
            Family::Flux1 => "FLUX.1",
            Family::Ernie => "Ernie Image",
            Family::HidreamO1 => "HiDream O1",
            Family::HidreamI1 => "HiDream I1",
            Family::Ideogram4 => "Ideogram 4",
            Family::QwenImage => "Qwen Image",
            Family::QwenImage21 => "Qwen Image 2.1",
            Family::Flux1Fill => "FLUX.1 Fill (inpainting)",
            Family::QwenInpaint => "Qwen Image + InstantX ControlNet (inpainting)",
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
        matches!(self, Family::ZImage | Family::HidreamI1 | Family::Ideogram4 | Family::QwenImage | Family::QwenInpaint | Family::HidreamO1)
    }

    /// The multiple each edge of the rendered image must be.
    pub fn size_multiple(self) -> u32 {
        // HiDream O1 works on pixels in patches of 32.
        if matches!(self, Family::QwenImage21 | Family::HidreamO1) { 32 } else { 16 }
    }

    /// Has a distilled-guidance control (FLUX), separate from CFG.
    pub fn has_guidance(self) -> bool {
        matches!(self, Family::Flux1 | Family::Flux2 | Family::Flux1Fill)
    }

    /// A dedicated inpainting model: it is given the picture and the mask as conditioning, so it
    /// paints what belongs inside the mask, and it can't generate from a prompt alone. These are
    /// the models of ComfyUI's own inpainting templates; the settings list them on their own page
    /// and the panel offers them on the InPaint tab only.
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

    /// A model of this family with the settings its reference workflow uses. File names are the
    /// ones the ComfyUI templates download; the user picks their own in the settings.
    pub fn preset(self) -> Model {
        let base = Model { provider: Provider::Comfy, family: self, name: self.label().to_string(), ..Model::default() };
        match self {
            Family::ZImage => Model {
                name: "Z-Image Turbo".into(),
                unet: "z_image_turbo_bf16.safetensors".into(),
                clip: "qwen_3_4b.safetensors".into(),
                vae: "ae.safetensors".into(),
                steps: 8,
                cfg: 1.0,
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
            Family::HidreamO1 => Model {
                name: "HiDream O1".into(),
                checkpoint: "hidream_o1_image_bf16.safetensors".into(),
                steps: 40,
                cfg: 3.6,
                sampler: "euler_ancestral".into(),
                scheduler: "normal".into(),
                shift: 8.0,
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
                cfg: 7.0,
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
            Family::Checkpoint => Model { steps: 25, cfg: 7.0, scheduler: "normal".into(), ..base },
            Family::Custom => base,
        }
    }
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
    /// Offered for inpainting (the panel's InPaint tab). `None`: what its architecture or
    /// provider can do ([`Model::can_inpaint`]); the settings' checkbox overrides that.
    pub inpaint: Option<bool>,

    // ----- cloud -----
    /// The provider's model id (`gpt-image-2.5-sunburst`, `gemini-3-pro-image-preview`, …).
    pub api_model: String,
    /// OpenAI: `auto`, `low`, `medium`, `high`, and for GPT Image 2.5 `xhigh` or `max`.
    pub quality: String,
    /// Gemini: `1K`, `2K` or `4K`.
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
            Provider::Openai | Provider::Gemini => false,
        })
    }

    /// Generates from a prompt alone (the panel's Txt2Img tab): everything but the dedicated
    /// inpainting models.
    pub fn can_generate(&self) -> bool {
        !(self.provider.is_local() && self.family.inpaint_only())
    }

    /// The cloud models offered by "Add Cloud Model".
    pub fn cloud_presets() -> Vec<Model> {
        vec![
            Model::cloud(Provider::Openai, "GPT Image 2.5 Sunburst", "gpt-image-2.5-sunburst", true),
            Model::cloud(Provider::Openai, "GPT Image 2.5 Flare", "gpt-image-2.5-flare", true),
            Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false),
            Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false),
            Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false),
        ]
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
    pub models: Vec<Model>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { comfy_url: "http://127.0.0.1:8188".into(), comfy_autostart: false, comfy_launch: Default::default(), timeout_seconds: 600, models: Vec::new() }
    }
}

impl Settings {
    /// The model with this id, or the first enabled one for an empty id.
    pub fn model(&self, id: &str) -> Option<&Model> {
        if id.is_empty() { self.models.iter().find(|m| m.enabled) } else { self.models.iter().find(|m| m.id == id) }
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
        let mut s = Settings::default();
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
        let mut s = Settings::default();
        assert_eq!(s.unique_id("Z-Image Turbo"), "z-image-turbo");
        s.models.push(Model { id: "z-image-turbo".into(), ..Model::default() });
        assert_eq!(s.unique_id("Z-Image Turbo"), "z-image-turbo-2");
        assert_eq!(s.unique_id("***"), "model");
    }

    #[test]
    fn the_first_enabled_model_is_the_default() {
        let mut s = Settings::default();
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
            assert_eq!(f.preset().can_generate(), !matches!(f, Family::Flux1Fill | Family::QwenInpaint), "{f:?}");
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
    fn every_family_has_a_preset_and_a_key() {
        for f in Family::ALL {
            assert_eq!(Family::from_key(f.key()), Some(f));
            assert_eq!(f.preset().family, f);
        }
    }
}
