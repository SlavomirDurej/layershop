//! Running a setup: the plan's steps on a worker thread, with progress and a way to stop.
//!
//! [`Handle::start`] plans (so what is already on disk is skipped), then downloads two files at a
//! time while the engine is unpacked as soon as its archive is there, writes the engine's model
//! paths, works out the settings and, if asked, starts the engine and renders a test image.
//! Stopping keeps everything that arrived: starting again continues from it.
//!
//! The worker never touches the app's settings. It returns an [`Outcome`], and the engine
//! applies it to the settings as they are then ([`crate::configure`]).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use photocraft_aigen::{Control, Launch, LocalServer, Settings, Transport};
use serde::Serialize;

use crate::download::{self, Spec};
use crate::fetch::Fetch;
use crate::manifest::Manifest;
use crate::plan::{self, Choices, Layout, Plan, Planned, PlannedModel, RealDisk, Route, Step};
use crate::runtime::{self, Marker};
use crate::state::{self, Phase};
use crate::unpack::{self, Limits};
use crate::{Error, Result, configure, human_bytes};

/// What the test image shows. Plain, quick to judge, and nothing a model would decline.
pub const TEST_PROMPT: &str = "A red apple on a wooden table by a window, soft morning light, photograph";
/// The test image's size: one megapixel, which every model here renders well.
const TEST_SIZE: u32 = 1024;
/// A fixed seed, so the same model gives the same test image on every machine.
const TEST_SEED: u64 = 2_026_101_300;

/// What a setup needs from the app.
pub struct Env {
    pub fetch: Arc<dyn Fetch>,
    pub transport: Arc<dyn Transport>,
    /// Starts the engine for the test image. `None`: it must already be running.
    pub server: Option<Arc<dyn LocalServer>>,
    /// Where the setup's record is kept ([`crate::state`]).
    pub state_file: Option<PathBuf>,
    pub sleep: fn(Duration),
    pub free_space: fn(&Path) -> Option<u64>,
}

/// One setup to run.
pub struct Job {
    pub manifest: Manifest,
    pub choices: Choices,
    /// The settings as they are now: what the test image's settings start from.
    pub settings: Settings,
    /// The Hugging Face token, for gated files.
    pub token: Option<String>,
    /// Managed: the port the engine is given (0: the first free one from 8190).
    pub port: u16,
    /// Existing: the server's address, and how it is started when that is known.
    pub url: String,
    pub launch: Option<Launch>,
}

/// The test image, as the engine returned it (PNG).
#[derive(Clone, Debug, PartialEq)]
pub struct TestImage {
    pub bytes: Vec<u8>,
    pub seconds: f64,
    pub model: String,
    pub model_name: String,
    pub width: u32,
    pub height: u32,
}

/// What a finished setup hands the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub route: Route,
    /// Where the engine answers, and how it is started.
    pub url: String,
    pub launch: Option<Launch>,
    pub port: u16,
    /// The model files the engine has, by folder.
    pub folders: BTreeMap<String, Vec<String>>,
    /// The manifest models installed whole.
    pub installed: Vec<String>,
    /// The chosen models that were left out, and why.
    pub skipped: Vec<PlannedModel>,
    pub test: Option<TestImage>,
    /// Why there is no test image, when one was asked for. Not a failed install.
    pub test_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    Pending,
    Active,
    Done,
    Failed,
    Skipped,
}

/// One row of the Install screen.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepProgress {
    pub key: String,
    pub label: String,
    pub state: StepState,
    /// Bytes done and in all (downloads and the unpack; 0 for the rest).
    pub done: u64,
    pub total: u64,
    /// What it is doing, or why it failed or was left out.
    pub note: String,
    pub download: bool,
}

/// A setup's progress, as the UI polls it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub steps: Vec<StepProgress>,
    /// Over all downloads.
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Bytes a second over the last few seconds.
    pub speed: f64,
    pub eta_seconds: Option<u64>,
    pub message: String,
}

/// How long the speed is averaged over.
const WINDOW: Duration = Duration::from_secs(8);

#[derive(Default)]
struct Inner {
    steps: Vec<StepProgress>,
    /// Bytes that came over the network, and when: what the speed is read from.
    arrived: u64,
    samples: VecDeque<(Instant, u64)>,
    message: String,
    result: Option<Result<Outcome>>,
}

#[derive(Default)]
struct Shared {
    cancel: AtomicBool,
    finished: AtomicBool,
    inner: Mutex<Inner>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn set_steps(&self, plan: &Plan) {
        let steps = plan
            .steps
            .iter()
            .map(|s| StepProgress {
                key: s.step.key(),
                label: s.label.clone(),
                state: if s.done {
                    StepState::Done
                } else if s.skipped.is_some() {
                    StepState::Skipped
                } else {
                    StepState::Pending
                },
                done: if s.done { s.bytes } else { s.have },
                total: s.bytes,
                note: s.skipped.clone().unwrap_or_default(),
                download: s.step.is_download(),
            })
            .collect();
        self.lock().steps = steps;
    }

    fn set(&self, index: usize, state: StepState, note: &str) {
        let mut inner = self.lock();
        if let Some(step) = inner.steps.get_mut(index) {
            step.state = state;
            step.note = note.to_string();
            if state == StepState::Done {
                step.done = step.total;
            }
        }
    }

    fn note(&self, index: usize, note: &str) {
        if let Some(step) = self.lock().steps.get_mut(index) {
            step.note = note.to_string();
        }
    }

    fn state_of(&self, index: usize) -> Option<StepState> {
        self.lock().steps.get(index).map(|s| s.state)
    }

    /// A step has `done` of its bytes. With `network`, the growth counts towards the speed.
    fn bytes(&self, index: usize, done: u64, network: bool) {
        let mut inner = self.lock();
        let Some(step) = inner.steps.get_mut(index) else { return };
        let grew = done.saturating_sub(step.done);
        step.done = done.min(step.total.max(done));
        if network && grew > 0 {
            inner.arrived = inner.arrived.saturating_add(grew);
            let now = Instant::now();
            if inner.samples.back().is_none_or(|(at, _)| now.duration_since(*at) >= Duration::from_millis(250)) {
                let arrived = inner.arrived;
                inner.samples.push_back((now, arrived));
                while inner.samples.front().is_some_and(|(at, _)| now.duration_since(*at) > WINDOW) && inner.samples.len() > 2 {
                    inner.samples.pop_front();
                }
            }
        }
    }

    fn message(&self, text: &str) {
        let mut inner = self.lock();
        if inner.message != text {
            text.clone_into(&mut inner.message);
        }
    }

    fn progress(&self) -> Progress {
        let inner = self.lock();
        let downloads = || inner.steps.iter().filter(|s| s.download && s.state != StepState::Skipped);
        let bytes_total = downloads().fold(0u64, |sum, s| sum.saturating_add(s.total));
        let bytes_done = downloads().fold(0u64, |sum, s| sum.saturating_add(s.done.min(s.total)));
        let speed = match (inner.samples.front(), inner.samples.back()) {
            // A window that ended a while ago is no speed: nothing is arriving now.
            (Some((t0, b0)), Some((t1, b1))) if t1 > t0 && t1.elapsed() < Duration::from_secs(5) => {
                b1.saturating_sub(*b0) as f64 / t1.duration_since(*t0).as_secs_f64()
            }
            _ => 0.0,
        };
        let left = bytes_total.saturating_sub(bytes_done);
        let eta_seconds = (speed > 1.0 && left > 0).then(|| (left as f64 / speed).min(86_400.0 * 30.0) as u64);
        Progress { steps: inner.steps.clone(), bytes_done, bytes_total, speed, eta_seconds, message: inner.message.clone() }
    }
}

/// A setup running on its worker thread.
pub struct Handle {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// What it was started with.
    pub choices: Choices,
}

impl Handle {
    /// Start `job` on a worker thread.
    pub fn start(env: Arc<Env>, job: Job) -> Result<Handle> {
        // Planned once here, so a setup that can't be planned says so at once and the rows are
        // there for the first poll.
        let first = plan::plan(&job.manifest, &job.choices, &RealDisk)?;
        let shared = Arc::new(Shared::default());
        shared.set_steps(&first);
        let choices = job.choices.clone();
        let worker = shared.clone();
        // On disk before anything else is: an app closed in the middle finds a setup to continue.
        record(&env, &job, Phase::Installing, None, "");
        let thread = std::thread::Builder::new()
            .name("photocraft-ai-setup".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&env, &job, &worker)))
                    .unwrap_or_else(|_| Err(Error::Engine("The setup stopped with an internal error. What arrived is kept: continue to try again.".into())));
                match &result {
                    Ok(done) => record(&env, &job, Phase::Done, Some(done), ""),
                    Err(Error::Cancelled) => record(&env, &job, Phase::Paused, None, ""),
                    Err(e) => record(&env, &job, Phase::Failed, None, &e.to_string()),
                }
                worker.lock().result = Some(result);
                worker.finished.store(true, Ordering::Release);
            })
            .map_err(|e| Error::Engine(format!("could not start the setup: {e}")))?;
        Ok(Handle { shared, thread: Some(thread), choices })
    }

    pub fn progress(&self) -> Progress {
        self.shared.progress()
    }

    /// Stop. What arrived is kept; the next start continues from it.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    /// The result, once: `None` while it runs, and after it has been taken.
    pub fn take(&mut self) -> Option<Result<Outcome>> {
        if !self.finished() {
            return None;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.shared.lock().result.take()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // The app is closing, or the session is gone: stop writing to disk.
        self.cancel();
    }
}

/// Note in the setup's record how far it got. The record keeps what else it holds (that the
/// wizard was offered); failing to write it is not a reason to fail the setup.
fn record(env: &Env, job: &Job, phase: Phase, done: Option<&Outcome>, error: &str) {
    let Some(path) = &env.state_file else { return };
    let mut state = state::load(path);
    state.phase = phase;
    state.choices = job.choices.clone();
    state.comfy_version = job.manifest.comfy.version.clone();
    state.error = error.to_string();
    if let Some(done) = done {
        state.installed = done.installed.clone();
        state.port = done.port;
    }
    let _ = state::save(path, &state);
}

/// The drive a path is on, where the system says (Windows); elsewhere every path is taken to
/// share one, which only ever asks for more room than needed.
fn drive_of(path: &Path) -> String {
    match path.components().next() {
        Some(std::path::Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().to_ascii_uppercase(),
        _ => String::new(),
    }
}

/// A folder the setup writes to: there, and writable.
fn writable(dir: &Path, what: &str) -> Result<()> {
    let refuse = |e: std::io::Error| Error::Disk(format!("The {what} folder `{}` can't be written to ({e}). Choose a folder you can write to.", dir.display()));
    std::fs::create_dir_all(dir).map_err(refuse)?;
    let probe = dir.join(".layershop-write-test");
    std::fs::write(&probe, b"ok").map_err(refuse)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// What can be known to fail before anything is downloaded: a path too long, a folder that
/// can't be written to, a drive without the room.
pub fn preflight(job: &Job, plan: &Plan, layout: &Layout, free_space: fn(&Path) -> Option<u64>) -> Result<()> {
    let managed = job.choices.route == Route::Managed;
    if managed {
        if let Some(why) = plan::path_too_long(job.manifest.asset(&job.choices.variant)?, layout) {
            return Err(Error::Config(why));
        }
        writable(&layout.engine_dir, "engine")?;
    }
    if plan.models_bytes > 0 || managed {
        writable(&layout.models_dir, "models")?;
    }
    let room = |dir: &Path, needed: u64, what: &str| match free_space(dir) {
        Some(free) if needed > 0 && free < needed.saturating_add(plan::HEADROOM) => {
            let drive = drive_of(dir);
            let place = if drive.is_empty() { format!("the drive of `{}`", dir.display()) } else { format!("drive {drive}") };
            Err(Error::Disk(format!(
                "Not enough room on {place}: {} is free, and {what} needs {} (with {} to spare). Free some space, choose fewer models, or choose another folder.",
                human_bytes(free),
                human_bytes(needed),
                human_bytes(plan::HEADROOM)
            )))
        }
        _ => Ok(()),
    };
    let engine = if managed { plan.engine_bytes } else { 0 };
    if managed && drive_of(&layout.engine_dir) == drive_of(&layout.models_dir) {
        room(&layout.models_dir, engine.saturating_add(plan.models_bytes), "the engine and the models")
    } else {
        if managed {
            room(&layout.engine_dir, engine, "the engine")?;
        }
        room(&layout.models_dir, plan.models_bytes, "the models")
    }
}

/// Bring one file down (the runtime archive, or a model file).
fn fetch_step(env: &Env, job: &Job, layout: &Layout, shared: &Shared, index: usize, planned: &Planned, stop: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    let (url, size, sha256, dest, token) = match &planned.step {
        Step::FetchRuntime => {
            let asset = job.manifest.asset(&job.choices.variant)?;
            (asset.url.as_str(), asset.size, asset.sha256.as_str(), layout.archive.clone(), None)
        }
        Step::FetchFile(key) => {
            let file = job.manifest.files.get(key).ok_or_else(|| Error::Config(format!("`{key}` is not a model file")))?;
            // The token is only for the files that need it.
            (file.url.as_str(), file.size, file.sha256.as_str(), layout.model_file(key)?, job.token.as_deref().filter(|_| file.gated))
        }
        _ => return Ok(()),
    };
    shared.set(index, StepState::Active, "");
    if planned.recheck {
        shared.note(index, "Checking");
        if download::verify(&dest, size, sha256, stop, &|n| shared.bytes(index, n, false))? {
            return Ok(());
        }
        // Not the file it should be: it is replaced.
        shared.bytes(index, 0, false);
        shared.note(index, "Damaged: downloading again");
    }
    let ctx = download::Ctx { fetch: env.fetch.as_ref(), cancelled: stop, sleep: &env.sleep, free_space: &env.free_space };
    // What arrived before is read again first (its checksum continues from there): on a large
    // file that takes a moment, in which the row would otherwise look stuck.
    let resumed = planned.have;
    if resumed > 0 {
        shared.note(index, "Checking what arrived");
    }
    download::download(&ctx, &Spec { url, size, sha256, token }, &dest, &|n| {
        if resumed > 0 && n > resumed {
            shared.note(index, "");
        }
        shared.bytes(index, n, true);
    })
}

/// Unpack the runtime archive into the engine folder, then delete the archive.
fn unpack_step(job: &Job, layout: &Layout, shared: &Shared, index: usize, stop: &dyn Fn() -> bool) -> Result<()> {
    shared.set(index, StepState::Active, "");
    let asset = job.manifest.asset(&job.choices.variant)?;
    let marker = Marker { version: job.manifest.comfy.version.clone(), variant: job.choices.variant.clone(), sha256: asset.sha256.clone() };
    let python = job.manifest.comfy.python.split('/').fold(PathBuf::new(), |p, part| p.join(part));
    let finish = |staging: &Path| {
        // An archive that unpacks without the Python it is started with is not an engine.
        if !staging.join(&python).is_file() {
            return Err(Error::Archive(format!(
                "The engine archive has no `{}`: it is not the build this version of the app installs.",
                job.manifest.comfy.python
            )));
        }
        runtime::write_marker(staging, &marker)
    };
    let done =
        unpack::unpack(&layout.archive, &layout.engine_root, &Limits::default(), stop, &|done, total| shared.bytes(index, done.min(total), false), &finish);
    match done {
        Ok(_) => {
            // 2 GB the engine no longer needs.
            let _ = std::fs::remove_file(&layout.archive);
            Ok(())
        }
        Err(Error::Archive(why)) => {
            // Its checksum was right when it arrived: damaged since, or not what was expected.
            // Either way the next try starts from a fresh download.
            let _ = std::fs::remove_file(&layout.archive);
            Err(Error::Archive(why))
        }
        Err(e) => Err(e),
    }
}

fn index_of(plan: &Plan, step: &Step) -> Option<usize> {
    plan.steps.iter().position(|s| s.step == *step)
}

fn run(env: &Env, job: &Job, shared: &Shared) -> Result<Outcome> {
    let plan = plan::plan(&job.manifest, &job.choices, &RealDisk)?;
    let layout = Layout::new(&job.manifest, &job.choices)?;
    shared.set_steps(&plan);
    let managed = job.choices.route == Route::Managed;
    shared.message("Checking the folders");
    preflight(job, &plan, &layout, env.free_space)?;

    // ----- downloads, two at a time; the engine is unpacked as soon as its archive is there -----
    let downloads: Vec<(usize, &Planned)> = plan.steps.iter().enumerate().filter(|(_, s)| s.step.is_download() && !s.done && s.skipped.is_none()).collect();
    let next = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let stop = || shared.cancelled() || abort.load(Ordering::Relaxed);
    let fail = |index: usize, e: Error| {
        shared.set(index, StepState::Failed, &e.to_string());
        failure.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(e);
        abort.store(true, Ordering::Relaxed);
    };
    let stopped = || {
        failure.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(Error::Cancelled);
        abort.store(true, Ordering::Relaxed);
    };
    if !downloads.is_empty() {
        shared.message("Downloading");
    }
    std::thread::scope(|scope| {
        let worker = || {
            while let Some((index, planned)) = downloads.get(next.fetch_add(1, Ordering::SeqCst)).copied() {
                if stop() {
                    break;
                }
                match fetch_step(env, job, &layout, shared, index, planned, &stop) {
                    Ok(()) => shared.set(index, StepState::Done, ""),
                    // A gated file without access: its models are left out, the rest carries on.
                    Err(Error::Access(why)) => shared.set(index, StepState::Skipped, &why),
                    // Stopped: nothing after this is to run on half a download.
                    Err(Error::Cancelled) => {
                        shared.set(index, StepState::Pending, "");
                        stopped();
                        break;
                    }
                    Err(e) => {
                        fail(index, e);
                        break;
                    }
                }
            }
        };
        let spawned = [scope.spawn(worker), scope.spawn(worker)];
        if let (true, Some(unpack_index)) = (managed, index_of(&plan, &Step::UnpackRuntime).filter(|i| plan.steps.get(*i).is_some_and(|s| !s.done))) {
            // Wait for the archive (it may be there already), then unpack beside the downloads.
            let fetched = index_of(&plan, &Step::FetchRuntime);
            let ready = loop {
                match fetched.and_then(|i| shared.state_of(i)) {
                    Some(StepState::Done) | None => break true,
                    Some(StepState::Failed | StepState::Skipped) => break false,
                    _ if stop() => break false,
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
            };
            if ready && !stop() {
                match unpack_step(job, &layout, shared, unpack_index, &stop) {
                    Ok(()) => shared.set(unpack_index, StepState::Done, ""),
                    Err(Error::Cancelled) => {
                        shared.set(unpack_index, StepState::Pending, "");
                        stopped();
                    }
                    Err(e) => fail(unpack_index, e),
                }
            }
        }
        for handle in spawned {
            let _ = handle.join();
        }
    });
    // A cancel wins over whatever else went wrong while stopping.
    if shared.cancelled() {
        return Err(Error::Cancelled);
    }
    if let Some(e) = failure.lock().unwrap_or_else(PoisonError::into_inner).take() {
        return Err(e);
    }

    // ----- where the engine finds the models -----
    let (url, launch, port) = if managed {
        if let Some(i) = index_of(&plan, &Step::WriteModelPaths) {
            shared.set(i, StepState::Active, "");
            runtime::write_model_paths(&layout)?;
            shared.set(i, StepState::Done, "");
        }
        let port = if job.port != 0 {
            job.port
        } else {
            runtime::free_port(runtime::FIRST_PORT)
                .ok_or_else(|| Error::Engine("No free port was found for the engine (8190 to 8389 are all taken).".into()))?
        };
        (runtime::url(port), Some(runtime::launch(&job.manifest, &layout)), port)
    } else {
        (job.url.clone(), job.launch.clone(), 0)
    };

    // ----- the settings -----
    let configure_index = index_of(&plan, &Step::Configure);
    if let Some(i) = configure_index {
        shared.set(i, StepState::Active, "");
    }
    shared.message("Setting up the models");
    let mut folders = if managed { runtime::list_models(&layout.models_dir) } else { job.choices.server_files.clone().unwrap_or_default() };
    if !managed {
        // What was just downloaded into the server's models folder is there too.
        for (folder, names) in runtime::list_models(&layout.models_dir) {
            let have = folders.entry(folder).or_default();
            for name in names {
                if !have.iter().any(|f| f.rsplit(['/', '\\']).next() == Some(name.as_str())) {
                    have.push(name);
                }
            }
        }
    }
    let mut settings = job.settings.clone();
    let configured = match (&launch, managed) {
        (Some(launch), true) => configure::managed(&mut settings, &job.manifest, &url, launch, &folders),
        _ => configure::existing(&mut settings, &job.manifest, &url, launch.as_ref(), &folders),
    };
    let installed: Vec<String> = configure::installed(&job.manifest, &folders).into_iter().filter(|id| job.choices.models.contains(id)).collect();
    // A chosen model that is not whole was left out: say why (the plan's reason, or the step's).
    let skipped = plan
        .models
        .iter()
        .filter(|m| !installed.contains(&m.id))
        .map(|m| {
            let why = m.skipped.clone().or_else(|| {
                let keys = job.manifest.model(&m.id).map(|e| e.files.clone()).unwrap_or_default();
                let progress = shared.progress();
                progress.steps.iter().find(|s| s.state == StepState::Skipped && keys.iter().any(|k| s.key == format!("file:{k}"))).map(|s| s.note.clone())
            });
            PlannedModel { id: m.id.clone(), name: m.name.clone(), skipped: Some(why.unwrap_or_else(|| "one of its files is missing".into())) }
        })
        .collect();
    if let Some(i) = configure_index {
        shared.set(i, StepState::Done, "");
    }

    // ----- the engine, and the test image -----
    let mut outcome = Outcome { route: job.choices.route, url, launch, port, folders, installed, skipped, test: None, test_error: None };
    if let Some(i) = index_of(&plan, &Step::Verify) {
        shared.set(i, StepState::Active, "Starting the engine");
        shared.message("Starting the engine");
        let cancelled = || shared.cancelled();
        let progress = |_: f32, text: &str| {
            shared.note(i, text.trim_end_matches('…'));
            shared.message(text.trim_end_matches('…'));
        };
        let sleep = |d: Duration| (env.sleep)(d);
        let control = Control { cancelled: &cancelled, progress: &progress, sleep: &sleep };
        let from_engine = |e: photocraft_aigen::Error| match e {
            photocraft_aigen::Error::Cancelled => Error::Cancelled,
            e => Error::Engine(e.to_string()),
        };
        let started = match &env.server {
            Some(server) => server.ensure(env.transport.as_ref(), &settings, &control).map_err(from_engine),
            None if photocraft_aigen::launch::is_running(env.transport.as_ref(), &outcome.url) => Ok(()),
            None => Err(Error::Engine(format!("ComfyUI is not answering at {}.", outcome.url))),
        };
        if let Err(e) = started {
            if e != Error::Cancelled {
                shared.set(i, StepState::Failed, &e.to_string());
            }
            return Err(e);
        }
        match configure::test_model(&settings, &configured.ready).filter(|m| outcome.installed.contains(&m.id) || !managed) {
            Some(model) => {
                shared.note(i, &format!("Rendering a test image with {}", model.name));
                let job =
                    photocraft_aigen::Job { prompt: TEST_PROMPT.into(), width: TEST_SIZE, height: TEST_SIZE, seed: TEST_SEED, alpha: false, source: None };
                let began = Instant::now();
                match photocraft_aigen::generate(env.transport.as_ref(), &settings, model, "", &job, &control) {
                    Ok(image) => {
                        outcome.test = Some(TestImage {
                            bytes: image.bytes,
                            seconds: began.elapsed().as_secs_f64(),
                            model: model.id.clone(),
                            model_name: model.name.clone(),
                            width: image.width,
                            height: image.height,
                        });
                        shared.set(i, StepState::Done, "");
                    }
                    Err(photocraft_aigen::Error::Cancelled) => return Err(Error::Cancelled),
                    // The engine runs; the picture did not come. That is not a failed install.
                    Err(e) => {
                        let why = e.to_string();
                        let lower = why.to_ascii_lowercase();
                        let hint = if lower.contains("out of memory") || lower.contains("allocation") {
                            " The model may be too large for this graphics card: try a smaller model, or a lower render size in its settings."
                        } else {
                            ""
                        };
                        outcome.test_error = Some(format!("The engine is installed and running, but the test image did not render: {why}{hint}"));
                        shared.set(i, StepState::Done, "The test image did not render");
                    }
                }
            }
            None => {
                outcome.test_error =
                    Some("The engine is installed and running. No model was installed whole, so there was nothing to render a test image with.".into());
                shared.set(i, StepState::Done, "No model to test with");
            }
        }
    }
    shared.message("Done");
    Ok(outcome)
}

/// The settings a finished setup leaves: `outcome` applied to the settings as they are now.
pub fn apply(settings: &mut Settings, manifest: &Manifest, outcome: &Outcome) -> configure::Configured {
    match (&outcome.launch, outcome.route) {
        (Some(launch), Route::Managed) => configure::managed(settings, manifest, &outcome.url, launch, &outcome.folders),
        _ => configure::existing(settings, manifest, &outcome.url, outcome.launch.as_ref(), &outcome.folders),
    }
}

#[cfg(test)]
mod tests;
