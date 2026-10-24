//! The Local AI setup wizard: File › Set Up Local AI…
//!
//! Five screens that take someone who has never heard of ComfyUI from "no Python on this PC" to
//! a first generated image: **Your PC** (what the machine has; nothing is changed), **Location**
//! (where the engine and the models go), **Models** (which ones, with sizes and licences),
//! **Install** (the downloads, which carry on with the window closed) and **Ready** (the test
//! image). "Use my ComfyUI" connects a ComfyUI the user already has and only fetches the model
//! files it lacks; "Cloud models only" skips local models altogether.
//!
//! The screens are thin: what they show comes from the engine's `ai.setup.*` commands, and what
//! they do is run them. The wizard is a window, not a dialog: the editor stays usable beside it.
//! Its state ([`AiSetupUi`]) is part of `UiState`, so `ui.inspect` reports it and a snapshot
//! script can put any screen in front.
//!
//! The strings go through [`tr`], like the AI Gen panel's: they fall back to English until the
//! catalogs gain them, and the product's name in them is this build's ([`crate::brand`]).

use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use photocraft_aisetup::probe::Stage;

use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// File › Set Up Local AI…: open the wizard. `{"page": "pc|location|models|install|ready"}`
/// puts a screen in front, `{"route": "managed|existing"}` chooses who installs ComfyUI,
/// `{"open": false}` closes it.
pub const OPEN: &str = "ai.setup.open";
const START: &str = "ai.setup.start";
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 600.0;
const SIDEBAR: f32 = 210.0;
/// The two big buttons above the footer of the first screen: the user's ComfyUI, or a new one.
const CHOICE: f32 = 96.0;
/// How often the running setup's progress is read.
const POLL_SECONDS: f64 = 0.25;

/// The wizard's screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Page {
    #[default]
    Pc,
    Location,
    Models,
    Install,
    Ready,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Pc, Page::Location, Page::Models, Page::Install, Page::Ready];

    fn key(self) -> &'static str {
        match self {
            Page::Pc => "pc",
            Page::Location => "location",
            Page::Models => "models",
            Page::Install => "install",
            Page::Ready => "ready",
        }
    }

    fn label(self, existing: bool) -> &'static str {
        match self {
            Page::Pc => "Your PC",
            Page::Location if existing => "Your ComfyUI",
            Page::Location => "Location",
            Page::Models => "Models",
            Page::Install => "Install",
            Page::Ready => "Ready",
        }
    }
}

/// A question over the wizard, until it is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ask {
    /// Install was pressed with a gated model ticked that can't be downloaded yet: sign in with
    /// Hugging Face and accept its licence, or install without it.
    SignIn,
    /// Close was pressed while a setup runs: keep it going in the background, or stop it.
    Close,
    /// Connect found no ComfyUI at the address, nor on ComfyUI's other port: start it, and try
    /// again.
    NotRunning,
    /// The setup stopped with an error: the whole of it, to read and to copy. It opens when a
    /// setup fails with the wizard open; Show error, or the failed row, opens it again.
    Error,
    /// What's happening? on the engine's step: what it is doing, for how long, and its own last
    /// lines, live.
    Activity,
}

/// The wizard's state (in `UiState`, so `ui.inspect` reports it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSetupUi {
    pub open: bool,
    pub page: Page,
    /// "Use my ComfyUI": nothing is installed but the model files it lacks.
    pub existing: bool,
    /// No graphics card the engine can use, and "Install anyway" is ticked: a new ComfyUI is
    /// installed to run on the processor.
    pub cpu: bool,
    /// The preset in front (`starter`, `recommended`, `everything`), or `custom`.
    pub preset: String,
    /// The ticked models (manifest ids).
    pub models: Vec<String>,
    pub engine_dir: String,
    pub models_dir: String,
    /// Use my ComfyUI: its address.
    pub url: String,
    /// What the engine last said: `ai.setup.probe`, `.manifest`, `.plan` and `.status`.
    pub probe: Option<Value>,
    pub manifest: Option<Value>,
    pub plan: Option<Value>,
    pub status: Option<Value>,
    /// Use my ComfyUI: what Connect found (`{folders, launch, local}`).
    pub server: Option<Value>,
    /// Use my ComfyUI: the folders it reads model files from, the one it saves new ones to
    /// first, when they could be found (from its settings, or from it running).
    pub folders: Vec<String>,
    /// What went wrong with the last thing asked for, shown where the eye is.
    pub note: String,
    /// The probe, or the look at the user's server, running in the background.
    pub probe_job: Option<u64>,
    pub server_job: Option<u64>,
    /// Remove local AI… is asking to be confirmed.
    pub confirm_remove: bool,
    /// The installed model whose files are asked to be deleted, until that is confirmed.
    pub remove_model: Option<String>,
    /// A local model rendered in this session: local AI works, however it was set up.
    pub local_worked: bool,
    /// Hugging Face, for the gated models: `ai.setup.hfStatus` (signed in, as whom; never a
    /// token) and `ai.setup.hfAccess` (where each ticked gated model stands), and that check
    /// running in the background.
    pub hf: Option<Value>,
    pub access: Option<Value>,
    pub access_job: Option<u64>,
    /// A question over the wizard.
    pub ask: Option<Ask>,
    /// "Use an access token instead" is open.
    pub token_open: bool,
    /// Asked for from the Preferences dialog, which is confirmed first (a window can't take
    /// input beside a dialog, and the dialog's draft would overwrite what a setup writes).
    #[serde(skip)]
    from_prefs: Option<String>,
    /// The token being typed. Never part of what `ui.inspect` reports.
    #[serde(skip)]
    token: String,
    /// What the plan in `plan` was asked for, so it is asked again only when that changes, and
    /// why it could not be made, when it could not.
    #[serde(skip)]
    plan_key: String,
    #[serde(skip)]
    plan_note: String,
    /// The setups the session has seen end, and when the status was last read.
    #[serde(skip)]
    seen_seq: u64,
    #[serde(skip)]
    status_at: f64,
    /// What `access` was asked for (the models and the sign-in), so it is asked again when that
    /// changes; the sign-in changes seen; and whether the app had the focus last frame (coming
    /// back from the browser checks again).
    #[serde(skip)]
    access_key: String,
    #[serde(skip)]
    hf_seen: u64,
    #[serde(skip)]
    focused: bool,
    /// The Install screen's active row last brought into view, so it is scrolled to once.
    #[serde(skip)]
    shown_active: String,
}

impl Default for AiSetupUi {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Pc,
            existing: false,
            cpu: false,
            preset: "recommended".into(),
            models: Vec::new(),
            engine_dir: String::new(),
            models_dir: String::new(),
            url: String::new(),
            probe: None,
            manifest: None,
            plan: None,
            status: None,
            server: None,
            folders: Vec::new(),
            note: String::new(),
            probe_job: None,
            server_job: None,
            confirm_remove: false,
            remove_model: None,
            local_worked: false,
            hf: None,
            access: None,
            access_job: None,
            ask: None,
            token_open: false,
            from_prefs: None,
            token: String::new(),
            plan_key: String::new(),
            plan_note: String::new(),
            seen_seq: 0,
            status_at: f64::NEG_INFINITY,
            access_key: String::new(),
            hf_seen: 0,
            focused: true,
            shown_active: String::new(),
        }
    }
}

fn tr(s: &str) -> &str {
    crate::i18n::t(s)
}

/// Menu ids handled here.
pub fn handles(id: &str) -> bool {
    id == OPEN
}

/// Can this build set up local AI at all (the desktop app, with its network and its folders)?
pub fn available(app: &PhotocraftApp) -> bool {
    app.session.is_enabled(START)
}

/// `bytes` as people read sizes: `18.5 GB`.
fn size(bytes: u64) -> String {
    photocraft_aisetup::human_bytes(bytes)
}

/// A download's size to a tenth of a gigabyte (`176.5 GB`), as the model lists give them; small
/// ones as [`size`] does.
fn download_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { size(bytes) }
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn number(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn list<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Is a setup running?
pub fn running(app: &PhotocraftApp) -> bool {
    app.session.ai_setup.running()
}

/// The running setup as a fraction and a line of text: for the status bar and the AI Gen panel.
pub fn progress_line(app: &PhotocraftApp) -> Option<(f32, String)> {
    if !running(app) {
        return None;
    }
    let status = app.ui.ai_setup.status.as_ref()?;
    let (done, total) = (number(status, "/progress/bytesDone"), number(status, "/progress/bytesTotal"));
    let fraction = if total > 0 { (done as f64 / total as f64) as f32 } else { 0.0 };
    let message = text(status, "/progress/message");
    let line = if total > 0 && done < total {
        format!("{} {} / {}", tr("Setting up local AI:"), size(done), size(total))
    } else if message.is_empty() {
        tr("Setting up local AI…").to_string()
    } else {
        format!("{} {}", tr("Setting up local AI:"), tr(message))
    };
    Some((fraction.clamp(0.0, 1.0), line))
}

/// Has local AI been set up: by the wizard, by hand in the Preferences, or simply by a ComfyUI
/// the user starts themselves, which a local generation in this session has shown to work?
pub fn set_up(app: &PhotocraftApp) -> bool {
    let ai = &app.session.prefs().integrations.ai;
    let launch = !ai.comfy_launch.python.trim().is_empty() && !ai.comfy_launch.folder.trim().is_empty();
    launch || ai.comfy_autostart || app.ui.ai_setup.local_worked || app.ui.ai_setup.status.as_ref().is_some_and(|s| text(s, "/state") == "done")
}

fn refresh_status(app: &mut PhotocraftApp) {
    app.ui.ai_setup.status = app.session.execute("ai.setup.status", json!({})).ok();
}

fn refresh_manifest(app: &mut PhotocraftApp) {
    let vram = app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("vramGb")).cloned().unwrap_or(Value::Null);
    app.ui.ai_setup.manifest = app.session.execute("ai.setup.manifest", json!({"vramGb": vram})).ok();
}

/// The model ids of a preset, from the manifest.
fn preset_models(app: &PhotocraftApp, preset: &str) -> Vec<String> {
    let Some(m) = &app.ui.ai_setup.manifest else { return Vec::new() };
    list(m, &format!("/presets/{preset}/models")).iter().filter_map(Value::as_str).map(str::to_string).collect()
}

/// Look at this PC, in the background where the app has jobs.
fn start_probe(app: &mut PhotocraftApp) {
    // An answer that is there is kept until Check Again asks for a new one.
    if app.ui.ai_setup.probe_job.is_some() || app.ui.ai_setup.probe.is_some() || !available(app) {
        return;
    }
    let started = if app.background_jobs {
        app.session.start("ai.setup.probe", json!({}))
    } else {
        app.session.execute("ai.setup.probe", json!({})).map(photocraft_engine::jobs::Started::Done)
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Done(found)) => probed(app, found),
        Ok(photocraft_engine::jobs::Started::Job(id)) => app.ui.ai_setup.probe_job = Some(id.0),
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Fill in what the probe suggests where nothing is chosen yet: the folders, and the address of
/// a ComfyUI that is running.
fn suggest(app: &mut PhotocraftApp) {
    let s = &mut app.ui.ai_setup;
    let Some(found) = s.probe.clone() else { return };
    if s.engine_dir.trim().is_empty() {
        s.engine_dir = text(&found, "/defaults/engineDir").to_string();
    }
    if s.models_dir.trim().is_empty() && !s.existing {
        s.models_dir = text(&found, "/defaults/modelsDir").to_string();
    }
    if let Some(running) = list(&found, "/probe/comfy/running").first().and_then(Value::as_str).filter(|_| s.url.trim().is_empty()) {
        s.url = running.to_string();
    }
}

/// The probe answered: keep it. A selection the user has not touched yet becomes the preset
/// suggested for the card (Light on a small one).
fn probed(app: &mut PhotocraftApp, found: Value) {
    app.ui.ai_setup.probe = Some(found);
    suggest(app);
    let untouched = app.ui.ai_setup.models.is_empty() || app.ui.ai_setup.models == preset_models(app, &app.ui.ai_setup.preset);
    let default = app.ui.ai_setup.preset == AiSetupUi::default().preset;
    refresh_manifest(app);
    if untouched && default {
        let preset = suggested_preset(app);
        app.ui.ai_setup.models = preset_models(app, &preset);
        app.ui.ai_setup.preset = preset;
    }
}

/// The preset the manifest suggests for this PC's card.
fn suggested_preset(app: &PhotocraftApp) -> String {
    app.ui.ai_setup.manifest.as_ref().map(|m| text(m, "/suggested")).filter(|p| !p.is_empty()).unwrap_or("recommended").to_string()
}

/// Open the wizard on the screen that fits where things stand.
pub fn open(app: &mut PhotocraftApp) {
    show(app, true);
}

/// Open the wizard; `recall` starts it from the models the last setup chose.
fn show(app: &mut PhotocraftApp, recall: bool) {
    refresh_status(app);
    if app.ui.ai_setup.manifest.is_none() {
        refresh_manifest(app);
    }
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let s = &mut app.ui.ai_setup;
    (s.open, s.confirm_remove, s.remove_model) = (true, false, None);
    s.note.clear();
    // What the last setup chose is where this one starts.
    if s.engine_dir.trim().is_empty() {
        s.engine_dir = text(&status, "/record/engineDir").to_string();
    }
    if s.models_dir.trim().is_empty() {
        s.models_dir = text(&status, "/record/modelsDir").to_string();
    }
    let recorded: Vec<String> = list(&status, "/record/models").iter().filter_map(Value::as_str).map(str::to_string).collect();
    let state = text(&status, "/state").to_string();
    if recall && !recorded.is_empty() && s.models.is_empty() {
        s.models = recorded;
        s.preset = "custom".into();
        s.existing = text(&status, "/record/route") == "existing";
    }
    s.page = match state.as_str() {
        "running" => Page::Install,
        _ => Page::Pc,
    };
    if app.ui.ai_setup.models.is_empty() {
        if app.ui.ai_setup.preset == AiSetupUi::default().preset {
            app.ui.ai_setup.preset = suggested_preset(app);
        }
        let preset = app.ui.ai_setup.preset.clone();
        app.ui.ai_setup.models = preset_models(app, &preset);
    }
    name_preset(app);
    suggest(app);
    if app.ui.ai_setup.url.trim().is_empty() {
        app.ui.ai_setup.url = app.session.prefs().integrations.ai.comfy_url.clone();
    }
    app.ui.ai_setup.ask = None;
    refresh_hf(app);
    start_probe(app);
}

/// The wizard from its first screen with the route and models chosen so far forgotten, so it
/// suggests afresh: what the AI Gen panel's "Run Setup Wizard Again…" does. The folders of the
/// last setup are kept (that is where its ComfyUI is), and so is what the engine said.
pub fn restart(app: &mut PhotocraftApp) {
    let s = &mut app.ui.ai_setup;
    let fresh = AiSetupUi::default();
    (s.existing, s.cpu, s.preset, s.models) = (fresh.existing, fresh.cpu, fresh.preset, fresh.models);
    (s.url, s.folders, s.server, s.token_open) = (fresh.url, fresh.folders, fresh.server, false);
    show(app, false);
}

/// The wizard on its Install screen, as the running setup's progress: what clicking the status
/// bar's readout (or the AI Gen panel's line) brings back.
pub fn show_progress(app: &mut PhotocraftApp) {
    open(app);
    app.ui.ai_setup.page = Page::Install;
}

/// The first launch after installing: offer the wizard once, when nothing is set up yet. The
/// app shell calls this once the session has its network and its folders.
pub fn offer_at_first_launch(app: &mut PhotocraftApp) {
    if !available(app) {
        return;
    }
    refresh_status(app);
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let offered = status.pointer("/record/offered").and_then(Value::as_bool).unwrap_or(true);
    // Interrupted last time: offered again, so it can be continued.
    let interrupted = matches!(text(&status, "/state"), "interrupted" | "paused" | "failed");
    if interrupted || (!offered && !set_up(app)) {
        open(app);
    }
    #[cfg(not(target_arch = "wasm32"))]
    app.session.ai_setup.mark_offered();
}

pub fn menu(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != OPEN {
        return None;
    }
    if !available(app) {
        return Some(Err(tr("Setting up local AI needs the desktop app").to_string()));
    }
    if params.get("open").and_then(Value::as_bool) == Some(false) {
        app.ui.ai_setup.open = false;
        return Some(Ok(json!({"open": false})));
    }
    open(app);
    match params.get("route").and_then(Value::as_str) {
        Some("existing") => app.ui.ai_setup.existing = true,
        Some("managed") => app.ui.ai_setup.existing = false,
        _ => {}
    }
    if let Some(page) = params.get("page").and_then(Value::as_str).and_then(|key| Page::ALL.into_iter().find(|p| p.key() == key)) {
        app.ui.ai_setup.page = page;
    }
    Some(Ok(json!({"open": true, "page": app.ui.ai_setup.page.key()})))
}

/// File › Set Up Local AI…, in a group of its own above File Info. Not in the web build, which
/// has no disk to install to.
pub fn insert_menu_items(app: &PhotocraftApp, items: &mut Vec<crate::menus::MenuItem>) {
    if cfg!(target_arch = "wasm32") || items.iter().any(|i| i.id == OPEN) {
        return;
    }
    let Some(at) = items.iter().position(|i| i.id == "file.fileInfo") else { return };
    let path = vec!["File".to_string()];
    let label = if set_up(app) { "Local AI Setup…" } else { "Set Up Local AI…" };
    let item = |id: &str, label: &str, enabled| crate::menus::MenuItem {
        id: id.into(),
        label: label.into(),
        path: path.clone(),
        shortcut: None,
        enabled,
        checked: None,
        color: None,
    };
    items.insert(at, item("---", "---", false));
    items.insert(at, item(OPEN, label, available(app)));
}

/// What the Preferences page asks for: open the wizard (`open`), or on a screen that does one
/// thing (`repair`, `remove`). The dialog is confirmed first, in [`window`].
pub fn request_from_prefs(app: &mut PhotocraftApp, what: &str) {
    app.ui.ai_setup.from_prefs = Some(what.to_string());
}

/// [`request_from_prefs`] for one model: the wizard on its Models screen with the model of this
/// manifest id ticked, ready to install.
pub fn install_from_prefs(app: &mut PhotocraftApp, id: &str) {
    request_from_prefs(app, &format!("{INSTALL}{id}"));
}

const INSTALL: &str = "install:";

/// For the Preferences page: a line saying what is installed, and what Manage Models needs to
/// tell installed models from the others (the installed engine's files, the wizard's models).
pub fn summary(app: &PhotocraftApp) -> Value {
    let Some(status) = &app.ui.ai_setup.status else { return json!({"available": available(app), "text": "", "catalog": catalog(app)}) };
    let installed = list(status, "/installedNow").len();
    let engine = list(status, "/engines").first().map(|e| text(e, "/version").to_string());
    let line = match (text(status, "/state"), engine) {
        ("running", _) => tr("A setup is running.").to_string(),
        ("interrupted" | "paused" | "failed", _) => tr("A setup was interrupted. Open the wizard to continue it.").to_string(),
        (_, Some(version)) => format!("{} {version} · {installed} {} · {}", tr("Installed: ComfyUI"), tr("models"), text(status, "/record/modelsDir")),
        ("done", None) => format!("{} · {installed} {}", tr("Using your own ComfyUI"), tr("of the recommended models")),
        _ => tr("Not set up yet. The wizard installs ComfyUI and the recommended models for you.").to_string(),
    };
    json!({
        "available": available(app), "text": line, "installed": text(status, "/state") == "done", "managed": !list(status, "/engines").is_empty(),
        "modelFiles": status.get("modelFiles").cloned().unwrap_or(Value::Null), "catalog": catalog(app),
    })
}

/// The model files of the engine this app installed, by folder: what that ComfyUI has when it is
/// started, known while it isn't running. `None` for the user's own ComfyUI, or before a setup.
pub fn installed_files(app: &PhotocraftApp) -> Option<std::collections::BTreeMap<String, Vec<String>>> {
    let files = app.ui.ai_setup.status.as_ref()?.get("modelFiles")?;
    serde_json::from_value(files.clone()).ok()
}

/// The wizard's models, each with its main file (the first it lists) and its download size:
/// `[{id, name, main, size}]`, for Manage Models to offer to install a model it lists.
fn catalog(app: &PhotocraftApp) -> Vec<Value> {
    let Some(manifest) = &app.ui.ai_setup.manifest else { return Vec::new() };
    list(manifest, "/models")
        .iter()
        .map(|m| {
            let main = list(m, "/files").first().map_or("", |f| text(f, "/key").rsplit('/').next().unwrap_or(""));
            json!({"id": text(m, "/id"), "name": text(m, "/name"), "main": main, "size": download_size(number(m, "/bytes"))})
        })
        .collect()
}

/// Read what [`summary`] tells Manage Models, once, without opening the wizard: the setup's
/// status and the wizard's models.
pub fn read_for_prefs(app: &mut PhotocraftApp) {
    if !available(app) {
        return;
    }
    if app.ui.ai_setup.status.is_none() {
        refresh_status(app);
    }
    if app.ui.ai_setup.manifest.is_none() {
        refresh_manifest(app);
    }
}

// ---------- every frame ----------

/// Per frame: follow the probe and the running setup, and react when one ends.
pub fn tick(app: &mut PhotocraftApp, ctx: &egui::Context) {
    // The Preferences dialog asked: confirm it (its draft would otherwise overwrite what a
    // setup writes), then open the wizard.
    if let Some(what) = app.ui.ai_setup.from_prefs.take() {
        let dialog = app.ui.dialogs.iter().find(|d| crate::prefs_ui::is_preferences(&d.fields)).map(|d| d.id);
        match dialog.map(|id| crate::dialogs::confirm(app, id)) {
            Some(Err(e)) => crate::notices::error(app, format!("{}: {e}", tr("Preferences"))),
            _ => {
                open(app);
                match what.as_str() {
                    "repair" => repair(app),
                    "remove" => app.ui.ai_setup.confirm_remove = true,
                    _ => {
                        // A model's Install in Manage Models: on the Models screen with it ticked.
                        if let Some(id) = what.strip_prefix(INSTALL) {
                            if !app.ui.ai_setup.models.iter().any(|m| m == id) {
                                toggle_model(app, id, true);
                            }
                            if !running(app) {
                                app.ui.ai_setup.page = Page::Models;
                            }
                        }
                    }
                }
            }
        }
    }
    let now = ctx.input(|i| i.time);
    for (job, which) in [(app.ui.ai_setup.probe_job, 0), (app.ui.ai_setup.server_job, 1), (app.ui.ai_setup.access_job, 2)] {
        let Some(id) = job else { continue };
        match app.session.jobs_with_recent().into_iter().rev().find(|j| j.id.0 == id) {
            Some(j) if j.state == "running" => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            ended => {
                let (result, error) = match ended {
                    Some(j) if j.state == "done" => (j.result, None),
                    Some(j) => (None, j.error),
                    None => (None, None),
                };
                match which {
                    0 => {
                        app.ui.ai_setup.probe_job = None;
                        match result {
                            Some(found) => probed(app, found),
                            None => app.ui.ai_setup.note = tr("This PC could not be checked. Press Check Again.").to_string(),
                        }
                    }
                    1 => {
                        app.ui.ai_setup.server_job = None;
                        connected(app, result);
                    }
                    _ => {
                        app.ui.ai_setup.access_job = None;
                        if result.is_none() {
                            app.ui.ai_setup.note = error.unwrap_or_else(|| tr("Hugging Face could not be asked. Press Check again.").to_string());
                        }
                        accessed(app, result);
                    }
                }
            }
        }
    }
    // Hugging Face: a sign-in that ended (or anything else that changed it) is saved and shown,
    // and the ticked gated models are asked about again.
    let hf_seq = app.session.ai_setup.hf_seq();
    if hf_seq != app.ui.ai_setup.hf_seen {
        app.ui.ai_setup.hf_seen = hf_seq;
        refresh_hf(app);
        let error = app.ui.ai_setup.hf.as_ref().map(|h| text(h, "/error").to_string()).unwrap_or_default();
        if !error.is_empty() {
            app.ui.ai_setup.note = error;
        }
    }
    if app.session.ai.take_keys_dirty() {
        save_keys(app);
    }
    if app.session.ai_setup.signing_in() {
        // The browser is out: notice it coming back.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    // Back from the browser (the licence accepted there, say): ask again.
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    let regained = focused && !app.ui.ai_setup.focused;
    app.ui.ai_setup.focused = focused;
    let asking = app.ui.ai_setup.open && (app.ui.ai_setup.page == Page::Models || app.ui.ai_setup.ask == Some(Ask::SignIn));
    if regained && asking && !all_ready(app) {
        check_access(app);
    }
    // A setup that has just ended is no longer running: its result is collected here, where its
    // settings are applied, whether or not the wizard is open.
    #[cfg(not(target_arch = "wasm32"))]
    photocraft_engine::ai_setup_cmds::poll(&mut app.session);
    if running(app) {
        if now - app.ui.ai_setup.status_at >= POLL_SECONDS {
            app.ui.ai_setup.status_at = now;
            refresh_status(app);
        }
        // Quarter-second steps: enough for a progress bar, and no load on an hour of downloads.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECONDS));
    }
    let seq = app.session.ai_setup.seq();
    if seq != app.ui.ai_setup.seen_seq {
        app.ui.ai_setup.seen_seq = seq;
        finished(app, ctx);
    }
}

/// A setup ended (or something was removed): show how.
fn finished(app: &mut PhotocraftApp, ctx: &egui::Context) {
    refresh_status(app);
    refresh_manifest(app);
    app.ui.ai_setup.plan_key.clear();
    forget_test_image(ctx);
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let Some(result) = status.get("result").filter(|r| !r.is_null()) else { return };
    let (ok, cancelled) = (result.get("ok").and_then(Value::as_bool) == Some(true), result.get("cancelled").and_then(Value::as_bool) == Some(true));
    if ok {
        app.ui.ai_setup.page = Page::Ready;
        app.ui.status = tr("Local AI is set up").to_string();
        app.ui.status_error = false;
        // Said where it will be seen: the window may have been closed for the download.
        if !app.ui.ai_setup.open {
            crate::notices::post(app, tr("Local AI is ready"), vec![tr("Open Window › AI Gen to generate your first image.").to_string()], false, None);
        }
    } else if !cancelled {
        app.ui.ai_setup.page = Page::Install;
        if !app.ui.ai_setup.open {
            crate::notices::error(app, format!("{}: {}", tr("Local AI setup stopped"), text(result, "/error")));
        } else if app.ui.ai_setup.ask.is_none() {
            // The whole of what went wrong, where it can be read and copied.
            app.ui.ai_setup.ask = Some(Ask::Error);
        }
    }
}

// ---------- actions ----------

fn params(app: &PhotocraftApp) -> Value {
    let s = &app.ui.ai_setup;
    let mut p = json!({"models": s.models, "modelsDir": s.models_dir});
    if s.existing {
        p["route"] = json!("existing");
        p["url"] = json!(s.url.trim());
        let server = s.server.clone().unwrap_or(Value::Null);
        p["serverFiles"] = server.get("folders").cloned().unwrap_or(json!({}));
        if let Some(launch) = server.get("launch").filter(|l| l.is_object()) {
            p["launch"] = launch.clone();
        }
        // A server on another computer has no folder here to download into.
        if server.get("local").and_then(Value::as_bool) == Some(false) {
            p["models"] = json!([]);
            p["modelsDir"] = Value::Null;
        }
    } else {
        p["route"] = json!("managed");
        p["engineDir"] = json!(s.engine_dir);
        p["variant"] = json!(s.probe.as_ref().map_or("nvidia", |v| text(v, "/variant")));
        // Without a card it can use, the engine runs on the processor (Install anyway was ticked).
        p["cpu"] = json!(s.probe.as_ref().and_then(|v| v.get("gpuUsable")).and_then(Value::as_bool) == Some(false));
    }
    p
}

/// Ask the engine what the choices would download, when they changed.
fn replan(app: &mut PhotocraftApp) {
    let p = params(app);
    // A token saved since, or a file removed, changes the plan as much as the choices do.
    let signed_in = app.ui.ai_setup.status.as_ref().and_then(|s| s.get("token")).and_then(Value::as_bool).unwrap_or(false);
    let key = format!("{p}|{signed_in}|{}", app.session.ai_setup.seq());
    if key == app.ui.ai_setup.plan_key {
        return;
    }
    app.ui.ai_setup.plan_key = key;
    match app.session.execute("ai.setup.plan", p) {
        Ok(plan) => {
            app.ui.ai_setup.plan = Some(plan);
            app.ui.ai_setup.plan_note.clear();
        }
        Err(e) => {
            app.ui.ai_setup.plan = None;
            app.ui.ai_setup.plan_note = e.to_string();
        }
    }
}

/// Something was removed: the engine counts that as an ending, but it is not a setup that
/// finished. Take it as seen, and read what is there now.
fn removed(app: &mut PhotocraftApp) {
    app.ui.ai_setup.seen_seq = app.session.ai_setup.seq();
    refresh_status(app);
    refresh_manifest(app);
}

fn install(app: &mut PhotocraftApp) {
    match app.session.execute(START, params(app)) {
        Ok(status) => {
            let s = &mut app.ui.ai_setup;
            (s.status, s.page) = (Some(status), Page::Install);
            s.note.clear();
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Continue the setup the record remembers.
/// What Continue Setup asks for: the setup as it was, except what this PC decides.
fn resume_params(app: &PhotocraftApp) -> Value {
    let existing = app.ui.ai_setup.status.as_ref().is_some_and(|s| text(s, "/record/route") == "existing");
    if existing {
        json!({"url": app.session.prefs().integrations.ai.comfy_url, "serverFiles": app.ui.ai_setup.server.as_ref().and_then(|s| s.get("folders")).cloned()})
    } else {
        // This PC decides how the engine runs, not the setup being continued (one started before
        // its missing card was noticed): without a card it can use, the processor is the only
        // way it starts at all.
        match app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("gpuUsable")).and_then(Value::as_bool) {
            Some(usable) => json!({"cpu": !usable}),
            None => json!({}),
        }
    }
}

fn resume(app: &mut PhotocraftApp) {
    let p = resume_params(app);
    match app.session.execute(START, p) {
        Ok(status) => {
            let s = &mut app.ui.ai_setup;
            (s.status, s.page) = (Some(status), Page::Install);
            s.note.clear();
        }
        Err(e) => {
            // Continue is on the Install screen, which says what went wrong in the error window.
            app.ui.ai_setup.note = e.to_string();
            app.ui.ai_setup.ask = Some(Ask::Error);
        }
    }
}

fn repair(app: &mut PhotocraftApp) {
    match app.session.execute("ai.setup.repair", json!({})) {
        Ok(status) => {
            let s = &mut app.ui.ai_setup;
            (s.status, s.page) = (Some(status), Page::Install);
            s.note.clear();
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

fn remove(app: &mut PhotocraftApp, models: bool) {
    match app.session.execute("ai.setup.remove", json!({"engine": true, "models": models})) {
        Ok(freed) => {
            let s = &mut app.ui.ai_setup;
            s.note = format!("{} {}", tr("Local AI was removed. Space freed:"), text(&freed, "/freedText"));
            (s.confirm_remove, s.page) = (false, Page::Pc);
            removed(app);
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Delete one installed model's files (a file another installed model uses stays).
fn remove_model(app: &mut PhotocraftApp, id: &str) {
    app.ui.ai_setup.remove_model = None;
    match app.session.execute("ai.setup.remove", json!({"models": [id]})) {
        Ok(freed) => {
            app.ui.ai_setup.models.retain(|m| m != id);
            app.ui.ai_setup.note = format!("{} {}", tr("The model's files were deleted. Space freed:"), text(&freed, "/freedText"));
            removed(app);
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Store the typed access token (the fallback to signing in), and check it.
fn save_token(app: &mut PhotocraftApp) {
    let token = std::mem::take(&mut app.ui.ai_setup.token);
    match app.session.execute("ai.setup.setToken", json!({"token": token})) {
        Ok(_) => {
            save_keys(app);
            refresh_status(app);
            app.ui.ai_setup.token_open = false;
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// The Hugging Face sign-in changed in the engine (signed in, renewed, out): save the keys
/// file it lives in, beside the API keys.
fn save_keys(app: &mut PhotocraftApp) {
    let _ = app.session.ai.take_keys_dirty();
    if let Some(save) = app.services.save_ai_keys.as_mut()
        && let Err(e) = save(&app.session.ai.keys_json())
    {
        app.ui.ai_setup.note = format!("{}: {e}", tr("The Hugging Face sign-in could not be saved"));
    }
}

// ---------- Hugging Face ----------

fn refresh_hf(app: &mut PhotocraftApp) {
    app.ui.ai_setup.hf = app.session.execute("ai.setup.hfStatus", json!({})).ok();
}

fn hf_flag(app: &PhotocraftApp, key: &str) -> bool {
    app.ui.ai_setup.hf.as_ref().and_then(|h| h.get(key)).and_then(Value::as_bool).unwrap_or(false)
}

/// The ticked models that need a Hugging Face sign-in before their files can come (the plan's
/// `needsSignIn`: gated, and not there already).
fn needs_sign_in(app: &PhotocraftApp) -> Vec<Value> {
    app.ui.ai_setup.plan.as_ref().map(|p| list(p, "/needsSignIn").to_vec()).unwrap_or_default()
}

/// Where a gated model stands: `signIn`, `licence`, `ready`, `unavailable`, or empty while not
/// known.
fn access_of(app: &PhotocraftApp, id: &str) -> String {
    let Some(access) = &app.ui.ai_setup.access else { return String::new() };
    list(access, "/models").iter().find(|m| text(m, "/id") == id).map(|m| text(m, "/access").to_string()).unwrap_or_default()
}

/// What Hugging Face said about a gated model, when it says more than its state.
fn detail_of(app: &PhotocraftApp, id: &str) -> String {
    let Some(access) = &app.ui.ai_setup.access else { return String::new() };
    list(access, "/models").iter().find(|m| text(m, "/id") == id).map(|m| text(m, "/detail").to_string()).unwrap_or_default()
}

/// Can every ticked gated model be downloaded?
fn all_ready(app: &PhotocraftApp) -> bool {
    needs_sign_in(app).iter().all(|m| access_of(app, text(m, "/id")) == "ready")
}

/// What the access in front was asked for: the gated models and the sign-in.
fn access_key(app: &PhotocraftApp) -> String {
    let ids: Vec<String> = needs_sign_in(app).iter().map(|m| text(m, "/id").to_string()).collect();
    format!("{}|{}", ids.join(","), app.session.ai_setup.hf_seq())
}

/// Ask Hugging Face where the ticked gated models stand, in the background where the app has
/// jobs. Renews a sign-in about to expire on the way.
fn check_access(app: &mut PhotocraftApp) {
    if app.ui.ai_setup.access_job.is_some() || app.session.ai_setup.signing_in() {
        return;
    }
    let ids: Vec<String> = needs_sign_in(app).iter().map(|m| text(m, "/id").to_string()).collect();
    if ids.is_empty() {
        return;
    }
    app.ui.ai_setup.access_key = access_key(app);
    let p = json!({"models": ids});
    let started = if app.background_jobs {
        app.session.start("ai.setup.hfAccess", p)
    } else {
        app.session.execute("ai.setup.hfAccess", p).map(photocraft_engine::jobs::Started::Done)
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Done(found)) => accessed(app, Some(found)),
        Ok(photocraft_engine::jobs::Started::Job(id)) => app.ui.ai_setup.access_job = Some(id.0),
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// The access check answered (`None`: it failed, and said why in the note).
fn accessed(app: &mut PhotocraftApp, found: Option<Value>) {
    if let Some(found) = found {
        app.ui.ai_setup.access = Some(found);
    }
    // A renewed sign-in, or one Hugging Face stopped taking, is saved as it is now.
    if app.session.ai.take_keys_dirty() {
        save_keys(app);
    }
    refresh_hf(app);
    app.ui.ai_setup.access_key = access_key(app);
}

/// Sign in with Hugging Face: the browser opens on its page, and comes back to the app.
fn sign_in(app: &mut PhotocraftApp, ctx: &egui::Context) {
    app.ui.ai_setup.note.clear();
    match app.session.execute("ai.setup.hfSignIn", json!({"product": crate::brand::NAME})) {
        Ok(started) => {
            if let Some(url) = started.get("url").and_then(Value::as_str).map(str::to_string) {
                crate::links::open(app, ctx, &url);
                app.ui.status = tr("Sign in with Hugging Face: continue in your browser").to_string();
            }
            refresh_hf(app);
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

fn cancel_sign_in(app: &mut PhotocraftApp) {
    let _ = app.session.execute("ai.setup.hfSignIn", json!({"cancel": true}));
    refresh_hf(app);
}

fn sign_out(app: &mut PhotocraftApp) {
    match app.session.execute("ai.setup.hfSignOut", json!({})) {
        Ok(_) => {
            save_keys(app);
            app.ui.ai_setup.access = None;
            refresh_hf(app);
            refresh_status(app);
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Use the Hugging Face token found on this PC: only ever on the user's click.
fn use_found(app: &mut PhotocraftApp) {
    match app.session.execute("ai.setup.hfUseFound", json!({})) {
        Ok(_) => {
            save_keys(app);
            refresh_hf(app);
            refresh_status(app);
        }
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

/// Install, or first ask for what a ticked gated model still needs.
fn install_clicked(app: &mut PhotocraftApp) {
    if all_ready(app) {
        install(app);
        return;
    }
    app.ui.ai_setup.ask = Some(Ask::SignIn);
    check_access(app);
}

/// Install without the gated models that can't be downloaded yet: they are unticked.
fn install_without(app: &mut PhotocraftApp) {
    let blocked: Vec<String> = needs_sign_in(app).iter().map(|m| text(m, "/id").to_string()).filter(|id| access_of(app, id) != "ready").collect();
    for id in &blocked {
        toggle_model(app, id, false);
    }
    app.ui.ai_setup.ask = None;
    // The plan follows the ticks before anything is started.
    replan(app);
    install(app);
}

/// Use my ComfyUI › Connect: read the model files of the ComfyUI at the address (on this
/// computer, at the Desktop app's port or the portable build's, whichever answers), and how it
/// was started when it runs on this computer.
fn connect(app: &mut PhotocraftApp) {
    let url = app.ui.ai_setup.url.trim().to_string();
    app.ui.ai_setup.server = None;
    app.ui.ai_setup.note.clear();
    let p = json!({"url": url, "start": false, "findPort": true});
    let started = if app.background_jobs {
        app.session.start("ai.comfyModels", p)
    } else {
        app.session.execute("ai.comfyModels", p).map(photocraft_engine::jobs::Started::Done)
    };
    match started {
        Ok(photocraft_engine::jobs::Started::Done(found)) => connected(app, Some(found)),
        Ok(photocraft_engine::jobs::Started::Job(id)) => app.ui.ai_setup.server_job = Some(id.0),
        // Without background jobs a ComfyUI that doesn't answer is an error here, as in a job.
        Err(_) if app.session.ai.transport.is_some() => connected(app, None),
        Err(e) => app.ui.ai_setup.note = e.to_string(),
    }
}

fn strings(v: &Value, pointer: &str) -> Vec<String> {
    list(v, pointer).iter().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string).collect()
}

/// The same folder, however it is written (`C:\x\models\` and `c:/x/models`).
fn same_folder(a: &str, b: &str) -> bool {
    let key = |s: &str| s.trim().replace('\\', "/").trim_end_matches('/').to_lowercase();
    key(a) == key(b)
}

/// Did the probe find a ComfyUI on this PC (running, or installed by ComfyUI Desktop)?
fn detected(app: &PhotocraftApp) -> bool {
    app.ui.ai_setup.probe.as_ref().is_some_and(|p| !list(p, "/probe/comfy/running").is_empty() || p.pointer("/probe/comfy/installed").is_some_and(Value::is_object))
}

/// The probe found a graphics card the engine can use (an older probe that doesn't say: yes).
fn gpu_usable(app: &PhotocraftApp) -> bool {
    app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("gpuUsable")).and_then(Value::as_bool).unwrap_or(true)
}

/// Can a new ComfyUI be installed here: the system allows it, and there is a card for it to run
/// on or the user chose the processor.
fn can_install_new(app: &PhotocraftApp) -> bool {
    let system = app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("canInstall")).and_then(Value::as_bool).unwrap_or(false);
    system && (gpu_usable(app) || app.ui.ai_setup.cpu)
}

/// Why a new ComfyUI can't be installed, for the controls that would.
fn cant_install_why(app: &PhotocraftApp) -> &'static str {
    if app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("canInstall")).and_then(Value::as_bool).unwrap_or(false) {
        tr("No graphics card the engine can use was found. Tick Install anyway to run it on the processor, or use cloud models")
    } else {
        tr("PhotoCraft can't install the engine on this system")
    }
}

/// Does the first screen ask which ComfyUI to use, with its big buttons? Once this PC has been
/// looked at, when nothing is set up (or setting up) yet: the user's own, found or not, or a new
/// one.
fn offer_choice(app: &PhotocraftApp) -> bool {
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    app.ui.ai_setup.probe.is_some() && list(&status, "/engines").is_empty() && !matches!(text(&status, "/state"), "done" | "running")
}

/// Is this PC being looked at for the first time: nothing set up, nothing begun, and no answer
/// yet? The first screen is then only its steps, in the middle.
fn first_look(app: &PhotocraftApp) -> bool {
    let s = &app.ui.ai_setup;
    let status = s.status.clone().unwrap_or(Value::Null);
    s.probe.is_none()
        && s.probe_job.is_some()
        && list(&status, "/engines").is_empty()
        && !matches!(text(&status, "/state"), "done" | "running" | "interrupted" | "paused" | "failed")
}

/// The step the look at this PC is on, by the message its job reports.
fn probe_stage(app: &PhotocraftApp) -> Stage {
    let Some(id) = app.ui.ai_setup.probe_job else { return Stage::Hardware };
    let message = app.session.jobs_with_recent().into_iter().find(|j| j.id.0 == id).map(|j| j.message).unwrap_or_default();
    stage_of(&message)
}

fn stage_of(message: &str) -> Stage {
    Stage::ALL.into_iter().find(|s| s.message() == message).unwrap_or(Stage::Hardware)
}

/// Use my ComfyUI learnt where it reads models (`found`, not empty): the folder chosen stays when
/// it is one of them, or when the user typed it in; none, or one the wizard suggested earlier
/// that is not among them, becomes the first, where ComfyUI saves new models itself.
fn follow_folders(s: &mut AiSetupUi, found: Vec<String>) {
    let Some(first) = found.first().cloned() else { return };
    let current = s.models_dir.clone();
    let listed = |folders: &[String]| folders.iter().any(|f| same_folder(f, &current));
    if current.trim().is_empty() || (!listed(&found) && listed(&s.folders)) {
        s.models_dir = first;
    }
    s.folders = found;
}

/// Use my ComfyUI: its screen, with its models folder filled in when it is known, and connected
/// at once when it has an address.
fn use_existing(app: &mut PhotocraftApp) {
    let found = app.ui.ai_setup.probe.as_ref().map(|p| strings(p, "/probe/comfy/models")).unwrap_or_default();
    let s = &mut app.ui.ai_setup;
    // The app's own models folder is not the user's ComfyUI's; one chosen for this route before
    // (last time's, say) is.
    if !s.existing {
        s.models_dir.clear();
    }
    follow_folders(s, found);
    (s.existing, s.page) = (true, Page::Location);
    s.note.clear();
    if !s.url.trim().is_empty() {
        connect(app);
    }
}

/// A private ComfyUI: where it goes. Coming from Use my ComfyUI, the models go to the app's own
/// folder again; a folder chosen for this route stays.
fn use_managed(app: &mut PhotocraftApp) {
    let default = app.ui.ai_setup.probe.as_ref().map(|p| text(p, "/defaults/modelsDir").to_string()).unwrap_or_default();
    let s = &mut app.ui.ai_setup;
    if s.existing || s.models_dir.trim().is_empty() {
        s.models_dir = default;
    }
    (s.existing, s.page) = (false, Page::Location);
    s.note.clear();
}

fn connected(app: &mut PhotocraftApp, found: Option<Value>) {
    let Some(found) = found else {
        // Neither the address nor ComfyUI's other port answered: say so where it can't be missed.
        app.ui.ai_setup.ask = Some(Ask::NotRunning);
        return;
    };
    // It answered on the other port (ComfyUI Desktop's 8000, say): that is its address.
    if let Some(answered) = found.get("url").and_then(Value::as_str).filter(|u| !u.trim().is_empty()) {
        app.ui.ai_setup.url = answered.to_string();
    }
    let url = app.ui.ai_setup.url.trim().to_string();
    let (_, local) = photocraft_aigen::launch::port_of(&url);
    // How it was started, so the app can start it from now on. Not knowing is fine.
    let launch = if local { app.session.execute("ai.server.find", json!({"url": url})).ok() } else { None };
    // Where it keeps its models: as it says itself, else as its settings give them.
    let mut folders = if local { strings(&found, "/modelsFolders") } else { Vec::new() };
    if folders.is_empty() {
        folders = launch.as_ref().map(|l| strings(l, "/modelsFolders")).unwrap_or_default();
    }
    let s = &mut app.ui.ai_setup;
    follow_folders(s, folders);
    let launch = launch.map(|mut l| {
        if let Some(o) = l.as_object_mut() {
            o.remove("modelsFolders");
        }
        l
    });
    s.server = Some(json!({"folders": found.get("folders").cloned().unwrap_or(json!({})), "launch": launch, "local": local}));
}

/// Tick or untick a model, keeping the preset tab honest.
fn toggle_model(app: &mut PhotocraftApp, id: &str, on: bool) {
    let s = &mut app.ui.ai_setup;
    s.models.retain(|m| m != id);
    if on {
        s.models.push(id.to_string());
    }
    name_preset(app);
}

/// Which preset the ticked models are, if they are one.
fn name_preset(app: &mut PhotocraftApp) {
    let chosen = app.ui.ai_setup.models.clone();
    let same = |preset: &str| {
        let mut a = preset_models(app, preset);
        let mut b = chosen.clone();
        a.sort();
        b.sort();
        a == b
    };
    app.ui.ai_setup.preset = PRESETS.into_iter().map(|(key, _)| key).find(|p| same(p)).unwrap_or("custom").to_string();
}

// ---------- the test image ----------

fn test_image_id() -> egui::Id {
    egui::Id::new("ai-setup-test-image")
}

fn forget_test_image(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<egui::TextureHandle>(test_image_id()));
}

/// The last setup's test image as a texture, decoded once.
fn test_image(app: &PhotocraftApp, ctx: &egui::Context) -> Option<egui::TextureHandle> {
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(test_image_id())) {
        return Some(tex);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use photocraft_codecs::{ChannelLayout, SampleType};
        let bytes = &app.session.ai_setup.last()?.test.as_ref()?.bytes;
        let img = photocraft_codecs::decode(bytes).ok()?.convert(ChannelLayout::Rgba, SampleType::U8);
        let (w, h) = (img.width() as usize, img.height() as usize);
        // Shown at a few hundred points: anything absurd is not a test image.
        if w == 0 || h == 0 || w.saturating_mul(h) > 64 << 20 || img.data().len() != w * h * 4 {
            return None;
        }
        let tex = ctx.load_texture("ai-setup-test", egui::ColorImage::from_rgba_unmultiplied([w, h], img.data()), egui::TextureOptions::LINEAR);
        ctx.data_mut(|d| d.insert_temp(test_image_id(), tex.clone()));
        Some(tex)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        None
    }
}

// ---------- drawing ----------

/// The status bar's readout while a setup runs: a click opens the wizard on its progress.
pub fn status_progress(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let Some((fraction, line)) = progress_line(app) else { return };
    let t = Tokens::get(ui.ctx());
    // The bar and its words are one button: either brings the wizard back on its progress.
    let inner = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (bar, bar_resp) = ui.allocate_exact_size(vec2(90.0, 6.0), Sense::click());
        crate::jobs_ui::bar(ui, bar, Some(fraction), &t);
        let label = ui.add(egui::Label::new(RichText::new(line).color(t.text_dim).size(12.0)).selectable(false).sense(Sense::click()));
        bar_resp.union(label)
    });
    let r = inner.inner;
    if r.hovered() {
        ui.painter().rect_stroke(r.rect.expand2(vec2(4.0, 3.0)), t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("Show the Local AI setup")));
    if r.on_hover_text(tr("Local AI is being set up in the background. Click to show its progress.")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        show_progress(app);
    }
}

/// A rounded row: the cards of the first screen, the model checklist and the install list.
fn row_frame(ui: &mut egui::Ui, height: f32) -> Rect {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, t.radius, t.field);
    ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, t.card_border), StrokeKind::Inside);
    rect
}

fn level_icon(level: &str, t: &Tokens) -> (&'static str, Color32) {
    match level {
        "ok" => ("check", t.accent),
        "warn" => ("triangle-alert", t.warning),
        "stop" => ("x", t.danger),
        _ => ("info", t.text_dim),
    }
}

/// A row with an icon, a title, a line under it and a short reading at the right.
fn info_row(ui: &mut egui::Ui, icon: (&str, Color32), title: &str, detail: &str, value: &str) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let value_width = if value.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(value.to_string(), crate::theme::mono(12.0), t.text_dim).size().x + 16.0 };
    let detail_galley =
        (!detail.is_empty()).then(|| ui.painter().layout(detail.to_string(), egui::FontId::proportional(12.0), t.text_faint, width - 62.0 - value_width));
    let height = 22.0 + detail_galley.as_ref().map_or(0.0, |g| g.size().y + 3.0) + 22.0;
    let rect = row_frame(ui, height.max(46.0));
    crate::icons::paint(ui, Rect::from_min_size(rect.min + vec2(14.0, (rect.height() - 18.0) / 2.0), vec2(18.0, 18.0)), icon.0, 16.0, icon.1);
    let x = rect.left() + 46.0;
    let title_y = if detail_galley.is_some() { rect.top() + 11.0 } else { rect.center().y - 9.0 };
    ui.painter().text(pos2(x, title_y), egui::Align2::LEFT_TOP, title, crate::theme::medium(13.5), t.text);
    if let Some(galley) = detail_galley {
        ui.painter().galley(pos2(x, title_y + 21.0), galley, t.text_faint);
    }
    if !value.is_empty() {
        ui.painter().text(pos2(rect.right() - 14.0, rect.center().y), egui::Align2::RIGHT_CENTER, value, crate::theme::mono(12.0), t.text_dim);
    }
    ui.add_space(6.0);
}

fn heading(ui: &mut egui::Ui, title: &str, sub: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(tr(title)).font(crate::theme::semibold(20.0)).color(t.text));
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(tr(sub)).color(t.text_dim).size(13.0)).wrap());
    ui.add_space(12.0);
}

/// A note in the danger colour, wrapped and selectable: something to act on, or to copy.
fn problem(ui: &mut egui::Ui, message: &str) {
    if message.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.add(egui::Label::new(RichText::new(message).color(t.danger).size(12.0)).wrap().selectable(true));
    ui.add_space(4.0);
}

/// A link that reads as one: opens `url` in the browser.
fn link(app: &mut PhotocraftApp, ui: &mut egui::Ui, label: &str, url: &str) {
    let t = Tokens::get(ui.ctx());
    let r = ui.add(egui::Label::new(RichText::new(label).color(t.text_dim).size(11.5).underline()).sense(Sense::click()));
    if r.on_hover_text(url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        crate::links::open(app, ui.ctx(), url);
    }
}

fn sidebar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (page, existing, busy) = (app.ui.ai_setup.page, app.ui.ai_setup.existing, running(app));
    ui.add_space(6.0);
    for (i, step) in Page::ALL.into_iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
        let current = step == page;
        if current {
            ui.painter().rect_filled(rect, t.radius, t.field);
        } else if resp.hovered() && step < page && !busy {
            ui.painter().rect_filled(rect, t.radius, t.hover);
        }
        let centre = pos2(rect.left() + 24.0, rect.center().y);
        if current {
            ui.painter().circle_filled(centre, 11.0, t.accent);
        } else {
            ui.painter().circle_stroke(centre, 11.0, Stroke::new(1.0, t.field_border));
        }
        let number_color = if current { t.primary_text } else { t.text_faint };
        if step < page {
            crate::icons::paint(ui, Rect::from_center_size(centre, vec2(14.0, 14.0)), "check", 12.0, t.text_dim);
        } else {
            ui.painter().text(centre, egui::Align2::CENTER_CENTER, (i + 1).to_string(), egui::FontId::proportional(11.5), number_color);
        }
        let color = if current { t.text } else { t.text_dim };
        ui.painter().text(pos2(rect.left() + 46.0, rect.center().y), egui::Align2::LEFT_CENTER, tr(step.label(existing)), crate::theme::medium(13.5), color);
        // Back to a screen already seen; forward only through Next, which checks each one.
        if resp.clicked() && step < page && !busy && page != Page::Ready {
            app.ui.ai_setup.page = step;
            app.ui.ai_setup.note.clear();
        }
        ui.add_space(2.0);
    }
}

fn page_pc(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, "Checking your PC", "Nothing is changed in this step. PhotoCraft looks at what you have and says what will work.");
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let state = text(&status, "/state").to_string();
    // What is already here, before what could be.
    if matches!(state.as_str(), "interrupted" | "paused" | "failed") {
        let detail = match text(&status, "/record/error") {
            "" => tr("What arrived is kept. Continuing downloads only what is missing.").to_string(),
            why => why.to_string(),
        };
        info_row(ui, ("info", t.warning), tr("A setup was interrupted"), &detail, "");
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, tr("Continue Setup"), 130.0).clicked() {
                resume(app);
            }
            ui.label(RichText::new(tr("or start over with the steps below")).color(t.text_faint).size(11.5));
        });
        ui.add_space(8.0);
    } else if state == "done" {
        let installed = list(&status, "/installedNow").len();
        let managed = !list(&status, "/engines").is_empty();
        let title = if managed {
            format!("{} {}", tr("Local AI is set up: ComfyUI"), text(&status, "/record/comfyVersion"))
        } else {
            tr("Local AI is set up with your own ComfyUI").to_string()
        };
        let detail = format!("{installed} {} · {}", tr("models installed"), text(&status, "/record/modelsDir"));
        info_row(ui, ("check", t.accent), &title, &detail, "");
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, tr("Add Models…"), 110.0).clicked() {
                app.ui.ai_setup.existing = !managed;
                app.ui.ai_setup.page = if managed { Page::Models } else { Page::Location };
            }
            if widgets::secondary_button(ui, tr("Repair"), 80.0)
                .on_hover_text(tr("Check every file against its checksum, and download again what is damaged or missing"))
                .clicked()
            {
                repair(app);
            }
            if managed && widgets::secondary_button(ui, tr("Remove Local AI…"), 140.0).clicked() {
                app.ui.ai_setup.confirm_remove = true;
            }
        });
        ui.add_space(8.0);
    }
    if app.ui.ai_setup.confirm_remove {
        let engine = list(&status, "/engines").first().map(|e| text(e, "/folder").to_string()).unwrap_or_default();
        info_row(
            ui,
            ("triangle-alert", t.warning),
            tr("Remove local AI?"),
            &format!(
                "{} {engine}. {}",
                tr("This deletes the engine in"),
                tr("Your own files in the models folder are never touched. Cloud models keep working.")
            ),
            "",
        );
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, tr("Remove Engine and Models"), 190.0).clicked() {
                remove(app, true);
            }
            if widgets::secondary_button(ui, tr("Remove Engine Only"), 150.0)
                .on_hover_text(tr("Keeps the downloaded model files, to use with another ComfyUI or a later setup"))
                .clicked()
            {
                remove(app, false);
            }
            if widgets::secondary_button(ui, tr("Cancel"), 80.0).clicked() {
                app.ui.ai_setup.confirm_remove = false;
            }
        });
        ui.add_space(8.0);
    }
    // No card the engine can use: a new ComfyUI is installed only when asked for knowingly, to
    // run on the processor (to try the setup in a virtual machine, say). Asked under the card's row.
    let system = app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("canInstall")).and_then(Value::as_bool).unwrap_or(false);
    let offer_cpu = system && !gpu_usable(app);
    match (app.ui.ai_setup.probe.clone(), app.ui.ai_setup.probe_job) {
        (Some(probe), _) => {
            // With the app's own engine installed, the row about finding a ComfyUI has been
            // answered above.
            let own = !list(&status, "/engines").is_empty();
            for check in list(&probe, "/checks").iter().filter(|c| !(own && text(c, "/id") == "comfy")) {
                info_row(ui, level_icon(text(check, "/level"), &t), tr(text(check, "/title")), tr(text(check, "/detail")), tr(text(check, "/value")));
                if offer_cpu && text(check, "/id") == "gpu" {
                    let mut cpu = app.ui.ai_setup.cpu;
                    widgets::checkbox(ui, &mut cpu, tr("Install anyway, to run on the processor (very slow: for trying the setup out)")).on_hover_text(tr(
                        "The engine starts without a graphics card. No test image is rendered: one image can take an hour or more on the processor",
                    ));
                    app.ui.ai_setup.cpu = cpu;
                    ui.add_space(6.0);
                }
            }
        }
        (None, Some(_)) => {
            let step = probe_stage(app);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("{}\u{2026}", tr(step.message()))).color(t.text_dim));
            });
        }
        (None, None) => {}
    }
    problem(ui, &app.ui.ai_setup.note.clone());
}

/// The first look at this PC, while it runs: its steps in the middle of the screen, each ticked
/// as the next begins, so the wait reads as work being done.
fn page_checking(app: &PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let current = probe_stage(app);
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let painter = ui.painter_at(area);
    let title = painter.layout_no_wrap(tr("Checking your PC").to_string(), crate::theme::semibold(20.0), t.text);
    let sub = painter.layout(
        tr("Nothing is changed: PhotoCraft only looks at what you have.").to_string(),
        egui::FontId::proportional(13.0),
        t.text_dim,
        area.width(),
    );
    const MARK: f32 = 40.0;
    const ROW: f32 = 50.0;
    const COLUMN: f32 = 340.0;
    let height = MARK + 18.0 + title.size().y + 6.0 + sub.size().y + 28.0 + ROW * Stage::ALL.len() as f32;
    let mut y = (area.center().y - height / 2.0).max(area.top());
    let centre = area.center().x;
    crate::brand::paint_mark(ui, Rect::from_center_size(pos2(centre, y + MARK / 2.0), vec2(MARK, MARK)));
    y += MARK + 18.0;
    let title_height = title.size().y;
    painter.galley(pos2(centre - title.size().x / 2.0, y), title, t.text);
    y += title_height + 6.0;
    let sub_height = sub.size().y;
    painter.galley(pos2(centre - sub.size().x / 2.0, y), sub, t.text_dim);
    y += sub_height + 28.0;
    let left = centre - COLUMN.min(area.width()) / 2.0;
    for (step, detail) in Stage::ALL.into_iter().zip([
        "Windows, the graphics card, memory and free disk space",
        "One that is running, or installed on this PC",
        "Where the engine and the models download from",
    ]) {
        let icon = Rect::from_center_size(pos2(left + 9.0, y + 10.0), vec2(18.0, 18.0));
        let (title_color, detail_color) = if step > current { (t.text_faint, t.text_faint) } else { (t.text, t.text_dim) };
        if step < current {
            crate::icons::paint(ui, icon, "check", 16.0, t.accent);
        } else if step == current {
            egui::Spinner::new().size(16.0).color(t.accent).paint_at(ui, icon);
        } else {
            painter.circle_stroke(icon.center(), 7.0, Stroke::new(1.0, t.field_border));
        }
        painter.text(pos2(left + 30.0, y + 10.0), egui::Align2::LEFT_CENTER, tr(step.message()), crate::theme::medium(13.5), title_color);
        painter.text(pos2(left + 30.0, y + 29.0), egui::Align2::LEFT_CENTER, tr(detail), egui::FontId::proportional(12.0), detail_color);
        y += ROW;
    }
}

/// A big button: a title, a line under it, and a badge beside the title. Filled when it is the
/// one recommended.
fn choice_button(ui: &mut egui::Ui, rect: Rect, title: &str, sub: &str, badge: &str, primary: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let resp = ui.allocate_rect(rect, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title));
    let enabled = ui.is_enabled();
    let down = resp.is_pointer_button_down_on();
    let (fill, fg, dim) = if primary {
        let fill = if down {
            t.primary_bg.gamma_multiply(0.85)
        } else if resp.hovered() {
            t.primary_bg.gamma_multiply(0.93)
        } else {
            t.primary_bg
        };
        (fill, t.primary_text, t.primary_text.gamma_multiply(0.85))
    } else {
        (if resp.hovered() || down { t.hover } else { t.field }, t.text, t.text_dim)
    };
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    ui.painter().rect_filled(rect, t.radius, fade(fill));
    if !primary {
        let edge = if resp.hovered() && enabled { t.text_dim } else { t.field_border };
        ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, edge), StrokeKind::Inside);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), t.radius + 2.0, Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
    let inner = rect.shrink2(vec2(16.0, 12.0));
    let painter = ui.painter_at(rect);
    let heading = painter.layout_no_wrap(title.to_string(), crate::theme::semibold(14.0), fade(fg));
    let heading_size = heading.size();
    painter.galley(inner.min, heading, fade(fg));
    if !badge.is_empty() {
        let label = painter.layout_no_wrap(badge.to_string(), crate::theme::medium(10.5), fade(fill));
        let pill = Rect::from_min_size(
            pos2(inner.left() + heading_size.x + 8.0, inner.top() + (heading_size.y - label.size().y - 4.0) / 2.0),
            label.size() + vec2(12.0, 4.0),
        );
        painter.rect_filled(pill, pill.height() / 2.0, fade(fg));
        painter.galley(pill.min + vec2(6.0, 2.0), label, fade(fill));
    }
    let line = painter.layout(sub.to_string(), egui::FontId::proportional(11.5), fade(dim), inner.width());
    painter.galley(pos2(inner.left(), inner.top() + heading_size.y + 5.0), line, fade(dim));
    resp
}

/// The first screen's question: the user's ComfyUI, or a new one. When one was found, keep
/// using it; when none was, one this look missed can still be connected.
fn choice(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let (can_install, why) = (can_install_new(app), cant_install_why(app));
    let (mine_title, mine_sub, new_title, new_sub) = if detected(app) {
        (
            tr("Continue with My ComfyUI"),
            tr("Keeps your ComfyUI and its models. Only missing model files are downloaded."),
            tr("Continue with New ComfyUI"),
            tr("A private copy with its own Python, set up for PhotoCraft. Yours is left as it is."),
        )
    } else {
        (
            tr("Use My ComfyUI"),
            tr("Have one this check didn't find? Connect it. Only missing model files are downloaded."),
            tr("Install New ComfyUI"),
            tr("A private copy with its own Python, set up for PhotoCraft. Nothing else on this PC is changed."),
        )
    };
    let area = ui.available_rect_before_wrap();
    let gap = 12.0;
    let width = ((area.width() - gap) / 2.0).max(0.0);
    let height = area.height().clamp(0.0, 80.0);
    let mine = Rect::from_min_size(area.min, vec2(width, height));
    let new = Rect::from_min_size(pos2(area.left() + width + gap, area.top()), vec2(width, height));
    // Each in a Ui of its own: side by side, and the second greyed where it can't install.
    let mut left = ui.new_child(UiBuilder::new().max_rect(mine));
    let r = choice_button(&mut left, mine, mine_title, mine_sub, "", false);
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        use_existing(app);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(new));
    if !can_install {
        right.disable();
    }
    let r = choice_button(
        &mut right,
        new,
        new_title,
        new_sub,
        tr("Recommended"),
        true,
    );
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_disabled_hover_text(why).clicked() {
        use_managed(app);
    }
}

/// Use my ComfyUI: the folders it reads models from, to pick one with a click; or, when none
/// was found, where such a folder usually is.
fn folder_choices(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let folders = app.ui.ai_setup.folders.clone();
    if folders.is_empty() {
        ui.add(
            egui::Label::new(
                RichText::new(tr(
                    "Not sure which folder? It is the one named models, with checkpoints, diffusion_models and vae in it. ComfyUI Desktop usually keeps it in Documents\\ComfyUI\\models, the portable build in ComfyUI_windows_portable\\ComfyUI\\models. With ComfyUI running, Connect finds it.",
                ))
                .color(t.text_faint)
                .size(11.5),
            )
            .wrap(),
        );
        ui.add_space(12.0);
        return;
    }
    let heading = if folders.len() == 1 { tr("Found in your ComfyUI's settings:") } else { tr("Your ComfyUI reads models from these folders. Any of them will do:") };
    ui.label(RichText::new(heading).color(t.text_dim).size(11.5));
    ui.add_space(4.0);
    for (i, folder) in folders.iter().enumerate() {
        let chosen = same_folder(&app.ui.ai_setup.models_dir, folder);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        let centre = pos2(rect.left() + 12.0, rect.center().y);
        ui.painter().circle_stroke(centre, 6.0, Stroke::new(1.2, if chosen { t.accent } else { t.text_faint }));
        if chosen {
            ui.painter().circle_filled(centre, 3.2, t.accent);
        }
        let painter = ui.painter_at(rect);
        let note = (i == 0 && folders.len() > 1)
            .then(|| painter.layout_no_wrap(tr("where ComfyUI saves new models").to_string(), egui::FontId::proportional(11.5), t.text_faint));
        let room = rect.width() - 30.0 - note.as_ref().map_or(0.0, |n| n.size().x + 12.0);
        let color = if chosen { t.text } else { t.text_dim };
        // A long path loses its middle, so the note and the folder's own name stay in view.
        let mut shown = folder.clone();
        let mut path = painter.layout_no_wrap(shown.clone(), crate::theme::mono(12.0), color);
        let chars: Vec<char> = folder.chars().collect();
        let mut cut = 0;
        while path.size().x > room && cut + 8 < chars.len() {
            cut += 2;
            let keep = chars.len() - cut;
            let head: String = chars.iter().take(keep * 2 / 5).collect();
            let tail: String = chars.iter().skip(chars.len() - (keep - keep * 2 / 5)).collect();
            shown = format!("{head}\u{2026}{tail}");
            path = painter.layout_no_wrap(shown.clone(), crate::theme::mono(12.0), color);
        }
        let path_width = path.size().x;
        painter.galley(pos2(rect.left() + 26.0, rect.center().y - path.size().y / 2.0), path, color);
        if let Some(note) = note {
            painter.galley(pos2(rect.left() + 26.0 + path_width + 12.0, rect.center().y - note.size().y / 2.0), note, t.text_faint);
        }
        let resp = if shown == *folder { resp } else { resp.on_hover_text(folder) };
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.ui.ai_setup.models_dir = folder.clone();
        }
    }
    // A folder it doesn't read: what is downloaded there would not show up in it.
    let chosen = app.ui.ai_setup.models_dir.clone();
    if !chosen.trim().is_empty() && !folders.iter().any(|f| same_folder(f, &chosen)) {
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                RichText::new(tr("Your ComfyUI doesn't read models from the folder above, so files downloaded there won't show up in it.")).color(t.warning).size(11.5),
            )
            .wrap(),
        );
    }
    ui.add_space(12.0);
}

/// A folder: a field, Browse, and the room on its drive.
fn folder_row(app: &mut PhotocraftApp, ui: &mut egui::Ui, engine: bool) {
    let t = Tokens::get(ui.ctx());
    let (title, about) = if engine {
        ("Engine folder", "ComfyUI and its own Python. Nothing is added to PATH, and no administrator rights are needed.")
    } else if app.ui.ai_setup.existing {
        ("Models folder", "Your ComfyUI's models folder: missing files are downloaded into it.")
    } else {
        ("Models folder", "Model files are large. Any drive will do, and they stay when the engine is updated or removed.")
    };
    ui.label(RichText::new(tr(title)).font(crate::theme::medium(13.0)).color(t.text));
    ui.add_space(3.0);
    let mut pick = false;
    ui.horizontal(|ui| {
        let s = &mut app.ui.ai_setup;
        let value = if engine { &mut s.engine_dir } else { &mut s.models_dir };
        widgets::text_field(ui, egui::Id::new(("ai-setup-folder", engine)), value, tr("Choose a folder"), ui.available_width() - 96.0, false);
        pick = widgets::secondary_button(ui, tr("Browse…"), 84.0).clicked();
    });
    ui.add_space(3.0);
    ui.add(egui::Label::new(RichText::new(tr(about)).color(t.text_faint).size(11.5)).wrap());
    if pick {
        let current = if engine { app.ui.ai_setup.engine_dir.clone() } else { app.ui.ai_setup.models_dir.clone() };
        let asked =
            app.ask_file(crate::file_dialog::FileDialogRequest::Folder { initial_dir: Some(current).filter(|d| !d.trim().is_empty()) }, move |app, answer| {
                if let crate::file_dialog::FileDialogAnswer::FolderTo(path) = answer {
                    let s = &mut app.ui.ai_setup;
                    *(if engine { &mut s.engine_dir } else { &mut s.models_dir }) = path;
                }
                Ok(Value::Null)
            });
        if let Err(e) = asked {
            app.ui.ai_setup.note = format!("{}: {e}", tr("Could not open the folder dialog"));
        }
    }
    ui.add_space(12.0);
}

/// What the plan says about room: needed against free, as a bar. Red when only another folder
/// helps; amber when fewer models would, which the Models page is for.
fn room(ui: &mut egui::Ui, plan: &Value) {
    let t = Tokens::get(ui.ctx());
    let same = plan.get("sameDrive").and_then(Value::as_bool).unwrap_or(false);
    let (engine, models) = (number(plan, "/engineBytes"), number(plan, "/modelsBytes"));
    let (folder_short, models_short) = (!list(plan, "/folderProblems").is_empty(), !list(plan, "/modelProblems").is_empty());
    let bar = |ui: &mut egui::Ui, what: &str, need: u64, free: Option<u64>, fill: Color32| {
        let Some(free) = free else { return };
        let fraction = if free > 0 { (need as f64 / free as f64).clamp(0.0, 1.0) as f32 } else { 1.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, t.field);
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height())), 3.0, fill);
        ui.add_space(3.0);
        ui.label(RichText::new(format!("{what}: {} {} · {} {}", size(need), tr("needed"), size(free), tr("free"))).color(t.text_dim).size(11.5));
        ui.add_space(8.0);
    };
    let models_fill = if models_short { t.warning } else { t.accent };
    if same {
        let fill = if folder_short { t.danger } else { models_fill };
        bar(ui, tr("Engine and models"), engine.saturating_add(models), plan.get("modelsFree").and_then(Value::as_u64), fill);
    } else {
        if plan.get("route").and_then(Value::as_str) == Some("managed") {
            let fill = if folder_short { t.danger } else { t.accent };
            bar(ui, tr("Engine"), engine, plan.get("engineFree").and_then(Value::as_u64), fill);
        }
        bar(ui, tr("Models"), models, plan.get("modelsFree").and_then(Value::as_u64), models_fill);
    }
}

fn page_location(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if app.ui.ai_setup.existing {
        heading(
            ui,
            "Your ComfyUI",
            "PhotoCraft connects to the ComfyUI you already have. It installs nothing, and only fetches the model files that are missing.",
        );
        ui.label(RichText::new(tr("Address")).font(crate::theme::medium(13.0)).color(t.text));
        ui.add_space(3.0);
        let busy = app.ui.ai_setup.server_job.is_some();
        let mut go = false;
        ui.horizontal(|ui| {
            widgets::text_field(ui, egui::Id::new("ai-setup-url"), &mut app.ui.ai_setup.url, "http://127.0.0.1:8188", ui.available_width() - 96.0, true);
            go = ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, tr("Connect"), 84.0)).inner.clicked();
        });
        if go {
            connect(app);
        }
        ui.add_space(3.0);
        let found = app.ui.ai_setup.server.clone();
        let line = match (&found, busy) {
            (_, true) => tr("Connecting…").to_string(),
            (Some(server), _) => {
                let count: usize = server.get("folders").and_then(Value::as_object).map_or(0, |f| f.values().filter_map(Value::as_array).map(Vec::len).sum());
                let started = if server.get("launch").is_some_and(Value::is_object) { tr("PhotoCraft can start it for you from now on.") } else { "" };
                format!("{} · {count} {} {started}", tr("Connected"), tr("model files."))
            }
            // Found installed but not running: open it, and that is all.
            (None, false) if detected(app) && app.ui.ai_setup.probe.as_ref().is_some_and(|p| list(p, "/probe/comfy/running").is_empty()) => {
                tr("Your ComfyUI Desktop is not running. Open it, then press Connect.").to_string()
            }
            (None, false) => tr("Start ComfyUI, then press Connect. It is found on either port: 8000 for ComfyUI Desktop, 8188 for the portable build.").to_string(),
        };
        ui.add(egui::Label::new(RichText::new(line).color(t.text_faint).size(11.5)).wrap());
        ui.add_space(12.0);
        if found.as_ref().is_some_and(|s| s.get("local").and_then(Value::as_bool) == Some(false)) {
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "That ComfyUI runs on another computer, so there is no models folder here to download into. Its models are set up as they are.",
                    ))
                    .color(t.text_dim)
                    .size(12.0),
                )
                .wrap(),
            );
        } else {
            folder_row(app, ui, false);
            folder_choices(app, ui);
        }
    } else {
        heading(ui, "Where it goes", "Two folders: a small one for the engine, and one for the models, which can be on any drive.");
        folder_row(app, ui, true);
        folder_row(app, ui, false);
    }
    replan(app);
    if let Some(plan) = app.ui.ai_setup.plan.clone() {
        room(ui, &plan);
        for p in list(&plan, "/folderProblems") {
            problem(ui, p.as_str().unwrap_or(""));
        }
        // The models are chosen on the next page: too little room for them is not a reason to stop here.
        if let Some(first) = list(&plan, "/modelProblems").first().and_then(Value::as_str) {
            let line = format!("{first} {}", tr("You choose the models on the next page."));
            ui.add(egui::Label::new(RichText::new(line).color(t.warning).size(12.0)).wrap().selectable(true));
            ui.add_space(4.0);
        }
    }
    problem(ui, &app.ui.ai_setup.plan_note.clone());
    problem(ui, &app.ui.ai_setup.note.clone());
}

/// How a licence reads beside a model: its name, and what it means for paid work.
fn licence_note(model: &Value) -> String {
    let name = text(model, "/licence/name");
    match model.pointer("/licence/commercial").and_then(Value::as_bool) {
        Some(true) => format!("{name} · {}", tr("commercial use allowed")),
        Some(false) => format!("{name} · {}", tr("non-commercial use only")),
        None => format!("{name} · {}", tr("read the licence for commercial use")),
    }
}

/// The presets of the Models step, smallest first: `light` is the one for a small card.
const PRESETS: [(&str, &str); 4] = [("light", "Light"), ("starter", "Starter"), ("recommended", "Recommended"), ("everything", "Everything")];

fn page_models(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, "Choose your models", "Each model is a few large files. Files that models share are downloaded once. You can add more later.");
    let manifest = app.ui.ai_setup.manifest.clone().unwrap_or(Value::Null);
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    // Presets.
    let mut chosen_preset = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (key, label) in PRESETS {
            let bytes = number(&manifest, &format!("/presets/{key}/bytes"));
            if widgets::pill_tab(ui, &format!("{} · {}", tr(label), download_size(bytes)), app.ui.ai_setup.preset == key).clicked() {
                chosen_preset = Some(key);
            }
        }
        if app.ui.ai_setup.preset == "custom" {
            widgets::pill_tab(ui, tr("Your choice"), true);
        }
    });
    if let Some(key) = chosen_preset {
        app.ui.ai_setup.preset = key.to_string();
        app.ui.ai_setup.models = preset_models(app, key);
    }
    ui.add_space(8.0);
    let vram = app.ui.ai_setup.probe.as_ref().and_then(|p| p.get("vramGb")).and_then(Value::as_u64);
    // On a small card the light preset's models come first, and say why.
    let for_card: Vec<String> = match text(&manifest, "/suggested") {
        "light" => list(&manifest, "/presets/light/models").iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    // Files this app downloaded for its own engine are its to delete again.
    let managed = !list(&status, "/engines").is_empty() && !running(app);
    let needing = needs_sign_in(app);
    let mut any_gated = false;
    // The list takes the page down to the footer, less what follows it (the Hugging Face panel,
    // a problem), as tall as that was last frame.
    let tail_id = egui::Id::new("ai-setup-models-tail");
    let tail = ui.ctx().data(|d| d.get_temp::<f32>(tail_id)).unwrap_or(0.0);
    let height = (ui.available_height() - tail).max(110.0);
    egui::ScrollArea::vertical().id_salt("ai-setup-models").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        for model in list(&manifest, "/models").to_vec() {
            let id = text(&model, "/id").to_string();
            let mut on = app.ui.ai_setup.models.contains(&id);
            let gated = model.get("gated").and_then(Value::as_bool).unwrap_or(false);
            let installed = model.get("installed").and_then(Value::as_bool).unwrap_or(false);
            any_gated |= gated && on;
            let rect = row_frame(ui, 82.0);
            let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 9.0))).layout(Layout::top_down(Align::Min)));
            inner.spacing_mut().item_spacing.y = 5.0;
            inner.horizontal(|ui| {
                if widgets::checkbox(ui, &mut on, text(&model, "/name")).changed() {
                    toggle_model(app, &id, on);
                }
                let does: Vec<&str> = list(&model, "/does").iter().filter_map(Value::as_str).map(tr).collect();
                ui.label(RichText::new(does.join(" · ")).color(t.text_faint).size(11.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if installed { tr("installed").to_string() } else { download_size(number(&model, "/bytes")) };
                    ui.label(RichText::new(label).font(crate::theme::mono(12.0)).color(if installed { t.accent_text } else { t.text_dim }));
                });
            });
            inner.label(RichText::new(tr(text(&model, "/about"))).color(t.text_dim).size(11.5));
            inner.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                link(app, ui, &licence_note(&model), text(&model, "/licence/url"));
                if let (true, Some(gb)) = (for_card.contains(&id), vram) {
                    ui.label(RichText::new(format!("{} {gb} GB {}", tr("suggested for your"), tr("card"))).color(t.accent_text).size(11.5));
                }
                match (text(&model, "/fit"), vram) {
                    ("tooLarge", Some(gb)) => {
                        ui.label(RichText::new(format!("{} {gb} GB", tr("too large for your card:"))).color(t.warning).size(11.5));
                    }
                    ("untested", Some(gb)) => {
                        ui.label(RichText::new(format!("{} {gb} GB {}", tr("not tested on"), tr("cards"))).color(t.text_faint).size(11.5));
                    }
                    _ => {}
                }
                // A gated model still to come: where it stands (the panel below says what to do).
                if gated && on && needing.iter().any(|m| text(m, "/id") == id) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| gated_state(app, ui, &id));
                }
                if installed && managed {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if app.ui.ai_setup.remove_model.as_deref() == Some(id.as_str()) {
                            if ui.add(egui::Label::new(RichText::new(tr("Keep")).color(t.text_dim).size(11.5).underline()).sense(Sense::click())).clicked() {
                                app.ui.ai_setup.remove_model = None;
                            }
                            if ui.add(egui::Label::new(RichText::new(tr("Delete")).color(t.danger).size(11.5).underline()).sense(Sense::click())).clicked() {
                                remove_model(app, &id);
                            }
                            ui.label(RichText::new(tr("Delete its files?")).color(t.text_dim).size(11.5));
                        } else {
                            let r =
                                ui.add(egui::Label::new(RichText::new(tr("Delete files…")).color(t.text_faint).size(11.5).underline()).sense(Sense::click()));
                            if r.on_hover_text(tr("Frees the space this model takes. A file another installed model uses is kept")).clicked() {
                                app.ui.ai_setup.remove_model = Some(id.clone());
                            }
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
    });
    let tail_top = ui.cursor().top();
    // A gated model among the ticked ones: signing in, the licence, and the token fallback.
    if any_gated && (!needing.is_empty() || hf_flag(app, "signedIn")) {
        ui.add_space(2.0);
        hf_panel(app, ui);
    }
    problem(ui, &app.ui.ai_setup.plan_note.clone());
    problem(ui, &app.ui.ai_setup.note.clone());
    let measured = ui.cursor().top() - tail_top;
    if (measured - tail).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(tail_id, measured));
        ui.ctx().request_repaint();
    }
    replan(app);
    // Where the ticked gated models stand, asked whenever they or the sign-in change.
    if !needs_sign_in(app).is_empty() && app.ui.ai_setup.access_key != access_key(app) {
        check_access(app);
    }
}

/// An underlined word that does something: the small actions in a model's row.
fn action(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    let r = ui.add(egui::Label::new(RichText::new(label).color(color).size(11.5).underline()).selectable(false).sense(Sense::click()));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A gated model's row (laid out right to left): where it stands, in a few words. What to do
/// about it is in the Hugging Face panel below the list, one place for every gated model.
fn gated_state(app: &PhotocraftApp, ui: &mut egui::Ui, id: &str) {
    let t = Tokens::get(ui.ctx());
    let small = |s: &str, c: Color32| RichText::new(s).color(c).size(11.5);
    if app.session.ai_setup.signing_in() {
        ui.label(small(tr("Waiting for your browser\u{2026}"), t.text_dim));
        ui.add(egui::Spinner::new().size(12.0));
        return;
    }
    match access_of(app, id).as_str() {
        "ready" => {
            ui.label(small(tr("Ready to download"), t.accent_text));
            let (mark, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            crate::icons::paint(ui, mark, "check", 12.0, t.accent_text);
        }
        "licence" => {
            ui.label(small(tr("Licence to accept (below)"), t.warning));
        }
        "unavailable" => {
            ui.label(small(tr("Not available in this version"), t.danger)).on_hover_text(detail_of(app, id));
        }
        _ if app.ui.ai_setup.access_job.is_some() => {
            ui.label(small(tr("Checking access\u{2026}"), t.text_dim));
            ui.add(egui::Spinner::new().size(12.0));
        }
        _ => {
            ui.label(small(tr("Needs a Hugging Face sign-in (below)"), t.warning));
        }
    }
}

/// Names joined for a sentence: "A", "A and B".
fn names_of(models: &[Value]) -> String {
    models.iter().map(|m| text(m, "/name")).collect::<Vec<_>>().join(&format!(" {} ", tr("and")))
}

/// Below the models, when a gated one is ticked: the one place to sign in. The account (Sign in
/// with Hugging Face, or who is signed in and Sign out), then the licence still to accept, then
/// the access token as the fallback.
fn hf_panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    use photocraft_aisetup::hf::{CONNECTED_APPS_URL, HELP_URL, TOKENS_URL};
    let t = Tokens::get(ui.ctx());
    let hf = app.ui.ai_setup.hf.clone().unwrap_or(Value::Null);
    let (signed, user, via) = (hf_flag(app, "signedIn"), text(&hf, "/user").to_string(), text(&hf, "/via").to_string());
    let (signing, checking, can) = (app.session.ai_setup.signing_in(), app.ui.ai_setup.access_job.is_some(), hf_flag(app, "canSignIn"));
    let small = |s: &str, c: Color32| RichText::new(s).color(c).size(11.5);
    let needs = needs_sign_in(app);
    let state = |app: &PhotocraftApp, wanted: &str| needs.iter().filter(|m| access_of(app, text(m, "/id")) == wanted).cloned().collect::<Vec<_>>();
    let (licence, gone) = (state(app, "licence"), state(app, "unavailable"));
    // Why the last sign-in was forgotten (it expired, or Hugging Face refused it).
    let forgotten = app.ui.ai_setup.access.as_ref().map(|a| text(a, "/note").to_string()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(tr("Hugging Face")).font(crate::theme::medium(12.0)).color(t.text));
        if signing {
            ui.add(egui::Spinner::new().size(12.0));
            ui.label(small(tr("Waiting for your browser: sign in there, then come back"), t.text_dim));
            if action(ui, tr("Cancel"), t.text_dim).clicked() {
                cancel_sign_in(app);
            }
        } else if signed {
            let mut who = if user.is_empty() { tr("Signed in").to_string() } else { format!("{} {user}", tr("Signed in as")) };
            if via == "token" {
                who = format!("{who} {}", tr("(access token)"));
            }
            ui.label(small(&who, t.text_dim));
            let out = action(ui, tr("Sign out"), t.text_dim)
                .on_hover_text(tr("Forgets the sign-in on this PC. To disconnect the app from your account too, use Connected apps on Hugging Face"));
            if out.clicked() {
                sign_out(app);
            }
            if via == "oauth" {
                link(app, ui, tr("Connected apps"), CONNECTED_APPS_URL);
            }
        } else {
            let waiting: Vec<Value> = needs.iter().filter(|m| access_of(app, text(m, "/id")) != "unavailable").cloned().collect();
            if waiting.is_empty() {
                ui.label(small(tr("Not signed in"), t.text_faint));
            } else {
                let need = if waiting.len() == 1 { tr("needs a free Hugging Face account.") } else { tr("need a free Hugging Face account.") };
                ui.label(small(&format!("{} {need}", names_of(&waiting)), t.text_dim));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| link(app, ui, tr("How does this work?"), HELP_URL));
    });
    if !signed && !signing && !forgotten.is_empty() {
        ui.add(egui::Label::new(small(&forgotten, t.warning)).wrap());
    }
    // Signed in: the licences still to accept, each with the button that opens its page.
    if signed && !signing {
        for model in &licence {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let next = format!("{} {} {}", tr("Next: accept the licence of"), text(model, "/name"), tr("on Hugging Face."));
                ui.label(small(&next, t.text_dim));
                if checking {
                    ui.add(egui::Spinner::new().size(12.0));
                } else {
                    let page = text(model, "/page").to_string();
                    if widgets::primary_button(ui, tr("Accept the Licence"), 0.0).on_hover_text(&page).clicked() {
                        crate::links::open(app, ui.ctx(), &page);
                    }
                    if action(ui, tr("Check again"), t.text_dim).clicked() {
                        check_access(app);
                    }
                }
            });
        }
        for model in &gone {
            ui.add(egui::Label::new(small(&detail_of(app, text(model, "/id")), t.danger)).wrap().selectable(true));
        }
    }
    // The access token: the fallback while not signed in with Hugging Face.
    if signed && via == "oauth" {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        // Not signed in: the one button that does it, first on the line.
        if !signed && !signing && can && widgets::primary_button(ui, tr("Sign In with Hugging Face"), 0.0).clicked() {
            sign_in(app, ui.ctx());
        }
        let toggled = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let (mark, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                crate::icons::paint(ui, mark, if app.ui.ai_setup.token_open { "chevron-down" } else { "chevron-right" }, 11.0, t.text_dim);
                action(ui, tr("Use an access token instead"), t.text_dim).clicked()
            })
            .inner;
        if toggled {
            app.ui.ai_setup.token_open = !app.ui.ai_setup.token_open;
        }
        if hf_flag(app, "found") && !signed {
            let r = action(ui, tr("Use the sign-in found on this PC"), t.accent)
                .on_hover_text(tr("A Hugging Face token is saved on this PC (by huggingface-cli, or in HF_TOKEN). It is used only if you click here."));
            if r.clicked() {
                use_found(app);
            }
        }
    });
    if app.ui.ai_setup.token_open {
        ui.horizontal(|ui| {
            let hint = if signed && via == "token" { tr("Saved. Paste a new one to replace it") } else { tr("Paste a read token") };
            ui.add(egui::TextEdit::singleline(&mut app.ui.ai_setup.token).password(true).desired_width(260.0).hint_text(hint));
            let typed = !app.ui.ai_setup.token.trim().is_empty();
            if ui.add_enabled_ui(typed, |ui| widgets::secondary_button(ui, tr("Save"), 64.0)).inner.clicked() {
                save_token(app);
            }
            link(app, ui, tr("Create a token"), TOKENS_URL);
        });
        let hint = "For organisation token policies, or if signing in doesn't work. A read token is enough (or a fine-grained one that can read public gated repos). It is sent only to huggingface.co.";
        ui.add(egui::Label::new(RichText::new(tr(hint)).color(t.text_faint).size(11.0)).wrap());
    }
}

fn eta(seconds: u64) -> String {
    match seconds {
        s if s < 90 => format!("{} {s} s", tr("about")),
        s if s < 5400 => format!("{} {} min", tr("about"), (s + 30) / 60),
        s => format!("{} {:.1} h", tr("about"), s as f64 / 3600.0),
    }
}

/// A row of the Install screen in words that say where it stands: what is coming (a plain
/// name), what is being done now ("Downloading …", "Starting ComfyUI…", what the engine says
/// while it renders), or what was done. Never words that read like something for the user to do.
fn step_words(step: &Value) -> String {
    let (key, label, note, state) = (text(step, "/key"), text(step, "/label"), text(step, "/note").trim_end_matches('…'), text(step, "/state"));
    let file = key.strip_prefix("file:").map(|_| label);
    let words = match (state, key, file) {
        ("active", _, Some(name)) if !note.is_empty() => format!("{}: {name}", tr(note)),
        ("active", _, Some(name)) => format!("{} {name}", tr("Downloading")),
        ("active", "runtime", _) => tr("Downloading the ComfyUI engine").to_string(),
        ("active", "unpack", _) => tr("Unpacking the engine").to_string(),
        ("active", "paths", _) => tr("Pointing the engine at your models").to_string(),
        ("active", "configure", _) => tr("Setting up the models").to_string(),
        // The engine reports each stage itself: starting, loading the model, rendering.
        ("active", "verify", _) if !note.is_empty() => tr(note).to_string(),
        ("active", "verify", _) => tr("Starting the engine").to_string(),
        ("done", "runtime", _) => tr("ComfyUI engine downloaded").to_string(),
        ("done", "unpack", _) => tr("Engine unpacked").to_string(),
        ("done", "paths", _) => tr("Engine pointed at your models").to_string(),
        ("done", "configure", _) => tr("Models set up").to_string(),
        ("done", "verify", _) if !note.is_empty() => format!("{} · {}", tr("Engine started"), tr(note)),
        ("done", "verify", _) => tr("Engine started, test image rendered").to_string(),
        (_, "runtime", _) => label.to_string(),
        (_, "unpack", _) => tr("Engine unpacking").to_string(),
        (_, "paths", _) => tr("The engine's model folders").to_string(),
        (_, "configure", _) => tr("Model settings").to_string(),
        (_, "verify", _) => tr("Engine start and a test image").to_string(),
        (_, _, Some(name)) if !note.is_empty() && state != "done" => format!("{name} · {}", tr(note)),
        _ => tr(label).to_string(),
    };
    // Busy, unless it ends on a running clock ("· 3:12"), which says so itself.
    let clock = words.rsplit_once(" \u{b7} ").is_some_and(|(_, end)| !end.is_empty() && end.chars().all(|c| c.is_ascii_digit() || c == ':'));
    if state == "active" && !clock { format!("{words}\u{2026}") } else { words }
}

/// What went wrong, in full, for the error window: the setup's own error (or the record's, from
/// a run before), what a failed row says beyond it, and what the last thing asked for said.
/// Empty when nothing did.
fn failure(app: &PhotocraftApp) -> String {
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let mut said: Vec<String> = Vec::new();
    let mut add = |line: &str| {
        let line = line.trim();
        if !line.is_empty() && !said.iter().any(|s| s.contains(line)) {
            said.push(line.to_string());
        }
    };
    if !running(app) && matches!(text(&status, "/state"), "failed" | "interrupted") {
        add(text(&status, "/result/error"));
        add(text(&status, "/record/error"));
    }
    for step in list(&status, "/progress/steps").iter().filter(|s| text(s, "/state") == "failed") {
        add(text(step, "/note"));
    }
    add(&app.ui.ai_setup.note);
    said.join("\n\n")
}

fn page_install(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let busy = running(app);
    let sub = if busy {
        "The editor stays usable. You can close this window: the setup carries on, and the status bar shows how far it is."
    } else {
        "The setup is not running."
    };
    heading(ui, if busy { "Installing" } else { "Install" }, sub);
    let progress = status.get("progress").cloned().unwrap_or(Value::Null);
    let (done, total) = (number(&progress, "/bytesDone"), number(&progress, "/bytesTotal"));
    if total > 0 {
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
        crate::jobs_ui::bar(ui, bar, Some((done as f64 / total as f64).clamp(0.0, 1.0) as f32), &t);
        ui.add_space(4.0);
        let speed = progress.get("speed").and_then(Value::as_f64).unwrap_or(0.0);
        let mut line = format!("{} / {}", size(done), size(total));
        if busy && speed > 1.0 {
            line = format!("{line} · {}/s", size(speed as u64));
        }
        if let Some(seconds) = progress.get("etaSeconds").and_then(Value::as_u64).filter(|_| busy) {
            line = format!("{line} · {} {}", eta(seconds), tr("left"));
        }
        ui.label(RichText::new(line).color(t.text_dim).size(12.0));
        ui.add_space(6.0);
    }
    // What the setup is doing this moment, where it is seen: a spinner and the step in words.
    let steps = list(&progress, "/steps").to_vec();
    let active = steps.iter().find(|s| text(s, "/state") == "active");
    if busy {
        let now = active.map_or_else(|| tr(text(&progress, "/message")).to_string(), step_words);
        // The engine's step can take many minutes (on the processor, an hour): what it is doing
        // is a click away, in its own words.
        let engine_step = active.is_some_and(|s| text(s, "/key") == "verify");
        let mut details = false;
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0).color(t.accent));
            ui.label(RichText::new(now).font(crate::theme::medium(13.0)).color(t.text));
            if engine_step {
                ui.add_space(8.0);
                let r = ui.add(egui::Label::new(RichText::new(tr("What's happening?")).color(t.accent).size(12.0)).sense(Sense::click()));
                details = r
                    .on_hover_text(tr("What the engine is doing, and its own last lines, live"))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
            }
        });
        if details {
            app.ui.ai_setup.ask = Some(Ask::Activity);
        }
        ui.add_space(8.0);
        // The spinner and the active row's bar move between the quarter-second status reads.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    let active_key = active.map(|s| text(s, "/key").to_string()).unwrap_or_default();
    // The rows take the page down to the footer, which says how it ended (paused, or stopped
    // with an error, in a window of its own).
    let mut show_error = false;
    egui::ScrollArea::vertical().id_salt("ai-setup-steps").max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
        for step in &steps {
            let state = text(step, "/state");
            let on = state == "active";
            let rect = row_frame(ui, 34.0);
            // A failed row opens the whole error, as its Show error link does.
            let failed = state == "failed" && !busy;
            if failed {
                let row = ui.interact(rect, ui.id().with(("ai-setup-failed-row", text(step, "/key"))), Sense::click());
                if row.hovered() {
                    ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, t.danger), StrokeKind::Inside);
                }
                show_error |= row.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            }
            if on {
                // The row being worked on stands out, and comes into view once when it starts.
                ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                if app.ui.ai_setup.shown_active != active_key {
                    ui.scroll_to_rect(rect, None);
                    app.ui.ai_setup.shown_active.clone_from(&active_key);
                }
            }
            let mark = Rect::from_min_size(rect.min + vec2(10.0, 9.0), vec2(16.0, 16.0));
            match state {
                "active" => {
                    let mut child = ui.new_child(UiBuilder::new().max_rect(mark));
                    child.add(egui::Spinner::new().size(14.0).color(t.accent));
                }
                "done" => crate::icons::paint(ui, mark, "check", 14.0, t.accent),
                "failed" => crate::icons::paint(ui, mark, "x", 14.0, t.danger),
                "skipped" => crate::icons::paint(ui, mark, "info", 14.0, t.warning),
                _ => crate::icons::paint(ui, mark, "circle", 14.0, t.text_faint),
            }
            let (step_done, step_total) = (number(step, "/done"), number(step, "/total"));
            let right = match (state, step_total) {
                ("skipped" | "failed", _) => String::new(),
                (_, 0) => String::new(),
                ("done", total) => size(total),
                (_, total) => format!("{} / {}", size(step_done), size(total)),
            };
            let right_width = if failed {
                let link = Rect::from_min_max(pos2(rect.right() - 150.0, rect.top()), rect.max - vec2(12.0, 0.0));
                let mut child = ui.new_child(UiBuilder::new().max_rect(link).layout(Layout::right_to_left(Align::Center)));
                let r = child.add(egui::Label::new(RichText::new(tr("Show error")).color(t.accent).size(12.0)).sense(Sense::click()));
                show_error |= r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                150.0
            } else if right.is_empty() {
                0.0
            } else {
                150.0
            };
            let label = step_words(step);
            let label_rect = Rect::from_min_max(rect.min + vec2(36.0, 0.0), rect.max - vec2(right_width + 12.0, 0.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(label_rect).layout(Layout::left_to_right(Align::Center)));
            if on {
                child.style_mut().visuals.override_text_color = Some(t.text);
            } else if state == "pending" {
                child.style_mut().visuals.override_text_color = Some(t.text_dim);
            }
            widgets::elided_label(&mut child, &label, label_rect.width()).on_hover_text(&label);
            if !right.is_empty() {
                ui.painter().text(pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, right, crate::theme::mono(11.5), t.text_dim);
            }
            if on {
                // Its own progress when it has a size; otherwise a bar that moves, so a step that
                // can't say how far it is (starting the engine, rendering) still shows it works.
                let track = Rect::from_min_max(pos2(rect.left() + 36.0, rect.bottom() - 6.0), pos2(rect.right() - 12.0, rect.bottom() - 3.0));
                let fraction = (step_total > 0).then(|| (step_done as f64 / step_total as f64).clamp(0.0, 1.0) as f32);
                crate::jobs_ui::bar(ui, track, fraction, &t);
            }
            ui.add_space(4.0);
        }
    });
    if show_error {
        app.ui.ai_setup.ask = Some(Ask::Error);
    }
}

fn page_ready(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let result = status.get("result").cloned().unwrap_or(Value::Null);
    let ready = list(&result, "/ready").len();
    let sub = if text(&result, "/route") == "existing" {
        "PhotoCraft is connected to your ComfyUI, and its models are set up."
    } else {
        "The engine starts by itself when a local model needs it, and stops when you quit."
    };
    heading(ui, "Local AI is ready", sub);
    let image = test_image(app, ui.ctx());
    ui.horizontal_top(|ui| {
        if let Some(tex) = &image {
            let side = 250.0;
            let [w, h] = tex.size();
            let scale = side / (w.max(h).max(1) as f32);
            let (rect, _) = ui.allocate_exact_size(vec2(w as f32 * scale, h as f32 * scale), Sense::hover());
            ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.card_border), StrokeKind::Inside);
            ui.add_space(14.0);
        }
        ui.vertical(|ui| {
            if let Some(test) = result.get("testImage").filter(|v| v.is_object()) {
                let seconds = test.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
                ui.label(RichText::new(tr("Your first image")).font(crate::theme::medium(14.0)).color(t.text));
                ui.label(
                    RichText::new(format!("{} {} {} {seconds:.1} s", tr("Rendered with"), text(test, "/modelName"), tr("in"))).color(t.text_dim).size(12.5),
                );
                ui.label(RichText::new(tr("The first image takes longest: the model had to be loaded.")).color(t.text_faint).size(11.5));
                ui.add_space(8.0);
            }
            let models = if ready == 1 {
                tr("1 local model is listed in the AI Gen panel.").to_string()
            } else {
                format!("{ready} {}", tr("local models are listed in the AI Gen panel."))
            };
            ui.label(RichText::new(models).color(t.text).size(13.0));
            ui.add_space(6.0);
            for skipped in list(&result, "/skipped") {
                let line = format!("{}: {}", text(skipped, "/name"), tr(text(skipped, "/skipped")));
                ui.add(egui::Label::new(RichText::new(line).color(t.warning).size(12.0)).wrap());
            }
            let why = text(&result, "/testError");
            if !why.is_empty() {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(why).color(t.text_dim).size(12.0)).wrap().selectable(true));
            }
        });
    });
}

fn footer(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    widgets::hairline(ui);
    ui.add_space(10.0);
    let (page, existing) = (app.ui.ai_setup.page, app.ui.ai_setup.existing);
    let (can_install, why_not) = (can_install_new(app), cant_install_why(app));
    let plan = app.ui.ai_setup.plan.clone().unwrap_or(Value::Null);
    let choosing = page == Page::Pc && offer_choice(app);
    // Right: where to next, laid out first; the left takes the room left of it, wrapping rather
    // than running under the buttons.
    let row = ui.available_rect_before_wrap();
    let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)));
    footer_buttons(app, &mut right, &plan, can_install, choosing);
    let left_edge = (right.min_rect().left() - 16.0).max(row.left());
    let mut left = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(row.min, pos2(left_edge, row.bottom()))).layout(Layout::left_to_right(Align::Center)));
    ui.advance_cursor_after_rect(row);
    {
        let ui = &mut left;
        // Left: the other ways in, or what the screen adds up to.
        match page {
            Page::Pc => {
                // The big buttons above ask which ComfyUI once this PC has been looked at; on a
                // setup that is there already, the user's own is a button here.
                if !choosing && !first_look(app) {
                    let r = widgets::secondary_button(ui, tr("Use My ComfyUI"), 0.0);
                    if r.on_hover_text(tr("Connect the ComfyUI you have. Nothing is installed; only missing model files are fetched")).clicked() {
                        use_existing(app);
                    }
                }
                let r = widgets::secondary_button(ui, tr("Cloud Models Only"), 0.0);
                if r.on_hover_text(tr("Skip local models: add an API key for OpenAI, Google, xAI, Meta or Black Forest Labs instead")).clicked() {
                    app.ui.ai_setup.open = false;
                    crate::ai_gen_ui::open_api_keys(app);
                }
            }
            Page::Location if existing => {
                let instead = ui.add_enabled_ui(can_install, |ui| widgets::secondary_button(ui, tr("Install New ComfyUI Instead"), 0.0)).inner;
                if instead
                    .on_hover_text(tr("A private copy with its own Python, set up for PhotoCraft. Your ComfyUI is left as it is"))
                    .on_disabled_hover_text(why_not)
                    .clicked()
                {
                    use_managed(app);
                }
            }
            Page::Models => {
                let download = number(&plan, "/downloadBytes");
                let skipped = list(&plan, "/models").iter().filter(|m| m.get("skipped").is_some_and(|s| !s.is_null())).count();
                let mut line = if download == 0 {
                    tr("Nothing to download: everything ticked is here.").to_string()
                } else {
                    format!("{} {}", tr("To download:"), download_size(download))
                };
                if let Some(free) = plan.get("modelsFree").and_then(Value::as_u64) {
                    line = format!("{line} · {} {}", size(free), tr("free"));
                }
                if skipped > 0 {
                    line = format!("{line} · {skipped} {}", tr("skipped (needs sign-in)"));
                }
                // Why Install can't be pressed, where it can be seen (untick models to make room).
                let color = match list(&plan, "/problems").first().and_then(Value::as_str) {
                    Some(why) => {
                        line = why.to_string();
                        t.danger
                    }
                    None => t.text_dim,
                };
                ui.add(egui::Label::new(RichText::new(line).color(color).size(12.0)).wrap());
            }
            // How it ended, when it isn't running: paused, or stopped with an error (the whole
            // of which is in a window of its own).
            Page::Install if !running(app) => {
                if !failure(app).is_empty() {
                    let (mark, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    crate::icons::paint(ui, mark, "x", 13.0, t.danger);
                    ui.label(RichText::new(tr("The setup stopped with an error.")).color(t.danger).size(12.0));
                    let r = ui.add(egui::Label::new(RichText::new(tr("Show error")).color(t.accent).size(12.0)).sense(Sense::click()));
                    if r.on_hover_text(tr("The whole message, to read or copy")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        app.ui.ai_setup.ask = Some(Ask::Error);
                    }
                } else if app.ui.ai_setup.status.as_ref().is_some_and(|s| text(s, "/state") == "paused") {
                    ui.label(RichText::new(tr("Paused. What arrived is kept.")).color(t.text_dim).size(12.0));
                }
            }
            _ => {}
        }
    }
}

/// Why the Location page can't go on to the Models page, if it can't. Only what another folder
/// fixes stops it: too little room for the models is the Models page's to fix, by ticking fewer.
fn location_blocked(app: &PhotocraftApp) -> Option<&'static str> {
    let s = &app.ui.ai_setup;
    if s.existing && s.server.is_none() {
        return Some("Connect to your ComfyUI first");
    }
    match &s.plan {
        Some(plan) if list(plan, "/folderProblems").is_empty() => None,
        _ => Some("Choose folders that have the room"),
    }
}

/// The footer's buttons, laid out right to left: where to next from the screen in front.
fn footer_buttons(app: &mut PhotocraftApp, ui: &mut egui::Ui, plan: &Value, can_install: bool, choosing: bool) {
    let (page, busy) = (app.ui.ai_setup.page, running(app));
    match page {
        Page::Pc => {
            // The big buttons above stand for Next; while this PC is first looked at, nothing does.
            if !choosing && !first_look(app) {
                let ready = app.ui.ai_setup.probe.is_some() && can_install;
                let next = ui.add_enabled_ui(ready, |ui| widgets::primary_button(ui, tr("Next"), 96.0)).inner;
                let why = if gpu_usable(app) {
                    tr("PhotoCraft can't install the engine on this system. Use my ComfyUI, or cloud models, still work")
                } else {
                    cant_install_why(app)
                };
                if next.on_disabled_hover_text(why).clicked()
                {
                    use_managed(app);
                }
            }
            if ui.add_enabled_ui(app.ui.ai_setup.probe_job.is_none(), |ui| widgets::secondary_button(ui, tr("Check Again"), 110.0)).inner.clicked() {
                app.ui.ai_setup.probe = None;
                app.ui.ai_setup.note.clear();
                start_probe(app);
            }
        }
        Page::Location => {
            let blocked = location_blocked(app);
            let next = ui.add_enabled_ui(blocked.is_none(), |ui| widgets::primary_button(ui, tr("Next"), 96.0)).inner;
            if next.on_disabled_hover_text(tr(blocked.unwrap_or_default())).clicked() {
                app.ui.ai_setup.page = Page::Models;
            }
            if widgets::secondary_button(ui, tr("Back"), 80.0).clicked() {
                // What went wrong here (Connect, say) is not the first screen's to show.
                app.ui.ai_setup.note.clear();
                app.ui.ai_setup.page = Page::Pc;
            }
        }
        Page::Models => {
            let problems = !list(plan, "/problems").is_empty() || app.ui.ai_setup.plan.is_none();
            let label = if number(plan, "/downloadBytes") == 0 { tr("Finish") } else { tr("Install") };
            let go = ui.add_enabled_ui(!problems && !busy, |ui| widgets::primary_button(ui, label, 96.0)).inner;
            let why = list(plan, "/problems").first().and_then(Value::as_str).unwrap_or("Choose folders first").to_string();
            if go.on_disabled_hover_text(why).clicked() {
                // A gated model that can't be downloaded yet is asked about first.
                install_clicked(app);
            }
            if widgets::secondary_button(ui, tr("Back"), 80.0).clicked() {
                app.ui.ai_setup.page = Page::Location;
            }
        }
        Page::Install => {
            if busy {
                if widgets::secondary_button(ui, tr("Pause"), 80.0)
                    .on_hover_text(tr("Stop for now. What arrived is kept, and Continue picks up from there"))
                    .clicked()
                {
                    let _ = app.session.execute("ai.setup.cancel", json!({}));
                }
                if widgets::secondary_button(ui, tr("Close"), 80.0).clicked() {
                    // Keep downloading in the background, or stop: asked.
                    app.ui.ai_setup.ask = Some(Ask::Close);
                }
            } else {
                let state = app.ui.ai_setup.status.as_ref().map_or("", |s| text(s, "/state")).to_string();
                if matches!(state.as_str(), "paused" | "failed" | "interrupted") && widgets::primary_button(ui, tr("Continue"), 110.0).clicked() {
                    resume(app);
                }
                if state == "done" && widgets::primary_button(ui, tr("Next"), 96.0).clicked() {
                    app.ui.ai_setup.page = Page::Ready;
                }
                if widgets::secondary_button(ui, tr("Back"), 80.0).clicked() {
                    app.ui.ai_setup.page = Page::Models;
                }
            }
        }
        Page::Ready => {
            if widgets::primary_button(ui, tr("Open AI Gen"), 120.0).clicked() {
                app.ui.ai_setup.open = false;
                let _ = crate::ai_gen_ui::menu(app, crate::ai_gen_ui::PANEL, &json!({"show": true}));
            }
            if widgets::secondary_button(ui, tr("Add Models…"), 110.0).clicked() {
                app.ui.ai_setup.page = Page::Models;
            }
        }
    }
}

/// The wizard window.
pub fn window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    tick(app, ctx);
    if !app.ui.ai_setup.open {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let (w, h) = (WIDTH.min(screen.width() - 24.0).max(520.0), HEIGHT.min(screen.height() - 24.0).max(360.0));
    let mut close = false;
    egui::Window::new(tr("PhotoCraft Setup: Local AI"))
        .id(egui::Id::new("ai-setup"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_size([w, h])
        .default_pos(screen.center() - vec2(w, h) / 2.0)
        .frame(egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.radius_lg).shadow(egui::Shadow {
            offset: [0, 12],
            blur: 36,
            spread: 0,
            color: t.shadow,
        }))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            // The title row.
            let title = Rect::from_min_size(rect.min, vec2(w, 40.0));
            crate::brand::paint_mark(ui, Rect::from_min_size(title.min + vec2(14.0, 10.0), vec2(20.0, 20.0)));
            ui.painter().text(
                pos2(title.left() + 44.0, title.center().y),
                egui::Align2::LEFT_CENTER,
                tr("PhotoCraft Setup: Local AI"),
                crate::theme::medium(13.0),
                t.text_dim,
            );
            let mut bar = ui.new_child(UiBuilder::new().max_rect(title.shrink2(vec2(10.0, 8.0))).layout(Layout::right_to_left(Align::Center)));
            close = crate::icons::button(&mut bar, "x", 24.0, false, tr("Close")).clicked();
            ui.painter().line_segment([title.left_bottom(), title.right_bottom()], Stroke::new(1.0, t.separator));
            // The steps on the left.
            let side = Rect::from_min_max(pos2(rect.left(), title.bottom()), pos2(rect.left() + SIDEBAR, rect.bottom()));
            ui.painter().line_segment([side.right_top(), side.right_bottom()], Stroke::new(1.0, t.separator));
            let mut steps = ui.new_child(UiBuilder::new().max_rect(side.shrink2(vec2(12.0, 10.0))).layout(Layout::top_down(Align::Min)));
            sidebar(app, &mut steps);
            // The screen in front, and under it where to next.
            let content = Rect::from_min_max(pos2(side.right(), title.bottom()), rect.max).shrink2(vec2(28.0, 18.0));
            let foot = Rect::from_min_max(pos2(content.left(), content.bottom() - 46.0), content.max);
            // Which ComfyUI to use is asked above the footer, where it is seen.
            let choice_height = if app.ui.ai_setup.page == Page::Pc && offer_choice(app) { CHOICE } else { 0.0 };
            let body = Rect::from_min_max(content.min, pos2(content.right(), foot.top() - 8.0 - choice_height));
            if choice_height > 0.0 {
                let strip = Rect::from_min_max(pos2(content.left(), body.bottom() + 12.0), pos2(content.right(), foot.top() - 4.0));
                let mut asked = ui.new_child(UiBuilder::new().max_rect(strip).layout(Layout::top_down(Align::Min)));
                choice(app, &mut asked);
            }
            let mut page = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
            page.set_clip_rect(body.expand(2.0));
            match app.ui.ai_setup.page {
                Page::Pc if first_look(app) => page_checking(app, &mut page),
                Page::Pc => {
                    egui::ScrollArea::vertical().id_salt("ai-setup-pc").auto_shrink([false, false]).show(&mut page, |ui| page_pc(app, ui));
                }
                Page::Location => {
                    egui::ScrollArea::vertical().id_salt("ai-setup-location").auto_shrink([false, false]).show(&mut page, |ui| page_location(app, ui));
                }
                Page::Models => page_models(app, &mut page),
                Page::Install => page_install(app, &mut page),
                Page::Ready => page_ready(app, &mut page),
            }
            let mut bottom = ui.new_child(UiBuilder::new().max_rect(foot).layout(Layout::top_down(Align::Min)));
            footer(app, &mut bottom);
        });
    if close {
        if running(app) {
            app.ui.ai_setup.ask = Some(Ask::Close);
        } else {
            app.ui.ai_setup.open = false;
        }
    }
    questions(app, ctx);
}

/// The question over the wizard, when one is asked.
fn questions(app: &mut PhotocraftApp, ctx: &egui::Context) {
    match app.ui.ai_setup.ask {
        Some(Ask::Close) => close_question(app, ctx),
        Some(Ask::SignIn) => sign_in_question(app, ctx),
        Some(Ask::NotRunning) => not_running_question(app, ctx),
        Some(Ask::Error) => error_window(app, ctx),
        Some(Ask::Activity) => activity_window(app, ctx),
        None => {}
    }
}

/// The error window's Copy was pressed since it opened: its button says Copied.
const ERROR_COPIED: &str = "ai-setup-error-copied";

/// The setup stopped with an error: all of it (a Python traceback, say), selectable and scrolled
/// when long, with Copy for the whole of it.
fn error_window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use widgets::{ButtonRole, DialogButton};
    let message = failure(app);
    if message.is_empty() {
        // Continued, or cleared, meanwhile: nothing to show.
        error_answered(app, ctx, ButtonRole::Cancel);
        return;
    }
    let t = Tokens::get(ctx);
    let copied = ctx.data(|d| d.get_temp::<bool>(egui::Id::new(ERROR_COPIED))).unwrap_or(false);
    let mut answer = None;
    let buttons =
        [DialogButton::new(ButtonRole::Default, tr("Close"), 84.0), DialogButton::new(ButtonRole::Apply, if copied { tr("Copied") } else { tr("Copy") }, 84.0)];
    let screen = ctx.content_rect();
    let width = 600.0f32.min(screen.width() - 64.0).max(320.0);
    let modal = egui::Modal::new(egui::Id::new("ai-setup-error")).show(ctx, |ui| {
        ui.set_width(width);
        ui.label(RichText::new(tr("The setup stopped with an error")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        let why = tr("This is what the engine said. Copy it to look it up, or to send with a bug report. Continue tries again from where the setup stopped.");
        ui.add(egui::Label::new(RichText::new(why).color(t.text_dim).size(12.5)).wrap());
        ui.add_space(10.0);
        egui::Frame::NONE.fill(t.field).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.radius).inner_margin(10.0).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ai-setup-error-text")
                .max_height((screen.height() - 300.0).clamp(120.0, 360.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&message).font(crate::theme::mono(11.5)).color(t.danger)).wrap().selectable(true));
                });
        });
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    if let Some(answer) = answer {
        error_answered(app, ctx, answer);
    }
}

/// What's happening? was pressed: the engine's step in words (what it is doing, for how long),
/// and the engine's own last lines, as they come. Copy takes them; Close closes it.
fn activity_window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use widgets::{ButtonRole, DialogButton};
    let t = Tokens::get(ctx);
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let steps = list(&status, "/progress/steps").to_vec();
    let step = steps.iter().find(|s| text(s, "/key") == "verify");
    let now = match step.map(|s| text(s, "/state")) {
        Some("active") => step.map(step_words).unwrap_or_default(),
        Some("done") => tr("The engine is running.").to_string(),
        Some("failed") => tr("The engine stopped with an error: Show error says what it was.").to_string(),
        _ => tr("The engine has not been started yet.").to_string(),
    };
    let lines: Vec<String> = list(&status, "/engine/output").iter().filter_map(Value::as_str).map(str::to_string).collect();
    let log = text(&status, "/engine/log").to_string();
    let mut answer = None;
    let buttons = [DialogButton::new(ButtonRole::Default, tr("Close"), 84.0), DialogButton::new(ButtonRole::Apply, tr("Copy"), 84.0)];
    let screen = ctx.content_rect();
    let width = 640.0f32.min(screen.width() - 64.0).max(320.0);
    let modal = egui::Modal::new(egui::Id::new("ai-setup-activity")).show(ctx, |ui| {
        ui.set_width(width);
        ui.label(RichText::new(tr("What the engine is doing")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if step.is_some_and(|s| text(s, "/state") == "active") {
                ui.add(egui::Spinner::new().size(13.0).color(t.accent));
            }
            ui.add(egui::Label::new(RichText::new(&now).font(crate::theme::medium(13.0)).color(t.text)).wrap());
        });
        ui.add_space(4.0);
        let why = tr(
            "Below are the engine's own last lines, as it prints them. Starting it and loading a model take minutes; on the processor, without a graphics card, a picture can take an hour. While the lines change, it is working.",
        );
        ui.add(egui::Label::new(RichText::new(why).color(t.text_dim).size(12.0)).wrap());
        ui.add_space(10.0);
        egui::Frame::NONE.fill(t.field).stroke(Stroke::new(1.0, t.card_border)).corner_radius(t.radius).inner_margin(10.0).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ai-setup-activity-lines")
                .max_height((screen.height() - 320.0).clamp(120.0, 340.0))
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if lines.is_empty() {
                        ui.label(RichText::new(tr("The engine has not printed anything yet.")).color(t.text_faint).size(12.0));
                    }
                    for line in &lines {
                        ui.add(egui::Label::new(RichText::new(line).font(crate::theme::mono(11.0)).color(t.text_dim)).wrap().selectable(true));
                    }
                });
        });
        if !log.is_empty() {
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(format!("{} {log}", tr("All of it:"))).color(t.text_faint).size(11.5)).wrap().selectable(true));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    // The lines come with the status, read a few times a second while the setup runs.
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    match answer {
        Some(ButtonRole::Apply) => ctx.copy_text(format!("{now}\n\n{}", lines.join("\n"))),
        Some(_) => app.ui.ai_setup.ask = None,
        None => {}
    }
}

/// The error window answered: Copy puts the whole message on the clipboard and keeps the window
/// open (its button says Copied); Close closes it, and Show error opens it again.
fn error_answered(app: &mut PhotocraftApp, ctx: &egui::Context, answer: widgets::ButtonRole) {
    let copied_id = egui::Id::new(ERROR_COPIED);
    if answer == widgets::ButtonRole::Apply {
        ctx.copy_text(failure(app));
        ctx.data_mut(|d| d.insert_temp(copied_id, true));
    } else {
        app.ui.ai_setup.ask = None;
        ctx.data_mut(|d| d.remove::<bool>(copied_id));
    }
}

/// Connect found no ComfyUI, at the address or on the other port: start it, then Try Again.
fn not_running_question(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use widgets::{ButtonRole, DialogButton};
    let tried = photocraft_aigen::launch::addresses(&app.ui.ai_setup.url);
    let mut answer = None;
    let buttons = [DialogButton::new(ButtonRole::Default, tr("Try Again"), 104.0), DialogButton::new(ButtonRole::Cancel, tr("Cancel"), 84.0)];
    let modal = egui::Modal::new(egui::Id::new("ai-setup-not-running")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.label(RichText::new(tr("Couldn't connect to ComfyUI")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        let message = format!(
            "{} {}. {}",
            tr("Nothing answered at"),
            tried.join(&format!(" {} ", tr("or"))),
            tr("Launch ComfyUI, make sure it is running (wait until its window has finished loading), then press Try Again.")
        );
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    if let Some(answer) = answer {
        app.ui.ai_setup.ask = None;
        if answer == ButtonRole::Default {
            connect(app);
        }
    }
}

/// Close, while a setup runs: carry on in the background (the status bar brings the wizard
/// back), or stop the downloads (what arrived is kept).
fn close_question(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use widgets::{ButtonRole, DialogButton};
    if !running(app) {
        // It ended while the question was open: nothing to decide.
        app.ui.ai_setup.ask = None;
        app.ui.ai_setup.open = false;
        return;
    }
    let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
    let (done, total) = (number(&status, "/progress/bytesDone"), number(&status, "/progress/bytesTotal"));
    let how_far = if total > 0 && done < total { format!(" {} {} {} {}.", tr("So far"), size(done), tr("of"), size(total)) } else { String::new() };
    let mut answer = None;
    let buttons = [
        DialogButton::new(ButtonRole::Default, tr("Keep Downloading"), 140.0),
        DialogButton::new(ButtonRole::Alternate, tr("Stop Download"), 120.0),
        DialogButton::new(ButtonRole::Cancel, tr("Cancel"), 84.0),
    ];
    let modal = egui::Modal::new(egui::Id::new("ai-setup-close")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.label(RichText::new(tr("Keep downloading in the background?")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        let message = format!(
            "{}{how_far} {}",
            tr("Local AI is still being set up."),
            tr("It can carry on while you work: the status bar shows how far it is, and clicking it brings this window back. Stop Download pauses it; what arrived is kept, and Set Up Local AI continues from there.")
        );
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    if let Some(answer) = answer {
        close_answered(app, answer);
    }
}

/// Close's question answered: keep downloading in the background (the window closes; the
/// status bar brings it back), stop (the setup pauses, what arrived is kept, the window
/// closes), or neither (back to the wizard).
fn close_answered(app: &mut PhotocraftApp, answer: widgets::ButtonRole) {
    use widgets::ButtonRole;
    app.ui.ai_setup.ask = None;
    match answer {
        ButtonRole::Default => app.ui.ai_setup.open = false,
        ButtonRole::Alternate => {
            let _ = app.session.execute("ai.setup.cancel", json!({}));
            app.ui.ai_setup.open = false;
        }
        _ => {}
    }
}

/// Install, with a gated model ticked that can't be downloaded yet: the two steps it takes (Sign
/// in with Hugging Face, Accept the licence), each where it stands; then Install, or install
/// without it.
fn sign_in_question(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use widgets::{ButtonRole, DialogButton};
    let needs = needs_sign_in(app);
    if needs.is_empty() {
        // Unticked, or there already, meanwhile: nothing to ask.
        app.ui.ai_setup.ask = None;
        return;
    }
    let is = |m: &Value, wanted: &str| access_of(app, text(m, "/id")) == wanted;
    // A model whose file moved can't come at all: no sign-in helps, so it is not one of the steps.
    let (steps, gone): (Vec<Value>, Vec<Value>) = needs.iter().cloned().partition(|m| !is(m, "unavailable"));
    let blocked: Vec<Value> = needs.iter().filter(|m| !is(m, "ready")).cloned().collect();
    let first = steps.first().cloned().unwrap_or(Value::Null);
    let t = Tokens::get(ctx);
    let names = names_of(if steps.is_empty() { &gone } else { &steps });
    let hf = app.ui.ai_setup.hf.clone().unwrap_or(Value::Null);
    let signed = hf_flag(app, "signedIn");
    let signing = app.session.ai_setup.signing_in();
    let checking = app.ui.ai_setup.access_job.is_some();
    let can = hf_flag(app, "canSignIn");
    let ready = steps.iter().all(|m| is(m, "ready"));
    let licence_left = steps.iter().any(|m| is(m, "licence"));
    let user = text(&hf, "/user").to_string();
    let (mut answer, mut go_sign_in, mut go_licence, mut recheck, mut use_token, mut cancel) = (None, false, false, false, false, false);
    let without = format!("{} {}", tr("Install Without"), match blocked.as_slice() { [one] => text(one, "/name"), _ => tr("Them") });
    let buttons = [
        DialogButton::new(ButtonRole::Default, tr("Install"), 96.0).enabled(all_ready(app) && !checking),
        DialogButton::new(ButtonRole::Alternate, &without, 120.0),
        DialogButton::new(ButtonRole::Cancel, tr("Cancel"), 84.0),
    ];
    let modal = egui::Modal::new(egui::Id::new("ai-setup-sign-in")).show(ctx, |ui| {
        ui.set_max_width(500.0);
        let title = match steps.len() {
            0 => format!("{names} {}", tr("can't be downloaded by this version")),
            1 => format!("{names} {}", tr("needs a Hugging Face sign-in")),
            _ => format!("{names} {}", tr("need a Hugging Face sign-in")),
        };
        ui.label(RichText::new(title).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        for model in &gone {
            ui.add(egui::Label::new(RichText::new(detail_of(app, text(model, "/id"))).color(t.danger)).wrap().selectable(true));
            ui.add_space(6.0);
        }
        if steps.is_empty() {
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                answer = widgets::dialog_buttons(ui, &buttons);
            });
            return;
        }
        let licence = text(&first, "/licence");
        let why = format!(
            "{} ({licence}). {}",
            tr("Its makers share it with people who are signed in to Hugging Face and have accepted its licence"),
            tr("Both steps happen in your browser; PhotoCraft only checks that they are done.")
        );
        ui.add(egui::Label::new(RichText::new(why).color(t.text_dim)).wrap());
        ui.add_space(10.0);
        // The two steps, each with where it stands.
        let step = |ui: &mut egui::Ui, n: &str, done: bool, title: &str| {
            let (dot, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            if done {
                ui.painter().circle_filled(dot.center(), 10.0, t.accent);
                crate::icons::paint(ui, Rect::from_center_size(dot.center(), vec2(13.0, 13.0)), "check", 11.0, t.primary_text);
            } else {
                ui.painter().circle_stroke(dot.center(), 10.0, Stroke::new(1.0, t.field_border));
                ui.painter().text(dot.center(), egui::Align2::CENTER_CENTER, n, egui::FontId::proportional(11.5), t.text_dim);
            }
            ui.label(RichText::new(title).font(crate::theme::medium(13.0)).color(t.text));
        };
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            step(ui, "1", signed, tr("Sign in with Hugging Face"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if signing {
                    cancel = widgets::secondary_button(ui, tr("Cancel"), 70.0).clicked();
                    ui.label(RichText::new(tr("Waiting for your browser\u{2026}")).color(t.text_dim).size(12.0));
                    ui.add(egui::Spinner::new().size(14.0));
                } else if signed {
                    let who = if user.is_empty() { tr("Signed in").to_string() } else { format!("{} {user}", tr("Signed in as")) };
                    ui.label(RichText::new(who).color(t.text_dim).size(12.0));
                } else {
                    let r = ui.add_enabled_ui(can, |ui| widgets::primary_button(ui, tr("Sign In with Hugging Face"), 0.0)).inner;
                    go_sign_in = r.on_disabled_hover_text(tr("Not set up in this build: use an access token instead")).clicked();
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            step(ui, "2", ready, tr("Accept the model's licence"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ready {
                    ui.label(RichText::new(tr("Accepted")).color(t.accent_text).size(12.0));
                } else if checking {
                    ui.label(RichText::new(tr("Checking\u{2026}")).color(t.text_dim).size(12.0));
                    ui.add(egui::Spinner::new().size(14.0));
                } else {
                    recheck = signed && action(ui, tr("Check again"), t.text_dim).clicked();
                    let r = ui.add_enabled_ui(signed, |ui| {
                        if licence_left { widgets::primary_button(ui, tr("Accept the Licence"), 0.0) } else { widgets::secondary_button(ui, tr("Accept the Licence"), 0.0) }
                    });
                    go_licence = r.inner.on_disabled_hover_text(tr("Sign in first: the licence is accepted with your Hugging Face account")).clicked();
                }
            });
        });
        let error = [app.ui.ai_setup.note.as_str(), text(&hf, "/error")].into_iter().find(|e| !e.is_empty()).unwrap_or("").to_string();
        let forgotten = app.ui.ai_setup.access.as_ref().map(|a| text(a, "/note").to_string()).unwrap_or_default();
        let detail = steps.iter().map(|m| detail_of(app, text(m, "/id"))).find(|d| !d.is_empty()).unwrap_or_default();
        let said = [error, forgotten, detail].into_iter().find(|e| !e.is_empty()).unwrap_or_default();
        if !said.is_empty() {
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(said).color(t.danger).size(12.0)).wrap().selectable(true));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            link(app, ui, tr("How does this work?"), photocraft_aisetup::hf::HELP_URL);
            use_token = action(ui, tr("Use an access token instead"), t.text_dim).clicked();
        });
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    if go_sign_in {
        sign_in(app, ctx);
    }
    if cancel {
        cancel_sign_in(app);
    }
    if go_licence {
        let page = text(&first, "/page").to_string();
        crate::links::open(app, ctx, &page);
    }
    if recheck {
        check_access(app);
    }
    if use_token {
        app.ui.ai_setup.ask = None;
        app.ui.ai_setup.token_open = true;
        app.ui.ai_setup.page = Page::Models;
    }
    if let Some(answer) = answer {
        sign_in_answered(app, answer);
    }
}

/// The sign-in question answered: Install (everything ticked can be downloaded now), Install
/// Without (the gated models that can't are unticked), or Cancel.
fn sign_in_answered(app: &mut PhotocraftApp, answer: widgets::ButtonRole) {
    use widgets::ButtonRole;
    match answer {
        ButtonRole::Default if all_ready(app) => {
            app.ui.ai_setup.ask = None;
            install(app);
        }
        ButtonRole::Default => {}
        ButtonRole::Alternate => install_without(app),
        _ => app.ui.ai_setup.ask = None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod demo;
#[cfg(test)]
mod tests;
