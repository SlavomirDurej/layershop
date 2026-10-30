//! Running a generation: queue, wait, download.

use std::collections::BTreeMap;
use std::time::Duration;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::comfy::{self, History};
use crate::http::{Request, Transport};
use crate::live::Live;
use crate::model::{Model, Provider, Settings};
use crate::size::render_size;
use crate::{Error, Result, cloud};

/// What to generate.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub prompt: String,
    /// The size of the area to fill, in pixels. Each back end derives its own render size.
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    /// Ask for a transparent background (models with [`Model::alpha`]).
    pub alpha: bool,
    /// Inpainting: the image the area holds now and the part of it to repaint. `None` generates
    /// from the prompt alone.
    pub source: Option<Source>,
}

/// The image an inpainting starts from, already at the size the model is asked to render
/// ([`source_size`]), so the answer lines up with it pixel for pixel.
#[derive(Clone, PartialEq)]
pub struct Source {
    /// The area as it is now: an opaque PNG, `width` × `height`.
    pub image: Vec<u8>,
    /// What to repaint: a PNG of the same size, transparent where the model paints and opaque
    /// where the image stays (the convention of OpenAI's masks and of ComfyUI's Load Image).
    /// Empty: image to image, the whole picture is redrawn.
    pub mask: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// How far the repainted part may depart from what is there, 0..=1: 1 paints it afresh,
    /// less keeps more of its shapes and colours. Local models; cloud models have no such control.
    pub strength: f64,
    /// Edit: the prompt is an instruction about the image, which an edit model
    /// ([`Model::can_edit`]) is shown whole. The strength does not apply. A local model may be
    /// given a `mask` too: it then paints only there, and what is outside comes back as it was.
    pub edit: bool,
    /// Edit: further pictures the instruction may refer to (PNG, any size), in the order the
    /// model numbers them after the image ([`Model::max_references`] at most).
    pub references: Vec<Vec<u8>>,
    /// Masked edit: the picture the model is shown as image 1 when it isn't `image` (PNG, any
    /// size), while `image` is still what it paints over. An outpainting shows it the picture
    /// alone, without the empty margins it is to fill: an edit model keeps what it is shown
    /// there, so margins of white, or of the picture's colours blurred, come back as they went.
    /// Empty: `image`.
    pub shown: Vec<u8>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("image_len", &self.image.len())
            .field("mask_len", &self.mask.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("strength", &self.strength)
            .field("edit", &self.edit)
            .field("references", &self.references.len())
            .field("shown_len", &self.shown.len())
            .finish()
    }
}

impl Source {
    /// The strength as a denoise fraction a sampler accepts.
    pub(crate) fn denoise(&self) -> f64 {
        if self.strength.is_finite() { self.strength.clamp(0.05, 1.0) } else { 1.0 }
    }
}

/// The size `model` renders a `width` × `height` area at: what a [`Source`] for it must measure.
pub fn source_size(model: &Model, width: u32, height: u32) -> (u32, u32) {
    match model.provider {
        Provider::Comfy => render_size(width, height, model.family.source_megapixels(model.megapixels), model.family.size_multiple()),
        Provider::Openai => cloud::openai_size(model, width, height),
        // These answer in the shape of the image they are given: send about what they render.
        Provider::Gemini | Provider::Xai | Provider::Meta | Provider::Bfl => {
            render_size(width, height, if model.image_size.trim() == "1K" { 1.0 } else { 4.0 }, 8)
        }
    }
}

/// An encoded image (PNG, JPEG or WebP, as the back end returned it).
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub bytes: Vec<u8>,
    /// The seed used (local models), for "render this one again".
    pub seed: u64,
    /// The size the model was asked to render.
    pub width: u32,
    pub height: u32,
    /// What the request cost in US dollars, when the provider reported the tokens it billed and
    /// their rates are known (see [`crate::pricing`]).
    pub cost: Option<f64>,
}

/// The caller's side of a running generation.
pub struct Control<'a> {
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Progress in 0..=1 with a short status.
    pub progress: &'a (dyn Fn(f32, &str) + Sync),
    /// Wait between polls. The app passes `std::thread::sleep`; tests pass nothing.
    pub sleep: &'a (dyn Fn(Duration) + Sync),
}

/// How often ComfyUI's history is asked whether the image is ready.
const POLL: Duration = Duration::from_millis(500);
/// Polls that may fail in a row before giving up (the server stalls while it loads a model).
const MAX_POLL_FAILURES: u32 = 30;

/// A seed for a model set to "random": 48 bits, which every back end and JSON number holds.
pub fn random_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    #[cfg(not(target_arch = "wasm32"))]
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    #[cfg(target_arch = "wasm32")]
    let now = 0u64;
    // SplitMix64 over the clock and a counter, so two calls in the same tick differ.
    let mut z = now ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) & 0xFFFF_FFFF_FFFF
}

/// Render `job` with `model`. Blocks until the image arrives, the back end fails, the timeout in
/// `settings` passes, or `control` reports a cancel. `key` is the provider's API key (unused for
/// local models).
pub fn generate(transport: &dyn Transport, settings: &Settings, model: &Model, key: &str, job: &Job, control: &Control<'_>) -> Result<Generated> {
    if job.prompt.trim().is_empty() {
        return Err(Error::Config("write a prompt first".into()));
    }
    if (control.cancelled)() {
        return Err(Error::Cancelled);
    }
    let timeout = u64::from(settings.timeout_seconds.clamp(10, 3600));
    match model.provider {
        Provider::Comfy => generate_comfy(transport, settings, model, job, control, timeout),
        Provider::Bfl => generate_bfl(transport, model, key, job, control, timeout),
        Provider::Openai | Provider::Gemini | Provider::Xai | Provider::Meta => {
            let who = model.provider.label();
            // An edit comes back in the shape of the picture it was given.
            let given = job.source.as_ref().map(|source| (source.width, source.height));
            let (request, size) = match (&job.source, model.provider) {
                (Some(source), Provider::Openai) => (cloud::openai_edit_request(model, job, source, key, timeout)?, (source.width, source.height)),
                (None, Provider::Openai) => (cloud::openai_request(model, job, key, timeout)?, cloud::openai_size(model, job.width, job.height)),
                (_, Provider::Xai) => (cloud::xai_request(model, job, key, timeout)?, given.unwrap_or((job.width, job.height))),
                (_, Provider::Meta) => (cloud::meta_request(model, job, key, timeout)?, given.unwrap_or_else(|| cloud::meta_size(job.width, job.height))),
                (_, _) => (cloud::gemini_request(model, job, key, timeout)?, given.unwrap_or((job.width, job.height))),
            };
            (control.progress)(0.1, &format!("Waiting for {who}…"));
            let response = transport.send(&request).map_err(|e| Error::Network(format!("Could not reach {who}: {e}")))?;
            if (control.cancelled)() {
                return Err(Error::Cancelled);
            }
            let (bytes, cost) = match model.provider {
                Provider::Openai => {
                    let (bytes, usage) = cloud::parse_openai_with_usage(response.status, &response.body)?;
                    (bytes, usage.and_then(|u| crate::pricing::billed(&model.api_model, &u)))
                }
                // xAI's answer says what it cost.
                Provider::Xai => {
                    let (bytes, usage) = cloud::parse_images(Provider::Xai, response.status, &response.body)?;
                    (bytes, usage.as_ref().and_then(cloud::xai_cost))
                }
                // Meta bills Muse Image a flat price for each image it returns.
                Provider::Meta => {
                    let (bytes, _) = cloud::parse_images(Provider::Meta, response.status, &response.body)?;
                    (bytes, model.api_model.trim().starts_with("muse-image").then_some(crate::pricing::MUSE_IMAGE_USD))
                }
                _ => (cloud::parse_gemini(response.status, &response.body)?, None),
            };
            Ok(Generated { bytes, seed: job.seed, width: size.0, height: size.1, cost })
        }
    }
}

/// How often Black Forest Labs is asked whether the image is ready.
const BFL_POLL: Duration = Duration::from_millis(1500);

/// Black Forest Labs: submit the job, ask for it until it is ready, download the image. The key
/// goes to its API only: not to the address the image is downloaded from.
fn generate_bfl(transport: &dyn Transport, model: &Model, key: &str, job: &Job, control: &Control<'_>, timeout: u64) -> Result<Generated> {
    let who = model.provider.label();
    let unreachable = |e: Error| Error::Network(format!("Could not reach {who}: {e}"));
    let request = cloud::bfl_request(model, job, key, timeout)?;
    let (width, height) = job.source.as_ref().map_or((job.width, job.height), |source| (source.width, source.height));
    (control.progress)(0.05, &format!("Sending to {who}…"));
    let submitted = transport.send(&request).map_err(unreachable)?;
    let (polling_url, cost) = cloud::parse_bfl_submitted(submitted.status, &submitted.body)?;
    let poll = cloud::bfl_poll_request(&polling_url, key)?;
    let label = if model.name.is_empty() { who } else { model.name.as_str() };
    let polls = (timeout * 1000 / BFL_POLL.as_millis().max(1) as u64).max(1);
    let mut failures = 0u32;
    for n in 0..polls {
        if (control.cancelled)() {
            return Err(Error::Cancelled);
        }
        (control.sleep)(BFL_POLL);
        let state = match transport.send(&poll) {
            Ok(r) => cloud::parse_bfl_job(r.status, &r.body)?,
            Err(_) => {
                failures += 1;
                if failures > MAX_POLL_FAILURES {
                    return Err(Error::Network(format!("Lost the connection to {who} while it was generating")));
                }
                continue;
            }
        };
        failures = 0;
        match state {
            cloud::BflJob::Working(status) => {
                // No step count: creep towards done so the bar moves.
                (control.progress)(0.1 + 0.85 * (1.0 - 1.0 / (1.0 + n as f32 / 12.0)), &format!("Generating with {label}… ({status})"));
            }
            cloud::BflJob::Ready(sample) => {
                (control.progress)(0.97, "Downloading the image…");
                let r =
                    transport.send(&Request::get(sample).timeout(120)).map_err(|e| Error::Network(format!("Could not download the image from {who}: {e}")))?;
                if !r.ok() || r.body.is_empty() {
                    return Err(Error::Server(format!("{who} could not serve the generated image (HTTP {})", r.status)));
                }
                return Ok(Generated { bytes: r.body, seed: job.seed, width, height, cost });
            }
        }
    }
    Err(Error::Server(format!("{who} did not finish within {timeout} seconds (raise the timeout in Preferences › Generative AI)")))
}

fn generate_comfy(transport: &dyn Transport, settings: &Settings, model: &Model, job: &Job, control: &Control<'_>, timeout: u64) -> Result<Generated> {
    let base = comfy::base_url(&settings.comfy_url)?;
    let unreachable = |e: Error| Error::Network(format!("Could not reach ComfyUI at {base}. Is it running? ({e})"));
    // An inpainting renders at the size of the image it was given.
    let (width, height) = match &job.source {
        Some(source) => (source.width, source.height),
        None => model.family.render_size(job.width, job.height, model.megapixels),
    };
    let mut render = Job { width, height, source: None, ..job.clone() };
    // A layout prompt's boxes are in the selection's pixels: put them where the render has them.
    if model.family == crate::model::Family::MossImage && job.source.is_none() {
        render.prompt = comfy::moss_scaled(&job.prompt, (job.width, job.height), (width, height));
    }
    let workflow = match &job.source {
        Some(source) if source.edit => {
            comfy::check_edit(model)?;
            if source.references.len() > model.max_references() {
                return Err(Error::Config(format!("{} takes {} reference pictures at most", model.name, model.max_references())));
            }
            (control.progress)(0.01, "Sending the picture to ComfyUI…");
            // The picture, its mask when it has one, what the model is shown of it, then the
            // references.
            let masked = !source.mask.is_empty();
            let shown = masked && !source.shown.is_empty();
            let mut names = Vec::with_capacity(3 + source.references.len());
            let uploads = std::iter::once((comfy::UPLOAD_IMAGE.to_string(), &source.image))
                .chain(masked.then(|| (comfy::UPLOAD_MASK.to_string(), &source.mask)))
                .chain(shown.then(|| (comfy::UPLOAD_SHOWN.to_string(), &source.shown)))
                .chain(source.references.iter().enumerate().map(|(i, bytes)| (comfy::upload_reference(i), bytes)));
            for (name, bytes) in uploads {
                let r = transport.send(&comfy::upload_request(&base, &name, bytes)).map_err(unreachable)?;
                names.push(comfy::parse_uploaded(r.status, &r.body)?);
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
            }
            let taken = |i: usize, sent: bool| names.get(i).filter(|_| sent).map(String::as_str);
            let Some(image) = taken(0, true) else { return Err(Error::BadResponse("ComfyUI did not take the picture".into())) };
            let mask = taken(1, masked);
            let shown = taken(1 + usize::from(masked), shown);
            let references = names.get(1 + usize::from(masked) + usize::from(shown.is_some())..).unwrap_or_default();
            comfy::edit_workflow(model, &render, &comfy::Edit { image, mask, shown, references })?
        }
        Some(source) => {
            // The set-up is checked before anything is uploaded.
            let masked = !source.mask.is_empty();
            if masked {
                comfy::check(model, true)?
            } else {
                comfy::check_img2img(model)?
            }
            (control.progress)(0.01, "Sending the image to ComfyUI…");
            let mut names = Vec::with_capacity(2);
            for (name, bytes) in [(comfy::UPLOAD_IMAGE, &source.image), (comfy::UPLOAD_MASK, &source.mask)] {
                if bytes.is_empty() {
                    continue;
                }
                let r = transport.send(&comfy::upload_request(&base, name, bytes)).map_err(unreachable)?;
                names.push(comfy::parse_uploaded(r.status, &r.body)?);
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
            }
            let Some(image) = names.first() else { return Err(Error::BadResponse("ComfyUI did not take the image".into())) };
            let mask = names.get(1).map(String::as_str).filter(|_| masked);
            comfy::inpaint_workflow(model, &render, &comfy::Inpaint { image, mask, denoise: source.denoise() })?
        }
        None => comfy::workflow(model, &render)?,
    };
    // A name of this render's own: ComfyUI tells the client that queued a render what it is doing
    // with it, over its websocket, which is listened to beside the polls.
    let client = format!("photocraft-{:012x}", random_seed());
    let listening = Listening { live: Mutex::new(Live::new(&workflow)), state: AtomicU8::new(CONNECTING), stop: AtomicBool::new(false) };
    let ws = base.strip_prefix("http://").map(|host| format!("ws://{host}/ws?clientId={client}"));
    with_listener(transport, ws.as_deref(), &listening, || {
        // Its first word is its queue's state: wait for it (briefly), so none about this render is missed.
        (control.progress)(0.02, "Sending to ComfyUI…");
        for _ in 0..40 {
            if listening.state.load(Ordering::Relaxed) != CONNECTING || (control.cancelled)() {
                break;
            }
            (control.sleep)(Duration::from_millis(50));
        }
        poll_comfy(transport, model, job, control, timeout, &Queued { base: &base, client: &client, workflow: &workflow, size: (width, height) }, &listening)
    })
}

/// The websocket's side of a render: what it has heard, and whether it is open.
struct Listening {
    live: Mutex<Live>,
    state: AtomicU8,
    stop: AtomicBool,
}

const CONNECTING: u8 = 0;
const OPEN: u8 = 1;
const CLOSED: u8 = 2;

/// Run `render` with `ws` listened to on a thread of its own (when there is an address to listen
/// at, and threads), until `render` returns.
fn with_listener<R>(transport: &dyn Transport, ws: Option<&str>, listening: &Listening, render: impl FnOnce() -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(url) = ws {
        return std::thread::scope(|scope| {
            let listener = std::thread::Builder::new().name("comfyui-live".into()).spawn_scoped(scope, || {
                let mut heard = |text: &str| {
                    listening.state.store(OPEN, Ordering::Relaxed);
                    listening.live.lock().unwrap_or_else(PoisonError::into_inner).take(text);
                };
                let _ = transport.listen(url, &|| listening.stop.load(Ordering::Relaxed), &mut heard);
                listening.state.store(CLOSED, Ordering::Relaxed);
            });
            if listener.is_err() {
                listening.state.store(CLOSED, Ordering::Relaxed);
            }
            // The listener is told to stop however the render ends: the scope waits for it.
            let _stop = StopOnDrop(&listening.stop);
            render()
        });
    }
    let _ = (transport, ws);
    listening.state.store(CLOSED, Ordering::Relaxed);
    render()
}

/// Sets its flag when dropped.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// What is queued: where, for which client, the workflow, and the size it renders.
struct Queued<'a> {
    base: &'a str,
    client: &'a str,
    workflow: &'a serde_json::Value,
    size: (u32, u32),
}

/// Queue the workflow, and follow it until its image is downloaded.
fn poll_comfy(
    transport: &dyn Transport,
    model: &Model,
    job: &Job,
    control: &Control<'_>,
    timeout: u64,
    q: &Queued<'_>,
    listening: &Listening,
) -> Result<Generated> {
    let (base, (width, height)) = (q.base, q.size);
    let unreachable = |e: Error| Error::Network(format!("Could not reach ComfyUI at {base}. Is it running? ({e})"));
    let queued = transport.send(&Request::post_json(format!("{base}/prompt"), comfy::queue_body(q.workflow, q.client))).map_err(unreachable)?;
    let id = comfy::parse_queued(queued.status, &queued.body).map_err(|e| too_old(model, e))?;
    let label = if model.name.is_empty() { model.family.label() } else { model.name.as_str() };
    let message = format!("Generating with {label}…");
    let polls = (timeout * 1000 / POLL.as_millis().max(1) as u64).max(1);
    let mut failures = 0u32;
    for n in 0..polls {
        if (control.cancelled)() {
            // Best effort: stop the render and drop it from the queue.
            let _ = transport.send(&Request::post_json(format!("{base}/interrupt"), b"{}".to_vec()).timeout(5));
            let _ = transport.send(&Request::post_json(format!("{base}/queue"), serde_json::json!({"delete": [id]}).to_string().into_bytes()).timeout(5));
            return Err(Error::Cancelled);
        }
        // What ComfyUI says it is doing; without its word, creep towards done so the bar moves.
        let heard = listening.live.lock().unwrap_or_else(PoisonError::into_inner).words(label);
        match heard {
            Some((fraction, words)) => (control.progress)(fraction, &words),
            None if listening.state.load(Ordering::Relaxed) == OPEN => (control.progress)(0.03, "Waiting for ComfyUI…"),
            None => (control.progress)(0.05 + 0.9 * (1.0 - 1.0 / (1.0 + n as f32 / 40.0)), &message),
        }
        (control.sleep)(POLL);
        let history = match transport.send(&Request::get(format!("{base}/history/{id}")).timeout(20)) {
            Ok(r) if r.ok() => comfy::parse_history(&r.body, &id)?,
            Ok(_) | Err(_) => {
                failures += 1;
                if failures > MAX_POLL_FAILURES {
                    return Err(Error::Network(format!("Lost the connection to ComfyUI at {base} while it was generating")));
                }
                continue;
            }
        };
        failures = 0;
        if let History::Done(image) = history {
            (control.progress)(0.97, "Downloading the image…");
            let r = transport
                .send(&Request::get(comfy::view_url(base, &image)).timeout(120))
                .map_err(|e| Error::Network(format!("Could not download the image from ComfyUI: {e}")))?;
            if !r.ok() || r.body.is_empty() {
                return Err(Error::Server(format!("ComfyUI could not serve the generated image (HTTP {})", r.status)));
            }
            return Ok(Generated { bytes: r.body, seed: job.seed, width, height, cost: None });
        }
    }
    let _ = transport.send(&Request::post_json(format!("{base}/interrupt"), b"{}".to_vec()).timeout(5));
    Err(Error::Server(format!("ComfyUI did not finish within {timeout} seconds (raise the timeout in Preferences › Generative AI)")))
}

/// `e`, ComfyUI's refusal of `model`'s workflow, with what to do when the server is older than
/// the model: MOSS Image came to ComfyUI with its 0.42.0 release and FIBO 2 with 0.43.0, and an
/// older server rejects their text encoder type (and FIBO 2's prompt node).
fn too_old(model: &Model, e: Error) -> Error {
    use crate::model::Family;
    // The engine the Local AI setup installed is updated by running the setup again.
    const UPDATE: &str = "update ComfyUI (for the one File › Set Up Local AI installed, run that again: it installs the newer engine)";
    match e {
        Error::Server(msg) if model.family == Family::MossImage && msg.contains("moss_image") => {
            Error::Server(format!("{msg}. This ComfyUI predates MOSS Image, which it supports since its 0.42.0 release: {UPDATE}"))
        }
        Error::Server(msg) if model.family == Family::Fibo2 && msg.to_ascii_lowercase().contains("fibo2") => {
            Error::Server(format!("{msg}. This ComfyUI predates FIBO 2, which it supports since its 0.43.0 release: {UPDATE}"))
        }
        e => e,
    }
}

/// The model files a ComfyUI server offers, by folder (see [`comfy::FOLDERS`]), for the file
/// pickers in the settings. A folder the server doesn't know is left out.
pub fn comfy_models(transport: &dyn Transport, comfy_url: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let base = comfy::base_url(comfy_url)?;
    let mut out = BTreeMap::new();
    for (i, folder) in comfy::FOLDERS.iter().enumerate() {
        match transport.send(&Request::get(format!("{base}/models/{folder}")).timeout(10)) {
            Ok(r) if r.ok() => {
                out.insert((*folder).to_string(), comfy::parse_models(&r.body)?);
            }
            // The first request tells whether there is a server at all.
            Err(e) if i == 0 => return Err(Error::Network(format!("Could not reach ComfyUI at {base}. Is it running? ({e})"))),
            Ok(r) if i == 0 => return Err(Error::Server(format!("{base} answered HTTP {}; is it a ComfyUI server?", r.status))),
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Method, Response};
    use crate::model::Family;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    type Answer = Box<dyn Fn(&Request, usize) -> Result<Response> + Send + Sync>;

    /// Answers each request with a closure (given the request and how many came before it), and
    /// logs what was asked.
    struct Fake {
        log: Mutex<Vec<(Method, String)>>,
        answer: Answer,
    }

    impl Fake {
        fn new(answer: impl Fn(&Request, usize) -> Result<Response> + Send + Sync + 'static) -> Self {
            Self { log: Mutex::new(Vec::new()), answer: Box::new(answer) }
        }
        fn urls(&self) -> Vec<String> {
            self.log.lock().unwrap().iter().map(|(_, u)| u.clone()).collect()
        }
    }

    impl Transport for Fake {
        fn send(&self, request: &Request) -> Result<Response> {
            let n = {
                let mut log = self.log.lock().unwrap();
                log.push((request.method, request.url.clone()));
                log.len() - 1
            };
            (self.answer)(request, n)
        }
    }

    fn ok(body: &str) -> Result<Response> {
        Ok(Response { status: 200, body: body.as_bytes().to_vec() })
    }

    fn control<'a>(cancelled: &'a (dyn Fn() -> bool + Sync)) -> Control<'a> {
        Control { cancelled, progress: &|_, _| {}, sleep: &|_| {} }
    }

    fn settings() -> Settings {
        Settings { comfy_url: "http://comfy:8188/".into(), timeout_seconds: 10, ..Settings::default() }
    }

    fn job() -> Job {
        Job { prompt: "a frog".into(), width: 500, height: 500, seed: 7, alpha: false, source: None }
    }

    const DONE: &str =
        r#"{"abc":{"outputs":{"9":{"images":[{"filename":"f.png","subfolder":"","type":"temp"}]}},"status":{"status_str":"success","completed":true}}}"#;

    #[test]
    fn comfy_queues_polls_and_downloads() {
        let fake = Fake::new(|r, n| match (r.url.as_str(), n) {
            (u, _) if u.ends_with("/prompt") => {
                let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                assert!(body["client_id"].as_str().unwrap().starts_with("photocraft-"), "{body}");
                assert!(body["prompt"].to_string().contains("a frog"));
                ok(r#"{"prompt_id":"abc"}"#)
            }
            (u, n) if u.ends_with("/history/abc") => ok(if n < 3 { "{}" } else { DONE }),
            (u, _) if u.contains("/view?filename=f.png") => ok("PNGDATA"),
            (u, _) => panic!("unexpected request {u}"),
        });
        let progress = AtomicU32::new(0);
        let c = Control {
            cancelled: &|| false,
            progress: &|_, _| {
                progress.fetch_add(1, Ordering::Relaxed);
            },
            sleep: &|_| {},
        };
        let g = generate(&fake, &settings(), &Family::ZImage.preset(), "", &job(), &c).unwrap();
        assert_eq!(g.bytes, b"PNGDATA");
        assert_eq!((g.width, g.height, g.seed), (1008, 1008, 7));
        assert_eq!(fake.urls().first().map(String::as_str), Some("http://comfy:8188/prompt"));
        assert!(progress.load(Ordering::Relaxed) >= 3);
    }

    /// A ComfyUI that says over its websocket what it does with the render (`messages`), and has
    /// the image ready once it has said all of it.
    struct Talking {
        messages: Vec<String>,
        said: AtomicBool,
        asked: Mutex<String>,
    }

    impl Transport for Talking {
        fn send(&self, r: &Request) -> Result<Response> {
            match r.url.as_str() {
                u if u.ends_with("/prompt") => {
                    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    // The render is queued for the client that listens.
                    assert!(self.asked.lock().unwrap().ends_with(body["client_id"].as_str().unwrap()), "{body}");
                    ok(r#"{"prompt_id":"abc"}"#)
                }
                u if u.ends_with("/history/abc") => ok(if self.said.load(Ordering::Relaxed) { DONE } else { "{}" }),
                u if u.contains("/view?") => ok("PNGDATA"),
                u => panic!("unexpected request {u}"),
            }
        }

        fn listen(&self, url: &str, stop: &dyn Fn() -> bool, on_text: &mut dyn FnMut(&str)) -> Result<()> {
            *self.asked.lock().unwrap() = url.to_string();
            on_text(r#"{"type":"status","data":{}}"#);
            // Each message once the render has had time to report the last (polls come every 40 ms).
            for message in &self.messages {
                std::thread::sleep(Duration::from_millis(150));
                on_text(message);
            }
            std::thread::sleep(Duration::from_millis(150));
            self.said.store(true, Ordering::Relaxed);
            while !stop() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }
    }

    #[test]
    fn a_local_render_says_what_comfyui_is_doing() {
        let model = Family::ZImage.preset();
        // The nodes as the workflow numbers them.
        let workflow = comfy::workflow(&model, &Job { width: 1008, height: 1008, ..job() }).unwrap();
        let id_of = |class: &str| workflow.as_object().unwrap().iter().find(|(_, n)| n["class_type"] == class).map(|(id, _)| id.clone()).unwrap();
        let executing = |class: &str| format!(r#"{{"type":"executing","data":{{"node":"{}"}}}}"#, id_of(class));
        let messages = vec![
            r#"{"type":"execution_start","data":{}}"#.to_string(),
            executing("UNETLoader"),
            executing("CLIPTextEncode"),
            executing("KSampler"),
            r#"{"type":"progress","data":{"value":4,"max":8}}"#.into(),
            executing("VAEDecode"),
        ];
        let talking = Talking { messages, said: AtomicBool::new(false), asked: Mutex::new(String::new()) };
        let said = Mutex::new(Vec::<String>::new());
        let c = Control {
            cancelled: &|| false,
            progress: &|_, words| {
                let mut said = said.lock().unwrap();
                if said.last().is_none_or(|last| last != words) {
                    said.push(words.to_string());
                }
            },
            // Polls of 40 ms: several for each message.
            sleep: &|d| std::thread::sleep(d.min(Duration::from_millis(40))),
        };
        let g = generate(&talking, &Settings { timeout_seconds: 60, ..settings() }, &model, "", &job(), &c).unwrap();
        assert_eq!(g.bytes, b"PNGDATA");
        assert!(talking.asked.lock().unwrap().starts_with("ws://comfy:8188/ws?clientId=photocraft-"));
        let said = said.into_inner().unwrap();
        let name = &model.name;
        for want in [
            "Sending to ComfyUI…".to_string(),
            format!("Loading {name}…"),
            "Reading the prompt…".into(),
            format!("Rendering with {name}: step 4 of 8"),
            "Finishing the image…".into(),
            "Downloading the image…".into(),
        ] {
            assert!(said.contains(&want), "{want:?} not in {said:?}");
        }
        assert!(!said.iter().any(|s| s.starts_with("Generating with")), "{said:?}");
        // Without live updates (a transport without them, or an https address), it creeps as before.
        let quiet = Fake::new(|r, n| match r.url.as_str() {
            u if u.ends_with("/prompt") => ok(r#"{"prompt_id":"abc"}"#),
            u if u.ends_with("/history/abc") => ok(if n < 3 { "{}" } else { DONE }),
            _ => ok("PNGDATA"),
        });
        let words = Mutex::new(Vec::<String>::new());
        let c = Control { cancelled: &|| false, progress: &|_, w| words.lock().unwrap().push(w.to_string()), sleep: &|_| {} };
        generate(&quiet, &settings(), &model, "", &job(), &c).unwrap();
        assert!(words.lock().unwrap().iter().any(|w| w == &format!("Generating with {name}…")), "{:?}", words.lock().unwrap());
    }

    /// A real render on the ComfyUI at `PHOTOCRAFT_COMFY_URL` (default ComfyUI Desktop's
    /// address), printing what the dialog would say and when: `cargo test -p photocraft-aigen
    /// --lib a_real_render -- --ignored --nocapture`. Z-Image Turbo, or MOSS Image with
    /// `PHOTOCRAFT_LIVE_MODEL=moss`; the model's files must be there.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "renders on a running ComfyUI"]
    fn a_real_render_says_each_step() {
        let url = std::env::var("PHOTOCRAFT_COMFY_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".into());
        let began = std::time::Instant::now();
        let last = Mutex::new(String::new());
        let c = Control {
            cancelled: &|| false,
            progress: &|fraction, words| {
                let mut last = last.lock().unwrap();
                if *last != words {
                    println!("{:>6.2} s  {:>3.0}%  {words}", began.elapsed().as_secs_f64(), fraction * 100.0);
                    *last = words.to_string();
                }
            },
            sleep: &|d| std::thread::sleep(d),
        };
        let settings = Settings { comfy_url: url, timeout_seconds: 600, ..Settings::default() };
        let job = Job { prompt: "a lighthouse on a cliff at dusk".into(), width: 768, height: 768, seed: random_seed(), alpha: false, source: None };
        let family = if std::env::var("PHOTOCRAFT_LIVE_MODEL").is_ok_and(|m| m == "moss") { Family::MossImage } else { Family::ZImage };
        let g = generate(&crate::UreqTransport::new(), &settings, &family.preset(), "", &job, &c).unwrap();
        println!("{:>6.2} s  done: {} bytes, {}x{}", began.elapsed().as_secs_f64(), g.bytes.len(), g.width, g.height);
        assert!(!g.bytes.is_empty());
    }

    fn source() -> Source {
        Source { image: b"IMAGEPNG".to_vec(), mask: b"MASKPNG".to_vec(), width: 768, height: 512, strength: 0.7, edit: false, references: Vec::new(), shown: Vec::new() }
    }

    #[test]
    fn editing_uploads_the_picture_and_its_references_then_queues_an_edit_workflow() {
        let fake = Fake::new(|r, n| match r.url.as_str() {
            u if u.ends_with("/upload/image") => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                // The picture first, then the references in order, each under its own name.
                let want = ["photocraft_inpaint_image.png", "photocraft_edit_reference_1.png", "photocraft_edit_reference_2.png"][n];
                assert!(body.contains(want) && body.contains(["IMAGEPNG", "SHIRT", "HAT"][n]), "{n}: {body}");
                ok(&format!(r#"{{"name":"stored {n}.png","subfolder":""}}"#))
            }
            u if u.ends_with("/prompt") => {
                let text = serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["prompt"].to_string();
                for want in ["TextEncodeQwenImage21", "stored 0.png", "stored 1.png", "stored 2.png", "a frog"] {
                    assert!(text.contains(want), "{want} missing in {text}");
                }
                assert!(!text.contains("SetLatentNoiseMask") && !text.contains("MASKPNG"), "{text}");
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let edit = Source { mask: Vec::new(), edit: true, references: vec![b"SHIRT".to_vec(), b"HAT".to_vec()], ..source() };
        let job = Job { source: Some(edit.clone()), ..job() };
        let g = generate(&fake, &settings(), &Family::QwenImage21.preset(), "", &job, &control(&|| false)).unwrap();
        // Rendered at the size of the picture it was given.
        assert_eq!((g.bytes.as_slice(), g.width, g.height), (&b"PNGDATA"[..], 768, 512));
        assert_eq!(fake.urls().iter().filter(|u| u.ends_with("/upload/image")).count(), 3);
        // A model that can't edit, and one given more references than it takes: refused before
        // anything is sent.
        let idle = Fake::new(|_, _| panic!("no request expected"));
        let e = generate(&idle, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap_err();
        assert!(matches!(&e, Error::Config(s) if s.contains("can't edit")), "{e}");
        let many = Job { source: Some(Source { references: vec![Vec::new(); 3], ..edit.clone() }), ..job.clone() };
        assert!(matches!(generate(&idle, &settings(), &Family::QwenEdit.preset(), "", &many, &control(&|| false)), Err(Error::Config(_))));
        // Cancelled between the uploads: nothing is queued.
        let n = AtomicU32::new(0);
        let uploads = Fake::new(|_, _| ok(r#"{"name":"a.png"}"#));
        let e = generate(&uploads, &settings(), &Family::Flux2.preset(), "", &job, &control(&|| n.fetch_add(1, Ordering::Relaxed) >= 2)).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert!(!uploads.urls().iter().any(|u| u.ends_with("/prompt")));
    }

    #[test]
    fn a_masked_edit_uploads_its_mask_after_the_picture() {
        let fake = Fake::new(|r, n| match r.url.as_str() {
            u if u.ends_with("/upload/image") => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                let want = ["photocraft_inpaint_image.png", "photocraft_inpaint_mask.png", "photocraft_edit_reference_1.png"][n];
                assert!(body.contains(want) && body.contains(["IMAGEPNG", "MASKPNG", "SHIRT"][n]), "{n}: {body}");
                ok(&format!(r#"{{"name":"stored {n}.png","subfolder":""}}"#))
            }
            u if u.ends_with("/prompt") => {
                let w = serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["prompt"].clone();
                let nodes = w.as_object().unwrap();
                let masked = nodes.values().find(|n| n["class_type"] == "SetLatentNoiseMask").unwrap();
                assert_eq!(nodes[masked["inputs"]["mask"][0].as_str().unwrap()]["inputs"]["image"], "stored 1.png");
                assert!(w.to_string().contains("stored 2.png"), "{w}");
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let edit = Source { edit: true, references: vec![b"SHIRT".to_vec()], ..source() };
        let job = Job { source: Some(edit), ..job() };
        generate(&fake, &settings(), &Family::QwenImage21.preset(), "", &job, &control(&|| false)).unwrap();
        assert_eq!(fake.urls().iter().filter(|u| u.ends_with("/upload/image")).count(), 3);
    }

    #[test]
    fn a_masked_edit_uploads_what_the_model_is_shown_before_the_references() {
        const NAMES: [&str; 4] = ["photocraft_inpaint_image.png", "photocraft_inpaint_mask.png", "photocraft_edit_shown.png", "photocraft_edit_reference_1.png"];
        let fake = Fake::new(|r, n| match r.url.as_str() {
            u if u.ends_with("/upload/image") => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                assert!(body.contains(NAMES[n]) && body.contains(["IMAGEPNG", "MASKPNG", "SHOWN", "SHIRT"][n]), "{n}: {body}");
                ok(&format!(r#"{{"name":"stored {n}.png","subfolder":""}}"#))
            }
            u if u.ends_with("/prompt") => {
                let w = serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["prompt"].clone();
                let nodes = w.as_object().unwrap();
                let encode = nodes.values().find(|n| n["class_type"] == "TextEncodeQwenImage21").unwrap();
                let loaded = |i: u32| nodes[encode["inputs"][format!("images.image_{i}")][0].as_str().unwrap()]["inputs"]["image"].clone();
                assert_eq!((loaded(1), loaded(2)), ("stored 2.png".into(), "stored 3.png".into()), "{w}");
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let edit = Source { edit: true, references: vec![b"SHIRT".to_vec()], shown: b"SHOWN".to_vec(), ..source() };
        generate(&fake, &settings(), &Family::QwenImage21.preset(), "", &Job { source: Some(edit.clone()), ..job() }, &control(&|| false)).unwrap();
        assert_eq!(fake.urls().iter().filter(|u| u.ends_with("/upload/image")).count(), 4);
        // Without a mask it isn't sent: there is nothing to paint over.
        let idle = Fake::new(|r, n| match r.url.as_str() {
            u if u.ends_with("/upload/image") => {
                assert!(!String::from_utf8_lossy(&r.body).contains("SHOWN"), "{n}");
                ok(&format!(r#"{{"name":"stored {n}.png","subfolder":""}}"#))
            }
            u if u.ends_with("/prompt") => ok(r#"{"prompt_id":"abc"}"#),
            u if u.ends_with("/history/abc") => ok(DONE),
            _ => ok("PNGDATA"),
        });
        let unmasked = Source { mask: Vec::new(), ..edit };
        generate(&idle, &settings(), &Family::QwenImage21.preset(), "", &Job { source: Some(unmasked), ..job() }, &control(&|| false)).unwrap();
        assert_eq!(idle.urls().iter().filter(|u| u.ends_with("/upload/image")).count(), 2);
    }

    #[test]
    fn cloud_editing_sends_the_references_after_the_picture() {
        let edit = Source { mask: Vec::new(), edit: true, references: vec![b"SHIRT".to_vec()], ..source() };
        let job = Job { source: Some(edit), alpha: true, ..job() };
        let fake = Fake::new(|r, _| {
            assert_eq!(r.url, "https://api.openai.com/v1/images/edits");
            let body = String::from_utf8_lossy(&r.body).into_owned();
            let (picture, reference) = (body.find("IMAGEPNG").unwrap(), body.find("SHIRT").unwrap());
            assert!(picture < reference && body.matches("name=\"image[]\"").count() == 2 && !body.contains("name=\"mask\""), "{body}");
            assert!(body.contains("name=\"background\"\r\n\r\ntransparent"), "{body}");
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#)
        });
        let m = Model::cloud(Provider::Openai, "GPT Image 2.5", "gpt-image-2.5-sunburst", true);
        assert_eq!(generate(&fake, &settings(), &m, "sk-x", &job, &control(&|| false)).unwrap().bytes, b"hello");
        let gem = Fake::new(|r, _| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            let parts = body["contents"][0]["parts"].as_array().unwrap();
            assert_eq!((parts.len(), parts[0]["text"].as_str()), (3, Some("a frog")));
            assert!(parts[1]["inlineData"]["data"] != parts[2]["inlineData"]["data"]);
            ok(r#"{"candidates":[{"content":{"parts":[{"inlineData":{"data":"aGVsbG8="}}]}}]}"#)
        });
        let m = Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false);
        assert_eq!(generate(&gem, &settings(), &m, "k", &job, &control(&|| false)).unwrap().bytes, b"hello");
    }

    #[test]
    fn inpainting_uploads_the_image_and_mask_then_queues_a_workflow_that_loads_them() {
        let fake = Fake::new(|r, _| match r.url.as_str() {
            u if u.ends_with("/upload/image") => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                // The server may store the file under another name: that is the one to load.
                ok(if body.contains("IMAGEPNG") {
                    r#"{"name":"stored image.png","subfolder":"","type":"input"}"#
                } else {
                    r#"{"name":"m.png","subfolder":"pc"}"#
                })
            }
            u if u.ends_with("/prompt") => {
                let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                let text = body["prompt"].to_string();
                for want in ["stored image.png", "pc/m.png", "SetLatentNoiseMask", "\"denoise\":0.7", "a frog"] {
                    assert!(text.contains(want), "{want} missing in {text}");
                }
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?filename=f.png") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let job = Job { source: Some(source()), ..job() };
        let g = generate(&fake, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap();
        // Rendered at the size of the image it was given, not one derived from the area.
        assert_eq!((g.bytes.as_slice(), g.width, g.height), (&b"PNGDATA"[..], 768, 512));
        let urls = fake.urls();
        assert_eq!(urls.iter().filter(|u| u.ends_with("/upload/image")).count(), 2);
        assert!(urls.iter().position(|u| u.ends_with("/prompt")) > urls.iter().rposition(|u| u.ends_with("/upload/image")), "{urls:?}");

        // A set-up that can't run is refused before anything is uploaded.
        let idle = Fake::new(|_, _| panic!("no request expected"));
        let mut broken = Family::ZImage.preset();
        broken.vae.clear();
        assert!(matches!(generate(&idle, &settings(), &broken, "", &job, &control(&|| false)), Err(Error::Config(_))));
        // A server that refuses the upload, or is not there, says so.
        let full = Fake::new(|_, _| Ok(Response { status: 507, body: b"disk full".to_vec() }));
        let e = generate(&full, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("HTTP 507") && e.contains("disk full"), "{e}");
        let down = Fake::new(|_, _| Err(Error::Network("refused".into())));
        assert!(generate(&down, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap_err().to_string().contains("Is it running?"));
        // Cancelled between the uploads: nothing is queued.
        let n = AtomicU32::new(0);
        let uploads = Fake::new(|_, _| ok(r#"{"name":"a.png"}"#));
        let e = generate(&uploads, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| n.fetch_add(1, Ordering::Relaxed) >= 1)).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert!(!uploads.urls().iter().any(|u| u.ends_with("/prompt")));
    }

    #[test]
    fn image_to_image_uploads_the_picture_alone() {
        let fake = Fake::new(|r, _| match r.url.as_str() {
            u if u.ends_with("/upload/image") => ok(r#"{"name":"picture.png","subfolder":""}"#),
            u if u.ends_with("/prompt") => {
                let text = serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["prompt"].to_string();
                assert!(text.contains("picture.png") && text.contains("\"denoise\":0.7") && !text.contains("SetLatentNoiseMask"), "{text}");
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let job = Job { source: Some(Source { mask: Vec::new(), ..source() }), ..job() };
        let g = generate(&fake, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap();
        assert_eq!((g.width, g.height), (768, 512));
        assert_eq!(fake.urls().iter().filter(|u| u.ends_with("/upload/image")).count(), 1);
        // A model made for inpainting can't: refused before anything is sent.
        let idle = Fake::new(|_, _| panic!("no request expected"));
        assert!(matches!(generate(&idle, &settings(), &Family::Flux1Fill.preset(), "", &job, &control(&|| false)), Err(Error::Config(_))));
    }

    #[test]
    fn cloud_inpainting_sends_the_image_and_reports_its_size() {
        let fake = Fake::new(|r, _| {
            assert_eq!(r.url, "https://api.openai.com/v1/images/edits");
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#)
        });
        let m = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        let job = Job { source: Some(source()), ..job() };
        let g = generate(&fake, &settings(), &m, "sk-x", &job, &control(&|| false)).unwrap();
        assert_eq!((g.bytes.as_slice(), g.width, g.height), (&b"hello"[..], 768, 512));
        let gem = Fake::new(|r, _| {
            assert!(String::from_utf8_lossy(&r.body).contains("inlineData"));
            ok(r#"{"candidates":[{"content":{"parts":[{"inlineData":{"data":"aGVsbG8="}}]}}]}"#)
        });
        let m = Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false);
        assert_eq!(generate(&gem, &settings(), &m, "k", &job, &control(&|| false)).unwrap().bytes, b"hello");
    }

    #[test]
    fn moss_image_renders_the_selections_own_size_and_its_layout_follows() {
        let moss = Family::MossImage.preset();
        // Its own size, to a multiple of 16, from a quarter to four megapixels; outside, the nearest end.
        assert_eq!(moss.family.render_size(1200, 500, moss.megapixels), (1200, 496));
        assert_eq!(moss.family.render_size(1920, 1080, moss.megapixels), (1920, 1088));
        let (w, h) = moss.family.render_size(300, 200, moss.megapixels);
        assert!((f64::from(w * h) / 1e6 - 0.25).abs() < 0.01 && (f64::from(w) / f64::from(h) - 1.5).abs() < 0.05, "{w}x{h}");
        let (w, h) = moss.family.render_size(6000, 4000, moss.megapixels);
        assert!((f64::from(w * h) / 1e6 - 4.0).abs() < 0.05, "{w}x{h}");
        // A size the user sets is theirs.
        assert_eq!(moss.family.render_size(1200, 500, 1.0), render_size(1200, 500, 1.0, 16));
        // A layout typed for a 300 x 200 selection has its boxes where the render puts them.
        let layout = r#"{"summary":{"caption":"a poster"},"items":[{"label":"text","text":"HI","bbox":[-50,-100,50,100]}]}"#;
        let fake = Fake::new(|r, _| match r.url.as_str() {
            u if u.ends_with("/prompt") => {
                let w = serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()["prompt"].clone();
                let text =
                    w.as_object().unwrap().values().find(|n| n["inputs"]["text"].as_str().is_some_and(|t| t.starts_with('{'))).unwrap()["inputs"]["text"]
                        .clone();
                let sent: serde_json::Value = serde_json::from_str(text.as_str().unwrap()).unwrap();
                let latent = w.as_object().unwrap().values().find(|n| n["class_type"] == "EmptyFlux2LatentImage").unwrap()["inputs"].clone();
                let (lw, lh) = (latent["width"].as_f64().unwrap(), latent["height"].as_f64().unwrap());
                let bbox = &sent["items"][0]["bbox"];
                assert_eq!((bbox[1].as_f64().unwrap(), bbox[3].as_f64().unwrap()), ((-100.0 * lw / 300.0).round(), (100.0 * lw / 300.0).round()), "{sent}");
                assert_eq!(bbox[2].as_f64().unwrap(), (50.0 * lh / 200.0).round(), "{sent}");
                ok(r#"{"prompt_id":"abc"}"#)
            }
            u if u.ends_with("/history/abc") => ok(DONE),
            u if u.contains("/view?") => ok("PNGDATA"),
            u => panic!("unexpected request {u}"),
        });
        let job = Job { prompt: layout.into(), width: 300, height: 200, ..job() };
        let g = generate(&fake, &settings(), &moss, "", &job, &control(&|| false)).unwrap();
        assert!(g.width * g.height >= 240_000 && g.width > g.height, "{}x{}", g.width, g.height);
        // A server older than the model says so, and what to do.
        let old = Fake::new(|r, _| {
            assert!(r.url.ends_with("/prompt"));
            ok(
                r#"{"error":{"message":"Prompt outputs failed validation"},"node_errors":{"2":{"class_type":"CLIPLoader","errors":[{"message":"Value not in list","details":"type: 'moss_image' not in ['stable_diffusion']"}]}}}"#,
            )
        });
        let e = generate(&old, &settings(), &moss, "", &job, &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("Value not in list") && e.contains("update ComfyUI"), "{e}");
        // FIBO 2's prompt node, which a server before 0.43.0 doesn't have.
        let older = Fake::new(|r, _| {
            assert!(r.url.ends_with("/prompt"));
            ok(r#"{"error":{"type":"invalid_prompt","message":"Cannot execute because node TextEncodeFibo2 does not exist."},"node_errors":{}}"#)
        });
        let e = generate(&older, &settings(), &Family::Fibo2.preset(), "", &job, &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("TextEncodeFibo2 does not exist") && e.contains("0.43.0") && e.contains("update ComfyUI") && e.contains("Set Up Local AI"), "{e}");
        // Another model's refusal is left as it is.
        let e = generate(&old, &settings(), &Family::ZImage.preset(), "", &job, &control(&|| false)).unwrap_err().to_string();
        assert!(!e.contains("update ComfyUI"), "{e}");
    }

    #[test]
    fn the_source_of_an_inpainting_is_sized_as_each_back_end_renders() {
        let z = Family::ZImage.preset();
        assert_eq!(source_size(&z, 500, 500), render_size(500, 500, z.megapixels, 16));
        assert_eq!(source_size(&Family::QwenImage21.preset(), 500, 500).0 % 32, 0);
        // Ming Image generates at two megapixels, and is given pictures at two when raised to four.
        let ming = Family::MingImage.preset();
        assert_eq!((source_size(&ming, 500, 500), render_size(500, 500, ming.megapixels, 16)), ((1408, 1408), (1408, 1408)));
        let raised = Model { megapixels: 4.0, ..ming };
        assert_eq!((source_size(&raised, 500, 500), render_size(500, 500, raised.megapixels, 16)), ((1408, 1408), (2000, 2000)));
        let gpt = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        assert_eq!(source_size(&gpt, 1600, 900), cloud::openai_size(&gpt, 1600, 900));
        let nb = Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false);
        let (w, h) = source_size(&nb, 1600, 900);
        assert!((f64::from(w) / f64::from(h) - 16.0 / 9.0).abs() < 0.03 && w * h <= 1_100_000, "{w}x{h}");
        for (w, h) in [(0, 0), (1, u32::MAX), (u32::MAX, u32::MAX)] {
            for m in [&z, &gpt, &nb] {
                let (sw, sh) = source_size(m, w, h);
                assert!((256..=4096).contains(&sw) && (256..=4096).contains(&sh), "{w}x{h} -> {sw}x{sh}");
            }
        }
        // The strength is a denoise fraction whatever it was given.
        for (given, want) in [(0.7, 0.7), (0.0, 0.05), (9.0, 1.0), (f64::NAN, 1.0), (-1.0, 0.05)] {
            assert_eq!(Source { strength: given, ..source() }.denoise(), want);
        }
        assert!(!format!("{:?}", source()).contains("IMAGEPNG"));
    }

    #[test]
    fn comfy_errors_name_the_cause() {
        let down = Fake::new(|_, _| Err(Error::Network("connection refused".into())));
        let e = generate(&down, &settings(), &Family::ZImage.preset(), "", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("Is it running?") && e.contains("http://comfy:8188"), "{e}");

        let never = Fake::new(|r, _| if r.url.ends_with("/prompt") { ok(r#"{"prompt_id":"abc"}"#) } else { ok("{}") });
        let e = generate(&never, &settings(), &Family::ZImage.preset(), "", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("did not finish within 10 seconds"), "{e}");
        assert!(never.urls().last().is_some_and(|u| u.ends_with("/interrupt")));

        let lost = Fake::new(|r, _| if r.url.ends_with("/prompt") { ok(r#"{"prompt_id":"abc"}"#) } else { Err(Error::Network("reset".into())) });
        let mut long = settings();
        long.timeout_seconds = 3600;
        let e = generate(&lost, &long, &Family::ZImage.preset(), "", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("Lost the connection"), "{e}");

        let e = generate(&down, &settings(), &Family::ZImage.preset(), "", &Job { prompt: "  ".into(), ..job() }, &control(&|| false)).unwrap_err();
        assert_eq!(e, Error::Config("write a prompt first".into()));
    }

    #[test]
    fn cancelling_interrupts_the_server() {
        let cancel = AtomicBool::new(false);
        let fake = Fake::new(|r, _| if r.url.ends_with("/prompt") { ok(r#"{"prompt_id":"abc"}"#) } else { ok("{}") });
        let polls = AtomicU32::new(0);
        let cancelled = || polls.fetch_add(1, Ordering::Relaxed) > 3 || cancel.load(Ordering::Relaxed);
        let e = generate(&fake, &settings(), &Family::ZImage.preset(), "", &job(), &control(&cancelled)).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        let urls = fake.urls();
        assert!(urls.iter().any(|u| u.ends_with("/interrupt")) && urls.iter().any(|u| u.ends_with("/queue")), "{urls:?}");
        // Cancelled before it started: nothing is sent.
        let idle = Fake::new(|_, _| panic!("no request expected"));
        assert_eq!(generate(&idle, &settings(), &Family::ZImage.preset(), "", &job(), &control(&|| true)).unwrap_err(), Error::Cancelled);
    }

    #[test]
    fn cloud_models_send_one_request() {
        let fake = Fake::new(|r, _| {
            assert!(r.url.starts_with("https://api.openai.com/"));
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#)
        });
        let m = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
        let g = generate(&fake, &settings(), &m, "sk-x", &job(), &control(&|| false)).unwrap();
        assert_eq!(g.bytes, b"hello");
        // No usage in the answer: no cost claimed.
        assert_eq!(g.cost, None);
        let billed = Fake::new(|_, _| {
            ok(
                r#"{"data":[{"b64_json":"aGVsbG8="}],"usage":{"input_tokens":20,"output_tokens":1000,"input_tokens_details":{"text_tokens":20,"image_tokens":0}}}"#,
            )
        });
        let cost = generate(&billed, &settings(), &m, "sk-x", &job(), &control(&|| false)).unwrap().cost.unwrap();
        assert!((cost - (20.0 * 5.0 + 1000.0 * 30.0) / 1e6).abs() < 1e-12, "{cost}");
        assert_eq!(fake.urls().len(), 1);
        let e = generate(&fake, &settings(), &m, "", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("API key"), "{e}");

        let gem = Fake::new(|_, _| ok(r#"{"candidates":[{"content":{"parts":[{"inlineData":{"data":"aGVsbG8="}}]}}]}"#));
        let m = Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false);
        assert_eq!(generate(&gem, &settings(), &m, "k", &job(), &control(&|| false)).unwrap().bytes, b"hello");
        let down = Fake::new(|_, _| Err(Error::Network("dns".into())));
        assert!(generate(&down, &settings(), &m, "k", &job(), &control(&|| false)).unwrap_err().to_string().contains("Could not reach Google Gemini"));
    }

    #[test]
    fn model_lists_come_from_each_folder() {
        let fake = Fake::new(|r, _| if r.url.ends_with("/models/loras") { Ok(Response { status: 404, body: Vec::new() }) } else { ok(r#"["a.safetensors"]"#) });
        let m = comfy_models(&fake, "http://comfy:8188").unwrap();
        assert_eq!(m.len(), 5);
        assert_eq!(m["vae"], vec!["a.safetensors"]);
        let down = Fake::new(|_, _| Err(Error::Network("refused".into())));
        assert!(comfy_models(&down, "http://comfy:8188").unwrap_err().to_string().contains("Is it running?"));
        assert!(comfy_models(&down, "nope").is_err());
    }

    #[test]
    fn random_seeds_differ_and_fit_48_bits() {
        let a = random_seed();
        let b = random_seed();
        assert_ne!(a, b);
        assert!(a < 1 << 48 && b < 1 << 48);
    }

    #[test]
    fn xai_and_meta_answer_at_once_and_say_what_the_image_cost() {
        let xai = Fake::new(|r, _| {
            assert_eq!(r.url, "https://api.x.ai/v1/images/generations");
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}],"usage":{"cost_in_usd_ticks":400000000}}"#)
        });
        let m = Model::cloud(Provider::Xai, "Grok Imagine Image 2.0", "grok-imagine-image-2.0", false);
        let g = generate(&xai, &settings(), &m, "k", &job(), &control(&|| false)).unwrap();
        assert_eq!((g.bytes.as_slice(), g.cost), (&b"hello"[..], Some(0.04)));
        assert_eq!(xai.urls().len(), 1);
        // An edit goes to the edit endpoint and comes back at the size of the picture given.
        let edit = Job { source: Some(Source { mask: Vec::new(), edit: true, ..source() }), ..job() };
        let edits = Fake::new(|r, _| {
            assert_eq!(r.url, "https://api.x.ai/v1/images/edits");
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#)
        });
        let g = generate(&edits, &settings(), &m, "k", &edit, &control(&|| false)).unwrap();
        let given = edit.source.as_ref().map(|s| (s.width, s.height)).unwrap();
        assert_eq!(((g.width, g.height), g.cost), (given, None));

        let meta = Fake::new(|r, _| {
            assert_eq!(r.url, "https://api.meta.ai/v1/images/generations");
            ok(r#"{"data":[{"b64_json":"aGVsbG8="}],"output_format":"png"}"#)
        });
        let muse = Model::cloud(Provider::Meta, "Muse Image", "muse-image-1.0", false);
        let g = generate(&meta, &settings(), &muse, "k", &job(), &control(&|| false)).unwrap();
        assert_eq!((g.bytes.as_slice(), g.cost), (&b"hello"[..], Some(0.01)));
        // Another model of Meta's has no price here.
        let other = Model { api_model: "something-else".into(), ..muse.clone() };
        assert_eq!(generate(&meta_any(), &settings(), &other, "k", &job(), &control(&|| false)).unwrap().cost, None);
        // Without a key nothing is sent; a refusal is the provider's message.
        assert!(matches!(generate(&meta, &settings(), &muse, "", &job(), &control(&|| false)), Err(Error::Config(_))));
        let refused = Fake::new(|_, _| Ok(Response { status: 429, body: br#"{"error":{"message":"slow down"}}"#.to_vec() }));
        let e = generate(&refused, &settings(), &m, "k", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("xAI: slow down"), "{e}");
    }

    fn meta_any() -> Fake {
        Fake::new(|_, _| ok(r#"{"data":[{"b64_json":"aGVsbG8="}]}"#))
    }

    #[test]
    fn black_forest_labs_is_submitted_polled_and_downloaded_without_the_key() {
        const POLL_URL: &str = "https://api.eu1.bfl.ai/v1/get_result?id=t1";
        const SAMPLE: &str = "https://delivery-eu1.bfl.ai/results/a.png?sig=1";
        let m = Model::cloud(Provider::Bfl, "FLUX 3 Image", "flux-3-image", false);
        let fake = Fake::new(|r, n| {
            let keyed = r.headers.iter().any(|(k, v)| k == "x-key" && v == "bfl-k");
            match (r.url.as_str(), n) {
                ("https://api.bfl.ai/v1/flux-3-image", 0) => {
                    assert!(keyed && r.method == Method::Post);
                    ok(&format!(r#"{{"id":"t1","polling_url":"{POLL_URL}","cost":4.8}}"#))
                }
                (POLL_URL, 1) => ok(r#"{"status":"Pending"}"#),
                (POLL_URL, 2) => ok(r#"{"status":"Generating"}"#),
                (POLL_URL, 3) => {
                    assert!(keyed && r.method == Method::Get);
                    ok(&format!(r#"{{"status":"Ready","result":{{"sample":"{SAMPLE}"}}}}"#))
                }
                (SAMPLE, 4) => {
                    // The signed address is not the API: the key stays home.
                    assert!(r.headers.is_empty(), "{:?}", r.headers);
                    ok("PNGDATA")
                }
                (u, n) => panic!("unexpected request {n}: {u}"),
            }
        });
        let g = generate(&fake, &settings(), &m, "bfl-k", &job(), &control(&|| false)).unwrap();
        assert_eq!(g.bytes, b"PNGDATA");
        assert!((g.cost.unwrap() - 0.048).abs() < 1e-12);
        assert_eq!(fake.urls().len(), 5);

        // An answer that points the poll somewhere else is refused before the key goes there.
        let hijack = Fake::new(|_, _| ok(r#"{"id":"t1","polling_url":"https://evil.example/get_result"}"#));
        let e = generate(&hijack, &settings(), &m, "bfl-k", &job(), &control(&|| false)).unwrap_err();
        assert!(matches!(e, Error::BadResponse(_)), "{e}");
        assert_eq!(hijack.urls().len(), 1);
        // A moderated job, one that never finishes, and a cancel each end the wait.
        let moderated =
            Fake::new(|_, n| if n == 0 { ok(&format!(r#"{{"id":"t","polling_url":"{POLL_URL}"}}"#)) } else { ok(r#"{"status":"Request Moderated"}"#) });
        assert!(generate(&moderated, &settings(), &m, "k", &job(), &control(&|| false)).unwrap_err().to_string().contains("refused the prompt"));
        let stuck = || Fake::new(|_, n| if n == 0 { ok(&format!(r#"{{"id":"t","polling_url":"{POLL_URL}"}}"#)) } else { ok(r#"{"status":"Pending"}"#) });
        let (slow, quick) = (stuck(), Settings { timeout_seconds: 10, ..settings() });
        let e = generate(&slow, &quick, &m, "k", &job(), &control(&|| false)).unwrap_err().to_string();
        assert!(e.contains("did not finish within 10 seconds"), "{e}");
        assert!(slow.urls().len() <= 8, "{}", slow.urls().len());
        let (slow, polls) = (stuck(), AtomicU32::new(0));
        let cancelled = generate(&slow, &settings(), &m, "k", &job(), &control(&|| polls.fetch_add(1, Ordering::Relaxed) >= 3)).unwrap_err();
        assert_eq!(cancelled, Error::Cancelled);
        // Out of credits, and a download that fails.
        let broke = Fake::new(|_, _| Ok(Response { status: 402, body: br#"{"detail":"Insufficient credits"}"#.to_vec() }));
        assert!(generate(&broke, &settings(), &m, "k", &job(), &control(&|| false)).unwrap_err().to_string().contains("out of credits"));
        let gone = Fake::new(|r, n| match n {
            0 => ok(&format!(r#"{{"id":"t","polling_url":"{POLL_URL}"}}"#)),
            1 => ok(&format!(r#"{{"status":"Ready","result":{{"sample":"{SAMPLE}"}}}}"#)),
            _ => {
                assert_eq!(r.url, SAMPLE);
                Ok(Response { status: 403, body: Vec::new() })
            }
        });
        assert!(generate(&gone, &settings(), &m, "k", &job(), &control(&|| false)).unwrap_err().to_string().contains("could not serve"));
    }
}
