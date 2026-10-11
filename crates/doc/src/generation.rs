//! What a generative-AI layer was made from: the model, the prompt and the settings of the
//! generation, kept on the layer so they can be read back (and the generation repeated or varied)
//! long after it ran. Pure data: the engine's `ai.generate` writes it, the AI Gen panel loads it
//! when the layer is clicked.

use serde::{Deserialize, Serialize};

/// What a generation worked from (`ai.generate`'s `mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationMode {
    /// The prompt alone.
    #[default]
    Txt2img,
    /// The selected part of a layer was repainted.
    Inpaint,
    /// A picture was redrawn.
    Img2img,
}

impl GenerationMode {
    /// As `ai.generate` names it.
    pub fn key(self) -> &'static str {
        match self {
            GenerationMode::Txt2img => "txt2img",
            GenerationMode::Inpaint => "inpaint",
            GenerationMode::Img2img => "img2img",
        }
    }
}

/// The picture an image-to-image started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationSource {
    /// A layer of the same document, by id.
    Layer(u64),
    /// Another open document, by id. Document ids last for the session: a saved file does not
    /// keep this.
    Document(u64),
}

/// The generation a layer came from ([`crate::Layer::generation`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Generation {
    pub mode: GenerationMode,
    /// The model's id in the settings (`integrations.ai.models`); it may since have been removed.
    pub model: String,
    /// The model's name when the layer was generated, to show when the model is gone.
    pub model_name: String,
    pub prompt: String,
    /// How far the result could depart from what was there, 0..=1 (inpaint, img2img).
    pub strength: f64,
    /// Inpaint: how much of the layer around the selection the model was shown, as a fraction of
    /// the selection's longer edge.
    pub context: f64,
    /// A transparent background was asked for.
    pub alpha: bool,
    /// The seed the image was rendered with.
    pub seed: u64,
    /// Img2img: the picture that was redrawn.
    pub source: Option<GenerationSource>,
}
