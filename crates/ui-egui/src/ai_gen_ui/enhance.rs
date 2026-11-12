//! Prompt enhancement in the shell: the AI Gen panel's Enhance Prompt button beside the model
//! (Shift-click puts back the prompt as it was), the veil over the prompt while the answer is
//! awaited (what it is waiting for, for how long, and Cancel), and Preferences › Generative AI ›
//! Prompt Enhancement: Local Models (none yet), Cloud Models (the one service that rewrites
//! prompts, a radio group, each with its model and a button to its key) and Settings (the
//! length, the timeout and the instructions, edited in place).
//!
//! The work is the engine's `ai.enhancePrompt`, run as a background job; the panel follows it by
//! its id ([`super::AiGenUi::enhance_job`]) and puts the answer in the prompt box.

use egui::{RichText, vec2};
use photocraft_aigen::enhance::{DEFAULT_INSTRUCTIONS, MAX_TIMEOUT, MAX_WORDS, MIN_TIMEOUT, MIN_WORDS, local};
use photocraft_aigen::{EnhanceSettings, TextProvider};
use photocraft_doc::GenerationMode;
use photocraft_engine::jobs::{JobEvent, JobInfo, JobOutcome};
use serde_json::{Map, Value, json};

use super::{ACTION, GOTO, KEY_FOCUS, KEY_FOCUS_ONCE, KEYS, tr};
use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// The engine command that enhances.
pub const ENHANCE: &str = "ai.enhancePrompt";
/// Press the panel's Enhance: the prompt box's text is enhanced for the task in front and the
/// style chosen, and the answer replaces it. `{"again": bool?}` (true: the prompt as typed before
/// the last enhancement, for the style chosen now) → `{"job": id}` (or, run inline, the
/// command's result).
pub const ENHANCE_MENU: &str = "window.panel.aiGen.enhance";
/// Put back the prompt as it was before the last enhancement: `{}` → `{"restored": bool}`.
pub const RESTORE_MENU: &str = "window.panel.aiGen.restorePrompt";

/// Dialog fields: the model listing running (`{id, provider}`), the models each service listed
/// (`{provider: [id…]}`), what the last listing said (`{provider, text, error}`), and the filter
/// typed over each service's list (`{provider: text}`).
const LIST_JOB: &str = "__aiEnhanceListJob";
const LISTED: &str = "__aiEnhanceListed";
const LIST_STATUS: &str = "__aiEnhanceStatus";
const LIST_FILTER: &str = "__aiEnhanceFilter";
/// The page's action that lists a service's models: `enhanceModels:<provider>`.
const LIST: &str = "enhanceModels:";

/// The panel's enhancement while it runs.
pub(super) fn running(app: &PhotocraftApp) -> Option<JobInfo> {
    let id = app.ui.ai_gen.enhance_job?;
    app.session.jobs().into_iter().find(|j| j.id.0 == id)
}

/// The service prompts are enhanced with, and its model.
fn chosen(app: &PhotocraftApp) -> (TextProvider, String) {
    let s = &app.session.prefs().integrations.ai.enhance;
    (s.provider, s.model(s.provider))
}

/// Why Enhance can't run now (its tooltip, and the reason it is off), or `None`.
pub(super) fn blocked(app: &PhotocraftApp) -> Option<String> {
    if app.session.ai.transport.is_none() {
        return Some(tr("Prompt enhancement needs the desktop app").to_string());
    }
    let (provider, _) = chosen(app);
    if !provider.is_local() && app.session.ai.text_key(provider).is_empty() {
        return Some(format!("{} {}", provider.label(), tr("needs an API key: Preferences › Generative AI › API Keys")));
    }
    if app.ui.ai_gen.prompt.trim().is_empty() {
        return Some(tr("Write a prompt first: Enhance adds detail to it").to_string());
    }
    None
}

/// Enhance the panel's prompt for `mode` (the task in front), the image model `model` and the
/// style preset chosen. `again`, after an enhancement: enhance the prompt as it was typed once
/// more (for a style chosen since), rather than the enhanced one.
pub fn start(app: &mut PhotocraftApp, mode: GenerationMode, model: Option<&str>, again: bool) -> Result<Value, String> {
    if running(app).is_some() {
        return Err(tr("The prompt is being enhanced already").to_string());
    }
    if let Some(why) = blocked(app) {
        return Err(why);
    }
    let prompt = match &app.ui.ai_gen.before_enhance {
        Some(typed) if again && !typed.trim().is_empty() => typed.clone(),
        _ => app.ui.ai_gen.prompt.clone(),
    };
    let mut params = json!({"prompt": prompt, "mode": mode.key()});
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        params["model"] = json!(model);
    }
    if let Some(style) = super::styles::param(app) {
        params["style"] = json!(style);
    }
    let result = crate::jobs_ui::run(app, ENHANCE, params)?;
    match result.get("job").and_then(Value::as_u64).filter(|_| result.get("pending").is_some()) {
        Some(job) => app.ui.ai_gen.enhance_job = Some(job),
        // Inline (no background jobs): the answer is here already.
        None => finished(app, &result),
    }
    Ok(result)
}

/// An enhancement ended with `result` (`ai.enhancePrompt`'s): its prompt replaces the panel's;
/// the one it was written from (the typed one, enhanced again) is kept to go back to, with the
/// style it was written for.
fn finished(app: &mut PhotocraftApp, result: &Value) {
    let Some(prompt) = result.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) else { return };
    let before = std::mem::replace(&mut app.ui.ai_gen.prompt, prompt.to_string());
    let original = result.get("original").and_then(Value::as_str).filter(|o| !o.trim().is_empty()).map(str::to_string);
    app.ui.ai_gen.before_enhance = Some(original.unwrap_or(before));
    app.ui.ai_gen.enhanced_style = result.get("style").and_then(Value::as_str).unwrap_or("").to_string();
    let provider = result.get("provider").and_then(Value::as_str).and_then(TextProvider::from_key).unwrap_or_default();
    let model = result.get("textModel").and_then(Value::as_str).unwrap_or("");
    app.ui.ai_gen.enhanced_by = format!("{} ({model})", provider.short_label());
}

/// The end of a job: the panel's enhancement is taken here. Returns whether it was that.
pub(super) fn on_job_event(app: &mut PhotocraftApp, e: &JobEvent) -> bool {
    if app.ui.ai_gen.enhance_job != Some(e.id.0) {
        return false;
    }
    app.ui.ai_gen.enhance_job = None;
    match &e.outcome {
        JobOutcome::Done(result) => finished(app, result),
        JobOutcome::Failed(err) => crate::notices::error(app, format!("{}: {err}", tr("Enhance Prompt"))),
        JobOutcome::Cancelled => {
            app.ui.status = tr("Prompt enhancement cancelled").to_string();
            app.ui.status_error = false;
        }
    }
    true
}

/// Put back the prompt as it was before the last enhancement. Returns whether there was one.
pub fn restore(app: &mut PhotocraftApp) -> bool {
    match app.ui.ai_gen.before_enhance.take() {
        Some(before) => {
            app.ui.ai_gen.prompt = before;
            true
        }
        None => false,
    }
}

/// After an enhancement, while the prompt is as it answered: who wrote it ("Enhanced by OpenAI
/// (gpt-5.4-nano)"), for the status bar and the button's tooltip.
pub(super) fn enhanced_by(app: &PhotocraftApp) -> Option<String> {
    app.ui.ai_gen.before_enhance.as_ref().map(|_| format!("{} {}", tr("Enhanced by"), app.ui.ai_gen.enhanced_by))
}

/// After an enhancement, when the style preset chosen is not the one the prompt was written for
/// (chosen, changed or taken away since): the status bar's warning. Enhance again writes it for
/// the one chosen now, from the prompt as typed.
pub(super) fn style_mismatch(app: &PhotocraftApp) -> Option<String> {
    let ui = &app.ui.ai_gen;
    if ui.before_enhance.is_none() || running(app).is_some() {
        return None;
    }
    let now = super::styles::chosen(app);
    let then = photocraft_aigen::style::find(&ui.enhanced_style);
    if now.map(|s| s.id) == then.map(|s| s.id) {
        return None;
    }
    Some(match (then, now) {
        (None, Some(now)) => format!("{} {} {}", tr("The prompt was enhanced without a style and may work against"), now.name, tr("(a \u{201c}photorealistic\u{201d} prompt, say).")),
        (Some(then), Some(now)) => format!("{} {} {} {}.", tr("The prompt was enhanced for"), then.name, tr("and may work against"), now.name),
        (Some(then), None) => format!("{} {}, {}", tr("The prompt was enhanced for"), then.name, tr("which is no longer chosen.")),
        (None, None) => return None,
    })
}

/// What Shift-click does, for the tooltips: put back the prompt as it was, after an enhancement.
pub(super) fn restore_tip(app: &PhotocraftApp) -> String {
    match enhanced_by(app) {
        Some(_) => tr("Shift-click Enhance prompt to restore the original prompt.").to_string(),
        None => tr("After an enhancement, Shift-click restores the original prompt.").to_string(),
    }
}

/// After the prompt box (its `rect`): the veil while it is enhanced, whose Cancel stops the job;
/// and a prompt `typed` in after an enhancement is the user's again, with nothing to go back to.
pub(super) fn after_prompt(app: &mut PhotocraftApp, ui: &mut egui::Ui, rect: egui::Rect, enhancing: Option<&JobInfo>, typed: bool) {
    if let Some(job) = enhancing {
        let provider = app.session.prefs().integrations.ai.enhance.provider;
        if veil(ui, rect, job, provider.short_label()) {
            crate::jobs_ui::cancel(app, job.id);
        }
    }
    if typed {
        app.ui.ai_gen.before_enhance = None;
    }
}

/// The tooltip of the Enhance button that can run.
pub(super) fn tip(app: &PhotocraftApp) -> String {
    let (provider, model) = chosen(app);
    let style = super::styles::chosen(app).map_or_else(String::new, |s| format!(" {} {} {}", tr("It writes for the"), s.name, tr("style chosen.")));
    format!(
        "{} {} ({model}). {}{style}",
        tr("Rewrite the prompt with"),
        provider.short_label(),
        tr("It adds detail about the subject, setting, light and style, and keeps what you wrote. Choose the service in Preferences › Generative AI › Prompt Enhancement")
    )
}

/// What the Enhance button was pressed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pressed {
    Enhance,
    /// Shift-click after an enhancement: the prompt as it was.
    Restore,
    /// Enhance again, from the prompt as typed, for the style now chosen (the status bar's).
    Again,
}

/// The words beside the sparkles (and, Shift held after an enhancement, the way back).
const WORDS: &str = "Enhance prompt";
const RESTORE_WORDS: &str = "Restore prompt";

/// The Enhance button's width: the sparkles, and with `words` the words beside them (as wide
/// either way, so the row doesn't move when Shift is pressed).
pub(super) fn button_width(ui: &egui::Ui, words: bool) -> f32 {
    if !words {
        return 26.0;
    }
    let text = [WORDS, RESTORE_WORDS]
        .iter()
        .map(|w| ui.painter().layout_no_wrap(tr(w).to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE).size().x)
        .fold(0.0, f32::max);
    8.0 + 16.0 + 6.0 + text + 8.0
}

/// The Enhance button beside the model: the sparkles with "Enhance prompt" where the row has
/// room, the sparkles alone where it hasn't. Shift-click after an enhancement puts the prompt
/// back as it was (the button says so while Shift is held over it). `busy`: a generation runs.
pub(super) fn button(app: &PhotocraftApp, ui: &mut egui::Ui, words: bool, busy: bool) -> Option<Pressed> {
    let t = Tokens::get(ui.ctx());
    let why = blocked(app);
    let can_restore = app.ui.ai_gen.before_enhance.is_some();
    let shift = ui.input(|i| i.modifiers.shift);
    let enabled = !busy && running(app).is_none() && (why.is_none() || can_restore);
    let restoring = shift && can_restore;
    let (icon, label) = if restoring { ("undo-2", tr(RESTORE_WORDS)) } else { ("sparkles", tr(WORDS)) };
    let size = vec2(button_width(ui, words), ui.spacing().interact_size.y.max(24.0));
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let on = ui.is_enabled();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, on, tr("Enhance Prompt")));
            let fill = if on && resp.hovered() { t.hover } else { t.field };
            ui.painter().rect_filled(rect, t.radius_sm, fill);
            ui.painter().rect_stroke(rect, t.radius_sm, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
            let color = if on { t.text } else { t.text_faint };
            let icon_rect = if words { egui::Rect::from_min_size(rect.left_center() + vec2(8.0, -8.0), vec2(16.0, 16.0)) } else { egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0)) };
            // The sparkles in the accent: the one button here that adds, rather than sets.
            crate::icons::paint(ui, icon_rect, icon, 14.0, if on { t.accent } else { color });
            if words {
                ui.painter().text(icon_rect.right_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.0), color);
            }
            resp
        })
        .inner;
    let main = why.clone().unwrap_or_else(|| tip(app));
    let text = format!("{main}\n\n{}", restore_tip(app));
    let resp = resp.on_hover_text(&text).on_disabled_hover_text(&text);
    if !resp.clicked() {
        return None;
    }
    if restoring {
        Some(Pressed::Restore)
    } else if why.is_none() {
        Some(Pressed::Enhance)
    } else {
        None
    }
}

/// Act on the Enhance button: enhance the prompt for `mode` and the image model `model`, or put
/// back the one typed.
pub(super) fn act(app: &mut PhotocraftApp, pressed: Pressed, mode: GenerationMode, model: &str) {
    match pressed {
        Pressed::Restore => {
            restore(app);
        }
        Pressed::Enhance | Pressed::Again => {
            if let Err(e) = start(app, mode, Some(model), pressed == Pressed::Again) {
                crate::notices::error(app, format!("{}: {e}", tr("Enhance Prompt")));
            }
        }
    }
}

/// The room between the veil's words and its Cancel button, when they are one under another.
const CANCEL_GAP: f32 = 10.0;
/// A prompt box at least this tall shows the veil's words and Cancel one under another.
const TALL_VEIL: f32 = 96.0;

/// How wide the shimmer band sweeping the veil is, as a fraction of its width, and how long one
/// sweep takes.
const SHIMMER: f32 = 0.45;
const SWEEP_SECONDS: f64 = 1.6;

/// The veil over the prompt box while it is enhanced: a band of light sweeping across it and a
/// bar running along its foot (there is no telling how long the answer takes), what is awaited
/// and for how long, and Cancel. Returns whether Cancel was pressed.
pub(super) fn veil(ui: &mut egui::Ui, rect: egui::Rect, job: &JobInfo, who: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    if !rect.is_positive() {
        return false;
    }
    let painter = ui.painter_at(rect);
    let time = ui.input(|i| i.time);
    let phase = ((time % SWEEP_SECONDS) / SWEEP_SECONDS) as f32;
    painter.rect_filled(rect, t.radius_sm, t.field);
    // The band: clear at its edges, the accent at its middle, left to right and round again.
    let band = rect.width() * SHIMMER;
    let x = rect.left() - band + phase * (rect.width() + band);
    let glow = t.accent.gamma_multiply(0.22);
    let clear = egui::Color32::TRANSPARENT;
    let mut mesh = egui::Mesh::default();
    for (left, right, from, to) in [(x, x + band / 2.0, clear, glow), (x + band / 2.0, x + band, glow, clear)] {
        let (l, r) = (left.max(rect.left()), right.min(rect.right()));
        if r <= l {
            continue;
        }
        // The colours where the band is cut by the box's edges.
        let at = |px: f32| {
            let f = ((px - left) / (right - left).max(1.0)).clamp(0.0, 1.0);
            from.lerp_to_gamma(to, f)
        };
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(l, rect.top()), at(l));
        mesh.colored_vertex(egui::pos2(r, rect.top()), at(r));
        mesh.colored_vertex(egui::pos2(r, rect.bottom()), at(r));
        mesh.colored_vertex(egui::pos2(l, rect.bottom()), at(l));
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
    // A bar along the foot: a short run of the accent, sweeping faster than the band.
    let foot = egui::Rect::from_min_max(egui::pos2(rect.left() + 1.0, rect.bottom() - 3.0), egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0));
    painter.rect_filled(foot, 1.0, t.field_border);
    let run = foot.width() * 0.3;
    let at = foot.left() - run + ((time * 1.7 % 1.0) as f32) * (foot.width() + run);
    let lit = egui::Rect::from_min_max(egui::pos2(at.max(foot.left()), foot.top()), egui::pos2((at + run).min(foot.right()), foot.bottom()));
    if lit.width() > 0.0 {
        painter.rect_filled(lit, 1.0, t.accent);
    }
    painter.rect_stroke(rect, t.radius_sm, egui::Stroke::new(1.0, t.accent_border), egui::StrokeKind::Inside);
    // Over it: the spinner and what is awaited, then for how long, then Cancel, centred where
    // the box is tall enough; else one row with the waiting and Cancel. In a child that takes no
    // room: the box below has it already.
    let title = format!("{} {who}…", tr("Enhancing the prompt with"));
    let status = if job.message.is_empty() { job.label.clone() } else { job.message.clone() };
    let inner = rect.shrink2(vec2(10.0, 4.0));
    let mut cancel = false;
    let cancel_button = |ui: &mut egui::Ui| widgets::secondary_button(ui, tr("Cancel"), 70.0).on_hover_text(tr("Stop waiting: the prompt stays as it is")).clicked();
    if inner.height() >= TALL_VEIL {
        let block = 18.0 + 16.0 + CANCEL_GAP + 30.0 + 2.0 * 6.0;
        let top = inner.center().y - block / 2.0;
        let column = egui::Rect::from_min_max(egui::pos2(inner.left(), top), egui::pos2(inner.right(), inner.bottom()));
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Center)));
        {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.horizontal(|ui| {
                // Centred as a pair: the row is only as wide as what it holds.
                let words = ui.painter().layout_no_wrap(title.clone(), egui::FontId::proportional(12.5), t.text).size().x;
                ui.add_space(((inner.width() - words - 16.0 - 8.0) / 2.0).max(0.0));
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add(egui::Spinner::new().size(16.0).color(t.accent));
                ui.label(RichText::new(&title).color(t.text).size(12.5));
            });
            ui.label(RichText::new(&status).color(t.text_dim).size(11.0));
            ui.add_space(CANCEL_GAP);
            cancel = cancel_button(ui);
        }
    } else {
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add(egui::Spinner::new().size(14.0).color(t.accent));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel = cancel_button(ui);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&status).color(t.text).size(11.5)).on_hover_text(&title);
                });
            });
        }
    }
    ui.ctx().request_repaint();
    cancel
}

// ---------- Preferences › Generative AI › Prompt Enhancement ----------

/// The page's tab in front (`local|cloud|settings`), the instructions being edited in the
/// Settings tab (until Save), and the services whose models were asked for once when their list
/// first opened (`{provider: true}`).
const TAB: &str = "__aiEnhanceTab";
const EDITING: &str = "__aiEnhanceEditing";
const ASKED: &str = "__aiEnhanceAsked";
/// A local model's download running (`{id, model}`), and what the last one said
/// (`{model, text, error}`).
const INSTALL_JOB: &str = "__aiEnhanceInstallJob";
const INSTALL_STATUS: &str = "__aiEnhanceInstallStatus";
/// The page's action that downloads a local model: `enhanceInstall:<id>`; and the one that
/// stops that download.
const INSTALL: &str = "enhanceInstall:";
const STOP_INSTALL: &str = "enhanceStopInstall";
/// The page's action that keeps the edited instructions at once (Save).
pub(super) const SAVE_INSTRUCTIONS: &str = "enhanceSaveInstructions";
/// The page's tabs: (key, title).
const TABS: [(&str, &str); 3] = [("local", "Local Models"), ("cloud", "Cloud Models"), ("settings", "Settings")];
/// Height of a service's row in Cloud Models, and the width of its name and key columns.
const ROW_HEIGHT: f32 = 28.0;
const NAME_WIDTH: f32 = 180.0;
const KEY_WIDTH: f32 = 34.0;
const REFRESH_WIDTH: f32 = 24.0;
/// The width of a local model's speed ("Fastest").
const SPEED_WIDTH: f32 = 130.0;

/// Photoshop-style radio button: a ring, filled when chosen.
pub(super) fn radio(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let enabled = ui.is_enabled();
        let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
        let ring = if on && enabled { t.accent } else { t.text_faint };
        ui.painter().circle_stroke(r.center(), 6.0, egui::Stroke::new(1.3, if enabled { ring } else { ring.gamma_multiply(0.5) }));
        if on {
            ui.painter().circle_filled(r.center(), 3.2, if enabled { t.accent } else { t.text_faint.gamma_multiply(0.5) });
        }
        let color = match (enabled, on) {
            (false, _) => t.text_faint,
            (true, true) => t.text,
            (true, false) => t.text_dim,
        };
        let l = ui.add(egui::Label::new(RichText::new(label).color(color)).selectable(false).sense(egui::Sense::click()));
        let l = if enabled { l.on_hover_cursor(egui::CursorIcon::PointingHand) } else { l };
        resp.union(l)
    })
    .inner
}

/// A tab of the page: its title, underlined in the accent colour when in front (the dialog's
/// own tabs above are pills, so the two rows read apart).
fn sub_tab(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let font = crate::theme::medium(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 26.0), egui::Sense::click());
    let color = if on {
        t.text
    } else if resp.hovered() {
        t.text_dim.gamma_multiply(1.2)
    } else {
        t.text_dim
    };
    let at = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 4.0);
    ui.painter().galley_with_override_text_color(at, galley, color);
    if on {
        let line = egui::Rect::from_min_max(egui::pos2(rect.left() + 4.0, rect.bottom() - 2.0), egui::pos2(rect.right() - 4.0, rect.bottom()));
        ui.painter().rect_filled(line, 1.0, t.accent);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The page: three tabs. Local Models (none yet), Cloud Models (the services as a radio group,
/// each with its model and its key) and Settings (the length, the timeout and the instructions
/// the model is given). `ai` is the draft `integrations.ai`; `f` the dialog's fields.
pub(super) fn page(ui: &mut egui::Ui, ai: &mut Map<String, Value>, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let mut s: EnhanceSettings = ai.get("enhance").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    wrapped(
        ui,
        tr("Enhance (beside Generate in the AI Gen panel) has a text model rewrite your prompt: it adds the detail an image model needs (the subject, setting and light) and keeps what you wrote."),
        t.text_dim,
        13.0,
        width,
    );
    ui.add_space(8.0);
    // The tab chosen, else the one for what is set up: Local Models once a local model is
    // installed (the folders are listed after the first frame, so this is not kept until a tab
    // is clicked).
    let stored = f.get(TAB).and_then(Value::as_str).filter(|k| TABS.iter().any(|(key, _)| key == k)).map(str::to_string);
    let mut tab = stored.clone().unwrap_or_else(|| default_tab(&s, f).to_string());
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (key, title) in TABS {
            if sub_tab(ui, tr(title), tab == key).clicked() {
                tab = key.to_string();
                clicked = true;
            }
        }
    });
    widgets::hairline(ui);
    ui.add_space(12.0);
    match tab.as_str() {
        "local" => local_tab(ui, &mut s, f, width),
        "settings" => settings_tab(ui, &mut s, f, width),
        _ => cloud_tab(ui, &mut s, f, width),
    }
    if stored.is_some() || clicked {
        f.insert(TAB.into(), json!(tab));
    } else {
        f.remove(TAB);
    }
    ai.insert("enhance".into(), serde_json::to_value(&s).unwrap_or_else(|_| json!({})));
}

/// Is a local model's `file` in one of the models folders the dialog lists?
fn local_installed(f: &Map<String, Value>, file: &str) -> bool {
    super::folders::files_of(f).get(local::FOLDER).is_some_and(|names| names.iter().any(|n| n == file))
}

/// The tab the page opens on: Local Models when local enhancement is chosen or a local model is
/// installed, else Cloud Models.
fn default_tab(s: &EnhanceSettings, f: &Map<String, Value>) -> &'static str {
    if s.provider.is_local() || local::MODELS.iter().any(|m| local_installed(f, m.file)) { "local" } else { "cloud" }
}

/// A label that wraps at `width`.
pub(super) fn wrapped(ui: &mut egui::Ui, text: &str, color: egui::Color32, size: f32, width: f32) {
    ui.scope(|ui| {
        ui.set_max_width(width);
        ui.add(egui::Label::new(RichText::new(text).color(color).size(size)).wrap());
    });
}

/// Local Models: the text models the local engine can enhance with, one row each. Its radio
/// button chooses it (and local enhancement: one choice across this tab and Cloud Models); then
/// what it is for, its size, and Installed, or Install (the Local AI setup's downloader).
fn local_tab(ui: &mut egui::Ui, s: &mut EnhanceSettings, f: &mut Map<String, Value>, width: f32) {
    let t = Tokens::get(ui.ctx());
    wrapped(
        ui,
        tr("A text model on this computer writes the prompt: free, private and without a key. The local engine runs it, starting when needed, and shares memory with image generation: only the model chosen is loaded."),
        t.text_faint,
        11.5,
        width,
    );
    ui.add_space(8.0);
    let set_up = f.get(super::SETUP).and_then(|st| st.get("route")).and_then(Value::as_str).is_some_and(|r| !r.is_empty());
    let have = |file: &str| local_installed(f, file);
    let job = f.get(INSTALL_JOB).cloned();
    let downloading = job.as_ref().and_then(|j| j.get("model")).and_then(Value::as_str).map(str::to_string);
    let chosen = s.provider.is_local().then(|| s.model(TextProvider::Local));
    let mut action = None;
    for m in &local::MODELS {
        let on = chosen.as_deref() == Some(m.id);
        let installed = have(m.file);
        let stroke = if on { egui::Stroke::new(1.5, t.accent) } else { egui::Stroke::new(1.0, t.card_border) };
        egui::Frame::new().fill(t.card).stroke(stroke).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(width - 22.0);
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.allocate_ui_with_layout(vec2(NAME_WIDTH, ROW_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(NAME_WIDTH);
                    if radio(ui, on, m.name).clicked() {
                        s.provider = TextProvider::Local;
                        s.models.insert(TextProvider::Local.key().into(), m.id.to_string());
                    }
                });
                ui.allocate_ui_with_layout(vec2(SPEED_WIDTH, ROW_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(SPEED_WIDTH);
                    ui.label(RichText::new(tr(m.speed)).font(crate::theme::medium(11.5)).color(t.text));
                });
                let state_width = 150.0;
                let role_width = (ui.available_width() - state_width - 70.0).max(80.0);
                ui.allocate_ui_with_layout(vec2(role_width, ROW_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(role_width);
                    ui.add(egui::Label::new(RichText::new(tr(m.role)).color(t.text_dim).size(11.5)).truncate()).on_hover_text(tr(m.role));
                });
                ui.allocate_ui_with_layout(vec2(60.0, ROW_HEIGHT), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.set_min_width(60.0);
                    ui.label(RichText::new(photocraft_aisetup::human_bytes(m.size)).color(t.text_faint).size(11.5));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if installed {
                        ui.label(RichText::new(tr("Installed")).color(t.success).size(12.0));
                        let (tick, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                        crate::icons::paint(ui, tick, "check", 14.0, t.success);
                    } else if downloading.as_deref() == Some(m.id) {
                        if ui.add(egui::Button::new(tr("Stop")).small()).on_hover_text(tr("Stop the download: what arrived is kept, and Install goes on from there")).clicked() {
                            action = Some(STOP_INSTALL.to_string());
                        }
                        ui.add_sized(vec2(18.0, 18.0), egui::Spinner::new().size(14.0));
                    } else {
                        let free = set_up && downloading.is_none();
                        let tip = if set_up {
                            format!("{} {} ({})", tr("Download"), m.name, photocraft_aisetup::human_bytes(m.size))
                        } else {
                            tr("Set up Local AI first: it installs the engine that runs the model").to_string()
                        };
                        let pressed = ui.add_enabled_ui(free, |ui| widgets::secondary_button(ui, tr("Install"), 90.0)).inner.on_hover_text(tip).clicked();
                        if pressed {
                            action = Some(format!("{INSTALL}{}", m.id));
                        }
                    }
                });
            });
            // The download's progress, or what the last one said.
            let line = match (&job, f.get(INSTALL_STATUS)) {
                (Some(j), _) if downloading.as_deref() == Some(m.id) => {
                    Some((j.get("text").and_then(Value::as_str).unwrap_or("").to_string(), false, j.get("progress").and_then(Value::as_f64)))
                }
                (_, Some(st)) if st.get("model").and_then(Value::as_str) == Some(m.id) => {
                    Some((st.get("text").and_then(Value::as_str).unwrap_or("").to_string(), st.get("error").and_then(Value::as_bool).unwrap_or(false), None))
                }
                _ => None,
            };
            if let Some((text, error, progress)) = line.filter(|(text, _, _)| !text.is_empty()) {
                ui.horizontal(|ui| {
                    ui.add_space(NAME_WIDTH + 8.0);
                    ui.vertical(|ui| {
                        ui.set_max_width((width - NAME_WIDTH - 40.0).max(120.0));
                        if let Some(p) = progress {
                            ui.add(egui::ProgressBar::new(p.clamp(0.0, 1.0) as f32).desired_height(4.0).desired_width(ui.available_width()));
                        }
                        ui.add(egui::Label::new(RichText::new(text).color(if error { t.danger } else { t.text_faint }).size(11.0)).wrap().selectable(true));
                    });
                });
            }
        });
        ui.add_space(6.0);
    }
    if !set_up {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Local AI isn't set up yet: its setup installs the engine that runs these models.")).color(t.warning).size(11.5));
            if widgets::secondary_button(ui, tr("Set Up Local AI…"), 130.0).clicked() {
                action = Some("setup".to_string());
            }
        });
    } else if chosen.as_deref().and_then(local::find).is_some_and(|m| !have(m.file)) {
        wrapped(ui, tr("The model chosen isn't installed: Install it, or choose one that is."), t.warning, 11.5, width);
    }
    if let Some(a) = action {
        f.insert(ACTION.into(), json!(a));
    }
}

/// Cloud Models: one row per service. Its radio button chooses the one that enhances; then its
/// model (from what it lists, or the default), a button that lists its models again, and its
/// key (a button to the API Keys page, which says whether there is one).
fn cloud_tab(ui: &mut egui::Ui, s: &mut EnhanceSettings, f: &mut Map<String, Value>, width: f32) {
    let t = Tokens::get(ui.ctx());
    wrapped(ui, tr("Choose the service that enhances prompts, and its model. Each request is billed to your account there."), t.text_faint, 11.5, width);
    ui.add_space(8.0);
    let keys = f.get(KEYS).and_then(Value::as_object).cloned().unwrap_or_default();
    let listing = f.get(LIST_JOB).and_then(|j| j.get("provider")).and_then(Value::as_str).map(str::to_string);
    let mut list_now = None;
    let mut go_key = None;
    for p in TextProvider::ALL {
        let on = s.provider == p;
        let has_key = has_key(&keys, p);
        let stroke = if on { egui::Stroke::new(1.5, t.accent) } else { egui::Stroke::new(1.0, t.card_border) };
        egui::Frame::new().fill(t.card).stroke(stroke).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(width - 22.0);
            // Fixed columns, so the services' models line up: name, model, refresh, key.
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT);
                ui.spacing_mut().item_spacing.x = 6.0;
                let column = |ui: &mut egui::Ui, w: f32, layout: egui::Layout, add: &mut dyn FnMut(&mut egui::Ui)| {
                    ui.allocate_ui_with_layout(vec2(w, ROW_HEIGHT), layout, |ui| {
                        ui.set_min_width(w);
                        add(ui);
                    });
                };
                // Without a key the row is off: only its key button works.
                let no_key = format!("{} {}", p.label(), tr("has no key: add one with the key button"));
                column(ui, NAME_WIDTH, egui::Layout::left_to_right(egui::Align::Center), &mut |ui| {
                    let picked = ui.add_enabled_ui(has_key, |ui| radio(ui, on, p.label())).inner.on_disabled_hover_text(&no_key).clicked();
                    if picked && has_key {
                        s.provider = p;
                    }
                });
                let busy = listing.as_deref() == Some(p.key());
                let field = (ui.available_width() - REFRESH_WIDTH - KEY_WIDTH - 12.0).max(120.0);
                if ui.add_enabled_ui(has_key, |ui| model_picker(ui, s, f, p, field, has_key, busy)).inner && has_key {
                    list_now = Some(p);
                }
                // Ask it for its models again.
                column(ui, REFRESH_WIDTH, egui::Layout::left_to_right(egui::Align::Center), &mut |ui| {
                    if busy {
                        ui.add_sized(vec2(REFRESH_WIDTH, 24.0), egui::Spinner::new().size(14.0));
                    } else {
                        let tip = if has_key { tr("List the service's models again") } else { tr("Add a key first: the service lists its models to those with one") };
                        let free = listing.is_none() && has_key;
                        if ui.add_enabled_ui(free, |ui| crate::icons::button(ui, "rotate-cw", REFRESH_WIDTH, false, tip)).inner.clicked() {
                            list_now = Some(p);
                        }
                    }
                });
                // Its key, at the right.
                column(ui, KEY_WIDTH, egui::Layout::right_to_left(egui::Align::Center), &mut |ui| {
                    if key_button(ui, p, has_key, key_typed(&keys, p)).clicked() {
                        go_key = Some(p);
                    }
                });
            });
            // What its last listing said.
            if let Some(status) = f.get(LIST_STATUS).filter(|st| st.get("provider").and_then(Value::as_str) == Some(p.key())) {
                let error = status.get("error").and_then(Value::as_bool).unwrap_or(false);
                let text = status.get("text").and_then(Value::as_str).unwrap_or("");
                ui.horizontal(|ui| {
                    ui.add_space(NAME_WIDTH + 8.0);
                    ui.scope(|ui| {
                        ui.set_max_width((width - NAME_WIDTH - 40.0).max(120.0));
                        ui.add(egui::Label::new(RichText::new(text).color(if error { t.danger } else { t.text_faint }).size(11.0)).wrap().selectable(true));
                    });
                });
            }
        });
        ui.add_space(6.0);
    }
    ui.add_space(2.0);
    wrapped(ui, tr("OpenAI, xAI and Gemini use the same keys as their image models. Keys are kept on the API Keys page."), t.text_faint, 11.0, width);
    if let Some(p) = list_now {
        f.insert(ACTION.into(), json!(format!("{LIST}{}", p.key())));
    }
    if let Some(p) = go_key {
        // The API Keys page, with this service's key in front (OpenAI, xAI and Gemini share their
        // image models' card there).
        f.insert(GOTO.into(), json!("keys"));
        f.insert(KEY_FOCUS.into(), json!(p.image_provider().map_or(p.key(), |i| i.key())));
        f.insert(KEY_FOCUS_ONCE.into(), json!(true));
    }
}

/// The name a service's key has in the dialog's `KEYS`: its image provider's for OpenAI, xAI and
/// Gemini (the same as its own).
fn key_name(p: TextProvider) -> &'static str {
    p.image_provider().map_or(p.key(), |i| i.key())
}

/// A key typed on the API Keys page and not kept yet (it is when the dialog's OK is pressed).
fn key_typed(keys: &Map<String, Value>, p: TextProvider) -> bool {
    keys.get(key_name(p)).and_then(Value::as_str).is_some_and(|k| !k.trim().is_empty())
}

/// Whether a service has a key, or will when the dialog's OK is pressed.
fn has_key(keys: &Map<String, Value>, p: TextProvider) -> bool {
    let name = key_name(p);
    let saved = keys.get(&format!("{name}Saved")).and_then(Value::as_bool).unwrap_or(false);
    let remove = keys.get(&format!("{name}Remove")).and_then(Value::as_bool).unwrap_or(false);
    key_typed(keys, p) || (saved && !remove)
}

/// A service's key: a key icon with a green check when it has one, a greyed key with a plus
/// when not. Either opens the API Keys page at its key.
fn key_button(ui: &mut egui::Ui, p: TextProvider, has_key: bool, typed: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 28.0), egui::Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let icon = egui::Rect::from_center_size(rect.center() - vec2(3.0, 2.0), vec2(20.0, 20.0));
    let key = if has_key {
        t.text
    } else if hovered {
        t.text_dim
    } else {
        t.text_faint.gamma_multiply(0.8)
    };
    crate::icons::paint(ui, icon, "key-round", 18.0, key);
    // The badge, at the key's lower right on a disc of the row's colour so it reads apart.
    let at = egui::pos2(rect.right() - 9.0, rect.bottom() - 8.0);
    ui.painter().circle_filled(at, 7.5, if hovered { t.hover } else { t.card });
    // Drawn rather than an icon, so their strokes can be bolder than the icon set's.
    let stroke = |color| egui::Stroke::new(2.2, color);
    if has_key {
        let tick = [at + vec2(-4.0, 0.2), at + vec2(-1.2, 3.0), at + vec2(4.2, -3.2)];
        ui.painter().line(tick.to_vec(), stroke(t.success));
    } else {
        let color = if hovered { t.text_dim } else { t.text_faint };
        ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], stroke(color));
        ui.painter().line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], stroke(color));
    }
    let (name, tip) = if !has_key {
        (tr("Add key"), format!("{} {}", p.label(), tr("has no key: add one on the API Keys page")))
    } else if typed {
        (tr("Key saved"), format!("{} {}", p.label(), tr("key: saved when you press OK. Click to see it on the API Keys page")))
    } else {
        (tr("Key saved"), format!("{} {}", p.label(), tr("key saved. Click to change or remove it on the API Keys page")))
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A service's model: a dropdown of what it listed (filtered as you type, which can also name a
/// model it didn't list), with its default first. Opening it the first time asks the service for
/// its models. Returns whether to ask now.
fn model_picker(ui: &mut egui::Ui, s: &mut EnhanceSettings, f: &mut Map<String, Value>, p: TextProvider, width: f32, has_key: bool, busy: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let listed: Vec<String> = f.get(LISTED).and_then(|l| l.get(p.key())).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let current = s.models.get(p.key()).map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    let shown = match &current {
        Some(m) => m.clone(),
        None => format!("{} {}", p.default_model(), tr("(default)")),
    };
    let asked = f.get(ASKED).and_then(|a| a.get(p.key())).and_then(Value::as_bool).unwrap_or(false);
    let mut filter = f.get(LIST_FILTER).and_then(|v| v.get(p.key())).and_then(Value::as_str).unwrap_or("").to_string();
    let mut chosen: Option<Option<String>> = None;
    let mut ask = false;
    let id = format!("ai-enhance-model-{}", p.key());
    let combo = craft_ui::ComboBox::from_id_salt(&id)
        .selected_text(shown)
        .width(width)
        .height(360.0)
        .icon(widgets::chevron_icon)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.set_min_width(width.max(260.0));
            // Opened for the first time: ask the service what it has.
            if listed.is_empty() && !asked && has_key && !busy {
                ask = true;
            }
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text(tr("Filter, or type a model id")).desired_width(f32::INFINITY)).request_focus();
            ui.add_space(2.0);
            let wanted = filter.trim().to_ascii_lowercase();
            if ui.selectable_label(current.is_none(), format!("{} {}", p.default_model(), tr("(default)"))).clicked() {
                chosen = Some(None);
            }
            widgets::menu_scroll(ui, |ui| {
                for m in listed.iter().filter(|m| m.as_str() != p.default_model() && (wanted.is_empty() || m.to_ascii_lowercase().contains(&wanted))) {
                    if ui.selectable_label(current.as_deref() == Some(m.as_str()), m.as_str()).clicked() {
                        chosen = Some(Some(m.clone()));
                    }
                }
                // A model chosen before that this listing doesn't have (or nothing listed yet).
                if let Some(m) = current.as_ref().filter(|m| !listed.contains(m)) {
                    let _ = ui.selectable_label(true, m.as_str());
                }
            });
            let typed = filter.trim();
            if !typed.is_empty() && !typed.contains(char::is_whitespace) && typed.len() <= 200 && !listed.iter().any(|m| m == typed) {
                ui.separator();
                if ui.button(format!("{} \u{201c}{typed}\u{201d}", tr("Use"))).clicked() {
                    chosen = Some(Some(typed.to_string()));
                }
            }
            if listed.is_empty() {
                let note = if busy {
                    tr("Asking the service for its models…")
                } else if has_key {
                    tr("The service's models aren't listed yet")
                } else {
                    tr("Add a key to list the service's models")
                };
                ui.label(RichText::new(note).color(t.text_faint).size(11.0));
            }
            if chosen.is_some() {
                ui.close();
            }
        });
    combo.response.accessible_name(format!("{} {}", p.label(), tr("model"))).on_hover_text(tr("The model that rewrites prompts. The default is a fast one that writes well"));
    if let Some(model) = chosen {
        match model {
            Some(m) if m != p.default_model() => {
                s.models.insert(p.key().to_string(), m);
            }
            _ => {
                s.models.remove(p.key());
            }
        }
        filter.clear();
    }
    let mut filters = f.get(LIST_FILTER).and_then(Value::as_object).cloned().unwrap_or_default();
    filters.insert(p.key().into(), json!(filter));
    f.insert(LIST_FILTER.into(), Value::Object(filters));
    if ask {
        let mut all = f.get(ASKED).and_then(Value::as_object).cloned().unwrap_or_default();
        all.insert(p.key().into(), json!(true));
        f.insert(ASKED.into(), Value::Object(all));
    }
    ask
}

/// Settings: how long the enhanced prompt may be, how long to wait, and the instructions the
/// model is given (the built-in ones to start from, edited in place and kept with Save).
fn settings_tab(ui: &mut egui::Ui, s: &mut EnhanceSettings, f: &mut Map<String, Value>, width: f32) {
    let t = Tokens::get(ui.ctx());
    let label = |ui: &mut egui::Ui, text: &str| {
        ui.allocate_ui_with_layout(vec2(110.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(110.0);
            ui.label(RichText::new(text).color(t.text_dim));
        });
    };
    ui.horizontal(|ui| {
        label(ui, tr("Prompt length"));
        let mut words = s.words();
        ui.add(egui::DragValue::new(&mut words).range(MIN_WORDS..=MAX_WORDS).speed(1.0).suffix(tr(" words")).custom_parser(widgets::parse_num))
            .on_hover_text(tr("The enhanced prompt is at most about this long. Short prompts suit fast models; long ones give detail-hungry models more to follow"));
        s.words = words;
        ui.label(RichText::new(tr("at most, roughly")).color(t.text_faint).size(11.0));
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        label(ui, tr("Timeout"));
        let mut seconds = s.timeout_seconds.clamp(MIN_TIMEOUT, MAX_TIMEOUT);
        ui.add(egui::DragValue::new(&mut seconds).range(MIN_TIMEOUT..=MAX_TIMEOUT).speed(1.0).suffix(" s").custom_parser(widgets::parse_num))
            .on_hover_text(tr("How long to wait for the answer before giving up. A slow connection, or a model that thinks before it answers, can need longer"));
        s.timeout_seconds = seconds;
        ui.label(RichText::new(tr("before giving up")).color(t.text_faint).size(11.0));
    });
    ui.add_space(10.0);
    widgets::hairline(ui);
    ui.add_space(8.0);
    ui.label(RichText::new(tr("Instructions")).font(crate::theme::medium(13.0)).color(t.text));
    wrapped(
        ui,
        tr("What the text model is told to do with your prompt. Edit it here and press Save. The panel's task (Create, Change, Fill, Restyle), the image model, the length and the style preset are added to it."),
        t.text_faint,
        11.5,
        width,
    );
    ui.add_space(6.0);
    let saved = s.instructions_text().to_string();
    let mut text = f.get(EDITING).and_then(Value::as_str).map_or_else(|| saved.clone(), str::to_string);
    ui.add(egui::TextEdit::multiline(&mut text).desired_width(width).desired_rows(12).hint_text(tr("Empty: the built-in instructions")));
    let edited = text.trim() != saved.trim();
    let built_in = text.trim() == DEFAULT_INSTRUCTIONS.trim() || text.trim().is_empty();
    ui.add_space(6.0);
    let mut save = false;
    ui.horizontal(|ui| {
        ui.set_width(width);
        let (note, color) = if edited {
            (tr("Not saved"), t.warning)
        } else if s.instructions.trim().is_empty() {
            (tr("The built-in instructions"), t.text_faint)
        } else {
            (tr("Your own instructions"), t.text_faint)
        };
        ui.label(RichText::new(note).color(color).size(11.0));
        // Save at the right, Restore Default before it.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            save = ui.add_enabled_ui(edited, |ui| widgets::primary_button(ui, tr("Save"), 90.0)).inner.on_hover_text(tr("Use these instructions from now on")).clicked();
            if ui.add_enabled_ui(!built_in, |ui| widgets::secondary_button(ui, tr("Restore Default"), 130.0)).inner.on_hover_text(tr("Put the built-in instructions back in the box (Save keeps them)")).clicked() {
                text = DEFAULT_INSTRUCTIONS.to_string();
            }
        });
    });
    if save {
        s.set_instructions(&text);
        f.insert(ACTION.into(), json!(SAVE_INSTRUCTIONS));
        f.remove(EDITING);
    } else if edited {
        f.insert(EDITING.into(), json!(text));
    } else {
        f.remove(EDITING);
    }
}

/// The page's own actions, from [`super::after_body`]: list a service's models, keep the edited
/// instructions. Returns whether
/// `action` was one.
pub(super) fn page_action(app: &mut PhotocraftApp, f: &mut Map<String, Value>, action: &str) -> bool {
    if let Some(id) = action.strip_prefix(INSTALL) {
        install(app, f, id);
        return true;
    }
    if action == STOP_INSTALL {
        if let Some(id) = f.get(INSTALL_JOB).and_then(|j| j.get("id")).and_then(Value::as_u64) {
            app.session.cancel_job(photocraft_engine::jobs::JobId(id));
        }
        return true;
    }
    if action == SAVE_INSTRUCTIONS {
        // Save keeps the instructions at once, as the dialog's OK would.
        let text = f.get("values").and_then(|v| v.pointer("/integrations/ai/enhance/instructions")).and_then(Value::as_str).unwrap_or("").to_string();
        app.session.prefs.edit(|p| p.integrations.ai.enhance.set_instructions(&text));
        return true;
    }
    let Some(key) = action.strip_prefix(LIST) else { return false };
    let Some(provider) = TextProvider::from_key(key) else { return true };
    let params = json!({"provider": provider.key()});
    let started = if app.background_jobs {
        app.session.start("ai.enhanceModels", params).map_err(|e| e.to_string())
    } else {
        app.session.execute("ai.enhanceModels", params).map(photocraft_engine::jobs::Started::Done).map_err(|e| e.to_string())
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Done(result)) => listed(f, provider, &result),
        Ok(photocraft_engine::jobs::Started::Job(id)) => {
            f.insert(LIST_JOB.into(), json!({"id": id.0, "provider": provider.key()}));
            f.insert(LIST_STATUS.into(), json!({"provider": provider.key(), "text": tr("Asking for its models…"), "error": false}));
        }
        Err(e) => list_failed(f, provider, &e),
    }
    true
}

/// Follow the model listing that runs, from [`super::after_body`].
pub(super) fn follow(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    follow_install(app, f);
    let Some(job) = f.get(LIST_JOB).cloned() else { return };
    let id = job.get("id").and_then(Value::as_u64).unwrap_or(0);
    let provider = job.get("provider").and_then(Value::as_str).and_then(TextProvider::from_key).unwrap_or_default();
    match app.session.jobs_with_recent().into_iter().rev().find(|j| j.id.0 == id) {
        Some(j) if j.state == "running" => {}
        Some(j) => {
            f.remove(LIST_JOB);
            match (j.state, j.result) {
                ("done", Some(result)) => listed(f, provider, &result),
                _ => list_failed(f, provider, &j.error.unwrap_or_else(|| tr("Cancelled").to_string())),
            }
        }
        None => {
            f.remove(LIST_JOB);
        }
    }
}

/// Download local model `id` (`ai.enhanceInstall`), in the background where there are jobs.
fn install(app: &mut PhotocraftApp, f: &mut Map<String, Value>, id: &str) {
    let Some(model) = local::find(id) else { return };
    let params = json!({"model": model.id});
    let started = if app.background_jobs {
        app.session.start("ai.enhanceInstall", params).map_err(|e| e.to_string())
    } else {
        app.session.execute("ai.enhanceInstall", params).map(photocraft_engine::jobs::Started::Done).map_err(|e| e.to_string())
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Job(job)) => {
            f.insert(INSTALL_JOB.into(), json!({"id": job.0, "model": model.id, "text": format!("{} {}…", tr("Downloading"), model.name), "progress": 0.0}));
            f.remove(INSTALL_STATUS);
        }
        Ok(photocraft_engine::jobs::Started::Done(_)) => installed(app, f, model),
        Err(e) => {
            f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": e, "error": true}));
        }
    }
}

/// Follow the download that runs: its progress while it runs, and once it ends the folders are
/// read again (it shows as installed) or why it stopped is said.
fn follow_install(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    let Some(job) = f.get(INSTALL_JOB).cloned() else { return };
    let id = job.get("id").and_then(Value::as_u64).unwrap_or(0);
    let Some(model) = job.get("model").and_then(Value::as_str).and_then(local::find) else {
        f.remove(INSTALL_JOB);
        return;
    };
    match app.session.jobs_with_recent().into_iter().rev().find(|j| j.id.0 == id) {
        Some(j) if j.state == "running" => {
            f.insert(INSTALL_JOB.into(), json!({"id": id, "model": model.id, "text": j.message, "progress": j.progress}));
        }
        Some(j) => {
            f.remove(INSTALL_JOB);
            match j.state {
                "done" => installed(app, f, model),
                "cancelled" => {
                    f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": tr("Download stopped: Install goes on from where it stopped"), "error": false}));
                }
                _ => {
                    f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": j.error.unwrap_or_default(), "error": true}));
                }
            }
        }
        None => {
            f.remove(INSTALL_JOB);
        }
    }
}

/// `model` was installed: the folders are read again, so the page shows it.
fn installed(app: &mut PhotocraftApp, f: &mut Map<String, Value>, model: &local::LocalModel) {
    super::folders::list(app, f);
    f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": format!("{} {}", model.name, tr("is installed.")), "error": false}));
}

fn listed(f: &mut Map<String, Value>, provider: TextProvider, result: &Value) {
    let models = result.get("models").cloned().unwrap_or_else(|| json!([]));
    let n = models.as_array().map_or(0, Vec::len);
    let mut all = f.get(LISTED).and_then(Value::as_object).cloned().unwrap_or_default();
    all.insert(provider.key().into(), models);
    f.insert(LISTED.into(), Value::Object(all));
    let text = if n == 0 { tr("It listed no model that writes text.").to_string() } else { format!("{n} {}", tr("models listed.")) };
    f.insert(LIST_STATUS.into(), json!({"provider": provider.key(), "text": text, "error": n == 0}));
}

fn list_failed(f: &mut Map<String, Value>, provider: TextProvider, why: &str) {
    f.insert(LIST_STATUS.into(), json!({"provider": provider.key(), "text": why, "error": true}));
}
