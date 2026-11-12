//! Edit › Fill… (Shift+F5, Shift+Backspace): Photoshop's Fill dialog in front of `edit.fill`.
//!
//! Contents (Foreground, Background, Color…, Content-Aware, AI Gen, Pattern, History, Black,
//! 50% Gray, White) with the content's own options, then Blending: Mode, Opacity and Preserve
//! Transparency. The fields are the command's params, so `ui.dialog.set` / `ui.dialog.confirm`
//! drive it like any dialog. OK remembers the choices in the preferences (`dialogs["edit.fill"]`),
//! which are saved to disk, so the dialog opens with them again after a restart.
//!
//! **AI Gen** is LayerShop's own (Generative Fill): it shows the model chosen in Preferences ›
//! Generative AI › Generative Fill and takes a prompt, and OK runs `ai.generate` on the selection
//! instead of `edit.fill` ([`crate::ai_gen_ui::fill`]). Its result is a new layer masked by the
//! selection, so Blending does not apply and is not shown.

use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

pub const COMMAND: &str = "edit.fill";
const MARK: &str = "__fill";

/// The Contents menu: (param, label), in Photoshop's order.
pub const CONTENTS: [(&str, &str); 10] = [
    ("foreground", "Foreground Color"),
    ("background", "Background Color"),
    ("color", "Color…"),
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
/// Asked by AI Gen's Change… link: the dialog loop opens Preferences on Generative Fill
/// ([`open_requested`]), since the body has no app to open it with.
const OPEN_SETTINGS: &str = "__fillAiSettings";

/// Params the dialog edits and remembers. AI Gen's prompt is kept too, so an undone fill can be
/// tried again with it.
const KEYS: [&str; 8] = ["contents", "color", "pattern", "colorAdaptation", "mode", "opacity", "preserveTransparency", PROMPT];

/// Is this dialog the Fill dialog?
pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// A remembered value, if it is one the dialog can show (a corrupt preference is ignored).
fn valid(app: &PhotocraftApp, key: &str, v: &Value) -> bool {
    match key {
        "contents" => v.as_str().is_some_and(|c| CONTENTS.iter().any(|(k, _)| *k == c)),
        "mode" | "opacity" | "preserveTransparency" => crate::dialog_blend_ui::valid(key, v),
        "color" => v.as_str().is_some_and(|h| h.len() == 7 && h.starts_with('#') && h.get(1..).is_some_and(|d| d.chars().all(|c| c.is_ascii_hexdigit()))),
        "pattern" => v.as_str().is_some_and(|p| app.session.patterns.items.iter().any(|q| q.id == p)),
        "colorAdaptation" => v.is_boolean(),
        PROMPT => v.as_str().is_some_and(|p| p.chars().count() <= MAX_PROMPT),
        _ => false,
    }
}

/// Photoshop's Fill defaults (no last-used prefs). Used by open and Alt+Reset.
pub fn default_fields(app: &PhotocraftApp) -> Map<String, Value> {
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__command".into(), json!(COMMAND));
    f.insert("__label".into(), json!("Fill"));
    f.insert("contents".into(), json!("foreground"));
    f.insert("color".into(), json!(hex(app.session.tools.foreground)));
    if let Some(p) = app.session.patterns.items.first() {
        f.insert("pattern".into(), json!(p.id));
    }
    f.insert("colorAdaptation".into(), json!(true));
    f.insert("mode".into(), json!("normal"));
    f.insert("opacity".into(), json!(100.0));
    f.insert("preserveTransparency".into(), json!(false));
    f.insert(PROMPT.into(), json!(""));
    f
}

/// The dialog's fields: Photoshop's defaults, overridden by the last choices.
pub fn fields(app: &PhotocraftApp) -> Map<String, Value> {
    let mut f = default_fields(app);
    if let Some(Value::Object(saved)) = app.session.prefs().dialogs.get(COMMAND) {
        for k in KEYS {
            if let Some(v) = saved.get(k).filter(|v| valid(app, k, v)) {
                f.insert(k.into(), v.clone());
            }
        }
    }
    f
}

/// Alt+Cancel (Reset): Photoshop Fill defaults, not last-used prefs. Dialog stays open.
pub fn reset(app: &PhotocraftApp, fields: &mut Map<String, Value>) {
    if !owns(fields) {
        return;
    }
    let defaults = default_fields(app);
    for k in KEYS {
        if let Some(v) = defaults.get(k) {
            fields.insert(k.into(), v.clone());
        }
    }
}

/// Open Fill with the current editing target, so live UI changes cannot redirect confirmation.
pub fn open(app: &mut PhotocraftApp) -> u64 {
    let mut f = fields(app);
    let target = app.with_mask_target(COMMAND, json!({}));
    let target = target.get("target").cloned().unwrap_or_else(|| crate::canvas::paint_target(app));
    if let Some(st) = app.session.active() {
        f.insert("__document".into(), json!(st.doc.id.0));
        // Alpha and Quick Mask belong to the document, independently of its active layer.
        if target.as_str() != Some("quickMask") && target.get("channel").is_none() {
            f.insert("__layer".into(), json!(st.active_layer));
        }
    }
    f.insert("__target".into(), target);
    app.ui.open_dialog(crate::state::DialogKind::Command, f)
}

/// The `edit.fill` params of the dialog's fields: only the options of the chosen contents.
pub fn params(f: &Map<String, Value>) -> Value {
    let contents = f.get("contents").and_then(Value::as_str).unwrap_or("foreground");
    let mut p = json!({ "contents": contents });
    for k in ["mode", "opacity", "preserveTransparency"] {
        if let Some(v) = f.get(k) {
            p[k] = v.clone();
        }
    }
    let extra = match contents {
        "color" => Some("color"),
        "pattern" => Some("pattern"),
        "contentAware" => Some("colorAdaptation"),
        _ => None,
    };
    if let Some(k) = extra
        && let Some(v) = f.get(k)
    {
        p[k] = v.clone();
    }
    if let Some(target) = f.get("__target") {
        p["target"] = target.clone();
    }
    p
}

/// Is AI Gen the chosen contents?
fn ai_gen(f: &Map<String, Value>) -> bool {
    f.get("contents").and_then(Value::as_str) == Some(AI_GEN)
}

/// AI Gen's prompt, within what the dialog keeps.
fn prompt(f: &Map<String, Value>) -> String {
    f.get(PROMPT).and_then(Value::as_str).unwrap_or("").chars().take(MAX_PROMPT).collect()
}

/// Before OK closes the dialog: AI Gen with no model that can fill, or a document it can't fill
/// (no pixels under the selection, a job running on it, …), keeps it open and says why.
pub fn preflight(app: &PhotocraftApp, f: &Map<String, Value>) -> Result<(), String> {
    if !owns(f) || !ai_gen(f) {
        return Ok(());
    }
    let model = crate::ai_gen_ui::fill::model(app).ok_or_else(|| crate::ai_gen_ui::fill::no_model().to_string())?;
    crate::ai_gen_ui::fill::blocked(app, &model).map_or(Ok(()), Err)
}

/// OK: remember the choices, then fill.
pub fn confirm(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<Value, String> {
    let st = app.session.active().ok_or(tl!("The document is closed"))?;
    if f.get("__document").and_then(Value::as_u64) != Some(st.doc.id.0) || f.get("__layer").is_some_and(|layer| layer != &json!(st.active_layer)) {
        return Err(tl!("The document or active layer changed. Open Fill again.").into());
    }
    let remembered: Map<String, Value> = KEYS.iter().filter_map(|k| f.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
    app.session.prefs.edit(|p| p.dialogs.insert(COMMAND.into(), Value::Object(remembered)));
    if ai_gen(f) {
        return crate::ai_gen_ui::fill::run(app, &prompt(f));
    }
    app.run(COMMAND, params(f))
}

/// AI Gen's Change… link was clicked: open Preferences on Generative Fill over the dialog.
pub fn open_requested(app: &mut PhotocraftApp, id: u64) {
    let asked = app.ui.dialog_mut(id).is_some_and(|d| owns(&d.fields) && d.fields.remove(OPEN_SETTINGS).is_some());
    if asked {
        crate::ai_gen_ui::fill::open_settings(app);
    }
}

/// Label column bounds for [`crate::widgets::label_column_width`]: the English labels' width, and a cap.
const LABEL_MIN_W: f32 = 112.0;
const LABEL_MAX_W: f32 = 240.0;
/// The labels above the Blending section, measured with its own in the current language.
const LABELS: [&str; 5] = ["Contents:", "Color:", "Custom Pattern:", "Model:", "Prompt:"];

fn get_str(f: &Map<String, Value>, k: &str, d: &str) -> String {
    f.get(k).and_then(Value::as_str).unwrap_or(d).to_string()
}

fn parse_hex(h: &str) -> [u8; 3] {
    let d = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(0);
    [d(1), d(3), d(5)]
}

/// The dialog body.
pub fn body(app: &PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    let labels: Vec<&str> = LABELS.iter().chain(&crate::dialog_blend_ui::LABELS).copied().map(crate::i18n::t).collect();
    let label_w = crate::widgets::label_column_width(ui, &labels, LABEL_MIN_W, LABEL_MAX_W);
    let label = |ui: &mut egui::Ui, text: &str| {
        crate::widgets::column_label(ui, text, label_w);
    };
    let mut contents = get_str(f, "contents", "foreground");
    ui.horizontal(|ui| {
        label(ui, tl!("Contents:"));
        let opts: Vec<(String, &str)> = CONTENTS.iter().map(|(k, l)| (k.to_string(), *l)).collect();
        if crate::widgets::dropdown(ui, "fill-contents", tl!("Contents:"), &mut contents, &opts, 170.0) {
            f.insert("contents".into(), json!(contents));
            // Choosing Color… opens the Color Picker, as in Photoshop.
            if contents == "color" {
                crate::color_picker_ui::request(f, tl!("Color Picker (Fill Color)"));
            }
        }
    });
    match contents.as_str() {
        "color" => {
            ui.horizontal(|ui| {
                label(ui, tl!("Color:"));
                // Photoshop shows no swatch; this one reopens the Color Picker on the colour.
                let rgb = parse_hex(&get_str(f, "color", "#000000"));
                if crate::widgets::color_swatch_button(ui, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), tl!("Fill color")).clicked() {
                    crate::color_picker_ui::request(f, tl!("Color Picker (Fill Color)"));
                }
            });
        }
        "pattern" => {
            ui.horizontal(|ui| {
                label(ui, tl!("Custom Pattern:"));
                let pat = get_str(f, "pattern", "");
                if app.session.patterns.items.is_empty() {
                    ui.label(egui::RichText::new(tl!("No patterns")).color(Tokens::get(ui.ctx()).text_faint));
                } else {
                    // Photoshop shows the pattern itself and opens a grid of swatches.
                    if let Some(id) = crate::preset_panels::pattern_picker(app, ui, &pat) {
                        f.insert("pattern".into(), json!(id));
                    }
                    let name = app.session.patterns.items.iter().find(|p| p.id == get_str(f, "pattern", "")).map(|p| p.display_name().to_string());
                    ui.label(egui::RichText::new(name.unwrap_or_default()).color(Tokens::get(ui.ctx()).text_dim));
                }
            });
        }
        "contentAware" => {
            ui.horizontal(|ui| {
                label(ui, "");
                let mut on = f.get("colorAdaptation").and_then(Value::as_bool).unwrap_or(true);
                if crate::widgets::checkbox(ui, &mut on, "Color Adaptation").changed() {
                    f.insert("colorAdaptation".into(), json!(on));
                }
            });
        }
        AI_GEN => {
            ai_gen_body(app, ui, f, label_w);
            // The result is a new layer masked by the selection: nothing to blend.
            return;
        }
        _ => {}
    }
    crate::dialog_blend_ui::body(ui, f, "fill", label_w);
}

/// Width of AI Gen's prompt box.
const PROMPT_W: f32 = 300.0;

/// AI Gen: the model that fills (with a link to choose another), the prompt, and why OK can't
/// fill now, when it can't.
fn ai_gen_body(app: &PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>, label_w: f32) {
    let t = Tokens::get(ui.ctx());
    let tr = crate::i18n::t;
    let model = crate::ai_gen_ui::fill::model(app);
    let link = |ui: &mut egui::Ui, text: &str| {
        let r = ui.add(egui::Label::new(egui::RichText::new(text).color(t.accent_text).underline()).sense(egui::Sense::click()));
        r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr("Preferences › Generative AI › Generative Fill")).clicked()
    };
    ui.horizontal(|ui| {
        crate::widgets::column_label(ui, tr("Model:"), label_w);
        let open = match &model {
            Some(m) => {
                ui.label(egui::RichText::new(&m.name).color(t.text).strong());
                link(ui, tr("Change…"))
            }
            None => {
                ui.label(egui::RichText::new(tr("None ready")).color(t.warning));
                link(ui, tr("Set Up…"))
            }
        };
        if open {
            f.insert(OPEN_SETTINGS.into(), json!(true));
        }
    });
    ui.horizontal_top(|ui| {
        crate::widgets::column_label(ui, tr("Prompt:"), label_w);
        let mut text = prompt(f);
        let hint = tr("Describe what to put in the selection, or leave it empty to remove what is there");
        let edit = egui::TextEdit::multiline(&mut text).id_salt("fill-ai-prompt").desired_rows(3).desired_width(PROMPT_W).hint_text(hint);
        if ui.add(edit).changed() {
            f.insert(PROMPT.into(), json!(text.chars().take(MAX_PROMPT).collect::<String>()));
        }
    });
    let note = match &model {
        None => (crate::ai_gen_ui::fill::no_model().to_string(), t.warning),
        Some(m) => match crate::ai_gen_ui::fill::blocked(app, m) {
            Some(why) => (why, t.warning),
            None => (tr("The result is a new layer, masked by the selection.").to_string(), t.text_faint),
        },
    };
    ui.horizontal(|ui| {
        crate::widgets::column_label(ui, "", label_w);
        ui.scope(|ui| {
            ui.set_max_width(PROMPT_W);
            ui.add(egui::Label::new(egui::RichText::new(note.0).color(note.1).size(11.5)).wrap());
        });
    });
}

#[cfg(test)]
mod tests;
