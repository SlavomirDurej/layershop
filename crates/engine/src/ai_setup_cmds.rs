//! Local AI setup: the commands behind the "Set Up Local AI" wizard.
//!
//! The wizard installs a private ComfyUI and the recommended model files, so local models work
//! for someone who has never heard of ComfyUI (`photocraft_aisetup` does the work). Its screens
//! are thin: everything they show and do is one of these commands.
//!
//! - `ai.setup.probe` looks at the machine (system, graphics card, free space, an existing
//!   ComfyUI, the download hosts) and changes nothing.
//! - `ai.setup.manifest` lists what can be installed: models, their files, sizes and licences.
//! - `ai.setup.plan` says what a choice of models and folders would download and how much room
//!   it needs.
//! - `ai.setup.status` reports the setup that is running, or how the last one ended.
//! - `ai.setup.start`, `ai.setup.cancel`, `ai.setup.repair` and `ai.setup.remove` install, stop,
//!   fix and delete.
//! - Hugging Face, for the gated models: `ai.setup.hfStatus` (signed in, as whom),
//!   `ai.setup.hfAccess` (where each gated model stands: sign in, accept the licence, ready),
//!   `ai.setup.hfSignIn` (Sign in with Hugging Face, in the browser), `ai.setup.hfSignOut`,
//!   `ai.setup.setToken` (an access token, the fallback) and `ai.setup.hfUseFound` (the sign-in
//!   found on this PC, on the user's click).
//!
//! The reading ones (probe, manifest, plan, status, hfStatus, hfAccess) are open to agents. The
//! rest download and start a program, delete files, or sign in and out: they are the user's to
//! decide, and the automation guard refuses them (`photocraft_automation::workspace`), as it
//! refuses changing the launch settings. No command ever reports a token.
//!
//! A setup runs for as long as the downloads take, so it is not a background job of the session
//! (those are cancelled by Esc and repaint the window while they run): it has a worker of its
//! own, and [`poll`] applies its result to the preferences when it ends.
//!
//! What a setup needs from the app is [`AiSetup::env`], which only the desktop shell provides.
//! Without it (tests, the browser, a headless session) the commands that touch the disk or the
//! network say so and do nothing.

use photocraft_aisetup::Manifest;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const NEEDS_DESKTOP: &str = "Setting up local AI needs the desktop app";

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

/// The manifest this build was made with.
fn bundled() -> Result<std::sync::Arc<Manifest>> {
    static SHARED: std::sync::OnceLock<std::result::Result<std::sync::Arc<Manifest>, String>> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| Manifest::bundled().map(|m| std::sync::Arc::new(m.clone())).map_err(|e| e.to_string())).clone().map_err(EngineError::Other)
}

/// How a model suits a card with `vram_gb` of memory: `tooLarge` when it is known to need more,
/// `tested` when it has run on a card no larger, else `untested`. Nothing is claimed to fit that
/// has not been seen to.
fn fit(model: &photocraft_aisetup::manifest::ModelEntry, vram_gb: Option<u64>) -> &'static str {
    match vram_gb {
        Some(have) if model.min_vram_gb.is_some_and(|need| u64::from(need) > have) => "tooLarge",
        Some(have) if model.tested_vram_gb.iter().any(|tested| u64::from(*tested) <= have) => "tested",
        _ => "untested",
    }
}

/// What the wizard offers, with sizes: `{comfy, models, presets, suggested}`. `installed`
/// (manifest ids) marks the models that are there already. A small card is offered the light
/// preset, its models listed first.
fn manifest_value(m: &Manifest, installed: &[String], vram_gb: Option<u64>) -> Value {
    let models: Vec<Value> = m
        .offered(vram_gb)
        .into_iter()
        .map(|model| {
            let files: Vec<Value> =
                model.files.iter().filter_map(|key| m.files.get(key).map(|f| json!({"key": key, "bytes": f.size, "gated": f.gated}))).collect();
            json!({
                "id": model.id, "name": model.name, "about": model.about, "family": model.family.key(), "does": model.does,
                "licence": {"name": model.licence.name, "url": model.licence.url, "commercial": model.licence.commercial},
                "page": model.page, "gated": model.gated, "minVramGb": model.min_vram_gb, "testedVramGb": model.tested_vram_gb,
                "bytes": m.download_bytes(std::slice::from_ref(&model.id)), "files": files,
                "installed": installed.contains(&model.id), "fit": fit(model, vram_gb),
            })
        })
        .collect();
    let presets: serde_json::Map<String, Value> =
        m.presets.iter().map(|(name, ids)| (name.clone(), json!({"models": ids, "bytes": m.download_bytes(ids)}))).collect();
    let assets: serde_json::Map<String, Value> =
        m.comfy.assets.iter().map(|(name, a)| (name.clone(), json!({"bytes": a.size, "unpackedBytes": a.unpacked_bytes}))).collect();
    json!({
        "version": m.version, "checked": m.checked, "comfy": {"version": m.comfy.version, "builds": assets}, "models": models, "presets": presets,
        "suggested": m.suggested_preset(vram_gb),
    })
}

fn vram_param(p: &Value, cmd: &str) -> Result<Option<u64>> {
    match p.get("vramGb") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().filter(|gb| *gb <= 4096).map(Some).ok_or_else(|| bad(cmd, "`vramGb` must be a whole number of gigabytes")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{AiSetup, Finished, SetupEnv, poll};

#[cfg(target_arch = "wasm32")]
pub use web::AiSetup;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{EngineError, Manifest, NEEDS_DESKTOP, Result, Session, Value, bad, bundled, json, manifest_value, vram_param};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use photocraft_aigen::Launch;
    use photocraft_aisetup::fetch::Fetch;
    use photocraft_aisetup::hf::{self, Credentials};
    use photocraft_aisetup::install::{self, Handle, Job, Progress, TestImage};
    use photocraft_aisetup::plan::{self, Choices, Layout, PlannedModel, RealDisk, Route, Step};
    use photocraft_aisetup::state::{self, Phase, SetupState};
    use photocraft_aisetup::{configure, download, probe, runtime};

    /// How many of the engine's last lines `ai.setup.status` reports.
    const ENGINE_LINES: usize = 40;

    /// What a setup needs from the app. Installed by the desktop shell.
    #[derive(Clone)]
    pub struct SetupEnv {
        /// Downloads the runtime and the model files.
        pub fetch: Arc<dyn Fetch>,
        /// Where the setup's record is kept (beside the preferences, not in them).
        pub state_file: PathBuf,
        /// Where the engine and the models go unless the user chooses otherwise.
        pub engine_dir: String,
        pub models_dir: String,
        /// What can be installed, when it is not what this build was made with (`None`).
        pub manifest: Option<Arc<Manifest>>,
        /// The OAuth app Sign in with Hugging Face signs in as (the app passes
        /// [`hf::client_id`]). Empty: Sign in is unavailable, and an access token is the way in.
        pub hf_client_id: String,
    }

    /// How the last setup ended, for the wizard's last screen.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Finished {
        pub ok: bool,
        /// Stopped by the user: what arrived is kept.
        pub cancelled: bool,
        pub error: String,
        pub route: Route,
        pub url: String,
        /// The manifest models installed whole, and the chosen ones left out (with why).
        pub installed: Vec<String>,
        pub skipped: Vec<PlannedModel>,
        /// The local models the panel can run now.
        pub ready: Vec<String>,
        pub test: Option<TestImage>,
        pub test_error: Option<String>,
        /// The rows as they ended.
        pub progress: Progress,
    }

    /// The Local AI setup's place in the session.
    #[derive(Default)]
    pub struct AiSetup {
        /// What the app provides. `None`: the commands that install say they need the desktop app.
        pub env: Option<SetupEnv>,
        handle: Option<Handle>,
        last: Option<Finished>,
        /// Counts the setups that ended, so the shell can tell a new ending from one it has seen.
        seq: u64,
        /// Sign in with Hugging Face, while the browser is out.
        sign_in: Option<hf::SignIn>,
        /// Why the last sign-in did not work (empty: it did, or it was cancelled).
        sign_in_error: String,
        /// Counts the changes to the Hugging Face sign-in (signed in, out, a token saved).
        hf_seq: u64,
    }

    impl std::fmt::Debug for AiSetup {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AiSetup").field("env", &self.env.is_some()).field("running", &self.running()).field("seq", &self.seq).finish()
        }
    }

    impl AiSetup {
        /// Is a setup running now?
        pub fn running(&self) -> bool {
            self.handle.as_ref().is_some_and(|h| !h.finished())
        }

        /// How the last setup of this session ended.
        pub fn last(&self) -> Option<&Finished> {
            self.last.as_ref()
        }

        /// How many setups have ended in this session.
        pub fn seq(&self) -> u64 {
            self.seq
        }

        /// Is Sign in with Hugging Face waiting for the browser?
        pub fn signing_in(&self) -> bool {
            self.sign_in.is_some()
        }

        /// How many times the Hugging Face sign-in has changed in this session.
        pub fn hf_seq(&self) -> u64 {
            self.hf_seq
        }

        fn hf_changed(&mut self) {
            self.hf_seq = self.hf_seq.saturating_add(1);
        }

        /// The running setup's progress.
        pub fn progress(&self) -> Option<Progress> {
            self.handle.as_ref().map(Handle::progress)
        }

        /// The setup's record: what was chosen and how far it got. Empty without the app.
        pub fn record(&self) -> SetupState {
            self.env.as_ref().map(|env| state::load(&env.state_file)).unwrap_or_default()
        }

        /// Note in the record that the wizard was offered at first launch (it is offered once).
        pub fn mark_offered(&self) {
            let Some(env) = &self.env else { return };
            let mut record = state::load(&env.state_file);
            if !record.offered {
                record.offered = true;
                let _ = state::save(&env.state_file, &record);
            }
        }
    }

    /// What can be installed: the app's manifest, or the one this build was made with.
    fn manifest(s: &Session) -> Result<Arc<Manifest>> {
        match s.ai_setup.env.as_ref().and_then(|env| env.manifest.clone()) {
            Some(m) => Ok(m),
            None => bundled(),
        }
    }

    fn env(s: &Session) -> Result<SetupEnv> {
        s.ai_setup.env.clone().ok_or_else(|| EngineError::Other(NEEDS_DESKTOP.into()))
    }

    fn setup_error(e: photocraft_aisetup::Error) -> EngineError {
        match e {
            photocraft_aisetup::Error::Cancelled => EngineError::Cancelled,
            e => EngineError::Other(e.to_string()),
        }
    }

    /// A setup ended: apply what it leaves to the preferences as they are now, and keep how it
    /// ended for the wizard. Returns whether one ended. The shell calls this every frame while a
    /// setup runs, and saves the preferences when [`AiSetup::seq`] has moved.
    pub fn poll(s: &mut Session) -> bool {
        poll_sign_in(s);
        let Some(result) = s.ai_setup.handle.as_mut().and_then(Handle::take) else { return false };
        let progress = s.ai_setup.handle.take().map(|h| h.progress()).unwrap_or_default();
        let (route, url) = (s.ai_setup.record().choices.route, s.prefs().integrations.ai.comfy_url.clone());
        let finished = match (result, manifest(s)) {
            (Ok(done), Ok(manifest)) => {
                let configured = s.edit_prefs(|prefs| install::apply(&mut prefs.integrations.ai, &manifest, &done));
                Finished {
                    ok: true,
                    cancelled: false,
                    error: String::new(),
                    route: done.route,
                    url: done.url,
                    installed: done.installed,
                    skipped: done.skipped,
                    ready: configured.ready,
                    test: done.test,
                    test_error: done.test_error,
                    progress,
                }
            }
            (Ok(_), Err(e)) => failed(route, url, e.to_string(), false, progress),
            (Err(photocraft_aisetup::Error::Cancelled), _) => failed(route, url, String::new(), true, progress),
            (Err(e), _) => failed(route, url, e.to_string(), false, progress),
        };
        s.ai_setup.last = Some(finished);
        s.ai_setup.seq = s.ai_setup.seq.saturating_add(1);
        true
    }

    /// A sign-in that has ended: what it brought is stored with the keys, or why it failed kept.
    fn poll_sign_in(s: &mut Session) {
        let Some(result) = s.ai_setup.sign_in.as_ref().and_then(hf::SignIn::take) else { return };
        s.ai_setup.sign_in = None;
        s.ai_setup.sign_in_error = match result {
            Ok(credentials) => {
                s.ai.set_hf(&credentials);
                String::new()
            }
            Err(photocraft_aisetup::Error::Cancelled) => String::new(),
            Err(e) => e.to_string(),
        };
        s.ai_setup.hf_changed();
    }

    fn failed(route: Route, url: String, error: String, cancelled: bool, progress: Progress) -> Finished {
        Finished {
            ok: false,
            cancelled,
            error,
            route,
            url,
            installed: Vec::new(),
            skipped: Vec::new(),
            ready: Vec::new(),
            test: None,
            test_error: None,
            progress,
        }
    }

    fn text_param(p: &Value, key: &str, cmd: &str) -> Result<Option<String>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_str()
                .filter(|t| t.len() <= 2048 && !t.chars().any(char::is_control))
                .map(|t| Some(t.to_string()))
                .ok_or_else(|| bad(cmd, format!("`{key}` must be text"))),
        }
    }

    fn flag_param(p: &Value, key: &str, default: bool, cmd: &str) -> Result<bool> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v.as_bool().ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
        }
    }

    /// The model files a server lists, as given: `{folder: [name, …]}`.
    fn server_files_param(p: &Value, cmd: &str) -> Result<Option<BTreeMap<String, Vec<String>>>> {
        let Some(v) = p.get("serverFiles").filter(|v| !v.is_null()) else { return Ok(None) };
        let malformed = || bad(cmd, "`serverFiles` must be {folder: [file name, …]}");
        let map = v.as_object().ok_or_else(malformed)?;
        if map.len() > 64 {
            return Err(malformed());
        }
        let mut out = BTreeMap::new();
        for (folder, names) in map {
            let names = names.as_array().ok_or_else(malformed)?;
            let names: Vec<String> = names.iter().take(20_000).filter_map(Value::as_str).filter(|n| n.len() <= 512).map(str::to_string).collect();
            out.insert(folder.chars().take(64).collect(), names);
        }
        Ok(Some(out))
    }

    /// The choices a `plan` or `start` asks for: what the params say, else what the record
    /// holds from last time, else the app's defaults.
    fn choices(s: &Session, p: &Value, cmd: &str) -> Result<Choices> {
        let env = env(s)?;
        let record = s.ai_setup.record();
        let m = manifest(s)?;
        let route = match p.get("route").map(|r| r.as_str().unwrap_or("?")) {
            None => record.choices.route,
            Some("managed") => Route::Managed,
            Some("existing") => Route::Existing,
            Some(_) => return Err(bad(cmd, "`route` must be managed|existing")),
        };
        let models = match p.get("models") {
            None | Some(Value::Null) => record.choices.models.clone(),
            Some(v) => {
                let list =
                    v.as_array().filter(|l| l.len() <= 256).ok_or_else(|| bad(cmd, "`models` must be a list of model ids (ai.setup.manifest lists them)"))?;
                list.iter()
                    .map(|id| id.as_str().map(str::to_string).ok_or_else(|| bad(cmd, "`models` must be a list of model ids")))
                    .collect::<Result<Vec<_>>>()?
            }
        };
        if let Some(unknown) = models.iter().find(|id| m.model(id).is_none()) {
            return Err(bad(cmd, format!("`{}` is not a model the setup offers (ai.setup.manifest lists them)", unknown.chars().take(80).collect::<String>())));
        }
        let or_default = |given: Option<String>, saved: &str, default: &str| {
            given.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| if saved.trim().is_empty() { default.to_string() } else { saved.to_string() })
        };
        let variant = text_param(p, "variant", cmd)?
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| if record.choices.variant.is_empty() { "nvidia".into() } else { record.choices.variant.clone() });
        if route == Route::Managed && !m.comfy.assets.contains_key(&variant) {
            let known: Vec<&str> = m.comfy.assets.keys().map(String::as_str).collect();
            return Err(bad(cmd, format!("`variant` must be {}", known.join("|"))));
        }
        Ok(Choices {
            route,
            models,
            engine_dir: or_default(text_param(p, "engineDir", cmd)?, &record.choices.engine_dir, &env.engine_dir),
            models_dir: or_default(text_param(p, "modelsDir", cmd)?, &record.choices.models_dir, &env.models_dir),
            variant,
            // Not given (Continue Setup): as the last setup ran, like the variant.
            cpu: route == Route::Managed && flag_param(p, "cpu", record.choices.cpu, cmd)?,
            token: !s.ai.hf_token().is_empty(),
            recheck: flag_param(p, "recheck", false, cmd)?,
            server_files: server_files_param(p, cmd)?,
            test_image: flag_param(p, "testImage", true, cmd)?,
        })
    }

    /// The models of the manifest that are installed whole in the record's models folder.
    fn installed_now(s: &Session) -> Vec<String> {
        let record = s.ai_setup.record();
        let (Ok(m), false) = (manifest(s), record.choices.models_dir.trim().is_empty()) else { return Vec::new() };
        configure::installed(&m, &runtime::list_models(Path::new(&record.choices.models_dir)))
    }

    pub(super) fn manifest_cmd(s: &mut Session, p: &Value) -> Result<Value> {
        let vram = vram_param(p, "ai.setup.manifest")?;
        Ok(manifest_value(&*manifest(s)?, &installed_now(s), vram))
    }

    pub(super) fn probe(s: &mut Session, _: &Value) -> Result<Value> {
        let env = env(s)?;
        let transport = s.ai.transport.clone().ok_or_else(|| EngineError::Other(NEEDS_DESKTOP.into()))?;
        let m = manifest(s)?;
        let mut urls = vec![s.prefs().integrations.ai.comfy_url.trim().trim_end_matches('/').to_string()];
        for usual in ["http://127.0.0.1:8188", "http://127.0.0.1:8000"] {
            if !urls.iter().any(|u| u == usual) {
                urls.push(usual.to_string());
            }
        }
        // The engine this app installed is not "an existing ComfyUI" to adopt.
        let record = s.ai_setup.record();
        let own = (record.choices.route == Route::Managed && record.port != 0).then(|| runtime::url(record.port));
        urls.retain(|u| !u.is_empty() && Some(u) != own.as_ref());
        let defaults = json!({"engineDir": env.engine_dir, "modelsDir": env.models_dir});
        crate::jobs::run(
            s,
            "Check This PC",
            false,
            move |ctx| {
                // Each step as it begins: the wizard lists them while it waits.
                let stage = |step: probe::Stage| ctx.progress(step.fraction(), step.message());
                let found = probe::collect(&m, transport.as_ref(), env.fetch.as_ref(), &urls, &|| ctx.cancelled(), &stage);
                ctx.check()?;
                Ok(found)
            },
            move |_, found: probe::Probe| {
                let (variant, tried) = found.variant();
                let vram_gb = found.gpu().and_then(|g| g.vram_mb).map(|mb| (mb as f64 / 1024.0).round() as u64);
                Ok(json!({
                    "probe": found, "checks": found.checks(), "variant": variant, "variantTested": tried, "canInstall": found.can_install(),
                    "gpuUsable": found.usable_gpu().is_some(),
                    "vramGb": vram_gb, "defaults": defaults,
                }))
            },
        )
    }

    /// The free bytes on the drive of `dir`, when it can be told.
    fn free(dir: &str) -> Option<u64> {
        Some(dir.trim()).filter(|d| !d.is_empty()).and_then(|d| download::free_space(Path::new(d)))
    }

    /// Room shortfalls, split by what fixes them.
    #[derive(Debug, Default, PartialEq)]
    pub(super) struct RoomProblems {
        /// Only another folder fixes these: the engine alone does not fit.
        pub folders: Vec<String>,
        /// Fewer models fix these.
        pub models: Vec<String>,
    }

    /// What does not fit, given `(free, needed)` bytes for the engine and the models. Free space
    /// that can't be told is not a problem; [`plan::HEADROOM`] is kept to spare.
    pub(super) fn room_problems(same_drive: bool, engine: (Option<u64>, u64), models: (Option<u64>, u64)) -> RoomProblems {
        let short = |have: Option<u64>, need: u64| have.is_some_and(|have| need > 0 && have < need.saturating_add(plan::HEADROOM));
        let needs = |bytes: u64| photocraft_aisetup::human_bytes(bytes.saturating_add(plan::HEADROOM));
        let human = |have: Option<u64>| photocraft_aisetup::human_bytes(have.unwrap_or(0));
        let mut out = RoomProblems::default();
        let engine_free = if same_drive { models.0 } else { engine.0 };
        if short(engine_free, engine.1) {
            out.folders.push(format!(
                "Not enough room for the engine: {} is free, and it needs {}. Choose another engine folder.",
                human(engine_free),
                needs(engine.1)
            ));
        } else if same_drive && short(models.0, engine.1.saturating_add(models.1)) {
            out.models.push(format!(
                "Not enough room: {} is free, and this needs {}. Choose fewer models or another folder.",
                human(models.0),
                needs(engine.1.saturating_add(models.1))
            ));
        }
        if !same_drive && short(models.0, models.1) {
            out.models.push(format!(
                "Not enough room for the models: {} is free, and they need {}. Choose fewer models or another models folder.",
                human(models.0),
                needs(models.1)
            ));
        }
        out
    }

    pub(super) fn plan_cmd(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.plan";
        let choices = choices(s, p, cmd)?;
        let m = manifest(s)?;
        let planned = plan::plan(&m, &choices, &RealDisk).map_err(|e| bad(cmd, e.to_string()))?;
        let layout = Layout::new(&m, &choices).map_err(|e| bad(cmd, e.to_string()))?;
        // What can be known to stand in the way, before Install is pressed. What is about the
        // folders blocks the Location page; what fewer models fix waits for the Models page, where
        // they are chosen.
        let mut folder_problems: Vec<String> = Vec::new();
        let managed = choices.route == Route::Managed;
        if managed && let Some(why) = m.asset(&choices.variant).ok().and_then(|asset| plan::path_too_long(asset, &layout)) {
            folder_problems.push(why);
        }
        let (engine_free, models_free) = (free(&choices.engine_dir).filter(|_| managed), free(&choices.models_dir));
        let same_drive = managed && Path::new(&choices.engine_dir).components().next() == Path::new(&choices.models_dir).components().next();
        let room = room_problems(same_drive, (engine_free, planned.engine_bytes), (models_free, planned.models_bytes));
        folder_problems.extend(room.folders);
        let problems: Vec<&String> = folder_problems.iter().chain(&room.models).collect();
        // The ticked gated models whose files are still to come: what needs a Hugging Face sign-in.
        let needs_sign_in: Vec<Value> = hf::gated(&m, &choices.models)
            .into_iter()
            .filter(|model| {
                let gated_file = |k: &&String| m.files.get(*k).is_some_and(|f| f.gated);
                model.files.iter().filter(gated_file).any(|k| planned.steps.iter().any(|st| st.step == Step::FetchFile(k.clone()) && !st.done))
            })
            .map(|model| json!({"id": model.id, "name": model.name, "page": model.page, "licence": model.licence.name}))
            .collect();
        Ok(json!({
            "needsSignIn": needs_sign_in,
            "route": choices.route, "models": planned.models, "steps": planned.steps,
            "downloadBytes": planned.download_bytes, "engineBytes": planned.engine_bytes, "modelsBytes": planned.models_bytes,
            "engineDir": choices.engine_dir, "modelsDir": choices.models_dir, "variant": choices.variant, "cpu": choices.cpu,
            "engineFree": engine_free, "modelsFree": models_free, "sameDrive": same_drive,
            "token": choices.token, "problems": problems, "folderProblems": folder_problems, "modelProblems": room.models,
        }))
    }

    fn finished_value(f: &Finished) -> Value {
        json!({
            "ok": f.ok, "cancelled": f.cancelled, "error": f.error, "route": f.route, "url": f.url,
            "installed": f.installed, "skipped": f.skipped, "ready": f.ready, "testError": f.test_error,
            "testImage": f.test.as_ref().map(|t| json!({"model": t.model, "modelName": t.model_name, "seconds": t.seconds, "width": t.width, "height": t.height})),
        })
    }

    pub(super) fn status(s: &mut Session, _: &Value) -> Result<Value> {
        poll(s);
        let running = s.ai_setup.running();
        if !running {
            // It may have ended since the line above: "not running" always comes with its result.
            poll(s);
        }
        let record = s.ai_setup.record();
        let state = match (running, record.phase) {
            (true, _) => "running",
            // The app was closed while it ran.
            (false, Phase::Installing) => "interrupted",
            (false, Phase::Paused) => "paused",
            (false, Phase::Failed) => "failed",
            (false, Phase::Done) => "done",
            (false, Phase::None) => "none",
        };
        let progress = s.ai_setup.progress().or_else(|| s.ai_setup.last.as_ref().map(|f| f.progress.clone()));
        // What is on disk, whatever the record says.
        let engines: Vec<Value> = match record.choices.route {
            Route::Managed if !record.choices.engine_dir.trim().is_empty() => runtime::installed_engines(Path::new(&record.choices.engine_dir))
                .into_iter()
                .map(|(path, marker)| json!({"version": marker.version, "variant": marker.variant, "folder": path.to_string_lossy()}))
                .collect(),
            _ => Vec::new(),
        };
        // The model files of the engine this app installed, by folder: what it will have when it
        // is started, for the model lists to tell what is installed while it isn't running.
        let model_files = match record.choices.route {
            Route::Managed if !record.choices.models_dir.trim().is_empty() => Some(runtime::list_models(Path::new(&record.choices.models_dir))),
            _ => None,
        };
        // What the engine the app started last printed, while a setup runs or after it failed:
        // what the wizard shows of a step that takes long (starting it, rendering on the processor).
        let engine =
            s.ai.server
                .as_ref()
                .filter(|_| matches!(state, "running" | "failed"))
                .map(|server| json!({"output": server.output(ENGINE_LINES), "log": server.log_file()}));
        Ok(json!({
            "available": s.ai_setup.env.is_some(), "state": state, "running": running, "seq": s.ai_setup.seq, "modelFiles": model_files,
            "engine": engine,
            "record": {
                "phase": record.phase, "route": record.choices.route, "models": record.choices.models, "engineDir": record.choices.engine_dir,
                "modelsDir": record.choices.models_dir, "variant": record.choices.variant, "cpu": record.choices.cpu, "port": record.port,
                "comfyVersion": record.comfy_version, "installed": record.installed, "error": record.error, "offered": record.offered,
            },
            "engines": engines, "installedNow": installed_now(s), "progress": progress,
            "result": s.ai_setup.last.as_ref().map(finished_value), "token": !s.ai.hf_token().is_empty(),
        }))
    }

    /// Start the setup `choices` describe.
    fn begin(s: &mut Session, choices: Choices, url: Option<String>, launch: Option<Launch>, cmd: &str) -> Result<Value> {
        poll(s);
        if s.ai_setup.running() {
            return Err(EngineError::Other("A setup is already running. Wait for it, or stop it first.".into()));
        }
        let env = env(s)?;
        let transport = s.ai.transport.clone().ok_or_else(|| EngineError::Other(NEEDS_DESKTOP.into()))?;
        let m = manifest(s)?;
        let settings = s.prefs().integrations.ai.clone();
        let record = s.ai_setup.record();
        // An engine this app installed keeps its port; a new one gets the first free one.
        let port = if choices.route == Route::Managed && record.choices.route == Route::Managed && record.phase == Phase::Done { record.port } else { 0 };
        let url = url.unwrap_or_else(|| settings.comfy_url.clone());
        if choices.route == Route::Existing {
            photocraft_aigen::comfy::base_url(&url).map_err(|e| bad(cmd, e.to_string()))?;
        }
        // A Hugging Face sign-in about to expire is renewed first (rarely needed: the wizard checks
        // access just before Install, and that renews it).
        let mut signed = s.ai.hf();
        let now = hf::unix_now();
        if signed.needs_refresh(now)
            && let Ok(fresh) = hf::refresh(transport.as_ref(), &env.hf_client_id, &signed, now)
        {
            s.ai.set_hf(&fresh);
            signed = fresh;
        }
        let token = Some(signed.access).filter(|t| !t.is_empty());
        let job = Job { manifest: (*m).clone(), choices, settings, token, port, url, launch };
        let setup = Arc::new(install::Env {
            fetch: env.fetch.clone(),
            transport,
            server: s.ai.server.clone(),
            state_file: Some(env.state_file.clone()),
            sleep: std::thread::sleep,
            free_space: download::free_space,
            // Windows: the C++ runtime PyTorch needs, from the app's own copy or System32.
            vc_runtime: cfg!(windows).then(photocraft_aisetup::vcrt::Sources::this_pc),
        });
        s.ai_setup.handle = Some(Handle::start(setup, job).map_err(|e| bad(cmd, e.to_string()))?);
        status(s, &Value::Null)
    }

    pub(super) fn start(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.start";
        env(s)?;
        // Installing is never a default: without `models` there must be a setup to continue.
        if p.get("models").is_none_or(Value::is_null) && !s.ai_setup.record().interrupted() {
            return Err(bad(
                cmd,
                "missing \"models\": the ids of the models to install (ai.setup.manifest lists them; an empty list installs the engine alone)",
            ));
        }
        let choices = choices(s, p, cmd)?;
        let url = text_param(p, "url", cmd)?;
        let launch = match p.get("launch").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(serde_json::from_value::<Launch>(v.clone()).map_err(|_| bad(cmd, "`launch` must be {python, folder, args}"))?),
        };
        begin(s, choices, url, launch, cmd)
    }

    /// `ai.setup.repair`: the last setup again, with every file's checksum looked at.
    pub(super) fn repair(s: &mut Session, _: &Value) -> Result<Value> {
        let cmd = "ai.setup.repair";
        env(s)?;
        let record = s.ai_setup.record();
        if record.phase == Phase::None || record.choices.models_dir.trim().is_empty() {
            return Err(EngineError::Other("There is no setup to repair yet: set up local AI first.".into()));
        }
        let choices = Choices { recheck: true, test_image: true, token: !s.ai.hf_token().is_empty(), ..record.choices };
        begin(s, choices, None, None, cmd)
    }

    pub(super) fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
        let running = s.ai_setup.running();
        if let Some(handle) = &s.ai_setup.handle {
            handle.cancel();
        }
        Ok(json!({"stopping": running}))
    }

    /// `ai.setup.remove`: delete what the setup installed. `{"engine": true}` removes the
    /// engine (and stops it), `{"models": true}` every model file the setup lists,
    /// `{"models": [id, …]}` those models' files (a file another installed model uses stays).
    /// Only the folders of the setup's own record are touched, and in them only what the setup
    /// puts there.
    pub(super) fn remove(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.remove";
        env(s)?;
        poll(s);
        if s.ai_setup.running() {
            return Err(EngineError::Other("A setup is running. Stop it before removing anything.".into()));
        }
        let m = manifest(s)?;
        let engine = flag_param(p, "engine", false, cmd)?;
        let all_ids: Vec<String> = m.models.iter().map(|x| x.id.clone()).collect();
        let models: Vec<String> = match p.get("models") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => Vec::new(),
            Some(Value::Bool(true)) => all_ids.clone(),
            Some(Value::Array(list)) if list.len() <= 256 => {
                let ids: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if ids.len() != list.len() || ids.iter().any(|id| m.model(id).is_none()) {
                    return Err(bad(cmd, "`models` must be true, or a list of model ids (ai.setup.manifest lists them)"));
                }
                ids
            }
            Some(_) => return Err(bad(cmd, "`models` must be true, or a list of model ids")),
        };
        if !engine && models.is_empty() {
            return Err(bad(cmd, "say what to remove: `engine` (true) and/or `models` (true, or a list of ids)"));
        }
        let mut record = s.ai_setup.record();
        if record.choices.route != Route::Managed || record.choices.engine_dir.trim().is_empty() {
            return Err(EngineError::Other("There is nothing this app installed to remove. A ComfyUI you set up yourself is yours to manage.".into()));
        }
        let mut freed = 0u64;
        if !models.is_empty() {
            let keep = installed_now(s);
            freed = freed.saturating_add(runtime::remove_models(&m, Path::new(&record.choices.models_dir), &models, &keep).map_err(setup_error)?);
        }
        if engine {
            // Its files are in use while it runs.
            if let Some(server) = &s.ai.server {
                server.stop();
            }
            freed = freed.saturating_add(runtime::remove_engines(Path::new(&record.choices.engine_dir)).map_err(setup_error)?);
        }
        // The settings follow the disk: what can no longer run is unticked, and an engine that
        // is gone is no longer started.
        let folders = runtime::list_models(Path::new(&record.choices.models_dir));
        let still = configure::installed(&m, &folders);
        s.edit_prefs(|prefs| {
            let ai = &mut prefs.integrations.ai;
            for model in ai.models.iter_mut().filter(|x| x.provider.is_local() && m.model(&x.id).is_some() && !still.contains(&x.id)) {
                model.enabled = false;
            }
            if engine {
                ai.comfy_autostart = false;
                ai.comfy_launch = Launch::default();
            }
        });
        record.installed = still.clone();
        record.choices.models.retain(|id| still.contains(id));
        if engine {
            record.phase = Phase::None;
            record.port = 0;
        }
        if let Some(env) = &s.ai_setup.env {
            let _ = state::save(&env.state_file, &record);
        }
        s.ai_setup.seq = s.ai_setup.seq.saturating_add(1);
        Ok(json!({"freed": freed, "freedText": photocraft_aisetup::human_bytes(freed), "installed": still, "engine": engine}))
    }

    pub(super) fn set_token(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.setToken";
        let token = p.get("token").and_then(Value::as_str).ok_or_else(|| bad(cmd, "missing \"token\" (an empty string removes it)"))?;
        if token.len() > 1024 || token.chars().any(|c| c.is_control() || c.is_whitespace() && c != ' ') {
            return Err(bad(cmd, "that is not a Hugging Face token"));
        }
        s.ai.set_hf_token(token);
        s.ai_setup.hf_changed();
        Ok(json!({"set": !token.trim().is_empty()}))
    }

    /// `ai.setup.hfStatus`: the Hugging Face sign-in, without its tokens.
    pub(super) fn hf_status(s: &mut Session, _: &Value) -> Result<Value> {
        poll(s);
        let c = s.ai.hf();
        let client = s.ai_setup.env.as_ref().map(|env| env.hf_client_id.clone()).unwrap_or_default();
        // A token on this PC is offered only when it is not the one in use already.
        let found = s.ai_setup.env.is_some() && hf::found_on_this_pc().is_some_and(|t| t != c.access);
        // Run out and not renewable: not a sign-in any more (the next access check forgets it).
        let expired = c.expired(hf::unix_now());
        Ok(json!({
            "available": s.ai_setup.env.is_some(),
            "signedIn": c.signed_in() && !expired, "expired": expired, "via": c.via, "user": c.user, "expiresAt": (c.expires_at != 0).then_some(c.expires_at),
            "canSignIn": !client.trim().is_empty() && s.ai.transport.is_some(),
            "signingIn": s.ai_setup.signing_in(), "error": s.ai_setup.sign_in_error, "found": found, "seq": s.ai_setup.hf_seq,
            "links": {"help": hf::HELP_URL, "connectedApps": hf::CONNECTED_APPS_URL, "tokens": hf::TOKENS_URL},
        }))
    }

    /// `ai.setup.hfAccess`: where each gated model among `models` stands. Renews a sign-in about to
    /// expire, and learns whose a pasted token is.
    pub(super) fn hf_access(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.hfAccess";
        let env = env(s)?;
        let transport = s.ai.transport.clone().ok_or_else(|| EngineError::Other(NEEDS_DESKTOP.into()))?;
        let m = manifest(s)?;
        let models: Vec<String> = match p.get("models") {
            None | Some(Value::Null) => hf::gated(&m, &m.models.iter().map(|x| x.id.clone()).collect::<Vec<_>>()).iter().map(|x| x.id.clone()).collect(),
            Some(Value::Array(list)) if list.len() <= 256 => {
                let ids: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if ids.len() != list.len() || ids.iter().any(|id| m.model(id).is_none()) {
                    return Err(bad(cmd, "`models` must be a list of model ids (ai.setup.manifest lists them)"));
                }
                ids
            }
            Some(_) => return Err(bad(cmd, "`models` must be a list of model ids")),
        };
        let before = s.ai.hf();
        let client = env.hf_client_id.clone();
        let start = before.clone();
        crate::jobs::run(
            s,
            "Check Hugging Face Access",
            false,
            move |ctx| {
                let mut c = start;
                let mut note = String::new();
                let now = hf::unix_now();
                if c.needs_refresh(now) {
                    match hf::refresh(transport.as_ref(), &client, &c, now) {
                        Ok(fresh) => c = fresh,
                        // Not renewable any more: signed out.
                        Err(photocraft_aisetup::Error::Access(why)) => {
                            c = Credentials::default();
                            note = why;
                        }
                        Err(e) => return Err(setup_error(e)),
                    }
                }
                if c.expired(now) {
                    c = Credentials::default();
                    note = "Your Hugging Face sign-in has expired. Sign in again.".into();
                }
                ctx.check()?;
                if c.signed_in() && c.user.is_empty() {
                    let who = if c.via == hf::Via::OAuth { hf::userinfo(transport.as_ref(), &c.access) } else { hf::whoami(transport.as_ref(), &c.access) };
                    match who {
                        Ok(user) => c.user = user,
                        Err(photocraft_aisetup::Error::Access(why)) => note = why,
                        Err(_) => {}
                    }
                }
                ctx.check()?;
                let token = c.signed_in().then(|| c.access.clone());
                let found = hf::access(&m, &models, token.as_deref(), env.fetch.as_ref()).map_err(setup_error)?;
                // With a token, "sign in" means Hugging Face refused it (401): it is no sign-in
                // any more, and the wizard says so instead of "Signed in as".
                if token.is_some() && found.iter().any(|f| f.access == hf::Access::SignIn) {
                    c = Credentials::default();
                    note = "Hugging Face no longer accepts your sign-in (it expired, or the app was disconnected in Connected apps). Sign in again.".into();
                }
                Ok((c, found, note))
            },
            move |s, (c, found, note)| {
                // Kept only when nothing changed the sign-in while this ran (a sign-in that ended).
                if s.ai.hf() == before && c != before {
                    s.ai.set_hf(&c);
                    s.ai_setup.hf_changed();
                }
                Ok(json!({"models": found, "signedIn": c.signed_in(), "user": c.user, "via": c.via, "note": note}))
            },
        )
    }

    /// `ai.setup.hfSignIn`: start Sign in with Hugging Face (with `cancel`, stop it). The app
    /// opens `url` in the browser.
    pub(super) fn hf_sign_in(s: &mut Session, p: &Value) -> Result<Value> {
        let cmd = "ai.setup.hfSignIn";
        let env = env(s)?;
        poll(s);
        if flag_param(p, "cancel", false, cmd)? {
            // Dropping it stops the wait and closes the listener.
            let was = s.ai_setup.sign_in.take().is_some();
            if was {
                s.ai_setup.sign_in_error.clear();
                s.ai_setup.hf_changed();
            }
            return Ok(json!({"cancelled": was}));
        }
        if let Some(sign_in) = &s.ai_setup.sign_in {
            return Ok(json!({"url": sign_in.url(), "started": false}));
        }
        let transport = s.ai.transport.clone().ok_or_else(|| EngineError::Other(NEEDS_DESKTOP.into()))?;
        let product: String = text_param(p, "product", cmd)?.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "the app".into()).chars().take(60).collect();
        let sign_in = hf::SignIn::start(transport, &env.hf_client_id, &product, std::time::Duration::from_secs(hf::SIGN_IN_SECONDS)).map_err(setup_error)?;
        let url = sign_in.url().to_string();
        s.ai_setup.sign_in = Some(sign_in);
        s.ai_setup.sign_in_error.clear();
        s.ai_setup.hf_changed();
        Ok(json!({"url": url, "started": true}))
    }

    /// `ai.setup.hfSignOut`: forget the Hugging Face sign-in here. Revoking the app is done on
    /// Hugging Face (`connectedApps`).
    pub(super) fn hf_sign_out(s: &mut Session, _: &Value) -> Result<Value> {
        s.ai_setup.sign_in = None;
        s.ai_setup.sign_in_error.clear();
        s.ai.set_hf(&Credentials::default());
        s.ai_setup.hf_changed();
        Ok(json!({"signedOut": true, "connectedApps": hf::CONNECTED_APPS_URL}))
    }

    /// `ai.setup.hfUseFound`: use the Hugging Face token found on this PC, as the user chose to.
    pub(super) fn hf_use_found(s: &mut Session, _: &Value) -> Result<Value> {
        env(s)?;
        let token = hf::found_on_this_pc().ok_or_else(|| EngineError::Other("No Hugging Face sign-in was found on this PC.".into()))?;
        s.ai.set_hf_token(&token);
        s.ai_setup.hf_changed();
        Ok(json!({"used": true}))
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;

    /// The browser has no disk to install to: the setup's place in the session is empty.
    #[derive(Debug, Default)]
    pub struct AiSetup;

    impl AiSetup {
        pub fn running(&self) -> bool {
            false
        }

        pub fn seq(&self) -> u64 {
            0
        }

        pub fn signing_in(&self) -> bool {
            false
        }

        pub fn hf_seq(&self) -> u64 {
            0
        }
    }

    fn unsupported() -> EngineError {
        EngineError::Other(NEEDS_DESKTOP.into())
    }

    pub(super) fn manifest_cmd(_: &mut Session, p: &Value) -> Result<Value> {
        let vram = vram_param(p, "ai.setup.manifest")?;
        Ok(manifest_value(&*bundled()?, &[], vram))
    }

    pub(super) fn status(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(json!({"available": false, "state": "none", "running": false, "seq": 0}))
    }

    pub(super) fn probe(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn plan_cmd(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn start(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn repair(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn cancel(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(json!({"stopping": false}))
    }

    pub(super) fn remove(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn set_token(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn hf_status(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(json!({"available": false, "signedIn": false, "canSignIn": false, "signingIn": false, "found": false, "seq": 0}))
    }

    pub(super) fn hf_access(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn hf_sign_in(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn hf_sign_out(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }

    pub(super) fn hf_use_found(_: &mut Session, _: &Value) -> Result<Value> {
        Err(unsupported())
    }
}

#[cfg(not(target_arch = "wasm32"))]
use native as imp;
#[cfg(target_arch = "wasm32")]
use web as imp;

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

/// The commands that install need what only the desktop app provides.
fn can_install(s: &Session) -> std::result::Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    if s.ai_setup.env.is_some() && s.ai.transport.is_some() {
        return Ok(());
    }
    let _ = s;
    Err(NEEDS_DESKTOP.into())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.setup.probe",
            label: "Check This PC for Local AI",
            menu: &[],
            shortcut: None,
            params: r##"{} → {probe:{os:{name,version,sixtyFourBit,windows},gpus:[{name,vendor,vramMb,driver}],drives:[{name,free,total}],comfy:{running:[url],installed:{python,folder,args}?,models:[folder] (where it reads model files, where it saves new ones first)},hosts:[{name,ok,note}]},checks:[{id:os|gpu|disk|comfy|net,level:ok|info|warn|stop,title,detail,value}],variant (the ComfyUI build for the graphics card),variantTested,canInstall,gpuUsable (a graphics card the engine can use; without one a new ComfyUI installs only with cpu:true),vramGb,defaults:{engineDir,modelsDir}}: what this machine has for running local models. Changes nothing"##,
            enabled: can_install,
            run: imp::probe,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.manifest",
            label: "Local AI Models to Install",
            menu: &[],
            shortcut: None,
            params: r##"{"vramGb":int?} → {comfy:{version,builds},models:[{id,name,about,family,does:[create|edit|fill],licence:{name,url,commercial (true|false|null: read the licence)},page,gated (needs a Hugging Face sign-in),minVramGb,testedVramGb,bytes (downloaded alone),files:[{key,bytes,gated}],installed,fit:tested|untested|tooLarge}],presets:{light|starter|recommended|everything:{models,bytes (shared files counted once)}},suggested:light|recommended (the preset to start on: light on a card under 16 GB, its models then listed first)}: what the setup can install"##,
            enabled: always,
            run: imp::manifest_cmd,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.plan",
            label: "Plan Local AI Setup",
            menu: &[],
            shortcut: None,
            params: r##"{"models":[id,…]?=the last setup's,"route":"managed|existing"?,"engineDir":str?,"modelsDir":str?,"variant":"nvidia|nvidia_cu126|amd|intel"?,"cpu":bool=false (managed: run the engine on the processor, for a PC with no graphics card it can use),"recheck":bool=false,"serverFiles":{folder:[file,…]}? (existing: what the server has)} → {steps:[{step,label,bytes,have,done,recheck,skipped}],models:[{id,name,skipped}],downloadBytes,engineBytes,modelsBytes,engineFree,modelsFree,problems:[str],folderProblems:[str] (only other folders fix these),modelProblems:[str] (fewer models fix these)}: what a setup would download and the room it needs, given what is already on disk. Changes nothing"##,
            enabled: can_install,
            run: imp::plan_cmd,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.status",
            label: "Local AI Setup Status",
            menu: &[],
            shortcut: None,
            params: r##"{} → {available,state:none|running|paused|failed|interrupted|done,running,record:{phase,route,models,engineDir,modelsDir,variant,port,comfyVersion,installed,error},engines:[{version,variant,folder}],installedNow:[id],modelFiles:{folder:[file]}? (the installed engine's model files; null for the user's own ComfyUI),progress:{steps:[{key,label,state:pending|active|done|failed|skipped,done,total,note,download}],bytesDone,bytesTotal,speed (bytes/s),etaSeconds,message}?,result:{ok,cancelled,error,installed,skipped,ready,testImage:{model,modelName,seconds,width,height}?,testError}?,engine:{output:[line] (the last 40 lines the engine the app started printed),log (where all of it is)}? (while a setup runs or after it failed),token (a Hugging Face token is stored)}"##,
            enabled: always,
            run: imp::status,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.start",
            label: "Set Up Local AI",
            menu: &[],
            shortcut: None,
            params: r##"{"route":"managed|existing"="managed" (managed: install a private ComfyUI; existing: use the one at `url`, downloading only missing model files),"models":[id,…],"engineDir":str?,"modelsDir":str?,"variant":str?,"cpu":bool=false (managed: run the engine on the processor; no test image then),"testImage":bool=true,"recheck":bool=false,"url":str? (existing),"launch":{python,folder,args}? (existing),"serverFiles":{folder:[file,…]}? (existing)} → ai.setup.status. Downloads, unpacks and starts a program: the user's to run from the wizard; automation is refused. Continues an interrupted setup from what is on disk"##,
            enabled: can_install,
            run: imp::start,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.cancel",
            label: "Stop Local AI Setup",
            menu: &[],
            shortcut: None,
            params: r##"{} → {stopping}: stops the running setup; what arrived is kept and ai.setup.start continues from it"##,
            enabled: always,
            run: imp::cancel,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.repair",
            label: "Repair Local AI",
            menu: &[],
            shortcut: None,
            params: r##"{} → ai.setup.status: runs the last setup again, comparing every file with its checksum and downloading again what is damaged or missing"##,
            enabled: can_install,
            run: imp::repair,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.remove",
            label: "Remove Local AI",
            menu: &[],
            shortcut: None,
            params: r##"{"engine":bool=false,"models":true|[id,…]?} → {freed,freedText,installed,engine}: deletes the engine this app installed and/or model files it lists (a file another installed model uses stays). A ComfyUI the user set up themselves is never touched"##,
            enabled: can_install,
            run: imp::remove,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.setToken",
            label: "Set Hugging Face Token",
            menu: &[],
            shortcut: None,
            params: r##"{"token":str (empty removes it)} → {set}: an access token pasted by hand, the fallback to Sign in with Hugging Face, that gated model files are downloaded with (the app saves it with the API keys, apart from the preferences). Sent only to huggingface.co"##,
            enabled: always,
            run: imp::set_token,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.hfStatus",
            label: "Hugging Face Sign-In",
            menu: &[],
            shortcut: None,
            params: r##"{} → {available,signedIn,expired (the sign-in ran out and can't be renewed: sign in again),via:oauth|token|none,user,expiresAt?,canSignIn (Sign in with Hugging Face is set up in this build),signingIn,error (why the last sign-in failed),found (a Hugging Face token is on this PC, for the user to choose to use),seq,links:{help,connectedApps,tokens}}: never a token"##,
            enabled: always,
            run: imp::hf_status,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.hfAccess",
            label: "Check Hugging Face Access",
            menu: &[],
            shortcut: None,
            params: r##"{"models":[id,…]?=every gated model} → {models:[{id,name,page,licence,access:signIn|licence|ready|unavailable,detail}],signedIn,user,via,note}: where each gated model stands (one HEAD of its gated file: 401 sign in, 403 accept the licence, 2xx ready, 404 unavailable: moved by its maker, a newer app version knows where). Renews a sign-in about to expire; one that expired or that Hugging Face refuses is forgotten, and `note` says so. Downloads nothing"##,
            enabled: can_install,
            run: imp::hf_access,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.hfSignIn",
            label: "Sign In with Hugging Face",
            menu: &[],
            shortcut: None,
            params: r##"{"cancel":bool=false,"product":str?} → {url,started} | {cancelled}: starts Sign in with Hugging Face (OAuth with PKCE, scopes openid profile gated-repos); the app opens `url` in the browser, which comes back to 127.0.0.1. Waits five minutes. The user's to do: automation is refused"##,
            enabled: can_install,
            run: imp::hf_sign_in,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.hfSignOut",
            label: "Sign Out of Hugging Face",
            menu: &[],
            shortcut: None,
            params: r##"{} → {signedOut,connectedApps}: forgets the Hugging Face sign-in here (revoking the app is done on Hugging Face, at connectedApps)"##,
            enabled: always,
            run: imp::hf_sign_out,
            journal: false,
        },
        CommandSpec {
            id: "ai.setup.hfUseFound",
            label: "Use the Hugging Face Sign-In on This PC",
            menu: &[],
            shortcut: None,
            params: r##"{} → {used}: uses the Hugging Face token found on this PC (HF_TOKEN, or huggingface_hub's token file), as the user chose to"##,
            enabled: can_install,
            run: imp::hf_use_found,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
