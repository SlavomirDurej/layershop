//! Generative Fill: Edit › Fill's **AI Gen** contents, which repaints the selection with an image
//! model as a prompt says. One model is always chosen for it ([`FillSettings::choose`]): the one
//! picked in Preferences › Generative AI › Generative Fill while it is offered, else the
//! recommended edit model ([`crate::preferred_edit`]). The model's own rendering settings stay in
//! Manage Models; the few here are the ones that matter for filling a selection.

use serde::{Deserialize, Serialize};

use crate::model::{Model, preferred_edit};

/// How much of the picture around the selection the model is shown by default, as a fraction of
/// the selection's longer edge. More than the AI Gen panel's: a fill has to continue what is
/// around it.
pub const DEFAULT_CONTEXT: f64 = 0.5;
/// The most context a fill shows (what `ai.generate` takes).
pub const MAX_CONTEXT: f64 = 4.0;
/// What the model is told when the Fill dialog's prompt is left empty: take out what is there.
pub const DEFAULT_EMPTY_PROMPT: &str =
    "Remove what is in the selected area and fill it in so that it continues its surroundings seamlessly, matching their light, colour, texture and perspective.";

/// How a model that can change a picture both ways is asked to fill. A model that can do only
/// one is asked that way, whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FillMethod {
    /// The model is shown the picture around the selection and told what to change there
    /// (`ai.generate`'s `imgEdit`).
    #[default]
    Instruct,
    /// The selection is the model's mask, and it paints what the prompt describes inside it
    /// (`ai.generate`'s `inpaint`).
    Inpaint,
}

impl FillMethod {
    pub const ALL: [FillMethod; 2] = [FillMethod::Instruct, FillMethod::Inpaint];

    pub fn key(self) -> &'static str {
        match self {
            FillMethod::Instruct => "instruct",
            FillMethod::Inpaint => "inpaint",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FillMethod::Instruct => "Instruct",
            FillMethod::Inpaint => "Inpaint",
        }
    }
}

/// How much of a local model's schedule an instructed fill runs (`ai.generate`'s `quality`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FillQuality {
    Draft,
    #[default]
    Standard,
    Detail,
}

impl FillQuality {
    pub const ALL: [FillQuality; 3] = [FillQuality::Draft, FillQuality::Standard, FillQuality::Detail];

    pub fn key(self) -> &'static str {
        match self {
            FillQuality::Draft => "draft",
            FillQuality::Standard => "standard",
            FillQuality::Detail => "detail",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FillQuality::Draft => "Draft",
            FillQuality::Standard => "Standard",
            FillQuality::Detail => "Detail",
        }
    }
}

/// `integrations.ai.fill` in the preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FillSettings {
    /// The id of the model that fills (empty: the recommended edit model offered).
    pub model: String,
    /// How a model that edits and inpaints is asked.
    pub method: FillMethod,
    /// How much of the picture around the selection the model is shown, 0..=[`MAX_CONTEXT`].
    pub context: f64,
    /// Instructed fills with a local model: how much of its schedule to run.
    pub quality: FillQuality,
    /// Inpainted fills with a local model: how far the result may depart from what is there,
    /// 0..=1.
    pub strength: f64,
    /// What the model is told when the dialog's prompt is left empty.
    pub empty_prompt: String,
}

impl Default for FillSettings {
    fn default() -> Self {
        Self {
            model: String::new(),
            method: FillMethod::default(),
            context: DEFAULT_CONTEXT,
            quality: FillQuality::default(),
            strength: 1.0,
            empty_prompt: DEFAULT_EMPTY_PROMPT.to_string(),
        }
    }
}

impl FillSettings {
    /// Can `model` fill a selection: it edits or it inpaints.
    pub fn fits(model: &Model) -> bool {
        model.can_edit() || model.can_inpaint()
    }

    /// The model a fill runs, among `offered` (the models that can run now, in the panel's
    /// order): the one chosen while it is offered and fits, else the recommended edit model of
    /// the ones that fit. `None` when none fits.
    pub fn choose<'a>(&self, offered: impl IntoIterator<Item = &'a Model>) -> Option<&'a Model> {
        let fitting: Vec<&Model> = offered.into_iter().filter(|m| Self::fits(m)).collect();
        let wanted = self.model.trim();
        fitting.iter().copied().find(|m| !wanted.is_empty() && m.id == wanted).or_else(|| preferred_edit(fitting.iter().copied()))
    }

    /// Is a fill with `model` painted into the selection as a mask (inpaint), rather than asked
    /// for as an instruction (imgEdit)?
    pub fn inpaints(&self, model: &Model) -> bool {
        match (model.can_edit(), model.can_inpaint()) {
            (true, true) => self.method == FillMethod::Inpaint,
            (edits, _) => !edits,
        }
    }

    /// The context to ask for, within what `ai.generate` takes.
    pub fn context(&self) -> f64 {
        if self.context.is_finite() { self.context.clamp(0.0, MAX_CONTEXT) } else { DEFAULT_CONTEXT }
    }

    /// The strength to ask for, within 0..=1.
    pub fn strength(&self) -> f64 {
        if self.strength.is_finite() { self.strength.clamp(0.0, 1.0) } else { 1.0 }
    }

    /// The prompt a fill sends: what was typed, else the empty-prompt text, else the default one.
    pub fn prompt_for(&self, typed: &str) -> String {
        let typed = typed.trim();
        if !typed.is_empty() {
            return typed.to_string();
        }
        let empty = self.empty_prompt.trim();
        if empty.is_empty() { DEFAULT_EMPTY_PROMPT.to_string() } else { empty.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Family, Provider};

    fn local(id: &str, family: Family) -> Model {
        Model { id: id.into(), name: id.into(), ..family.preset() }
    }

    #[test]
    fn the_chosen_model_fills_while_it_is_offered_else_the_recommended_edit_model() {
        let qwen = local("qwen", Family::QwenImage21);
        let fibo = local("fibo", Family::Fibo2);
        let cloud = Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT", "gpt-image-2", false) };
        let offered = [cloud.clone(), fibo.clone(), qwen.clone()];
        let mut s = FillSettings::default();
        // Nothing chosen: Qwen Image 2.1, ahead of the cloud model listed first.
        assert_eq!(s.choose(&offered).map(|m| m.id.as_str()), Some("qwen"));
        s.model = "fibo".into();
        assert_eq!(s.choose(&offered).map(|m| m.id.as_str()), Some("fibo"));
        // Chosen, but no longer offered: the recommended one again.
        s.model = "gone".into();
        assert_eq!(s.choose(&offered).map(|m| m.id.as_str()), Some("qwen"));
        // No Qwen: the first that fits.
        assert_eq!(s.choose([&cloud, &fibo]).map(|m| m.id.as_str()), Some("gpt"));
        assert!(s.choose(std::iter::empty()).is_none());
    }

    #[test]
    fn a_model_that_cannot_change_a_picture_is_never_chosen() {
        let creator = local("z", Family::ZImage);
        assert!(!FillSettings::fits(&creator), "Z-Image only creates");
        let s = FillSettings { model: "z".into(), ..FillSettings::default() };
        assert!(s.choose([&creator]).is_none());
    }

    #[test]
    fn the_method_only_counts_for_a_model_that_does_both() {
        let edits = local("qwen", Family::QwenImage21);
        let inpaints = local("fill", Family::Flux1Fill);
        let both = Model { inpaint: Some(true), ..edits.clone() };
        let mut s = FillSettings::default();
        assert!(!s.inpaints(&edits));
        assert!(s.inpaints(&inpaints));
        assert!(!s.inpaints(&both));
        s.method = FillMethod::Inpaint;
        assert!(s.inpaints(&both));
        assert!(!s.inpaints(&edits), "a model that only edits is still asked to edit");
    }

    #[test]
    fn an_empty_prompt_asks_for_the_selection_to_be_filled_from_its_surroundings() {
        let mut s = FillSettings::default();
        assert_eq!(s.prompt_for("  a red kite  "), "a red kite");
        assert_eq!(s.prompt_for("   "), DEFAULT_EMPTY_PROMPT);
        s.empty_prompt = "Clean plate".into();
        assert_eq!(s.prompt_for(""), "Clean plate");
        s.empty_prompt = "  ".into();
        assert_eq!(s.prompt_for(""), DEFAULT_EMPTY_PROMPT);
    }

    #[test]
    fn hostile_numbers_are_kept_within_what_generation_takes() {
        let s = FillSettings { context: f64::NAN, strength: f64::INFINITY, ..FillSettings::default() };
        assert_eq!((s.context(), s.strength()), (DEFAULT_CONTEXT, 1.0));
        let s = FillSettings { context: 99.0, strength: -3.0, ..FillSettings::default() };
        assert_eq!((s.context(), s.strength()), (MAX_CONTEXT, 0.0));
    }

    #[test]
    fn settings_saved_before_generative_fill_load_with_its_defaults() {
        let s: crate::Settings = serde_json::from_value(serde_json::json!({"comfyUrl": "http://127.0.0.1:8188"})).unwrap();
        assert_eq!(s.fill, FillSettings::default());
        let s: FillSettings = serde_json::from_value(serde_json::json!({"model": "qwen", "method": "inpaint"})).unwrap();
        assert_eq!((s.model.as_str(), s.method, s.context), ("qwen", FillMethod::Inpaint, DEFAULT_CONTEXT));
    }
}
