//! A whole setup against a fake network and a fake engine: nothing leaves the test's folder.

use super::*;
use crate::fetch::{Begin, Head, Request, Sink};
use crate::plan::part_of;
use photocraft_aigen::{Method, Response, ServerState};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A stand-in for the portable build: its top folder, a Python and a script.
fn build_archive(dir: &Path, with_python: bool) -> Vec<u8> {
    let path = dir.join("fixture.7z");
    let mut writer = ArchiveWriter::create(&path).unwrap();
    writer.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory("ComfyUI_windows_portable"), None).unwrap();
    if with_python {
        writer.push_archive_entry(ArchiveEntry::new_file("ComfyUI_windows_portable/python_embeded/python.exe"), Some(&b"MZ python"[..])).unwrap();
    }
    writer.push_archive_entry(ArchiveEntry::new_file("ComfyUI_windows_portable/ComfyUI/main.py"), Some(&b"print('comfy')"[..])).unwrap();
    writer.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

/// The contents of a model file in the tests: a few kilobytes that differ by file.
fn contents(key: &str) -> Vec<u8> {
    let seed = key.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    (0..40_000u32).map(|i| (i.wrapping_mul(seed) >> 7) as u8).collect()
}

/// The real manifest cut down to three models, with files small enough to serve from memory.
fn manifest(archive: &[u8]) -> Manifest {
    let mut m = Manifest::bundled().unwrap().clone();
    m.models.retain(|x| ["qwen-image-2-1", "z-image-turbo", "flux-3-klein"].contains(&x.id.as_str()));
    let used: Vec<String> = m.models.iter().flat_map(|x| x.files.clone()).collect();
    m.files.retain(|key, _| used.contains(key));
    for (key, file) in &mut m.files {
        let bytes = contents(key);
        (file.size, file.sha256) = (bytes.len() as u64, sha(&bytes));
    }
    m.presets.clear();
    let asset = m.comfy.assets.get_mut("nvidia").unwrap();
    (asset.size, asset.sha256, asset.unpacked_bytes) = (archive.len() as u64, sha(archive), 1000);
    m
}

/// What the fake network does with an address.
#[derive(Clone, Copy, PartialEq)]
enum Net {
    Serve,
    /// Send this many bytes, then stop as a cancel would.
    StopAfter(usize),
    Down,
    /// Refuse without the right token.
    Gated,
    Corrupt,
}

#[derive(Default)]
struct FakeNet {
    files: HashMap<String, Vec<u8>>,
    rules: Mutex<HashMap<String, Net>>,
    /// Bytes served, by address.
    served: Mutex<HashMap<String, u64>>,
    tokens: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeNet {
    fn new(m: &Manifest, archive: &[u8]) -> Self {
        let mut files: HashMap<String, Vec<u8>> = m.files.iter().map(|(key, f)| (f.url.clone(), contents(key))).collect();
        files.insert(m.comfy.assets["nvidia"].url.clone(), archive.to_vec());
        Self { files, ..Self::default() }
    }

    fn rule(&self, m: &Manifest, key: &str, rule: Net) {
        let url = if key == "runtime" { m.comfy.assets["nvidia"].url.clone() } else { m.files[key].url.clone() };
        self.rules.lock().unwrap().insert(url, rule);
    }

    fn served(&self, m: &Manifest, key: &str) -> u64 {
        let url = if key == "runtime" { m.comfy.assets["nvidia"].url.clone() } else { m.files[key].url.clone() };
        self.served.lock().unwrap().get(&url).copied().unwrap_or(0)
    }

    fn total_served(&self) -> u64 {
        self.served.lock().unwrap().values().sum()
    }
}

impl Fetch for FakeNet {
    fn head(&self, url: &str, _: Option<&str>) -> Result<Head> {
        Ok(Head { status: 200, size: self.files.get(url).map(|f| f.len() as u64), sha256: None, ..Head::default() })
    }

    fn fetch(&self, request: &Request<'_>, sink: &mut dyn Sink, _: &dyn Fn() -> bool) -> Result<()> {
        self.tokens.lock().unwrap().push((request.url.to_string(), request.token.map(str::to_string)));
        let rule = self.rules.lock().unwrap().get(request.url).copied().unwrap_or(Net::Serve);
        let data = self.files.get(request.url).ok_or_else(|| crate::fetch::status_error(404, request.url))?;
        let from = (request.offset as usize).min(data.len());
        let mut send = |bytes: &[u8]| {
            sink.begin(&Begin { offset: from as u64, total: Some(data.len() as u64), validator: "\"v1\"".into() })?;
            *self.served.lock().unwrap().entry(request.url.to_string()).or_default() += bytes.len() as u64;
            sink.write(bytes)
        };
        match rule {
            Net::Serve => send(&data[from..]),
            Net::StopAfter(n) => {
                send(&data[from..(from + n).min(data.len())])?;
                Err(Error::Cancelled)
            }
            Net::Down => Err(Error::Network("no route to host".into())),
            Net::Gated if request.token != Some("hf_ok") => Err(crate::fetch::status_error(401, request.url)),
            Net::Gated => send(&data[from..]),
            Net::Corrupt => send(&vec![0x55; data.len() - from]),
        }
    }
}

const CARD_OUT_OF_MEMORY: &str = "Allocation on device: out of memory";

/// A ComfyUI that renders one fixed "image", or fails as `fails` says.
struct FakeComfy {
    running: bool,
    /// The `exception_message` the render fails with, if it does.
    fails: Option<&'static str>,
    prompts: AtomicU64,
}

impl Transport for FakeComfy {
    fn send(&self, request: &photocraft_aigen::Request) -> photocraft_aigen::Result<Response> {
        let ok = |body: &str| Ok(Response { status: 200, body: body.as_bytes().to_vec() });
        if !self.running {
            return Err(photocraft_aigen::Error::Network("refused".into()));
        }
        let url = request.url.as_str();
        if url.ends_with("/system_stats") {
            ok(r#"{"system":{"argv":["main.py"]}}"#)
        } else if url.ends_with("/prompt") && request.method == Method::Post {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            ok(r#"{"prompt_id":"p1"}"#)
        } else if let (true, Some(why)) = (url.contains("/history/p1"), self.fails) {
            ok(
                &serde_json::json!({"p1": {"status": {"status_str": "error", "messages": [["execution_error", {"node_type": "KSampler", "exception_message": why}]]}}}).to_string(),
            )
        } else if url.contains("/history/p1") {
            ok(r#"{"p1":{"outputs":{"9":{"images":[{"filename":"test.png","subfolder":"","type":"output"}]}},"status":{"completed":true}}}"#)
        } else if url.contains("/view?") {
            ok("\u{89}PNG test image")
        } else {
            Ok(Response { status: 404, body: Vec::new() })
        }
    }
}

#[derive(Default)]
struct FakeServer {
    /// What `ensure` answers; the settings it was asked to start.
    refuse: Option<String>,
    asked: Mutex<Vec<Settings>>,
    /// What it last printed.
    printed: Vec<String>,
}

impl LocalServer for FakeServer {
    fn output(&self, lines: usize) -> Vec<String> {
        self.printed.iter().rev().take(lines).rev().cloned().collect()
    }

    fn ensure(&self, _: &dyn Transport, settings: &Settings, _: &Control<'_>) -> photocraft_aigen::Result<()> {
        self.asked.lock().unwrap().push(settings.clone());
        match &self.refuse {
            Some(why) => Err(photocraft_aigen::Error::Server(why.clone())),
            None => Ok(()),
        }
    }

    fn stop(&self) -> bool {
        false
    }

    fn state(&self) -> ServerState {
        ServerState::NotStarted
    }

    fn find(&self, _: &dyn Transport, _: &str) -> photocraft_aigen::Result<Launch> {
        Ok(Launch::default())
    }
}

struct Rig {
    dir: tempfile::TempDir,
    manifest: Manifest,
    net: Arc<FakeNet>,
    comfy: Arc<FakeComfy>,
    server: Arc<FakeServer>,
    /// Where the C++ runtime is put beside the engine's Python from (`None`: it isn't).
    vc_runtime: std::cell::RefCell<Option<crate::vcrt::Sources>>,
}

fn plenty(_: &Path) -> Option<u64> {
    Some(u64::MAX)
}

fn cramped(_: &Path) -> Option<u64> {
    Some(1_000_000)
}

impl Rig {
    fn new() -> Self {
        Self::with(true, FakeComfy { running: true, fails: None, prompts: AtomicU64::new(0) }, FakeServer::default())
    }

    fn with(python: bool, comfy: FakeComfy, server: FakeServer) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let archive = build_archive(dir.path(), python);
        let manifest = manifest(&archive);
        let net = Arc::new(FakeNet::new(&manifest, &archive));
        Self { dir, manifest, net, comfy: Arc::new(comfy), server: Arc::new(server), vc_runtime: Default::default() }
    }

    fn state_file(&self) -> PathBuf {
        self.dir.path().join("settings").join("ai-setup.json")
    }

    fn choices(&self, models: &[&str]) -> Choices {
        Choices {
            models: models.iter().map(|s| s.to_string()).collect(),
            engine_dir: self.dir.path().join("engine").to_string_lossy().into_owned(),
            models_dir: self.dir.path().join("models").to_string_lossy().into_owned(),
            variant: "nvidia".into(),
            test_image: true,
            ..Choices::default()
        }
    }

    fn job(&self, choices: Choices) -> Job {
        Job { manifest: self.manifest.clone(), choices, settings: Settings::default(), token: None, port: 0, url: String::new(), launch: None }
    }

    /// Run a setup to its end.
    fn run_job(&self, job: Job, free_space: fn(&Path) -> Option<u64>) -> (Result<Outcome>, Progress) {
        let env = Env {
            fetch: self.net.clone(),
            transport: self.comfy.clone(),
            server: Some(self.server.clone()),
            state_file: Some(self.state_file()),
            sleep: |_| {},
            free_space,
            vc_runtime: self.vc_runtime.borrow().clone(),
        };
        let mut handle = Handle::start(Arc::new(env), job).unwrap();
        let began = Instant::now();
        loop {
            if let Some(result) = handle.take() {
                return (result, handle.progress());
            }
            assert!(began.elapsed() < Duration::from_secs(30), "the setup did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(&self, models: &[&str]) -> (Result<Outcome>, Progress) {
        self.run_job(self.job(self.choices(models)), plenty)
    }

    fn layout(&self) -> Layout {
        Layout::new(&self.manifest, &self.choices(&[])).unwrap()
    }

    fn phase(&self) -> Phase {
        state::load(&self.state_file()).phase
    }
}

const UNET: &str = "diffusion_models/qwen_image_2.1_int8_convrot.safetensors";
const ENCODER: &str = "text_encoders/qwen3vl_8b_fp8_scaled.safetensors";

/// The goal of the plan: from nothing to a test image, with no decisions left to make.
#[test]
fn a_clean_machine_ends_with_an_engine_its_models_and_a_test_image() {
    let rig = Rig::new();
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let done = result.unwrap();
    let layout = rig.layout();
    // The engine: unpacked without its top folder, marked, its archive gone again.
    assert_eq!(std::fs::read(&layout.python).unwrap(), b"MZ python");
    assert!(layout.script.is_file() && layout.model_paths.is_file());
    assert_eq!(
        runtime::read_marker(&layout.engine_root).unwrap(),
        Marker { version: "0.43.0".into(), variant: "nvidia".into(), sha256: rig.manifest.comfy.assets["nvidia"].sha256.clone() }
    );
    assert!(!layout.archive.exists() && !part_of(&layout.archive).exists());
    // The models: each file whole, under its final name.
    for key in &rig.manifest.model("qwen-image-2-1").unwrap().files {
        assert_eq!(std::fs::read(layout.model_file(key).unwrap()).unwrap(), contents(key), "{key}");
    }
    assert_eq!(done.installed, ["qwen-image-2-1"]);
    assert!(done.skipped.is_empty());
    assert_eq!(done.folders["vae"], ["qwen_image_2.1_vae_bf16.safetensors"]);
    // The engine gets a port of its own, not ComfyUI's usual ones, and is started as the
    // portable build starts itself.
    assert!(done.port >= runtime::FIRST_PORT && done.url == format!("http://127.0.0.1:{}", done.port));
    let launch = done.launch.clone().unwrap();
    assert_eq!(launch.python, layout.python.to_string_lossy());
    assert!(launch.args.contains("--disable-auto-launch") && launch.args.contains("layershop-models.yaml"));
    // The engine was started with exactly those settings, and rendered the test image.
    let asked = rig.server.asked.lock().unwrap();
    assert_eq!((asked.len(), asked[0].comfy_url.as_str(), asked[0].comfy_autostart, &asked[0].comfy_launch), (1, done.url.as_str(), true, &launch));
    let test = done.test.clone().unwrap();
    assert_eq!((test.model.as_str(), test.model_name.as_str(), test.bytes.as_slice()), ("qwen-image-2-1", "Qwen Image 2.1", "\u{89}PNG test image".as_bytes()));
    assert_eq!((done.test_error.clone(), rig.comfy.prompts.load(Ordering::SeqCst)), (None, 1));
    // Every row ended done, and the bytes add up.
    assert!(progress.steps.iter().all(|s| s.state == StepState::Done), "{:?}", progress.steps);
    assert_eq!(progress.bytes_done, progress.bytes_total);
    assert_eq!(
        progress.steps.iter().map(|s| s.key.as_str()).filter(|k| !k.starts_with("file:")).collect::<Vec<_>>(),
        ["runtime", "unpack", "paths", "configure", "verify"]
    );
    let record = state::load(&rig.state_file());
    assert_eq!(
        (record.phase, record.installed, record.comfy_version.as_str(), record.port),
        (Phase::Done, vec!["qwen-image-2-1".to_string()], "0.43.0", done.port)
    );
    // Applied to the settings as they are then: the panel offers what was installed.
    let mut settings = Settings::default();
    let configured = apply(&mut settings, &rig.manifest, &done);
    assert_eq!(configured.ready, ["qwen-image-2-1"]);
    assert_eq!((settings.comfy_url.as_str(), settings.comfy_autostart), (done.url.as_str(), true));
}

#[test]
fn a_setup_stopped_half_way_continues_with_only_what_is_missing() {
    let rig = Rig::new();
    rig.net.rule(&rig.manifest, ENCODER, Net::StopAfter(15_000));
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    assert_eq!(result.unwrap_err(), Error::Cancelled);
    assert_eq!(rig.phase(), Phase::Paused);
    let layout = rig.layout();
    // What arrived is kept: a part of the encoder, and nothing under its final name.
    let encoder = layout.model_file(ENCODER).unwrap();
    assert_eq!(std::fs::metadata(part_of(&encoder)).unwrap().len(), 15_000);
    assert!(!encoder.exists());
    assert!(progress.steps.iter().all(|s| s.state != StepState::Failed));
    assert!(rig.server.asked.lock().unwrap().is_empty(), "nothing is started on half an install");
    // Started again: planning finds what is there, and only the rest is asked for.
    let before = rig.net.total_served();
    let (unet_before, runtime_before) = (rig.net.served(&rig.manifest, UNET), rig.net.served(&rig.manifest, "runtime"));
    rig.net.rule(&rig.manifest, ENCODER, Net::Serve);
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let done = result.unwrap();
    assert_eq!(done.installed, ["qwen-image-2-1"]);
    assert_eq!(std::fs::read(&encoder).unwrap(), contents(ENCODER));
    let again = rig.net.total_served() - before;
    let whole: u64 =
        rig.manifest.model("qwen-image-2-1").unwrap().files.iter().map(|k| rig.manifest.files[k].size).sum::<u64>() + rig.manifest.comfy.assets["nvidia"].size;
    assert!(again < whole, "{again} of {whole} bytes were fetched again");
    // A file that was finished is not fetched twice; the encoder only from where it stopped.
    let first_unet = unet_before.max(rig.net.served(&rig.manifest, UNET));
    assert_eq!(first_unet, rig.manifest.files[UNET].size);
    assert_eq!(rig.net.served(&rig.manifest, ENCODER), rig.manifest.files[ENCODER].size);
    assert_eq!(runtime_before.max(rig.net.served(&rig.manifest, "runtime")), rig.manifest.comfy.assets["nvidia"].size);
    assert!(progress.steps.iter().all(|s| s.state == StepState::Done));
    assert_eq!(rig.phase(), Phase::Done);
}

#[test]
fn adding_a_model_later_downloads_only_its_own_files_and_keeps_the_port() {
    let rig = Rig::new();
    let first = rig.run(&["qwen-image-2-1"]).0.unwrap();
    let before = rig.net.total_served();
    let mut job = rig.job(rig.choices(&["qwen-image-2-1", "z-image-turbo"]));
    job.port = first.port;
    let (result, progress) = rig.run_job(job, plenty);
    let done = result.unwrap();
    assert_eq!(done.installed, ["qwen-image-2-1", "z-image-turbo"]);
    assert_eq!(done.port, first.port);
    let added: u64 = rig.manifest.model("z-image-turbo").unwrap().files.iter().map(|k| rig.manifest.files[k].size).sum();
    assert_eq!(rig.net.total_served() - before, added);
    assert!(progress.steps.iter().all(|s| s.state == StepState::Done));
}

#[test]
fn a_gated_model_is_left_out_and_the_rest_installs() {
    let rig = Rig::new();
    let klein = "diffusion_models/flux3_image_klein_base_int8_convrot.safetensors";
    rig.net.rule(&rig.manifest, klein, Net::Gated);
    // No token: the plan leaves the model out, and nothing of it is asked for.
    let (result, progress) = rig.run(&["qwen-image-2-1", "flux-3-klein"]);
    let done = result.unwrap();
    assert_eq!(done.installed, ["qwen-image-2-1"]);
    assert_eq!(
        done.skipped.iter().map(|m| (m.id.as_str(), m.skipped.as_deref().unwrap())).collect::<Vec<_>>(),
        [("flux-3-klein", "needs a Hugging Face sign-in")]
    );
    assert_eq!(rig.net.served(&rig.manifest, klein) + rig.net.served(&rig.manifest, "text_encoders/qwen3vl_4b_int8_convrot.safetensors"), 0);
    assert_eq!(progress.steps.iter().filter(|s| s.state == StepState::Skipped).count(), 3);
    assert!(done.test.is_some());
    // A token the model's licence was not accepted with: refused by the host, still not fatal.
    let mut job = rig.job(Choices { token: true, ..rig.choices(&["qwen-image-2-1", "flux-3-klein"]) });
    job.token = Some("hf_wrong".into());
    let (result, progress) = rig.run_job(job, plenty);
    let done = result.unwrap();
    assert_eq!(done.installed, ["qwen-image-2-1"]);
    assert!(done.skipped[0].skipped.as_deref().unwrap().contains("gated"), "{:?}", done.skipped);
    let refused = progress.steps.iter().find(|s| s.key == format!("file:{klein}")).unwrap();
    assert_eq!(refused.state, StepState::Skipped);
    // With access: it installs, and the token went to the gated file only.
    let mut job = rig.job(Choices { token: true, ..rig.choices(&["qwen-image-2-1", "flux-3-klein"]) });
    job.token = Some("hf_ok".into());
    let done = rig.run_job(job, plenty).0.unwrap();
    assert_eq!(done.installed, ["qwen-image-2-1", "flux-3-klein"]);
    let tokens = rig.net.tokens.lock().unwrap();
    assert!(tokens.iter().all(|(url, token)| token.is_none() || *url == rig.manifest.files[klein].url), "the token went to a file that did not need it");
}

#[test]
fn a_network_that_is_down_fails_the_setup_and_leaves_it_resumable() {
    let rig = Rig::new();
    rig.net.rule(&rig.manifest, UNET, Net::Down);
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let e = result.unwrap_err();
    assert!(matches!(&e, Error::Network(m) if m.contains("kept failing")), "{e:?}");
    let failed = progress.steps.iter().find(|s| s.key == format!("file:{UNET}")).unwrap();
    assert!(failed.state == StepState::Failed && failed.note.contains("kept failing"));
    let record = state::load(&rig.state_file());
    assert!(record.phase == Phase::Failed && record.error.contains("kept failing") && record.interrupted());
    assert!(rig.server.asked.lock().unwrap().is_empty());
    // The network is back: the same setup finishes.
    rig.net.rule(&rig.manifest, UNET, Net::Serve);
    assert_eq!(rig.run(&["qwen-image-2-1"]).0.unwrap().installed, ["qwen-image-2-1"]);
}

#[test]
fn a_drive_without_the_room_is_found_before_anything_is_downloaded() {
    let rig = Rig::new();
    let (result, _) = rig.run_job(rig.job(rig.choices(&["qwen-image-2-1"])), cramped);
    let e = result.unwrap_err();
    assert!(matches!(&e, Error::Disk(m) if m.contains("Not enough room") && m.contains("1.00 MB is free")), "{e:?}");
    assert_eq!(rig.net.total_served(), 0);
    assert_eq!(rig.phase(), Phase::Failed);
    // A folder that can't be one (a file is in the way) is said plainly too.
    std::fs::write(rig.dir.path().join("blocked"), b"a file").unwrap();
    let choices = Choices { models_dir: rig.dir.path().join("blocked").to_string_lossy().into_owned(), ..rig.choices(&["qwen-image-2-1"]) };
    let e = rig.run_job(rig.job(choices), plenty).0.unwrap_err();
    assert!(matches!(&e, Error::Disk(m) if m.contains("models folder") && m.contains("can't be written")), "{e:?}");
    assert_eq!(rig.net.total_served(), 0);
}

#[test]
fn a_repair_replaces_a_damaged_file_and_keeps_the_good_ones() {
    let rig = Rig::new();
    rig.run(&["qwen-image-2-1"]).0.unwrap();
    let layout = rig.layout();
    // Same length, other bytes: only a checksum can tell.
    let unet = layout.model_file(UNET).unwrap();
    std::fs::write(&unet, vec![0u8; contents(UNET).len()]).unwrap();
    let before = rig.net.total_served();
    let (result, progress) = rig.run_job(rig.job(Choices { recheck: true, ..rig.choices(&["qwen-image-2-1"]) }), plenty);
    assert_eq!(result.unwrap().installed, ["qwen-image-2-1"]);
    assert_eq!(std::fs::read(&unet).unwrap(), contents(UNET));
    assert_eq!(rig.net.total_served() - before, rig.manifest.files[UNET].size, "only the damaged file was fetched");
    assert!(progress.steps.iter().all(|s| s.state == StepState::Done));
    // A file that keeps arriving wrong fails the setup with both checksums.
    std::fs::remove_file(&unet).unwrap();
    rig.net.rule(&rig.manifest, UNET, Net::Corrupt);
    let e = rig.run(&["qwen-image-2-1"]).0.unwrap_err();
    assert!(matches!(&e, Error::Integrity(m) if m.contains(&rig.manifest.files[UNET].sha256)), "{e:?}");
    assert!(!unet.exists() && !part_of(&unet).exists());
}

#[test]
fn an_engine_that_does_not_start_fails_the_setup_with_its_own_words() {
    let server = FakeServer { refuse: Some("ComfyUI stopped while starting: ImportError: DLL load failed".into()), ..FakeServer::default() };
    let rig = Rig::with(true, FakeComfy { running: false, fails: None, prompts: AtomicU64::new(0) }, server);
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let e = result.unwrap_err();
    assert!(matches!(&e, Error::Engine(m) if m.contains("DLL load failed")), "{e:?}");
    let verify = progress.steps.last().unwrap();
    assert!(verify.key == "verify" && verify.state == StepState::Failed);
    // Everything before it is done and stays done: a repair has nothing to download again.
    assert!(progress.steps.iter().rev().skip(1).all(|s| s.state == StepState::Done));
    assert_eq!(rig.phase(), Phase::Failed);
}

/// A render that takes long says how far the engine is, and for how long it has been at it.
#[test]
fn a_rendering_test_image_says_how_far_the_engine_is() {
    let rendering = FakeServer {
        printed: vec!["got prompt".into(), "Requested to load QwenImage".into(), " 35%|###5      | 7/20 [00:41<03:50,  5.20s/it]".into()],
        ..FakeServer::default()
    };
    assert_eq!(render_words("Generating with Qwen Image 2.1", 754, Some(&rendering)), "Generating with Qwen Image 2.1: step 7 of 20, about 03:50 left · 12:34");
    let loading = FakeServer { printed: vec!["got prompt".into(), "Requested to load QwenImage".into()], ..FakeServer::default() };
    assert_eq!(render_words("Generating with X", 5, Some(&loading)), "Generating with X: loading QwenImage into memory · 0:05");
    // A server that prints nothing (or none at all) leaves the clock.
    assert_eq!(render_words("Generating with X", 61, Some(&FakeServer::default())), "Generating with X · 1:01");
    assert_eq!(render_words("Generating with X", 0, None), "Generating with X · 0:00");
    // ComfyUI's own word on the render gets the clock alone; the engine starting has its own.
    assert!(from_render("Rendering with X: step 7 of 20") && from_render("Loading X") && from_render("Finishing the image"));
    assert!(!from_render("Starting ComfyUI: loading PyTorch · 0:12") && !from_render("Sending to ComfyUI") && !from_render("Downloading the image"));
}

/// What a virtual machine said (2026-10-20): PyTorch reached for a graphics card that isn't there.
const NO_CARD_CRASH: &str = "ComfyUI stopped while starting: exit code 0xC0000005, a crash (access violation); it last printed: Windows fatal exception: \
access violation, in File \"C:\\e\\python_embeded\\Lib\\site-packages\\torch\\cuda\\__init__.py\", line 591 in _lazy_init";

#[test]
fn an_engine_that_crashes_reaching_for_a_missing_card_says_how_it_can_run() {
    let refused = |cpu: bool| {
        let server = FakeServer { refuse: Some(NO_CARD_CRASH.into()), ..FakeServer::default() };
        let rig = Rig::with(true, FakeComfy { running: false, fails: None, prompts: AtomicU64::new(0) }, server);
        let e = rig.run_job(rig.job(Choices { cpu, ..rig.choices(&["qwen-image-2-1"]) }), plenty).0.unwrap_err();
        let Error::Engine(why) = e else { panic!("{e:?}") };
        why
    };
    let why = refused(false);
    assert!(why.starts_with(NO_CARD_CRASH) && why.contains("on the processor") && why.contains("Cloud models"), "{why}");
    // On the processor already, a crash is something else: it is told as it is.
    assert_eq!(refused(true), NO_CARD_CRASH);
    // Other ways PyTorch says there is no card.
    assert!(no_card("RuntimeError: Found no NVIDIA driver on your system.") && no_card("AssertionError: Torch not compiled with CUDA enabled"));
    assert!(!no_card("ImportError: DLL load failed") && !no_card("access violation in python312.dll"));
}

/// A stand-in runtime DLL: a version resource saying 14.51.
fn runtime_dll(path: &Path) {
    let mut bytes = b"MZ".to_vec();
    bytes.extend_from_slice(&[0, 0]);
    for word in [0xFEEF_04BDu32, 0x0001_0000, (14 << 16) | 51, 36247 << 16] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// The engine's PyTorch needs msvcp140.dll, which a PC without the Visual C++ Redistributable
/// lacks (Windows Sandbox: WinError 126): the app's copy is put beside the engine's Python.
#[test]
fn the_cpp_runtime_is_put_beside_the_engines_python_and_continue_mends_an_engine_without_it() {
    use crate::vcrt::{DLLS, Sources};
    let rig = Rig::new();
    let bundle = rig.dir.path().join("vc-runtime");
    std::fs::create_dir_all(&bundle).unwrap();
    for name in DLLS {
        runtime_dll(&bundle.join(name));
    }
    let no_system = rig.dir.path().join("System32-without-it");
    *rig.vc_runtime.borrow_mut() = Some(Sources { bundled: Some(bundle), system: Some(no_system.clone()) });
    rig.run(&["qwen-image-2-1"]).0.unwrap();
    let python_dir = rig.layout().python.parent().unwrap().to_path_buf();
    for name in DLLS {
        assert_eq!(crate::vcrt::file_version(&python_dir.join(name)), Some([14, 51, 36247, 0]), "{name}");
    }
    // An engine unpacked before there was this, without the runtime: Continue (or Repair) puts it
    // there, and nothing is downloaded again.
    std::fs::remove_file(python_dir.join("msvcp140.dll")).unwrap();
    let served = rig.net.total_served();
    rig.run(&["qwen-image-2-1"]).0.unwrap();
    assert!(python_dir.join("msvcp140.dll").is_file());
    assert_eq!(rig.net.total_served(), served);
    // Nowhere to have it from: the setup stops at its last row, before starting the engine, and
    // says how to get it.
    std::fs::remove_file(python_dir.join("msvcp140.dll")).unwrap();
    let asked = rig.server.asked.lock().unwrap().len();
    *rig.vc_runtime.borrow_mut() = Some(Sources { bundled: None, system: Some(no_system) });
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let e = result.unwrap_err();
    assert!(matches!(&e, Error::Engine(m) if m.contains("msvcp140.dll") && m.contains(crate::vcrt::REDIST_URL)), "{e:?}");
    let verify = progress.steps.last().unwrap();
    assert!(verify.key == "verify" && verify.state == StepState::Failed && verify.note.contains("Visual C++"), "{verify:?}");
    assert_eq!(rig.server.asked.lock().unwrap().len(), asked, "the engine was not started without it");
}

#[test]
fn without_a_graphics_card_the_engine_runs_on_the_processor_and_renders_no_test_image() {
    let rig = Rig::new();
    let (result, progress) = rig.run_job(rig.job(Choices { cpu: true, ..rig.choices(&["qwen-image-2-1"]) }), plenty);
    let done = result.unwrap();
    // Started as ComfyUI's own run_cpu.bat starts it, and so in the settings it leaves.
    assert!(done.launch.as_ref().is_some_and(|l| l.args.ends_with("--cpu")), "{:?}", done.launch);
    assert!(done.test.is_none() && done.test_error.clone().unwrap().contains("processor"));
    assert_eq!(rig.comfy.prompts.load(Ordering::SeqCst), 0, "no test image on the processor");
    let verify = progress.steps.last().unwrap();
    assert_eq!((verify.state, verify.label.as_str()), (StepState::Done, "Start the engine on the processor"));
    // With a card, the same setup renders its test image.
    let rig = Rig::new();
    let done = rig.run(&["qwen-image-2-1"]).0.unwrap();
    assert!(done.launch.is_some_and(|l| !l.args.contains("--cpu")) && done.test.is_some());
}

#[test]
fn a_test_image_that_does_not_render_is_not_a_failed_install() {
    let rig = Rig::with(true, FakeComfy { running: true, fails: Some(CARD_OUT_OF_MEMORY), prompts: AtomicU64::new(0) }, FakeServer::default());
    let (result, progress) = rig.run(&["qwen-image-2-1"]);
    let done = result.unwrap();
    let why = done.test_error.clone().unwrap();
    assert!(why.contains("installed and running") && why.contains("out of memory") && why.contains("smaller model"), "{why}");
    assert!(done.test.is_none() && done.installed == ["qwen-image-2-1"]);
    assert_eq!(progress.steps.last().map(|s| s.state), Some(StepState::Done));
    assert_eq!(rig.phase(), Phase::Done);
    // Windows' paging file ran out (error 1455): that is system memory, not the card's, and the
    // message says what to do about it.
    let paging = "The paging file is too small for this operation to complete. (os error 1455)";
    let rig = Rig::with(true, FakeComfy { running: true, fails: Some(paging), prompts: AtomicU64::new(0) }, FakeServer::default());
    let why = rig.run(&["qwen-image-2-1"]).0.unwrap().test_error.unwrap();
    assert!(why.contains("os error 1455") && why.contains("Virtual memory") && !why.contains("graphics card"), "{why}");
    // With no model installed there is nothing to render with: the engine alone is still a setup.
    let rig = Rig::new();
    let done = rig.run(&[]).0.unwrap();
    assert!(done.test.is_none() && done.test_error.clone().unwrap().contains("No model was installed"));
    assert!(rig.layout().python.is_file());
    assert_eq!(rig.comfy.prompts.load(Ordering::SeqCst), 0);
}

#[test]
fn an_archive_that_is_not_the_build_leaves_no_engine_and_is_fetched_again() {
    // The right checksum (the manifest pins it), but no Python inside.
    let rig = Rig::with(false, FakeComfy { running: true, fails: None, prompts: AtomicU64::new(0) }, FakeServer::default());
    let (result, progress) = rig.run(&[]);
    let e = result.unwrap_err();
    assert!(matches!(&e, Error::Archive(m) if m.contains("python_embeded/python.exe")), "{e:?}");
    let layout = rig.layout();
    assert!(!layout.engine_root.exists() && !unpack::staging_of(&layout.engine_root).exists() && !layout.archive.exists());
    assert_eq!(progress.steps.iter().find(|s| s.key == "unpack").map(|s| s.state), Some(StepState::Failed));
}

#[test]
fn the_users_own_comfyui_gets_the_missing_files_and_nothing_else() {
    let rig = Rig::new();
    let server_files = BTreeMap::from([("diffusion_models".to_string(), vec!["qwen_image_2.1_int8_convrot.safetensors".to_string()])]);
    let choices = Choices {
        route: Route::Existing,
        engine_dir: String::new(),
        variant: String::new(),
        server_files: Some(server_files),
        ..rig.choices(&["qwen-image-2-1"])
    };
    let mut job = rig.job(choices);
    job.url = "http://127.0.0.1:8000".into();
    job.launch = Some(Launch { python: "C:/comfy/python.exe".into(), folder: "C:/comfy".into(), args: "main.py".into() });
    let (result, progress) = rig.run_job(job, plenty);
    let done = result.unwrap();
    // No engine was installed, and the file their server has was not downloaded again.
    assert!(!rig.dir.path().join("engine").exists());
    assert_eq!(rig.net.served(&rig.manifest, UNET) + rig.net.served(&rig.manifest, "runtime"), 0);
    assert_eq!(rig.net.served(&rig.manifest, ENCODER), rig.manifest.files[ENCODER].size);
    assert_eq!((done.url.as_str(), done.port, done.route), ("http://127.0.0.1:8000", 0, Route::Existing));
    assert_eq!(done.installed, ["qwen-image-2-1"]);
    assert!(done.folders["text_encoders"].contains(&"qwen3vl_8b_fp8_scaled.safetensors".to_string()));
    assert_eq!(progress.steps.iter().map(|s| s.key.as_str()).filter(|k| !k.starts_with("file:")).collect::<Vec<_>>(), ["configure", "verify"]);
    assert!(done.test.is_some());
    // Their launch settings are kept as theirs.
    let mut settings = Settings::default();
    apply(&mut settings, &rig.manifest, &done);
    assert_eq!((settings.comfy_url.as_str(), settings.comfy_launch.folder.as_str(), settings.comfy_autostart), ("http://127.0.0.1:8000", "C:/comfy", true));
}

#[test]
fn a_setup_that_cannot_be_planned_does_not_start() {
    let rig = Rig::new();
    let env = || {
        Arc::new(Env {
            fetch: rig.net.clone(),
            transport: rig.comfy.clone(),
            server: None,
            state_file: None,
            sleep: |_| {},
            free_space: plenty,
            vc_runtime: None,
        })
    };
    for choices in [
        rig.choices(&["no-such-model"]),
        Choices { models_dir: "relative".into(), ..rig.choices(&[]) },
        Choices { variant: "voodoo".into(), ..rig.choices(&[]) },
    ] {
        assert!(matches!(Handle::start(env(), rig.job(choices)), Err(Error::Config(_))));
    }
    assert!(!rig.dir.path().join("engine").exists() && rig.net.total_served() == 0);
    // Cancelled before it gets anywhere: it stops, and says so.
    rig.net.rule(&rig.manifest, "runtime", Net::StopAfter(10));
    let mut handle = Handle::start(env(), rig.job(Choices { test_image: false, ..rig.choices(&[]) })).unwrap();
    assert_eq!(handle.choices.variant, "nvidia");
    handle.cancel();
    let began = Instant::now();
    let result = loop {
        if let Some(result) = handle.take() {
            break result;
        }
        assert!(began.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(result.unwrap_err(), Error::Cancelled);
    assert!(handle.take().is_none(), "the result is handed over once");
}

#[test]
fn progress_reports_speed_only_while_bytes_arrive() {
    let shared = Shared::default();
    let rig = Rig::new();
    let plan = plan::plan(&rig.manifest, &rig.choices(&["qwen-image-2-1"]), &RealDisk).unwrap();
    shared.set_steps(&plan);
    let p = shared.progress();
    let all: u64 = plan.steps.iter().filter(|s| s.step.is_download()).map(|s| s.bytes).sum();
    assert_eq!((p.bytes_done, p.bytes_total, p.speed, p.eta_seconds), (0, all, 0.0, None));
    // Checking a file that is there moves its row, not the speed.
    shared.bytes(2, 10_000, false);
    assert_eq!((shared.progress().bytes_done, shared.progress().speed), (10_000, 0.0));
    shared.bytes(2, 20_000, true);
    std::thread::sleep(Duration::from_millis(300));
    shared.bytes(2, 40_000, true);
    let p = shared.progress();
    assert!(p.speed > 0.0 && p.eta_seconds.is_some(), "{p:?}");
    assert_eq!(p.bytes_done, 40_000);
    // A step that is done counts whole; one that was left out counts for nothing.
    shared.set(2, StepState::Done, "");
    shared.set(3, StepState::Skipped, "needs a Hugging Face sign-in");
    let p = shared.progress();
    assert_eq!(p.bytes_done, plan.steps[2].bytes);
    assert_eq!(p.bytes_total, all - plan.steps[3].bytes);
    // Rows that don't exist are ignored.
    shared.bytes(99, 1, true);
    shared.set(99, StepState::Done, "");
    shared.note(99, "x");
    assert_eq!(shared.state_of(99), None);
}
