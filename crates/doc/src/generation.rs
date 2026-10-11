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
    /// A picture was changed following an instruction.
    ImgEdit,
}

impl GenerationMode {
    /// As `ai.generate` names it.
    pub fn key(self) -> &'static str {
        match self {
            GenerationMode::Txt2img => "txt2img",
            GenerationMode::Inpaint => "inpaint",
            GenerationMode::Img2img => "img2img",
            GenerationMode::ImgEdit => "imgEdit",
        }
    }
}

/// How much of a model's sampling schedule a generation ran (`ai.generate`'s `quality`): fewer
/// steps to try an idea quickly, more for difficult detail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationQuality {
    Draft,
    /// The steps the model is set up with.
    #[default]
    Standard,
    Detail,
}

impl GenerationQuality {
    pub const ALL: [GenerationQuality; 3] = [GenerationQuality::Draft, GenerationQuality::Standard, GenerationQuality::Detail];

    /// As `ai.generate` names it.
    pub fn key(self) -> &'static str {
        match self {
            GenerationQuality::Draft => "draft",
            GenerationQuality::Standard => "standard",
            GenerationQuality::Detail => "detail",
        }
    }

    pub fn from_key(key: &str) -> Option<GenerationQuality> {
        GenerationQuality::ALL.into_iter().find(|q| q.key() == key)
    }

    /// The steps to sample for a model set up with `steps`: about a quarter for a draft (four at
    /// least, which is what the distilled models take anyway), a fifth more for detail.
    pub fn steps(self, steps: u32) -> u32 {
        let steps = steps.clamp(1, 1000);
        match self {
            GenerationQuality::Draft => (steps / 4).max(steps.min(4)),
            GenerationQuality::Standard => steps,
            GenerationQuality::Detail => steps.saturating_add(steps.div_ceil(5)).min(1000),
        }
    }
}

/// A picture a generation started from or referred to.
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
    /// Img2img, imgEdit: the picture that was redrawn or edited.
    pub source: Option<GenerationSource>,
    /// ImgEdit: the pictures the instruction could refer to, in the order the model numbered
    /// them after the source.
    pub references: Vec<GenerationSource>,
    /// ImgEdit: how much of the model's schedule was run.
    pub quality: GenerationQuality,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_scales_the_models_steps() {
        use GenerationQuality::{Detail, Draft, Standard};
        for (steps, draft, detail) in [(25, 6, 30), (20, 5, 24), (4, 4, 5), (8, 4, 10), (1, 1, 2), (0, 1, 2), (u32::MAX, 250, 1000)] {
            assert_eq!((Draft.steps(steps), Standard.steps(steps), Detail.steps(steps)), (draft, steps.clamp(1, 1000), detail), "{steps}");
        }
        for q in GenerationQuality::ALL {
            assert_eq!(GenerationQuality::from_key(q.key()), Some(q));
        }
        assert_eq!(GenerationQuality::from_key("best"), None);
        // A layer saved before edits existed has neither field.
        let old: Generation = serde_json::from_str(r#"{"mode":"img2img","prompt":"x"}"#).unwrap();
        assert_eq!((old.references.len(), old.quality), (0, Standard));
        assert_eq!(serde_json::to_value(GenerationMode::ImgEdit).unwrap(), "imgEdit");
    }
}
