//! AI upscale command. ComfyUI receives only RGB from the original raster surface.

use std::{sync::Arc, time::Duration};

use photocraft_aigen::{Control, Settings, Transport, UpscalePlan, launch};
use photocraft_codecs::{ChannelLayout, Format, Image, SampleType as CodecSample};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, LayerId};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::{
    CommandSpec, EngineError, Result, Session, ai_cmds,
    jobs::{self, JobCtx},
    memory_gate,
};

const ID: &str = "ai.upscale";

impl Session {
    /// Enables only headless Upscale on an endpoint supplied by the process owner. The endpoint
    /// is outside command params and the headless path never starts or installs ComfyUI.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn configure_headless_upscale(&mut self, url: String) {
        self.ai.headless_upscale = Some((url, Arc::new(photocraft_aigen::UreqTransport::new())));
    }
}

pub(crate) fn spec() -> CommandSpec {
    CommandSpec {
        id: ID,
        label: "Upscale Raster Layer",
        menu: &[],
        shortcut: None,
        params: r#"{"scale":2|4,"model":string,"layer":id?} → {job,status,sourceLayer,layer,scale,model,sourceSize,outputSize}"#,
        enabled,
        run,
        journal: true,
    }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    if s.active().is_none() {
        return Err("no document open".into());
    }
    if s.ai.transport.is_none() && s.ai.headless_upscale.is_none() {
        return Err("Upscale needs the desktop network transport or PHOTOCRAFT_UPSCALE_COMFY_URL in a headless process".into());
    }
    Ok(())
}

fn bad(message: impl Into<String>) -> EngineError {
    ai_cmds::bad(ID, message)
}

fn params(s: &Session, p: &Value) -> Result<(LayerId, u32, String)> {
    if let Some(key) = p.as_object().into_iter().flat_map(|obj| obj.keys()).find(|key| !matches!(key.as_str(), "scale" | "model" | "layer")) {
        return Err(bad(format!("unknown parameter {key:?}")));
    }
    let scale = p.get("scale").and_then(Value::as_u64).filter(|n| matches!(n, 2 | 4)).ok_or_else(|| bad("scale must be 2 or 4"))? as u32;
    let model = p
        .get("model")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty() && name.len() <= 512)
        .ok_or_else(|| bad("model must be the exact name of an available ComfyUI upscale model"))?;
    let source = match p.get("layer") {
        Some(value) => LayerId(value.as_u64().ok_or_else(|| bad("layer must be a layer ID in the active document"))?),
        None => {
            let selected = s.active().ok_or(EngineError::NoDocument)?.selected_layers();
            let [layer] = selected.as_slice() else { return Err(bad("select exactly one raster layer")) };
            *layer
        }
    };
    Ok((source, scale, model.into()))
}

fn check_color(doc: &Document) -> Result<()> {
    let srgb = doc
        .icc_profile
        .as_ref()
        .is_none_or(|bytes| crate::color_cmds::profile_from_bytes(bytes).is_ok_and(|profile| profile.same_colors(craft_cms::Builtin::Srgb.profile())));
    if doc.mode != ColorMode::Rgb || doc.depth != SampleType::U8 || !srgb {
        return Err(bad("Upscale currently requires an 8-bit sRGB document; convert the document explicitly before upscaling"));
    }
    Ok(())
}

fn connection(s: &Session) -> Result<(Arc<dyn Transport>, Settings)> {
    let mut settings = ai_cmds::settings(s);
    if let Some((url, transport)) = &s.ai.headless_upscale {
        settings.comfy_url.clone_from(url);
        settings.comfy_autostart = false;
        return Ok((transport.clone(), settings));
    }
    Ok((ai_cmds::transport(s)?, settings))
}

fn server_address(transport: &dyn Transport, settings: &mut Settings, server: Option<&dyn photocraft_aigen::LocalServer>, control: &Control<'_>) -> Result<()> {
    // Only look at the existing well-known local alternatives before an otherwise permitted
    // autostart. The configured endpoint wins even if it lacks nodes or weights.
    if let Some(server) = server.filter(|_| settings.comfy_autostart) {
        let base = photocraft_aigen::comfy::base_url(&settings.comfy_url).map_err(ai_cmds::ai_error)?;
        for address in launch::addresses(&base) {
            if (control.cancelled)() {
                return Err(EngineError::Cancelled);
            }
            if launch::is_running(transport, &address) {
                settings.comfy_url = address;
                return Ok(());
            }
        }
        server.ensure(transport, settings, control).map_err(ai_cmds::ai_error)?;
    }
    Ok(())
}

fn checked_source(s: &Session, source: LayerId) -> Result<(&Surface, Rect)> {
    let (_, pixels, bounds) = s.upscale_source(source)?;
    check_color(&s.active().ok_or(EngineError::NoDocument)?.doc)?;
    if pixels.format().mode != ColorMode::Rgb || pixels.format().sample != SampleType::U8 {
        return Err(bad("Upscale requires an 8-bit RGB raster source"));
    }
    Ok((pixels, bounds))
}

/// Check the same source and full pixel bounds used by the command before offering Upscale.
pub fn source_size(s: &Session, source: LayerId) -> Result<(u32, u32)> {
    let (_, bounds) = checked_source(s, source)?;
    Ok((bounds.width(), bounds.height()))
}

/// Validate model, output size and the engine's memory budget without preparing any pixels.
pub fn output_plan(model: &str, scale: u32, input: (u32, u32)) -> Result<UpscalePlan> {
    let plan = UpscalePlan::new(model, scale, input).map_err(ai_cmds::ai_error)?;
    check_memory(input, plan)?;
    Ok(plan)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (source, scale, model) = params(s, p)?;
    let (pixels, bounds) = checked_source(s, source)?;
    let input = (bounds.width(), bounds.height());
    let plan = output_plan(&model, scale, input)?;
    let pixels = pixels.clone();
    let (transport, mut settings) = connection(s)?;
    let server = s.ai.server.clone();
    let render_model = model.clone();
    jobs::run(
        s,
        "Upscale Raster Layer",
        true,
        move |ctx| {
            let control = Control {
                cancelled: &|| ctx.cancelled(),
                progress: &|fraction, message| ctx.progress(0.12 + 0.61 * fraction, message),
                sleep: &|duration| std::thread::sleep(duration.min(Duration::from_millis(500))),
            };
            server_address(transport.as_ref(), &mut settings, server.as_deref(), &control)?;
            ctx.progress(0.1, "Preparing the raster layer…");
            let (png, alpha) = prepare_rgb(&pixels, bounds, ctx)?;
            let result = photocraft_aigen::run_upscale(transport.as_ref(), &settings, plan, &render_model, &png, &control).map_err(ai_cmds::ai_error)?;
            drop(png);
            ctx.check()?;
            ctx.progress(0.76, "Restoring transparency…");
            let png = restore_rgba(result, &alpha, input, plan.output(), ctx)?;
            Ok((png, ctx.clone()))
        },
        move |s, (png, ctx)| {
            let output = plan.output();
            let layer = s.place_upscaled_raster(source, png, Size::new(output.0, output.1), scale, &ctx)?;
            Ok(json!({
                "job":ctx.id().map(|id| id.0), "status":"done", "sourceLayer":source.0, "layer":layer.0,
                "scale":scale, "model":model, "sourceSize":input, "outputSize":output
            }))
        },
    )
}

fn count((width, height): (u32, u32)) -> Result<usize> {
    let limits = photocraft_codecs::Limits::default();
    limits.check(width, height, ChannelLayout::Rgba, CodecSample::U8).map_err(|e| bad(format!("Upscale image exceeds decoder limits: {e}")))?;
    usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| bad("Upscale pixel count overflows"))
}

fn check_memory(input: (u32, u32), plan: UpscalePlan) -> Result<()> {
    let input = count(input)? as u64;
    let native = count(plan.native())? as u64;
    let output = count(plan.output())? as u64;
    let bytes = input.saturating_mul(24).saturating_add(native.saturating_mul(16)).saturating_add(output.saturating_mul(36));
    memory_gate::ensure("prepare and place the upscaled image", bytes)
}

fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(|_| EngineError::Other("Not enough memory to prepare upscale pixels".into()))?;
    Ok(result)
}

fn encode(size: (u32, u32), layout: ChannelLayout, data: Vec<u8>) -> Result<Vec<u8>> {
    let png = Image::from_u8(size.0, size.1, layout, data)
        .and_then(|image| photocraft_codecs::encode(&image, Format::Png, &Default::default()))
        .map_err(|e| bad(format!("could not encode the upscale PNG: {e}")))?;
    if png.len() as u64 > photocraft_aigen::MAX_RESPONSE_BYTES {
        return Err(bad("Upscale PNG exceeds the transfer and placement budget"));
    }
    Ok(png)
}

fn prepare_rgb(pixels: &Surface, bounds: Rect, ctx: &JobCtx) -> Result<(Vec<u8>, Vec<u8>)> {
    let size = (bounds.width(), bounds.height());
    let n = count(size)?;
    let width = size.0 as usize;
    let mut raw = reserve(width)?;
    raw.resize(width, [0; 4]);
    let mut colors = reserve(n)?;
    let mut alpha = reserve(n)?;
    for y in bounds.y0..bounds.y1 {
        ctx.check()?;
        pixels.read_rgba8_into(Rect::new(bounds.x0, y, bounds.x1, y + 1), &mut raw);
        for &[r, g, b, a] in &raw {
            alpha.push(a);
            colors.push((a != 0).then_some([r, g, b]));
        }
        let start = colors.len() - width;
        if let Some(row) = colors.get_mut(start..) {
            ai_cmds::fill_line(row);
        }
    }
    extend_empty_rows(&mut colors, width, ctx)?;
    let mut rgb = reserve(n.checked_mul(3).ok_or_else(|| bad("RGB buffer overflows"))?)?;
    for color in colors {
        rgb.extend(color.unwrap_or([0; 3]));
    }
    Ok((encode(size, ChannelLayout::Rgb, rgb)?, alpha))
}

fn extend_empty_rows(colors: &mut [Option<[u8; 3]>], width: usize, ctx: &JobCtx) -> Result<()> {
    // A row with any visible pixel is already filled. Nearest covered rows fill the others.
    let mut covered: Vec<Option<usize>> =
        colors.chunks_exact(width).enumerate().map(|(i, row)| row.first().is_some_and(Option::is_some).then_some(i)).collect();
    ai_cmds::fill_line(&mut covered);
    for (i, from) in covered.into_iter().enumerate() {
        ctx.check()?;
        if let Some(from) = from.filter(|&from| from != i) {
            colors.copy_within(from * width..(from + 1) * width, i * width);
        }
    }
    Ok(())
}

fn restore_rgba(png: Vec<u8>, alpha: &[u8], input: (u32, u32), output: (u32, u32), ctx: &JobCtx) -> Result<Vec<u8>> {
    let n = count(output)?;
    if memory_gate::png_dimensions(&png) != Some(output) || png.len() as u64 > photocraft_aigen::MAX_RESPONSE_BYTES {
        return Err(bad("Invalid or wrong-sized upscale output PNG"));
    }
    let image = photocraft_codecs::decode(&png).map_err(|e| bad(format!("could not decode the upscale result: {e}")))?;
    drop(png);
    if image.dimensions() != output || image.layout() != ChannelLayout::Rgb || image.sample_type() != CodecSample::U8 || alpha.len() != count(input)? {
        return Err(bad("Upscale output must be an 8-bit RGB PNG of the exact requested dimensions"));
    }
    let rgb = image.into_data();
    let width = output.0 as usize;
    let mut rgba = reserve(n.checked_mul(4).ok_or_else(|| bad("RGBA buffer overflows"))?)?;
    for (y, row) in rgb.chunks_exact(width * 3).enumerate() {
        ctx.check()?;
        for (x, pixel) in row.chunks_exact(3).enumerate() {
            rgba.extend(pixel);
            rgba.push(sample_alpha(alpha, input, output, x, y));
        }
    }
    encode(output, ChannelLayout::Rgba, rgba)
}

fn sample_alpha(alpha: &[u8], input: (u32, u32), output: (u32, u32), x: usize, y: usize) -> u8 {
    let map = |p: usize, n: u32, m: u32| {
        let v = ((p as f64 + 0.5) * f64::from(n) / f64::from(m) - 0.5).clamp(0.0, f64::from(n - 1));
        (v.floor() as usize, v.ceil() as usize, v.fract())
    };
    let (x0, x1, dx) = map(x, input.0, output.0);
    let (y0, y1, dy) = map(y, input.1, output.1);
    let w = input.0 as usize;
    let row = |y| f64::from(alpha[y * w + x0]) * (1.0 - dx) + f64::from(alpha[y * w + x1]) * dx;
    (row(y0) * (1.0 - dy) + row(y1) * dy).round() as u8
}

#[cfg(test)]
mod tests;
