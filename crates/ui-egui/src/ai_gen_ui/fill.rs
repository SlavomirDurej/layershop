//! Generative Fill: Edit › Fill's **AI Gen** contents ([`crate::fill_ui`]) and its page in
//! Preferences › Generative AI.
//!
//! The Fill dialog shows the model that will fill and takes a prompt; OK runs `ai.generate` on
//! the selection with that model, as a change of the picture (`imgEdit`, the composite of the
//! document around the selection) or, for a model that paints into a mask, an `inpaint` of the
//! active layer. The result is a new layer masked by the selection, as the AI Gen panel's Change
//! makes. One model is always chosen ([`photocraft_aigen::FillSettings::choose`]): the one picked
//! on the page while it can run, else the recommended edit model.

use egui::{RichText, vec2};
use photocraft_aigen::{FillMethod, FillQuality, FillSettings, Model};
use serde_json::{Map, Value, json};

use super::{FILTER, Filter, GENERATE, GOTO, SELECTED, Standing, model_of, tr};
use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// The page's key among the section's pages.
pub(super) const PAGE: &str = "fill";

/// The models a fill can run now, in the panel's order: the ticked models of the kinds shown
/// that edit or inpaint and can run ([`super::usable`]).
fn offered(app: &PhotocraftApp) -> Vec<&Model> {
    app.session.prefs().integrations.ai.listed().into_iter().filter(|m| FillSettings::fits(m) && super::usable(app, m)).collect()
}

/// The model a fill runs now, `None` when no model that edits or inpaints can run.
pub fn model(app: &PhotocraftApp) -> Option<Model> {
    app.session.prefs().integrations.ai.fill.choose(offered(app)).cloned()
}

/// What to say when no model can fill.
pub fn no_model() -> &'static str {
    tr("No AI model that edits or inpaints is ready. Switch one on in Preferences › Generative AI › Manage Models.")
}

/// Why a fill with `model` can't run now, as the user can act on it; `None` when it can.
pub fn blocked(app: &PhotocraftApp, model: &Model) -> Option<String> {
    let Some(doc) = app.session.active() else { return Some(tr("Open a document to fill.").to_string()) };
    if let Err(why) = app.session.ai.ready(model) {
        return Some(why);
    }
    let local = model.provider.is_local();
    if local && let Some(why) = photocraft_aigen::detect::problem(model) {
        return Some(why);
    }
    let s = &app.session.prefs().integrations.ai.fill;
    if s.inpaints(model) {
        if let Err(why) = photocraft_engine::ai_cmds::inpaint_blocked(&app.session) {
            return Some(why);
        }
        if local && let Err(why) = photocraft_aigen::comfy::check(model, true) {
            return Some(why.to_string());
        }
    } else {
        if let Err(why) = photocraft_engine::ai_cmds::edit_plan(&app.session, Some(source(doc.doc.id.0)), s.context(), None) {
            return Some(why);
        }
        if local && let Err(why) = photocraft_aigen::comfy::check_edit(model) {
            return Some(why.to_string());
        }
    }
    if let Some(job) = app.session.job_on(doc.doc.id) {
        return Some(format!("{} ({})", tr("The document is busy"), job.label));
    }
    None
}

/// What an instructed fill changes: the document as it is seen, all its layers together.
fn source(doc: u64) -> photocraft_engine::ai_cmds::SourceRef {
    photocraft_engine::ai_cmds::SourceRef::Document(doc)
}

/// The `ai.generate` params of a fill with `model` and the `typed` prompt (empty: the page's
/// empty-prompt text).
pub fn params(app: &PhotocraftApp, model: &Model, typed: &str) -> Value {
    let s = &app.session.prefs().integrations.ai.fill;
    let mut p = json!({"model": model.id, "prompt": s.prompt_for(typed), "context": s.context(), "references": []});
    if s.inpaints(model) {
        p["mode"] = json!("inpaint");
        p["strength"] = json!(s.strength());
    } else {
        p["mode"] = json!("imgEdit");
        p["quality"] = json!(s.quality.key());
        if let Some(doc) = app.session.active() {
            p["source"] = json!(source(doc.doc.id.0));
        }
    }
    p
}

/// Fill the selection with the Generative Fill model, as `typed` says: a background job where
/// the app has them. Refused in words when no model can fill or the document can't be filled.
pub fn run(app: &mut PhotocraftApp, typed: &str) -> Result<Value, String> {
    let model = model(app).ok_or_else(|| no_model().to_string())?;
    if let Some(why) = blocked(app, &model) {
        return Err(why);
    }
    let params = params(app, &model, typed);
    let result = crate::jobs_ui::run(app, GENERATE, params)?;
    // Inline (no background jobs): the result is here already. A job reports through `finished`.
    if result.get("pending").is_none() {
        super::finished(app, &result);
    }
    Ok(result)
}

/// Open Preferences on Generative AI › Generative Fill.
pub fn open_settings(app: &mut PhotocraftApp) -> u64 {
    let id = crate::prefs_ui::open_preferences(app, photocraft_engine::prefs::GENERATIVE_AI.0);
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert(super::PAGE.into(), json!(PAGE));
    }
    id
}

// ---------- Preferences › Generative AI › Generative Fill ----------

/// A model of the draft the page lists: its index in the draft's models, and the model.
struct Listed {
    index: usize,
    model: Model,
}

/// The draft's models that can fill now, in the panel's order (cloud first, each kind in the
/// list's order): switched on, of a kind that is shown, editing or inpainting, and not lacking a
/// key or its files ([`Standing`]).
fn ready(ai: &Map<String, Value>, f: &Map<String, Value>) -> Vec<Listed> {
    let standing = Standing::of(ai, f);
    let shown = |key: &str| ai.get(key).and_then(Value::as_bool).unwrap_or(true);
    let models: Vec<Listed> = ai
        .get("models")
        .and_then(Value::as_array)
        .map(|list| list.iter().enumerate().filter_map(|(index, v)| model_of(v).map(|model| Listed { index, model })).collect())
        .unwrap_or_default();
    [(false, shown("showCloud")), (true, shown("showLocal"))]
        .into_iter()
        .filter(|(_, on)| *on)
        .flat_map(|(local, _)| models.iter().filter(move |l| l.model.provider.is_local() == local))
        .filter(|l| l.model.enabled && FillSettings::fits(&l.model) && standing.missing(&l.model).is_none())
        .map(|l| Listed { index: l.index, model: l.model.clone() })
        .collect()
}

/// What a model does, in the words of Manage Models' filters, for its row.
fn what_it_does(m: &Model) -> String {
    let mut does = Vec::new();
    if m.can_edit() {
        does.push(tr("Editing"));
    }
    if m.can_inpaint() {
        does.push(tr("Inpainting"));
    }
    let kind = if m.provider.is_local() { tr("Local") } else { m.provider.label() };
    format!("{kind} · {}", does.join(", "))
}

/// A link-styled label; returns its click.
fn link(ui: &mut egui::Ui, text: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let r = ui.add(egui::Label::new(RichText::new(text).color(t.accent_text).size(11.5).underline()).sense(egui::Sense::click()));
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Label column of the page's settings grid.
const LABEL_WIDTH: f32 = 150.0;
/// Height of a model's row.
const ROW_HEIGHT: f32 = 26.0;

/// The page: which model fills (the ones that can, as radio buttons), and the few settings that
/// are the fill's own. `ai` is the draft `integrations.ai`; `f` the dialog's fields.
pub(super) fn page(ui: &mut egui::Ui, ai: &mut Map<String, Value>, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    super::peek_local_server(ai, f);
    let mut s: FillSettings = ai.get("fill").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    super::enhance::wrapped(
        ui,
        tr("Edit › Fill (Shift+Backspace) has an AI Gen choice: it fills the selection with the model chosen here, as your prompt says, on a new layer masked by the selection."),
        t.text_dim,
        13.0,
        width,
    );
    ui.add_space(10.0);
    ui.label(RichText::new(tr("Model")).font(crate::theme::medium(12.5)).color(t.text));
    ui.add_space(2.0);
    super::enhance::wrapped(
        ui,
        tr("The models that are switched on in Manage Models and can run now, and that edit or inpaint. Models that only create pictures can't fill."),
        t.text_faint,
        11.5,
        width,
    );
    ui.add_space(6.0);
    let listed = ready(ai, f);
    let chosen = s.choose(listed.iter().map(|l| &l.model)).map(|m| m.id.clone());
    // The recommended edit model is Qwen Image 2.1 (local); a model that is only first in the
    // list when it is missing is not called recommended.
    let recommended = |m: &Model| m.provider.is_local() && m.family == photocraft_aigen::Family::QwenImage21;
    if listed.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(tr("No model that edits or inpaints is ready yet.")).color(t.warning).size(11.5));
            if link(ui, tr("Manage Models…")) {
                f.insert(FILTER.into(), json!(Filter::Edit.key()));
                f.insert(GOTO.into(), json!("models"));
            }
        });
    }
    for l in &listed {
        let on = chosen.as_deref() == Some(l.model.id.as_str());
        let stroke = if on { egui::Stroke::new(1.5, t.accent) } else { egui::Stroke::new(1.0, t.card_border) };
        egui::Frame::new().fill(t.card).stroke(stroke).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.set_width(width - 22.0);
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT);
                ui.spacing_mut().item_spacing.x = 8.0;
                if super::enhance::radio(ui, on, &l.model.name).clicked() {
                    s.model = l.model.id.clone();
                }
                if recommended(&l.model) {
                    ui.label(RichText::new(tr("Recommended")).color(t.success).size(11.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(what_it_does(&l.model)).color(t.text_faint).size(11.5)).truncate());
                });
            });
        });
        ui.add_space(3.0);
    }
    let Some(current) = chosen.and_then(|id| listed.iter().find(|l| l.model.id == id)) else {
        ai.insert("fill".into(), serde_json::to_value(&s).unwrap_or_else(|_| json!({})));
        return;
    };
    let m = &current.model;
    let local = m.provider.is_local();
    let inpaints = s.inpaints(m);

    ui.add_space(12.0);
    ui.label(RichText::new(format!("{} {}", tr("Settings for"), m.name)).font(crate::theme::medium(12.5)).color(t.text));
    ui.add_space(4.0);
    let field = (width - LABEL_WIDTH - 30.0).clamp(160.0, 420.0);
    let label = |ui: &mut egui::Ui, text: &str| {
        ui.allocate_ui_with_layout(vec2(LABEL_WIDTH, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.label(RichText::new(tr(text)).color(t.text_dim));
        });
    };
    egui::Grid::new("ai-fill-settings").num_columns(2).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
        // How a model that does both is asked; one that does one thing is asked that way.
        if super::both_ways(m) {
            label(ui, "Method:");
            let mut method = s.method;
            let options: Vec<(FillMethod, &str)> = FillMethod::ALL.iter().map(|x| (*x, x.label())).collect();
            widgets::dropdown(ui, "ai-fill-method", tr("Method:"), &mut method, &options, 160.0);
            s.method = method;
            ui.end_row();
        }
        label(ui, "Context:");
        let mut pct = (s.context() * 100.0).round();
        ui.add(egui::DragValue::new(&mut pct).range(0.0..=photocraft_aigen::fill::MAX_CONTEXT * 100.0).speed(1.0).suffix("%").max_decimals(0))
            .on_hover_text(tr("How much of the picture around the selection the model is shown, as a share of the selection's longer side. More helps the fill match its surroundings; less keeps more detail."));
        s.context = pct / 100.0;
        ui.end_row();
        if local && !inpaints {
            label(ui, "Quality:");
            let mut quality = s.quality;
            let options: Vec<(FillQuality, &str)> = FillQuality::ALL.iter().map(|x| (*x, x.label())).collect();
            widgets::dropdown(ui, "ai-fill-quality", tr("Quality:"), &mut quality, &options, 160.0);
            s.quality = quality;
            ui.end_row();
        }
        if local && inpaints {
            label(ui, "Strength:");
            let mut pct = (s.strength() * 100.0).round();
            ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(1.0).suffix("%").max_decimals(0))
                .on_hover_text(tr("How far the fill may depart from what the layer holds there: 100% paints it anew."));
            s.strength = pct / 100.0;
            ui.end_row();
        }
        label(ui, "Empty prompt:");
        ui.vertical(|ui| {
            ui.add(egui::TextEdit::multiline(&mut s.empty_prompt).desired_rows(3).desired_width(field))
                .on_hover_text(tr("What the model is told when the Fill dialog's prompt is left empty."));
            if s.empty_prompt.trim() != photocraft_aigen::fill::DEFAULT_EMPTY_PROMPT && link(ui, tr("Restore the default")) {
                s.empty_prompt = photocraft_aigen::fill::DEFAULT_EMPTY_PROMPT.to_string();
            }
        });
        ui.end_row();
    });
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(format!("{} {}", m.name, tr("renders with its own settings in Manage Models (size, quality, steps…)."))).color(t.text_faint).size(11.5));
        if link(ui, tr("Open them…")) {
            f.insert(SELECTED.into(), json!(current.index));
            f.insert(FILTER.into(), json!(Filter::All.key()));
            f.insert(GOTO.into(), json!("models"));
        }
    });
    ai.insert("fill".into(), serde_json::to_value(&s).unwrap_or_else(|_| json!({})));
}

#[cfg(test)]
mod tests;
