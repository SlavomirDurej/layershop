//! Generative AI in the shell: the Window › AI Gen panel (a tab of the Properties dock group)
//! and the model settings in Edit › Preferences › Integrations.
//!
//! The panel picks a model, takes a prompt and runs the engine's `ai.generate` as a background
//! job: the result lands on a new layer over the selection. The settings page edits
//! `integrations.ai` (the ComfyUI server, local and cloud models and their rendering parameters)
//! in the Preferences dialog's draft, like every other section; API keys are saved apart from
//! the preferences through [`crate::Services::save_ai_keys`].
//!
//! The strings here go through [`tr`] rather than `tl!`: they fall back to English until the
//! catalogs gain them (the `tl!` scan requires every complete language to have each literal).

use egui::{RichText, vec2};
use photocraft_aigen::{Family, Model, Provider, Settings};
use photocraft_engine::jobs::JobInfo;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// Window › AI Gen.
pub const PANEL: &str = "window.panel.aiGen";
/// The tab's position in the Properties dock group (Properties | Adjustments | AI Gen).
pub const TAB: usize = 2;
/// The engine command the panel runs.
const GENERATE: &str = "ai.generate";

/// The panel's state (in `UiState`, so `ui.get` / `ui.set` can read and drive it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiGenUi {
    /// The chosen model's id (empty: the first enabled one).
    pub model: String,
    pub prompt: String,
    /// Ask for a transparent background (models that support it).
    pub alpha: bool,
    /// What the last generation cost in US dollars, when the provider reported it (OpenAI).
    pub last_cost: Option<f64>,
    /// The floating panel is open. Only the themes that don't dock Properties (Studio, Classic)
    /// use it; in the Pro themes the panel is a tab of that dock group.
    pub floating: bool,
}

fn tr(s: &str) -> &str {
    crate::i18n::t(s)
}

/// Menu ids handled here (live menu items).
pub fn handles(id: &str) -> bool {
    id == PANEL
}

/// The theme docks the Properties group (Pro); the others float the panel over the canvas.
fn docked(app: &PhotocraftApp) -> bool {
    Tokens::for_kind(app.ui.theme).pro
}

fn shown(app: &PhotocraftApp) -> bool {
    if !docked(app) {
        return app.ui.ai_gen.floating;
    }
    app.ui.panels.dock && app.ui.panels.properties && app.ui.dock_tabs.properties == TAB && !app.ui.dock.is_collapsed(crate::dock::Group::Properties)
}

pub fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    (id == PANEL).then(|| shown(app))
}

/// Window › AI Gen: show the panel (its dock group, expanded, on its tab), or hide the group when
/// it is already showing. `{"show": bool}` sets it instead of toggling.
pub fn menu(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != PANEL {
        return None;
    }
    let show = params.get("show").and_then(Value::as_bool).unwrap_or_else(|| !shown(app));
    if !docked(app) {
        app.ui.ai_gen.floating = show;
        return Some(Ok(json!({"open": show})));
    }
    if show {
        app.ui.panels.dock = true;
        app.ui.panels.properties = true;
        app.ui.dock_tabs.properties = TAB;
        app.ui.dock.set_collapsed(crate::dock::Group::Properties, false);
    } else if app.ui.dock_tabs.properties == TAB {
        app.ui.panels.properties = false;
    }
    Some(Ok(json!({"open": show})))
}

/// The floating AI Gen panel of the themes that don't dock Properties.
pub fn window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.ui.ai_gen.floating || docked(app) {
        return;
    }
    let t = Tokens::get(ctx);
    let canvas = app.last_canvas_rect;
    let width = 300.0;
    let mut close = false;
    egui::Window::new(tr("AI Gen"))
        .id(egui::Id::new("ai-gen"))
        .title_bar(false)
        .resizable(false)
        .movable(!app.session.prefs().workspace_locked)
        .min_width(width)
        .max_width(width)
        .frame(crate::analysis_ui::float_frame(&t))
        .default_pos(egui::pos2((canvas.right() - width - 16.0).max(canvas.left() + 8.0), canvas.top() + 60.0))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_width(width);
            close = crate::analysis_ui::title_row(ui, "AI Gen");
            panel(app, ui, false);
        });
    if close {
        app.ui.ai_gen.floating = false;
    }
}

/// The models offered in the panel: enabled ones, in settings order.
fn enabled_models(app: &PhotocraftApp) -> Vec<Model> {
    app.session.prefs().integrations.ai.models.iter().filter(|m| m.enabled).cloned().collect()
}

/// The running generation, if any.
fn running(app: &PhotocraftApp) -> Option<JobInfo> {
    app.session.jobs().into_iter().find(|j| j.command == GENERATE)
}

/// Open Preferences on Integrations with `model` selected for editing.
pub fn open_settings(app: &mut PhotocraftApp, model: &str) -> u64 {
    let models = &app.session.prefs().integrations.ai.models;
    let index = models.iter().position(|m| m.id == model).unwrap_or(0);
    let cloud = models.get(index).is_some_and(|m| !m.provider.is_local());
    let id = crate::prefs_ui::open_preferences(app, "integrations");
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert(SELECTED.into(), json!(index));
        d.fields.insert(PAGE.into(), json!(if cloud { "cloud" } else { "local" }));
    }
    id
}

/// Start a generation with the panel's model, prompt and options.
pub fn generate(app: &mut PhotocraftApp) -> Result<Value, String> {
    let ui = &app.ui.ai_gen;
    let params = json!({"model": ui.model, "prompt": ui.prompt, "alpha": ui.alpha});
    let result = crate::jobs_ui::run(app, GENERATE, params)?;
    // Inline (no background jobs): the result is here already. A job reports through `finished`.
    if result.get("pending").is_none() {
        finished(app, &result);
    }
    Ok(result)
}

/// A generation ended with `result` (`ai.generate`'s): remember what it cost.
pub fn finished(app: &mut PhotocraftApp, result: &Value) {
    app.ui.ai_gen.last_cost = result.get("cost").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0);
}

/// The area a generation would fill, in pixels: the selection's bounds, else the canvas.
fn target_size(app: &PhotocraftApp) -> Option<(u32, u32, bool)> {
    let d = app.session.active()?;
    let canvas = d.doc.bounds();
    Some(match d.doc.selection.as_ref() {
        Some(sel) => {
            let b = sel.content_bounds().intersect(&canvas);
            (b.width(), b.height(), true)
        }
        None => (canvas.width(), canvas.height(), false),
    })
}

/// Height of the prompt box when the panel has no height of its own to fill.
const PROMPT_HEIGHT: f32 = 84.0;
/// What the rows under the prompt need: spacing, the Generate row and two lines of notes.
const BELOW_PROMPT: f32 = 6.0 + 32.0 + 3.0 + 34.0;

/// The AI Gen panel body. With `fill` the prompt box takes the height the panel has left (the
/// dock group: drag its edge and the prompt grows); without it the box has a fixed height.
pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui, fill: bool) {
    let t = Tokens::get(ui.ctx());
    let models = enabled_models(app);
    if models.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(tr("No AI models are set up yet.")).color(t.text_dim));
        ui.add_space(2.0);
        ui.label(RichText::new(tr("Add local models run by ComfyUI, or cloud models with an API key.")).color(t.text_faint).size(11.5));
        ui.add_space(8.0);
        if widgets::secondary_button(ui, tr("Set Up Models…"), 130.0).clicked() {
            open_settings(app, "");
        }
        return;
    }
    // A model removed or disabled since it was chosen falls back to the first one.
    if !models.iter().any(|m| m.id == app.ui.ai_gen.model) {
        app.ui.ai_gen.model = models.first().map(|m| m.id.clone()).unwrap_or_default();
    }
    let current = models.iter().find(|m| m.id == app.ui.ai_gen.model).cloned().unwrap_or_default();
    let job = running(app);
    let busy = job.is_some();
    let width = ui.available_width();

    // Model, and the shortcut to its settings.
    let mut settings = false;
    ui.horizontal(|ui| {
        let names: Vec<String> = models.iter().map(|m| if m.provider.is_local() { m.name.clone() } else { format!("{} · {}", m.name, tr("cloud")) }).collect();
        let options: Vec<(String, &str)> = models.iter().map(|m| m.id.clone()).zip(names.iter().map(String::as_str)).collect();
        ui.add_enabled_ui(!busy, |ui| {
            widgets::dropdown(ui, "ai-gen-model", &mut app.ui.ai_gen.model, &options, (width - 34.0).max(80.0));
        });
        settings = crate::icons::button(ui, "settings", 24.0, false, tr("Model settings…")).clicked();
    });
    if settings {
        let model = app.ui.ai_gen.model.clone();
        open_settings(app, &model);
    }
    ui.add_space(6.0);

    // Prompt. ⌘/Ctrl+Return generates.
    let mut submit = false;
    ui.add_enabled_ui(!busy, |ui| {
        let left = ui.available_height();
        let height = if fill && left.is_finite() { (left - BELOW_PROMPT).clamp(40.0, 1200.0) } else { PROMPT_HEIGHT };
        // A prompt longer than the box scrolls inside it instead of pushing Generate away.
        egui::ScrollArea::vertical().id_salt("ai-gen-prompt").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut app.ui.ai_gen.prompt)
                .hint_text(tr("Describe what to generate…"))
                .desired_width(width)
                .min_size(vec2(width, height));
            let resp = ui.add(edit);
            submit = resp.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        });
    });
    ui.add_space(6.0);

    // One row, so the button stays in view in a short dock group: the alpha option on the left,
    // Generate on the right; a running generation shows its progress and Cancel there instead.
    let why = generate_blocked(app, &current);
    let mut clicked = false;
    let mut cancel = None;
    ui.horizontal(|ui| match &job {
        Some(job) => {
            ui.add(egui::ProgressBar::new(job.progress.clamp(0.0, 1.0)).desired_width((width - 84.0).max(40.0)).desired_height(6.0).fill(t.accent));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::secondary_button(ui, tr("Cancel"), 70.0).clicked() {
                    cancel = Some(job.id);
                }
            });
        }
        None => {
            ui.add_enabled_ui(current.alpha, |ui| {
                // The choice is kept across models; one without alpha shows it off and ignores it.
                let mut on = app.ui.ai_gen.alpha && current.alpha;
                let resp = widgets::checkbox(ui, &mut on, tr("Alpha Channel"));
                if current.alpha {
                    app.ui.ai_gen.alpha = on;
                    resp.on_hover_text(tr("Ask the model for a transparent background"));
                } else {
                    resp.on_disabled_hover_text(tr("This model can't generate a transparent background"));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui.add_enabled_ui(why.is_none(), |ui| widgets::primary_button(ui, tr("Generate"), 96.0).clicked()).inner;
            });
        }
    });
    if let Some(id) = cancel {
        crate::jobs_ui::cancel(app, id);
    }
    ui.add_space(3.0);

    // Under it: the job's status, or why Generate is off, or what will be rendered (so the size
    // is no surprise).
    let note = match (&job, &why) {
        (Some(job), _) => Some(if job.message.is_empty() { job.label.clone() } else { job.message.clone() }),
        (None, Some(why)) => Some(why.clone()),
        (None, None) => target_size(app).map(|(w, h, selection)| {
            let (rw, rh) = render_size(&current, w, h);
            let area = if selection { tr("Selection") } else { tr("Canvas") };
            if current.provider == Provider::Gemini {
                format!("{area} {w} × {h} px → {rw}:{rh}, {}", current.image_size)
            } else {
                format!("{area} {w} × {h} px → {rw} × {rh} px")
            }
        }),
    };
    if let Some(note) = note {
        ui.label(RichText::new(note).color(t.text_faint).size(11.0));
    }
    // OpenAI bills per image: what this one should cost, and what the last one did.
    if current.provider == Provider::Openai {
        let (text, tip) = price_note(app, &current);
        ui.label(RichText::new(text).color(t.text_faint).size(11.0)).on_hover_text(tip);
    }
    if job.is_none()
        && why.is_none()
        && (clicked || submit)
        && let Err(e) = generate(app)
    {
        crate::notices::error(app, format!("{}: {e}", tr("AI Generate")));
    }
}

/// The price line for an OpenAI model, and its tooltip.
fn price_note(app: &PhotocraftApp, model: &Model) -> (String, String) {
    use photocraft_aigen::pricing;
    let quality = if model.quality.trim().is_empty() { "auto" } else { model.quality.trim() };
    let estimate = target_size(app).and_then(|(w, h, _)| pricing::estimate(model, w, h));
    let mut text = match &estimate {
        Some(e) => format!("{} {} ({quality})", tr("Est. cost"), e.label()),
        None => match pricing::rates(&model.api_model) {
            Some(r) => format!("{} ${} / 1M {} ({quality})", tr("No published price;"), r.image_output, tr("output tokens")),
            None => tr("No published price for this model").to_string(),
        },
    };
    if let Some(cost) = app.ui.ai_gen.last_cost {
        text = format!("{text} · {} {}", tr("last"), pricing::format_usd(cost));
    }
    let mut tip = tr("From OpenAI's published image prices for this quality. The prompt adds a fraction of a cent.").to_string();
    if estimate.is_some_and(|e| e.approximate) {
        tip.push(' ');
        tip.push_str(tr("≈: OpenAI publishes prices for three sizes of GPT Image 2 only, so other sizes and GPT Image 2.5 are estimated from those."));
    }
    tip.push(' ');
    tip.push_str(tr("\"last\" is what the previous image was billed, from the tokens OpenAI reported."));
    (text, tip)
}

/// Why Generate can't run now (shown under the disabled button), or `None`.
fn generate_blocked(app: &PhotocraftApp, model: &Model) -> Option<String> {
    if app.session.active().is_none() {
        return Some(tr("Open a document to generate into.").to_string());
    }
    if let Err(why) = app.session.ai.ready(model) {
        return Some(why);
    }
    if app.ui.ai_gen.prompt.trim().is_empty() {
        return Some(tr("Write a prompt first.").to_string());
    }
    if let Some(job) = app.session.active().and_then(|d| app.session.job_on(d.doc.id)) {
        return Some(format!("{} ({})", tr("The document is busy"), job.label));
    }
    None
}

/// The size `model` is asked to render for a `w` × `h` area (Gemini: the aspect ratio).
fn render_size(model: &Model, w: u32, h: u32) -> (u32, u32) {
    match model.provider {
        Provider::Comfy => photocraft_aigen::size::render_size(w, h, model.megapixels, model.family.size_multiple()),
        Provider::Openai => photocraft_aigen::cloud::openai_size(model, w, h),
        Provider::Gemini => photocraft_aigen::cloud::gemini_ratio(w, h),
    }
}

// ---------- Preferences › Integrations ----------

/// Dialog fields (not preferences): the model being edited, the server's file lists, the API
/// keys typed in this dialog, what the last "Connect" said, and a request for [`after_body`].
const SELECTED: &str = "__aiSelected";
const FILES: &str = "__aiFiles";
const KEYS: &str = "__aiKeys";
const STATUS: &str = "__aiStatus";
const ACTION: &str = "__aiAction";
/// The page of the section in view, and that the local page already tried to read the server.
const PAGE: &str = "__aiPage";
const AUTO: &str = "__aiAutoConnect";
/// A Connect / Scan running in the background (`{id, action}`), and whether the app has a ComfyUI
/// it started itself (for the Stop button).
const JOB: &str = "__aiJob";
const STARTED: &str = "__aiStarted";

fn text_of(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn label(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(tr(text)).color(t.text_dim));
}

/// A row with a text field.
fn text_row(ui: &mut egui::Ui, obj: &mut Map<String, Value>, key: &str, title: &str, hint: &str) {
    label(ui, title);
    let mut s = text_of(obj, key);
    ui.add(egui::TextEdit::singleline(&mut s).desired_width(FIELD).hint_text(hint));
    obj.insert(key.into(), json!(s));
    ui.end_row();
}

/// A row with a number field.
fn number_row(ui: &mut egui::Ui, obj: &mut Map<String, Value>, key: &str, title: &str, range: std::ops::RangeInclusive<f64>, integer: bool, tip: &str) {
    label(ui, title);
    let mut x = obj.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let drag = egui::DragValue::new(&mut x).range(range).custom_parser(widgets::parse_num);
    let drag = if integer { drag.speed(1.0).max_decimals(0) } else { drag.speed(0.05).max_decimals(2) };
    let resp = ui.add(drag);
    if !tip.is_empty() {
        resp.on_hover_text(tr(tip));
    }
    obj.insert(key.into(), if integer { json!(x.round() as i64) } else { json!(x) });
    ui.end_row();
}

/// A row choosing a model file: a list of the server's files once "Connect" has read them (the
/// current value stays offered even when the server doesn't have it), else a text field.
fn file_row(ui: &mut egui::Ui, obj: &mut Map<String, Value>, key: &str, title: &str, files: Option<&Vec<Value>>, optional: bool) {
    label(ui, title);
    let mut cur = text_of(obj, key);
    match files.filter(|f| !f.is_empty()) {
        Some(list) => {
            let mut names: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
            if !cur.is_empty() && !names.contains(&cur) {
                names.insert(0, cur.clone());
            }
            let none = tr("(none)").to_string();
            let mut options: Vec<(String, &str)> = Vec::with_capacity(names.len() + 1);
            if optional || cur.is_empty() {
                options.push((String::new(), none.as_str()));
            }
            options.extend(names.iter().map(|n| (n.clone(), n.as_str())));
            widgets::dropdown(ui, &format!("ai-file-{key}"), &mut cur, &options, FIELD);
        }
        None => {
            ui.add(egui::TextEdit::singleline(&mut cur).desired_width(FIELD).hint_text("model.safetensors"));
        }
    }
    obj.insert(key.into(), json!(cur));
    ui.end_row();
}

/// A row with a fixed list of choices.
fn choice_row(ui: &mut egui::Ui, obj: &mut Map<String, Value>, key: &str, title: &str, choices: &[&str]) {
    label(ui, title);
    let mut cur = text_of(obj, key);
    let mut options: Vec<(String, &str)> = choices.iter().map(|c| (c.to_string(), *c)).collect();
    let extra = cur.clone();
    if !choices.contains(&cur.as_str()) {
        options.insert(0, (extra.clone(), extra.as_str()));
    }
    widgets::dropdown(ui, &format!("ai-choice-{key}"), &mut cur, &options, FIELD);
    obj.insert(key.into(), json!(cur));
    ui.end_row();
}

/// Samplers and schedulers ComfyUI ships; a value outside the list (a custom node's) is kept.
const SAMPLERS: [&str; 14] = [
    "euler",
    "euler_ancestral",
    "heun",
    "dpm_2",
    "dpmpp_2s_ancestral",
    "dpmpp_sde",
    "dpmpp_2m",
    "dpmpp_2m_sde",
    "dpmpp_3m_sde",
    "lcm",
    "res_multistep",
    "er_sde",
    "ddim",
    "uni_pc",
];
const SCHEDULERS: [&str; 9] = ["simple", "normal", "karras", "exponential", "sgm_uniform", "beta", "ddim_uniform", "linear_quadratic", "kl_optimal"];

/// Add `model` to the draft with a fresh id, and select it.
fn push_model(ai: &mut Map<String, Value>, f: &mut Map<String, Value>, mut model: Model) {
    let settings: Settings = serde_json::from_value(Value::Object(ai.clone())).unwrap_or_default();
    model.id = settings.unique_id(&model.name);
    let Ok(v) = serde_json::to_value(&model) else { return };
    if let Some(list) = ai.entry("models").or_insert_with(|| json!([])).as_array_mut() {
        list.push(v);
        f.insert(SELECTED.into(), json!(list.len().saturating_sub(1)));
    }
}

/// The pages of the Integrations section: (key, title).
const PAGES: [(&str, &str); 4] = [("local", "Local Models"), ("cloud", "Cloud Models"), ("keys", "API Keys"), ("server", "Local Server")];
/// Width of the model list beside the selected model's settings, and height of both.
const LIST_WIDTH: f32 = 180.0;
const PANE_HEIGHT: f32 = 420.0;
/// Width of a field in the model settings (what the pane leaves beside the labels).
const FIELD: f32 = 200.0;

fn is_local_model(m: &Value) -> bool {
    m.get("provider").and_then(Value::as_str).unwrap_or(Provider::Comfy.key()) == Provider::Comfy.key()
}

/// The Integrations section's body: draws and edits `integrations.ai` in the dialog's draft
/// (`obj` is the section's object; `f` the dialog's fields). Four pages: local models and cloud
/// models (each a list beside the selected model's settings), API keys, and the local server.
pub fn prefs_section(ui: &mut egui::Ui, obj: &mut Map<String, Value>, f: &mut Map<String, Value>) {
    let mut ai = obj.get("ai").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut page = f.get(PAGE).and_then(Value::as_str).filter(|p| PAGES.iter().any(|(k, _)| k == p)).unwrap_or("local").to_string();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (key, title) in PAGES {
            if widgets::pill_tab(ui, tr(title), page == key).clicked() {
                page = key.to_string();
            }
        }
    });
    ui.add_space(4.0);
    widgets::hairline(ui);
    ui.add_space(8.0);
    match page.as_str() {
        "keys" => keys_page(ui, f),
        "server" => server_page(ui, &mut ai, f),
        "cloud" => models_page(ui, &mut ai, f, false),
        _ => models_page(ui, &mut ai, f, true),
    }
    f.insert(PAGE.into(), json!(page));
    obj.insert("ai".into(), Value::Object(ai));
}

/// What the last Connect / Detect said, or `hint` before there was one.
fn status_line(ui: &mut egui::Ui, f: &Map<String, Value>, hint: &str, width: f32) {
    let t = Tokens::get(ui.ctx());
    let (text, color, size) = match f.get(STATUS).and_then(Value::as_object) {
        Some(status) => {
            let error = status.get("error").and_then(Value::as_bool).unwrap_or(false);
            (status.get("text").and_then(Value::as_str).unwrap_or("").to_string(), if error { t.danger } else { t.text_faint }, 11.5)
        }
        None => (tr(hint).to_string(), t.text_faint, 11.0),
    };
    // Wrapped to the page (an error can be three sentences long: unwrapped it ran off the dialog
    // and widened everything else with it) and selectable, so it can be copied.
    ui.scope(|ui| {
        ui.set_max_width(width);
        ui.add(egui::Label::new(RichText::new(text).color(color).size(size)).wrap().selectable(true));
    });
}

/// Local Server: where local models run, and how PhotoCraft starts it.
fn server_page(ui: &mut egui::Ui, ai: &mut Map<String, Value>, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let busy = f.get(JOB).is_some();
    ui.label(
        RichText::new(tr("Local models are run by ComfyUI. PhotoCraft sends it the workflow and collects the image; it can also start it for you, hidden."))
            .color(t.text_dim),
    );
    ui.add_space(8.0);
    egui::Grid::new("ai-server").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        label(ui, "Address");
        ui.horizontal(|ui| {
            let mut url = text_of(ai, "comfyUrl");
            ui.add(egui::TextEdit::singleline(&mut url).desired_width(240.0).hint_text("http://127.0.0.1:8188"));
            ai.insert("comfyUrl".into(), json!(url));
            let connect = ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, tr("Connect"), 80.0)).inner;
            if connect.on_hover_text(tr("Check the server (starting it if that is switched on below) and read the model files it has")).clicked() {
                f.insert(ACTION.into(), json!("connect"));
            }
        });
        ui.end_row();
        label(ui, "");
        status_line(ui, f, "The portable build of ComfyUI listens on port 8188, ComfyUI Desktop on port 8000.", 420.0);
        ui.end_row();
        label(ui, "Give up after");
        let mut secs = ai.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(600);
        ui.add(egui::DragValue::new(&mut secs).range(10..=3600).suffix(" s").custom_parser(widgets::parse_num))
            .on_hover_text(tr("A generation that takes longer than this is stopped. Raise it for large models that load slowly"));
        ai.insert("timeoutSeconds".into(), json!(secs));
        ui.end_row();
    });

    // ----- starting it -----
    ui.add_space(10.0);
    widgets::section_label(ui, tr("Starting ComfyUI"));
    ui.add_space(2.0);
    let mut autostart = ai.get("comfyAutostart").and_then(Value::as_bool).unwrap_or(false);
    widgets::checkbox(ui, &mut autostart, tr("Start ComfyUI when a local model needs it and it isn't running"));
    ai.insert("comfyAutostart".into(), json!(autostart));
    ui.label(
        RichText::new(tr("A ComfyUI that is already running (ComfyUI Desktop, for example) is used as it is and left running. One that PhotoCraft starts runs hidden, on the port of the address above, and is stopped when PhotoCraft quits."))
            .color(t.text_faint)
            .size(11.0),
    );
    ui.add_space(6.0);
    let mut launch = ai.get("comfyLaunch").and_then(Value::as_object).cloned().unwrap_or_default();
    egui::Grid::new("ai-launch").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
        for (key, title, hint, tip) in [
            (
                "python",
                "Python",
                "…\\.venv\\Scripts\\python.exe",
                "The Python that has ComfyUI's packages: its virtual environment's, or the portable build's python_embeded",
            ),
            ("folder", "Folder", "…\\ComfyUI", "The folder ComfyUI is started in"),
        ] {
            label(ui, title);
            let mut v = text_of(&launch, key);
            ui.add(egui::TextEdit::singleline(&mut v).desired_width(380.0).hint_text(hint)).on_hover_text(tr(tip));
            launch.insert(key.into(), json!(v));
            ui.end_row();
        }
        label(ui, "Arguments");
        let mut v = text_of(&launch, "args");
        ui.add(egui::TextEdit::multiline(&mut v).desired_rows(3).desired_width(380.0).hint_text("main.py"))
            .on_hover_text(tr("The script and its options, as on ComfyUI's command line. Leave out --port: the address above decides it"));
        // One line: a pasted command line with line breaks is still one command.
        launch.insert("args".into(), json!(v.replace(['\r', '\n'], " ")));
        ui.end_row();
    });
    ai.insert("comfyLaunch".into(), Value::Object(launch));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let find = ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, tr("Find Installation"), 130.0)).inner;
        if find.on_hover_text(tr("Fill these in from the ComfyUI that is running now. Start it once the way you usually do, then press this")).clicked() {
            f.insert(ACTION.into(), json!("find"));
        }
        let started = f.get(STARTED).and_then(Value::as_bool).unwrap_or(false);
        let stop = ui.add_enabled_ui(started && !busy, |ui| widgets::secondary_button(ui, tr("Stop Server"), 100.0)).inner;
        if stop.on_hover_text(tr("Stop the ComfyUI that PhotoCraft started")).on_disabled_hover_text(tr("PhotoCraft has not started a ComfyUI")).clicked() {
            f.insert(ACTION.into(), json!("stop"));
        }
        if started {
            ui.label(RichText::new(tr("Running, started by PhotoCraft")).color(t.text_faint).size(11.5));
        }
    });
    // What the server offers, once it has been read.
    if let Some(files) = f.get(FILES).and_then(Value::as_object) {
        ui.add_space(10.0);
        let count = |folder: &str| files.get(folder).and_then(Value::as_array).map_or(0, Vec::len);
        ui.label(
            RichText::new(format!(
                "{}: {} {}, {} {}, {} {}, {} {}, {} {}",
                tr("On the server"),
                count("diffusion_models"),
                tr("diffusion models"),
                count("checkpoints"),
                tr("checkpoints"),
                count("text_encoders"),
                tr("text encoders"),
                count("vae"),
                tr("VAEs"),
                count("loras"),
                tr("LoRAs"),
            ))
            .color(t.text_faint)
            .size(11.0),
        );
    }
}

/// API Keys: one per cloud provider.
fn keys_page(ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let mut keys = f.get(KEYS).and_then(Value::as_object).cloned().unwrap_or_default();
    // Two columns: a field only gets its full width in a grid's last column.
    egui::Grid::new("ai-keys").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        for p in Provider::ALL.into_iter().filter(|p| !p.is_local()) {
            label(ui, p.label());
            let saved = keys.get(&format!("{}Saved", p.key())).and_then(Value::as_bool).unwrap_or(false);
            let mut key = text_of(&keys, p.key());
            let mut remove = keys.get(&format!("{}Remove", p.key())).and_then(Value::as_bool).unwrap_or(false);
            ui.horizontal(|ui| {
                let hint = if saved { tr("Saved. Type a new key to replace it") } else { tr("Paste your API key") };
                ui.add(egui::TextEdit::singleline(&mut key).password(true).desired_width(300.0).hint_text(hint));
                if saved {
                    widgets::checkbox(ui, &mut remove, tr("Remove"));
                }
            });
            keys.insert(p.key().into(), json!(key));
            keys.insert(format!("{}Remove", p.key()), json!(remove));
            ui.end_row();
        }
    });
    f.insert(KEYS.into(), Value::Object(keys));
    ui.add_space(8.0);
    ui.label(
        RichText::new(tr("Keys are stored on this computer, apart from the preferences, and are sent only to their provider.")).color(t.text_faint).size(11.0),
    );
    ui.label(RichText::new(tr("Without a saved key, the OPENAI_API_KEY and GEMINI_API_KEY environment variables are used.")).color(t.text_faint).size(11.0));
}

/// Local Models / Cloud Models: the models of that kind in a list, and the selected one's
/// settings beside it, so choosing a model shows its settings without scrolling.
fn models_page(ui: &mut egui::Ui, ai: &mut Map<String, Value>, f: &mut Map<String, Value>, local: bool) {
    let t = Tokens::get(ui.ctx());
    // Local pages pick files from the server's lists: read them once, when the server is on
    // this machine (it answers or refuses at once; a remote one waits for Connect).
    if local && f.get(FILES).is_none() && f.get(STATUS).is_none() && f.get(AUTO).is_none() {
        f.insert(AUTO.into(), json!(true));
        let url = text_of(ai, "comfyUrl");
        if url.contains("//127.0.0.1") || url.contains("//localhost") || url.contains("//[::1]") {
            f.insert(ACTION.into(), json!("peek"));
        }
    }
    ui.horizontal(|ui| {
        if local {
            let add = widgets::secondary_button(ui, tr("Add Model"), 96.0);
            egui::Popup::menu(&add).show(|ui| {
                for family in Family::ALL {
                    if ui.button(family.label()).clicked() {
                        push_model(ai, f, family.preset());
                        ui.close();
                    }
                }
            });
            if widgets::secondary_button(ui, tr("Scan Server"), 104.0)
                .on_hover_text(tr("Add a model for every text-to-image model file the ComfyUI server has"))
                .clicked()
            {
                f.insert(ACTION.into(), json!("detect"));
            }
        } else {
            let add = widgets::secondary_button(ui, tr("Add Model"), 96.0);
            egui::Popup::menu(&add).show(|ui| {
                for m in Model::cloud_presets() {
                    if ui.button(format!("{} ({})", m.name, m.provider.label())).clicked() {
                        push_model(ai, f, m);
                        ui.close();
                    }
                }
            });
        }
    });
    if local {
        status_line(ui, f, "Scan Server adds the models your ComfyUI server has. Set its address on the Local Server page.", 520.0);
    } else {
        // A cloud model without its key can't run: say so here, where the model is.
        let keys = f.get(KEYS).and_then(Value::as_object);
        let missing: Vec<&str> = Provider::ALL
            .into_iter()
            .filter(|p| !p.is_local())
            .filter(|p| ai.get("models").and_then(Value::as_array).is_some_and(|ms| ms.iter().any(|m| text_of_value(m, "provider") == p.key())))
            .filter(|p| {
                let saved = keys.and_then(|k| k.get(&format!("{}Saved", p.key()))).and_then(Value::as_bool).unwrap_or(false);
                let typed = keys.is_some_and(|k| !text_of(k, p.key()).trim().is_empty());
                !saved && !typed
            })
            .map(Provider::label)
            .collect();
        if missing.is_empty() {
            ui.label(RichText::new(tr("Cloud models are billed by their provider for each image.")).color(t.text_faint).size(11.0));
        } else {
            ui.label(RichText::new(format!("{}: {}", tr("No API key yet (add it on the API Keys page)"), missing.join(", "))).color(t.warning).size(11.5));
        }
    }
    ui.add_space(6.0);

    let mut models = ai.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    let shown: Vec<usize> = models.iter().enumerate().filter(|(_, m)| m.is_object() && is_local_model(m) == local).map(|(i, _)| i).collect();
    // The selection is an index into all models; on a page without it, the page's first.
    let mut selected = f.get(SELECTED).and_then(Value::as_u64).map(|i| i as usize).filter(|i| shown.contains(i)).or_else(|| shown.first().copied());
    if shown.is_empty() {
        let hint = if local { "No local models yet. Use Scan Server, or add one." } else { "No cloud models yet. Add one, then its API key." };
        ui.label(RichText::new(tr(hint)).color(t.text_faint));
        ai.insert("models".into(), Value::Array(models));
        return;
    }
    let files = f.get(FILES).and_then(Value::as_object).cloned();
    let mut remove = None;
    ui.horizontal_top(|ui| {
        // ----- the list -----
        ui.vertical(|ui| {
            ui.set_width(LIST_WIDTH);
            egui::ScrollArea::vertical().id_salt(("ai-model-list", local)).max_height(PANE_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
                for &i in &shown {
                    let Some(m) = models.get_mut(i).and_then(Value::as_object_mut) else { continue };
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::click());
                    let on = selected == Some(i);
                    if on {
                        ui.painter().rect_filled(rect, t.radius_sm, t.row_selected);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                    }
                    if resp.clicked() {
                        selected = Some(i);
                    }
                    let side = |x: f32| egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(22.0, rect.height()));
                    let mut check =
                        ui.new_child(egui::UiBuilder::new().max_rect(side(rect.left() + 4.0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    let mut enabled = m.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                    widgets::checkbox(&mut check, &mut enabled, "").on_hover_text(tr("Show in the AI Gen panel"));
                    m.insert("enabled".into(), json!(enabled));
                    // The delete button shows on the row in use, so a long list stays quiet.
                    if on || resp.hovered() {
                        let mut bin =
                            ui.new_child(egui::UiBuilder::new().max_rect(side(rect.right() - 24.0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
                        if crate::icons::button(&mut bin, "trash", 20.0, false, tr("Remove this model")).clicked() {
                            remove = Some(i);
                        }
                    }
                    let name = text_of(m, "name");
                    let color = if enabled { t.text } else { t.text_faint };
                    let mut job = egui::text::LayoutJob::simple_singleline(name.clone(), egui::FontId::proportional(12.5), color);
                    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 56.0).max(20.0));
                    let galley = ui.painter().layout_job(job);
                    let elided = galley.elided;
                    ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0), galley, color);
                    if elided {
                        resp.on_hover_text(name);
                    }
                }
            });
        });
        widgets::vline(ui, PANE_HEIGHT);
        // ----- the selected model -----
        ui.vertical(|ui| {
            egui::ScrollArea::vertical().id_salt(("ai-model-settings", local)).max_height(PANE_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
                if let Some(m) = selected.and_then(|i| models.get_mut(i)).and_then(Value::as_object_mut) {
                    model_settings(ui, m, files.as_ref(), selected.unwrap_or(0));
                }
            });
        });
    });
    if let Some(i) = remove.filter(|i| *i < models.len()) {
        models.remove(i);
        // Stay near where the removed model was: the next of this kind, else the previous.
        let after: Vec<usize> = models.iter().enumerate().filter(|(_, m)| m.is_object() && is_local_model(m) == local).map(|(i, _)| i).collect();
        selected = after.iter().copied().find(|x| *x >= i).or_else(|| after.last().copied());
    }
    ai.insert("models".into(), Value::Array(models));
    if let Some(i) = selected {
        f.insert(SELECTED.into(), json!(i));
    }
}

fn text_of_value(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The settings of one model (`m` is its object in the draft; `files` the server's lists).
fn model_settings(ui: &mut egui::Ui, m: &mut Map<String, Value>, files: Option<&Map<String, Value>>, index: usize) {
    let t = Tokens::get(ui.ctx());
    let folder = |name: &str| files.and_then(|f| f.get(name)).and_then(Value::as_array);
    let provider = Provider::ALL.into_iter().find(|p| p.key() == text_of(m, "provider")).unwrap_or_default();
    let group = |ui: &mut egui::Ui, title: &str| {
        ui.label(RichText::new(tr(title)).font(crate::theme::medium(11.5)).color(t.text_faint));
        ui.label("");
        ui.end_row();
    };
    egui::Grid::new(("ai-model", index)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        text_row(ui, m, "name", "Name", "");
        if provider.is_local() {
            label(ui, "Architecture");
            let mut family = Family::from_key(&text_of(m, "family")).unwrap_or_default();
            let options: Vec<(Family, &str)> = Family::ALL.into_iter().map(|x| (x, x.label())).collect();
            widgets::dropdown(ui, "ai-family", &mut family, &options, FIELD);
            m.insert("family".into(), json!(family.key()));
            ui.end_row();

            group(ui, "Files");
            if family == Family::Custom {
                label(ui, "Workflow");
                let mut text = text_of(m, "workflow");
                ui.vertical(|ui| {
                    ui.set_max_width(FIELD + 8.0);
                    ui.add(egui::TextEdit::multiline(&mut text).desired_rows(6).desired_width(FIELD).font(egui::TextStyle::Monospace).hint_text("{ \"1\": { \"class_type\": … } }"));
                    ui.label(
                        RichText::new(tr("ComfyUI › Workflow › Export (API). Use %prompt%, %negative%, %width%, %height%, %seed%, %steps% and %cfg% where those values go."))
                            .color(t.text_faint)
                            .size(11.0),
                    );
                });
                m.insert("workflow".into(), json!(text));
                ui.end_row();
            } else if family.uses_checkpoint() {
                file_row(ui, m, "checkpoint", "Checkpoint", folder("checkpoints"), false);
            } else {
                file_row(ui, m, "unet", "Diffusion model", folder("diffusion_models"), false);
                if family == Family::Ideogram4 {
                    file_row(ui, m, "unet2", "Unconditional", folder("diffusion_models"), true);
                }
                let encoders = [("clip", "Text encoder"), ("clip2", "Text encoder 2"), ("clip3", "Text encoder 3"), ("clip4", "Text encoder 4")];
                for (key, title) in encoders.into_iter().take(family.text_encoders()) {
                    file_row(ui, m, key, title, folder("text_encoders"), false);
                }
                file_row(ui, m, "vae", "VAE", folder("vae"), false);
            }
            if family != Family::Custom && (!family.uses_checkpoint() || family == Family::Checkpoint) {
                file_row(ui, m, "lora", "LoRA", folder("loras"), true);
                if !text_of(m, "lora").is_empty() {
                    number_row(ui, m, "loraStrength", "LoRA strength", -4.0..=4.0, false, "");
                }
            }

            group(ui, "Sampling");
            if family != Family::Custom {
                choice_row(ui, m, "sampler", "Sampler", &SAMPLERS);
                if family != Family::Flux2 {
                    choice_row(ui, m, "scheduler", "Scheduler", &SCHEDULERS);
                }
            }
            number_row(ui, m, "steps", "Steps", 1.0..=200.0, true, "");
            number_row(ui, m, "cfg", "CFG", 0.0..=30.0, false, "Classifier-free guidance. 1 turns it off (distilled and turbo models)");
            if family.has_guidance() {
                number_row(ui, m, "guidance", "Guidance", 0.0..=30.0, false, "FLUX distilled guidance. 0 leaves it out");
            }
            if family.has_shift() {
                let title = if family == Family::HidreamO1 { "Noise scale" } else { "Shift" };
                number_row(ui, m, "shift", title, 0.0..=30.0, false, "Model sampling shift. 0 uses the model's own");
            }
            number_row(ui, m, "seed", "Seed", -1.0..=281_474_976_710_655.0, true, "-1 uses a new random seed each time");
            text_row(ui, m, "negativePrompt", "Negative prompt", tr("Used when CFG is above 1"));

            group(ui, "Output");
            number_row(ui, m, "megapixels", "Render size (MP)", 0.0..=16.0, false, "The model renders at about this many megapixels in the selection's shape, then the result is fitted to the selection. 0 renders at the selection's own size");
        } else {
            label(ui, "Provider");
            ui.label(RichText::new(provider.label()).color(t.text));
            ui.end_row();
            text_row(ui, m, "apiModel", "Model ID", if provider == Provider::Openai { "gpt-image-2.5-sunburst" } else { "gemini-3-pro-image-preview" });
            group(ui, "Output");
            if provider == Provider::Openai {
                choice_row(ui, m, "quality", "Quality", &["auto", "low", "medium", "high", "xhigh", "max"]);
                number_row(ui, m, "megapixels", "Render size (MP)", 0.0..=8.0, false, "About this many megapixels in the selection's shape (models that take any size). 0 follows the selection");
            } else {
                choice_row(ui, m, "imageSize", "Image size", &["1K", "2K", "4K"]);
            }
        }
        label(ui, "Transparency");
        let mut alpha = m.get("alpha").and_then(Value::as_bool).unwrap_or(false);
        widgets::checkbox(ui, &mut alpha, tr("Can generate it")).on_hover_text(tr("The model can return a transparent background. Enables the Alpha Channel box in the AI Gen panel for it"));
        m.insert("alpha".into(), json!(alpha));
        ui.end_row();
    });
}

/// Preferences opened: note which providers already have a key (never the keys themselves).
pub fn prefs_opened(app: &PhotocraftApp, f: &mut Map<String, Value>) {
    let mut keys = Map::new();
    for p in Provider::ALL.into_iter().filter(|p| !p.is_local()) {
        keys.insert(format!("{}Saved", p.key()), json!(!app.session.ai.key(p).is_empty()));
    }
    f.insert(KEYS.into(), Value::Object(keys));
}

/// The draft `integrations.ai` of the dialog.
fn draft(f: &Map<String, Value>) -> Value {
    f.get("values").and_then(|v| v.pointer("/integrations/ai")).cloned().unwrap_or_else(|| json!({}))
}

/// A Connect / Scan ended with `result` (`ai.comfyModels`'s): keep the file lists, add the
/// detected models for a scan, and say what happened.
fn connected(f: &mut Map<String, Value>, action: &str, result: &Value) {
    let folders = result.get("folders").cloned().unwrap_or_else(|| json!({}));
    let count = folders.get("diffusion_models").and_then(Value::as_array).map_or(0, Vec::len)
        + folders.get("checkpoints").and_then(Value::as_array).map_or(0, Vec::len);
    let mut text = format!("{} · {count} {}", tr("Connected"), tr("model files"));
    if result.get("startedHere").and_then(Value::as_bool) == Some(true) {
        text = format!("{text} · {}", tr("started by PhotoCraft"));
    }
    if action == "detect" {
        let added = add_detected(f, &folders);
        text = format!("{text} · {added} {}", tr("models added"));
    }
    f.insert(FILES.into(), folders);
    f.insert(STATUS.into(), json!({"error": false, "text": text}));
}

fn failed(f: &mut Map<String, Value>, why: &str) {
    f.insert(STATUS.into(), json!({"error": true, "text": why}));
}

/// A Connect / Scan / look failed. The look made when the page opens is not something the user
/// asked for, so a server that isn't running is a note about what will happen, not an error.
fn connect_failed(f: &mut Map<String, Value>, action: &str, why: &str) {
    if action != "peek" {
        return failed(f, why);
    }
    let autostart = draft(f).get("comfyAutostart").and_then(Value::as_bool).unwrap_or(false);
    let text = if autostart {
        tr("ComfyUI isn't running. PhotoCraft will start it when you generate or press Scan Server.")
    } else {
        tr("ComfyUI isn't running. Start it to pick files from lists, or set PhotoCraft up to start it on the Local Server page.")
    };
    f.insert(STATUS.into(), json!({"error": false, "text": text}));
}

/// After the dialog body: run what a button asked for (the body has no access to the app), and
/// follow a Connect / Scan that is running in the background.
pub fn after_body(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    f.insert(STARTED.into(), json!(app.session.ai.server.as_ref().is_some_and(|s| s.state() == photocraft_aigen::ServerState::Started)));
    if let Some(job) = f.get(JOB).cloned() {
        let id = job.get("id").and_then(Value::as_u64).unwrap_or(0);
        let action = job.get("action").and_then(Value::as_str).unwrap_or("connect").to_string();
        match app.session.jobs_with_recent().into_iter().rev().find(|j| j.id.0 == id) {
            Some(j) if j.state == "running" => {
                let text = if j.message.is_empty() { tr("Connecting…").to_string() } else { j.message };
                f.insert(STATUS.into(), json!({"error": false, "text": text}));
            }
            Some(j) => {
                f.remove(JOB);
                match (j.state, j.result) {
                    ("done", Some(result)) => connected(f, &action, &result),
                    (_, _) => connect_failed(f, &action, &j.error.unwrap_or_else(|| tr("Cancelled").to_string())),
                }
            }
            None => {
                f.remove(JOB);
            }
        }
        return;
    }
    let Some(action) = f.remove(ACTION).and_then(|a| a.as_str().map(str::to_string)) else { return };
    let settings = draft(f);
    match action.as_str() {
        "find" => match app.session.execute("ai.server.find", json!({"settings": settings})) {
            Ok(found) => {
                let complete = ["python", "folder"].iter().all(|k| found.get(*k).and_then(Value::as_str).is_some_and(|v| !v.is_empty()));
                if let Some(ai) = f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")).and_then(Value::as_object_mut) {
                    ai.insert("comfyLaunch".into(), found);
                    if complete {
                        ai.insert("comfyAutostart".into(), json!(true));
                    }
                }
                let text = if complete {
                    tr("Found the running ComfyUI. PhotoCraft will start it this way when it isn't running.")
                } else {
                    tr("Found its arguments, but not its Python and folder: fill those in.")
                };
                f.insert(STATUS.into(), json!({"error": !complete, "text": text}));
            }
            Err(e) => failed(f, &e.to_string()),
        },
        "stop" => match app.session.execute("ai.server.stop", json!({})) {
            Ok(r) if r.get("stopped").and_then(Value::as_bool) == Some(true) => {
                f.insert(STATUS.into(), json!({"error": false, "text": tr("Stopped the ComfyUI that PhotoCraft started.")}));
                f.insert(STARTED.into(), json!(false));
            }
            Ok(_) => failed(f, tr("PhotoCraft has not started a ComfyUI. One started elsewhere is left running.")),
            Err(e) => failed(f, &e.to_string()),
        },
        _ => {
            // Opening the page only looks: it never starts a server.
            let params = json!({"settings": settings, "start": action != "peek"});
            // Starting ComfyUI takes a while: in the background where the app has jobs.
            let started = if app.background_jobs {
                app.session.start("ai.comfyModels", params).map_err(|e| e.to_string())
            } else {
                app.session.execute("ai.comfyModels", params).map(photocraft_engine::jobs::Started::Done).map_err(|e| e.to_string())
            };
            match started {
                Ok(photocraft_engine::jobs::Started::Done(result)) => connected(f, &action, &result),
                Ok(photocraft_engine::jobs::Started::Job(id)) => {
                    f.insert(JOB.into(), json!({"id": id.0, "action": action}));
                    f.insert(STATUS.into(), json!({"error": false, "text": tr("Connecting…")}));
                }
                Err(e) => connect_failed(f, &action, &e),
            }
        }
    }
}

/// Add the models detected in `folders` that the draft doesn't have yet. Returns how many.
fn add_detected(f: &mut Map<String, Value>, folders: &Value) -> usize {
    let Ok(folders) = serde_json::from_value(folders.clone()) else { return 0 };
    let Some(ai) = f.get("values").and_then(|v| v.pointer("/integrations/ai")).and_then(Value::as_object).cloned() else { return 0 };
    let mut ai = ai;
    let before: Settings = serde_json::from_value(Value::Object(ai.clone())).unwrap_or_default();
    let mut added = 0;
    for m in photocraft_aigen::detect::detect(&folders) {
        let have = before.models.iter().any(|x| x.provider.is_local() && x.family == m.family && x.unet == m.unet && x.checkpoint == m.checkpoint);
        if !have {
            push_model(&mut ai, f, m);
            added += 1;
        }
    }
    if let Some(slot) = f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")) {
        *slot = Value::Object(ai);
    }
    added
}

/// Preferences confirmed (OK / Apply): store the keys typed in the dialog, and save them.
pub fn prefs_confirmed(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<(), String> {
    let Some(keys) = f.get(KEYS).and_then(Value::as_object) else { return Ok(()) };
    let mut changed = false;
    for p in Provider::ALL.into_iter().filter(|p| !p.is_local()) {
        let typed = text_of(keys, p.key());
        let remove = keys.get(&format!("{}Remove", p.key())).and_then(Value::as_bool).unwrap_or(false);
        if !typed.trim().is_empty() {
            app.session.execute("ai.setKey", json!({"provider": p.key(), "key": typed})).map_err(|e| e.to_string())?;
            changed = true;
        } else if remove {
            app.session.execute("ai.setKey", json!({"provider": p.key(), "key": ""})).map_err(|e| e.to_string())?;
            changed = true;
        }
    }
    if changed && let Some(save) = app.services.save_ai_keys.as_mut() {
        save(&app.session.ai.keys_json())?;
    }
    Ok(())
}

/// At launch: restore the API keys the app saved.
pub fn load_keys(app: &mut PhotocraftApp) {
    if let Some(text) = app.services.load_ai_keys.as_mut().and_then(|f| f()) {
        app.session.ai.load_keys_json(&text);
    }
}

#[cfg(test)]
mod tests;
