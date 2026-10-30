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
    /// A layer of another open document: its pixels alone. Like [`GenerationSource::Document`],
    /// it lasts for the session.
    DocumentLayer { document: u64, layer: u64 },
}

/// A picture an edit's instruction could refer to, and the name the prompt called it by
/// (`@Luna`); empty when it had none and was referred to by number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationReference {
    #[serde(flatten)]
    pub source: GenerationSource,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl From<GenerationSource> for GenerationReference {
    fn from(source: GenerationSource) -> Self {
        GenerationReference { source, name: String::new() }
    }
}

/// A picture the document keeps for edits to refer to by name (the AI Gen panel's references,
/// `@Luna` in a prompt; [`crate::Document::ai_references`]). Saved with the document, so a
/// character or a prop is still there the next time it is opened.
#[derive(Clone, Debug, PartialEq)]
pub struct AiReference {
    /// The name a prompt calls it by (letters, digits, `_` and `-`); empty: by number.
    pub name: String,
    pub picture: AiPicture,
}

/// Where an [`AiReference`]'s picture is.
#[derive(Clone, Debug, PartialEq)]
pub enum AiPicture {
    /// A layer of this document, by id, read as it is when an edit runs.
    Layer(u64),
    /// This document as it shows, all layers together.
    Document,
    /// A copy kept in the document of a picture from elsewhere (another document, or a layer of
    /// one), as a PNG, and what it was (to show).
    Image { png: std::sync::Arc<Vec<u8>>, label: String },
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
    /// ImgEdit: the pictures the instruction could refer to, with their names, in the order the
    /// panel listed them. The prompt (kept as typed, `@Luna` and all) says which were sent.
    pub references: Vec<GenerationReference>,
    /// ImgEdit: how much of the model's schedule was run.
    pub quality: GenerationQuality,
    /// Txt2img with regions: the boxes the scene was laid out in, front to back, as the document
    /// held them ([`crate::Document::ai_regions`]); Load Prompt puts them back.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<AiRegion>,
}

/// What an [`AiRegion`] holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiRegionKind {
    #[default]
    Object,
    /// A person or creature.
    Character,
    /// Words to render exactly ([`AiRegion::text`]).
    Text,
}

impl AiRegionKind {
    pub const ALL: [AiRegionKind; 3] = [AiRegionKind::Object, AiRegionKind::Character, AiRegionKind::Text];

    /// As the commands name it.
    pub fn key(self) -> &'static str {
        match self {
            AiRegionKind::Object => "object",
            AiRegionKind::Character => "character",
            AiRegionKind::Text => "text",
        }
    }

    pub fn from_key(key: &str) -> Option<AiRegionKind> {
        AiRegionKind::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// A box a text-to-image is told to place something in (the AI Gen panel's Create › Regions,
/// [`crate::Document::ai_regions`]), for models trained on boxes. Kept with the document, so a
/// layout can be refined over many generations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiRegion {
    /// Stable within the document (the commands address a region by it).
    pub id: u32,
    pub kind: AiRegionKind,
    /// What goes in the box; for text, how the words look.
    pub caption: String,
    /// Text regions: the words to render.
    pub text: String,
    /// Document pixels.
    pub rect: crate::Rect,
    /// The outline (and fill) colour it is drawn in on the canvas, sRGB.
    pub color: [u8; 3],
    /// The box is tinted with its colour on the canvas, at `fill_opacity` (0..=1).
    pub fill: bool,
    pub fill_opacity: f32,
}

impl Default for AiRegion {
    fn default() -> Self {
        AiRegion {
            id: 0,
            kind: AiRegionKind::Object,
            caption: String::new(),
            text: String::new(),
            rect: crate::Rect::EMPTY,
            color: [0xf2, 0xb3, 0x3d],
            fill: true,
            fill_opacity: 0.12,
        }
    }
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

    #[test]
    fn a_reference_keeps_its_name_beside_its_source() {
        // Saved before references had names: the source alone.
        let old: Generation = serde_json::from_str(r#"{"mode":"imgEdit","references":[{"layer":4},{"document":2}]}"#).unwrap();
        assert_eq!(old.references, [GenerationSource::Layer(4).into(), GenerationSource::Document(2).into()]);
        let named = GenerationReference { source: GenerationSource::DocumentLayer { document: 3, layer: 9 }, name: "Luna".into() };
        let json = serde_json::to_value(&named).unwrap();
        assert_eq!(json, serde_json::json!({"documentLayer": {"document": 3, "layer": 9}, "name": "Luna"}));
        assert_eq!(serde_json::from_value::<GenerationReference>(json).unwrap(), named);
        // No name: none is written.
        assert_eq!(serde_json::to_value(GenerationReference::from(GenerationSource::Layer(1))).unwrap(), serde_json::json!({"layer": 1}));
        for bad in [r#"{"name":"x"}"#, r#"{"layer":"x"}"#, r#"7"#] {
            assert!(serde_json::from_str::<GenerationReference>(bad).is_err(), "{bad}");
        }
    }
}
