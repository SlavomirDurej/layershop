//! AI Gen › Edit › **Remove BG**, and its page in Preferences › Generative AI.
//!
//! The tab says what to keep ([`Keep`]), what kind of picture it is (which matting model cuts the
//! edge) and where the result goes, and runs the engine's `ai.removeBackground`. Keeping what is
//! clicked has the canvas: while that is chosen the pointer places dots on the picture
//! ([`pointer`], [`canvas`]), as Regions has it for its boxes. The page lists the models with
//! their licences and installs them.

use egui::{Color32, RichText, Stroke, vec2};
use photocraft_aigen::bg_removal::{self, BgModel};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{ACTION, OPEN_URL, PAGE as PAGE_FIELD, EditTask, Mode, edit_label, tr};
use crate::PhotocraftApp;
use super::{CanvasEvent as ToolEvent, ViewXform};
use crate::state::Tool;
use crate::theme::Tokens;
use crate::ai_widgets as widgets;

/// The engine's command.
pub(super) const CMD: &str = "ai.removeBackground";
/// The page's key among the section's pages.
pub(super) const PAGE: &str = "removeBg";
/// A model's download running (`{id, model, text, progress}`), and what the last one said.
const INSTALL_JOB: &str = "__aiRemoveBgInstallJob";
const INSTALL_STATUS: &str = "__aiRemoveBgInstallStatus";
/// The page's actions: download a model (`removeBgInstall:<id>`), stop that download.
const INSTALL: &str = "removeBgInstall:";
const STOP_INSTALL: &str = "removeBgStopInstall";
/// How often the panel looks at the disk for the models, in seconds.
const LOOK_EVERY: f64 = 2.0;
/// How near a dot a click takes it away, in points of the screen.
const DOT_REACH: f32 = 9.0;
const DOT_RADIUS: f32 = 6.5;

/// What to keep of the layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    /// What the matting model takes for the main subject.
    #[default]
    Subject,
    /// What is in the selection.
    Selection,
    /// What SAM 3.1 finds from a few words.
    Describe,
    /// What SAM 3.1 finds at clicks on the picture.
    Points,
}

impl Keep {
    const ALL: [Keep; 4] = [Keep::Subject, Keep::Selection, Keep::Describe, Keep::Points];

    fn key(self) -> &'static str {
        match self {
            Keep::Subject => "subject",
            Keep::Selection => "selection",
            Keep::Describe => "describe",
            Keep::Points => "points",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Keep::Subject => "Main subject",
            Keep::Selection => "Inside selection",
            Keep::Describe => "Describe it",
            Keep::Points => "Click on it",
        }
    }

    /// It is SAM 3.1 that finds it.
    fn needs_target(self) -> bool {
        matches!(self, Keep::Describe | Keep::Points)
    }
}

/// What kind of picture it is: which matting model cuts the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// BiRefNet: quick, and free for any use.
    #[default]
    Photo,
    /// RMBG 3.0: single strands of hair. Non-commercial, so it is chosen and never the default.
    Fine,
    Graphic,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Photo, Kind::Fine, Kind::Graphic];

    fn label(self) -> &'static str {
        match self {
            Kind::Photo => "Photo",
            Kind::Fine => "Photo, finest edges",
            Kind::Graphic => "Graphic, logo or glass",
        }
    }

    fn model(self) -> &'static BgModel {
        match self {
            Kind::Photo => &bg_removal::MATTE_MODELS[0],
            Kind::Graphic => &bg_removal::MATTE_MODELS[1],
            Kind::Fine => &bg_removal::MATTE_MODELS[2],
        }
    }
}

/// Where the result goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Output {
    #[default]
    Mask,
    Layer,
    Selection,
}

impl Output {
    const ALL: [Output; 3] = [Output::Mask, Output::Layer, Output::Selection];

    fn key(self) -> &'static str {
        match self {
            Output::Mask => "mask",
            Output::Layer => "layer",
            Output::Selection => "selection",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Output::Mask => "Layer mask",
            Output::Layer => "New layer",
            Output::Selection => "Selection",
        }
    }
}

/// A click on the picture, in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub x: f64,
    pub y: f64,
    /// On what to keep (else on what to leave out).
    pub keep: bool,
}

/// The tab's state (in `AiGenUi`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoveBgUi {
    pub keep: Keep,
    pub kind: Kind,
    pub output: Output,
    /// Clean the background's colour out of the edge.
    pub clean: bool,
    /// Describe it: what to keep, in a few words.
    pub phrase: String,
    /// Click on it: the dots, on the layer they were placed on.
    #[serde(skip)]
    pub dots: Vec<Dot>,
    #[serde(skip)]
    dots_on: Option<(u64, u64)>,
    /// The tool the canvas was armed with (the Move tool); picking another hands it back.
    #[serde(skip)]
    armed: Option<Tool>,
    #[serde(skip)]
    was_on: bool,
    /// Which models are installed, and when the disk was last looked at (UI time).
    #[serde(skip)]
    installed: Vec<(&'static str, bool)>,
    /// What each still has to download, in bytes.
    #[serde(skip)]
    to_fetch: Vec<(&'static str, u64)>,
    #[serde(skip)]
    looked: Option<f64>,
    /// The offscreen snapshot says what is installed: the disk is not looked at.
    #[serde(skip)]
    staged: bool,
}

/// Is the Remove BG tab in front?
pub fn tab_open(app: &PhotocraftApp) -> bool {
    let ui = &app.ui.ai_gen;
    ui.mode == Mode::Edit && ui.task == EditTask::RemoveBg
}

/// Are the dots on the canvas: the panel showing, on this tab, keeping what is clicked?
fn on_canvas(app: &PhotocraftApp) -> bool {
    super::shown(app) && tab_open(app) && app.ui.ai_gen.remove_bg.keep == Keep::Points && app.session.active().is_some()
}

/// Does the pointer place dots now (the canvas was armed, and no other tool picked since)?
pub(crate) fn placing(app: &PhotocraftApp) -> bool {
    on_canvas(app) && app.ui.ai_gen.remove_bg.armed.is_some_and(|t| t == app.ui.tool)
}

/// Give the pointer to the dots: the Move tool, which draws nothing of its own over them.
fn arm(app: &mut PhotocraftApp) {
    app.ui.tool = Tool::Move;
    app.ui.ai_gen.remove_bg.armed = Some(Tool::Move);
}

/// The document and layer in front.
fn target(app: &PhotocraftApp) -> Option<(u64, u64)> {
    let d = app.session.active()?;
    Some((d.doc.id.0, d.active_layer?.0))
}

/// Once a frame from the panel: arm the canvas when clicking comes to the front, let go of it
/// when another tool was picked, and drop dots that were placed on another layer.
pub(super) fn follow(app: &mut PhotocraftApp) {
    let on = on_canvas(app);
    if on && !app.ui.ai_gen.remove_bg.was_on {
        arm(app);
    }
    let here = target(app);
    let tool = app.ui.tool;
    let st = &mut app.ui.ai_gen.remove_bg;
    st.was_on = on;
    if !on || st.armed.is_some_and(|t| t != tool) {
        st.armed = None;
    }
    if st.dots_on != here {
        st.dots.clear();
        st.dots_on = here;
    }
}

/// Pointer input while dots are being placed. A press places one (with Alt, on what to leave
/// out), or takes away the one it is on. Returns true when the event was theirs.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) -> bool {
    if !placing(app) {
        return false;
    }
    if let ToolEvent::Down { x, y, .. } = ev {
        let Some(canvas) = app.session.active().map(|d| d.doc.bounds()) else { return true };
        let reach = f64::from(DOT_REACH) / f64::from(app.point_zoom().max(0.01));
        let dots = &mut app.ui.ai_gen.remove_bg.dots;
        if let Some(i) = dots.iter().position(|d| (d.x - x).hypot(d.y - y) <= reach) {
            dots.remove(i);
        } else if dots.len() < bg_removal::MAX_POINTS && x >= f64::from(canvas.x0) && x < f64::from(canvas.x1) && y >= f64::from(canvas.y0) && y < f64::from(canvas.y1) {
            dots.push(Dot { x, y, keep: !mods.alt });
        }
    }
    true
}

/// The dots over the canvas, and the pointer's shape while they are being placed.
pub fn canvas(app: &mut PhotocraftApp, ui: &mut egui::Ui, painter: &egui::Painter, xf: &ViewXform, hover: Option<egui::Pos2>) {
    if !on_canvas(app) {
        return;
    }
    for d in &app.ui.ai_gen.remove_bg.dots {
        let c = xf.to_screen(d.x as f32, d.y as f32);
        let fill = if d.keep { Color32::from_rgb(0x2f, 0xa8, 0x5a) } else { Color32::from_rgb(0xd9, 0x48, 0x3b) };
        painter.circle_filled(c, DOT_RADIUS + 1.0, Color32::from_black_alpha(170));
        painter.circle(c, DOT_RADIUS, fill, Stroke::new(1.5, Color32::WHITE));
        let sign = Stroke::new(1.6, Color32::WHITE);
        painter.line_segment([c - vec2(3.2, 0.0), c + vec2(3.2, 0.0)], sign);
        if d.keep {
            painter.line_segment([c - vec2(0.0, 3.2), c + vec2(0.0, 3.2)], sign);
        }
    }
    if placing(app) && hover.is_some_and(|p| xf.rect.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
}

/// `ai.removeBackground`'s params for the tab as it stands.
fn params(st: &RemoveBgUi) -> Value {
    let mut p = json!({"keep": st.keep.key(), "model": st.kind.model().id, "output": st.output.key()});
    if st.clean && st.output != Output::Selection {
        p["decontaminate"] = json!(true);
    }
    match st.keep {
        Keep::Describe => p["prompt"] = json!(st.phrase),
        Keep::Points => p["points"] = st.dots.iter().map(|d| json!({"x": d.x, "y": d.y, "keep": d.keep})).collect(),
        Keep::Subject | Keep::Selection => {}
    }
    p
}

/// Look at the disk for the models, now and then.
fn look(app: &mut PhotocraftApp, now: f64) {
    if app.ui.ai_gen.remove_bg.staged {
        return;
    }
    if app.ui.ai_gen.remove_bg.looked.is_none_or(|t| now - t >= LOOK_EVERY || now < t) {
        let found = photocraft_engine::ai_bg_cmds::installed(&app.session);
        let to_fetch = photocraft_engine::ai_bg_cmds::to_download(&app.session);
        let st = &mut app.ui.ai_gen.remove_bg;
        st.installed = found;
        st.to_fetch = to_fetch;
        st.looked = Some(now);
    }
}

fn has(st: &RemoveBgUi, model: &BgModel) -> bool {
    st.installed.iter().any(|(id, on)| *id == model.id && *on)
}

/// What `model` still has to download.
fn fetch_size(st: &RemoveBgUi, model: &BgModel) -> u64 {
    st.to_fetch.iter().find(|(id, _)| *id == model.id).map_or(model.size, |(_, n)| *n)
}

/// The models this choice needs and doesn't have.
fn missing(st: &RemoveBgUi) -> Vec<&'static BgModel> {
    let mut out = Vec::new();
    if st.keep.needs_target() && !has(st, &bg_removal::TARGET_MODEL) {
        out.push(&bg_removal::TARGET_MODEL);
    }
    if !has(st, st.kind.model()) {
        out.push(st.kind.model());
    }
    out
}

/// Why Remove Background can't run now, or `None`.
fn blocked(app: &PhotocraftApp) -> Option<String> {
    if app.session.active().is_none() {
        return Some(tr("Open a document first.").to_string());
    }
    if let Err(why) = photocraft_engine::ai_bg_cmds::can_cut(&app.session) {
        return Some(why);
    }
    let st = &app.ui.ai_gen.remove_bg;
    let wanted = missing(st);
    if !wanted.is_empty() {
        let names: Vec<&str> = wanted.iter().map(|m| m.name).collect();
        return Some(format!("{}: {}", tr("Not installed yet"), names.join(", ")));
    }
    match st.keep {
        Keep::Subject => {}
        Keep::Selection => {
            if !photocraft_engine::ai_cmds::inpaint_marked(&app.session) {
                return Some(tr("Select or paint roughly around what to keep.").to_string());
            }
        }
        Keep::Describe => {
            if bg_removal::object_phrase(&st.phrase).is_empty() {
                return Some(tr("Say what to keep, in a few words.").to_string());
            }
        }
        Keep::Points => {
            if !st.dots.iter().any(|d| d.keep) {
                return Some(tr("Click on what to keep.").to_string());
            }
        }
    }
    if let Some(job) = app.session.active().and_then(|d| app.session.job_on(d.doc.id)) {
        return Some(format!("{} ({})", tr("The document is busy"), job.label));
    }
    None
}

/// Open Preferences on Generative AI › Remove BG.
pub fn open_settings(app: &mut PhotocraftApp) {
    super::open_generative_ai(app, |f| {
        f.insert(PAGE_FIELD.into(), json!(PAGE));
    });
}

/// Start the cut-out.
fn start(app: &mut PhotocraftApp) {
    let st = &app.ui.ai_gen.remove_bg;
    let clicks: Vec<[f64; 2]> = if st.keep == Keep::Points { st.dots.iter().filter(|d| d.keep).map(|d| [d.x, d.y]).collect() } else { Vec::new() };
    if photocraft_engine::ai_bg_cmds::nothing_to_remove(&app.session, &clicks) {
        let alert = craft::ui::dialog::Alert::new(tr(photocraft_engine::ai_bg_cmds::NOTHING_TITLE).to_owned(), tr(photocraft_engine::ai_bg_cmds::NOTHING_HINT).to_owned());
        crate::dialogs::alerts::show(app, alert);
        return;
    }
    let p = params(&app.ui.ai_gen.remove_bg);
    match crate::jobs_ui::run(app, CMD, p) {
        // A job's dots stay: the same clicks can be run again with another model.
        Ok(_) => {}
        Err(e) => crate::notices::error(app, format!("{}: {e}", tr("Remove Background"))),
    }
}

/// A labelled dropdown row of the tab.
fn choice<T: PartialEq + Clone>(ui: &mut egui::Ui, id: &str, label: &str, value: &mut T, options: &[(T, &str)], tip: &str) {
    ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        edit_label(ui, label);
        let left = (ui.available_width() - 2.0).max(80.0);
        widgets::dropdown(ui, id, label, value, options, left);
    })
    .response
    .on_hover_text(tip);
    ui.add_space(6.0);
}

/// The model or models this choice runs, as the model row names them.
fn model_names(st: &RemoveBgUi) -> String {
    if st.keep.needs_target() { format!("{} + {}", bg_removal::TARGET_MODEL.name, st.kind.model().name) } else { st.kind.model().name.to_string() }
}

/// The row at the top, as on the other tasks: the model, and the gear to its settings. Keep and
/// Type say which model it is, so it is a name here rather than a menu.
fn model_row(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let names = model_names(&app.ui.ai_gen.remove_bg);
    let mut settings = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 24.0 - ui.spacing().item_spacing.x).max(80.0);
        let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), egui::Sense::hover());
        let field = ui.visuals().widgets.inactive;
        ui.painter().rect_filled(rect, field.corner_radius, field.weak_bg_fill);
        let inner = rect.shrink2(vec2(8.0, 0.0));
        ui.painter().with_clip_rect(inner).text(inner.left_center(), egui::Align2::LEFT_CENTER, &names, egui::TextStyle::Body.resolve(ui.style()), t.text);
        response.on_hover_text(tr("Keep and Type choose the model"));
        settings = crate::ai_widgets::icon_button(ui, "settings", 24.0, false, tr("Background removal models…")).clicked();
    });
    if settings {
        open_settings(app);
    }
}

/// What is missing, in the middle of the `room` it has, as the panel's other empty states: why,
/// and the button to the page that installs it.
fn install_prompt(app: &mut PhotocraftApp, ui: &mut egui::Ui, title: &str, hint: &str, wanted: &[&BgModel], room: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    let total: u64 = wanted.iter().map(|m| fetch_size(&app.ui.ai_gen.remove_bg, m)).sum();
    let label = format!("{} ({})\u{2026}", tr("Install"), photocraft_aisetup::human_bytes(total));
    // As tall as it was last frame, so the space above it is known before it is drawn.
    let height_id = ui.scope_id().with("ai-remove-bg-install-height");
    let height = ui.ctx().data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0);
    ui.add_space(room.map_or(16.0, |room| ((room - height) * 0.45).max(16.0)));
    let full = ui.available_rect_before_wrap();
    let width = (full.width() - 40.0).clamp(120.0, 340.0);
    let column = egui::Rect::from_min_size(egui::pos2(full.center().x - width / 2.0, full.top()), vec2(width, f32::INFINITY));
    let mut open = false;
    let block = ui.scope_builder(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
        ui.label(RichText::new(title).font(crate::theme::medium(14.0)).color(t.text));
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(hint).color(t.text_dim).size(12.0)).wrap());
        ui.add_space(18.0);
        let button = widgets::button_width(ui, &[label.as_str()]).max(150.0).min(width);
        open = widgets::primary_button(ui, &label, button).on_hover_text(tr("Opens Preferences › Generative AI › Remove BG, with each model's licence")).clicked();
    });
    let drawn = block.response.rect.height();
    if (drawn - height).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(height_id, drawn));
        ui.ctx().request_repaint();
    }
    if open {
        open_settings(app);
    }
}

/// The tab's body, under the task row. `job`: the cut-out running. `room`: the height it has to
/// fill, when it fills one.
pub(super) fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, job: Option<&photocraft_engine::jobs::JobInfo>, room: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    let busy = job.is_some();
    let top = ui.cursor().top();
    look(app, ui.input(|i| i.time));
    // Without a model that cuts an edge there is nothing to choose between yet.
    if !busy && bg_removal::MATTE_MODELS.iter().all(|m| !has(&app.ui.ai_gen.remove_bg, m)) {
        let wanted = missing(&app.ui.ai_gen.remove_bg);
        let names: Vec<&str> = wanted.iter().map(|m| m.name).collect();
        let hint = format!("{} {}. {}", tr("Install"), names.join(" + "), tr("It runs on this computer, and nothing is uploaded."));
        install_prompt(app, ui, tr("No background removal model yet."), &hint, &wanted, room);
        return;
    }
    model_row(app, ui);
    ui.add_space(6.0);
    let layer = app.session.active().and_then(|d| d.active_layer.and_then(|id| d.doc.layer(id))).map(|l| l.name.clone());
    ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        edit_label(ui, tr("Layer"));
        let name = layer.unwrap_or_else(|| tr("No layer").to_string());
        ui.add(egui::Label::new(RichText::new(&name).color(t.text)).truncate()).on_hover_text(name);
    });
    ui.add_space(6.0);
    let mut st = app.ui.ai_gen.remove_bg.clone();
    ui.add_enabled_ui(!busy, |ui| {
        let keeps: Vec<(Keep, &str)> = Keep::ALL.iter().map(|k| (*k, tr(k.label()))).collect();
        choice(ui, "ai-remove-bg-keep", tr("Keep"), &mut st.keep, &keeps, tr("What stays. Main subject needs nothing from you. A selection can be rough. Describing or clicking uses SAM 3.1 to find the object"));
        let kinds: Vec<(Kind, &str)> = Kind::ALL.iter().map(|k| (*k, tr(k.label()))).collect();
        choice(ui, "ai-remove-bg-kind", tr("Type"), &mut st.kind, &kinds, tr("Which model cuts the edge: BiRefNet for quick photo cut-outs, RMBG 3.0 for the finest hair and soft detail (non-commercial), Lucida for graphics, logos, text and glass"));
    });
    // The keep chosen has the canvas from this frame on.
    app.ui.ai_gen.remove_bg.keep = st.keep;
    app.ui.ai_gen.remove_bg.kind = st.kind;
    let mut submit = false;
    match st.keep {
        Keep::Subject => {}
        Keep::Selection => {
            super::mask_bar(app, ui, busy);
            ui.add_space(6.0);
        }
        Keep::Describe => {
            ui.add_enabled_ui(!busy, |ui| {
                let field = widgets::plain("What to keep", &mut app.ui.ai_gen.remove_bg.phrase)
                    .placeholder(tr("What to keep: \u{201c}couple\u{201d}, \u{201c}woman in red\u{201d}\u{2026}"))
                    .submits();
                submit = widgets::show_text(ui, "ai-remove-bg-phrase", 0.0, field).submitted;
            });
            ui.add_space(6.0);
        }
        Keep::Points => {
            ui.add(egui::Label::new(RichText::new(tr("Click what to keep. Alt-click what to leave out. Click a dot to take it away.")).color(t.text_dim).size(11.5)).wrap());
            let dots = app.ui.ai_gen.remove_bg.dots.len();
            let armed = placing(app);
            let (mut pick, mut clear) = (false, false);
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    if !armed {
                        pick = widgets::secondary_button(ui, tr("Click on Picture"), 120.0).on_hover_text(tr("Another tool was picked: this gives the pointer back to the dots")).clicked();
                    }
                    ui.label(RichText::new(format!("{dots} {}", tr(if dots == 1 { "click" } else { "clicks" }))).color(t.text_faint).size(11.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear = ui.add_enabled_ui(dots > 0, |ui| crate::ai_widgets::icon_button(ui, "trash", 24.0, false, tr("Take away every click"))).inner.clicked();
                    });
                });
            });
            if pick {
                arm(app);
            }
            if clear {
                app.ui.ai_gen.remove_bg.dots.clear();
            }
            ui.add_space(6.0);
        }
    }
    let mut st = app.ui.ai_gen.remove_bg.clone();
    ui.add_enabled_ui(!busy, |ui| {
        let outputs: Vec<(Output, &str)> = Output::ALL.iter().map(|o| (*o, tr(o.label()))).collect();
        choice(ui, "ai-remove-bg-output", tr("Output"), &mut st.output, &outputs, tr("Layer mask deletes nothing and can be painted on. New layer keeps the original, hidden, under a masked copy"));
        ui.add_enabled_ui(st.output != Output::Selection, |ui| {
            let mut on = st.clean && st.output != Output::Selection;
            let r = widgets::checkbox(ui, &mut on, tr("Clean edge colours"))
                .on_hover_text(tr("Takes the old background's colour out of hair and soft edges. It makes new pixels, so the result is a new layer"));
            if r.changed() {
                st.clean = on;
            }
        });
    });
    app.ui.ai_gen.remove_bg.output = st.output;
    app.ui.ai_gen.remove_bg.clean = st.clean;
    ui.add_space(8.0);

    let why = blocked(app);
    let (mut clicked, mut cancel) = (false, None);
    ui.horizontal(|ui| {
        if let Some(job) = job {
            ui.add(egui::ProgressBar::new(job.progress.clamp(0.0, 1.0)).desired_width((ui.available_width() - 84.0).max(40.0)).desired_height(6.0).fill(t.accent));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::secondary_button(ui, tr("Cancel"), 70.0).clicked() {
                    cancel = Some(job.id);
                }
            });
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = ui.add_enabled_ui(why.is_none(), |ui| widgets::primary_button(ui, tr("Remove Background"), 160.0)).inner.clicked();
        });
    });
    if let Some(id) = cancel {
        crate::jobs_ui::cancel(app, id);
    }
    ui.add_space(3.0);
    widgets::hairline(ui);
    ui.add_space(4.0);
    let wanted = missing(&app.ui.ai_gen.remove_bg);
    let line = match (job, &why) {
        (Some(job), _) => if job.message.is_empty() { job.label.clone() } else { job.message.clone() },
        (None, Some(why)) => why.clone(),
        (None, None) => tr("Runs on this computer. One undo takes it away.").to_string(),
    };
    if busy || wanted.is_empty() {
        ui.add(egui::Label::new(RichText::new(line).color(if why.is_some() && !busy { t.text_dim } else { t.text_faint }).size(11.5)).wrap());
    }
    if !busy && why.is_none() && (clicked || submit) {
        start(app);
    }
    if !busy && !wanted.is_empty() {
        let names: Vec<&str> = wanted.iter().map(|m| m.name).collect();
        let title = format!("{} {}", names.join(" + "), tr(if wanted.len() == 1 { "isn't installed yet." } else { "aren't installed yet." }));
        let left = room.map(|r| r - (ui.cursor().top() - top));
        install_prompt(app, ui, &title, tr("This choice needs it. It runs on this computer, and nothing is uploaded."), &wanted, left);
    }
}

/// The offscreen snapshot's Remove BG (`--ai-setup-demo remove-bg…`): the tab with `what` chosen
/// (`-describe`, `-click`, none), with no model (`-none`) or without SAM 3.1 (`-no-sam`), or the
/// page (`-prefs`). False for anything else.
pub fn demo(app: &mut PhotocraftApp, what: &str) -> bool {
    (app.ui.ai_gen.mode, app.ui.ai_gen.task) = (Mode::Edit, EditTask::RemoveBg);
    let here = target(app);
    let st = &mut app.ui.ai_gen.remove_bg;
    let on = |have: &dyn Fn(&BgModel) -> bool| bg_removal::models().iter().map(|m| (m.id, have(m))).collect::<Vec<_>>();
    if what != "-prefs" {
        st.installed = on(&|m| what != "-none" && !(what == "-no-sam" && m.id == bg_removal::TARGET_MODEL.id));
        st.staged = true;
    }
    match what {
        "" | "-prefs" | "-none" => {}
        "-no-sam" => st.keep = Keep::Points,
        "-describe" => (st.keep, st.phrase) = (Keep::Describe, "couple".into()),
        "-click" => {
            st.keep = Keep::Points;
            st.dots_on = here;
            st.dots = vec![Dot { x: 300.0, y: 260.0, keep: true }, Dot { x: 500.0, y: 280.0, keep: true }, Dot { x: 120.0, y: 90.0, keep: false }];
        }
        _ => return false,
    }
    if what == "-prefs" {
        open_settings(app);
    }
    true
}

// ---------- Preferences › Generative AI › Remove BG ----------

/// A link-styled label; returns its click.
fn link(ui: &mut egui::Ui, text: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let r = ui.add(egui::Label::new(RichText::new(text).color(t.accent_text).size(11.5).underline()).sense(egui::Sense::click()));
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Is this file among those the dialog lists, under its folder or the folder's older name?
fn file_listed(files: &std::collections::BTreeMap<String, Vec<String>>, folder: &str, file: &str) -> bool {
    let older = photocraft_aisetup::manifest::LEGACY_FOLDERS.iter().filter(|(now, _)| *now == folder).map(|(_, old)| *old);
    std::iter::once(folder).chain(older).any(|name| files.get(name).is_some_and(|names| names.iter().any(|n| n == file)))
}

/// Is `model` there, with the files it shares with other models?
fn listed(f: &Map<String, Value>, model: &BgModel) -> bool {
    let files = super::folders::files_of(f);
    file_listed(&files, model.folder, model.file) && model.shared.iter().all(|s| file_listed(&files, s.folder, s.file))
}

/// What installing `model` would download, by what the dialog lists.
fn listed_fetch(f: &Map<String, Value>, model: &BgModel) -> u64 {
    let files = super::folders::files_of(f);
    let own = if file_listed(&files, model.folder, model.file) { 0 } else { model.size };
    own + model.shared.iter().filter(|s| !file_listed(&files, s.folder, s.file)).map(|s| s.size).sum::<u64>()
}

/// One model's card: its name, what it is for, its licence, its size, and Installed or Install.
fn model_card(ui: &mut egui::Ui, f: &mut Map<String, Value>, m: &BgModel, width: f32, set_up: bool, action: &mut Option<String>) {
    let t = Tokens::get(ui.ctx());
    let installed = listed(f, m);
    let size = if installed { m.size } else { listed_fetch(f, m) };
    let job = f.get(INSTALL_JOB).cloned();
    let downloading = job.as_ref().and_then(|j| j.get("model")).and_then(Value::as_str).map(str::to_string);
    let mut open = None;
    egui::Frame::new().fill(t.card).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(width - 22.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.vertical(|ui| {
                ui.set_max_width((width - 240.0).max(160.0));
                ui.label(RichText::new(m.name).font(crate::theme::medium(13.0)).color(t.text));
                ui.add(egui::Label::new(RichText::new(tr(m.role)).color(t.text_dim).size(11.5)).wrap());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if link(ui, m.licence) {
                        open = Some(m.licence_url);
                    }
                    if link(ui, tr("Model page")) {
                        open = Some(m.page);
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(RichText::new(tr("Installed")).color(t.success).size(12.0));
                    let (tick, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                    crate::ai_widgets::paint_icon(ui, tick, "check", 14.0, t.success);
                } else if downloading.as_deref() == Some(m.id) {
                    if ui.add(egui::Button::new(tr("Stop")).small()).on_hover_text(tr("Stop the download: what arrived is kept, and Install goes on from there")).clicked() {
                        *action = Some(STOP_INSTALL.to_string());
                    }
                    ui.add_sized(vec2(18.0, 18.0), egui::Spinner::new().size(14.0));
                } else {
                    let tip = if set_up {
                        format!("{} {} ({})", tr("Download"), m.name, photocraft_aisetup::human_bytes(size))
                    } else {
                        tr("Set up Local AI first: it installs the engine that runs the model").to_string()
                    };
                    let free = set_up && downloading.is_none();
                    if ui.add_enabled_ui(free, |ui| widgets::secondary_button(ui, tr("Install"), 90.0)).inner.on_hover_text(tip).clicked() {
                        *action = Some(format!("{INSTALL}{}", m.id));
                    }
                }
                ui.label(RichText::new(photocraft_aisetup::human_bytes(size)).color(t.text_faint).size(11.5));
            });
        });
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
            if let Some(p) = progress {
                ui.add(egui::ProgressBar::new(p.clamp(0.0, 1.0) as f32).desired_height(4.0).desired_width(ui.available_width()));
            }
            ui.add(egui::Label::new(RichText::new(text).color(if error { t.danger } else { t.text_faint }).size(11.0)).wrap().selectable(true));
        }
    });
    if let Some(url) = open {
        f.insert(OPEN_URL.into(), json!(url));
    }
    ui.add_space(6.0);
}

/// The page: the models that cut the edge, and SAM 3.1, which finds what to keep.
pub(super) fn page(ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let wrapped = |ui: &mut egui::Ui, text: &str, color: Color32, size: f32| super::enhance::wrapped(ui, text, color, size, width);
    wrapped(ui, tr("AI Gen › Edit › Remove BG takes a layer's background away on this computer. Nothing is uploaded, and the result is a layer mask you can still paint on."), t.text_dim, 13.0);
    ui.add_space(10.0);
    let set_up = f.get(super::SETUP).and_then(|st| st.get("route")).and_then(Value::as_str).is_some_and(|r| !r.is_empty());
    let mut action = None;
    ui.label(RichText::new(tr("Edge models")).font(crate::theme::medium(12.5)).color(t.text));
    wrapped(ui, tr("These cut the edge, hair included. They find the main subject on their own. The panel's Type chooses between them."), t.text_faint, 11.5);
    ui.add_space(6.0);
    for m in &bg_removal::MATTE_MODELS {
        model_card(ui, f, m, width, set_up, &mut action);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(tr("Finding what to keep")).font(crate::theme::medium(12.5)).color(t.text));
    wrapped(ui, tr("Optional. SAM 3.1 finds an object from a few words or clicks, for pictures with more than one subject. It comes from Comfy-Org's copy of Meta's model, under Meta's SAM License, which forbids military and weapons uses among others: read it before you install."), t.text_faint, 11.5);
    ui.add_space(6.0);
    model_card(ui, f, &bg_removal::TARGET_MODEL, width, set_up, &mut action);
    if !set_up {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Local AI isn't set up yet: its setup installs the engine that runs these models.")).color(t.warning).size(11.5));
            if widgets::secondary_button(ui, tr("Set Up Local AI…"), 130.0).clicked() {
                action = Some("setup".to_string());
            }
        });
    }
    if let Some(a) = action {
        f.insert(ACTION.into(), json!(a));
    }
}

/// The page's Install and Stop. Returns whether `action` was one of them.
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
    false
}

/// Download model `id`, in the background where there are jobs.
fn install(app: &mut PhotocraftApp, f: &mut Map<String, Value>, id: &str) {
    let Some(model) = bg_removal::find(id) else { return };
    let params = json!({"model": model.id});
    let started = if app.background_jobs {
        app.session.start("ai.removeBackgroundInstall", params).map_err(|e| e.to_string())
    } else {
        app.session.execute("ai.removeBackgroundInstall", params).map(photocraft_engine::jobs::Started::Done).map_err(|e| e.to_string())
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Job(job)) => {
            f.insert(INSTALL_JOB.into(), json!({"id": job.0, "model": model.id, "text": format!("{} {}…", tr("Downloading"), model.name), "progress": 0.0}));
            f.remove(INSTALL_STATUS);
        }
        Ok(photocraft_engine::jobs::Started::Done(_)) => done(app, f, model),
        Err(e) => {
            f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": e, "error": true}));
        }
    }
}

/// `model` arrived: the folders are read again so the page shows it, and the panel looks again.
fn done(app: &mut PhotocraftApp, f: &mut Map<String, Value>, model: &BgModel) {
    super::folders::list(app, f);
    app.ui.ai_gen.remove_bg.looked = None;
    f.insert(INSTALL_STATUS.into(), json!({"model": model.id, "text": format!("{} {}", model.name, tr("is installed.")), "error": false}));
}

/// Follow the download that runs, from the dialog's frame.
pub(super) fn follow_install(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    let Some(job) = f.get(INSTALL_JOB).cloned() else { return };
    let id = job.get("id").and_then(Value::as_u64).unwrap_or(0);
    let Some(model) = job.get("model").and_then(Value::as_str).and_then(bg_removal::find) else {
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
                "done" => done(app, f, model),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tab_asks_the_engine_for_what_it_shows() {
        let mut st = RemoveBgUi::default();
        assert_eq!(params(&st), json!({"keep": "subject", "model": "birefnet", "output": "mask"}));
        st.kind = Kind::Graphic;
        st.keep = Keep::Describe;
        st.phrase = "couple".into();
        st.clean = true;
        assert_eq!(params(&st), json!({"keep": "describe", "model": "lucida", "output": "mask", "prompt": "couple", "decontaminate": true}));
        st.keep = Keep::Points;
        st.output = Output::Selection;
        st.dots = vec![Dot { x: 3.0, y: 4.0, keep: true }, Dot { x: 5.0, y: 6.0, keep: false }];
        // A selection has no pixels to clean.
        assert_eq!(
            params(&st),
            json!({"keep": "points", "model": "lucida", "output": "selection", "points": [{"x": 3.0, "y": 4.0, "keep": true}, {"x": 5.0, "y": 6.0, "keep": false}]})
        );
    }

    #[test]
    fn only_finding_an_object_needs_sam() {
        let mut st = RemoveBgUi { installed: vec![("birefnet", true), ("lucida", false), ("sam-3-1", false)], ..Default::default() };
        assert!(missing(&st).is_empty());
        st.kind = Kind::Graphic;
        assert_eq!(missing(&st).iter().map(|m| m.id).collect::<Vec<_>>(), ["lucida"]);
        st.kind = Kind::Photo;
        st.keep = Keep::Points;
        assert_eq!(missing(&st).iter().map(|m| m.id).collect::<Vec<_>>(), ["sam-3-1"]);
    }

    #[test]
    fn the_model_row_names_what_the_choice_runs() {
        let mut st = RemoveBgUi::default();
        assert_eq!(model_names(&st), "BiRefNet General");
        st.kind = Kind::Graphic;
        assert_eq!(model_names(&st), "Lucida");
        st.keep = Keep::Points;
        assert_eq!(model_names(&st), "SAM 3.1 + Lucida");
        st.kind = Kind::Fine;
        assert_eq!(model_names(&st), "SAM 3.1 + RMBG 3.0");
    }

    #[test]
    fn the_non_commercial_model_is_never_the_default() {
        assert_eq!(RemoveBgUi::default().kind.model().id, bg_removal::DEFAULT_MATTE);
        assert_eq!(params(&RemoveBgUi { kind: Kind::Fine, ..Default::default() })["model"], "rmbg-3");
    }

    #[test]
    fn the_state_saves_without_what_belongs_to_this_session() {
        let st = RemoveBgUi { keep: Keep::Describe, phrase: "dog".into(), dots: vec![Dot { x: 1.0, y: 1.0, keep: true }], ..Default::default() };
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!((v["keep"].as_str(), v["phrase"].as_str()), (Some("describe"), Some("dog")));
        assert!(v.get("dots").is_none());
        let back: RemoveBgUi = serde_json::from_value(json!({"keep": "points"})).unwrap();
        assert_eq!((back.keep, back.kind, back.output), (Keep::Points, Kind::Photo, Output::Mask));
    }
}
