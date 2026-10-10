//! Running a generation: queue, wait, download.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::comfy::{self, History};
use crate::http::{Request, Transport};
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
    pub mask: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// How far the repainted part may depart from what is there, 0..=1: 1 paints it afresh,
    /// less keeps more of its shapes and colours. Local models; cloud models have no such control.
    pub strength: f64,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("image_len", &self.image.len())
            .field("mask_len", &self.mask.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("strength", &self.strength)
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
        Provider::Comfy => render_size(width, height, model.megapixels, model.family.size_multiple()),
        Provider::Openai => cloud::openai_size(model, width, height),
        // Gemini answers in the shape of the image it is given: send about what it renders.
        Provider::Gemini => render_size(width, height, if model.image_size.trim() == "1K" { 1.0 } else { 4.0 }, 8),
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
        Provider::Openai | Provider::Gemini => {
            let who = model.provider.label();
            let (request, size) = match (&job.source, model.provider) {
                (Some(source), Provider::Openai) => (cloud::openai_edit_request(model, job, source, key, timeout)?, (source.width, source.height)),
                (None, Provider::Openai) => (cloud::openai_request(model, job, key, timeout)?, cloud::openai_size(model, job.width, job.height)),
                (Some(source), _) => (cloud::gemini_request(model, job, key, timeout)?, (source.width, source.height)),
                (None, _) => (cloud::gemini_request(model, job, key, timeout)?, (job.width, job.height)),
            };
            (control.progress)(0.1, &format!("Waiting for {who}…"));
            let response = transport.send(&request).map_err(|e| Error::Network(format!("Could not reach {who}: {e}")))?;
            if (control.cancelled)() {
                return Err(Error::Cancelled);
            }
            let (bytes, cost) = if model.provider == Provider::Openai {
                let (bytes, usage) = cloud::parse_openai_with_usage(response.status, &response.body)?;
                (bytes, usage.and_then(|u| crate::pricing::billed(&model.api_model, &u)))
            } else {
                (cloud::parse_gemini(response.status, &response.body)?, None)
            };
            Ok(Generated { bytes, seed: job.seed, width: size.0, height: size.1, cost })
        }
    }
}

fn generate_comfy(transport: &dyn Transport, settings: &Settings, model: &Model, job: &Job, control: &Control<'_>, timeout: u64) -> Result<Generated> {
    let base = comfy::base_url(&settings.comfy_url)?;
    let unreachable = |e: Error| Error::Network(format!("Could not reach ComfyUI at {base}. Is it running? ({e})"));
    // An inpainting renders at the size of the image it was given.
    let (width, height) = match &job.source {
        Some(source) => (source.width, source.height),
        None => render_size(job.width, job.height, model.megapixels, model.family.size_multiple()),
    };
    let render = Job { width, height, source: None, ..job.clone() };
    let workflow = match &job.source {
        Some(source) => {
            // The set-up is checked before anything is uploaded.
            comfy::check(model, true)?;
            (control.progress)(0.01, "Sending the image to ComfyUI…");
            let mut names = Vec::with_capacity(2);
            for (name, bytes) in [(comfy::UPLOAD_IMAGE, &source.image), (comfy::UPLOAD_MASK, &source.mask)] {
                let r = transport.send(&comfy::upload_request(&base, name, bytes)).map_err(unreachable)?;
                names.push(comfy::parse_uploaded(r.status, &r.body)?);
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
            }
            let [image, mask] = names.as_slice() else { return Err(Error::BadResponse("ComfyUI did not take the image".into())) };
            comfy::inpaint_workflow(model, &render, &comfy::Inpaint { image, mask, denoise: source.denoise() })?
        }
        None => comfy::workflow(model, &render)?,
    };
    (control.progress)(0.02, "Sending to ComfyUI…");
    let queued = transport.send(&Request::post_json(format!("{base}/prompt"), comfy::queue_body(&workflow, "photocraft"))).map_err(unreachable)?;
    let id = comfy::parse_queued(queued.status, &queued.body)?;
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
        // No step count without ComfyUI's websocket: creep towards done so the bar moves.
        (control.progress)(0.05 + 0.9 * (1.0 - 1.0 / (1.0 + n as f32 / 40.0)), &message);
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
                .send(&Request::get(comfy::view_url(&base, &image)).timeout(120))
                .map_err(|e| Error::Network(format!("Could not download the image from ComfyUI: {e}")))?;
            if !r.ok() || r.body.is_empty() {
                return Err(Error::Server(format!("ComfyUI could not serve the generated image (HTTP {})", r.status)));
            }
            return Ok(Generated { bytes: r.body, seed: job.seed, width, height, cost: None });
        }
    }
    let _ = transport.send(&Request::post_json(format!("{base}/interrupt"), b"{}".to_vec()).timeout(5));
    Err(Error::Server(format!("ComfyUI did not finish within {timeout} seconds (raise the timeout in Preferences › Integrations)")))
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
                assert_eq!(body["client_id"], "photocraft");
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

    fn source() -> Source {
        Source { image: b"IMAGEPNG".to_vec(), mask: b"MASKPNG".to_vec(), width: 768, height: 512, strength: 0.7 }
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
    fn the_source_of_an_inpainting_is_sized_as_each_back_end_renders() {
        let z = Family::ZImage.preset();
        assert_eq!(source_size(&z, 500, 500), render_size(500, 500, z.megapixels, 16));
        assert_eq!(source_size(&Family::QwenImage21.preset(), 500, 500).0 % 32, 0);
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
}
