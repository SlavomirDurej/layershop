//! Generative AI: fill the selection with an image from a local (ComfyUI) or cloud model.
//!
//! - `ai.generate` renders a prompt with one of the models set up in Preferences › Generative AI
//!   (`integrations.ai`) and places the result on a new layer over the selection's bounds (the
//!   whole canvas without a selection), masked by the selection. The selection's size decides
//!   the size the model is asked for. It is a background job: cancellable, one undo step.
//!   With `"mode": "inpaint"` it repaints the selected part of the active pixel layer instead:
//!   the model is shown the layer around the selection and paints only inside it; the result is
//!   again a new layer, masked by the selection, so the layer underneath is untouched.
//!   With `"mode": "img2img"` it redraws a picture, following it as closely as `strength` says:
//!   a layer of the document (in place), or another open document (fitted to the area).
//!   With `"mode": "imgEdit"` an edit model changes a picture following the prompt as an
//!   instruction, with up to three more pictures to refer to, by number or by name (`@Luna`,
//!   see [`crate::ai_refs`]). With a selection only the part of
//!   the picture around it is sent (crop, edit, composite): the result is masked by the selection,
//!   so the rest of the picture is untouched and a large picture is edited near its own resolution.
//!   The new layer keeps what it was made from (`Layer::generation`: model, prompt, mode,
//!   strength, context, alpha, seed, source, references, quality), which the layer tree reports as `generation` and
//!   the AI Gen panel loads back when the layer is clicked.
//! - `ai.mask` paints the area an inpainting repaints: it is Quick Mask, started empty and shown
//!   as a tint over what is selected, so a brush adds to it; `ai.generate` turns it back into
//!   the selection before it runs.
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
use photocraft_aisetup::hf::{Credentials as HfCredentials, Via as HfVia};
use photocraft_algo::resample::{Resample, crop_surface, resize_surface_in_canvas, translate_surface};
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_color::PixelFormat;
use photocraft_doc::{
    AiPicture, ColorIndicates, Document, Generation, GenerationMode, GenerationQuality, GenerationReference, GenerationSource, Layer, LayerContent, LayerId,
    LayerMask,
};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// History label of a generation.
pub const LABEL: &str = "AI Generate";
/// History label of an inpainting.
pub const INPAINT_LABEL: &str = "AI Inpaint";
/// History label of an image-to-image generation.
pub const IMG2IMG_LABEL: &str = "AI Image to Image";
/// History label of an edit.
pub const EDIT_LABEL: &str = "AI Image Edit";
/// What [`img2img_plan`] says when there is no picture at all.
const NO_PICTURE: &str = "Image to image needs a picture to start from: a layer with pixels, or another open document";
/// How far an image-to-image may depart from its picture when `strength` is not given: enough to
/// redraw it in the prompt's terms, little enough to keep its composition.
pub const DEFAULT_IMG2IMG_STRENGTH: f64 = 0.6;
/// How much of the layer around the selection the model is shown when `context` is not given:
/// this fraction of the selection's longer edge, on every side. A model frames its subject in
/// the picture it renders, so the nearer that picture is to the selection, the surer what it
/// paints fits inside it; a margin is still needed to match the light and the perspective. A
/// local model editing is also told where the selection is ([`edit_mask`]).
pub const DEFAULT_CONTEXT: f64 = 0.25;
/// How far into the selection the mask a local model gets fades in, as a fraction of the
/// selection's shorter edge: in that band the model keeps more and more of what is there, so
/// what it paints is composed inside the selection and joins the picture without a seam.
const MASK_FADE: f64 = 0.12;
/// Longest prompt accepted (characters).
const MAX_PROMPT: usize = 20_000;

/// The names the Hugging Face sign-in is stored under, beside the providers' keys: the access
/// token, and for Sign in with Hugging Face what renews it, when it expires, whose it is, and how
/// it was had.
const HF_TOKEN: &str = "huggingface";
const HF_REFRESH: &str = "huggingfaceRefresh";
const HF_EXPIRES: &str = "huggingfaceExpires";
const HF_USER: &str = "huggingfaceUser";
const HF_VIA: &str = "huggingfaceVia";

/// What generation needs beyond the preferences: the network and the API keys.
#[derive(Default, Clone)]
pub struct AiRuntime {
    /// Sends the requests. `None` where the app has no network (web, tests, headless).
    pub transport: Option<Arc<dyn Transport>>,
    /// Starts ComfyUI when a local model needs it and none is running (desktop; see
    /// `photocraft_aigen::launch`). `None`: the user starts it.
    pub server: Option<Arc<dyn LocalServer>>,
    /// API keys by provider key (`openai`, `gemini`), and the Hugging Face sign-in. Never
    /// serialised with the preferences.
    keys: BTreeMap<String, String>,
    /// The Hugging Face sign-in changed here (signed in, renewed, signed out): the app saves the
    /// keys ([`Self::take_keys_dirty`]).
    keys_dirty: bool,
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

    /// Store (or, with an empty token, forget) an access token pasted by hand: what downloads a
    /// gated model's files in the Local AI setup, as the fallback to Sign in with Hugging Face.
    pub fn set_hf_token(&mut self, token: &str) {
        self.set_hf(&HfCredentials { access: token.trim().to_string(), via: HfVia::Token, ..HfCredentials::default() });
    }

    /// The Hugging Face access token, as stored (never one found on this PC that the user did
    /// not choose to use).
    pub fn hf_token(&self) -> String {
        self.keys.get(HF_TOKEN).cloned().unwrap_or_default()
    }

    /// The Hugging Face sign-in, as stored.
    pub fn hf(&self) -> HfCredentials {
        let text = |k: &str| self.keys.get(k).cloned().unwrap_or_default();
        let access = text(HF_TOKEN);
        // A token saved before signing in existed was pasted by hand.
        let via = match HfVia::from_key(&text(HF_VIA)) {
            HfVia::None if !access.is_empty() => HfVia::Token,
            via => via,
        };
        HfCredentials { access, refresh: text(HF_REFRESH), expires_at: text(HF_EXPIRES).parse().unwrap_or(0), user: text(HF_USER), via }
    }

    /// Store the Hugging Face sign-in (or, signed out, forget it), kept with the API keys and like
    /// them never in the preferences.
    pub fn set_hf(&mut self, c: &HfCredentials) {
        for k in [HF_TOKEN, HF_REFRESH, HF_EXPIRES, HF_USER, HF_VIA] {
            self.keys.remove(k);
        }
        if c.signed_in() {
            let fields = [
                (HF_TOKEN, c.access.trim().to_string()),
                (HF_REFRESH, c.refresh.trim().to_string()),
                (HF_EXPIRES, if c.expires_at == 0 { String::new() } else { c.expires_at.to_string() }),
                (HF_USER, c.user.trim().to_string()),
                (HF_VIA, c.via.key().to_string()),
            ];
            for (k, v) in fields.into_iter().filter(|(_, v)| !v.is_empty()) {
                self.keys.insert(k.to_string(), v);
            }
        }
        self.keys_dirty = true;
    }

    /// Has the Hugging Face sign-in changed since this was last asked? The app then saves the keys.
    pub fn take_keys_dirty(&mut self) -> bool {
        std::mem::take(&mut self.keys_dirty)
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
        for k in [HF_TOKEN, HF_REFRESH, HF_EXPIRES, HF_USER, HF_VIA] {
            if let Some(v) = m.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty() && v.len() <= 2048 && !v.chars().any(char::is_control)) {
                self.keys.insert(k.to_string(), v.to_string());
            }
        }
    }

    /// Can `model` run now: `Err` says what is missing.
    pub fn ready(&self, model: &Model) -> std::result::Result<(), String> {
        if self.transport.is_none() {
            return Err("AI generation needs the desktop app".into());
        }
        if !model.provider.is_local() && self.key(model.provider).is_empty() {
            return Err(format!("{} needs an API key (Preferences › Generative AI)", model.provider.label()));
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

/// A render's error `e`, saying why when the ComfyUI this app started stopped under it: without
/// that, losing the server reads as a network problem. A server that ends without a word was
/// usually stopped by the system for want of memory.
fn lost_server(server: Option<&Arc<dyn LocalServer>>, e: photocraft_aigen::Error) -> EngineError {
    match (&e, server.and_then(|server| server.stopped())) {
        (photocraft_aigen::Error::Network(_), Some(why)) => EngineError::Other(format!(
            "ComfyUI stopped while it was generating ({why}). When it stops without an error of its own, this computer most likely ran out of memory for the model: close other programs, or choose a smaller model"
        )),
        _ => ai_error(e),
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

/// Is the inpainting mask being painted ([`mask`]): Quick Mask, showing its tint over what is
/// selected?
pub fn mask_painting(s: &Session) -> bool {
    s.active().and_then(|d| d.doc.quick_mask.as_ref()).is_some_and(|q| q.indicates == ColorIndicates::SelectedAreas)
}

/// `ai.mask`: paint the area to inpaint, instead of selecting it.
///
/// `{"paint": true}` enters Quick Mask with the current selection as the mask (none: an empty
/// mask, so the first stroke is all of it) and its tint over the **selected** part, so painting
/// white adds to the area and black takes away. `{"paint": false}` leaves it: the mask becomes
/// the selection. `{"clear": true}` empties the mask, or drops the selection outside the mode.
fn mask(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.mask";
    let flag = |key: &str| match p.get(key) {
        None => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
    };
    let (paint, clear) = (flag("paint")?, flag("clear")?.unwrap_or(false));
    if paint.is_none() && !clear {
        return Err(bad(cmd, "give `paint` (true|false) or `clear` (true)"));
    }
    let on = s.active().ok_or(EngineError::NoDocument)?.doc.quick_mask.is_some();
    if clear {
        s.edit("Clear Inpaint Mask", |doc, _| {
            let fmt = crate::channel_cmds::channel_format(doc);
            match &mut doc.quick_mask {
                Some(q) => q.surface = Surface::new(fmt),
                None => doc.selection = None,
            }
            Ok(())
        })?;
    }
    match paint {
        Some(true) => {
            let opts = crate::channel_cmds::QuickMaskOptions { indicates: ColorIndicates::SelectedAreas, ..s.quick_mask_options };
            s.edit("Paint Inpaint Mask", |doc, _| {
                match &mut doc.quick_mask {
                    // Already in Quick Mask: the mask stays, its tint moves onto the selected part.
                    Some(q) => q.indicates = ColorIndicates::SelectedAreas,
                    None => {
                        let mut q = crate::channel_cmds::quick_mask_from(doc, &opts);
                        // Quick Mask takes "no selection" for "everything"; here nothing is
                        // chosen yet.
                        if doc.selection.is_none() {
                            q.surface = Surface::new(crate::channel_cmds::channel_format(doc));
                        }
                        doc.quick_mask = Some(q);
                        doc.selection = None;
                    }
                }
                Ok(())
            })?;
            if let Some(st) = s.active_mut() {
                st.channel_view.target = crate::channel_cmds::ChannelTarget::Composite;
                st.channel_view.quick_mask_hidden = false;
            }
        }
        Some(false) if on => leave_quick_mask(s)?,
        _ => {}
    }
    let d = s.active().ok_or(EngineError::NoDocument)?;
    Ok(json!({"painting": mask_painting(s), "empty": pending_selection(&d.doc).is_none()}))
}

/// Leave Quick Mask: the mask becomes the selection (an empty one: none).
fn leave_quick_mask(s: &mut Session) -> Result<()> {
    s.edit("Exit Quick Mask", |doc, _| {
        if let Some(q) = doc.quick_mask.take() {
            doc.selection = crate::channel_cmds::selection_from(doc, &q).filter(|sel| !sel.content_bounds().is_empty());
        }
        Ok(())
    })
}

/// The bounds of what is selected, within the canvas: the selection's, or in Quick Mask the
/// painted mask's (what leaving it would select).
fn pending_selection(doc: &Document) -> Option<Rect> {
    let surface = match &doc.quick_mask {
        Some(q) => Some(&q.surface),
        None => doc.selection.as_ref(),
    };
    surface.map(|sel| sel.content_bounds().intersect(&doc.bounds())).filter(|b| !b.is_empty())
}

/// Is anything marked for inpainting: a selection, or a painted mask?
pub fn inpaint_marked(s: &Session) -> bool {
    s.active().is_some_and(|d| pending_selection(&d.doc).is_some())
}

/// Why `ai.generate {"mode": "inpaint"}` can't run on the active document, or the selection's
/// bounds within the canvas (a mask being painted counts as the selection it will become). The
/// panel shows the reason under its button.
pub fn inpaint_blocked(s: &Session) -> std::result::Result<Rect, String> {
    let d = s.active().ok_or("no document open")?;
    let selected = pending_selection(&d.doc).ok_or(
        "Inpainting needs a selection: mark the part of the layer to repaint with the mask tools under the prompt (rectangle, ellipse or brush), or any selection tool",
    )?;
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

/// Blur `v` (`width` wide) with a box of `radius` on each side, along rows; beyond the edges it
/// counts as zero.
fn box_blur_rows(v: &[f32], width: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![0.0; v.len()];
    let window = (2 * radius + 1) as f32;
    for (row, out_row) in v.chunks_exact(width.max(1)).zip(out.chunks_exact_mut(width.max(1))) {
        let mut sum: f32 = row.iter().take(radius + 1).sum();
        for (x, o) in out_row.iter_mut().enumerate() {
            *o = sum / window;
            sum += row.get(x + radius + 1).copied().unwrap_or(0.0) - x.checked_sub(radius).and_then(|i| row.get(i)).copied().unwrap_or(0.0);
        }
    }
    out
}

fn transposed(v: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(v.len());
    for x in 0..width {
        out.extend((0..height).map(|y| v.get(y * width + x).copied().unwrap_or(0.0)));
    }
    out
}

/// Fade a `width` × `height` coverage mask in from its edge: zero at the edge of what it covers
/// (and at the picture's own edge, except on its `open` sides: left, top, right, bottom, where
/// the mask goes on as it is), rising to the mask's value `band` pixels inside. Nothing outside
/// the mask is added to it.
fn fade_inwards(mask: &mut [f32], width: usize, height: usize, band: usize, open: [bool; 4]) {
    if band == 0 || width == 0 || height == 0 || mask.len() != width * height {
        return;
    }
    // Three box blurs make a Gaussian near enough, about `band` / 2 wide. Wider than the picture
    // they blur no further, and the padding below stays the picture's size.
    let radius = (band / 4).clamp(1, width.max(height));
    // An open side is padded with its edge, as far as the blurs reach.
    let [left, top, right, bottom] = open.map(|o| if o { 3 * radius + 1 } else { 0 });
    let (w, h) = (width + left + right, height + top + bottom);
    let mut soft = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = y.saturating_sub(top).min(height - 1) * width;
        soft.extend((0..w).map(|x| mask.get(row + x.saturating_sub(left).min(width - 1)).copied().unwrap_or(0.0)));
    }
    for _ in 0..3 {
        soft = box_blur_rows(&soft, w, radius);
    }
    soft = transposed(&soft, w, h);
    for _ in 0..3 {
        soft = box_blur_rows(&soft, h, radius);
    }
    soft = transposed(&soft, h, w);
    // Half covered is the edge itself: from there to fully covered becomes 0 to 1.
    for (i, m) in mask.iter_mut().enumerate() {
        let s = soft.get((i / width + top) * w + i % width + left).copied().unwrap_or(0.0);
        *m = m.min((2.0 * s - 1.0).clamp(0.0, 1.0));
    }
}

/// How many pixels a picture of `size` has, when that is something to send a model.
fn pixel_count(size: (u32, u32)) -> Result<usize> {
    let pixels = (size.0 as usize).checked_mul(size.1 as usize).filter(|n| *n > 0 && *n <= 64 << 20);
    pixels.ok_or_else(|| EngineError::Other("the area is too large to send to the model".into()))
}

/// The pixels of `picture` inside `crop` at `size`, as straight RGBA in 0..=1.
fn picture_pixels(picture: &Surface, crop: Rect, size: (u32, u32)) -> Result<Vec<[f32; 4]>> {
    let pixels = pixel_count(size)?;
    let fmt = picture.format();
    let unit = |v: f32, or: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { or };
    let out: Vec<[f32; 4]> = resampled(picture, crop, size.0, size.1)
        .chunks_exact(fmt.channels())
        .map(|px| {
            let [r, g, b, a] = photocraft_raster::to_rgba(&fmt, px);
            [unit(r, 1.0), unit(g, 1.0), unit(b, 1.0), unit(a, 0.0)]
        })
        .collect();
    if out.len() != pixels {
        return Err(EngineError::Other("could not read the picture to send to the model".into()));
    }
    Ok(out)
}

/// `pixels` (at `size`) as an opaque PNG for a model: over white where they are transparent,
/// except where they are `free` (0..=1 each: the part the model paints), which is over the
/// picture's own colours carried out ([`extended`]). An edit model keeps what it is shown in an
/// empty part: white comes back white, so an outpainting would stay blank.
fn opaque_png(pixels: &[[f32; 4]], size: (u32, u32), free: &[f32]) -> Result<Vec<u8>> {
    png(size.0, size.1, ChannelLayout::Rgb, opaque_rgb(pixels, size, free)?)
}

/// What [`opaque_png`] encodes: three bytes a pixel.
fn opaque_rgb(pixels: &[[f32; 4]], size: (u32, u32), free: &[f32]) -> Result<Vec<u8>> {
    let (width, height) = size;
    if pixels.len() != pixel_count(size)? {
        return Err(EngineError::Other("could not read the picture to send to the model".into()));
    }
    let empty_and_free = free.iter().zip(pixels).any(|(f, p)| *f > 0.0 && p[3] < 1.0);
    let backdrop = if empty_and_free { extended(pixels, width as usize, height as usize) } else { Vec::new() };
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for (i, [r, g, b, a]) in pixels.iter().enumerate() {
        let f = free.get(i).copied().filter(|f| f.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        let under = backdrop.get(i).map_or([1.0; 3], |c| c.map(|v| v * f + (1.0 - f)));
        rgb.extend([r, g, b].into_iter().zip(under).map(|(c, u)| byte(c * a + u * (1.0 - a))));
    }
    Ok(rgb)
}

/// The smallest picture an outpainting's model is shown on its own ([`shown_png`]): less than
/// this a side, it is shown the whole area.
const MIN_SHOWN: usize = 64;

/// What an edit model is shown of an outpainting ([`photocraft_aigen::Source::shown`]): the
/// picture in `pixels` (at `size`) alone, cut from `rgb` ([`opaque_rgb`] of them) to its bounds,
/// when part of what is `free` (0..=1 each) is empty. Qwen Image 2.1 keeps what it is shown in
/// an empty part, whether white or the picture's colours carried out and blurred, so it is
/// shown none of it: the picture alone, it draws the scene on beyond it. Empty when nothing
/// free is empty, or the picture is small or is all of the area.
fn shown_png(pixels: &[[f32; 4]], rgb: &[u8], size: (u32, u32), free: &[f32]) -> Result<Vec<u8>> {
    let width = size.0 as usize;
    let solid = |p: &[f32; 4]| p[3] >= 0.5;
    let empty_and_free = free.iter().zip(pixels).any(|(f, p)| *f > 0.0 && !solid(p));
    if !empty_and_free || width == 0 || rgb.len() != pixels.len().saturating_mul(3) {
        return Ok(Vec::new());
    }
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in pixels.iter().enumerate().filter(|(_, p)| solid(p)) {
        let (x, y) = (i % width, i / width);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
    }
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w < MIN_SHOWN || h < MIN_SHOWN || (w, h) == (width, pixels.len() / width) {
        return Ok(Vec::new());
    }
    let mut cut = Vec::with_capacity(w * h * 3);
    for y in y0..y1 {
        let row = rgb.get((y * width + x0) * 3..(y * width + x1) * 3).ok_or_else(|| EngineError::Other("could not read the picture to send to the model".into()))?;
        cut.extend_from_slice(row);
    }
    png(w as u32, h as u32, ChannelLayout::Rgb, cut)
}

/// The colours of the picture in `pixels` (`width` × `height`) carried out over where it is
/// empty, softly: each empty pixel takes the nearest pixel of the picture in its row (a row with
/// none of it, the nearest one so filled in its column), and all of it is blurred. So an
/// outpainting starts from the picture's sky, sea and sand going on, which the model then draws
/// in detail. White where there is no picture at all.
fn extended(pixels: &[[f32; 4]], width: usize, height: usize) -> Vec<[f32; 3]> {
    if width == 0 || height == 0 || pixels.len() != width * height {
        return Vec::new();
    }
    let mut filled: Vec<Option<[f32; 3]>> = pixels.iter().map(|p| (p[3] >= 0.5).then_some([p[0], p[1], p[2]])).collect();
    for row in filled.chunks_exact_mut(width) {
        fill_line(row);
    }
    for x in 0..width {
        let mut column: Vec<_> = (0..height).map(|y| filled.get(y * width + x).copied().flatten()).collect();
        fill_line(&mut column);
        for (y, v) in column.into_iter().enumerate() {
            if let Some(slot) = filled.get_mut(y * width + x) {
                *slot = v;
            }
        }
    }
    // Three box blurs, divided by the same blur of ones so the edges don't darken.
    let radius = (width.max(height) / 40).max(1);
    let soft = |plane: Vec<f32>| {
        let mut v = plane;
        for _ in 0..3 {
            v = box_blur_rows(&v, width, radius);
        }
        v = transposed(&v, width, height);
        for _ in 0..3 {
            v = box_blur_rows(&v, height, radius);
        }
        transposed(&v, height, width)
    };
    let weight = soft(vec![1.0; width * height]);
    let [r, g, b] = [0, 1, 2].map(|c| soft(filled.iter().map(|v| v.map_or(1.0, |rgb| rgb[c])).collect()));
    (0..width * height)
        .map(|i| {
            let w = weight.get(i).copied().filter(|w| *w > 1e-6).unwrap_or(1.0);
            [&r, &g, &b].map(|plane| (plane.get(i).copied().unwrap_or(w) / w).clamp(0.0, 1.0))
        })
        .collect()
}

/// Give each empty (`None`) entry of `line` the nearest filled one's value; a line with none
/// filled stays empty.
fn fill_line(line: &mut [Option<[f32; 3]>]) {
    if line.iter().all(Option::is_some) || line.iter().all(Option::is_none) {
        return;
    }
    let mut before = Vec::with_capacity(line.len());
    let mut last = None;
    for (i, v) in line.iter().enumerate() {
        if let Some(c) = v {
            last = Some((i, *c));
        }
        before.push(last);
    }
    let mut after = vec![None; line.len()];
    let mut next = None;
    for ((i, v), slot) in line.iter().enumerate().rev().zip(after.iter_mut().rev()) {
        if let Some(c) = v {
            next = Some((i, *c));
        }
        *slot = next;
    }
    for (i, ((v, b), a)) in line.iter_mut().zip(before).zip(after).enumerate() {
        if v.is_none() {
            *v = match (b, a) {
                (Some((bi, bc)), Some((ai, ac))) => Some(if i.abs_diff(bi) <= i.abs_diff(ai) { bc } else { ac }),
                (Some((_, c)), None) | (None, Some((_, c))) => Some(c),
                (None, None) => None,
            };
        }
    }
}

/// The picture an image-to-image or an edit starts from, or one an edit refers to. In JSON
/// `{"layer": id}`, `{"document": id}`, or both for a layer of another open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceRef {
    /// A layer of the active document that has pixels, by id.
    Layer(u64),
    /// An open document, by id: its image as it shows, all layers together. The active document
    /// itself is taken where it is; another one is fitted to the area.
    Document(u64),
    /// A layer of an open document: its pixels alone. Of the active document it is
    /// [`SourceRef::Layer`]; of another it is fitted to the area like the document.
    DocumentLayer { document: u64, layer: u64 },
}

impl serde::Serialize for SourceRef {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = ser.serialize_map(None)?;
        match *self {
            SourceRef::Layer(id) => map.serialize_entry("layer", &id)?,
            SourceRef::Document(id) => map.serialize_entry("document", &id)?,
            SourceRef::DocumentLayer { document, layer } => {
                map.serialize_entry("document", &document)?;
                map.serialize_entry("layer", &layer)?;
            }
        }
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for SourceRef {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            layer: Option<u64>,
            document: Option<u64>,
        }
        let raw = Raw::deserialize(de)?;
        match (raw.document, raw.layer) {
            (None, Some(id)) => Ok(SourceRef::Layer(id)),
            (Some(id), None) => Ok(SourceRef::Document(id)),
            (Some(document), Some(layer)) => Ok(SourceRef::DocumentLayer { document, layer }),
            (None, None) => Err(serde::de::Error::custom("a picture is {\"layer\": id}, {\"document\": id} or both")),
        }
    }
}

impl SourceRef {
    /// A layer of the active document as [`SourceRef::Layer`], whichever way it was named.
    pub(crate) fn settled(self, s: &Session) -> SourceRef {
        match self {
            SourceRef::DocumentLayer { document, layer } if s.active().is_some_and(|d| d.doc.id.0 == document) => SourceRef::Layer(layer),
            other => other,
        }
    }
}

/// A picture an edit may refer to, and the name its instruction calls it by (`@Luna`; empty:
/// by number). In JSON the source's keys and `name`: `{"layer": 4, "name": "Luna"}`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamedRef {
    #[serde(flatten)]
    pub source: SourceRef,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl From<SourceRef> for NamedRef {
    fn from(source: SourceRef) -> Self {
        NamedRef { source, name: String::new() }
    }
}

impl From<NamedRef> for GenerationReference {
    fn from(r: NamedRef) -> Self {
        GenerationReference { source: r.source.into(), name: r.name }
    }
}

impl From<GenerationReference> for NamedRef {
    fn from(r: GenerationReference) -> Self {
        NamedRef { source: r.source.into(), name: r.name }
    }
}

impl From<SourceRef> for GenerationSource {
    fn from(source: SourceRef) -> Self {
        match source {
            SourceRef::Layer(id) => GenerationSource::Layer(id),
            SourceRef::Document(id) => GenerationSource::Document(id),
            SourceRef::DocumentLayer { document, layer } => GenerationSource::DocumentLayer { document, layer },
        }
    }
}

impl From<GenerationSource> for SourceRef {
    fn from(source: GenerationSource) -> Self {
        match source {
            GenerationSource::Layer(id) => SourceRef::Layer(id),
            GenerationSource::Document(id) => SourceRef::Document(id),
            GenerationSource::DocumentLayer { document, layer } => SourceRef::DocumentLayer { document, layer },
        }
    }
}

/// The pixels behind a [`SourceRef`].
#[derive(Clone)]
enum Picture {
    Layer(Surface),
    Document(Arc<Document>),
}

impl Picture {
    /// The part of the picture inside `rect` at `size`, as an opaque PNG.
    fn png(&self, rect: Rect, size: (u32, u32)) -> Result<Vec<u8>> {
        opaque_png(&self.pixels(rect, size)?, size, &[])
    }

    /// The part of the picture inside `rect` at `size` ([`picture_pixels`]).
    fn pixels(&self, rect: Rect, size: (u32, u32)) -> Result<Vec<[f32; 4]>> {
        match self {
            Picture::Layer(pixels) => picture_pixels(pixels, rect, size),
            Picture::Document(doc) => {
                let fmt = doc.pixel_format();
                let flat = crate::file_cmds::flattened(doc, PixelFormat::new(fmt.mode, fmt.sample, true));
                picture_pixels(&flat, rect, size)
            }
        }
    }
}

/// A picture an edit refers to, read when the edit runs.
#[derive(Clone)]
struct Reference {
    picture: Picture,
    rect: Rect,
}

impl Reference {
    /// The size the picture is sent at: its own, brought to between a quarter of a megapixel and
    /// one, which is what the edit models read a reference at.
    fn size(&self) -> (u32, u32) {
        let (w, h) = (f64::from(self.rect.width().max(1)), f64::from(self.rect.height().max(1)));
        let scale = ((w * h).clamp(512.0 * 512.0, 1024.0 * 1024.0) / (w * h)).sqrt();
        let edge = |v: f64| (v * scale).round().clamp(16.0, 8192.0) as u32;
        (edge(w), edge(h))
    }

    fn png(&self) -> Result<Vec<u8>> {
        self.picture.png(self.rect, self.size())
    }
}

/// The picture behind a reference of an edit: a layer's pixels on the canvas, or a document as
/// it shows. `Err` says why it can't be one.
fn reference(s: &Session, source: SourceRef) -> std::result::Result<Reference, String> {
    let d = s.active().ok_or("no document open")?;
    match source.settled(s) {
        SourceRef::Layer(id) => {
            let layer = d.doc.layer(LayerId(id)).ok_or_else(|| format!("the reference layer {id} is not in this document"))?;
            let pixels =
                layer.surface().ok_or_else(|| format!("\u{201c}{}\u{201d} has no pixels to refer to: choose a layer that has, or rasterize it", layer.name))?;
            let rect = pixels.content_bounds().intersect(&d.doc.bounds());
            if rect.is_empty() {
                return Err(format!("\u{201c}{}\u{201d} is empty: choose a reference that has pixels", layer.name));
            }
            Ok(Reference { picture: Picture::Layer(pixels.clone()), rect })
        }
        SourceRef::Document(id) => {
            let other = s.documents().iter().find(|o| o.doc.id.0 == id).ok_or_else(|| format!("the reference document {id} is not open"))?;
            if !has_image(&other.doc) {
                return Err(format!("\u{201c}{}\u{201d} is empty: there is no image in it to refer to", other.doc.name));
            }
            Ok(Reference { picture: Picture::Document(other.doc.clone()), rect: other.doc.bounds() })
        }
        SourceRef::DocumentLayer { document, layer } => {
            let (pixels, rect) = other_layer(s, document, layer, "refer to")?;
            Ok(Reference { picture: Picture::Layer(pixels), rect })
        }
    }
}

/// The pixels of layer `layer` of open document `document` and where they are on its canvas;
/// `Err` says why there are none to `what` (start from, refer to).
fn other_layer(s: &Session, document: u64, layer: u64, what: &str) -> std::result::Result<(Surface, Rect), String> {
    let other = s.documents().iter().find(|o| o.doc.id.0 == document).ok_or_else(|| format!("the document {document} is not open"))?;
    let l = other.doc.layer(LayerId(layer)).ok_or_else(|| format!("the layer {layer} is not in \u{201c}{}\u{201d}", other.doc.name))?;
    let pixels = l.surface().ok_or_else(|| format!("\u{201c}{}\u{201d} has no pixels to {what}: choose a layer that has, or rasterize it", l.name))?;
    let rect = pixels.content_bounds().intersect(&other.doc.bounds());
    if rect.is_empty() {
        return Err(format!("\u{201c}{}\u{201d} is empty: choose a layer that has pixels", l.name));
    }
    Ok((pixels.clone(), rect))
}

/// What an image-to-image will do, worked out before it runs.
#[derive(Clone)]
pub struct Img2ImgPlan {
    /// Where the result lands in the active document.
    pub area: Rect,
    /// The part of the source that is redrawn, in the source's own coordinates.
    pub source_rect: Rect,
    picture: Picture,
}

/// The largest rectangle of the shape `width` : `height` centred in `within`.
fn cover_crop(within: Rect, width: u32, height: u32) -> Rect {
    let (sw, sh) = (f64::from(within.width().max(1)), f64::from(within.height().max(1)));
    let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
    let (cw, ch) = if sw / sh > aspect { ((sh * aspect).round().clamp(1.0, sw), sh) } else { (sw, (sw / aspect).round().clamp(1.0, sh)) };
    let x0 = within.x0.saturating_add(((sw - cw) / 2.0).floor() as i32);
    let y0 = within.y0.saturating_add(((sh - ch) / 2.0).floor() as i32);
    Rect::new(x0, y0, x0.saturating_add(cw as i32), y0.saturating_add(ch as i32)).intersect(&within)
}

/// Does `doc` show anything: a layer with pixels on the canvas, or a fill layer? An empty
/// document is no picture to start from.
fn has_image(doc: &Document) -> bool {
    let canvas = doc.bounds();
    let shows = |l: &Layer| matches!(l.content, LayerContent::Fill(_)) || l.surface().is_some_and(|px| !px.content_bounds().intersect(&canvas).is_empty());
    !canvas.is_empty() && doc.walk().into_iter().any(|(_, _, l)| shows(l))
}

/// The pictures an image-to-image can start from now, with a name for each: the layers of the
/// active document that have pixels on the canvas (top first), then the other open documents.
pub fn img2img_sources(s: &Session) -> Vec<(SourceRef, String)> {
    let Some(d) = s.active() else { return Vec::new() };
    let canvas = d.doc.bounds();
    let mut out: Vec<(SourceRef, String)> = d
        .doc
        .walk()
        .into_iter()
        .rev()
        .filter(|(_, _, l)| l.surface().is_some_and(|px| !px.content_bounds().intersect(&canvas).is_empty()))
        .map(|(_, _, l)| (SourceRef::Layer(l.id.0), format!("Layer: {}", l.name)))
        .collect();
    let others = s.documents().iter().filter(|o| o.doc.id != d.doc.id && has_image(&o.doc));
    out.extend(others.map(|o| (SourceRef::Document(o.doc.id.0), format!("Document: {}", o.doc.name))));
    out
}

/// The pictures an edit can start from, or refer to: the active document as it shows (all its
/// layers together), then [`img2img_sources`].
pub fn edit_sources(s: &Session) -> Vec<(SourceRef, String)> {
    let Some(d) = s.active() else { return Vec::new() };
    let mut out = Vec::new();
    if has_image(&d.doc) {
        out.push((SourceRef::Document(d.doc.id.0), "This document (all layers)".to_string()));
    }
    out.extend(img2img_sources(s));
    out
}

/// The pictures an edit can refer to: [`edit_sources`], then the layers with pixels of the other
/// open documents, each after its document.
pub fn reference_sources(s: &Session) -> Vec<(SourceRef, String)> {
    let Some(d) = s.active() else { return Vec::new() };
    let mut out = edit_sources(s);
    for o in s.documents().iter().filter(|o| o.doc.id != d.doc.id && has_image(&o.doc)) {
        let canvas = o.doc.bounds();
        let at = out.iter().position(|(r, _)| *r == SourceRef::Document(o.doc.id.0)).map_or(out.len(), |i| i + 1);
        let layers: Vec<(SourceRef, String)> = o
            .doc
            .walk()
            .into_iter()
            .rev()
            .filter(|(_, _, l)| l.surface().is_some_and(|px| !px.content_bounds().intersect(&canvas).is_empty()))
            .map(|(_, _, l)| (SourceRef::DocumentLayer { document: o.doc.id.0, layer: l.id.0 }, format!("Layer: {} ({})", l.name, o.doc.name)))
            .collect();
        out.splice(at..at, layers);
    }
    out
}

/// A name for a reference to `source`: its layer's or document's ([`crate::ai_refs::suggest_name`]),
/// empty when that leaves nothing or the picture is not there.
pub fn reference_name(s: &Session, source: SourceRef) -> String {
    let doc = |id: u64| s.documents().iter().find(|o| o.doc.id.0 == id);
    let label = match source.settled(s) {
        SourceRef::Layer(id) => s.active().and_then(|d| d.doc.layer(LayerId(id))).map(|l| l.name.clone()),
        SourceRef::Document(id) => doc(id).map(|o| o.doc.name.clone()),
        SourceRef::DocumentLayer { document, layer } => doc(document).and_then(|o| o.doc.layer(LayerId(layer))).map(|l| l.name.clone()),
    };
    label.map(|l| crate::ai_refs::suggest_name(&l)).unwrap_or_default()
}

/// The source an image-to-image uses when none is named: the active layer when it has pixels,
/// else the first of [`img2img_sources`].
pub fn default_img2img_source(s: &Session) -> Option<SourceRef> {
    let sources = img2img_sources(s);
    let active = s.active().and_then(|d| d.active_layer).map(|id| SourceRef::Layer(id.0));
    active.filter(|a| sources.iter().any(|(r, _)| r == a)).or_else(|| sources.first().map(|(r, _)| *r))
}

/// Plan an image-to-image from `source` (`None`: [`default_img2img_source`]) into `rect`, or
/// say why it can't run.
///
/// A layer is redrawn in place: over the selection's bounds, else over the layer's own pixels.
/// Another document is fitted to the selection's bounds, else the canvas: scaled to cover the
/// area and cropped to its shape, like a generated image.
pub fn img2img_plan(s: &Session, source: Option<SourceRef>, rect: Option<Rect>) -> std::result::Result<Img2ImgPlan, String> {
    let d = s.active().ok_or("no document open")?;
    let canvas = d.doc.bounds();
    let source = source.or_else(|| default_img2img_source(s)).ok_or(NO_PICTURE)?.settled(s);
    let selected = d.doc.selection.as_ref().map(|sel| sel.content_bounds().intersect(&canvas)).filter(|b| !b.is_empty());
    let inside = |area: Rect| Some(area.intersect(&canvas)).filter(|a| !a.is_empty()).ok_or("the area to generate into is empty or outside the canvas");
    match source {
        SourceRef::Layer(id) => {
            let layer = d.doc.layer(LayerId(id)).ok_or_else(|| format!("the source layer {id} is not in this document"))?;
            let pixels = layer
                .surface()
                .ok_or_else(|| format!("\u{201c}{}\u{201d} has no pixels to start from: choose a layer that has, or rasterize it", layer.name))?;
            let content = pixels.content_bounds().intersect(&canvas);
            if content.is_empty() {
                return Err(format!("\u{201c}{}\u{201d} is empty: choose a layer that has pixels", layer.name));
            }
            let area = inside(rect.or(selected).unwrap_or(content))?;
            if content.intersect(&area).is_empty() {
                return Err(format!(
                    "\u{201c}{}\u{201d} is empty inside the selection: move the selection onto the layer's pixels, or choose another source",
                    layer.name
                ));
            }
            Ok(Img2ImgPlan { area, source_rect: area, picture: Picture::Layer(pixels.clone()) })
        }
        SourceRef::Document(id) => {
            let other = s.documents().iter().find(|o| o.doc.id.0 == id).ok_or_else(|| format!("the source document {id} is not open"))?;
            let whole = other.doc.bounds();
            if !has_image(&other.doc) {
                return Err(format!("\u{201c}{}\u{201d} is empty: there is no image in it to start from", other.doc.name));
            }
            let area = inside(rect.or(selected).unwrap_or(canvas))?;
            // The document itself is taken where it is; another one is fitted to the area.
            let source_rect = if id == d.doc.id.0 { area } else { cover_crop(whole, area.width(), area.height()) };
            Ok(Img2ImgPlan { area, source_rect, picture: Picture::Document(other.doc.clone()) })
        }
        // Another document's layer is fitted to the area like the document, its pixels alone.
        SourceRef::DocumentLayer { document, layer } => {
            let (pixels, content) = other_layer(s, document, layer, "start from")?;
            let area = inside(rect.or(selected).unwrap_or(canvas))?;
            Ok(Img2ImgPlan { area, source_rect: cover_crop(content, area.width(), area.height()), picture: Picture::Layer(pixels) })
        }
    }
}

/// Plan an edit of `source` (`None`: [`default_img2img_source`]), or say why it can't run.
///
/// Like [`img2img_plan`], but a selection scopes the edit of a picture of this document: the
/// model is sent the selection's bounds with `context` around them ([`inpaint_area`]), not the
/// whole picture, and its answer goes back there under the selection as a mask. So what is not
/// selected stays as it was, and a large picture is edited near its own resolution.
pub fn edit_plan(s: &Session, source: Option<SourceRef>, context: f64, rect: Option<Rect>) -> std::result::Result<Img2ImgPlan, String> {
    let d = s.active().ok_or("no document open")?;
    let canvas = d.doc.bounds();
    let source = source.or_else(|| default_img2img_source(s)).map(|r| r.settled(s));
    let here = !matches!(source, Some(SourceRef::Document(id) | SourceRef::DocumentLayer { document: id, .. }) if id != d.doc.id.0);
    // A mask being painted ([`mask`]) counts as the selection it will become.
    let selected = pending_selection(&d.doc);
    let rect = rect.or_else(|| selected.filter(|_| here).map(|b| inpaint_area(b, canvas, context)));
    img2img_plan(s, source, rect)
        .map_err(|why| if why == NO_PICTURE { "Editing needs a picture: a layer with pixels, or an open document with an image".to_string() } else { why })
}

impl Img2ImgPlan {
    /// What the model is sent: the source's pixels at `size`, with no mask (all of it is redrawn).
    fn source(&self, size: (u32, u32), strength: f64) -> Result<Source> {
        let image = self.picture.png(self.source_rect, size)?;
        Ok(Source { image, mask: Vec::new(), width: size.0, height: size.1, strength, edit: false, references: Vec::new(), shown: Vec::new() })
    }

    /// What a local model editing inside `selection` is sent: the source's pixels at `size`, and
    /// the selection as the mask ([`edit_mask`]). Where the selection is over nothing the picture
    /// is carried out into it ([`opaque_png`]), and the model is shown the picture alone
    /// ([`shown_png`]), so it paints there instead of keeping white or a blur.
    fn masked_source(&self, selection: &Surface, canvas: Rect, size: (u32, u32), strength: f64) -> Result<Source> {
        let pixels = self.picture.pixels(self.source_rect, size)?;
        let opacity: Vec<f32> = pixels.iter().map(|p| p[3]).collect();
        let mask = edit_mask(selection, self.area, canvas, size, &opacity)?;
        let free = edge_held_coverage(selection, self.area, size);
        let rgb = opaque_rgb(&pixels, size, &free)?;
        let shown = if mask.is_empty() { Vec::new() } else { shown_png(&pixels, &rgb, size, &free)? };
        let image = png(size.0, size.1, ChannelLayout::Rgb, rgb)?;
        Ok(Source { image, mask, width: size.0, height: size.1, strength, edit: false, references: Vec::new(), shown })
    }
}

/// A reference an edit is given: a picture read when it runs, or a copy the document keeps.
enum Given {
    Live(SourceRef),
    Copy(Arc<Vec<u8>>),
}

/// A reference's picture as it is sent.
enum Sent {
    Read(Reference),
    Copy(Arc<Vec<u8>>),
}

/// The references the active document keeps ([`Document::ai_references`]), with their names.
fn kept_references(s: &Session) -> Vec<(Given, String)> {
    let Some(d) = s.active() else { return Vec::new() };
    d.doc
        .ai_references
        .iter()
        .map(|r| {
            let given = match &r.picture {
                AiPicture::Layer(id) => Given::Live(SourceRef::Layer(*id)),
                AiPicture::Document => Given::Live(SourceRef::Document(d.doc.id.0)),
                AiPicture::Image { png, .. } => Given::Copy(png.clone()),
            };
            (given, r.name.clone())
        })
        .collect()
}

/// A copy of the picture behind `source`, as an edit's reference is sent (a quarter of a
/// megapixel to one, a PNG), and what it is (`Layer: Max (Other)`), for a document to keep.
pub fn reference_copy(s: &Session, source: SourceRef) -> std::result::Result<(Arc<Vec<u8>>, String), String> {
    let picture = reference(s, source)?;
    let png = picture.png().map_err(|e| e.to_string())?;
    let label = reference_sources(s).into_iter().find(|(r, _)| *r == source).map(|(_, label)| label).unwrap_or_else(|| "Picture".to_string());
    Ok((Arc::new(png), label))
}

/// The `references` param of `ai.generate`: up to [`photocraft_aigen::MAX_REFERENCES`] sources,
/// each with a name or none ([`crate::ai_refs::clean_name`]); no two named alike.
fn references_param(p: &Value) -> Result<Vec<NamedRef>> {
    let malformed = || bad("ai.generate", "`references` must be a list of {\"layer\": id}, {\"document\": id} or both, each with a \"name\" or none");
    let Some(v) = p.get("references") else { return Ok(Vec::new()) };
    let list = v.as_array().ok_or_else(malformed)?;
    if list.len() > photocraft_aigen::MAX_REFERENCES {
        return Err(bad("ai.generate", format!("`references` takes {} pictures at most", photocraft_aigen::MAX_REFERENCES)));
    }
    let mut refs: Vec<NamedRef> = list.iter().map(|r| serde_json::from_value(r.clone()).map_err(|_| malformed())).collect::<Result<_>>()?;
    for r in &mut refs {
        r.name = crate::ai_refs::clean_name(&r.name);
    }
    if let Some(name) = crate::ai_refs::duplicate(&refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()) {
        return Err(bad("ai.generate", format!("two references are named \u{201c}{name}\u{201d}: give each its own name")));
    }
    Ok(refs)
}

/// The `source` param of `ai.generate`: `{"layer": id}` or `{"document": id}`.
fn source_param(p: &Value) -> Result<Option<SourceRef>> {
    match p.get("source") {
        None => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|_| bad("ai.generate", "`source` must be {\"layer\": id} or {\"document\": id}")),
    }
}

/// What an inpainting sends the model: the layer's pixels inside `crop` and the selection there
/// as the mask, both at `size`. Where the layer is transparent the picture goes on under the
/// selection ([`opaque_png`]) and is white elsewhere. With `fade` the mask fades in from the
/// selection's edge ([`MASK_FADE`]) where the layer has pixels to keep.
fn inpaint_source(layer: &Surface, selection: &Surface, selected: Rect, crop: Rect, size: (u32, u32), strength: f64, fade: bool) -> Result<Source> {
    let pixels = picture_pixels(layer, crop, size)?;
    let whole = coverage(selection, crop, size);
    let image = opaque_png(&pixels, size, &whole)?;
    let mut coverage = whole.clone();
    if fade {
        let opacity: Vec<f32> = pixels.iter().map(|p| p[3]).collect();
        fade_selection(&mut coverage, selected, crop, size, [false; 4]);
        unfade_where_empty(&mut coverage, &whole, &opacity);
    }
    let mask = mask_png(coverage, size).map_err(|_| EngineError::Other("could not read the area to inpaint".into()))?;
    Ok(Source { image, mask, width: size.0, height: size.1, strength, edit: false, references: Vec::new(), shown: Vec::new() })
}

/// The mask an edit by a local model is given, so it composes what it paints inside the
/// selection instead of anywhere in the picture it is shown: the selection inside `crop` at
/// `size`, fading in from its edge ([`MASK_FADE`]) but not where the crop meets the canvas's
/// edge and the selection goes on to it, nor where the picture (`opacity` at `size`) is empty.
/// Empty when it is all of the crop: the edit is the picture's.
fn edit_mask(selection: &Surface, crop: Rect, canvas: Rect, size: (u32, u32), opacity: &[f32]) -> Result<Vec<u8>> {
    pixel_count(size)?;
    let whole = edge_held_coverage(selection, crop, size);
    let mut coverage = whole.clone();
    let open = [crop.x0 <= canvas.x0, crop.y0 <= canvas.y0, crop.x1 >= canvas.x1, crop.y1 >= canvas.y1];
    fade_selection(&mut coverage, selection.content_bounds().intersect(&crop), crop, size, open);
    unfade_where_empty(&mut coverage, &whole, opacity);
    if coverage.iter().all(|v| byte(*v) == 255) {
        return Ok(Vec::new());
    }
    mask_png(coverage, size).map_err(|_| EngineError::Other("could not read the selection to edit in".into()))
}

/// [`coverage`], sampled bilinearly with the crop's edge pixels held beyond it: a selection
/// that goes on to the crop's edge is whole up to it, not half covered there.
fn edge_held_coverage(selection: &Surface, crop: Rect, size: (u32, u32)) -> Vec<f32> {
    let (cw, ch) = (crop.width().max(1) as usize, crop.height().max(1) as usize);
    let channels = selection.format().channels().max(1);
    let src: Vec<f32> =
        selection.read_region(crop).chunks_exact(channels).map(|px| px.first().copied().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0)).collect();
    // Where output pixel `i` of `n` falls between source pixels of `m`: the two and the weight of the second.
    let axis = |i: u32, n: u32, m: usize| {
        let u = ((f64::from(i) + 0.5) * m as f64 / f64::from(n.max(1)) - 0.5).clamp(0.0, m.saturating_sub(1) as f64);
        let a = u.floor() as usize;
        (a, (a + 1).min(m.saturating_sub(1)), (u - a as f64) as f32)
    };
    let at = |x: usize, y: usize| src.get(y * cw + x).copied().unwrap_or(0.0);
    let columns: Vec<_> = (0..size.0).map(|x| axis(x, size.0, cw)).collect();
    let mut out = Vec::with_capacity(size.0 as usize * size.1 as usize);
    for y in 0..size.1 {
        let (y0, y1, fy) = axis(y, size.1, ch);
        out.extend(columns.iter().map(|&(x0, x1, fx)| {
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            top + (bottom - top) * fy
        }));
    }
    out
}

/// How much of each pixel of `crop`, rendered at `size`, the selection covers (0..=1).
fn coverage(selection: &Surface, crop: Rect, size: (u32, u32)) -> Vec<f32> {
    resampled(selection, crop, size.0, size.1)
        .chunks_exact(selection.format().channels().max(1))
        .map(|px| px.first().copied().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0))
        .collect()
}

/// Fade `coverage` (of `crop` at `size`) in from the selection's edge over [`MASK_FADE`] of
/// `selected`'s shorter edge; `open` as [`fade_inwards`].
fn fade_selection(coverage: &mut [f32], selected: Rect, crop: Rect, size: (u32, u32), open: [bool; 4]) {
    let (width, height) = size;
    // The selection's shorter edge, in rendered pixels.
    let (sx, sy) = (f64::from(width) / f64::from(crop.width().max(1)), f64::from(height) / f64::from(crop.height().max(1)));
    let shorter = (f64::from(selected.width()) * sx).min(f64::from(selected.height()) * sy);
    fade_inwards(coverage, width as usize, height as usize, (shorter * MASK_FADE).round().clamp(0.0, 256.0) as usize, open);
}

/// Where the picture sent is empty (`opacity` 0, sent as white) a faded mask has nothing to
/// keep: there the mask is the selection's `whole` coverage again. Otherwise the model keeps
/// the white in the fade, and an outpainting leaves white strips along the picture it extends.
fn unfade_where_empty(faded: &mut [f32], whole: &[f32], opacity: &[f32]) {
    for ((m, w), a) in faded.iter_mut().zip(whole).zip(opacity) {
        *m = m.max(w * (1.0 - a.clamp(0.0, 1.0)));
    }
}

/// `coverage` at `size` as a mask PNG: transparent where the model paints.
fn mask_png(coverage: Vec<f32>, size: (u32, u32)) -> Result<Vec<u8>> {
    let pixels = pixel_count(size)?;
    if coverage.len() != pixels {
        return Err(EngineError::Other("the mask is not the picture's size".into()));
    }
    let mut mask = Vec::with_capacity(pixels * 4);
    for v in coverage {
        mask.extend([0, 0, 0, 255 - byte(v)]);
    }
    png(size.0, size.1, ChannelLayout::Rgba, mask)
}

/// How many more seeds a model that drew its refusal ([`photocraft_aigen::Family::draws_refusal`])
/// is given before the generation gives up.
const REFUSAL_RETRIES: u32 = 3;

/// Whether `bytes` is the picture a model draws in place of an image its training declined: flat
/// mid-grey with the words "Image blocked by safety filter" in light type. Measured on Ideogram 4
/// (1328×752 and a 1443×638 crop): median grey 109, 96–98% of the pixels within ±6 of it, 1–3%
/// light type, and no colour at all, where every real image has under 20% of its pixels that
/// flat and a chroma of 20 or more. Anything that cannot be decoded is not it.
fn is_refusal_picture(bytes: &[u8]) -> bool {
    let Ok(image) = photocraft_codecs::decode(bytes) else { return false };
    let rgba = image.to_rgba8();
    let (pixels, _) = rgba.as_chunks::<4>();
    let total = pixels.len();
    if total == 0 {
        return false;
    }
    let mut histogram = [0u64; 256];
    let mut chroma = 0u64;
    for &[r, g, b, _] in pixels {
        let (r, g, b) = (u64::from(r), u64::from(g), u64::from(b));
        let luma = (r + g + b) / 3;
        if let Some(bin) = histogram.get_mut(luma as usize) {
            *bin += 1;
        }
        chroma += r.abs_diff(g) + g.abs_diff(b);
    }
    let total = total as u64;
    let mut seen = 0u64;
    let median = histogram
        .iter()
        .position(|&n| {
            seen += n;
            seen * 2 >= total
        })
        .unwrap_or(0);
    let within = |lo: usize, hi: usize| histogram.get(lo..=hi.min(255)).map_or(0, |bins| bins.iter().sum::<u64>());
    let flat = within(median.saturating_sub(6), median + 6) as f64 / total as f64;
    let light = within(median + 35, 255) as f64 / total as f64;
    let chroma = chroma as f64 / total as f64;
    (96..=124).contains(&median) && flat >= 0.90 && chroma <= 5.0 && (0.002..=0.08).contains(&light)
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

/// A short name for a model, for layer names: its name in lower case with `_` between the words
/// (`flux_2_klein`), without the words that only tell one file of the model from another
/// (precision, quantisation, parameter count).
pub fn short_model_name(model: &Model) -> String {
    let noise = |w: &str| {
        let digits_then = |suffix: &str| w.strip_suffix(suffix).is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.'));
        let after = |prefix: &str| w.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
        after("fp")
            || after("bf")
            || after("int")
            || after("q")
            || digits_then("b")
            || matches!(w, "scaled" | "convrot" | "mixed" | "e4m3fn" | "safetensors" | "gguf")
    };
    let short = |text: &str| {
        let lower = text.to_lowercase();
        let mut out = String::new();
        for word in lower.split(|c: char| !(c.is_alphanumeric() || c == '.')).map(|w| w.trim_matches('.')).filter(|w| !w.is_empty() && !noise(w)) {
            // Whole words, to a length a layer name has room for.
            if !out.is_empty() && out.chars().count() + 1 + word.chars().count() > 28 {
                break;
            }
            if !out.is_empty() {
                out.push('_');
            }
            out.extend(word.chars().take(28));
        }
        out
    };
    Some(short(&model.name)).filter(|s| !s.is_empty()).or_else(|| Some(short(&model.id)).filter(|s| !s.is_empty())).unwrap_or_else(|| "ai".into())
}

/// A layer name for a generation: the model in brackets, then the prompt's first words, on one
/// line (`[flux_2_klein] a deer eating grass`).
fn layer_name(model: &Model, prompt: &str) -> String {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let start: String = if flat.chars().count() <= 40 { flat } else { flat.chars().take(40).chain(['…']).collect() };
    format!("[{}] {start}", short_model_name(model))
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.generate";
    let prompt = p.get("prompt").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "missing \"prompt\""))?.to_string();
    if prompt.chars().count() > MAX_PROMPT {
        return Err(bad(cmd, format!("the prompt is longer than {MAX_PROMPT} characters")));
    }
    let settings = settings(s);
    let id = p.get("model").and_then(Value::as_str).unwrap_or("");
    // An edit without a model runs the recommended edit model, not the first listed (which may
    // not edit at all).
    let editing = p.get("mode").and_then(Value::as_str) == Some("imgEdit");
    let mut model = if editing { settings.edit_model(id) } else { settings.model(id) }.cloned().ok_or_else(|| {
        if id.is_empty() {
            EngineError::Other("no AI model is set up: add one in Preferences › Generative AI".into())
        } else {
            bad(cmd, format!("no AI model `{id}`"))
        }
    })?;
    let transport = transport(s)?;
    let server = s.ai.server.clone().filter(|_| model.provider.is_local());
    let key = s.ai.key(model.provider);
    s.ai.ready(&model).map_err(EngineError::Other)?;
    let kind = match p.get("mode").map(|m| m.as_str().unwrap_or("?")) {
        None | Some("txt2img") => GenerationMode::Txt2img,
        Some("inpaint") => GenerationMode::Inpaint,
        Some("img2img") => GenerationMode::Img2img,
        Some("imgEdit") => GenerationMode::ImgEdit,
        Some(_) => return Err(bad(cmd, "`mode` must be txt2img|inpaint|img2img|imgEdit")),
    };
    let (inpaint, img2img, edit) = (kind == GenerationMode::Inpaint, kind == GenerationMode::Img2img, kind == GenerationMode::ImgEdit);
    let quality = match p.get("quality") {
        None => GenerationQuality::Standard,
        Some(v) => v.as_str().and_then(GenerationQuality::from_key).ok_or_else(|| bad(cmd, "`quality` must be draft|standard|detail"))?,
    };
    let number = |key: &str, default: f64, max: f64| match p.get(key) {
        None => Ok(default),
        Some(v) => v.as_f64().filter(|x| x.is_finite() && (0.0..=max).contains(x)).ok_or_else(|| bad(cmd, format!("`{key}` must be a number from 0 to {max}"))),
    };
    let strength = number("strength", if img2img { DEFAULT_IMG2IMG_STRENGTH } else { 1.0 }, 1.0)?;
    let context = number("context", DEFAULT_CONTEXT, 4.0)?;
    if edit && !model.can_edit() {
        return Err(EngineError::Other(format!(
            "{} can't edit a picture from an instruction: choose an edit model (Qwen Image 2.1, FIBO 2, Ming Image, Qwen Image Edit, FLUX.2 or a cloud model; the panel's Edit tab lists them)",
            model.name
        )));
    }
    if !inpaint && !edit && !model.can_generate() {
        return Err(EngineError::Other(if model.family.edit_only() {
            format!(
                "{} is an edit model: it changes a picture following an instruction (`\"mode\": \"imgEdit\"`, the panel's Edit tab) and does nothing else",
                model.name
            )
        } else {
            format!(
                "{} is an inpainting model: it repaints a selection (`\"mode\": \"inpaint\"`, the panel's Edit tab) and can't generate from a prompt alone",
                model.name
            )
        }));
    }
    // The references: those asked for, else the ones the document keeps (`ai.setReferences`). A
    // layer of the active document is kept as one, whichever way it was named: a saved file
    // keeps layers of its own document only.
    let references: Vec<(Given, String)> = match (edit, p.get("references")) {
        (false, _) => Vec::new(),
        (true, Some(_)) => references_param(p)?.into_iter().map(|r| (Given::Live(r.source.settled(s)), r.name)).collect(),
        (true, None) => kept_references(s),
    };
    // The prompt says which references go (those it mentions by name, else all), and the model
    // is told them by number.
    let resolved = crate::ai_refs::resolve(&prompt, &references.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>());
    let sent: Vec<&Given> = resolved.sent.iter().filter_map(|i| references.get(*i)).map(|(given, _)| given).collect();
    if sent.len() > model.max_references() {
        return Err(EngineError::Other(format!("{} takes {} reference pictures at most, not {}", model.name, model.max_references(), sent.len())));
    }
    if inpaint && !model.can_inpaint() {
        return Err(EngineError::Other(format!(
            "{} is not set up for inpainting (Preferences \u{203a} Generative AI, its Supports inpainting box)",
            model.name
        )));
    }
    // A mask painted for it (Quick Mask) becomes the selection first: what an inpainting
    // repaints, and what scopes an edit (an empty one: no selection, the whole picture).
    if (inpaint || edit) && s.active().is_some_and(|d| d.doc.quick_mask.is_some()) {
        if inpaint {
            inpaint_blocked(s).map_err(EngineError::Other)?;
        } else {
            // An edit that can't run is refused before the mask is given up.
            let canvas = s.active().ok_or(EngineError::NoDocument)?.doc.bounds();
            let rect = if p.get("rect").is_some() { Some(target_area(p, None, canvas)?) } else { None };
            edit_plan(s, source_param(p)?, context, rect).map_err(EngineError::Other)?;
        }
        leave_quick_mask(s)?;
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
    // What an image-to-image or an edit reads: a layer of this document, or an open document.
    // The source is settled here, so the layer can say which picture it was made from.
    let source = if img2img || edit { source_param(p)?.or_else(|| default_img2img_source(s)).map(|r| r.settled(s)) } else { None };
    let plan = if img2img || edit {
        let rect = if p.get("rect").is_some() { Some(target_area(p, None, d.doc.bounds())?) } else { None };
        let plan = if edit { edit_plan(s, source, context, rect) } else { img2img_plan(s, source, rect) };
        Some(plan.map_err(EngineError::Other)?)
    } else {
        None
    };
    // An edit of a picture of this document by a local model paints only inside the selection
    // ([`edit_mask`]). A cloud model is shown the picture and answers with all of it.
    let here = !matches!(source, Some(SourceRef::Document(id) | SourceRef::DocumentLayer { document: id, .. }) if id != d.doc.id.0);
    let edit_selection = selection.clone().filter(|_| edit && here && model.provider.is_local());
    let canvas = d.doc.bounds();
    // The pictures an edit refers to, as they are now.
    let reference_pictures = sent
        .iter()
        .map(|given| match given {
            Given::Live(r) => reference(s, *r).map(Sent::Read),
            Given::Copy(png) => Ok(Sent::Copy(png.clone())),
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(EngineError::Other)?;
    let area = match (selected, &plan) {
        (_, Some(plan)) => plan.area,
        // The model is shown the layer around the selection, and its answer covers that area.
        (Some(selected), _) if p.get("rect").is_none() => inpaint_area(selected, d.doc.bounds(), context),
        (Some(selected), _) => {
            let area = target_area(p, selection.as_ref(), d.doc.bounds())?;
            if area.intersect(&selected).is_empty() {
                return Err(bad(cmd, "`rect` does not reach the selection"));
            }
            area
        }
        (None, None) => target_area(p, selection.as_ref(), d.doc.bounds())?,
    };
    let fmt = d.doc.pixel_format();
    let seed = match p.get("seed") {
        Some(v) => v.as_u64().ok_or_else(|| bad(cmd, "`seed` must be a non-negative integer"))? & 0xFFFF_FFFF_FFFF,
        None if model.seed >= 0 => model.seed as u64 & 0xFFFF_FFFF_FFFF,
        None => photocraft_aigen::random_seed(),
    };
    // A seed the user asked for stays; only a random one is changed when the model refuses it.
    let reseed = p.get("seed").is_none() && model.seed < 0;
    let alpha = p.get("alpha").and_then(Value::as_bool).unwrap_or(false);
    let masked = p.get("mask").and_then(Value::as_bool).unwrap_or(true);
    let model_prompt = resolved.prompt;
    let mut job = Job { prompt: model_prompt.clone(), width: area.width(), height: area.height(), seed, alpha, source: None };
    let name = layer_name(&model, &prompt);
    let model_id = model.id.clone();
    let label = match kind {
        GenerationMode::Inpaint => INPAINT_LABEL,
        GenerationMode::Img2img => IMG2IMG_LABEL,
        GenerationMode::ImgEdit => EDIT_LABEL,
        GenerationMode::Txt2img => LABEL,
    };
    let mode = kind.key();
    // What the new layer keeps of its generation (`Layer::generation`), for the panel to load back.
    let mut generation = Generation {
        mode: kind,
        model: model.id.clone(),
        model_name: model.name.clone(),
        prompt: prompt.clone(),
        strength,
        context,
        alpha,
        seed,
        source: source.map(Into::into),
        // A copy the document keeps is not a picture the layer can name.
        references: references
            .into_iter()
            .filter_map(|(given, name)| match given {
                Given::Live(source) => Some(GenerationReference { source: source.into(), name }),
                Given::Copy(_) => None,
            })
            .collect(),
        quality,
    };
    // A draft runs part of a local model's schedule, detail a little more of it.
    if model.provider.is_local() {
        model.steps = quality.steps(model.steps);
    }
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
                // A model that is not told where the mask is gets one that fades in, so it
                // composes inside it. The dedicated inpainting models are given the mask, and a
                // cloud model's mask is where it may paint: those get the selection as it is.
                let selected = selected.unwrap_or(area);
                let fade = model.provider.is_local() && !model.family.inpaint_only();
                job.source = Some(inpaint_source(layer, selection, selected, area, size, strength, fade)?);
                ctx.check()?;
            }
            if let Some(plan) = &plan {
                ctx.progress(0.0, "Reading the picture\u{2026}");
                let size = photocraft_aigen::source_size(&model, area.width(), area.height());
                let mut source = match &edit_selection {
                    Some(selection) => plan.masked_source(selection, canvas, size, strength)?,
                    None => plan.source(size, strength)?,
                };
                ctx.check()?;
                if edit {
                    source.edit = true;
                    for picture in &reference_pictures {
                        source.references.push(match picture {
                            Sent::Read(r) => r.png()?,
                            Sent::Copy(png) => png.to_vec(),
                        });
                        ctx.check()?;
                    }
                }
                job.source = Some(source);
            }
            // A local model needs its server: use the one running, or start it.
            if let Some(server) = &server {
                server.ensure(transport.as_ref(), &settings, &control).map_err(ai_error)?;
            }
            let mut refused = 0;
            let image = loop {
                let image =
                    photocraft_aigen::generate(transport.as_ref(), &settings, &model, &key, &job, &control).map_err(|e| lost_server(server.as_ref(), e))?;
                ctx.check()?;
                if !model.family.draws_refusal() || !is_refusal_picture(&image.bytes) {
                    break image;
                }
                // The words are in the picture, not in an error: never put them on a layer. The
                // next seed usually draws the image; a seed the user fixed is theirs to change.
                refused += 1;
                if !reseed || refused > REFUSAL_RETRIES {
                    let what = if reseed { format!("on {refused} seeds in a row") } else { format!("at seed {}", job.seed) };
                    return Err(EngineError::Other(format!(
                        "{} drew its own \"Image blocked by safety filter\" picture {what}. The refusal is part of the model, not a PhotoCraft or ComfyUI filter, and it can fire on harmless prompts: \
                         generate again with another seed, or reword the prompt.",
                        model.name
                    )));
                }
                job.seed = photocraft_aigen::random_seed();
                ctx.progress(0.0, &format!("{} refused that seed; trying another…", model.name));
            };
            ctx.progress(0.98, "Placing the image…");
            // What was given a picture answers in its shape: it goes back where that came from.
            let surface = fit(&image.bytes, fmt, area, kind != GenerationMode::Txt2img)?;
            Ok((surface, image))
        },
        move |s, (surface, image): (Surface, Generated)| {
            let layer = s.edit(label, |doc, active| {
                let mut l = Layer::raster(name, surface.format());
                *crate::pixels_mut(&mut l)? = surface;
                // The seed the image was rendered with (a random one is only known now).
                generation.seed = image.seed;
                l.generation = Some(Box::new(generation));
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
                "mode": mode,
                "seed": image.seed,
                "renderSize": [image.width, image.height],
                // What the model was told: the mentions of references written out.
                "modelPrompt": model_prompt,
                // US dollars, when the provider reported what it billed (OpenAI).
                "cost": image.cost,
            }))
        },
    )
}

/// `ai.sources`: what an image-to-image (or, with `"mode": "imgEdit"`, an edit) can start from now.
fn sources(s: &mut Session, p: &Value) -> Result<Value> {
    let offered = match p.get("mode").map(|m| m.as_str().unwrap_or("?")) {
        None | Some("img2img") => img2img_sources(s),
        Some("imgEdit") => edit_sources(s),
        Some("reference") => reference_sources(s),
        Some(_) => return Err(bad("ai.sources", "`mode` must be img2img|imgEdit|reference")),
    };
    let default = default_img2img_source(s);
    let list: Vec<Value> = offered.into_iter().map(|(source, name)| json!({"source": source, "name": name, "default": Some(source) == default})).collect();
    Ok(json!({"sources": list}))
}

fn models(s: &mut Session, _: &Value) -> Result<Value> {
    let settings = settings(s);
    let listed: Vec<&str> = settings.listed().into_iter().map(|m| m.id.as_str()).collect();
    let list: Vec<Value> = settings
        .models
        .iter()
        .map(|m| {
            let ready = s.ai.ready(m);
            json!({
                "id": m.id, "name": m.name, "provider": m.provider.key(), "local": m.provider.is_local(), "enabled": m.enabled,
                "listed": listed.contains(&m.id.as_str()),
                "alpha": m.alpha, "generate": m.can_generate(), "inpaint": m.can_inpaint(), "edit": m.can_edit(), "references": m.max_references(),
                "ready": ready.is_ok(), "why": ready.err(),
            })
        })
        .collect();
    Ok(json!({"models": list, "listed": listed, "comfyUrl": settings.comfy_url, "available": s.ai.transport.is_some()}))
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
    // `"findPort": true`: a ComfyUI on this computer that is not at the address may be on the
    // other port (the Desktop app's or the portable build's).
    let find_port = p.get("findPort").and_then(Value::as_bool).unwrap_or(false);
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
            let url = if find_port {
                let tried = photocraft_aigen::launch::addresses(&settings.comfy_url);
                let answered = tried.iter().find(|u| photocraft_aigen::launch::is_running(transport.as_ref(), u)).cloned();
                answered.ok_or_else(|| EngineError::Other(format!("ComfyUI is not answering at {}", tried.join(" or "))))?
            } else {
                settings.comfy_url.clone()
            };
            ctx.progress(0.5, "Reading the model files…");
            let folders = photocraft_aigen::comfy_models(transport.as_ref(), &url).map_err(ai_error)?;
            let roots = photocraft_aigen::launch::served_models_folders(transport.as_ref(), &url);
            // Which ComfyUI it is: the Local AI setup says which models it is too old for.
            let version = photocraft_aigen::launch::server_version(transport.as_ref(), &url).unwrap_or_default();
            Ok((folders, roots, started, url, version))
        },
        |_, (folders, roots, started, url, version)| {
            Ok(json!({"folders": folders, "modelsFolders": roots, "startedHere": started, "url": url, "version": version}))
        },
    )
}

/// `ai.server.find`: the launch settings of the ComfyUI that is running now.
fn server_find(s: &mut Session, p: &Value) -> Result<Value> {
    let settings = server_settings(s, p, "ai.server.find")?;
    let transport = transport(s)?;
    let server = s.ai.server.clone().ok_or_else(|| EngineError::Other("starting ComfyUI is not available here: it needs the desktop app".into()))?;
    let launch = server.find(transport.as_ref(), &settings.comfy_url).map_err(ai_error)?;
    let found = serde_json::to_value(&launch).map_err(|e| EngineError::Other(e.to_string()))?;
    // Where a ComfyUI started so keeps its models, as its settings give them.
    #[cfg(not(target_arch = "wasm32"))]
    let found = {
        let mut found = found;
        found["modelsFolders"] = json!(photocraft_aigen::launch::models_folders_here(&launch));
        found
    };
    Ok(found)
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
    Provider::ALL.into_iter().find(|x| x.key() == key && !x.is_local()).ok_or_else(|| bad(cmd, format!("`provider` must be {}", Provider::cloud_keys())))
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

fn can_mask(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".to_string())
}

fn can_generate(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".to_string())?;
    if s.ai.transport.is_none() {
        return Err("AI generation needs the desktop app".into());
    }
    if s.prefs().integrations.ai.listed().is_empty() {
        return Err("no AI model is set up (Preferences › Generative AI)".into());
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
            params: r##"{"prompt":str,"model":id?=first enabled model (imgEdit: Qwen Image 2.1 when it is listed, else the first that edits),"mode":"txt2img|inpaint|img2img|imgEdit"="txt2img" (inpaint repaints the selected part of the active pixel layer: it needs a selection and pixels there; img2img redraws a picture; imgEdit changes a picture following the prompt as an instruction, with a model that edits: with a selection only the part around it is sent and the result is masked by it; a local model paints only inside it),"source":{"layer":id}|{"document":id}?=the active layer when it has pixels, else the first of ai.sources (img2img, imgEdit: a layer is taken in place, over the selection's bounds or else its own pixels; the active document likewise, all layers together; another open document is fitted to the selection's bounds or else the canvas),"references":[{"layer":id}|{"document":id}|{"document":id,"layer":id} (a layer of another open document), each with "name":str? (letters, digits, _ and -; no two alike),…]?=none (imgEdit: up to 3 more pictures the instruction may refer to, as image 2, image 3, … or by name as @Name; when the prompt @mentions any, only those are sent, each mention written out for the model as "Name (image N)"; fewer for some models, see ai.models),"quality":"draft|standard|detail"="standard" (local models: about a quarter of the model's steps, its steps, or a fifth more),"strength":0..1=1, img2img 0.6 (local models: how far the result may depart from what is there),"context":0..4=0.25 (inpaint, imgEdit with a selection: how much of the picture around the selection the model is shown, as a fraction of the selection's longer edge),"alpha":bool=false (transparent background, models that support it),"seed":int?=the model's (random when -1),"rect":[x,y,w,h]?=the selection's bounds (inpaint: grown by the context), else the canvas,"mask":bool=true (the selection becomes the layer mask)} → {layer,bounds,model,mode,seed,renderSize,modelPrompt (the prompt the model was given, mentions written out),cost (USD billed, when the provider reports or publishes it; else null)}; the new layer keeps these settings: its `generation` in the layer tree is {mode,model,modelName,prompt,strength,context,alpha,seed,source,references,quality}"##,
            enabled: can_generate,
            run: generate,
            journal: true,
        },
        CommandSpec {
            id: "ai.mask",
            label: "AI Inpaint Mask",
            menu: &[],
            shortcut: None,
            params: r##"{"paint":bool?,"clear":bool?} → {painting,empty}: paint the area to inpaint, or to scope an edit, instead of selecting it. paint:true enters Quick Mask with the selection as the mask (none: empty) and the tint over the selected part, so paint.stroke {"target":"quickMask","color":"#ffffff"} adds to it and "#000000" takes away; paint:false turns the mask into the selection; clear:true empties the mask (outside the mode: deselects). ai.generate leaves the mode itself, for "mode":"inpaint" and "imgEdit""##,
            enabled: can_mask,
            run: mask,
            journal: true,
        },
        CommandSpec {
            id: "ai.sources",
            label: "AI Image Sources",
            menu: &[],
            shortcut: None,
            params: r##"{"mode":"img2img|imgEdit|reference"="img2img"} → {sources:[{source:{"layer":id}|{"document":id}|{"document":id,"layer":id},name,default}]}: what ai.generate can start from in that mode: the active document's layers that have pixels and the other open documents; for imgEdit the active document itself, all layers together, comes first; reference (what an edit's references are chosen from) adds the layers with pixels of the other open documents, each after its document"##,
            enabled: always,
            run: sources,
            journal: false,
        },
        CommandSpec {
            id: "ai.models",
            label: "AI Models",
            menu: &[],
            shortcut: None,
            params: r##"{} → {models:[{id,name,provider,local,enabled,listed (offered in the AI Gen panel),alpha,generate (text to image),inpaint,edit (imgEdit),references (how many an edit takes),ready,why}],listed:[id,…] (the panel's order: cloud first; the first is the default model),comfyUrl,available}"##,
            enabled: always,
            run: models,
            journal: false,
        },
        CommandSpec {
            id: "ai.comfyModels",
            label: "ComfyUI Model Files",
            menu: &[],
            shortcut: None,
            params: r##"{"url":str?=the ComfyUI address in the preferences,"settings":{…}?=the saved integrations.ai,"start":bool=true,"findPort":bool=false (an address on this computer that doesn't answer: try ComfyUI's other port too, 8000 for ComfyUI Desktop, 8188 for the portable build)} → {folders:{diffusion_models|text_encoders|vae|checkpoints|loras|controlnet:[file,…]},modelsFolders:[folder,…] (where it reads model files, where it saves new ones first; empty when it doesn't say),startedHere,url (the address that answered),version (the ComfyUI version it reports; empty when it doesn't)} (starts ComfyUI first when the settings say so and none is running)"##,
            enabled: always,
            run: comfy_models,
            journal: false,
        },
        CommandSpec {
            id: "ai.server.find",
            label: "Find ComfyUI Installation",
            menu: &[],
            shortcut: None,
            params: r##"{"url":str?} → {python,folder,args,modelsFolders:[folder,…] (where it reads model files, as its settings give them)}: how the ComfyUI running at the address was started (it must be running)"##,
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
            params: r##"{"provider":"openai|gemini|xai|meta|bfl","key":str (empty removes it)} (kept for this session; the app saves it apart from the preferences)"##,
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
