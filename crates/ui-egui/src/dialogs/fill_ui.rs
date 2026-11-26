//! Edit › Fill… (Shift+F5, Shift+Backspace): Photoshop's Fill dialog in front of `edit.fill`.
//!
//! Contents (Foreground, Background, Color, Content-Aware, Pattern, History, Black, 50% Gray,
//! White) with the content's own options, then Blending: Mode, Opacity and Preserve
//! Transparency. OK remembers the choices in the saved tier.
//!
//! **AI Gen** is LayerShop's own (Generative Fill): it shows the model chosen in Preferences ›
//! Generative AI › Generative Fill and takes a prompt, and OK runs `ai.generate` on the selection
//! instead of `edit.fill` ([`crate::ai_gen_ui::fill`]). Its result is a new layer masked by the
//! selection, so Blending does not apply and is not shown.

use craft::ui::contract::Bind;
use craft::ui::dialog::DialogId;
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, Param, ParamDialog, ParamKind, ParamResult};
use craft::ui::runtime::env::Label;
use craft::ui::runtime::memory::{MemoryStage, SaveKey, set_saved_value};
use photocraft_color::BlendMode;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::dialogs::blend_options::{blending_params, blending_rows};
use crate::dialogs::params::{captured, open_param};

/// The command of the dialog, which is also its dialog key.
pub const COMMAND: &str = "edit.fill";
const REMEMBER: &str = "photocraft.dialog.edit.fill";
const REMEMBER_INKS: &str = "photocraft.dialog.edit.fill.inks";

/// The Contents menu: (param, label), in Photoshop's order. Color reads without an ellipsis: choosing it opens nothing.
pub const CONTENTS: [(&str, &str); 10] = [
    ("foreground", "Foreground Color"),
    ("background", "Background Color"),
    ("color", "Color"),
    ("contentAware", "Content-Aware"),
    (AI_GEN, "AI Gen"),
    ("pattern", "Pattern"),
    ("history", "History"),
    ("black", "Black"),
    ("gray", "50% Gray"),
    ("white", "White"),
];

/// The Contents choice that fills with an image model (Generative Fill).
pub const AI_GEN: &str = "aiGen";
/// AI Gen's prompt field.
pub const PROMPT: &str = "prompt";
/// The longest prompt the dialog keeps.
const MAX_PROMPT: usize = 8000;

/// AI Gen's rows: the model that fills, the prompt, and why OK can't fill now, when it can't. The
/// model is the one chosen in Preferences › Generative AI › Generative Fill.
fn ai_gen_rows(app: &PhotocraftApp) -> Vec<Param> {
    let model = crate::ai_gen_ui::fill::model(app);
    let name = model.as_ref().map_or_else(|| tl!("None ready").to_string(), |m| m.name.clone());
    let note = match &model {
        None => crate::ai_gen_ui::fill::no_model().to_string(),
        Some(m) => crate::ai_gen_ui::fill::blocked(app, m).unwrap_or_else(|| tl!("The result is a new layer, masked by the selection.").to_string()),
    };
    vec![
        Param::read_only_text("aiModel", tl!("Model:"), name).visible_when("contents", AI_GEN),
        Param::new(PROMPT, tl!("Prompt:"), ParamKind::text(""))
            .multiline(3)
            .max_chars(MAX_PROMPT)
            .hint(tl!("Describe what to put in the selection, or leave it empty to remove what is there"))
            .visible_when("contents", AI_GEN),
        Param::read_only_text("aiNote", "", note).visible_when("contents", AI_GEN),
    ]
}

/// The contents other than AI Gen: the ones Blending applies to.
fn blended_contents() -> impl Iterator<Item = &'static str> {
    CONTENTS.iter().map(|(k, _)| *k).filter(|k| *k != AI_GEN)
}

/// `#rrggbb` of an sRGB colour: the form the engine's colour params take.
pub(crate) fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The dialog's rows.
fn rows(app: &PhotocraftApp, foreground: [f32; 3], first_pattern: Option<String>) -> Vec<Param> {
    let contents = CONTENTS.iter().map(|(k, l)| (k.to_string(), Label::from(tl!(l)))).collect();
    let mut rows = vec![
        Param::new("contents", tl!("Contents:"), ParamKind::choice(contents, Some("foreground".into()))),
        Param::new("color", tl!("Color:"), ParamKind::color(Some(foreground))).visible_when("contents", "color"),
        Param::new("pattern", tl!("Custom Pattern:"), ParamKind::pattern(first_pattern)).visible_when("contents", "pattern"),
        Param::new("colorAdaptation", tl!("Color Adaptation"), ParamKind::bool(true)).visible_when("contents", "contentAware"),
    ];
    rows.extend(ai_gen_rows(app));
    // AI Gen's result is a new layer masked by the selection: nothing to blend.
    rows.extend(blending_rows(app).into_iter().map(|row| row.visible_when_in("contents", blended_contents())));
    rows
}

/// Photoshop's Fill defaults, which Alt+Reset restores instead of the last-used values.
fn defaults(foreground: [f32; 3], first_pattern: Option<String>, inks: Option<[f32; 4]>) -> DialogFields {
    let mut f = DialogFields::new();
    f.set("contents", "foreground");
    f.set("color", FieldValue::Color(foreground));
    f.set(crate::color_picker_ui::ink_field("color"), crate::color_picker_ui::ink_value(craft::theme::Srgba::from_rgb_array(foreground), inks));
    if let Some(id) = first_pattern {
        f.set("pattern", id);
    }
    f.set("colorAdaptation", true);
    f.set("mode", BlendMode::Normal.key().to_string());
    f.set("opacity", 100.0);
    f.set("preserveTransparency", false);
    f
}

/// Open Fill with the current editing target captured, so live UI changes cannot redirect confirmation.
pub fn open(app: &mut PhotocraftApp) -> DialogId {
    let target = app.with_mask_target(COMMAND, json!({}));
    let target = target.get("target").cloned().unwrap_or_else(|| crate::canvas::paint_target(app));
    // Alpha and Quick Mask belong to the document, independently of its active layer.
    let layer_bound = target.as_str() != Some("quickMask") && target.get("channel").is_none();
    let mut held = json!({ "target": target });
    if let Some(st) = app.session.active() {
        held["document"] = json!(st.doc.id.0);
        if layer_bound {
            held["layer"] = json!(st.active_layer);
        }
    }
    let [r, g, b, _] = app.session.tools.foreground;
    let first = app.session.patterns.items.first().map(|p| p.id.clone());
    let patterns = crate::panels::preset_browser::pattern_swatches(app);
    let rows = rows(app, [r, g, b], first.clone());
    let inks = crate::color_picker_ui::ink_value(craft::theme::Srgba::rgb(r, g, b), app.session.tools.picked_role_cmyk(0));
    let reset = defaults([r, g, b], first, app.session.tools.picked_role_cmyk(0));
    let mut fields = DialogFields::new();
    let remembered: Option<DialogFields> = MemoryStage::peek_saved(app.ctx(), &SaveKey::named(REMEMBER_INKS));
    let remembered_inks = remembered.as_ref().and_then(|saved| saved.get(&crate::color_picker_ui::ink_field("color"))).cloned();
    fields.set(crate::color_picker_ui::ink_field("color"), remembered_inks.unwrap_or(inks));
    let title = Label::from(tl!("Fill"));
    open_param(app, COMMAND, fields, held, move |fields| {
        ParamDialog::new(COMMAND, title.clone(), rows.clone())
            .values(Bind::bound(fields))
            .remember(SaveKey::named(REMEMBER))
            .reset_to(reset.clone())
            .patterns(crate::panels::preset_browser::pattern_source(patterns.clone()))
    })
}

/// The `edit.fill` params of the dialog's fields: only the options of the chosen contents, and the captured target.
pub fn params(f: &DialogFields, held: &Value) -> Value {
    let contents = f.text("contents").unwrap_or("foreground");
    let mut p = blending_params(f);
    p.insert("contents".into(), json!(contents));
    match contents {
        "color" => {
            if let Some(c) = f.color("color") {
                p.insert("color".into(), json!(hex(c)));
                if let Some(inks) = crate::color_picker_ui::request_inks(f, "color", craft::theme::Srgba::from_rgb_array(c)) {
                    p.insert("colorCmyk".into(), json!(inks));
                }
            }
        }
        "pattern" => {
            if let Some(id) = f.text("pattern") {
                p.insert("pattern".into(), json!(id));
            }
        }
        "contentAware" => {
            if let Some(on) = f.bool("colorAdaptation") {
                p.insert("colorAdaptation".into(), json!(on));
            }
        }
        _ => {}
    }
    if let Some(target) = held.get("target") {
        p.insert("target".into(), target.clone());
    }
    Value::Object(p)
}

/// Whether the dialog `key` is the Fill dialog.
pub(crate) fn owns(key: &str) -> bool {
    key == COMMAND
}

/// OK is held until the document and layer are the ones the dialog opened on.
pub(crate) fn check(app: &mut PhotocraftApp, result: &ParamResult) -> Result<(), Label> {
    let captured = &captured(app, &result.key);
    let st = app.session.active().ok_or_else(|| Label::from(tl!("The document is closed")))?;
    if captured.get("document").and_then(Value::as_u64) != Some(st.doc.id.0) || captured.get("layer").is_some_and(|layer| layer != &json!(st.active_layer)) {
        return Err(Label::from(tl!("The document or active layer changed. Open Fill again.")));
    }
    // AI Gen with no model that can fill, or a document it can't fill (no pixels under the
    // selection, a job running on it, …), keeps the dialog open and says why.
    if result.fields.text("contents") == Some(AI_GEN) {
        let model = crate::ai_gen_ui::fill::model(app).ok_or_else(|| Label::from(crate::ai_gen_ui::fill::no_model().to_string()))?;
        if let Some(why) = crate::ai_gen_ui::fill::blocked(app, &model) {
            return Err(Label::from(why));
        }
    }
    Ok(())
}

/// Fill with the confirmed choices; they are remembered only when the fill succeeded.
pub(crate) fn run(app: &mut PhotocraftApp, ctx: &egui::Context, result: ParamResult) {
    let captured = captured(app, &result.key);
    let outcome = if result.fields.text("contents") == Some(AI_GEN) {
        let prompt: String = result.fields.text(PROMPT).unwrap_or("").chars().take(MAX_PROMPT).collect();
        crate::ai_gen_ui::fill::run(app, &prompt)
    } else {
        app.run(COMMAND, params(&result.fields, &captured))
    };
    match outcome {
        Ok(_) => {
            result.remember(ctx);
            // The framework remembers visible parameter rows only; keep their ink companion in the same saved tier.
            let mut inks = DialogFields::new();
            if let Some(value) = result.fields.get(&crate::color_picker_ui::ink_field("color")) {
                inks.set(crate::color_picker_ui::ink_field("color"), value.clone());
            }
            set_saved_value(ctx, &SaveKey::named(REMEMBER_INKS), Some(inks));
        }
        Err(e) => crate::notices::error(app, e),
    }
}

#[cfg(test)]
mod tests;
