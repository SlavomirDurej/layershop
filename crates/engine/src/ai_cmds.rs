//! Generative AI: fill the selection with an image from a local (ComfyUI) or cloud model.
//!
//! - `ai.generate` renders a prompt with one of the models set up in Preferences › Integrations
//!   (`integrations.ai`) and places the result on a new layer over the selection's bounds (the
//!   whole canvas without a selection), masked by the selection. The selection's size decides
//!   the size the model is asked for. It is a background job: cancellable, one undo step.
//!   With `"mode": "inpaint"` it repaints the selected part of the active pixel layer instead:
//!   the model is shown the layer around the selection and paints only inside it; the result is
//!   again a new layer, masked by the selection, so the layer underneath is untouched.
//! - `ai.models` lists the models and whether each can run now.
//! - `ai.comfyModels` asks the ComfyUI server which model files it has (the settings' pickers).
//! - `ai.setKey` / `ai.keys` hold the cloud API keys for this session.
//!
//! The network is the app's to provide: [`AiRuntime::transport`] is set by the desktop shell and
//! is `None` in the browser, in tests and in a plain headless session, where `ai.generate`
//! reports that instead of failing obscurely. API keys live in [`AiRuntime`], apart from the
//! preferences, so `prefs.get` (which agents can call) never returns one.

use std::collections::BTreeMap;
use std::sync::Arc;

use photocraft_aigen::{Control, Generated, Job, LocalServer, Model, Provider, ServerState, Settings, Source, Transport};
use photocraft_algo::resample::{Resample, crop_surface, resize_surface_in_canvas, translate_surface};
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_color::PixelFormat;
use photocraft_doc::{Layer, LayerContent, LayerMask};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// History label of a generation.
pub const LABEL: &str = "AI Generate";
/// History label of an inpainting.
pub const INPAINT_LABEL: &str = "AI Inpaint";
/// How much of the layer around the selection the model is shown when `context` is not given:
/// this fraction of the selection's longer edge, on every side. Enough to match the light and
/// the perspective; more leaves fewer rendered pixels for the selection itself.
pub const DEFAULT_CONTEXT: f64 = 0.5;
/// Longest prompt accepted (characters).
const MAX_PROMPT: usize = 20_000;

/// What generation needs beyond the preferences: the network and the API keys.
#[derive(Default, Clone)]
pub struct AiRuntime {
    /// Sends the requests. `None` where the app has no network (web, tests, headless).
    pub transport: Option<Arc<dyn Transport>>,
    /// Starts ComfyUI when a local model needs it and none is running (desktop; see
    /// `photocraft_aigen::launch`). `None`: the user starts it.
    pub server: Option<Arc<dyn LocalServer>>,
    /// API keys by provider key (`openai`, `gemini`). Never serialised with the preferences.
    keys: BTreeMap<String, String>,
}

impl std::fmt::Debug for AiRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Which keys exist, never their values.
        f.debug_struct("AiRuntime")
            .field("transport", &self.transport.is_some())
            .field("server", &self.server.is_some())
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AiRuntime {
    /// Store (or, with an empty key, forget) a provider's API key.
    pub fn set_key(&mut self, provider: Provider, key: &str) {
        let key = key.trim();
        if key.is_empty() {
            self.keys.remove(provider.key());
        } else {
            self.keys.insert(provider.key().to_string(), key.to_string());
        }
    }

    /// The provider's key: the stored one, else its environment variable (desktop).
    pub fn key(&self, provider: Provider) -> String {
        if let Some(k) = self.keys.get(provider.key()) {
            return k.clone();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(k) = provider.key_env().and_then(|name| std::env::var(name).ok()).filter(|k| !k.trim().is_empty()) {
            return k.trim().to_string();
        }
        String::new()
    }

    /// The stored keys as JSON text, for the app to save apart from the preferences.
    pub fn keys_json(&self) -> String {
        serde_json::to_string_pretty(&self.keys).unwrap_or_else(|_| "{}".into())
    }

    /// Replace the stored keys with those saved by [`Self::keys_json`]. Unknown providers and
    /// anything that isn't a string are skipped; unreadable text leaves no keys.
    pub fn load_keys_json(&mut self, text: &str) {
        self.keys.clear();
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) else { return };
        for p in Provider::ALL {
            if let Some(k) = m.get(p.key()).and_then(Value::as_str) {
                self.set_key(p, k);
            }
        }
    }

    /// Can `model` run now: `Err` says what is missing.
    pub fn ready(&self, model: &Model) -> std::result::Result<(), String> {
        if self.transport.is_none() {
            return Err("AI generation needs the desktop app".into());
        }
        if !model.provider.is_local() && self.key(model.provider).is_empty() {
            return Err(format!("{} needs an API key (Preferences › Integrations)", model.provider.label()));
        }
        Ok(())
    }
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn ai_error(e: photocraft_aigen::Error) -> EngineError {
    match e {
        photocraft_aigen::Error::Cancelled => EngineError::Cancelled,
        e => EngineError::Other(e.to_string()),
    }
}

fn settings(s: &Session) -> Settings {
    s.prefs().integrations.ai.clone()
}

fn transport(s: &Session) -> Result<Arc<dyn Transport>> {
    s.ai.transport.clone().ok_or_else(|| EngineError::Other("AI generation is not available here: it needs the desktop app's network access".into()))
}

/// The area to fill: `rect` (`[x, y, w, h]`) when given, else the selection's bounds, else the
/// canvas; always clipped to the canvas.
fn target_area(p: &Value, selection: Option<&Surface>, canvas: Rect) -> Result<Rect> {
    let area = match p.get("rect") {
        Some(r) => {
            let v: Vec<i64> = r.as_array().into_iter().flatten().filter_map(|x| x.as_f64().filter(|x| x.is_finite()).map(|x| x.round() as i64)).collect();
            let [x, y, w, h] = v[..] else { return Err(bad("ai.generate", "`rect` must be [x, y, width, height]")) };
            let clamp = |v: i64| v.clamp(-1_000_000, 1_000_000) as i32;
            if w <= 0 || h <= 0 {
                return Err(bad("ai.generate", "`rect` must have a positive width and height"));
            }
            Rect::new(clamp(x), clamp(y), clamp(x.saturating_add(w)), clamp(y.saturating_add(h)))
        }
        None => selection.map_or(canvas, Surface::content_bounds),
    };
    let area = area.intersect(&canvas);
    if area.is_empty() {
        return Err(EngineError::Other("the area to generate into is empty or outside the canvas".into()));
    }
    Ok(area)
}

/// The area an inpainting works on: the selection's bounds grown by `context` (a fraction of
/// their longer edge) on every side, within the canvas.
pub fn inpaint_area(selection: Rect, canvas: Rect, context: f64) -> Rect {
    let context = if context.is_finite() { context.clamp(0.0, 4.0) } else { DEFAULT_CONTEXT };
    let pad = (f64::from(selection.width().max(selection.height())) * context).round().min(1_000_000.0) as i32;
    Rect::new(selection.x0.saturating_sub(pad), selection.y0.saturating_sub(pad), selection.x1.saturating_add(pad), selection.y1.saturating_add(pad))
        .intersect(&canvas)
}

/// Why `ai.generate {"mode": "inpaint"}` can't run on the active document, or the selection's
/// bounds within the canvas. The panel shows the reason under its button.
pub fn inpaint_blocked(s: &Session) -> std::result::Result<Rect, String> {
    let d = s.active().ok_or("no document open")?;
    let selected = d.doc.selection.as_ref().map(|sel| sel.content_bounds().intersect(&d.doc.bounds())).filter(|b| !b.is_empty());
    let selected = selected.ok_or("Inpainting needs a selection: select the part of the layer to repaint (the Marquee or Lasso tool)")?;
    let layer = d.active_layer.and_then(|id| d.doc.layer(id)).ok_or("Inpainting needs a layer: choose the pixel layer to repaint")?;
    let LayerContent::Raster(pixels) = &layer.content else {
        return Err(format!(
            "Inpainting repaints a pixel layer, and \u{201c}{}\u{201d} is a {} layer: choose a pixel layer, or rasterize this one",
            layer.name,
            layer.content.kind_name()
        ));
    };
    if pixels.content_bounds().intersect(&selected).is_empty() {
        return Err(format!("\u{201c}{}\u{201d} is empty inside the selection: choose the layer that holds the image to repaint", layer.name));
    }
    Ok(selected)
}

/// The part of `s` inside `crop`, resampled to `width` × `height`: interleaved samples in the
/// surface's own format.
fn resampled(s: &Surface, crop: Rect, width: u32, height: u32) -> Vec<f32> {
    let local = translate_surface(&crop_surface(s, crop), crop.x0.saturating_neg(), crop.y0.saturating_neg());
    let (sx, sy) = (f64::from(width) / f64::from(crop.width().max(1)), f64::from(height) / f64::from(crop.height().max(1)));
    let scaled = if (sx - 1.0).abs() > 1e-9 || (sy - 1.0).abs() > 1e-9 {
        resize_surface_in_canvas(&local, sx, sy, Resample::Bicubic, Rect::new(0, 0, crop.width() as i32, crop.height() as i32))
    } else {
        local
    };
    scaled.read_region(Rect::new(0, 0, width as i32, height as i32))
}

fn png(width: u32, height: u32, layout: ChannelLayout, data: Vec<u8>) -> Result<Vec<u8>> {
    let image = Image::from_u8(width, height, layout, data).map_err(|e| EngineError::Other(format!("could not prepare the image to inpaint ({e})")))?;
    photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default())
        .map_err(|e| EngineError::Other(format!("could not prepare the image to inpaint ({e})")))
}

fn byte(v: f32) -> u8 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 }
}

/// What an inpainting sends the model: the layer's pixels inside `crop` (over white where the
/// layer is transparent) and the selection there as the mask, both at `size`.
fn inpaint_source(layer: &Surface, selection: &Surface, crop: Rect, size: (u32, u32), strength: f64) -> Result<Source> {
    let (width, height) = size;
    let pixels = (width as usize).checked_mul(height as usize).filter(|n| *n > 0 && *n <= 64 << 20);
    let pixels = pixels.ok_or_else(|| EngineError::Other("the area to inpaint is too large to send to the model".into()))?;
    let fmt = layer.format();
    let mut rgb = Vec::with_capacity(pixels * 3);
    for px in resampled(layer, crop, width, height).chunks_exact(fmt.channels()) {
        let [r, g, b, a] = photocraft_raster::to_rgba(&fmt, px);
        let a = if a.is_finite() { a.clamp(0.0, 1.0) } else { 0.0 };
        rgb.extend([r, g, b].map(|c| byte(c * a + (1.0 - a))));
    }
    // Transparent where the model paints: the selection's coverage, inverted, as alpha.
    let mut mask = Vec::with_capacity(pixels * 4);
    for px in resampled(selection, crop, width, height).chunks_exact(selection.format().channels().max(1)) {
        mask.extend([0, 0, 0, 255 - byte(px.first().copied().unwrap_or(0.0))]);
    }
    if rgb.len() != pixels * 3 || mask.len() != pixels * 4 {
        return Err(EngineError::Other("could not read the area to inpaint".into()));
    }
    Ok(Source { image: png(width, height, ChannelLayout::Rgb, rgb)?, mask: png(width, height, ChannelLayout::Rgba, mask)?, width, height, strength })
}

/// Decode a generated image and fit it to `area`, in the document's format with alpha: scaled
/// to cover it (keeping the image's own aspect ratio), centred, and cropped to it; or, with
/// `exact`, stretched onto it, so an inpainting lands where the image it was given came from.
fn fit(bytes: &[u8], fmt: PixelFormat, area: Rect, exact: bool) -> Result<Surface> {
    let src = crate::file_cmds::import("generated image", bytes)
        .map_err(|e| EngineError::Other(format!("the model's answer is not an image PhotoCraft can read ({e})")))?;
    let (w, h) = (f64::from(src.size.width), f64::from(src.size.height));
    if w < 1.0 || h < 1.0 {
        return Err(EngineError::Other("the model returned an empty image".into()));
    }
    let target = PixelFormat::new(fmt.mode, fmt.sample, true);
    let mut px = crate::file_cmds::flattened(&src, target);
    let (aw, ah) = (f64::from(area.width()), f64::from(area.height()));
    let cover = (aw / w).max(ah / h);
    let (sx, sy) = if exact { (aw / w, ah / h) } else { (cover, cover) };
    if (sx - 1.0).abs() > 1e-6 || (sy - 1.0).abs() > 1e-6 {
        // Edge pixels repeat outwards, so an opaque image stays opaque up to its border.
        px = resize_surface_in_canvas(&px, sx, sy, Resample::Bicubic, Rect::from_size(src.size));
    }
    let b = px.content_bounds();
    let dx = area.x0 + ((aw - f64::from(b.width())) / 2.0).round() as i32 - b.x0;
    let dy = area.y0 + ((ah - f64::from(b.height())) / 2.0).round() as i32 - b.y0;
    let px = translate_surface(&px, dx, dy);
    let mut out = Surface::new(target);
    out.write_region(area, &px.read_region(area));
    out.prune();
    Ok(out)
}

/// A layer name from the prompt: its first words, on one line.
fn layer_name(prompt: &str) -> String {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 40 { flat } else { flat.chars().take(40).chain(['…']).collect() }
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.generate";
    let prompt = p.get("prompt").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "missing \"prompt\""))?.to_string();
    if prompt.chars().count() > MAX_PROMPT {
        return Err(bad(cmd, format!("the prompt is longer than {MAX_PROMPT} characters")));
    }
    let settings = settings(s);
    let id = p.get("model").and_then(Value::as_str).unwrap_or("");
    let model = settings.model(id).cloned().ok_or_else(|| {
        if id.is_empty() {
            EngineError::Other("no AI model is set up: add one in Preferences › Integrations".into())
        } else {
            bad(cmd, format!("no AI model `{id}`"))
        }
    })?;
    let transport = transport(s)?;
    let server = s.ai.server.clone().filter(|_| model.provider.is_local());
    let key = s.ai.key(model.provider);
    s.ai.ready(&model).map_err(EngineError::Other)?;
    let inpaint = match p.get("mode").map(|m| m.as_str().unwrap_or("?")) {
        None | Some("txt2img") => false,
        Some("inpaint") => true,
        Some(m @ ("img2img" | "imgEdit")) => return Err(EngineError::Other(format!("`{m}` is not available yet; use txt2img or inpaint"))),
        Some(_) => return Err(bad(cmd, "`mode` must be txt2img|inpaint")),
    };
    let number = |key: &str, default: f64, max: f64| match p.get(key) {
        None => Ok(default),
        Some(v) => v.as_f64().filter(|x| x.is_finite() && (0.0..=max).contains(x)).ok_or_else(|| bad(cmd, format!("`{key}` must be a number from 0 to {max}"))),
    };
    let (strength, context) = (number("strength", 1.0, 1.0)?, number("context", DEFAULT_CONTEXT, 4.0)?);
    if inpaint && !model.can_inpaint() {
        return Err(EngineError::Other(format!("{} is not set up for inpainting (Preferences \u{203a} AI Integrations, its Inpainting box)", model.name)));
    }
    // What an inpainting reads: the active pixel layer, and the selection's bounds on it.
    let selected = if inpaint { Some(inpaint_blocked(s).map_err(EngineError::Other)?) } else { None };
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let selection = d.doc.selection.clone();
    let source_layer = match selected {
        Some(_) => d.active_layer.and_then(|id| d.doc.layer(id)).and_then(|l| match &l.content {
            LayerContent::Raster(pixels) => Some(pixels.clone()),
            _ => None,
        }),
        None => None,
    };
    let area = match selected {
        // The model is shown the layer around the selection, and its answer covers that area.
        Some(selected) if p.get("rect").is_none() => inpaint_area(selected, d.doc.bounds(), context),
        Some(selected) => {
            let area = target_area(p, selection.as_ref(), d.doc.bounds())?;
            if area.intersect(&selected).is_empty() {
                return Err(bad(cmd, "`rect` does not reach the selection"));
            }
            area
        }
        None => target_area(p, selection.as_ref(), d.doc.bounds())?,
    };
    let fmt = d.doc.pixel_format();
    let seed = match p.get("seed") {
        Some(v) => v.as_u64().ok_or_else(|| bad(cmd, "`seed` must be a non-negative integer"))? & 0xFFFF_FFFF_FFFF,
        None if model.seed >= 0 => model.seed as u64 & 0xFFFF_FFFF_FFFF,
        None => photocraft_aigen::random_seed(),
    };
    let alpha = p.get("alpha").and_then(Value::as_bool).unwrap_or(false);
    let masked = p.get("mask").and_then(Value::as_bool).unwrap_or(true);
    let mut job = Job { prompt: prompt.clone(), width: area.width(), height: area.height(), seed, alpha, source: None };
    let name = layer_name(&prompt);
    let model_id = model.id.clone();
    let label = if inpaint { INPAINT_LABEL } else { LABEL };
    let job_selection = selection.clone().filter(|_| inpaint);
    crate::jobs::run(
        s,
        label,
        true,
        move |ctx| {
            let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m), sleep: &|d| std::thread::sleep(d) };
            if let (Some(layer), Some(selection)) = (&source_layer, &job_selection) {
                ctx.progress(0.0, "Reading the layer\u{2026}");
                let size = photocraft_aigen::source_size(&model, area.width(), area.height());
                job.source = Some(inpaint_source(layer, selection, area, size, strength)?);
                ctx.check()?;
            }
            // A local model needs its server: use the one running, or start it.
            if let Some(server) = &server {
                server.ensure(transport.as_ref(), &settings, &control).map_err(ai_error)?;
            }
            let image = photocraft_aigen::generate(transport.as_ref(), &settings, &model, &key, &job, &control).map_err(ai_error)?;
            ctx.check()?;
            ctx.progress(0.98, "Placing the image…");
            let surface = fit(&image.bytes, fmt, area, inpaint)?;
            Ok((surface, image))
        },
        move |s, (surface, image): (Surface, Generated)| {
            let layer = s.edit(label, |doc, active| {
                let mut l = Layer::raster(name, surface.format());
                *crate::pixels_mut(&mut l)? = surface;
                // Like Generative Fill: the selection becomes the new layer's mask, so a lasso or
                // feathered selection shapes the result and stays editable.
                if masked && let Some(sel) = selection {
                    l.mask = Some(LayerMask { surface: sel, ..LayerMask::reveal_all() });
                }
                let id = doc.insert_above(*active, l);
                *active = Some(id);
                Ok(id)
            })?;
            Ok(json!({
                "layer": layer.0,
                "bounds": [area.x0, area.y0, area.width(), area.height()],
                "model": model_id,
                "mode": if inpaint { "inpaint" } else { "txt2img" },
                "seed": image.seed,
                "renderSize": [image.width, image.height],
                // US dollars, when the provider reported what it billed (OpenAI).
                "cost": image.cost,
            }))
        },
    )
}

fn models(s: &mut Session, _: &Value) -> Result<Value> {
    let settings = settings(s);
    let list: Vec<Value> = settings
        .models
        .iter()
        .map(|m| {
            let ready = s.ai.ready(m);
            json!({
                "id": m.id, "name": m.name, "provider": m.provider.key(), "local": m.provider.is_local(), "enabled": m.enabled,
                "alpha": m.alpha, "inpaint": m.can_inpaint(), "ready": ready.is_ok(), "why": ready.err(),
            })
        })
        .collect();
    Ok(json!({"models": list, "comfyUrl": settings.comfy_url, "available": s.ai.transport.is_some()}))
}

/// The settings a server command works with: `settings` when given (the Preferences dialog's
/// unsaved draft), else the saved ones; `url` overrides the address.
fn server_settings(s: &Session, p: &Value, cmd: &str) -> Result<Settings> {
    let mut settings = match p.get("settings") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad(cmd, format!("`settings`: {e}")))?,
        None => settings(s),
    };
    if let Some(url) = p.get("url") {
        settings.comfy_url = url.as_str().ok_or_else(|| bad(cmd, "`url` must be a string"))?.to_string();
    }
    Ok(settings)
}

fn comfy_models(s: &mut Session, p: &Value) -> Result<Value> {
    let settings = server_settings(s, p, "ai.comfyModels")?;
    let transport = transport(s)?;
    // `"start": false` only looks: the Preferences page checks the server when it opens.
    let server = s.ai.server.clone().filter(|_| p.get("start").and_then(Value::as_bool).unwrap_or(true));
    crate::jobs::run(
        s,
        "Connect to ComfyUI",
        false,
        move |ctx| {
            let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m), sleep: &|d| std::thread::sleep(d) };
            if let Some(server) = &server {
                server.ensure(transport.as_ref(), &settings, &control).map_err(ai_error)?;
            }
            let started = server.as_ref().is_some_and(|s| s.state() == ServerState::Started);
            ctx.progress(0.5, "Reading the model files…");
            photocraft_aigen::comfy_models(transport.as_ref(), &settings.comfy_url).map(|folders| (folders, started)).map_err(ai_error)
        },
        |_, (folders, started)| Ok(json!({"folders": folders, "startedHere": started})),
    )
}

/// `ai.server.find`: the launch settings of the ComfyUI that is running now.
fn server_find(s: &mut Session, p: &Value) -> Result<Value> {
    let settings = server_settings(s, p, "ai.server.find")?;
    let transport = transport(s)?;
    let server = s.ai.server.clone().ok_or_else(|| EngineError::Other("starting ComfyUI is not available here: it needs the desktop app".into()))?;
    let launch = server.find(transport.as_ref(), &settings.comfy_url).map_err(ai_error)?;
    serde_json::to_value(&launch).map_err(|e| EngineError::Other(e.to_string()))
}

/// `ai.server.stop`: stop the ComfyUI this app started (never one started elsewhere).
fn server_stop(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"stopped": s.ai.server.as_ref().is_some_and(|server| server.stop())}))
}

/// `ai.server.status`: what is known without asking the server.
fn server_status(s: &mut Session, _: &Value) -> Result<Value> {
    let settings = settings(s);
    let started = s.ai.server.as_ref().is_some_and(|server| server.state() == ServerState::Started);
    Ok(json!({"url": settings.comfy_url, "autostart": settings.comfy_autostart, "canStart": s.ai.server.is_some(), "startedHere": started}))
}

fn provider_param(p: &Value, cmd: &str) -> Result<Provider> {
    let key = p.get("provider").and_then(Value::as_str).unwrap_or("");
    Provider::ALL.into_iter().find(|x| x.key() == key && !x.is_local()).ok_or_else(|| bad(cmd, "`provider` must be openai|gemini"))
}

fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.setKey";
    let provider = provider_param(p, cmd)?;
    let key = p.get("key").and_then(Value::as_str).ok_or_else(|| bad(cmd, "missing \"key\" (an empty string removes it)"))?;
    if key.len() > 1024 || key.chars().any(char::is_control) {
        return Err(bad(cmd, "that is not an API key"));
    }
    s.ai.set_key(provider, key);
    Ok(json!({"provider": provider.key(), "set": !key.trim().is_empty()}))
}

/// Which providers have a key (stored or from the environment). Never the keys themselves.
fn keys(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Object(Provider::ALL.into_iter().filter(|p| !p.is_local()).map(|p| (p.key().to_string(), json!(!s.ai.key(p).is_empty()))).collect()))
}

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

fn can_generate(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".to_string())?;
    if s.ai.transport.is_none() {
        return Err("AI generation needs the desktop app".into());
    }
    if !s.prefs().integrations.ai.models.iter().any(|m| m.enabled) {
        return Err("no AI model is set up (Preferences › Integrations)".into());
    }
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.generate",
            label: "AI Generate…",
            menu: &[],
            shortcut: None,
            params: r##"{"prompt":str,"model":id?=first enabled model,"mode":"txt2img|inpaint"="txt2img" (inpaint repaints the selected part of the active pixel layer: it needs a selection and pixels there),"strength":0..1=1 (inpaint, local models: how far the result may depart from what is there),"context":0..4=0.5 (inpaint: how much of the layer around the selection the model is shown, as a fraction of the selection's longer edge),"alpha":bool=false (transparent background, models that support it),"seed":int?=the model's (random when -1),"rect":[x,y,w,h]?=the selection's bounds (inpaint: grown by the context), else the canvas,"mask":bool=true (the selection becomes the layer mask)} → {layer,bounds,model,mode,seed,renderSize,cost (USD billed, OpenAI; else null)}"##,
            enabled: can_generate,
            run: generate,
            journal: true,
        },
        CommandSpec {
            id: "ai.models",
            label: "AI Models",
            menu: &[],
            shortcut: None,
            params: r##"{} → {models:[{id,name,provider,local,enabled,alpha,inpaint,ready,why}],comfyUrl,available}"##,
            enabled: always,
            run: models,
            journal: false,
        },
        CommandSpec {
            id: "ai.comfyModels",
            label: "ComfyUI Model Files",
            menu: &[],
            shortcut: None,
            params: r##"{"url":str?=the ComfyUI address in the preferences,"settings":{…}?=the saved integrations.ai,"start":bool=true} → {folders:{diffusion_models|text_encoders|vae|checkpoints|loras:[file,…]},startedHere} (starts ComfyUI first when the settings say so and none is running)"##,
            enabled: always,
            run: comfy_models,
            journal: false,
        },
        CommandSpec {
            id: "ai.server.find",
            label: "Find ComfyUI Installation",
            menu: &[],
            shortcut: None,
            params: r##"{"url":str?} → {python,folder,args}: how the ComfyUI running at the address was started (it must be running)"##,
            enabled: always,
            run: server_find,
            journal: false,
        },
        CommandSpec {
            id: "ai.server.stop",
            label: "Stop ComfyUI",
            menu: &[],
            shortcut: None,
            params: r##"{} → {stopped}: stops the ComfyUI PhotoCraft started; one started elsewhere is left running"##,
            enabled: always,
            run: server_stop,
            journal: false,
        },
        CommandSpec {
            id: "ai.server.status",
            label: "ComfyUI Server Status",
            menu: &[],
            shortcut: None,
            params: r##"{} → {url,autostart,canStart,startedHere}"##,
            enabled: always,
            run: server_status,
            journal: false,
        },
        CommandSpec {
            id: "ai.setKey",
            label: "Set AI API Key",
            menu: &[],
            shortcut: None,
            params: r##"{"provider":"openai|gemini","key":str (empty removes it)} (kept for this session; the app saves it apart from the preferences)"##,
            enabled: always,
            run: set_key,
            journal: false,
        },
        CommandSpec {
            id: "ai.keys",
            label: "AI API Keys",
            menu: &[],
            shortcut: None,
            params: r##"{} → {openai:bool,gemini:bool} (whether a key is set, never the key)"##,
            enabled: always,
            run: keys,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
