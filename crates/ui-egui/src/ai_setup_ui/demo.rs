//! A stand-in network and engine for the wizard, so its screens can be drawn and driven without
//! downloading anything: what the wizard's tests and the offscreen snapshots
//! (`--example snapshot -- --ai-setup-demo <page>`) run against.
//!
//! Nothing here reaches the network. [`Scale::Real`] keeps the real manifest (real names and
//! sizes) and trickles zeros in place of the files, so a setup shows its progress and never
//! finishes; [`Scale::Small`] cuts the manifest down to files of a few kilobytes that arrive
//! whole, so a setup runs to its end and its test image; [`Scale::Gated`] is the same with
//! FLUX.3 Klein in place of Z-Image Turbo, a gated model whose file opens only to the token
//! [`install`] returns a handle on (as Hugging Face's does only once the licence is accepted).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use photocraft_aigen::{Method, Request, Response, Transport};
use photocraft_aisetup::Manifest;
use photocraft_aisetup::fetch::{Begin, Fetch, Head, Sink};
use photocraft_aisetup::probe::Existing;
use photocraft_engine::ai_setup_cmds::SetupEnv;
use serde_json::{Value, json};

use super::Page;
use crate::PhotocraftApp;

/// How much like the real thing the stand-in is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// The real manifest; downloads trickle and never finish.
    Real,
    /// Two models whose files are a few kilobytes; a setup finishes in moments.
    Small,
    /// Qwen Image 2.1 and FLUX.3 Klein (gated), their files a few kilobytes.
    Gated,
}

/// The stand-in's Hugging Face access token, for a demo that is signed in.
pub const TOKEN: &str = "hf_demo_token";

/// The stand-in "test image": the app's own icon.
const IMAGE: &[u8] = include_bytes!("../../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.photocraft.png");

/// What a small manifest's file holds: a few kilobytes that differ by file.
fn contents(key: &str) -> Vec<u8> {
    let seed = key.bytes().fold(5u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    (0..8_000u32).map(|i| (i.wrapping_mul(seed) >> 6) as u8).collect()
}

/// The bundled manifest cut down to two models with small files (and their real names).
pub fn small_manifest() -> Option<Manifest> {
    manifest_of(&["qwen-image-2-1", "z-image-turbo"])
}

/// [`small_manifest`] with FLUX.3 Klein, which is gated, in place of Z-Image Turbo.
pub fn gated_manifest() -> Option<Manifest> {
    manifest_of(&["qwen-image-2-1", "flux-3-klein"])
}

fn manifest_of(ids: &[&str]) -> Option<Manifest> {
    let mut m = Manifest::bundled().ok()?.clone();
    m.models.retain(|x| ids.contains(&x.id.as_str()));
    let used: Vec<String> = m.models.iter().flat_map(|x| x.files.clone()).collect();
    m.files.retain(|key, _| used.contains(key));
    for (key, file) in &mut m.files {
        let bytes = contents(key);
        (file.size, file.sha256) = (bytes.len() as u64, photocraft_aisetup::download::sha256_hex(&bytes));
    }
    for list in m.presets.values_mut() {
        list.retain(|id| m.models.iter().any(|x| x.id == *id));
    }
    Some(m)
}

struct DemoFetch {
    /// The files that arrive whole, by address. Any other address trickles zeros.
    files: HashMap<String, Vec<u8>>,
    /// Bytes a second of a trickle.
    rate: u64,
    /// The gated files, by address, and the token whose owner accepted their licence: no token
    /// (or a `revoked…` one) is 401, another one 403, as Hugging Face answers. `moved`: the files
    /// are not there any more, 404 to anyone signed in.
    gated: Vec<String>,
    accepted: Arc<Mutex<String>>,
}

impl Fetch for DemoFetch {
    fn head(&self, url: &str, token: Option<&str>) -> photocraft_aisetup::Result<Head> {
        if self.gated.iter().any(|g| g == url) {
            let accepted = self.accepted.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let status = match token {
                None => 401,
                Some(t) if t.starts_with("revoked") => 401,
                Some(_) if accepted == "moved" => 404,
                Some(t) if !accepted.is_empty() && t == accepted => 200,
                Some(_) => 403,
            };
            let code = match status {
                200 => None,
                404 => Some("EntryNotFound".to_string()),
                _ => Some("GatedRepo".to_string()),
            };
            return Ok(Head { status, error_code: code, ..Head::default() });
        }
        Ok(Head { status: 200, size: self.files.get(url).map(|f| f.len() as u64), sha256: None, ..Head::default() })
    }

    fn fetch(&self, request: &photocraft_aisetup::fetch::Request<'_>, sink: &mut dyn Sink, cancelled: &dyn Fn() -> bool) -> photocraft_aisetup::Result<()> {
        if let Some(data) = self.files.get(request.url) {
            let from = usize::try_from(request.offset).unwrap_or(usize::MAX).min(data.len());
            sink.begin(&Begin { offset: request.offset.min(data.len() as u64), total: Some(data.len() as u64), validator: String::new() })?;
            return sink.write(data.get(from..).unwrap_or(&[]));
        }
        sink.begin(&Begin { offset: request.offset, total: None, validator: String::new() })?;
        let chunk = vec![0u8; usize::try_from(self.rate / 10).unwrap_or(65_536).clamp(1024, 1 << 20)];
        loop {
            if cancelled() {
                return Err(photocraft_aisetup::Error::Cancelled);
            }
            sink.write(&chunk)?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A ComfyUI that is running, has no models of its own, and renders the stand-in image. Its
/// models folders are `folders`.
struct DemoComfy {
    folders: Vec<String>,
}

impl Transport for DemoComfy {
    fn send(&self, request: &Request) -> photocraft_aigen::Result<Response> {
        let ok = |body: &[u8]| Ok(Response { status: 200, body: body.to_vec() });
        let url = request.url.as_str();
        if url.ends_with("/system_stats") {
            ok(br#"{"system":{"argv":["main.py"]}}"#)
        } else if url.ends_with("/experiment/models") {
            let paths: Vec<String> = self.folders.iter().map(|f| Path::new(f).join("diffusion_models").to_string_lossy().into_owned()).collect();
            ok(json!([{"name": "diffusion_models", "folders": paths}]).to_string().as_bytes())
        } else if url.contains("/models/") {
            ok(b"[]")
        } else if url.ends_with("/prompt") && request.method == Method::Post {
            ok(br#"{"prompt_id":"demo"}"#)
        } else if url.contains("/history/") {
            ok(br#"{"demo":{"outputs":{"9":{"images":[{"filename":"demo.png","subfolder":"","type":"output"}]}},"status":{"completed":true}}}"#)
        } else if url.contains("/view?") {
            ok(IMAGE)
        } else {
            Ok(Response { status: 404, body: Vec::new() })
        }
    }
}

/// Give `app` the stand-in network and engine, with its folders and its record in `dir`.
/// Returns the token the gated files open to (none until one is set: the licence not accepted
/// by anyone yet).
pub fn install(app: &mut PhotocraftApp, dir: &Path, scale: Scale) -> Arc<Mutex<String>> {
    let manifest = match scale {
        Scale::Real => None,
        Scale::Small => small_manifest().map(Arc::new),
        Scale::Gated => gated_manifest().map(Arc::new),
    };
    let files = manifest.as_ref().map(|m| m.files.iter().map(|(key, f)| (f.url.clone(), contents(key))).collect()).unwrap_or_default();
    let all = manifest.clone().or_else(|| Manifest::bundled().ok().map(|m| Arc::new(m.clone())));
    let gated = all.map(|m| m.files.values().filter(|f| f.gated).map(|f| f.url.clone()).collect()).unwrap_or_default();
    let accepted = Arc::new(Mutex::new(String::new()));
    app.session.ai.transport = Some(Arc::new(DemoComfy { folders: comfy_folders(dir) }));
    app.session.ai_setup.env = Some(SetupEnv {
        fetch: Arc::new(DemoFetch { files, rate: 6_000_000, gated, accepted: accepted.clone() }),
        state_file: dir.join("ai-setup.json"),
        engine_dir: dir.join("engine").to_string_lossy().into_owned(),
        models_dir: dir.join("models").to_string_lossy().into_owned(),
        manifest,
        // An app to sign in as, so Sign in with Hugging Face is offered (it never gets that far).
        hf_client_id: "demo-client".into(),
    });
    accepted
}

/// What the engine printed while starting on the processor in a virtual machine (2026-10-20).
const SLOW_START: &[&str] = &[
    "\u{1b}[32m[INFO]\u{1b}[0m setup plugin alembic.autogenerate.comments",
    "\u{1b}[1m\u{1b}[33m[WARNING]\u{1b}[0m On windows we are currently forcing single GPU mode in ComfyUI due to a Nvidia related issue, if you want to disable this use: --cuda-device all",
    "\u{1b}[32m[INFO]\u{1b}[0m Adding extra search path diffusion_models C:\\Users\\Admin\\AppData\\Local\\LayerShop\\models\\diffusion_models",
    "\u{1b}[32m[INFO]\u{1b}[0m Adding extra search path vae C:\\Users\\Admin\\AppData\\Local\\LayerShop\\models\\vae",
    "\u{1b}[32m[INFO]\u{1b}[0m Found comfy_kitchen backend eager: {'available': True, 'disabled': False, 'unavailable_reason': None}",
    "\u{1b}[32m[INFO]\u{1b}[0m Found comfy_kitchen backend cuda: {'available': False, 'disabled': False, 'unavailable_reason': 'CUDA not available on this system'}",
];

/// An engine that takes its time starting, as one without a graphics card does: it reports
/// where it is until the setup is stopped, and has its own output to show.
struct SlowEngine;

impl photocraft_aigen::LocalServer for SlowEngine {
    fn ensure(&self, _: &dyn Transport, _: &photocraft_aigen::Settings, control: &photocraft_aigen::Control<'_>) -> photocraft_aigen::Result<()> {
        let began = std::time::Instant::now();
        let doing = photocraft_aigen::activity::describe(&self.output(80)).unwrap_or_default();
        loop {
            if (control.cancelled)() {
                return Err(photocraft_aigen::Error::Cancelled);
            }
            // Three minutes in, as the screenshot of a slow start would be.
            let clock = photocraft_aigen::activity::clock(began.elapsed().as_secs() + 192);
            (control.progress)(0.02, &format!("Starting ComfyUI: {doing} · {clock}"));
            (control.sleep)(Duration::from_millis(100));
        }
    }

    fn stop(&self) -> bool {
        false
    }

    fn state(&self) -> photocraft_aigen::ServerState {
        photocraft_aigen::ServerState::NotStarted
    }

    fn find(&self, _: &dyn Transport, _: &str) -> photocraft_aigen::Result<photocraft_aigen::Launch> {
        Ok(photocraft_aigen::Launch::default())
    }

    fn output(&self, lines: usize) -> Vec<String> {
        let all = photocraft_aigen::activity::lines(&SLOW_START.join("\n"));
        let skip = all.len().saturating_sub(lines);
        all.into_iter().skip(skip).collect()
    }

    fn log_file(&self) -> Option<String> {
        Some("C:\\Users\\Admin\\AppData\\Roaming\\LayerShop\\comfyui.log".into())
    }
}

/// A setup whose engine is slow to start ([`SlowEngine`]), on the Install screen at that step;
/// with `details`, What's happening? pressed. It runs until it is stopped.
pub fn slow_engine(app: &mut PhotocraftApp, dir: &Path, details: bool) -> bool {
    install(app, dir, Scale::Small);
    app.session.ai.server = Some(Arc::new(SlowEngine));
    app.ui.ai_setup.probe = Some(probe(dir));
    super::open(app);
    let s = &mut app.ui.ai_setup;
    (s.existing, s.url) = (true, "http://127.0.0.1:8188".into());
    s.server = Some(json!({"folders": {}, "launch": null, "local": true}));
    s.models = vec!["qwen-image-2-1".into()];
    super::install(app);
    // The downloads are a few kilobytes: wait for the engine's step.
    let began = std::time::Instant::now();
    let at_engine = |app: &mut PhotocraftApp| {
        super::refresh_status(app);
        let status = app.ui.ai_setup.status.clone().unwrap_or(Value::Null);
        super::list(&status, "/progress/steps").iter().any(|s| super::text(s, "/key") == "verify" && super::text(s, "/state") == "active")
    };
    while !at_engine(app) {
        if began.elapsed() > Duration::from_secs(20) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(150));
    super::refresh_status(app);
    if details {
        app.ui.ai_setup.ask = Some(super::Ask::Activity);
    }
    true
}

/// A ComfyUI that does not answer: a setup against it stops at its last step.
struct Down;

impl Transport for Down {
    fn send(&self, _: &Request) -> photocraft_aigen::Result<Response> {
        Err(photocraft_aigen::Error::Network("refused".into()))
    }
}

/// What ComfyUI said when it would not start in a Windows Sandbox: a traceback longer than the
/// Install screen has room for.
pub const TRACEBACK: &str = "ComfyUI stopped while starting: File \"C:\\Users\\WDAGUtilityAccount\\AppData\\Local\\PhotoCraft\\engine\\0.39.0\\python_embeded\\Lib\\site-packages\\torch\\__init__.py\", line 141, in <module>
    raise err
OSError: [WinError 126] The specified module could not be found. Error loading \"C:\\Users\\WDAGUtilityAccount\\AppData\\Local\\PhotoCraft\\engine\\0.39.0\\python_embeded\\Lib\\site-packages\\torch\\lib\\c10_cuda.dll\" or one of its dependencies.

Traceback (most recent call last):
  File \"C:\\Users\\WDAGUtilityAccount\\AppData\\Local\\PhotoCraft\\engine\\0.39.0\\ComfyUI\\main.py\", line 138, in <module>
    import execution
  File \"C:\\Users\\WDAGUtilityAccount\\AppData\\Local\\PhotoCraft\\engine\\0.39.0\\ComfyUI\\execution.py\", line 13, in <module>
    import comfy.model_management
  File \"C:\\Users\\WDAGUtilityAccount\\AppData\\Local\\PhotoCraft\\engine\\0.39.0\\ComfyUI\\comfy\\model_management.py\", line 21, in <module>
    import torch";

/// A setup against the user's own ComfyUI, [`Scale::Small`], run to its end with that ComfyUI
/// not answering: it stops at its last step, with the wizard open on the Install screen. With
/// `said`, the engine's error reads that instead (what a real failure says is longer).
pub fn fail(app: &mut PhotocraftApp, dir: &Path, said: Option<&str>) {
    install(app, dir, Scale::Small);
    app.session.ai.transport = Some(Arc::new(Down));
    app.ui.ai_setup.probe = Some(probe(dir));
    super::open(app);
    let s = &mut app.ui.ai_setup;
    (s.existing, s.url) = (true, "http://127.0.0.1:8188".into());
    s.server = Some(json!({"folders": {}, "launch": null, "local": true}));
    s.models = vec!["qwen-image-2-1".into()];
    s.models_dir = dir.join("models").to_string_lossy().into_owned();
    super::install(app);
    let began = std::time::Instant::now();
    while super::running(app) && began.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(5));
    }
    photocraft_engine::ai_setup_cmds::poll(&mut app.session);
    let Some(said) = said else { return };
    // Taken as seen, so the next frame doesn't read the status again over what is put in it.
    app.ui.ai_setup.seen_seq = app.session.ai_setup.seq();
    super::refresh_status(app);
    if let Some(status) = app.ui.ai_setup.status.as_mut() {
        status["result"]["error"] = json!(said);
        if status["record"].is_object() {
            status["record"]["error"] = json!(said);
        }
        if let Some(steps) = status.pointer_mut("/progress/steps").and_then(Value::as_array_mut) {
            for step in steps.iter_mut().filter(|s| s["state"] == "failed") {
                step["note"] = json!(said);
            }
        }
    }
    app.ui.ai_setup.page = Page::Install;
}

/// Signed in to Hugging Face as the demo's user (with [`TOKEN`]), as Sign in leaves it.
pub fn sign_in(app: &mut PhotocraftApp) {
    use photocraft_aisetup::hf::{Credentials, Via};
    app.session.ai.set_hf(&Credentials {
        access: TOKEN.into(),
        refresh: "hf_demo_refresh".into(),
        expires_at: photocraft_aisetup::hf::unix_now() + 28_800,
        user: "pixel-painter".into(),
        via: Via::OAuth,
    });
}

/// The models folders of the stand-in's own ComfyUI, in `dir`: ComfyUI Desktop's shared one,
/// where it saves new models, and the one in its base folder.
pub fn comfy_folders(dir: &Path) -> Vec<String> {
    [dir.join("ComfyUI-Shared").join("models"), dir.join("Documents").join("ComfyUI").join("models")]
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// The machine of the website's walkthrough, as `ai.setup.probe` would describe it, with the
/// folders the app would suggest in `dir`.
pub fn probe(dir: &Path) -> Value {
    probe_with(dir, Existing::default())
}

/// [`probe`] with a smaller card: an 8 GB one, which is offered the Light preset.
pub fn probe_small_card(dir: &Path) -> Value {
    use photocraft_aisetup::probe::{Gpu, Vendor};
    let mut p: photocraft_aisetup::probe::Probe = serde_json::from_value(probe(dir)["probe"].clone()).unwrap_or_default();
    p.gpus = vec![Gpu { name: "NVIDIA GeForce RTX 4060".into(), vendor: Vendor::Nvidia, vram_mb: Some(8188), driver: "591.59".into() }];
    describe(dir, &p)
}

/// A virtual machine's: its display adapter is no card the engine can use.
pub fn probe_without_card(dir: &Path) -> Value {
    use photocraft_aisetup::probe::{Gpu, Vendor};
    let mut p: photocraft_aisetup::probe::Probe = serde_json::from_value(probe(dir)["probe"].clone()).unwrap_or_default();
    p.gpus = vec![Gpu { name: "VMware SVGA 3D".into(), vendor: Vendor::Other, vram_mb: Some(256), driver: "9.17.7.4".into() }];
    describe(dir, &p)
}

/// [`probe`] on a machine with ComfyUI Desktop installed but not running, its models in the
/// folders of [`comfy_folders`].
pub fn probe_with_comfy(dir: &Path) -> Value {
    let launch = photocraft_aigen::Launch {
        python: dir.join("Documents").join("ComfyUI").join(".venv").join("Scripts").join("python.exe").to_string_lossy().into_owned(),
        folder: dir.join("ComfyUI-Installs").join("ComfyUI").to_string_lossy().into_owned(),
        args: "ComfyUI\\main.py --enable-manager".into(),
    };
    probe_with(dir, Existing { running: Vec::new(), installed: Some(launch), models: comfy_folders(dir) })
}

fn probe_with(dir: &Path, comfy: Existing) -> Value {
    use photocraft_aisetup::probe::{Drive, Gpu, Host, Os, Probe, VcRuntime, Vendor};
    let host = |name: &str| Host { name: name.into(), ok: true, note: String::new() };
    let p = Probe {
        os: Os {
            name: "Windows 11 Pro".into(),
            version: "10.0.26100".into(),
            sixty_four_bit: true,
            windows: true,
            memory_mb: Some(65_536),
            virtual_mb: Some(75_000),
            paging_managed: Some(true),
        },
        gpus: vec![Gpu { name: "NVIDIA GeForce RTX 4090".into(), vendor: Vendor::Nvidia, vram_mb: Some(24564), driver: "591.59".into() }],
        drives: vec![Drive { name: "D:".into(), free: 412_000_000_000, total: 2_000_000_000_000 }],
        comfy,
        hosts: vec![host("github.com"), host("huggingface.co")],
        vc_runtime: VcRuntime::Bundled,
    };
    describe(dir, &p)
}

/// `ai.setup.probe`'s answer for `p`.
fn describe(dir: &Path, p: &photocraft_aisetup::probe::Probe) -> Value {
    let (variant, tested) = p.variant();
    let vram = p.usable_gpu().and_then(|g| g.vram_mb).map(|mb| (mb as f64 / 1024.0).round() as u64);
    json!({
        "probe": p, "checks": p.checks(), "variant": variant, "variantTested": tested, "canInstall": p.can_install(),
        "gpuUsable": p.usable_gpu().is_some(), "vramGb": vram,
        "defaults": {"engineDir": dir.join("engine").to_string_lossy(), "modelsDir": dir.join("models").to_string_lossy()},
    })
}

/// Make `dir` look as a finished setup leaves it, without running one: an engine folder with
/// its marker, both of the small manifest's models in the models folder, and the record that
/// says so. For [`Scale::Small`]. Returns the engine's folder.
pub fn pretend_installed(app: &PhotocraftApp, dir: &Path) -> Option<std::path::PathBuf> {
    use photocraft_aisetup::plan::{Choices, Route};
    use photocraft_aisetup::state::{Phase, SetupState, save};
    use photocraft_aisetup::{manifest::split_key, runtime};
    let manifest = small_manifest()?;
    let version = manifest.comfy.version.clone();
    let root = dir.join("engine").join(&version);
    std::fs::create_dir_all(root.join("python_embeded")).ok()?;
    std::fs::write(root.join("python_embeded").join("python.exe"), vec![0u8; 4000]).ok()?;
    runtime::write_marker(&root, &runtime::Marker { version: version.clone(), variant: "nvidia".into(), sha256: "ab".into() }).ok()?;
    for key in manifest.files.keys() {
        let (folder, name) = split_key(key)?;
        std::fs::create_dir_all(dir.join("models").join(folder)).ok()?;
        std::fs::write(dir.join("models").join(folder).join(name), contents(key)).ok()?;
    }
    let models: Vec<String> = manifest.models.iter().map(|m| m.id.clone()).collect();
    let record = SetupState {
        phase: Phase::Done,
        choices: Choices {
            route: Route::Managed,
            models: models.clone(),
            engine_dir: dir.join("engine").to_string_lossy().into_owned(),
            models_dir: dir.join("models").to_string_lossy().into_owned(),
            variant: "nvidia".into(),
            ..Choices::default()
        },
        port: 8190,
        comfy_version: version,
        installed: models,
        ..SetupState::default()
    };
    save(&app.session.ai_setup.env.as_ref()?.state_file, &record).ok()?;
    Some(root)
}

/// Put one of the wizard's screens in front of `app`, as it looks in use: `pc`, `location`,
/// `models`, `install` (a setup under way) or `ready` (one that has just finished); `installed`
/// and `installed-models` are the first screen and the Models screen of a setup that is there
/// already; `pc-comfy` and `location-comfy` the first screen and Use my ComfyUI's on a machine
/// with ComfyUI Desktop; `pc-checking` the first screen while this PC is looked at (for real);
/// `models-hf`, `models-licence` and `models-ready` the Models screen with
/// FLUX.3 Klein ticked (signed out; signed in without its licence accepted; ready),
/// `models-moved` and `sign-in-moved` signed in with FLUX.3 Klein's file moved by its maker,
/// `models-light` the Models screen on an 8 GB card (the Light preset, MOSS Image first),
/// `sign-in` and `sign-in-licence` the question Install asks then; `close` the question Close
/// asks while a setup runs; `aigen-empty` the AI Gen panel with nothing set up and no ComfyUI
/// answering; `install-error` and `install-failed` a setup that stopped at its last
/// step, with the window that shows its error open, and closed; `location-full` the Location screen on a drive too small for the
/// recommended models; `env` only gives the app the stand-in, with the wizard closed (its
/// ways in: the File menu, the Preferences page, the AI Gen panel). Returns whether `page` names
/// one of these.
pub fn show(app: &mut PhotocraftApp, dir: &Path, page: &str) -> bool {
    // A gated model ticked: where it stands, on the Models screen or in the question Install asks.
    let gated = match page {
        "models-hf" => Some((false, None, false)),
        "models-licence" => Some((true, Some("someone-elses"), false)),
        "models-ready" => Some((true, Some(TOKEN), false)),
        "sign-in" => Some((false, None, true)),
        "sign-in-licence" => Some((true, Some("someone-elses"), true)),
        "models-moved" => Some((true, Some("moved"), false)),
        "sign-in-moved" => Some((true, Some("moved"), true)),
        _ => None,
    };
    if let Some((signed_in, accepted_by, asked)) = gated {
        let accepted = install(app, dir, Scale::Gated);
        *accepted.lock().unwrap_or_else(PoisonError::into_inner) = accepted_by.unwrap_or("").to_string();
        if signed_in {
            sign_in(app);
        }
        app.ui.ai_setup.probe = Some(probe(dir));
        super::open(app);
        app.ui.ai_setup.models = vec!["qwen-image-2-1".into(), "flux-3-klein".into()];
        super::name_preset(app);
        app.ui.ai_setup.page = Page::Models;
        super::replan(app);
        super::check_access(app);
        if asked {
            super::install_clicked(app);
        }
        return true;
    }
    if page == "models-light" {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe_small_card(dir));
        super::open(app);
        app.ui.ai_setup.page = Page::Models;
        return true;
    }
    if page == "close" {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe(dir));
        super::open(app);
        app.ui.ai_setup.models = vec!["qwen-image-2-1".into()];
        super::install(app);
        app.ui.ai_setup.ask = Some(super::Ask::Close);
        return true;
    }
    if page == "not-running" {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe(dir));
        super::open(app);
        let s = &mut app.ui.ai_setup;
        (s.existing, s.page, s.url) = (true, Page::Location, "http://127.0.0.1:8188".into());
        s.ask = Some(super::Ask::NotRunning);
        return true;
    }
    // A setup that stopped at its last step: the error window it opens, or the screen under it
    // once that is closed.
    if matches!(page, "install-error" | "install-failed") {
        fail(app, dir, Some(TRACEBACK));
        app.ui.ai_setup.ask = (page == "install-error").then_some(super::Ask::Error);
        return true;
    }
    // The AI Gen panel on a PC with nothing set up: no API key, local AI not installed, and no
    // ComfyUI answering. What it offers is the setup, and a key.
    if page == "aigen-empty" {
        install(app, dir, Scale::Real);
        app.session.ai.transport = Some(Arc::new(Down));
        app.ui.ai_setup.probe = Some(probe(dir));
        super::refresh_status(app);
        let _ = crate::ai_gen_ui::menu(app, crate::ai_gen_ui::PANEL, &json!({"show": true}));
        return true;
    }
    // A PC without a graphics card the engine can use (a virtual machine): the first screen
    // stops there, unless installing on the processor is ticked (`pc-no-card-cpu`).
    if let Some(ticked) = page.strip_prefix("pc-no-card").map(|rest| rest == "-cpu") {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe_without_card(dir));
        super::open(app);
        app.ui.ai_setup.cpu = ticked;
        return true;
    }
    // A setup whose engine takes its time starting (a PC without a graphics card): the Install
    // screen at that step, and What's happening? open over it.
    if let Some(details) = page.strip_prefix("install-engine").map(|rest| rest == "-details") {
        return slow_engine(app, dir, details);
    }
    // The first look at this PC, under way: the real one, in the background, so the screen in
    // front is its steps (this machine's, for the few seconds they take).
    if page == "pc-checking" {
        install(app, dir, Scale::Real);
        app.background_jobs = true;
        super::open(app);
        return app.ui.ai_setup.probe_job.is_some();
    }
    if page == "env" {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe(dir));
        super::refresh_status(app);
        return true;
    }
    // A machine with ComfyUI Desktop: the first screen asks which ComfyUI to use; Use my
    // ComfyUI's screen has its models folders.
    if let Some(screen) = page.strip_suffix("-comfy") {
        if !matches!(screen, "pc" | "location") {
            return false;
        }
        install(app, dir, Scale::Real);
        for folder in comfy_folders(dir) {
            let _ = std::fs::create_dir_all(folder);
        }
        app.ui.ai_setup.probe = Some(probe_with_comfy(dir));
        super::open(app);
        if screen == "location" {
            super::use_existing(app);
        }
        return true;
    }
    // A setup that is there already: its first screen, or its models with what can be deleted.
    if let Some(screen) = page.strip_prefix("installed") {
        install(app, dir, Scale::Small);
        app.ui.ai_setup.probe = Some(probe(dir));
        if pretend_installed(app, dir).is_none() {
            return false;
        }
        super::open(app);
        app.ui.ai_setup.page = if screen == "-models" { Page::Models } else { Page::Pc };
        return true;
    }
    // A small drive: the engine fits, the recommended models don't, and Next still goes on to
    // the Models page, where fewer can be ticked.
    if page == "location-full" {
        install(app, dir, Scale::Real);
        app.ui.ai_setup.probe = Some(probe(dir));
        super::open(app);
        app.ui.ai_setup.page = Page::Location;
        super::replan(app);
        let Some(plan) = app.ui.ai_setup.plan.as_mut() else { return false };
        let free = 40_000_000_000u64;
        let need = plan["engineBytes"].as_u64().unwrap_or(0).saturating_add(plan["modelsBytes"].as_u64().unwrap_or(0));
        let short = format!(
            "Not enough room: {} is free, and this needs {}. Choose fewer models or another folder.",
            photocraft_aisetup::human_bytes(free),
            photocraft_aisetup::human_bytes(need.saturating_add(photocraft_aisetup::plan::HEADROOM))
        );
        (plan["sameDrive"], plan["modelsFree"], plan["engineFree"]) = (json!(true), json!(free), json!(free));
        (plan["problems"], plan["modelProblems"], plan["folderProblems"]) = (json!([short]), json!([short]), json!([]));
        return true;
    }
    let Some(page) = Page::ALL.into_iter().find(|p| p.key() == page) else { return false };
    let scale = if page == Page::Ready { Scale::Small } else { Scale::Real };
    install(app, dir, scale);
    app.ui.ai_setup.probe = Some(probe(dir));
    super::open(app);
    app.ui.ai_setup.page = page;
    match page {
        Page::Install => {
            let starter = super::preset_models(app, "recommended");
            app.ui.ai_setup.models = starter;
            super::install(app);
        }
        Page::Ready => {
            // The user's own ComfyUI (the stand-in), which has none of the files yet.
            let s = &mut app.ui.ai_setup;
            (s.existing, s.url) = (true, "http://127.0.0.1:8188".into());
            s.server = Some(json!({"folders": {}, "launch": null, "local": true}));
            s.models = vec!["qwen-image-2-1".into(), "z-image-turbo".into()];
            super::install(app);
            // It is a few kilobytes: wait for it here, so the screen in front is its ending.
            let began = std::time::Instant::now();
            while super::running(app) && began.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(5));
            }
            photocraft_engine::ai_setup_cmds::poll(&mut app.session);
        }
        _ => {}
    }
    true
}
