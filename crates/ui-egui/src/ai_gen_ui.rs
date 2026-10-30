//! Generative AI in the shell: the Window › AI Gen panel (a tab of the Properties dock group)
//! and the model settings in Edit › Preferences › Generative AI.
//!
//! The panel picks a model, takes a prompt and runs the engine's `ai.generate` as a background
//! job: the result lands on a new layer over the selection. Its two tabs say what the user is
//! there to do ([`Mode`]): **Create** a new image from the prompt alone, or **Edit** a picture
//! that is there. Edit has a task row ([`EditTask`]): **Change** the picture, or the selected
//! part of it, as the prompt says, or **Restyle** it (redraw it, keeping its composition). Which
//! of the engine's techniques that is (`ai.generate`'s `mode`: txt2img, inpaint, img2img,
//! imgEdit) follows from the task and from what the chosen model can do ([`technique`]); the
//! settings few people change sit in a section that opens (Advanced). The settings page edits
//! `integrations.ai` (the ComfyUI server, local and cloud models and their rendering parameters)
//! in the Preferences dialog's draft, like every other section; API keys are saved apart from
//! the preferences through [`crate::Services::save_ai_keys`].
//!
//! The strings here go through [`tr`] rather than `tl!`: they fall back to English until the
//! catalogs gain them (the `tl!` scan requires every complete language to have each literal).

use egui::{RichText, vec2};
use photocraft_aigen::{Family, Model, Provider, Settings};
use photocraft_doc::{GenerationMode, GenerationQuality};
use photocraft_engine::ai_cmds::SourceRef;
use photocraft_engine::jobs::JobInfo;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// Window › AI Gen.
pub const PANEL: &str = "window.panel.aiGen";
/// Put the prompt a generated layer was made with into the prompt box: `{"layer": id?}` (the
/// active layer without it). What the panel's Load Prompt button does.
pub const LOAD_PROMPT: &str = "window.panel.aiGen.loadPrompt";
/// The tab's position in the Properties dock group (Properties | Adjustments | AI Gen).
pub const TAB: usize = 2;
/// The engine command the panel runs.
const GENERATE: &str = "ai.generate";

/// What the user is there to do: the panel's two tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// A new image from the prompt alone.
    #[default]
    #[serde(alias = "txt2img")]
    Create,
    /// A picture that is there is changed ([`EditTask`]). The aliases are the tabs an earlier
    /// build had, one per technique: a state saved then still loads.
    #[serde(alias = "inpaint", alias = "img2img", alias = "imgEdit")]
    Edit,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Create, Mode::Edit];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Create => "Create",
            Mode::Edit => "Edit",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Mode::Create => "Generate a new image from the prompt, into the selection or the whole canvas",
            Mode::Edit => "Change a picture that is there: repaint a selection, change it as you say, or restyle it. The result is a new layer",
        }
    }
}

/// What Edit does with the picture: the row under the tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditTask {
    /// The picture, or with a selection the selected part of it, is changed as the prompt says.
    #[default]
    Change,
    /// The picture is redrawn following the prompt, as closely as the strength says.
    Restyle,
}

impl EditTask {
    pub const ALL: [EditTask; 2] = [EditTask::Change, EditTask::Restyle];

    pub fn label(self) -> &'static str {
        match self {
            EditTask::Change => "Change",
            EditTask::Restyle => "Restyle",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            EditTask::Change => {
                "Change the picture as the prompt says. With a selection (any selection tool, or the mask tools under the prompt) only that part changes, and the rest stays exactly as it was"
            }
            EditTask::Restyle => {
                "Redraw the picture following the prompt, keeping its composition: a layer of this document, or another open document. Strength is how far it may depart"
            }
        }
    }
}

/// Create's two ways, under the tabs: the prompt alone, or the scene laid out in boxes
/// ([`regions`]), which only models trained on boxes take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CreateTask {
    #[default]
    Prompt,
    Regions,
}

impl CreateTask {
    pub const ALL: [CreateTask; 2] = [CreateTask::Prompt, CreateTask::Regions];

    pub fn label(self) -> &'static str {
        match self {
            CreateTask::Prompt => "Prompt",
            CreateTask::Regions => "Regions",
        }
    }
}

/// How a change is asked of a model that can do it both ways (Advanced › Method). A model that
/// can do only one is asked that way, whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    /// The model is shown the picture and told what to change (`ai.generate`'s `imgEdit`).
    #[default]
    Instruct,
    /// The selection is the model's mask, and it paints what the prompt describes inside it
    /// (`ai.generate`'s `inpaint`).
    Fill,
}

/// The panel's state (in `UiState`, so `ui.inspect` reports it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiGenUi {
    /// The tab in front.
    pub mode: Mode,
    /// Edit: what to do with the picture.
    pub task: EditTask,
    /// Create: the prompt alone, or the scene laid out in regions (a model that takes them).
    pub create_task: CreateTask,
    /// Create › Regions: the region selected in the list and on the canvas.
    pub region: Option<u32>,
    #[serde(skip)]
    regions: regions::RegionsUi,
    /// Change: how a model that both edits and inpaints is asked.
    pub method: Method,
    /// The model chosen for Create and for Restyle, which list the models that generate (empty:
    /// the first enabled one).
    pub model: String,
    /// Change, filling: how far the result may depart from what the layer holds, 0..=1.
    pub strength: f64,
    /// Change: how much of the picture around the selection the model is shown, as a fraction of
    /// the selection's longer edge.
    pub context: f64,
    /// Change: the mask is being painted with the panel's brush (see [`mask_bar`]); while it is,
    /// the Brush adds to the mask and the Eraser takes away, whatever the colours are.
    pub mask_brush: bool,
    /// Restyle: the picture to redraw (`None`: the active layer when it has pixels, else the
    /// first that is offered).
    pub img_source: Option<SourceRef>,
    /// Restyle: how far the result may depart from the picture, 0..=1.
    pub img_strength: f64,
    /// The model chosen for Change, which lists the models that edit or inpaint.
    pub edit_model: String,
    /// Change, by instruction: the picture to change (`None`: the active layer when it has
    /// pixels, else the first that is offered).
    pub edit_source: Option<SourceRef>,
    /// The reference chip whose name and picture are open under the chips (the references are
    /// the document's, see [`refs`]).
    #[serde(skip)]
    editing_ref: Option<usize>,
    /// Change, by instruction: how much of a local model's schedule to run.
    pub edit_quality: GenerationQuality,
    pub prompt: String,
    /// Ask for a transparent background (models that support it).
    pub alpha: bool,
    /// The seed to render with (Advanced). `None`: the model's own setting, a new one each time
    /// unless it has a fixed seed.
    pub seed: Option<u64>,
    /// The Advanced section is open.
    pub advanced: bool,
    /// The seed the last generation was rendered with, to keep for the next (Advanced).
    pub last_seed: Option<u64>,
    /// What the last generation cost in US dollars, when the provider reported it (OpenAI).
    pub last_cost: Option<f64>,
    /// The floating panel is open. Only the themes that don't dock Properties (Studio, Classic)
    /// use it; in the Pro themes the panel is a tab of that dock group.
    pub floating: bool,
    /// The model files the ComfyUI in the settings listed when last asked (`ai.comfyModels`,
    /// without starting it): which local models can run. `None`: it has not answered in this
    /// session. And that look, running in the background.
    #[serde(skip)]
    server_files: Option<std::collections::BTreeMap<String, Vec<String>>>,
    #[serde(skip)]
    server_job: Option<u64>,
    /// When the server was last asked (UI time; never: −∞), the setup endings seen then, and
    /// whether the app had the focus last frame.
    #[serde(skip)]
    server_asked: f64,
    #[serde(skip)]
    server_seq: u64,
    #[serde(skip)]
    focused: bool,
}

impl Default for AiGenUi {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            task: EditTask::default(),
            create_task: CreateTask::default(),
            region: None,
            regions: regions::RegionsUi::default(),
            method: Method::default(),
            model: String::new(),
            strength: 1.0,
            context: photocraft_engine::ai_cmds::DEFAULT_CONTEXT,
            mask_brush: false,
            img_source: None,
            img_strength: photocraft_engine::ai_cmds::DEFAULT_IMG2IMG_STRENGTH,
            edit_model: String::new(),
            edit_source: None,
            editing_ref: None,
            edit_quality: GenerationQuality::default(),
            prompt: String::new(),
            alpha: false,
            seed: None,
            advanced: false,
            last_seed: None,
            last_cost: None,
            floating: false,
            server_files: None,
            server_job: None,
            server_asked: f64::NEG_INFINITY,
            server_seq: 0,
            focused: true,
        }
    }
}

impl AiGenUi {
    /// Is the task in front Change?
    fn changing(&self) -> bool {
        self.mode == Mode::Edit && self.task == EditTask::Change
    }

    /// The model chosen for the task in front. Change has its own: it lists other models.
    fn chosen(&self) -> &str {
        if self.changing() { &self.edit_model } else { &self.model }
    }

    fn chosen_mut(&mut self) -> &mut String {
        if self.changing() { &mut self.edit_model } else { &mut self.model }
    }

    fn strength(&self) -> f64 {
        if self.strength.is_finite() { self.strength.clamp(0.0, 1.0) } else { 1.0 }
    }

    fn img_strength(&self) -> f64 {
        if self.img_strength.is_finite() { self.img_strength.clamp(0.0, 1.0) } else { photocraft_engine::ai_cmds::DEFAULT_IMG2IMG_STRENGTH }
    }

    fn context(&self) -> f64 {
        if self.context.is_finite() { self.context.clamp(0.0, 4.0) } else { photocraft_engine::ai_cmds::DEFAULT_CONTEXT }
    }

    /// The seed to ask for, within what `ai.generate` takes (48 bits).
    fn seed(&self) -> Option<u64> {
        self.seed.map(|s| s & SEED_MASK)
    }
}

/// The seeds `ai.generate` renders with: 48 bits.
const SEED_MASK: u64 = 0xFFFF_FFFF_FFFF;

/// What `ai.generate` is asked for (its `mode`) with `model` chosen. The tab and the task say
/// what the user is doing; for a change, the model says how it can be done: an edit model is
/// shown the picture and told what to change, a model made for inpainting is given the selection
/// as its mask, and one that does both follows [`AiGenUi::method`]. With no model to ask
/// (`None`: none is set up for the task) a change is taken for an instruction.
fn technique(ui: &AiGenUi, model: Option<&Model>) -> GenerationMode {
    match (ui.mode, ui.task) {
        (Mode::Create, _) => GenerationMode::Txt2img,
        (Mode::Edit, EditTask::Restyle) => GenerationMode::Img2img,
        (Mode::Edit, EditTask::Change) => {
            let fill = match model.map_or((true, false), |m| (m.can_edit(), m.can_inpaint())) {
                (true, true) => ui.method == Method::Fill,
                (edits, _) => !edits,
            };
            if fill { GenerationMode::Inpaint } else { GenerationMode::ImgEdit }
        }
    }
}

/// Can `model` change a picture both ways, so that [`AiGenUi::method`] is a choice?
fn both_ways(model: &Model) -> bool {
    model.can_edit() && model.can_inpaint()
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
    if id == LOAD_PROMPT {
        let layer = match params.get("layer") {
            Some(v) => v.as_u64().ok_or_else(|| "`layer` must be a layer id".to_string()),
            None => active_layer(app).ok_or_else(|| "no active layer".to_string()),
        };
        return Some(layer.map(|layer| json!({"loaded": load_prompt(app, layer)})));
    }
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
        .id(egui::Id::unique("ai-gen"))
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

/// Is `model` offered for a change (`changing`: the models that edit or inpaint), or for Create
/// and Restyle (those that generate from a prompt: all but the dedicated inpainting and edit
/// models)?
fn fits(model: &Model, changing: bool) -> bool {
    if changing { model.can_edit() || model.can_inpaint() } else { model.can_generate() }
}

/// The models offered for the task in front: the ticked ones of the kinds the settings show that
/// can run now ([`usable`]), cloud models first, each kind in the order the settings give it
/// ([`Settings::listed`]).
fn enabled_models(app: &PhotocraftApp) -> Vec<Model> {
    let changing = app.ui.ai_gen.changing();
    app.session.prefs().integrations.ai.listed().into_iter().filter(|m| fits(m, changing) && usable(app, m)).cloned().collect()
}

/// Where local models stand for the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Local {
    /// The Local AI setup is installing them now.
    Installing,
    /// Nothing says a ComfyUI is there: local AI is not set up, and none has answered.
    NotSetUp,
    Ready,
}

fn local_state(app: &PhotocraftApp) -> Local {
    if crate::ai_setup_ui::running(app) {
        Local::Installing
    } else if crate::ai_setup_ui::set_up(app) || app.ui.ai_gen.server_files.is_some() {
        Local::Ready
    } else {
        Local::NotSetUp
    }
}

/// Can `model` run now? A cloud model when its API key is there. A local one when local AI is
/// ready (not being installed; set up, or a ComfyUI answered) and its file is among the files
/// known to be there: the server's, once it has said, else those of the engine this app
/// installed (on disk, so known while it isn't running).
fn usable(app: &PhotocraftApp, model: &Model) -> bool {
    if app.session.ai.ready(model).is_err() {
        return false;
    }
    if !model.provider.is_local() {
        return true;
    }
    if local_state(app) != Local::Ready {
        return false;
    }
    match &app.ui.ai_gen.server_files {
        Some(files) => photocraft_aigen::detect::on_server(files, model),
        None => crate::ai_setup_ui::installed_files(app).is_none_or(|files| photocraft_aigen::detect::on_server(&files, model)),
    }
}

/// The server is asked again at most this often when the app gets the focus back (seconds).
const LOOK_AGAIN: f64 = 60.0;

/// Ask the ComfyUI in the settings which model files it has, without starting it: when the
/// panel is first shown, after a Local AI setup has ended, and when the app gets the focus back
/// (at most once a minute). In the background where the app has jobs; how it ends is
/// [`on_job_event`]'s, quietly (a ComfyUI that isn't running is not an error to report).
fn look_at_server(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    let regained = focused && !app.ui.ai_gen.focused;
    app.ui.ai_gen.focused = focused;
    let seq = app.session.ai_setup.seq();
    let s = &app.ui.ai_gen;
    let due = s.server_asked == f64::NEG_INFINITY || seq != s.server_seq || (regained && now - s.server_asked >= LOOK_AGAIN);
    if !due || s.server_job.is_some() || app.session.ai.transport.is_none() || crate::ai_setup_ui::running(app) {
        return;
    }
    (app.ui.ai_gen.server_asked, app.ui.ai_gen.server_seq) = (now, seq);
    let p = json!({"start": false});
    let started = if app.background_jobs {
        app.session.start("ai.comfyModels", p)
    } else {
        app.session.execute("ai.comfyModels", p).map(photocraft_engine::jobs::Started::Done)
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Done(found)) => server_answered(app, &found),
        Ok(photocraft_engine::jobs::Started::Job(id)) => app.ui.ai_gen.server_job = Some(id.0),
        // Not running, or not there: what was learnt before stays (its files are still there).
        Err(_) => {}
    }
}

fn server_answered(app: &mut PhotocraftApp, found: &Value) {
    if let Some(files) = found.get("folders").and_then(|f| serde_json::from_value(f.clone()).ok()) {
        app.ui.ai_gen.server_files = Some(files);
    }
}

/// The end of a job: the panel's own look at the server is taken here, without a notice when it
/// failed. Returns whether it was that.
pub fn on_job_event(app: &mut PhotocraftApp, e: &photocraft_engine::jobs::JobEvent) -> bool {
    // A Connect or Scan the Preferences dialog follows: the dialog says how it ended, on the page
    // (where a server that isn't running is not an error when nobody asked to connect).
    let followed = |d: &crate::state::Dialog| d.fields.get(JOB).and_then(|j| j.get("id")).and_then(Value::as_u64) == Some(e.id.0);
    if app.ui.dialogs.iter().any(|d| crate::prefs_ui::is_preferences(&d.fields) && followed(d)) {
        return true;
    }
    if app.ui.ai_gen.server_job != Some(e.id.0) {
        return false;
    }
    app.ui.ai_gen.server_job = None;
    if let photocraft_engine::jobs::JobOutcome::Done(found) = &e.outcome {
        server_answered(app, found);
    }
    true
}

/// The headings of the panel's model list, each with the index of the model it stands above:
/// the cloud models, then the local ones (the order of [`Settings::listed`]). A kind with no
/// model offered has no heading.
fn model_headings(models: &[Model]) -> Vec<(usize, &'static str)> {
    let first = |local: bool| models.iter().position(|m| m.provider.is_local() == local);
    [(first(false), "Cloud API"), (first(true), "Local Models")].into_iter().filter_map(|(at, title)| Some((at?, tr_static(title)))).collect()
}

/// [`tr`] for a literal: the translation, or the literal itself.
fn tr_static(s: &'static str) -> &'static str {
    crate::i18n::t(s)
}

/// The model the panel would run: the one chosen for the task in front while it is still
/// offered, else [`fallback`].
fn current_model(app: &PhotocraftApp) -> Option<&Model> {
    let ui = &app.ui.ai_gen;
    let offered: Vec<&Model> = app.session.prefs().integrations.ai.listed().into_iter().filter(|m| fits(m, ui.changing()) && usable(app, m)).collect();
    offered.iter().copied().find(|m| m.id == ui.chosen()).or_else(|| fallback(offered, ui.changing()))
}

/// The model offered for the task in front when none chosen is: for Change Qwen Image 2.1, the
/// recommended edit model ([`photocraft_aigen::preferred_edit`]), else the first offered.
fn fallback<'a>(offered: impl IntoIterator<Item = &'a Model>, changing: bool) -> Option<&'a Model> {
    let mut offered = offered.into_iter();
    if changing { photocraft_aigen::preferred_edit(offered) } else { offered.next() }
}

/// The tabs across the top of the panel: equal shares of its width.
fn mode_tabs(ui: &mut egui::Ui, mode: &mut Mode, enabled: bool) {
    let t = Tokens::get(ui.ctx());
    let gap = 2.0;
    let count = Mode::ALL.len() as f32;
    let width = ((ui.available_width() - gap * (count - 1.0)) / count).max(40.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for m in Mode::ALL {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
            let on = *mode == m;
            if on {
                widgets::surface(ui, rect, t.hover, true);
                if !t.bevel {
                    ui.painter().rect_stroke(rect, t.radius_sm, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
                }
            } else if enabled && resp.hovered() {
                ui.painter().rect_filled(rect, t.radius_sm, t.hover.gamma_multiply(0.6));
            }
            let color = if on { t.text } else { t.text_dim };
            let galley = ui.painter().layout_no_wrap(tr(m.label()).to_owned(), crate::theme::medium(11.5), color);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
            if resp.on_hover_text(tr(m.tip())).clicked() {
                *mode = m;
            }
        }
    });
}

/// Edit's tasks, under the tabs: plain names, the one in front underlined, so the row reads as
/// part of the tab above it and not as a second set of tabs.
fn task_row(ui: &mut egui::Ui, task: &mut EditTask, enabled: bool) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for k in EditTask::ALL {
            let on = *task == k;
            let galley = ui.painter().layout_no_wrap(tr(k.label()).to_owned(), crate::theme::medium(11.5), t.text);
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 24.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
            let color = if on || (enabled && resp.hovered()) { t.text } else { t.text_dim };
            ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
            if on {
                let y = rect.bottom() - 1.0;
                ui.painter().line_segment([egui::pos2(rect.left() + 6.0, y), egui::pos2(rect.right() - 6.0, y)], egui::Stroke::new(2.0, t.accent));
            }
            if resp.on_hover_text(tr(k.tip())).clicked() {
                *task = k;
            }
        }
    });
}

/// Create's ways, under the tabs, as Edit's tasks are drawn. Regions is off for a model that
/// can't place them, and says which can.
fn create_row(ui: &mut egui::Ui, task: &mut CreateTask, enabled: bool, model: Option<&Model>, able: &[String]) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut picked = false;
    let regions_ok = model.is_some_and(|m| m.layout().is_some());
    // A model that can't place regions shows Prompt, keeping the choice for one that can.
    let showing_regions = *task == CreateTask::Regions && regions_ok;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for k in CreateTask::ALL {
            let allowed = enabled && (k == CreateTask::Prompt || regions_ok);
            let on = (k == CreateTask::Regions) == showing_regions;
            let galley = ui.painter().layout_no_wrap(tr(k.label()).to_owned(), crate::theme::medium(11.5), t.text);
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 24.0), if allowed { egui::Sense::click() } else { egui::Sense::hover() });
            let color = if !allowed && k == CreateTask::Regions {
                t.text_faint
            } else if on || (allowed && resp.hovered()) {
                t.text
            } else {
                t.text_dim
            };
            ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
            if on {
                let y = rect.bottom() - 1.0;
                ui.painter().line_segment([egui::pos2(rect.left() + 6.0, y), egui::pos2(rect.right() - 6.0, y)], egui::Stroke::new(2.0, t.accent));
            }
            let tip = match (k, allowed) {
                (CreateTask::Prompt, _) => tr("Generate the whole picture from one prompt").to_string(),
                (CreateTask::Regions, true) => tr("Lay the scene out in boxes: what goes where, and what is in front").to_string(),
                (CreateTask::Regions, false) => {
                    let name = model.map_or("This model", |m| m.name.as_str());
                    let which = if able.is_empty() { "MOSS Image, Ideogram 4, FLUX.3 Klein, FLUX 3 Image or FIBO 2".to_string() } else { able.join(", ") };
                    format!("{name} {} {which}", tr("can't place regions. Models that can:"))
                }
            };
            if resp.on_hover_text(tip).clicked() && allowed {
                picked |= k == CreateTask::Regions;
                *task = k;
            }
        }
    });
    picked
}

/// The running generation, if any.
fn running(app: &PhotocraftApp) -> Option<JobInfo> {
    app.session.jobs().into_iter().find(|j| j.command == GENERATE)
}

/// Open Preferences on Generative AI › Manage Models with `model` selected for editing.
pub fn open_settings(app: &mut PhotocraftApp, model: &str) -> u64 {
    let models = &app.session.prefs().integrations.ai.models;
    let index = models.iter().position(|m| m.id == model).unwrap_or(0);
    let id = crate::prefs_ui::open_preferences(app, photocraft_engine::prefs::GENERATIVE_AI.0);
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert(SELECTED.into(), json!(index));
        d.fields.insert(PAGE.into(), json!("models"));
        d.fields.insert(FILTER.into(), json!(Filter::All.key()));
    }
    id
}

/// Open Preferences on Generative AI › API Keys: where the Local AI wizard's "Cloud models
/// only" leads.
pub fn open_api_keys(app: &mut PhotocraftApp) -> u64 {
    let id = crate::prefs_ui::open_preferences(app, photocraft_engine::prefs::GENERATIVE_AI.0);
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert(PAGE.into(), json!("keys"));
    }
    id
}

/// The line under Generate that leads to the Local AI wizard: the setup's progress while one
/// runs, else an offer until local AI is set up. `None` where there is nothing to set up (the
/// browser), or nothing left to.
fn setup_line(app: &PhotocraftApp) -> Option<String> {
    if let Some((_, line)) = crate::ai_setup_ui::progress_line(app) {
        return Some(format!("{line}. {}", tr("Local models appear here when it has finished.")));
    }
    (crate::ai_setup_ui::available(app) && !crate::ai_setup_ui::set_up(app)).then(|| tr("Local models need a one-time setup: Set Up Local AI…").to_string())
}

/// Where the panel's empty state leads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lead {
    Progress,
    SetUp,
    Keys,
    Manage,
    Again,
}

/// The panel when no model can run for the task in front: why, and the way to one. While the
/// Local AI setup installs, that is what it says, with its progress. Centred in the `room` left
/// below the tabs (when the panel fills a height), with margins either side, and its buttons of
/// one width: side by side where they fit, else one above the other.
fn nothing_ready(app: &mut PhotocraftApp, ui: &mut egui::Ui, changing: bool, room: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    let keys = Provider::ALL.into_iter().any(|p| !p.is_local() && !app.session.ai.key(p).is_empty());
    let local = local_state(app);
    let (title, hint) = match local {
        Local::Installing => {
            let progress = crate::ai_setup_ui::progress_line(app).map(|(_, line)| format!("{line}. ")).unwrap_or_default();
            let after = if keys {
                tr("Its models appear here when it has finished; none of the cloud models with a key does this task.")
            } else {
                tr("Its models appear here when it has finished. You can keep working meanwhile, or add an API key to use a cloud model now.")
            };
            (tr("Local AI is being set up.").to_string(), format!("{progress}{after}"))
        }
        Local::NotSetUp if !keys => (
            tr("No AI model is ready yet.").to_string(),
            tr("Set up local AI to run models on this PC, or add an API key to use cloud models.").to_string(),
        ),
        _ if changing => (
            tr("No model that changes a picture is ready.").to_string(),
            tr("A change needs a model that follows an instruction (Qwen Image 2.1, Ming Image, Qwen Image Edit, FLUX.2, or a cloud model with an API key) or one made for inpainting (FLUX.1 Fill, or Qwen Image with the InstantX ControlNet). Scan Server in the settings adds the ones your ComfyUI has.").to_string(),
        ),
        _ => (
            tr("No AI model is ready.").to_string(),
            tr("Local models need their files on your ComfyUI; cloud models need an API key. Tick the ones to offer here in Manage Models.").to_string(),
        ),
    };
    // The ways on, the first the one to take (drawn as the primary button where it is).
    let setup = crate::ai_setup_ui::available(app);
    let mut leads: Vec<(Lead, bool)> = match local {
        Local::Installing => [(Lead::Progress, true)].into_iter().chain((!keys).then_some((Lead::Keys, false))).collect(),
        Local::NotSetUp if !keys => setup.then_some((Lead::SetUp, true)).into_iter().chain([(Lead::Keys, !setup)]).collect(),
        _ => [(Lead::Manage, false)].into_iter().chain((setup && local == Local::NotSetUp).then_some((Lead::SetUp, false))).collect(),
    };
    // Last: the wizard from its first screen, whatever the last setup chose.
    if setup && local != Local::Installing {
        leads.push((Lead::Again, false));
    }
    let label = |lead: Lead| match lead {
        Lead::Progress => tr("Show Progress"),
        Lead::SetUp => tr("Set Up Local AI…"),
        Lead::Keys => tr("Add an API Key…"),
        Lead::Manage => tr("Manage Models…"),
        Lead::Again => tr("Run Setup Wizard Again…"),
    };
    // Centred: as tall as it was last frame, so the space above it is known before it is drawn.
    let height_id = ui.scope_id().with("ai-gen-nothing-ready-height");
    let height = ui.ctx().data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0);
    let above = room.map_or(16.0, |room| ((room - height) * 0.45).max(16.0));
    ui.add_space(above);
    let full = ui.available_rect_before_wrap();
    let width = (full.width() - 2.0 * NOTHING_READY_MARGIN).clamp(120.0, NOTHING_READY_WIDTH);
    let column = egui::Rect::from_min_size(egui::pos2(full.center().x - width / 2.0, full.top()), vec2(width, f32::INFINITY));
    let gap = 10.0;
    let mut clicked = None;
    let block = ui.scope_builder(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
        ui.label(RichText::new(title).font(crate::theme::medium(14.0)).color(t.text));
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(hint).color(t.text_dim).size(12.0)).wrap());
        ui.add_space(18.0);
        let labels: Vec<&str> = leads.iter().map(|(lead, _)| label(*lead)).collect();
        let button = widgets::button_width(ui, &labels).max(150.0).min(width);
        let row = leads.len() > 1 && button * leads.len() as f32 + gap * (leads.len() - 1) as f32 <= width;
        let mut draw = |ui: &mut egui::Ui, lead: Lead, primary: bool| {
            let r = if primary { widgets::primary_button(ui, label(lead), button) } else { widgets::secondary_button(ui, label(lead), button) };
            if r.clicked() {
                clicked = Some(lead);
            }
        };
        if row {
            let size = vec2(button * leads.len() as f32 + gap * (leads.len() - 1) as f32, 0.0);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (lead, primary) in &leads {
                    draw(ui, *lead, *primary);
                }
            });
        } else {
            ui.spacing_mut().item_spacing.y = gap;
            for (lead, primary) in &leads {
                draw(ui, *lead, *primary);
            }
        }
    });
    let drawn = block.response.rect.height();
    if (drawn - height).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(height_id, drawn));
        ui.ctx().request_repaint();
    }
    match clicked {
        Some(Lead::Progress) => crate::ai_setup_ui::show_progress(app),
        Some(Lead::SetUp) => crate::ai_setup_ui::open(app),
        Some(Lead::Again) => crate::ai_setup_ui::restart(app),
        Some(Lead::Keys) => {
            open_api_keys(app);
        }
        Some(Lead::Manage) => {
            open_settings(app, "");
        }
        None => {}
    }
}

/// The empty state's widest text and buttons, and the least room either side of it.
const NOTHING_READY_WIDTH: f32 = 340.0;
const NOTHING_READY_MARGIN: f32 = 20.0;

/// Start a generation with the panel's model, prompt and options.
pub fn generate(app: &mut PhotocraftApp) -> Result<Value, String> {
    let Some(model) = current_model(app).cloned() else {
        return Err(tr("No AI model is set up for this.").to_string());
    };
    let ui = &app.ui.ai_gen;
    let kind = technique(ui, Some(&model));
    let mut params = json!({"model": model.id, "prompt": ui.prompt, "mode": kind.key()});
    match kind {
        GenerationMode::Txt2img => {
            params["alpha"] = json!(ui.alpha);
            // The document's regions, laid out for the model.
            if regions::tab_open(app) {
                params["regions"] = json!(true);
            }
        }
        GenerationMode::Inpaint => {
            params["strength"] = json!(ui.strength());
            params["context"] = json!(ui.context());
        }
        GenerationMode::Img2img => {
            params["strength"] = json!(ui.img_strength());
            // A source that is gone (its layer deleted, its document closed) is not asked for: the
            // engine then takes the one the panel shows instead.
            if let Some(source) = img_source(app) {
                params["source"] = json!(source);
            }
        }
        GenerationMode::ImgEdit => {
            params["quality"] = json!(ui.edit_quality.key());
            params["alpha"] = json!(ui.alpha);
            params["context"] = json!(ui.context());
            // The references are the document's own: `ai.generate` sends them when none are named.
            if let Some(source) = edit_source(app) {
                params["source"] = json!(source);
            }
        }
    }
    if let Some(seed) = ui.seed() {
        params["seed"] = json!(seed);
    }
    let result = crate::jobs_ui::run(app, GENERATE, params)?;
    // Inline (no background jobs): the result is here already. A job reports through `finished`.
    if result.get("pending").is_none() {
        finished(app, &result);
    }
    Ok(result)
}

/// A generation ended with `result` (`ai.generate`'s): remember what it cost, and its seed.
pub fn finished(app: &mut PhotocraftApp, result: &Value) {
    app.ui.ai_gen.last_cost = result.get("cost").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0);
    app.ui.ai_gen.last_seed = result.get("seed").and_then(Value::as_u64).map(|s| s & SEED_MASK);
    // A local model rendered: local AI works, and the offer to set it up has nothing to add.
    let model = result.get("model").and_then(Value::as_str).unwrap_or("");
    if app.session.prefs().integrations.ai.model(model).is_some_and(|m| !model.is_empty() && m.provider.is_local()) {
        app.ui.ai_setup.local_worked = true;
    }
}

/// The active layer of the document in front.
fn active_layer(app: &PhotocraftApp) -> Option<u64> {
    app.session.active().and_then(|d| d.active_layer).map(|id| id.0)
}

/// The prompt `layer` was generated with (`Layer::generation`), when it has one.
fn layer_prompt(app: &PhotocraftApp, layer: u64) -> Option<&str> {
    let d = app.session.active()?;
    let g = d.doc.layer(photocraft_doc::LayerId(layer))?.generation.as_ref()?;
    Some(g.prompt.as_str()).filter(|p| !p.trim().is_empty())
}

/// Put the prompt `layer` was generated with into the prompt box, in place of what is there.
/// Only the text: the tab, the task, the model and the other settings stay as they are. `false`,
/// and nothing changes, for a layer that was not generated (or has no prompt) and while a
/// generation runs (the panel shows that one).
pub fn load_prompt(app: &mut PhotocraftApp, layer: u64) -> bool {
    if running(app).is_some() {
        return false;
    }
    let Some(prompt) = layer_prompt(app, layer).map(str::to_owned) else { return false };
    app.ui.ai_gen.prompt = prompt;
    // A layer laid out in regions brings them back, and the Regions tab with them.
    let laid_out = app
        .session
        .active()
        .and_then(|d| d.doc.layer(photocraft_doc::LayerId(layer)))
        .and_then(|l| l.generation.as_ref())
        .map(|g| g.regions.clone())
        .filter(|r| !r.is_empty());
    if let Some(laid_out) = laid_out {
        regions::restore(app, &laid_out);
    }
    true
}

/// The area a generation would fill, in pixels, and whether a selection (or a mask being
/// painted) scopes it: the selection's bounds, else the canvas. For a change: the selection's
/// bounds with the context around them, which is what the model renders.
fn target_size(app: &PhotocraftApp) -> Option<(u32, u32, bool)> {
    let d = app.session.active()?;
    let ui = &app.ui.ai_gen;
    match technique(ui, current_model(app)) {
        GenerationMode::Img2img => {
            // Where the redrawn picture lands: the selection, the layer's pixels, or the canvas.
            let plan = photocraft_engine::ai_cmds::img2img_plan(&app.session, img_source(app), None).ok()?;
            Some((plan.area.width(), plan.area.height(), d.doc.selection.is_some()))
        }
        GenerationMode::ImgEdit => {
            // What the model is sent: the selection with its context, the layer's pixels, or the canvas.
            let plan = photocraft_engine::ai_cmds::edit_plan(&app.session, edit_source(app), ui.context(), None).ok()?;
            Some((plan.area.width(), plan.area.height(), photocraft_engine::ai_cmds::inpaint_marked(&app.session)))
        }
        kind @ (GenerationMode::Txt2img | GenerationMode::Inpaint) => {
            let canvas = d.doc.bounds();
            Some(match d.doc.selection.as_ref() {
                Some(sel) => {
                    let mut b = sel.content_bounds().intersect(&canvas);
                    if kind == GenerationMode::Inpaint {
                        b = photocraft_engine::ai_cmds::inpaint_area(b, canvas, ui.context());
                    }
                    (b.width(), b.height(), true)
                }
                None => (canvas.width(), canvas.height(), false),
            })
        }
    }
}

/// Room the prompt's text leaves on its right for Load Prompt, in the box's corner.
const LOAD_PROMPT_GAP: i8 = 34;
/// Height of the prompt box when the panel has no height of its own to fill.
const PROMPT_HEIGHT: f32 = 84.0;
/// Height of the scene box on the Regions tab: a few lines, the regions take the rest.
const SCENE_HEIGHT: f32 = 64.0;
/// The least the prompt box shrinks to in a short panel: two lines.
const MIN_PROMPT_HEIGHT: f32 = 40.0;
/// What the rows under the prompt need besides the notes: spacing and the Generate row.
const BELOW_PROMPT: f32 = 6.0 + 32.0 + 3.0 + 6.0;
/// One of the panel's rows of controls (Edit's tasks, a picture to choose, the mask tools, the
/// Advanced heading and what it opens), with its spacing.
const ROW: f32 = 24.0 + 6.0;
/// The brush the mask tools start with when the session's is smaller: a mask is painted broadly.
const MASK_BRUSH_MIN: f32 = 40.0;
/// Size of the notes under Generate.
const NOTE_SIZE: f32 = 11.0;

/// The rows the Advanced section opens for a generation of `kind` with `model`: the seed, for a
/// change also its strength or quality with the context, and the method when the model leaves
/// that open.
fn advanced_rows(model: Option<&Model>, kind: GenerationMode) -> usize {
    match kind {
        GenerationMode::Txt2img | GenerationMode::Img2img => 1,
        GenerationMode::Inpaint | GenerationMode::ImgEdit => 2 + usize::from(model.is_some_and(both_ways)),
    }
}

/// The rows above the prompt that depend on the task: Edit's tasks, the picture to change or
/// redraw, and the pictures an instruction may refer to (with a row to add one).
fn rows_above(ui: &AiGenUi, kind: GenerationMode, references: usize) -> usize {
    usize::from(ui.mode == Mode::Edit)
        + match kind {
            GenerationMode::Txt2img | GenerationMode::Inpaint => 0,
            GenerationMode::Img2img => 1,
            GenerationMode::ImgEdit => 1 + refs::rows(ui.editing_ref, references),
        }
}

/// The rows between the prompt and Generate: the mask tools of a change, the Advanced heading
/// and, open, its rows.
fn rows_below(ui: &AiGenUi, model: Option<&Model>, kind: GenerationMode) -> usize {
    usize::from(ui.changing()) + 1 + if ui.advanced { advanced_rows(model, kind) } else { 0 }
}

/// The height the panel's content wants in the dock (`panel_docking`'s `dock_heights`): the
/// tabs, the model, a prompt box of three lines, Generate and two lines of notes, and the rows
/// the task in front adds.
pub fn wanted_height(app: &PhotocraftApp) -> f32 {
    let ui = &app.ui.ai_gen;
    let model = current_model(app);
    let kind = technique(ui, model);
    let rows = 30.0 + 30.0 + 56.0 + BELOW_PROMPT + 2.0 * (NOTE_SIZE + 6.0) + 12.0;
    // The line that leads to the Local AI wizard, while there is one (it may wrap).
    let setup = if setup_line(app).is_some() { 2.0 * (NOTE_SIZE + 4.0) } else { 0.0 };
    // Create's ways (Prompt | Regions), and the regions under the scene.
    let create = usize::from(ui.mode == Mode::Create) + regions::rows(app);
    rows + setup + (create + rows_above(ui, kind, refs::kept(&app.session).len()) + rows_below(ui, model, kind)) as f32 * ROW
}

/// Is the inpaint mask being painted with the panel's brush?
fn mask_brushing(app: &PhotocraftApp) -> bool {
    app.ui.ai_gen.mask_brush && photocraft_engine::ai_cmds::mask_painting(&app.session)
}

/// A Brush or Eraser stroke about to be painted (`paint.stroke` params): while the inpaint mask
/// is being painted with the panel's brush, the Brush paints it in (white) and the Eraser out
/// (black), whatever the foreground and background colours are.
pub fn mask_stroke(app: &PhotocraftApp, erase: bool, params: &mut Value) {
    if mask_brushing(app) {
        params["erase"] = json!(false);
        params["color"] = json!(if erase { "#000000" } else { "#ffffff" });
    }
}

/// Run a mask command for the panel; a failure is a notice.
fn mask_run(app: &mut PhotocraftApp, id: &str, params: Value) -> bool {
    match app.run(id, params) {
        Ok(_) => true,
        Err(e) => {
            crate::notices::error(app, format!("{}: {e}", tr("Inpaint mask")));
            false
        }
    }
}

/// The mask tools under the prompt of a change: how to mark what to change without
/// leaving the panel. Rectangle and ellipse are the marquee tools (drag on the canvas; Shift adds,
/// Alt takes away); the brush paints the mask as a tint over the picture (Quick Mask, started
/// empty), with the eraser to take away and − / + for its size; the bin clears it. Generate turns
/// a painted mask into the selection, which becomes the new layer's mask.
fn mask_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui, busy: bool) {
    use crate::state::Tool;
    let t = Tokens::get(ui.ctx());
    let has_doc = app.session.active().is_some();
    let painting = mask_brushing(app);
    // A narrow panel keeps the brush size field but not its − and + (the field still takes the
    // wheel and typing), so nothing runs into the bin.
    let roomy = ui.available_width() >= 250.0;
    ui.add_enabled_ui(!busy && has_doc, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // (tool, icon, tooltip, paints the mask)
            let tools = [
                (Tool::RectMarquee, "square-dashed", "Mark a rectangle to repaint: drag on the picture. Shift adds to it, Alt takes away", false),
                (Tool::EllipseMarquee, "circle-dashed", "Mark an ellipse to repaint: drag on the picture. Shift adds to it, Alt takes away", false),
                (Tool::Brush, "brush", "Paint what to repaint: it shows as a tint over the picture", true),
                (Tool::Eraser, "eraser", "Paint out what should stay", true),
            ];
            for (tool, icon, tip, paints) in tools {
                let on = app.ui.tool == tool && painting == paints;
                if !crate::icons::button(ui, icon, 24.0, on, tr(tip)).clicked() {
                    continue;
                }
                // The brush works on the mask (Quick Mask, its tint over what is selected); the
                // marquees on the selection, which a painted mask becomes.
                if paints != photocraft_engine::ai_cmds::mask_painting(&app.session) && !mask_run(app, "ai.mask", json!({"paint": paints})) {
                    continue;
                }
                app.ui.ai_gen.mask_brush = paints;
                app.ui.tool = tool;
                if paints && app.session.tools.brush.size < MASK_BRUSH_MIN {
                    mask_run(app, "tools.setBrush", json!({"brush": {"size": MASK_BRUSH_MIN}}));
                }
            }
            ui.add_space(6.0);
            // The brush's size, for the two tools that paint.
            ui.add_enabled_ui(painting, |ui| {
                let size = app.session.tools.brush.size;
                let mut next = None;
                if roomy && crate::icons::button(ui, "minus", 22.0, false, tr("Smaller brush")).clicked() {
                    next = Some(size / 1.25);
                }
                let mut v = size;
                if widgets::value_field(ui, &mut v, 1.0..=2000.0, "px", 58.0).on_hover_text(tr("Brush size")).changed() {
                    next = Some(v);
                }
                if roomy && crate::icons::button(ui, "plus", 22.0, false, tr("Larger brush")).clicked() {
                    next = Some(size * 1.25);
                }
                if let Some(n) = next.filter(|n| n.is_finite()) {
                    mask_run(app, "tools.setBrush", json!({"brush": {"size": n.round().clamp(1.0, 2000.0)}}));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let marked = photocraft_engine::ai_cmds::inpaint_marked(&app.session);
                let clear = ui.add_enabled_ui(marked, |ui| crate::icons::button(ui, "trash", 24.0, false, tr("Clear the mask"))).inner;
                if clear.clicked() {
                    mask_run(app, "ai.mask", json!({"clear": true}));
                }
                // In a narrow panel the word gives way to the brush size rather than cover it.
                let word = ui.painter().layout_no_wrap(tr("Mask").to_string(), egui::FontId::proportional(NOTE_SIZE), t.text_faint).size().x;
                if !marked && ui.available_width() >= word + ui.spacing().item_spacing.x {
                    ui.label(RichText::new(tr("Mask")).color(t.text_faint).size(NOTE_SIZE));
                }
            });
        });
    });
}

/// The picture a restyle would redraw: the one chosen while it is still on offer, else the
/// engine's own default.
fn img_source(app: &PhotocraftApp) -> Option<SourceRef> {
    let offered = photocraft_engine::ai_cmds::img2img_sources(&app.session);
    app.ui.ai_gen.img_source.filter(|s| offered.iter().any(|(o, _)| o == s)).or_else(|| photocraft_engine::ai_cmds::default_img2img_source(&app.session))
}

/// The picture a change by instruction would change: the one chosen while it is still on offer, else the
/// engine's own default.
fn edit_source(app: &PhotocraftApp) -> Option<SourceRef> {
    let offered = photocraft_engine::ai_cmds::edit_sources(&app.session);
    app.ui.ai_gen.edit_source.filter(|s| offered.iter().any(|(o, _)| o == s)).or_else(|| photocraft_engine::ai_cmds::default_img2img_source(&app.session))
}

/// Width of the labels of the panel's labelled rows (Source, Image 2, Method, Seed), so their
/// fields line up.
const EDIT_LABEL: f32 = 54.0;

fn edit_label(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.allocate_ui_with_layout(vec2(EDIT_LABEL, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        // The cell keeps its width whatever the label's.
        ui.set_min_width(EDIT_LABEL);
        ui.label(RichText::new(text).color(t.text_dim));
    });
}

/// The Alpha Channel box. The choice is kept across models; one without alpha shows it off and
/// ignores it.
fn alpha_box(app: &mut PhotocraftApp, ui: &mut egui::Ui, current: &Model) {
    ui.add_enabled_ui(current.alpha, |ui| {
        let mut on = app.ui.ai_gen.alpha && current.alpha;
        let resp = widgets::checkbox(ui, &mut on, tr("Alpha Channel"));
        if current.alpha {
            app.ui.ai_gen.alpha = on;
            resp.on_hover_text(tr("Ask the model for a transparent background"));
        } else {
            resp.on_disabled_hover_text(tr("This model can't generate a transparent background"));
        }
    });
}

/// The heading of the Advanced section: a row that opens and closes it.
fn advanced_heading(ui: &mut egui::Ui, open: &mut bool) {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::click());
    let color = if resp.hovered() { t.text } else { t.text_dim };
    let chevron = egui::Rect::from_min_size(rect.min, vec2(14.0, rect.height()));
    crate::icons::paint(ui, chevron, if *open { "chevron-down" } else { "chevron-right" }, 12.0, color);
    let galley = ui.painter().layout_no_wrap(tr("Advanced").to_owned(), crate::theme::medium(11.5), color);
    ui.painter().galley(egui::pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, color);
    if resp.on_hover_text(tr("The settings few generations need: the seed and, for a change, how the model is asked and how much it is shown")).clicked() {
        *open = !*open;
    }
}

/// The seed typed into the Advanced field: its digits, whatever else was typed or pasted with
/// them. None: no seed. More than a seed holds: `kept`, what was there.
fn typed_seed(text: &str, kept: Option<u64>) -> Option<u64> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() { None } else { digits.parse::<u64>().ok().filter(|s| *s <= SEED_MASK).or(kept) }
}

/// Advanced: the seed. Empty, the model's own setting applies (a new seed each time, unless it
/// has a fixed one); the lock keeps the seed of the last image, to change the prompt and nothing
/// else.
fn seed_row(app: &mut PhotocraftApp, ui: &mut egui::Ui, current: &Model, enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            edit_label(ui, tr("Seed"));
            let kept = app.ui.ai_gen.seed();
            let mut text = kept.map(|s| s.to_string()).unwrap_or_default();
            let hint = if current.seed >= 0 { current.seed.to_string() } else { tr("Random").to_string() };
            let width = (ui.available_width() - 30.0).max(60.0);
            // As tall as the row's other fields, whatever the font: the rows have a set height.
            let edit = egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(width);
            let field = ui.add(edit.margin(egui::Margin::symmetric(4, 0)).min_size(vec2(0.0, 24.0)).align(egui::Align2::LEFT_CENTER));
            if field.changed() {
                app.ui.ai_gen.seed = typed_seed(&text, kept);
            }
            field.on_hover_text(tr("The number the image starts from: the same seed, model, prompt and settings give the same image. Empty: the model's own setting, a new one each time unless it has a fixed seed"));
            let last = app.ui.ai_gen.last_seed;
            let (icon, tip) = if kept.is_some() { ("lock", "A new seed each time") } else { ("lock-open", "Keep the last image's seed") };
            let lock = ui.add_enabled_ui(kept.is_some() || last.is_some(), |ui| crate::icons::button(ui, icon, 24.0, kept.is_some(), tr(tip))).inner;
            if lock.clicked() {
                app.ui.ai_gen.seed = if kept.is_some() { None } else { last };
            }
        });
    });
    ui.add_space(6.0);
}

/// Advanced, for a change: how a model that both edits and inpaints is asked.
fn method_row(app: &mut PhotocraftApp, ui: &mut egui::Ui, enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            edit_label(ui, tr("Method"));
            let options = [(Method::Instruct, tr("By instruction")), (Method::Fill, tr("Fill the selection"))];
            let mut method = app.ui.ai_gen.method;
            widgets::dropdown(ui, "ai-gen-method", tr("Method"), &mut method, &options, (ui.available_width() - 2.0).max(80.0));
            app.ui.ai_gen.method = method;
        })
        .response
        .on_hover_text(tr(
            "This model changes a picture both ways. By instruction: it is shown the picture and told what to change (\u{201c}make the jacket red\u{201d}). Fill the selection: the selection is its mask, and it paints what the prompt describes inside it",
        ));
    });
    ui.add_space(6.0);
}

/// Advanced, for a change: how far a fill may depart from what is there, or how much of an
/// instruction model's schedule to run, and how much of the picture around the selection the
/// model is shown.
fn change_row(app: &mut PhotocraftApp, ui: &mut egui::Ui, current: &Model, fill: bool, width: f32, enabled: bool) {
    let t = Tokens::get(ui.ctx());
    // A cloud model repaints the selection outright, and its quality is its own setting.
    let local = current.provider.is_local();
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let field = ((width - 6.0 * 3.0) / 2.0 - 58.0).clamp(48.0, 74.0);
            ui.add_enabled_ui(local, |ui| {
                if fill {
                    ui.label(RichText::new(tr("Strength")).color(t.text_dim));
                    let mut v = (app.ui.ai_gen.strength() * 100.0) as f32;
                    widgets::value_field(ui, &mut v, 5.0..=100.0, "%", field)
                        .on_hover_text(tr("How far the result may depart from what is there. 100% paints the selection afresh; less keeps more of its shapes and colours"))
                        .on_disabled_hover_text(tr("Cloud models repaint the selection outright"));
                    if local {
                        app.ui.ai_gen.strength = f64::from(v.round()) / 100.0;
                    }
                } else {
                    ui.label(RichText::new(tr("Quality")).color(t.text_dim));
                    let mut quality = app.ui.ai_gen.edit_quality;
                    let options = [(GenerationQuality::Draft, "Draft"), (GenerationQuality::Standard, "Standard"), (GenerationQuality::Detail, "Detail")];
                    ui.horizontal(|ui| widgets::dropdown(ui, "ai-gen-quality", tr("Quality"), &mut quality, &options, 96.0))
                        .response
                        .on_hover_text(format!(
                            "{} {} · {} {} · {} {}",
                            tr("How many of the model's steps to run. Draft, to try an idea:"),
                            GenerationQuality::Draft.steps(current.steps),
                            tr("Standard, the model's own:"),
                            GenerationQuality::Standard.steps(current.steps),
                            tr("Detail, for hands and fine detail:"),
                            GenerationQuality::Detail.steps(current.steps),
                        ))
                        .on_disabled_hover_text(tr("A cloud model's quality is set in its settings"));
                    if local {
                        app.ui.ai_gen.edit_quality = quality;
                    }
                }
            });
            ui.label(RichText::new(tr("Context")).color(t.text_dim));
            let mut v = (app.ui.ai_gen.context() * 100.0) as f32;
            widgets::value_field(ui, &mut v, 0.0..=200.0, "%", field).on_hover_text(tr(
                "How much of the picture around the selection the model is shown, as a share of the selection's longer edge. More matches the surroundings better; less leaves more detail for the selection",
            ));
            app.ui.ai_gen.context = f64::from(v.round()) / 100.0;
        });
    });
    ui.add_space(6.0);
}

/// The AI Gen panel body. With `fill` the prompt box takes the height the panel has left (the
/// dock group: drag its edge and the prompt grows); without it the box has a fixed height.
pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui, fill: bool) {
    if !fill {
        return panel_body(app, ui, None);
    }
    // The prompt box takes the height that is left. In a dock group too short even for a small
    // one (the tabs and a task's rows need more than the group's compact height) the panel
    // scrolls, so Generate can always be reached.
    let room = ui.available_height();
    egui::ScrollArea::vertical().id_salt("ai-gen-panel").auto_shrink([false, false]).show(ui, |ui| panel_body(app, ui, Some(room).filter(|r| r.is_finite())));
}

/// [`panel`]'s content; `room` is the height it has to fill, when it fills one.
fn panel_body(app: &mut PhotocraftApp, ui: &mut egui::Ui, room: Option<f32>) {
    let t = Tokens::get(ui.ctx());
    let top = ui.cursor().top();
    // Which local models can run is the server's to say.
    look_at_server(app, ui.ctx());
    let job = running(app);
    let busy = job.is_some();
    // What the user is there to do. A running generation keeps the tab and task it was started on.
    let mut mode = app.ui.ai_gen.mode;
    mode_tabs(ui, &mut mode, !busy);
    app.ui.ai_gen.mode = mode;
    ui.add_space(6.0);
    if mode == Mode::Edit {
        let mut task = app.ui.ai_gen.task;
        task_row(ui, &mut task, !busy);
        app.ui.ai_gen.task = task;
        ui.add_space(6.0);
    } else {
        let model = current_model(app).cloned();
        let able: Vec<String> = enabled_models(app).into_iter().filter(|m| m.layout().is_some()).map(|m| m.name).collect();
        let mut task = app.ui.ai_gen.create_task;
        let picked = create_row(ui, &mut task, !busy, model.as_ref(), &able);
        app.ui.ai_gen.create_task = task;
        if picked {
            regions::arm(app);
        }
        ui.add_space(6.0);
    }
    regions::follow(app);
    let changing = app.ui.ai_gen.changing();
    // The panel's brush paints the mask only while the mask is there to paint.
    if !photocraft_engine::ai_cmds::mask_painting(&app.session) {
        app.ui.ai_gen.mask_brush = false;
    }
    let models = enabled_models(app);
    if models.is_empty() {
        let left = room.map(|r| r - (ui.cursor().top() - top));
        nothing_ready(app, ui, changing, left);
        return;
    }
    // A model removed or disabled since it was chosen (or none chosen yet) falls back to the
    // first one, or for Change to Qwen Image 2.1. Change keeps its own choice: it lists other
    // models.
    let mut chosen = app.ui.ai_gen.chosen().to_string();
    if !models.iter().any(|m| m.id == chosen) {
        chosen = fallback(&models, changing).map(|m| m.id.clone()).unwrap_or_default();
    }
    let width = ui.available_width();

    // Model, and the shortcut to its settings.
    let mut settings = false;
    ui.horizontal(|ui| {
        let options: Vec<(String, &str)> = models.iter().map(|m| (m.id.clone(), m.name.as_str())).collect();
        let headings = model_headings(&models);
        ui.add_enabled_ui(!busy, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            widgets::dropdown_sectioned(ui, "ai-gen-model", tr("Model"), &mut chosen, &options, &headings, (width - 34.0).max(80.0));
        });
        settings = crate::icons::button(ui, "settings", 24.0, false, tr("Model settings…")).clicked();
    });
    app.ui.ai_gen.chosen_mut().clone_from(&chosen);
    if settings {
        open_settings(app, &chosen);
    }
    ui.add_space(6.0);
    let current = models.iter().find(|m| m.id == chosen).cloned().unwrap_or_default();
    // What the model is asked for: the task says what the user is doing, the model how.
    let kind = technique(&app.ui.ai_gen, Some(&current));
    let (inpaint, img2img, edit) = (kind == GenerationMode::Inpaint, kind == GenerationMode::Img2img, kind == GenerationMode::ImgEdit);
    // A restyle: the picture to redraw. A change by instruction: the picture to change.
    if img2img || edit {
        let offered = if edit { photocraft_engine::ai_cmds::edit_sources(&app.session) } else { photocraft_engine::ai_cmds::img2img_sources(&app.session) };
        ui.add_enabled_ui(!busy && !offered.is_empty(), |ui| {
            ui.horizontal(|ui| {
                // A long layer name (a generated layer is named after its prompt) is cut short:
                // grown to fit, the dropdown would push Generate out of a narrow panel.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                edit_label(ui, tr("Source"));
                let left = (ui.available_width() - 2.0).max(80.0);
                match if edit { edit_source(app) } else { img_source(app) } {
                    Some(mut source) => {
                        let options: Vec<(SourceRef, &str)> = offered.iter().map(|(s, name)| (*s, name.as_str())).collect();
                        widgets::dropdown(ui, "ai-gen-source", tr("Source"), &mut source, &options, left);
                        if edit {
                            app.ui.ai_gen.edit_source = Some(source);
                        } else {
                            app.ui.ai_gen.img_source = Some(source);
                        }
                    }
                    None => {
                        ui.label(RichText::new(tr("No layer with pixels, no other document")).color(t.text_faint));
                    }
                }
            })
            .response
            .on_hover_text(tr(if edit {
                "The picture to change (image 1 to the model): a layer, this document with all its layers, or another open document. With a selection, only the part of it around the selection is sent to the model, and the result is masked by the selection: the rest of the picture stays untouched and keeps its resolution"
            } else {
                "The picture to redraw. A layer of this document is redrawn where it is (in the selection, when there is one); another open document is fitted to the selection or the canvas"
            }));
        });
        ui.add_space(6.0);
    }
    if edit {
        refs::chips(app, ui, busy, current.max_references());
    }

    // What goes under Generate, worked out first: the prompt box takes the height that is left,
    // so it has to know how tall the notes are (a reason Generate is off can be several lines).
    let why = generate_blocked(app, &current);
    let note = match (&job, &why) {
        (Some(job), _) => Some(if job.message.is_empty() { job.label.clone() } else { job.message.clone() }),
        (None, Some(why)) => Some(why.clone()),
        (None, None) => target_size(app).map(|(w, h, selection)| {
            let given = inpaint || img2img || edit;
            let (rw, rh) = if given { photocraft_aigen::source_size(&current, w, h) } else { render_size(&current, w, h) };
            // An edit of a picture of this document is shown the selection's surroundings, too.
            let here = app.session.active().map(|d| d.doc.id.0);
            let around = edit && !matches!(edit_source(app), Some(SourceRef::Document(id)) if Some(id) != here);
            let area = match (inpaint || around, img2img || edit, selection) {
                (true, _, true) => tr("Selection and context"),
                (_, _, true) => tr("Selection"),
                (_, true, false) => tr("Area"),
                (_, false, false) => tr("Canvas"),
            };
            // These are asked for a shape and a size tier, not for pixels.
            if matches!(current.provider, Provider::Gemini | Provider::Xai | Provider::Bfl) && !given {
                format!("{area} {w} × {h} px → {rw}:{rh}, {}", current.image_size)
            } else {
                format!("{area} {w} × {h} px → {rw} × {rh} px")
            }
        }),
    };
    let price = match current.provider {
        Provider::Openai => Some(price_note(app, &current)),
        Provider::Xai | Provider::Meta | Provider::Bfl => reported_price_note(app, &current),
        Provider::Comfy | Provider::Gemini => None,
    };
    let setup = setup_line(app);
    let line_gap = ui.spacing().item_spacing.y;
    let notes_height: f32 = note
        .iter()
        .chain(price.iter().map(|(text, _)| text))
        .chain(setup.iter())
        .map(|text| ui.painter().layout(text.clone(), egui::FontId::proportional(NOTE_SIZE), t.text_faint, width).size().y + line_gap)
        .sum();

    // Prompt. ⌘/Ctrl+Return generates. Laying out regions, it is the scene, a box of a few lines
    // with the regions under it.
    let laying_out = regions::tab_open(app);
    if laying_out {
        ui.label(RichText::new(tr("Scene")).color(t.text_dim).size(11.5));
    }
    let mut submit = false;
    ui.add_enabled_ui(!busy, |ui| {
        let below = BELOW_PROMPT + rows_below(&app.ui.ai_gen, Some(&current), kind) as f32 * (ROW + line_gap);
        let height = match room {
            _ if laying_out => SCENE_HEIGHT,
            Some(room) => (room - (ui.cursor().top() - top) - below - notes_height).clamp(MIN_PROMPT_HEIGHT, 1200.0),
            None => PROMPT_HEIGHT,
        };
        // A prompt longer than the box scrolls inside it instead of pushing Generate away.
        // An instruction's references: `@Name` shows in the accent colour, and typing `@` offers
        // the names.
        let names: Vec<String> = if edit { refs::kept(&app.session).into_iter().map(|r| r.name).filter(|n| !n.is_empty()).collect() } else { Vec::new() };
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        if edit {
            refs::before_prompt(ui, &mut app.ui.ai_gen.prompt);
        }
        let area = egui::ScrollArea::vertical().id_salt("ai-gen-prompt").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let color = ui.visuals().override_text_color.unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                ui.painter().layout_job(refs::prompt_job(text.as_str(), &names, font.clone(), color, t.accent, wrap))
            };
            let mut edit_box = egui::TextEdit::multiline(&mut app.ui.ai_gen.prompt)
                .id(refs::prompt_id())
                .hint_text(tr(match kind {
                    GenerationMode::Inpaint => "Describe what to paint in the selection…",
                    GenerationMode::Img2img => "Describe what the picture should become…",
                    GenerationMode::ImgEdit if !names.is_empty() => "Say what to change, calling a reference by name: \u{201c}@Luna sits on the bench\u{201d}…",
                    GenerationMode::ImgEdit => "Say what to change: \u{201c}make the jacket red leather\u{201d}…",
                    GenerationMode::Txt2img if laying_out => "Describe the whole scene: the setting, the light, the style…",
                    GenerationMode::Txt2img => "Describe what to generate…",
                }))
                .desired_width(width)
                .min_size(vec2(width, height))
                // The text keeps clear of Load Prompt, in the box's corner.
                .margin(egui::Margin { left: 4, right: LOAD_PROMPT_GAP, top: 2, bottom: 2 });
            if edit {
                // While names are offered for an `@`, Tab takes the first instead of moving on.
                let tab = refs::offering(ui.ctx());
                edit_box = edit_box.layouter(&mut layouter).event_filter(egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    tab,
                    ..Default::default()
                });
            }
            let output = edit_box.show(ui);
            submit = output.response.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
            if edit {
                refs::after_prompt(ui, &output, &app.ui.ai_gen.prompt, &names);
            }
        });
        // Load Prompt, in the box's top right corner (clear of the scroll bar): the prompt the
        // selected layer was generated with, the text only (the tab, model and settings stay).
        let layer = active_layer(app).filter(|l| layer_prompt(app, *l).is_some());
        let corner = egui::Rect::from_min_size(egui::pos2(area.inner_rect.right() - 24.0 - 8.0, area.inner_rect.top() + 2.0), vec2(24.0, 24.0));
        let mut button = ui.new_child(egui::UiBuilder::new().max_rect(corner));
        let tip = if layer.is_some() { "Load the prompt the selected layer was generated with" } else { "Load prompt: select a layer made with AI Gen" };
        let load = button.add_enabled_ui(layer.is_some(), |ui| crate::icons::button(ui, "import", 24.0, false, tr(tip))).inner;
        if let Some(layer) = layer.filter(|_| load.clicked()) {
            load_prompt(app, layer);
        }
    });
    ui.add_space(6.0);

    // Create › Regions: the boxes the scene is laid out in.
    if laying_out {
        regions::section(app, ui, busy, &current.id);
        ui.add_space(6.0);
    }

    // A change: what to change, marked from here.
    if changing {
        mask_bar(app, ui, busy);
        ui.add_space(6.0);
    }

    // What few generations need.
    let mut open = app.ui.ai_gen.advanced;
    advanced_heading(ui, &mut open);
    app.ui.ai_gen.advanced = open;
    ui.add_space(6.0);
    if open {
        // Each row is the panel's own (not rows of one group), so each is as tall as a row is.
        if changing && both_ways(&current) {
            method_row(app, ui, !busy);
        }
        if inpaint || edit {
            change_row(app, ui, &current, inpaint, width, !busy);
        }
        seed_row(app, ui, &current, !busy);
    }

    // One row, so the button stays in view in a short dock group: an option on the left,
    // Generate on the right; a running generation shows its progress and Cancel there instead.
    let mut clicked = false;
    let mut cancel = None;
    ui.horizontal(|ui| {
        if let Some(job) = &job {
            ui.add(egui::ProgressBar::new(job.progress.clamp(0.0, 1.0)).desired_width((width - 84.0).max(40.0)).desired_height(6.0).fill(t.accent));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::secondary_button(ui, tr("Cancel"), 70.0).clicked() {
                    cancel = Some(job.id);
                }
            });
            return;
        }
        match kind {
            // A restyle: how closely the picture is followed.
            GenerationMode::Img2img => {
                // Cloud models are given the picture and the prompt, and no such control.
                let local = current.provider.is_local();
                ui.add_enabled_ui(local, |ui| {
                    ui.label(RichText::new(tr("Strength")).color(t.text_dim));
                    let mut v = (app.ui.ai_gen.img_strength() * 100.0) as f32;
                    widgets::value_field(ui, &mut v, 5.0..=100.0, "%", 64.0)
                        .on_hover_text(tr("How far the result may depart from the picture. Low keeps its shapes and colours and changes the surface; high keeps little more than the composition; 100% ignores it"))
                        .on_disabled_hover_text(tr("Cloud models are given the picture and the prompt, without a strength"));
                    if local {
                        app.ui.ai_gen.img_strength = f64::from(v.round()) / 100.0;
                    }
                });
            }
            GenerationMode::Txt2img | GenerationMode::ImgEdit => alpha_box(app, ui, &current),
            // A fill paints what it repaints: there is no background to leave out.
            GenerationMode::Inpaint => {}
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = ui.add_enabled_ui(why.is_none(), |ui| widgets::primary_button(ui, tr("Generate"), 96.0).clicked()).inner;
        });
    });
    if let Some(id) = cancel {
        crate::jobs_ui::cancel(app, id);
    }
    ui.add_space(3.0);

    // Under it: the job's status, or why Generate is off, or what will be rendered (so the size
    // is no surprise).
    if let Some(note) = note {
        // A reason Generate is off is something to act on: a shade stronger, and selectable.
        let color = if job.is_none() && why.is_some() { t.text_dim } else { t.text_faint };
        ui.add(egui::Label::new(RichText::new(note).color(color).size(NOTE_SIZE)).wrap().selectable(true));
    }
    // OpenAI bills per image: what this one should cost, and what the last one did.
    if let Some((text, tip)) = price {
        ui.label(RichText::new(text).color(t.text_faint).size(NOTE_SIZE)).on_hover_text(tip);
    }
    // The way into the Local AI wizard: its progress while it runs, an offer until it has.
    if let Some(text) = setup {
        let line = ui.add(egui::Label::new(RichText::new(text).color(t.accent).size(NOTE_SIZE)).wrap().sense(egui::Sense::click()));
        let tip = tr("Installs ComfyUI and the recommended models for you: no Python, no terminal. Also File › Set Up Local AI…");
        if line.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            crate::ai_setup_ui::open(app);
        }
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

/// The line under Generate for the providers whose answer says what the image cost (xAI, Black
/// Forest Labs) or that publish one flat price (Meta's Muse Image), with its tooltip. `None`
/// until there is something to say.
fn reported_price_note(app: &PhotocraftApp, model: &Model) -> Option<(String, String)> {
    use photocraft_aigen::pricing;
    let flat = (model.provider == Provider::Meta && model.api_model.trim().starts_with("muse-image")).then_some(pricing::MUSE_IMAGE_USD);
    let last = app.ui.ai_gen.last_cost;
    let text = match (flat, last) {
        (Some(price), _) => format!("{} {}", tr("Cost"), pricing::format_usd(price)),
        (None, Some(cost)) => format!("{} {}", tr("Last image"), pricing::format_usd(cost)),
        (None, None) => return None,
    };
    let tip = match model.provider {
        Provider::Meta => tr("Meta bills Muse Image one flat price for each image it returns, whatever its size."),
        _ => tr("What the previous image cost, as the provider's answer reported it."),
    };
    Some((text, tip.to_string()))
}

/// Why Generate can't run now (shown under the disabled button), or `None`.
fn generate_blocked(app: &PhotocraftApp, model: &Model) -> Option<String> {
    if app.session.active().is_none() {
        return Some(tr("Open a document to generate into.").to_string());
    }
    if let Err(why) = app.session.ai.ready(model) {
        return Some(why);
    }
    // A model set up in a way that can't run: say so before a server is started for it.
    if model.provider.is_local()
        && let Some(why) = photocraft_aigen::detect::problem(model)
    {
        return Some(why);
    }
    let ui = &app.ui.ai_gen;
    // A state set from outside may name a model the task does not list.
    if ui.changing() && !fits(model, true) {
        return Some(format!("{} {}", model.name, tr("can't change a picture: choose a model that edits or inpaints.")));
    }
    let local = model.provider.is_local();
    match technique(ui, Some(model)) {
        GenerationMode::Txt2img => {}
        GenerationMode::Inpaint => {
            // What the engine would refuse: no selection, or no pixels of the layer under it.
            if let Err(why) = photocraft_engine::ai_cmds::inpaint_blocked(&app.session) {
                return Some(why);
            }
            if local && let Err(why) = photocraft_aigen::comfy::check(model, true) {
                return Some(why.to_string());
            }
        }
        GenerationMode::Img2img => {
            // What the engine would refuse: no picture to start from, or none in the selection.
            if let Err(why) = photocraft_engine::ai_cmds::img2img_plan(&app.session, img_source(app), None) {
                return Some(why);
            }
            if local && let Err(why) = photocraft_aigen::comfy::check_img2img(model) {
                return Some(why.to_string());
            }
        }
        GenerationMode::ImgEdit => {
            // What the engine would refuse: no picture to change, or none in the selection.
            if let Err(why) = photocraft_engine::ai_cmds::edit_plan(&app.session, edit_source(app), ui.context(), None) {
                return Some(why);
            }
            // More references than the model takes: the prompt has to say which, by name.
            let kept = refs::kept(&app.session);
            let sent = photocraft_engine::ai_refs::resolve(&app.ui.ai_gen.prompt, &kept.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()).sent.len();
            if sent > model.max_references() {
                return Some(format!(
                    "{} {} {}",
                    model.name,
                    tr("takes fewer references than the document has:"),
                    tr("mention the ones to send by name (@Name), or take some out.")
                ));
            }
            if local && let Err(why) = photocraft_aigen::comfy::check_edit(model) {
                return Some(why.to_string());
            }
        }
    }
    if app.ui.ai_gen.prompt.trim().is_empty() {
        return Some(tr("Write a prompt first.").to_string());
    }
    if let Some(job) = app.session.active().and_then(|d| app.session.job_on(d.doc.id)) {
        return Some(format!("{} ({})", tr("The document is busy"), job.label));
    }
    None
}

/// The size `model` is asked to render for a `w` × `h` area (Gemini, xAI and Black Forest Labs:
/// the aspect ratio).
fn render_size(model: &Model, w: u32, h: u32) -> (u32, u32) {
    use photocraft_aigen::cloud;
    match model.provider {
        Provider::Comfy => model.family.render_size(w, h, model.megapixels),
        Provider::Openai => cloud::openai_size(model, w, h),
        Provider::Gemini => cloud::gemini_ratio(w, h),
        Provider::Xai => cloud::xai_ratio(w, h),
        Provider::Meta => cloud::meta_size(w, h),
        Provider::Bfl => cloud::bfl_ratio(w, h),
    }
}

// ---------- Preferences › Generative AI ----------

/// Dialog fields (not preferences): the model being edited, the server's file lists, the API
/// keys typed in this dialog, what the last "Connect" said, and a request for [`after_body`].
const SELECTED: &str = "__aiSelected";
const FILES: &str = "__aiFiles";
const KEYS: &str = "__aiKeys";
const STATUS: &str = "__aiStatus";
const ACTION: &str = "__aiAction";
/// The page of the section in view, what the Manage Models list is narrowed to ([`Filter`]), and
/// that the page already tried to read the server.
const PAGE: &str = "__aiPage";
const FILTER: &str = "__aiFilter";
const AUTO: &str = "__aiAutoConnect";
/// A Connect / Scan running in the background (`{id, action}`), and whether the app has a ComfyUI
/// it started itself (for the Stop button).
const JOB: &str = "__aiJob";
const STARTED: &str = "__aiStarted";
/// What the last Find Installation / Browse / Stop Server did (`{error, text}`).
const LAUNCH_STATUS: &str = "__aiLaunchStatus";
/// What the Local AI setup has installed (`{available, text, installed, managed}`), for the Local
/// Server page's first block.
const SETUP: &str = "__aiSetup";
/// A page to go to from the one drawn (`keys`, say), the provider whose key the API Keys page
/// puts in front (and that its field takes the keyboard once), and an address to open in the
/// browser. The body has no app: the dialog acts on them after it.
const GOTO: &str = "__aiGoto";
const KEY_FOCUS: &str = "__aiKeyFocus";
const KEY_FOCUS_ONCE: &str = "__aiKeyFocusOnce";
const OPEN_URL: &str = "__aiOpenUrl";
/// What the dialog read once when it opened (the setup's status and the wizard's models).
const READ: &str = "__aiRead";

/// Why a model of the draft can't run yet, as Manage Models says it and what it offers to do.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Missing {
    /// A cloud model whose provider has no API key.
    Key(Provider),
    /// A local model whose file isn't among the ones known to be there; the wizard's model that
    /// installs it, if it has one (`{id, name, size}` of [`crate::ai_setup_ui::summary`]).
    Files(Option<Value>),
    /// A local model before local AI is set up (nothing installed, nothing to start, no server
    /// answered); the wizard's model, if it has one.
    NoLocalAi(Option<Value>),
}

/// What Manage Models knows of where the draft's models stand.
struct Standing {
    /// The cloud providers with a key: saved, or typed in this dialog.
    keyed: Vec<Provider>,
    /// The model files known to be there: the server's when it answered, else those of the
    /// engine the setup installed. `None` when neither is known: then nothing is marked.
    files: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// Something will run local models: a server answered, the setup has run, or ComfyUI is set
    /// up to be started.
    local: bool,
    /// The wizard's models (`[{id, name, main, size}]`).
    catalog: Vec<Value>,
}

impl Standing {
    fn of(ai: &Map<String, Value>, f: &Map<String, Value>) -> Self {
        let keys = f.get(KEYS).and_then(Value::as_object);
        let keyed = Provider::ALL
            .into_iter()
            .filter(|p| !p.is_local())
            .filter(|p| {
                let saved = keys.and_then(|k| k.get(&format!("{}Saved", p.key()))).and_then(Value::as_bool).unwrap_or(false);
                let typed = keys.is_some_and(|k| !text_of(k, p.key()).trim().is_empty());
                saved || typed
            })
            .collect();
        let setup = f.get(SETUP);
        let parse = |v: Option<&Value>| v.filter(|v| v.is_object()).and_then(|v| serde_json::from_value(v.clone()).ok());
        let answered = parse(f.get(FILES));
        let installed = setup.and_then(|s| s.get("installed")).and_then(Value::as_bool).unwrap_or(false);
        let launch = ai.get("comfyLaunch").and_then(Value::as_object).is_some_and(|l| !text_of(l, "python").trim().is_empty());
        let autostart = ai.get("comfyAutostart").and_then(Value::as_bool).unwrap_or(false);
        Self {
            keyed,
            local: answered.is_some() || installed || launch || autostart,
            files: answered.or_else(|| parse(setup.and_then(|s| s.get("modelFiles")))),
            catalog: setup.and_then(|s| s.get("catalog")).and_then(Value::as_array).cloned().unwrap_or_default(),
        }
    }

    /// The wizard's model `model` is: the one of its id (the recommended models keep the
    /// wizard's), else the one whose main file is `model`'s.
    fn wizard_model(&self, model: &Model) -> Option<Value> {
        let id = |m: &&Value| m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let file = model.main_file();
        self.catalog
            .iter()
            .find(|m| !model.id.is_empty() && id(m) == model.id)
            .or_else(|| self.catalog.iter().find(|m| !file.is_empty() && m.get("main").and_then(Value::as_str) == Some(file)))
            .cloned()
    }

    fn missing(&self, model: &Model) -> Option<Missing> {
        if !model.provider.is_local() {
            return (!self.keyed.contains(&model.provider)).then_some(Missing::Key(model.provider));
        }
        if !self.local {
            return Some(Missing::NoLocalAi(self.wizard_model(model)));
        }
        let files = self.files.as_ref()?;
        (!photocraft_aigen::detect::on_server(files, model)).then(|| Missing::Files(self.wizard_model(model)))
    }
}

/// Where a provider's API keys are made (the page, and the address to show of it), and what to
/// know before making one.
fn key_source(p: Provider) -> (&'static str, &'static str, &'static str) {
    match p {
        Provider::Openai => (
            "https://platform.openai.com/api-keys",
            "platform.openai.com",
            "Create a secret key on the API keys page. Images are billed to your OpenAI API account, which needs credit: a ChatGPT subscription doesn't cover it.",
        ),
        Provider::Gemini => (
            "https://aistudio.google.com/apikey",
            "aistudio.google.com",
            "Create a key in Google AI Studio. Images are billed to the key's Google Cloud project, so it needs billing set up.",
        ),
        Provider::Xai => ("https://console.x.ai", "console.x.ai", "Create an API key in the xAI console. Images are paid for from your team's credits."),
        Provider::Meta => ("https://dev.meta.ai", "dev.meta.ai", "Sign up for the Meta Model API on Meta's developer site, and create a key there."),
        Provider::Bfl => (
            "https://dashboard.bfl.ai",
            "dashboard.bfl.ai",
            "Add a key under your project's API keys in the dashboard. Images are paid for with credits bought there.",
        ),
        Provider::Comfy => ("", "", ""),
    }
}

/// Ask the dialog to show the API Keys page with `provider`'s key in front.
fn go_to_key(f: &mut Map<String, Value>, provider: Provider) {
    f.insert(GOTO.into(), json!("keys"));
    f.insert(KEY_FOCUS.into(), json!(provider.key()));
    f.insert(KEY_FOCUS_ONCE.into(), json!(true));
}

/// What a model that can't run offers to do about it: (icon, tooltip, its action on `f`).
fn missing_action(lack: &Missing, name: &str) -> Option<(&'static str, String)> {
    match lack {
        Missing::Key(p) => Some(("key-round", format!("{} {}", tr("Add an API key for"), p.label()))),
        Missing::Files(Some(_)) | Missing::NoLocalAi(Some(_)) => {
            Some(("download", format!("{} {name}: {}", tr("Install"), tr("the Local AI setup opens with it ticked"))))
        }
        Missing::NoLocalAi(None) => Some(("download", tr("Set up local AI: the setup installs ComfyUI and the models you choose").to_string())),
        Missing::Files(None) => None,
    }
}

/// Do what [`missing_action`] offers.
fn act_on_missing(f: &mut Map<String, Value>, lack: &Missing) {
    match lack {
        Missing::Key(p) => go_to_key(f, *p),
        Missing::Files(Some(wizard)) | Missing::NoLocalAi(Some(wizard)) => {
            let id = wizard.get("id").and_then(Value::as_str).unwrap_or("");
            f.insert(ACTION.into(), json!(format!("install:{id}")));
        }
        Missing::NoLocalAi(None) => {
            f.insert(ACTION.into(), json!("setup"));
        }
        Missing::Files(None) => {}
    }
}

/// Above the selected model's settings when it can't run: why, and the way to make it run.
/// Returns whether that way was taken (see [`act_on_missing`]).
fn missing_banner(ui: &mut egui::Ui, lack: &Missing, model: &Model) -> bool {
    let t = Tokens::get(ui.ctx());
    let name = if model.name.trim().is_empty() { model.family.label() } else { model.name.as_str() };
    let size = |w: &Value| w.get("size").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| format!(" ({s})")).unwrap_or_default();
    let (text, button) = match lack {
        Missing::Key(p) => (
            format!("{name} {} {}. {}", tr("runs on the servers of"), p.label(), tr("It needs your API key from them, added on the API Keys page.")),
            Some(tr("Add API Key…")),
        ),
        Missing::Files(Some(wizard)) => {
            (format!("{name} {}{}. {}", tr("is not installed"), size(wizard), tr("The Local AI setup downloads it and sets it up.")), Some(tr("Install…")))
        }
        Missing::Files(None) => (
            format!(
                "{} {}. {}",
                tr("The ComfyUI server doesn't have"),
                model.main_file(),
                tr("Download it into ComfyUI's models folder and press Scan Server, or choose another file below.")
            ),
            None,
        ),
        Missing::NoLocalAi(wizard) => (
            format!(
                "{}{}",
                tr("Local models need local AI set up first: the setup installs ComfyUI and the models you choose."),
                wizard.as_ref().map(|w| format!(" {name}{}.", size(w))).unwrap_or_default()
            ),
            Some(if wizard.is_some() { tr("Install…") } else { tr("Set Up Local AI…") }),
        ),
    };
    let mut clicked = false;
    // Clear of the scroll area's edge, which would clip its border.
    ui.add_space(2.0);
    egui::Frame::new().fill(t.field).stroke(egui::Stroke::new(1.0, t.warning)).corner_radius(t.radius_sm).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).color(t.text).size(11.5)).wrap());
        if let Some(label) = button {
            ui.add_space(6.0);
            clicked = widgets::primary_button(ui, label, 120.0).clicked();
        }
    });
    ui.add_space(8.0);
    clicked
}

/// The address the API Keys page asked to open, opened (the dialog body has no app).
pub fn open_requested(app: &mut PhotocraftApp, ctx: &egui::Context, f: &mut Map<String, Value>) {
    if let Some(url) = f.remove(OPEN_URL).and_then(|u| u.as_str().map(str::to_string)) {
        crate::links::open(app, ctx, &url);
    }
}

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
    ui.add(egui::TextEdit::singleline(&mut s).desired_width(field_width(ui)).hint_text(hint));
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
            widgets::dropdown(ui, &format!("ai-file-{key}"), tr(title), &mut cur, &options, field_width(ui));
        }
        None => {
            ui.add(egui::TextEdit::singleline(&mut cur).desired_width(field_width(ui)).hint_text(tr("Not set")));
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
    widgets::dropdown(ui, &format!("ai-choice-{key}"), tr(title), &mut cur, &options, field_width(ui));
    obj.insert(key.into(), json!(cur));
    ui.end_row();
}

/// Samplers and schedulers ComfyUI ships; a value outside the list (a custom node's) is kept.
const SAMPLERS: [&str; 15] = [
    "euler",
    "euler_ancestral",
    "heun",
    "dpm_2",
    "dpmpp_2s_ancestral",
    "dpmpp_sde",
    "dpmpp_2m",
    "dpmpp_2m_sde",
    "dpmpp_2m_sde_gpu",
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
    let settings: Settings = serde_json::from_value(Value::Object(ai.clone())).unwrap_or_else(|_| Settings::default_without_models());
    model.id = settings.unique_id(&model.name);
    let Ok(v) = serde_json::to_value(&model) else { return };
    if let Some(list) = ai.entry("models").or_insert_with(|| json!([])).as_array_mut() {
        list.push(v);
        f.insert(SELECTED.into(), json!(list.len().saturating_sub(1)));
    }
}

/// The pages of the Generative AI section: (key, title).
const PAGES: [(&str, &str); 3] = [("models", "Manage Models"), ("keys", "API Keys"), ("server", "Local Server")];

/// What the Manage Models list is narrowed to: every model, or the ones that do one of the AI Gen
/// panel's jobs. A model that does several (FLUX.2 generates, edits and inpaints) is listed
/// under each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    /// Generates from a prompt alone ([`Model::can_generate`]).
    Create,
    /// Changes a picture following an instruction ([`Model::can_edit`]).
    Edit,
    /// Repaints a selection ([`Model::can_inpaint`]).
    Inpaint,
}

impl Filter {
    const ALL: [Filter; 4] = [Filter::All, Filter::Create, Filter::Edit, Filter::Inpaint];

    fn key(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Create => "create",
            Filter::Edit => "edit",
            Filter::Inpaint => "inpaint",
        }
    }

    fn from_key(key: &str) -> Filter {
        Filter::ALL.into_iter().find(|x| x.key() == key).unwrap_or(Filter::All)
    }

    fn title(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Create => "Text to Image",
            Filter::Edit => "Editing",
            Filter::Inpaint => "Inpainting",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Filter::All => "Ticked models are offered in the AI Gen panel. One that can't run yet (no API key, or not installed) can't be ticked until it can.",
            Filter::Create => "Models that make a picture from a prompt: the AI Gen panel's Create tab.",
            Filter::Edit => "Models that change a picture as an instruction says: the AI Gen panel's Edit tab.",
            Filter::Inpaint => {
                "Models that repaint a selection (the Edit tab). FLUX.1 Fill and Qwen Image with InstantX's ControlNet are made for it; any other can be tried by ticking its Supports inpainting box."
            }
        }
    }

    /// Lists `model`: a model of the draft, or `None` for an entry that could not be read (which
    /// is listed under All only, where it can be removed).
    fn shows(self, model: Option<&Model>) -> bool {
        match (self, model) {
            (Filter::All, _) => true,
            (_, None) => false,
            (Filter::Create, Some(m)) => m.can_generate(),
            (Filter::Edit, Some(m)) => m.can_edit(),
            (Filter::Inpaint, Some(m)) => m.can_inpaint(),
        }
    }
}

/// What a model is used for, in the words of the list's filters.
fn uses(m: &Model) -> Vec<&'static str> {
    [(m.can_generate(), "Text to image"), (m.can_edit(), "Editing"), (m.can_inpaint(), "Inpainting")]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, what)| what)
        .collect()
}

/// A model of the draft, when it reads as one.
fn model_of(v: &Value) -> Option<Model> {
    if v.is_object() { serde_json::from_value(v.clone()).ok() } else { None }
}

/// Height of a row of the model list: the name, and under it what the model is used for.
const ROW_HEIGHT: f32 = 36.0;
/// The least height of the model list and the settings beside it; they take what the dialog has.
const MIN_PANE_HEIGHT: f32 = 200.0;

/// The widths of the model list and of a field in the model settings, for a page `page` wide:
/// the Preferences dialog's width is the app's to decide, so the page divides what it gets
/// (two fifths to the list, the rest less the labels to the fields) within sensible bounds.
fn page_widths(page: f32) -> (f32, f32) {
    let page = if page.is_finite() { page } else { 540.0 };
    let list = (page * 0.4).clamp(210.0, 340.0);
    (list, (page - list - 160.0).clamp(170.0, 420.0))
}

fn widths_id() -> egui::Id {
    egui::Id::unique("ai-settings-widths")
}

/// The field width of the page being drawn (see [`page_widths`]).
fn field_width(ui: &egui::Ui) -> f32 {
    ui.data(|d| d.get_temp::<(f32, f32)>(widths_id())).map_or(200.0, |(_, field)| field)
}

fn list_width(ui: &egui::Ui) -> f32 {
    ui.data(|d| d.get_temp::<(f32, f32)>(widths_id())).map_or(180.0, |(list, _)| list)
}

fn is_local_model(m: &Value) -> bool {
    m.get("provider").and_then(Value::as_str).unwrap_or(Provider::Comfy.key()) == Provider::Comfy.key()
}

/// The Generative AI section's body: draws and edits `integrations.ai` in the dialog's draft
/// (`obj` is the Integrations object; `f` the dialog's fields), in the `height` the dialog has
/// for it. Three pages: Manage Models (every model in a list beside the selected one's
/// settings), API keys, and the local server.
pub fn prefs_section(ui: &mut egui::Ui, obj: &mut Map<String, Value>, f: &mut Map<String, Value>, height: f32) {
    let mut ai = obj.get("ai").and_then(Value::as_object).cloned().unwrap_or_default();
    let widths = page_widths(ui.available_width());
    ui.data_mut(|d| d.insert_temp(widths_id(), widths));
    let height = if height.is_finite() { height.clamp(MIN_PANE_HEIGHT, 4000.0) } else { MIN_PANE_HEIGHT };
    let bottom = ui.cursor().top() + height;
    let mut page = f.get(PAGE).and_then(Value::as_str).filter(|p| PAGES.iter().any(|(k, _)| k == p)).unwrap_or("models").to_string();
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
        "models" => models_page(ui, &mut ai, f, bottom),
        // These two are longer than a small window: they scroll in what is left.
        other => {
            let room = (bottom - ui.cursor().top()).max(MIN_PANE_HEIGHT);
            egui::ScrollArea::vertical().id_salt(("ai-page", other)).max_height(room).auto_shrink([false, true]).show(ui, |ui| match other {
                "keys" => keys_page(ui, &ai, f),
                _ => server_page(ui, &mut ai, f),
            });
        }
    }
    // A page asked for another (a model's "Add an API key"): it is in front from the next frame.
    if let Some(go) = f.remove(GOTO).and_then(|g| g.as_str().map(str::to_string)).filter(|g| PAGES.iter().any(|(k, _)| k == g)) {
        page = go;
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
    // ----- having it all set up for you -----
    let setup = f.get(SETUP).cloned().unwrap_or(Value::Null);
    if setup.get("available").and_then(Value::as_bool) == Some(true) {
        widgets::section_label(ui, tr("Local AI setup"));
        ui.add_space(2.0);
        ui.scope(|ui| {
            ui.set_max_width(520.0);
            ui.add(egui::Label::new(RichText::new(setup.get("text").and_then(Value::as_str).unwrap_or("")).color(t.text_faint).size(11.5)).wrap());
        });
        ui.add_space(4.0);
        let installed = setup.get("installed").and_then(Value::as_bool).unwrap_or(false);
        ui.horizontal(|ui| {
            let open = if installed { tr("Add Models…") } else { tr("Set Up Local AI…") };
            if widgets::secondary_button(ui, open, 150.0)
                .on_hover_text(tr("Opens the setup wizard: it installs ComfyUI and the recommended models for you. These preferences are saved first"))
                .clicked()
            {
                f.insert(ACTION.into(), json!("setup"));
            }
            if installed
                && widgets::secondary_button(ui, tr("Repair"), 80.0)
                    .on_hover_text(tr("Check every installed file against its checksum, and download again what is damaged or missing"))
                    .clicked()
            {
                f.insert(ACTION.into(), json!("setupRepair"));
            }
            if setup.get("managed").and_then(Value::as_bool) == Some(true) && widgets::secondary_button(ui, tr("Remove Local AI…"), 150.0).clicked() {
                f.insert(ACTION.into(), json!("setupRemove"));
            }
        });
        ui.add_space(10.0);
        widgets::section_label(ui, tr("Server"));
        ui.add_space(2.0);
    }
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
        // (key, title, tooltip, what Browse asks for)
        for (key, title, tip, browse) in [
            ("python", "Python", "The Python that has ComfyUI's packages: its virtual environment's, or the portable build's python_embeded", "browsePython"),
            ("folder", "Folder", "The folder ComfyUI is started in: the one with main.py, unless the arguments name the script with a path", "browseScript"),
        ] {
            label(ui, title);
            ui.horizontal(|ui| {
                let mut v = text_of(&launch, key);
                // "Not set", not a sample path: a sample reads as if the field were filled in.
                ui.add(egui::TextEdit::singleline(&mut v).desired_width(300.0).hint_text(tr("Not set"))).on_hover_text(tr(tip));
                launch.insert(key.into(), json!(v));
                // Python has no file type to filter by outside Windows: there the path is typed.
                if (key != "python" || cfg!(windows)) && ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, tr("Browse…"), 72.0)).inner.clicked() {
                    f.insert(ACTION.into(), json!(browse));
                }
            });
            ui.end_row();
        }
        label(ui, "Arguments");
        let mut v = text_of(&launch, "args");
        ui.add(egui::TextEdit::multiline(&mut v).desired_rows(3).desired_width(380.0).hint_text(tr("Not set (main.py)")))
            .on_hover_text(tr("The script and its options, as on ComfyUI's command line. Leave out --port: the address above decides it"));
        // One line: a pasted command line with line breaks is still one command.
        launch.insert("args".into(), json!(v.replace(['\r', '\n'], " ")));
        ui.end_row();
    });
    ai.insert("comfyLaunch".into(), Value::Object(launch));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let find = ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, tr("Find Installation"), 130.0)).inner;
        if find
            .on_hover_text(tr(
                "Fill these in: from the ComfyUI that is running now, or else from the one ComfyUI Desktop installed. For any other installation use Browse",
            ))
            .clicked()
        {
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
    // What Find, Browse or Stop did, where the eye is: right under the buttons.
    if let Some(status) = f.get(LAUNCH_STATUS).and_then(Value::as_object) {
        let error = status.get("error").and_then(Value::as_bool).unwrap_or(false);
        let text = status.get("text").and_then(Value::as_str).unwrap_or("");
        ui.add_space(2.0);
        ui.scope(|ui| {
            ui.set_max_width(520.0);
            ui.add(egui::Label::new(RichText::new(text).color(if error { t.danger } else { t.text_dim }).size(11.5)).wrap().selectable(true));
        });
    }
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

/// API Keys: one per cloud provider, each in a card that says which of the listed models it is
/// for, where to make one (a link to the provider's page) and what to know before doing so.
fn keys_page(ui: &mut egui::Ui, ai: &Map<String, Value>, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let mut keys = f.get(KEYS).and_then(Value::as_object).cloned().unwrap_or_default();
    let focus = f.get(KEY_FOCUS).and_then(Value::as_str).unwrap_or("").to_string();
    let mut focus_once = f.get(KEY_FOCUS_ONCE).and_then(Value::as_bool).unwrap_or(false);
    let models: Vec<Model> = ai.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(model_of).collect();
    let width = ui.available_width().min(560.0);
    let mut open = None;
    ui.scope(|ui| {
        ui.set_max_width(width);
        ui.add(
            egui::Label::new(
                RichText::new(tr(
                    "Cloud models run on their provider's servers, which bill each image to your account there. Each provider needs its own key: make one on its site and paste it here.",
                ))
                .color(t.text_dim),
            )
            .wrap(),
        );
    });
    ui.add_space(8.0);
    for p in Provider::ALL.into_iter().filter(|p| !p.is_local()) {
        let saved = keys.get(&format!("{}Saved", p.key())).and_then(Value::as_bool).unwrap_or(false);
        let mut key = text_of(&keys, p.key());
        let mut remove = keys.get(&format!("{}Remove", p.key())).and_then(Value::as_bool).unwrap_or(false);
        let (url, host, note) = key_source(p);
        let names: Vec<&str> = models.iter().filter(|m| m.provider == p).map(|m| m.name.as_str()).collect();
        let front = focus == p.key();
        let stroke = if front { egui::Stroke::new(1.5, t.accent) } else { egui::Stroke::new(1.0, t.card_border) };
        let card = egui::Frame::new().fill(t.card).stroke(stroke).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(width - 26.0);
            // The provider, whether it has a key, and where to get one.
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.label()).font(crate::theme::medium(13.0)).color(t.text));
                let (state, color) = if !key.trim().is_empty() {
                    (tr("Saved when you press OK"), t.text_dim)
                } else if saved && remove {
                    (tr("Removed when you press OK"), t.warning)
                } else if saved {
                    (tr("Key saved"), t.accent_text)
                } else {
                    (tr("No key"), t.warning)
                };
                ui.label(RichText::new(state).color(color).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let icon = crate::icons::button(ui, "external-link", 18.0, false, url);
                    let link = ui.add(
                        egui::Label::new(RichText::new(format!("{} {host}", tr("Get a key at"))).color(t.accent_text).size(11.5).underline())
                            .sense(egui::Sense::click()),
                    );
                    if icon.clicked() || link.on_hover_text(url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        open = Some(url);
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let hint = if saved { tr("Saved. Paste a new key to replace it") } else { tr("Paste your API key") };
                let field_width = if saved { ui.available_width() - 90.0 } else { ui.available_width() };
                let field = ui.add(egui::TextEdit::singleline(&mut key).password(true).desired_width(field_width.max(120.0)).hint_text(hint));
                if front && focus_once {
                    field.request_focus();
                    field.scroll_to_me(Some(egui::Align::Center));
                    focus_once = false;
                }
                if saved {
                    widgets::checkbox(ui, &mut remove, tr("Remove"));
                }
            });
            ui.add_space(4.0);
            let used = if names.is_empty() { tr("None of the listed models uses it.").to_string() } else { format!("{} {}", tr("For:"), names.join(", ")) };
            ui.add(egui::Label::new(RichText::new(used).color(t.text_dim).size(11.0)).wrap());
            ui.add(egui::Label::new(RichText::new(tr(note)).color(t.text_faint).size(11.0)).wrap());
        });
        if front && f.get(KEY_FOCUS_ONCE).and_then(Value::as_bool) == Some(true) {
            card.response.scroll_to_me(Some(egui::Align::Center));
        }
        keys.insert(p.key().into(), json!(key));
        keys.insert(format!("{}Remove", p.key()), json!(remove));
        ui.add_space(8.0);
    }
    f.insert(KEYS.into(), Value::Object(keys));
    f.insert(KEY_FOCUS_ONCE.into(), json!(focus_once));
    if let Some(url) = open {
        f.insert(OPEN_URL.into(), json!(url));
    }
    ui.label(
        RichText::new(tr("Keys are stored on this computer, apart from the preferences, and are sent only to their provider.")).color(t.text_faint).size(11.0),
    );
    let variables: Vec<&str> = Provider::ALL.into_iter().filter_map(Provider::key_env).collect();
    ui.scope(|ui| {
        ui.set_max_width(width);
        let text = format!("{} {}", tr("Without a saved key, a provider's environment variable is used:"), variables.join(", "));
        ui.add(egui::Label::new(RichText::new(text).color(t.text_faint).size(11.0)).wrap());
    });
}

/// A filter of the model list: a small pill with the filter's name and how many models it lists.
fn filter_chip(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let color = if on { t.text } else { t.text_dim };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(11.5), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 20.0), egui::Sense::click());
    let radius = rect.height() / 2.0;
    if on {
        ui.painter().rect_filled(rect, radius, t.row_selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, radius, t.hover);
    } else {
        ui.painter().rect_stroke(rect, radius, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Add the recommended models ([`Model::defaults`]) the draft doesn't list: a cloud model by its
/// provider and model id, a local one by what model its file is. Returns how many were added.
fn restore_defaults(ai: &mut Map<String, Value>, f: &mut Map<String, Value>) -> usize {
    let mut added = 0;
    for wanted in Model::defaults() {
        let listed = ai.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(model_of).any(|m| {
            if wanted.provider.is_local() {
                photocraft_aigen::detect::same_variant(&m, &wanted)
            } else {
                m.provider == wanted.provider && m.api_model.trim() == wanted.api_model
            }
        });
        if !listed {
            push_model(ai, f, wanted);
            added += 1;
        }
    }
    added
}

/// A switch: on or off at a click, drawn as one in every theme (the shared toggle is a
/// checkbox in the Pro themes).
fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(30.0, 17.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how_on = ui.ctx().animate_bool(resp.id, *on);
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, if *on { t.accent } else { t.field_border });
    if resp.hovered() {
        ui.painter().rect_stroke(rect, radius, egui::Stroke::new(1.0, t.text_faint), egui::StrokeKind::Outside);
    }
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), radius - 2.0, egui::Color32::WHITE);
    resp
}

/// One kind of model in the summary at the top of Manage Models: its name, how many there are,
/// and a switch that offers the kind in the AI Gen panel or leaves it out. What the kind is, and
/// what is worth knowing about it now, is the tooltip.
fn kind_switch(ui: &mut egui::Ui, ai: &mut Map<String, Value>, key: &str, title: &str, count: usize, tip: &str) {
    let t = Tokens::get(ui.ctx());
    let mut shown = ai.get(key).and_then(Value::as_bool).unwrap_or(true);
    let name = ui.label(RichText::new(tr(title)).color(if shown { t.text } else { t.text_faint }));
    let number = ui.label(RichText::new(count.to_string()).color(t.text_faint));
    let toggle = switch(ui, &mut shown);
    for resp in [name, number, toggle] {
        resp.on_hover_text(tip);
    }
    ai.insert(key.into(), json!(shown));
}

/// Move the model at `from` to where the model at `to` is (both positions in the draft's list),
/// the ones between shifting by one: how a model is moved up or down past its neighbour in the
/// list on screen, whatever the list is narrowed to.
fn move_model(models: &mut Vec<Value>, from: usize, to: usize) {
    if from < models.len() && to < models.len() && from != to {
        let model = models.remove(from);
        models.insert(to, model);
    }
}

/// Put the draft's models in the recommended order ([`Settings::sort_recommended`]): cloud first,
/// each kind best and newest first. Entries that don't read as models keep their place at the end.
fn sort_recommended(models: &mut Vec<Value>) {
    let (mut known, unread): (Vec<(Model, Value)>, Vec<Value>) = {
        let mut known = Vec::new();
        let mut unread = Vec::new();
        for v in models.drain(..) {
            match model_of(&v) {
                Some(m) => known.push((m, v)),
                None => unread.push(v),
            }
        }
        (known, unread)
    };
    known.sort_by_key(|(m, _)| (m.provider.is_local(), m.rank()));
    models.extend(known.into_iter().map(|(_, v)| v).chain(unread));
}

/// Manage Models: what is set up (cloud and local models, each with a switch for the AI Gen
/// panel), then every model in one list in the panel's order (cloud, then local), narrowed by
/// what the models are used for, and the selected one's settings beside it, down to `bottom`.
fn models_page(ui: &mut egui::Ui, ai: &mut Map<String, Value>, f: &mut Map<String, Value>, bottom: f32) {
    let t = Tokens::get(ui.ctx());
    // Local models pick files from the server's lists: read them once, when the server is on
    // this machine (it answers or refuses at once; a remote one waits for Connect).
    if f.get(FILES).is_none() && f.get(STATUS).is_none() && f.get(AUTO).is_none() {
        f.insert(AUTO.into(), json!(true));
        let url = text_of(ai, "comfyUrl");
        if url.contains("//127.0.0.1") || url.contains("//localhost") || url.contains("//[::1]") {
            f.insert(ACTION.into(), json!("peek"));
        }
    }
    let mut filter = Filter::from_key(f.get(FILTER).and_then(Value::as_str).unwrap_or(""));
    let page = ui.available_width();
    let page_right = ui.cursor().left() + page;

    // ----- what is set up, and Add Model -----
    let count =
        |local: bool| ai.get("models").and_then(Value::as_array).map_or(0, |ms| ms.iter().filter(|m| m.is_object() && is_local_model(m) == local).count());
    let (clouds, locals) = (count(false), count(true));
    // Which models can run now, and why the others can't.
    let standing = Standing::of(ai, f);
    // A cloud model without its key can't run: say so here.
    let missing: Vec<Provider> = Provider::ALL
        .into_iter()
        .filter(|p| !p.is_local() && !standing.keyed.contains(p))
        .filter(|p| ai.get("models").and_then(Value::as_array).is_some_and(|ms| ms.iter().any(|m| text_of_value(m, "provider") == p.key())))
        .collect();
    // What the last Connect / Scan said about the server.
    let status = f
        .get(STATUS)
        .and_then(Value::as_object)
        .map(|status| (status.get("text").and_then(Value::as_str).unwrap_or("").to_string(), status.get("error").and_then(Value::as_bool).unwrap_or(false)));
    let off = tr("Switched off, they are left out of the AI Gen panel's model lists; they stay set up.");
    let cloud_tip =
        format!("{} {off}", tr("Cloud models are run by their provider with your API key, and billed by it for each image. Best quality; nothing to install."));
    let local_tip = format!("{} {}. {off}", tr("Local models are run on this computer by ComfyUI, free of charge, at"), text_of(ai, "comfyUrl"));
    let add_width = 132.0;
    ui.horizontal(|ui| {
        kind_switch(ui, ai, "showCloud", "Cloud models", clouds, &cloud_tip);
        ui.add_space(18.0);
        kind_switch(ui, ai, "showLocal", "Local models", locals, &local_tip);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let add = widgets::primary_button(ui, &format!("+ {}", tr("Add Model")), add_width);
            egui::Popup::menu(&add).show(|ui| {
                widgets::menu_scroll(ui, |ui| {
                    for (heading, presets) in add_presets() {
                        ui.label(RichText::new(tr(heading)).color(t.text_faint).size(11.0));
                        for (title, preset) in presets {
                            if ui.button(title).clicked() {
                                push_model(ai, f, preset);
                                // Whatever the list was narrowed to, the new model is in view.
                                filter = Filter::All;
                                ui.close();
                            }
                        }
                        ui.add_space(4.0);
                    }
                });
            });
        });
    });
    // What needs doing before a model can run is said in the open, not in a tooltip: a provider
    // without its key, a server that could not be read.
    let problem = |ui: &mut egui::Ui, text: &str, color: egui::Color32| {
        ui.scope(|ui| {
            ui.set_max_width(page);
            ui.add(egui::Label::new(RichText::new(text).color(color).size(11.5)).wrap().selectable(true));
        });
    };
    if let Some(first) = missing.first().copied() {
        let names: Vec<&str> = missing.iter().map(|p| p.label()).collect();
        let mut add = false;
        ui.scope(|ui| {
            ui.set_max_width(page);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(egui::Label::new(RichText::new(format!("{}: {}.", tr("No API key yet for"), names.join(", "))).color(t.warning).size(11.5)));
                let link = ui.add(egui::Label::new(RichText::new(tr("Add API keys…")).color(t.accent_text).size(11.5).underline()).sense(egui::Sense::click()));
                add = link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            });
        });
        if add {
            go_to_key(f, first);
        }
    }
    if let Some((text, true)) = &status {
        problem(ui, text, t.danger);
    }
    ui.add_space(8.0);

    // ----- the filters -----
    let mut models = ai.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    let parsed: Vec<Option<Model>> = models.iter().map(model_of).collect();
    let lists = |filter: Filter, i: usize| models.get(i).is_some_and(Value::is_object) && filter.shows(parsed.get(i).and_then(Option::as_ref));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for x in Filter::ALL {
            let count = (0..models.len()).filter(|&i| lists(x, i)).count();
            if filter_chip(ui, &format!("{}  {count}", tr(x.title())), filter == x).clicked() {
                filter = x;
            }
        }
    });
    ui.scope(|ui| {
        ui.set_max_width(page);
        ui.add(egui::Label::new(RichText::new(tr(filter.hint())).color(t.text_faint).size(11.0)).wrap());
    });
    ui.add_space(6.0);

    // Cloud models first, then local, each in the order of the settings: the panel's order.
    let mut shown: Vec<usize> = Vec::new();
    for local in [false, true] {
        shown.extend((0..models.len()).filter(|&i| lists(filter, i) && models.get(i).is_some_and(is_local_model) == local));
    }
    // The selection is an index into all models; when the list doesn't show it, the list's first.
    let mut selected = f.get(SELECTED).and_then(Value::as_u64).map(|i| i as usize).filter(|i| shown.contains(i)).or_else(|| shown.first().copied());
    // The buttons under the list keep their row whatever the list holds.
    let footer = 34.0;
    let pane = (bottom - ui.cursor().top() - footer - 4.0).max(MIN_PANE_HEIGHT - footer);
    let files = f.get(FILES).and_then(Value::as_object).cloned();
    let (show_cloud, show_local) = (ai.get("showCloud").and_then(Value::as_bool).unwrap_or(true), ai.get("showLocal").and_then(Value::as_bool).unwrap_or(true));
    let mut remove = None;
    // A model moved up or down: (its position, the position of the neighbour it passes).
    let mut moved: Option<(usize, usize)> = None;
    // What a model that can't run asked to do about it (its row's button, or the banner's).
    let mut act: Option<Missing> = None;
    let selected_lack = selected.and_then(|i| parsed.get(i)).and_then(Option::as_ref).and_then(|m| standing.missing(m).map(|lack| (lack, m.clone())));
    if shown.is_empty() {
        let hint = match filter {
            Filter::All => "No models yet. Restore Defaults lists the recommended ones; Scan Server adds the ones your ComfyUI server has.",
            Filter::Create => "No text-to-image models yet. Restore Defaults lists the recommended ones.",
            Filter::Edit => "No editing models yet. Scan Server adds the ones your ComfyUI server has; cloud models edit too.",
            Filter::Inpaint => "No inpainting models yet. Add one with Add Model, or tick Supports inpainting on a model.",
        };
        ui.label(RichText::new(tr(hint)).color(t.text_faint));
        ui.add_space(6.0);
    } else {
        ui.horizontal_top(|ui| {
            // ----- the list -----
            ui.vertical(|ui| {
                ui.set_width(list_width(ui));
                egui::ScrollArea::vertical().id_salt("ai-model-list").max_height(pane).auto_shrink([false, false]).show(ui, |ui| {
                    let mut group = None;
                    for (at, &i) in shown.iter().enumerate() {
                        let local = models.get(i).is_some_and(is_local_model);
                        if group != Some(local) {
                            if group.is_some() {
                                ui.add_space(6.0);
                            }
                            group = Some(local);
                            let offered = if local { show_local } else { show_cloud };
                            let mut title = tr(if local { "Local" } else { "Cloud" }).to_string();
                            if !offered {
                                title = format!("{title} · {}", tr("not offered in the panel"));
                            }
                            ui.label(RichText::new(title).font(crate::theme::medium(11.0)).color(t.text_faint));
                        }
                        // The neighbours of its own kind in the list on screen: where Move Up and
                        // Move Down take it.
                        let neighbour = |step: isize| {
                            at.checked_add_signed(step).and_then(|n| shown.get(n)).copied().filter(|&n| models.get(n).is_some_and(is_local_model) == local)
                        };
                        let (above, below) = (neighbour(-1), neighbour(1));
                        let model = parsed.get(i).and_then(Option::as_ref);
                        // Can it run? A cloud model needs its key, a local one its files.
                        let lack = model.and_then(|m| standing.missing(m));
                        let Some(m) = models.get_mut(i).and_then(Value::as_object_mut) else { continue };
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::click());
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
                        // One that can't run can't be offered: its box is off and greyed, and its
                        // own choice is kept for when it can run.
                        if lack.is_none() {
                            widgets::checkbox(&mut check, &mut enabled, "").on_hover_text(tr("Show in the AI Gen panel"));
                            m.insert("enabled".into(), json!(enabled));
                        } else {
                            let mut off = false;
                            check
                                .add_enabled_ui(false, |ui| widgets::checkbox(ui, &mut off, ""))
                                .inner
                                .on_disabled_hover_text(tr("It can't run yet, so the AI Gen panel doesn't offer it"));
                        }
                        let name = text_of(m, "name");
                        // The way to make it run is always in view: it is what the row waits for.
                        // The row's own buttons show on the row in use, so a long list stays
                        // quiet: remove on the row under the pointer, and on the selected row the
                        // two that move it up and down the list (the order of the panel's list).
                        let fix = lack.as_ref().and_then(|lack| missing_action(lack, &name).map(|(icon, tip)| (lack, icon, tip)));
                        let mut used = 0.0;
                        if on || resp.hovered() || fix.is_some() {
                            let mut tools = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(egui::Rect::from_min_max(
                                        egui::pos2(rect.left() + 30.0, rect.top()),
                                        egui::pos2(rect.right() - 4.0, rect.bottom()),
                                    ))
                                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                            );
                            tools.spacing_mut().item_spacing.x = 0.0;
                            if let Some((lack, icon, tip)) = &fix {
                                if crate::icons::button(&mut tools, icon, 20.0, false, tip).clicked() {
                                    act = Some((*lack).clone());
                                }
                                used += 24.0;
                            }
                            if on || resp.hovered() {
                                if crate::icons::button(&mut tools, "trash", 20.0, false, tr("Remove this model")).clicked() {
                                    remove = Some(i);
                                }
                                used += 24.0;
                            }
                            if on {
                                let down = tools
                                    .add_enabled_ui(below.is_some(), |ui| {
                                        crate::icons::button(ui, "chevron-down", 20.0, false, tr("Move down: later in the AI Gen panel's model list"))
                                    })
                                    .inner;
                                let up = tools
                                    .add_enabled_ui(above.is_some(), |ui| {
                                        crate::icons::button(ui, "chevron-up", 20.0, false, tr("Move up: earlier in the AI Gen panel's model list"))
                                    })
                                    .inner;
                                if let (true, Some(to)) = (down.clicked(), below) {
                                    moved = Some((i, to));
                                }
                                if let (true, Some(to)) = (up.clicked(), above) {
                                    moved = Some((i, to));
                                }
                                used += 40.0;
                            }
                        }
                        let width = (rect.width() - 34.0 - used).max(20.0);
                        let line = |text: String, size: f32, color: egui::Color32| {
                            let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(size), color);
                            job.wrap = egui::text::TextWrapping::truncate_at_width(width);
                            ui.painter().layout_job(job)
                        };
                        let color = if enabled && lack.is_none() { t.text } else { t.text_faint };
                        let title = line(name.clone(), 12.5, color);
                        // What it is used for; or why it can't run yet.
                        let (caption, caption_color) = match (&lack, model) {
                            (Some(Missing::Key(p)), _) => (format!("{} {}", tr("Needs an API key from"), p.label()), t.warning),
                            (Some(Missing::Files(_)), _) => (tr("Not installed").to_string(), t.warning),
                            (Some(Missing::NoLocalAi(_)), _) => (tr("Local AI is not set up").to_string(), t.warning),
                            (None, Some(model)) => (uses(model).into_iter().map(tr).collect::<Vec<_>>().join(" · "), t.text_faint),
                            (None, None) => (String::new(), t.text_faint),
                        };
                        let under = line(caption.clone(), 10.5, caption_color);
                        let elided = title.elided || under.elided;
                        ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.top() + 4.0), title, color);
                        ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.top() + 20.0), under, caption_color);
                        if let (Some(Missing::Files(None)), Some(model)) = (&lack, model) {
                            resp.on_hover_text(format!(
                                "{name}\n{}: {}. {}",
                                tr("The ComfyUI server doesn't list"),
                                model.main_file(),
                                tr("Download it, press Scan Server, or choose another file")
                            ));
                        } else if elided {
                            resp.on_hover_text(format!("{name}\n{caption}"));
                        }
                    }
                });
            });
            widgets::vline(ui, pane);
            // ----- the selected model -----
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().id_salt("ai-model-settings").max_height(pane).auto_shrink([false, false]).show(ui, |ui| {
                    if let Some((lack, model)) = &selected_lack
                        && missing_banner(ui, lack, model)
                    {
                        act = Some(lack.clone());
                    }
                    if let Some(m) = selected.and_then(|i| models.get_mut(i)).and_then(Value::as_object_mut) {
                        model_settings(ui, m, files.as_ref(), selected.unwrap_or(0));
                    }
                });
            });
        });
    }
    if let Some(lack) = &act {
        act_on_missing(f, lack);
    }
    if let Some((from, to)) = moved {
        move_model(&mut models, from, to);
        selected = Some(to);
    }
    if let Some(i) = remove.filter(|i| *i < models.len()) {
        models.remove(i);
        // Stay near where the removed model was: the one listed after it, else the one before.
        let at = shown.iter().position(|x| *x == i).unwrap_or(0);
        let neighbour = shown.get(at + 1).or_else(|| at.checked_sub(1).and_then(|before| shown.get(before)));
        selected = neighbour.map(|&x| if x > i { x - 1 } else { x });
    }

    // ----- under the list: what fills and orders it -----
    ui.add_space(6.0);
    let mut reorder = false;
    let mut restore = false;
    ui.horizontal(|ui| {
        if widgets::secondary_button(ui, tr("Scan Server"), 104.0)
            .on_hover_text(tr("Add a model for every text-to-image, inpainting and edit model file the ComfyUI server has, and point the listed models at the files it has of them"))
            .clicked()
        {
            f.insert(ACTION.into(), json!("detect"));
        }
        restore = widgets::secondary_button(ui, tr("Restore Defaults"), 128.0)
            .on_hover_text(tr("Add the recommended models that are missing from the list. Nothing is removed or changed"))
            .clicked();
        reorder = widgets::secondary_button(ui, tr("Recommended Order"), 150.0)
            .on_hover_text(tr("Put the models in the recommended order: the best and newest first, cloud models before local ones. The selected model's arrows move one model up or down"))
            .clicked();
        // What the server last said, on one line (all of it in the tooltip); an error is shown
        // whole at the top of the page instead.
        if let Some((text, false)) = &status {
            let mut job = egui::text::LayoutJob::simple_singleline(text.clone(), egui::FontId::proportional(11.5), t.text_faint);
            job.wrap = egui::text::TextWrapping::truncate_at_width((page_right - ui.cursor().left() - 8.0).max(0.0));
            ui.add(egui::Label::new(job)).on_hover_text(text);
        }
    });
    if reorder {
        let was = selected.and_then(|i| models.get(i)).cloned();
        sort_recommended(&mut models);
        selected = was.and_then(|was| models.iter().position(|m| *m == was));
        filter = Filter::All;
    }
    ai.insert("models".into(), Value::Array(models));
    if restore {
        let added = restore_defaults(ai, f);
        let text = if added == 0 { tr("The recommended models are all listed.").to_string() } else { format!("{added} {}", tr("recommended models added")) };
        f.insert(STATUS.into(), json!({"error": false, "text": text}));
        filter = Filter::All;
    } else {
        // A list that shows nothing keeps the selection for when it shows it again.
        if let Some(i) = selected {
            f.insert(SELECTED.into(), json!(i));
        }
    }
    f.insert(FILTER.into(), json!(filter.key()));
}

/// What "Add Model" offers, in the groups its menu shows: (heading, [(menu title, model)]).
fn add_presets() -> Vec<(&'static str, Vec<(String, Model)>)> {
    let cloud = Model::cloud_presets().into_iter().map(|m| (format!("{} ({})", m.name, m.provider.label()), m)).collect();
    let local = Family::ALL.into_iter().filter(|f| !f.inpaint_only()).map(|f| (f.label().to_string(), f.preset())).collect();
    // The three models of ComfyUI's inpainting templates.
    let inpaint = [
        Family::Flux1Fill.preset(),
        Model { name: "FLUX.1 Fill OneReward".into(), unet: "flux.1-fill-dev-OneReward-transformer_fp8.safetensors".into(), ..Family::Flux1Fill.preset() },
        Family::QwenInpaint.preset(),
    ]
    .into_iter()
    .map(|m| (m.name.clone(), m))
    .collect();
    vec![("Cloud", cloud), ("Local", local), ("Local, made for inpainting", inpaint)]
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
        label(ui, "Used for");
        let used = model_of(&Value::Object(m.clone())).map(|model| uses(&model).into_iter().map(tr).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        ui.label(RichText::new(used).color(t.text)).on_hover_text(tr("What the AI Gen panel offers this model for: Create (text to image), or Edit (editing, inpainting)"));
        ui.end_row();
        if provider.is_local() {
            label(ui, "Architecture");
            let mut family = Family::from_key(&text_of(m, "family")).unwrap_or_default();
            let options: Vec<(Family, &str)> = Family::ALL.into_iter().map(|x| (x, x.label())).collect();
            widgets::dropdown(ui, "ai-family", tr("Architecture"), &mut family, &options, field_width(ui));
            m.insert("family".into(), json!(family.key()));
            ui.end_row();

            group(ui, "Files");
            if family == Family::Custom {
                label(ui, "Workflow");
                let mut text = text_of(m, "workflow");
                ui.vertical(|ui| {
                    ui.set_max_width(field_width(ui) + 8.0);
                    ui.add(egui::TextEdit::multiline(&mut text).desired_rows(6).desired_width(field_width(ui)).font(egui::TextStyle::Monospace).hint_text("{ \"1\": { \"class_type\": … } }"));
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
                if family == Family::HidreamO1 {
                    // Both are for a picture made from a prompt alone (the Create tab).
                    file_row(ui, m, "enhancer", "Prompt writer", folder("text_encoders"), true);
                    file_row(ui, m, "refiner", "Refine with", folder("diffusion_models"), true);
                    if !text_of(m, "refiner").is_empty() {
                        file_row(ui, m, "refinerClip", "Refiner text encoder", folder("text_encoders"), false);
                        file_row(ui, m, "refinerVae", "Refiner VAE", folder("vae"), false);
                        number_row(ui, m, "refineStrength", "Refine strength", 0.05..=1.0, false, "How far the refining pass may move from the picture. 0.35 adds skin and fabric texture and keeps the picture");
                    }
                }
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
                if family == Family::QwenInpaint {
                    file_row(ui, m, "controlnet", "ControlNet", folder("controlnet"), false);
                }
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
            let example = match provider {
                Provider::Openai => "gpt-image-2.5-sunburst",
                Provider::Xai => "grok-imagine-image-2.0",
                Provider::Meta => "muse-image-1.0",
                Provider::Bfl => "flux-3-image",
                Provider::Gemini | Provider::Comfy => "gemini-3-pro-image-preview",
            };
            text_row(ui, m, "apiModel", "Model ID", example);
            // What each provider lets the request choose.
            match provider {
                Provider::Openai => {
                    group(ui, "Output");
                    choice_row(ui, m, "quality", "Quality", &["auto", "low", "medium", "high", "xhigh", "max"]);
                    number_row(ui, m, "megapixels", "Render size (MP)", 0.0..=8.0, false, "About this many megapixels in the selection's shape (models that take any size). 0 follows the selection");
                }
                Provider::Xai => {
                    group(ui, "Output");
                    choice_row(ui, m, "quality", "Quality", &["auto", "low", "medium"]);
                    choice_row(ui, m, "imageSize", "Image size", &["1K", "2K"]);
                }
                // Muse Image has one quality and one size: the request only gives it a shape.
                Provider::Meta => {}
                Provider::Gemini | Provider::Bfl | Provider::Comfy => {
                    group(ui, "Output");
                    choice_row(ui, m, "imageSize", "Image size", &["1K", "2K", "4K"]);
                }
            }
        }
        label(ui, "Transparency");
        let mut alpha = m.get("alpha").and_then(Value::as_bool).unwrap_or(false);
        widgets::checkbox(ui, &mut alpha, tr("Can generate it")).on_hover_text(tr("The model can return a transparent background. Enables the Alpha Channel box in the AI Gen panel for it"));
        m.insert("alpha".into(), json!(alpha));
        ui.end_row();
        // A dedicated inpainting or edit model does nothing else: there is nothing to tick.
        if provider.is_local() && Family::from_key(&text_of(m, "family")).is_some_and(|f| f.inpaint_only() || f.edit_only()) {
            return;
        }
        label(ui, "Inpainting");
        // Until the box is used the model follows what its architecture or provider does well;
        // using it stores the choice.
        let was = serde_json::from_value::<Model>(Value::Object(m.clone())).map(|model| model.can_inpaint()).unwrap_or(false);
        let mut inpaint = was;
        widgets::checkbox(ui, &mut inpaint, tr("Supports inpainting")).on_hover_text(tr(
            "Let this model fill a selection (the AI Gen panel's Edit tab). On for FLUX.2, which is given the picture as a reference. Other models are not told where the selection is: they can be tried, but tend to paint a picture of their own inside it. The Inpainting filter of the list shows the models that have this on",
        ));
        if inpaint != was {
            m.insert("inpaint".into(), json!(inpaint));
        }
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
        let (added, repaired, matched) = add_detected(f, &folders);
        text = format!("{text} · {added} {}", tr("models added"));
        if repaired > 0 {
            text = format!("{text} · {repaired} {}", tr("repaired"));
        }
        if matched > 0 {
            text = format!("{text} · {matched} {}", tr("matched to the server's files"));
        }
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

fn launch_status(f: &mut Map<String, Value>, error: bool, text: &str) {
    f.insert(LAUNCH_STATUS.into(), json!({"error": error, "text": text}));
}

/// Browse…: ask for ComfyUI's Python (`python`) or its `main.py`, and fill the launch settings
/// of the open Preferences dialog from the choice when it is made (the file dialog answers on a
/// later frame, so the answer goes to whichever Preferences dialog is open then).
fn browse(app: &mut PhotocraftApp, f: &mut Map<String, Value>, python: bool) {
    let current: photocraft_aigen::Launch = serde_json::from_value(draft(f).get("comfyLaunch").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    // Start where the current choice is, else where the other half points.
    let near = |p: &str| std::path::Path::new(p.trim()).parent().filter(|d| !d.as_os_str().is_empty()).map(|d| d.to_string_lossy().into_owned());
    let initial_dir =
        if python { near(&current.python).or_else(|| Some(current.folder.clone())) } else { Some(current.folder.clone()).or_else(|| near(&current.python)) }
            .filter(|d| !d.trim().is_empty());
    let extensions = Some(vec![if python { "exe" } else { "py" }.to_string()]);
    let asked = app.ask_file(crate::file_dialog::FileDialogRequest::Open { multiple: false, initial_dir, extensions }, |app, answer| {
        let crate::file_dialog::FileDialogAnswer::Paths(paths) = answer else { return Ok(Value::Null) };
        let Some(path) = paths.first() else { return Ok(Value::Null) };
        let Some(d) = app.ui.dialogs.iter_mut().find(|d| crate::prefs_ui::is_preferences(&d.fields)) else { return Ok(Value::Null) };
        picked(&mut d.fields, path);
        Ok(Value::Null)
    });
    if let Err(e) = asked {
        launch_status(f, true, &format!("{}: {e}", tr("Could not open the file dialog")));
    }
}

/// The user chose `path` with Browse: fill the draft's launch settings from it and say what is
/// still missing.
fn picked(f: &mut Map<String, Value>, path: &str) {
    let current: photocraft_aigen::Launch = serde_json::from_value(draft(f).get("comfyLaunch").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    let launch = photocraft_aigen::launch::from_picked(path, &current, &|p| p.is_file());
    let missing = match (launch.python.trim().is_empty(), launch.folder.trim().is_empty()) {
        (true, _) => Some(tr("Now choose its Python with Browse (python.exe in ComfyUI's .venv or python_embeded folder).")),
        (_, true) => Some(tr("Now choose ComfyUI's main.py with Browse.")),
        _ => None,
    };
    if let (Some(ai), Ok(v)) =
        (f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")).and_then(Value::as_object_mut), serde_json::to_value(&launch))
    {
        ai.insert("comfyLaunch".into(), v);
    }
    match missing {
        Some(text) => launch_status(f, false, text),
        None => launch_status(f, false, tr("Python and folder are set. Switch on \"Start ComfyUI…\" above and press OK; Connect starts it now to check.")),
    }
}

/// After the dialog body: run what a button asked for (the body has no access to the app), and
/// follow a Connect / Scan that is running in the background.
pub fn after_body(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    f.insert(STARTED.into(), json!(app.session.ai.server.as_ref().is_some_and(|s| s.state() == photocraft_aigen::ServerState::Started)));
    if f.get(READ).is_none() {
        f.insert(READ.into(), json!(true));
        crate::ai_setup_ui::read_for_prefs(app);
    }
    f.insert(SETUP.into(), crate::ai_setup_ui::summary(app));
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
            Ok(mut found) => {
                // The launch settings alone: where it keeps its models is not one of them.
                if let Some(o) = found.as_object_mut() {
                    o.remove("modelsFolders");
                }
                let complete = ["python", "folder"].iter().all(|k| found.get(*k).and_then(Value::as_str).is_some_and(|v| !v.is_empty()));
                if let Some(ai) = f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")).and_then(Value::as_object_mut) {
                    ai.insert("comfyLaunch".into(), found);
                    if complete {
                        ai.insert("comfyAutostart".into(), json!(true));
                    }
                }
                let text = if complete {
                    tr("Found your ComfyUI and switched on starting it. Press OK to save: PhotoCraft will then start it whenever a local model needs it.")
                } else {
                    tr("Found its arguments, but not its Python and folder: choose them with Browse.")
                };
                launch_status(f, !complete, text);
            }
            Err(e) => launch_status(f, true, &e.to_string()),
        },
        "stop" => match app.session.execute("ai.server.stop", json!({})) {
            Ok(r) if r.get("stopped").and_then(Value::as_bool) == Some(true) => {
                launch_status(f, false, tr("Stopped the ComfyUI that PhotoCraft started."));
                f.insert(STARTED.into(), json!(false));
            }
            Ok(_) => launch_status(f, true, tr("PhotoCraft has not started a ComfyUI. One started elsewhere is left running.")),
            Err(e) => launch_status(f, true, &e.to_string()),
        },
        "browsePython" | "browseScript" => browse(app, f, action == "browsePython"),
        // The wizard is a window: the dialog is confirmed first, then it opens (see `ai_setup_ui`).
        "setup" => crate::ai_setup_ui::request_from_prefs(app, "open"),
        "setupRepair" => crate::ai_setup_ui::request_from_prefs(app, "repair"),
        "setupRemove" => crate::ai_setup_ui::request_from_prefs(app, "remove"),
        // A model's Install in Manage Models: the wizard, with it ticked.
        _ if action.starts_with("install:") => crate::ai_setup_ui::install_from_prefs(app, action.trim_start_matches("install:")),
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

/// Add the models detected in `folders` that the draft doesn't have yet; repair the ones it has
/// under the wrong architecture (the same model file, set up in a way that can't run); and point
/// a listed model whose own file the server doesn't have at the file it has of the same model
/// (a recommended model stored another way). Returns how many were added, repaired and matched.
fn add_detected(f: &mut Map<String, Value>, folders: &Value) -> (usize, usize, usize) {
    use photocraft_aigen::detect;
    let Ok(folders) = serde_json::from_value::<std::collections::BTreeMap<String, Vec<String>>>(folders.clone()) else { return (0, 0, 0) };
    let Some(ai) = f.get("values").and_then(|v| v.pointer("/integrations/ai")).and_then(Value::as_object).cloned() else { return (0, 0, 0) };
    let mut ai = ai;
    let (mut added, mut repaired, mut matched) = (0, 0, 0);
    for m in detect::detect(&folders) {
        // Read afresh each time: the model before this one may have changed the list.
        let now: Settings = serde_json::from_value(Value::Object(ai.clone())).unwrap_or_else(|_| Settings::default_without_models());
        let same_file = |x: &&Model| x.provider.is_local() && x.unet == m.unet && x.checkpoint == m.checkpoint;
        if now.models.iter().filter(same_file).any(|x| x.family == m.family && detect::problem(x).is_none()) {
            continue;
        }
        // The file is listed, but in a way that is known not to work: fix that entry in place
        // instead of adding a second one beside it.
        let broken = now.models.iter().position(|x| same_file(&x) && detect::problem(x).is_some());
        let stray = || now.models.iter().position(|x| detect::same_variant(x, &m) && !detect::on_server(&folders, x));
        let (at, fixed) = match broken {
            Some(i) => (Some(i), now.models.get(i).map(|x| detect::repaired(x, &m))),
            None => {
                let i = stray();
                (i, i.and_then(|i| now.models.get(i)).map(|x| detect::adopted(x, &m)))
            }
        };
        let slot = at.and_then(|i| ai.get_mut("models").and_then(Value::as_array_mut).and_then(|list| list.get_mut(i)));
        match (slot, fixed.and_then(|model| serde_json::to_value(model).ok())) {
            (Some(slot), Some(fixed)) => {
                *slot = fixed;
                if broken.is_some() { repaired += 1 } else { matched += 1 }
            }
            _ => {
                push_model(&mut ai, f, m);
                added += 1;
            }
        }
    }
    if let Some(slot) = f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")) {
        *slot = Value::Object(ai);
    }
    (added, repaired, matched)
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

mod refs;
pub mod regions;
#[cfg(test)]
mod tests;
