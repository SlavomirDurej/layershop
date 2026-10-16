//! A stand-in network and engine for the wizard, so its screens can be drawn and driven without
//! downloading anything: what the wizard's tests and the offscreen snapshots
//! (`--example snapshot -- --ai-setup-demo <page>`) run against.
//!
//! Nothing here reaches the network. [`Scale::Real`] keeps the real manifest (real names and
//! sizes) and trickles zeros in place of the files, so a setup shows its progress and never
//! finishes; [`Scale::Small`] cuts the manifest down to files of a few kilobytes that arrive
//! whole, so a setup runs to its end and its test image; [`Scale::Gated`] is the same with
//! FLUX.2 Klein 9B in place of Z-Image Turbo, a gated model whose file opens only to the token
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
    /// Qwen Image 2.1 and FLUX.2 Klein 9B (gated), their files a few kilobytes.
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

/// [`small_manifest`] with FLUX.2 Klein 9B, which is gated, in place of Z-Image Turbo.
pub fn gated_manifest() -> Option<Manifest> {
    manifest_of(&["qwen-image-2-1", "flux-2-klein-9b"])
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
    /// is 401, another one 403, as Hugging Face answers.
    gated: Vec<String>,
    accepted: Arc<Mutex<String>>,
}

impl Fetch for DemoFetch {
    fn head(&self, url: &str, token: Option<&str>) -> photocraft_aisetup::Result<Head> {
        if self.gated.iter().any(|g| g == url) {
            let accepted = self.accepted.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let status = match token {
                None => 401,
                Some(t) if !accepted.is_empty() && t == accepted => 200,
                Some(_) => 403,
            };
            return Ok(Head { status, error_code: (status != 200).then(|| "GatedRepo".to_string()), ..Head::default() });
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
    use photocraft_aisetup::probe::{Drive, Gpu, Host, Os, Probe, Vendor};
    let host = |name: &str| Host { name: name.into(), ok: true, note: String::new() };
    let p = Probe {
        os: Os { name: "Windows 11 Pro".into(), version: "10.0.26100".into(), sixty_four_bit: true, windows: true },
        gpus: vec![Gpu { name: "NVIDIA GeForce RTX 4090".into(), vendor: Vendor::Nvidia, vram_mb: Some(24564), driver: "591.59".into() }],
        drives: vec![Drive { name: "D:".into(), free: 412_000_000_000, total: 2_000_000_000_000 }],
        comfy,
        hosts: vec![host("github.com"), host("huggingface.co")],
    };
    let (variant, tested) = p.variant();
    json!({
        "probe": p, "checks": p.checks(), "variant": variant, "variantTested": tested, "canInstall": p.can_install(), "vramGb": 24,
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
    let root = dir.join("engine").join("0.39.0");
    std::fs::create_dir_all(root.join("python_embeded")).ok()?;
    std::fs::write(root.join("python_embeded").join("python.exe"), vec![0u8; 4000]).ok()?;
    runtime::write_marker(&root, &runtime::Marker { version: "0.39.0".into(), variant: "nvidia".into(), sha256: "ab".into() }).ok()?;
    let manifest = small_manifest()?;
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
        comfy_version: "0.39.0".into(),
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
/// with ComfyUI Desktop; `models-hf`, `models-licence` and `models-ready` the Models screen with
/// FLUX.2 Klein 9B ticked (signed out; signed in without its licence accepted; ready),
/// `sign-in` and `sign-in-licence` the question Install asks then; `close` the question Close
/// asks while a setup runs; `env` only gives the app the stand-in, with the wizard closed (its
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
        app.ui.ai_setup.models = vec!["qwen-image-2-1".into(), "flux-2-klein-9b".into()];
        super::name_preset(app);
        app.ui.ai_setup.page = Page::Models;
        super::replan(app);
        super::check_access(app);
        if asked {
            super::install_clicked(app);
        }
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
