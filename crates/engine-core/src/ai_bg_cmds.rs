//! AI Gen › Edit › Remove BG: the background of a pixel layer taken away by models on the local
//! engine, without changing a pixel of the layer unless asked to.
//!
//! - `ai.removeBackground` runs as a background job, one undo step. A matting model (BiRefNet,
//!   Lucida, RMBG 3.0) cuts the edge. What it cuts is told four ways (`keep`): the main subject, what is in
//!   the selection (a rough one will do: the model is shown that part and its answer is held to
//!   it), or what SAM 3.1 finds from a description or from clicks. SAM says which object; the
//!   matting model then gets the picture cropped to it, so a small subject is cut at the model's
//!   full size. SAM decides the shape: the matting model's edge is used only where it runs along
//!   SAM's, so what it wrongly took for the subject next to the object does not come back.
//! - The main subject is cut twice when it fills only part of the layer: the model squeezes what
//!   it is shown into a square, so a second look at a crop around the subject keeps more detail.
//! - `ai.removeBackgroundModels` lists the models and which are installed.
//! - `ai.removeBackgroundInstall` downloads one, with the Local AI setup's downloader and the
//!   checksums pinned in its manifest.

use photocraft_aigen::Control;
use photocraft_aigen::bg_removal::{self, BgModel, Target};
use photocraft_algo::matting;
use photocraft_algo::resample::{Resample, crop_surface, resize_surface_in_canvas, translate_surface};
use photocraft_algo::segment::RgbImage;
use photocraft_color::PixelFormat;
use photocraft_doc::{Layer, LayerContent, LayerMask};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::ai_cmds::{ai_error, bad, box_blur_rows, coverage, leave_quick_mask, opaque_png, picture_pixels, settings, transport, transposed};
use crate::commands::{CommandSpec, layer_param};
use crate::{EngineError, Result, Session};

const CMD: &str = "ai.removeBackground";
/// History label, and the job's name.
pub const LABEL: &str = "AI Remove Background";
/// Room left around an object when the picture is cropped to it, as a share of its longer side.
const MARGIN: f64 = 0.08;
/// The least room left around it, in pixels of the document.
const MIN_MARGIN: i32 = 16;
/// How far the matting model's answer may reach past a selection or SAM's mask, as a share of
/// the longer side of what it was shown: hair and fur go past a rough outline.
const REACH: f32 = 0.025;
/// Fewer pixels than this in SAM's answer is nothing found.
const MIN_FOUND: usize = 16;
/// How much of SAM's object the matting model has to cover for its edge to be used.
const MIN_AGREEMENT: f32 = 0.25;
/// How far edge colours are pulled in from the opaque part, in pixels.
const CLEAN_RADIUS: f32 = 4.0;
/// A crop around the main subject is looked at again when it is at most this share of the layer.
const MAX_CLOSER: f64 = 0.85;
/// How much of a click's pixel has to show for it to be on something.
const MIN_SHOWN: f32 = 0.02;
/// The alert for a layer with nothing where it was asked to keep something.
pub const NOTHING_TITLE: &str = "Nothing to remove";
pub const NOTHING_HINT: &str = "Please make sure to select the layer containing the object you'd like to keep.";

/// What to keep.
#[derive(Clone)]
enum Keep {
    Subject,
    Selection(Surface),
    Text(String),
    Points { keep: Vec<[f64; 2]>, drop: Vec<[f64; 2]> },
}

/// Where the result goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    /// A layer mask on the layer: nothing is deleted.
    Mask,
    /// A masked copy above the layer, which is hidden.
    Layer,
    Selection,
}

/// A size no larger than `max` on its longer side, the same shape as `width` × `height`.
fn fitted(width: u32, height: u32, max: u32) -> (u32, u32) {
    let scale = (f64::from(max) / f64::from(width.max(height).max(1))).min(1.0);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max.max(1));
    (side(width), side(height))
}

/// `r` with room around it, inside `within`.
fn padded(r: Rect, within: Rect) -> Rect {
    let margin = ((f64::from(r.width().max(r.height())) * MARGIN).ceil() as i32).max(MIN_MARGIN);
    Rect::new(r.x0.saturating_sub(margin), r.y0.saturating_sub(margin), r.x1.saturating_add(margin), r.y1.saturating_add(margin)).intersect(&within)
}

/// The mask in an image the engine answered with, 0..=1, which must be `size`.
fn read_mask(bytes: &[u8], size: (u32, u32)) -> Result<Vec<f32>> {
    let image = photocraft_codecs::decode(bytes).map_err(|e| EngineError::Other(format!("the engine's mask is not an image PhotoCraft can read ({e})")))?;
    if image.dimensions() != size {
        return Err(EngineError::Other("the engine answered with a mask of another size than the picture".into()));
    }
    Ok(image.to_rgba8().iter().step_by(4).map(|red| f32::from(*red) / 255.0).collect())
}

/// The box around what `mask` (`size`) holds, as (x0, y0, x1, y1), when it holds enough.
fn found_box(mask: &[f32], size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let width = size.0 as usize;
    if width == 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1, mut count) = (usize::MAX, usize::MAX, 0, 0, 0usize);
    for (i, _) in mask.iter().enumerate().filter(|(_, v)| **v >= 0.5) {
        let (x, y) = (i % width, i / width);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        count += 1;
    }
    (count >= MIN_FOUND).then_some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
}

/// `mask` (of the picture of `area`, at `from`) as it falls on `crop` at `to`, nearest pixel.
fn mask_in_crop(mask: &[f32], from: (u32, u32), area: Rect, crop: Rect, to: (u32, u32)) -> Vec<f32> {
    let (fw, fh) = (f64::from(from.0), f64::from(from.1));
    let (aw, ah) = (f64::from(area.width().max(1)), f64::from(area.height().max(1)));
    let (cw, ch) = (f64::from(crop.width()) / f64::from(to.0.max(1)), f64::from(crop.height()) / f64::from(to.1.max(1)));
    let mut out = Vec::with_capacity(to.0 as usize * to.1 as usize);
    for j in 0..to.1 {
        let y = f64::from(crop.y0) + (f64::from(j) + 0.5) * ch;
        let ty = (((y - f64::from(area.y0)) * fh / ah) as usize).min(from.1.saturating_sub(1) as usize);
        for i in 0..to.0 {
            let x = f64::from(crop.x0) + (f64::from(i) + 0.5) * cw;
            let tx = (((x - f64::from(area.x0)) * fw / aw) as usize).min(from.0.saturating_sub(1) as usize);
            out.push(mask.get(ty * from.0 as usize + tx).copied().unwrap_or(0.0));
        }
    }
    out
}

/// A blur of `radius` on each side, even up to the edges of the picture.
fn soft(v: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return v.to_vec();
    }
    let both = |plane: &[f32]| {
        let rows = box_blur_rows(plane, width, radius);
        let columns = box_blur_rows(&transposed(&rows, width, height), height, radius);
        transposed(&columns, height, width)
    };
    let weight = both(&vec![1.0; v.len()]);
    both(v).iter().zip(&weight).map(|(s, w)| if *w > 1e-6 { (s / w).clamp(0.0, 1.0) } else { 0.0 }).collect()
}

/// `mask` grown by `radius`, its new edge softened: how far a matting model's answer may go.
fn grown(mask: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let spread = |v: &[f32], w: usize| -> Vec<f32> { box_blur_rows(v, w, radius).into_iter().map(|x| f32::from(x > 1e-3)).collect() };
    let hard: Vec<f32> = mask.iter().map(|v| f32::from(*v > 0.02)).collect();
    let rows = spread(&hard, width);
    let columns = spread(&transposed(&rows, width, height), height);
    soft(&transposed(&columns, height, width), width, height, radius / 2)
}

/// How far the answer of a model shown `size` may reach past the outline it was held to.
fn reach(size: (u32, u32)) -> usize {
    ((size.0.max(size.1) as f32 * REACH).ceil() as usize).max(3)
}

/// The matte held to what it may reach of `outline` (all of the picture, without one).
fn held(matte: &[f32], outline: &[f32], size: (u32, u32)) -> Vec<f32> {
    let limit = grown(outline, size.0 as usize, size.1 as usize, reach(size));
    matte.iter().zip(&limit).map(|(m, l)| m * l).collect()
}

/// Which pixels are within `radius` of the edge of the hard `mask`.
fn edge_band(mask: &[f32], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let outer = matting::morph(mask, width, height, radius, true);
    let inner = matting::morph(mask, width, height, radius, false);
    outer.iter().zip(&inner).map(|(o, i)| o - i > 0.5).collect()
}

/// The hard `object` with its edge moved onto the picture's own: SAM answers in coarse steps.
fn snapped(object: &[f32], picture: &RgbImage) -> Vec<f32> {
    let (width, height) = (picture.w, picture.h);
    let radius = (width.max(height) / 256).clamp(2, 8);
    let band = edge_band(object, width, height, radius);
    let fitted = matting::guided_filter_color(picture, object, radius, matting::EPS);
    object.iter().zip(&fitted).zip(&band).map(|((o, f), near)| if *near { f.clamp(0.0, 1.0) } else { *o }).collect()
}

/// The result for an object SAM found, and whether the matting model's edge is in it. SAM says
/// the shape. Near its edge the matte is used where the two run together; where they part by more
/// than a few strands of hair, the matte has taken something else for the subject (what stands on
/// the object, what is behind it) and SAM's edge is kept. Multiplying the two instead leaves a
/// blurred band of that something else all along the outline.
fn cut_object(matte: &[f32], object: &[f32], picture: &RgbImage, size: (u32, u32)) -> (Vec<f32>, bool) {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let hard: Vec<f32> = object.iter().map(|v| f32::from(*v >= 0.5)).collect();
    if picture.px.len() != hard.len() || matte.len() != hard.len() {
        return (hard, false);
    }
    let base = snapped(&hard, picture);
    let total: f32 = hard.iter().sum();
    let shared: f32 = matte.iter().zip(&hard).map(|(m, o)| m.min(*o)).sum();
    if total <= 0.0 || shared / total < MIN_AGREEMENT {
        return (base, false);
    }
    let band_radius = reach(size);
    let strand = (band_radius / 3).max(2);
    let differ: Vec<f32> = matte.iter().zip(&hard).map(|(m, o)| f32::from((*m >= 0.5) != (*o >= 0.5))).collect();
    let wide = matting::morph(&matting::morph(&differ, width, height, strand, false), width, height, strand + 2, true);
    let parted = matting::gaussian_blur(&wide, width, height, strand as f32 / 2.0);
    let band = edge_band(&hard, width, height, band_radius);
    let out = (0..hard.len())
        .map(|i| {
            if band[i] {
                let trust = 1.0 - parted[i].clamp(0.0, 1.0);
                trust * matte[i] + (1.0 - trust) * base[i]
            } else {
                hard[i]
            }
        })
        .collect();
    (out, true)
}

/// A box of a picture of `area` sent at `size`, in document pixels.
fn box_in_layer(found: (u32, u32, u32, u32), size: (u32, u32), area: Rect) -> Rect {
    let (sx, sy) = (f64::from(area.width()) / f64::from(size.0.max(1)), f64::from(area.height()) / f64::from(size.1.max(1)));
    let low = |v: u32, scale: f64, from: i32| from.saturating_add((f64::from(v) * scale).floor() as i32);
    let high = |v: u32, scale: f64, from: i32| from.saturating_add((f64::from(v) * scale).ceil() as i32);
    Rect::new(low(found.0, sx, area.x0), low(found.1, sy, area.y0), high(found.2, sx, area.x0), high(found.3, sy, area.y0))
}

/// A crop around the subject in `matte` (of `crop`, at `size`), when it is enough smaller to be
/// worth a second look.
fn closer_crop(matte: &[f32], size: (u32, u32), crop: Rect) -> Option<Rect> {
    let closer = padded(box_in_layer(found_box(matte, size)?, size, crop), crop);
    let whole = f64::from(crop.width()) * f64::from(crop.height());
    (!closer.is_empty() && whole > 0.0 && f64::from(closer.width()) * f64::from(closer.height()) / whole <= MAX_CLOSER).then_some(closer)
}

/// Does `layer` show anything at this point of the document?
fn shows_at(layer: &Layer, area: Rect, x: f64, y: f64) -> bool {
    if x < f64::from(area.x0) || x >= f64::from(area.x1) || y < f64::from(area.y0) || y >= f64::from(area.y1) {
        return false;
    }
    let Some(surface) = layer.surface() else { return false };
    let (px, py) = (x.floor() as i32, y.floor() as i32);
    let at = Rect::new(px, py, px.saturating_add(1), py.saturating_add(1));
    let alpha = picture_pixels(surface, at, (1, 1)).ok().and_then(|p| p.first().map(|q| q[3])).unwrap_or(0.0);
    let mask = layer.mask.as_ref().filter(|m| m.enabled).map_or(1.0, |m| m.value(px, py));
    alpha * mask > MIN_SHOWN
}

/// A layer of pixels with none of them on the canvas.
fn is_blank(layer: &Layer, canvas: Rect) -> bool {
    layer.surface().is_some_and(|s| s.format().alpha && s.default_pixel().last().is_some_and(|a| *a <= 0.0) && s.content_bounds().intersect(&canvas).is_empty())
}

fn nothing_there() -> EngineError {
    EngineError::Other(format!("{NOTHING_TITLE}. {NOTHING_HINT}"))
}

/// Is the active layer empty, or, with clicks to `keep`, empty at every one of them? (Usually
/// another layer is the one that was meant.)
pub fn nothing_to_remove(s: &Session, keep: &[[f64; 2]]) -> bool {
    let Some(d) = s.active() else { return false };
    let Some(layer) = d.active_layer.and_then(|id| d.doc.layer(id)).filter(|l| l.surface().is_some()) else { return false };
    let canvas = d.doc.bounds();
    let area = crate::cutout_cmds::subject_area(layer, canvas, false);
    is_blank(layer, canvas) || (!keep.is_empty() && !keep.iter().any(|[x, y]| shows_at(layer, area, *x, *y)))
}

/// `mask` (at `size`) as a mask of the document: stretched onto `crop`, hiding all around it.
fn mask_surface(mask: &[f32], size: (u32, u32), crop: Rect) -> Surface {
    let mut local = Surface::new(PixelFormat::GRAY8);
    let at = Rect::new(0, 0, size.0 as i32, size.1 as i32);
    local.write_region(at, mask);
    let (sx, sy) = (f64::from(crop.width()) / f64::from(size.0.max(1)), f64::from(crop.height()) / f64::from(size.1.max(1)));
    let scaled = if (sx - 1.0).abs() > 1e-9 || (sy - 1.0).abs() > 1e-9 { resize_surface_in_canvas(&local, sx, sy, Resample::Bicubic, at) } else { local };
    let mut out = crop_surface(&translate_surface(&scaled, crop.x0, crop.y0), crop);
    out.prune();
    out
}

fn keep_param(s: &mut Session, p: &Value, id: photocraft_doc::LayerId, area: Rect) -> Result<Keep> {
    match p.get("keep").map(|k| k.as_str().unwrap_or("?")).unwrap_or("subject") {
        "subject" => Ok(Keep::Subject),
        "selection" => {
            // A mask painted with the panel's brush is the selection from here on.
            if s.active().is_some_and(|d| d.doc.quick_mask.is_some()) {
                leave_quick_mask(s)?;
            }
            let selection = s.active().and_then(|d| d.doc.selection.clone());
            let selection = selection.filter(|sel| !sel.content_bounds().intersect(&area).is_empty());
            selection.map(Keep::Selection).ok_or_else(|| EngineError::Other("Select what to keep first: nothing of the layer is selected".into()))
        }
        "describe" => {
            let typed = p.get("prompt").and_then(Value::as_str).unwrap_or("");
            let phrase = bg_removal::object_phrase(typed);
            if phrase.is_empty() {
                return Err(bad(CMD, "`keep: describe` needs a `prompt` naming what to keep (\"couple\", \"woman in red\")"));
            }
            Ok(Keep::Text(phrase))
        }
        "points" => {
            let list = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`keep: points` needs `points`: [{\"x\",\"y\",\"keep\"},…]"))?;
            if list.len() > bg_removal::MAX_POINTS {
                return Err(bad(CMD, format!("at most {} `points`", bg_removal::MAX_POINTS)));
            }
            let layer = s.active().and_then(|d| d.doc.layer(id)).ok_or(EngineError::NoLayer(id))?;
            let (mut keep, mut drop, mut missed) = (Vec::new(), Vec::new(), false);
            for point in list {
                let at = |key: &str| point.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
                let (Some(x), Some(y)) = (at("x"), at("y")) else { return Err(bad(CMD, "each point needs a number `x` and `y`")) };
                let shows = shows_at(layer, area, x, y);
                match point.get("keep").map(Value::as_bool) {
                    None | Some(Some(true)) if shows => keep.push([x, y]),
                    None | Some(Some(true)) => missed = true,
                    // Off the layer there is nothing to leave out.
                    Some(Some(false)) if shows => drop.push([x, y]),
                    Some(Some(false)) => {}
                    Some(None) => return Err(bad(CMD, "a point's `keep` must be true or false")),
                }
            }
            if keep.is_empty() {
                return Err(if missed { nothing_there() } else { bad(CMD, "click at least once on what to keep") });
            }
            Ok(Keep::Points { keep, drop })
        }
        _ => Err(bad(CMD, "`keep` must be subject|selection|describe|points")),
    }
}

/// `points` of the document as pixels of the picture of `area` sent at `size`.
fn sent_points(points: &[[f64; 2]], area: Rect, size: (u32, u32)) -> Vec<[u32; 2]> {
    let (sx, sy) = (f64::from(size.0) / f64::from(area.width().max(1)), f64::from(size.1) / f64::from(area.height().max(1)));
    let last = |v: u32| f64::from(v.saturating_sub(1));
    points.iter().map(|[x, y]| [((x - f64::from(area.x0)) * sx).clamp(0.0, last(size.0)) as u32, ((y - f64::from(area.y0)) * sy).clamp(0.0, last(size.1)) as u32]).collect()
}

/// What a cut-out made.
struct Cut {
    mask: Surface,
    /// The layer's pixels with their edge colours cleaned, when that was asked for.
    cleaned: Option<Surface>,
    /// The matting model cut the edge (not SAM's own mask).
    matted: bool,
}

/// Everything the job needs, read before it starts.
struct Plan {
    pixels: Surface,
    area: Rect,
    keep: Keep,
    model: &'static BgModel,
    clean: bool,
}

/// The work of a cut-out: ask SAM what to keep when it is to be asked, then the matting model.
fn cut(transport: &dyn photocraft_aigen::Transport, base: &str, plan: &Plan, ctx: &crate::jobs::JobCtx) -> Result<Cut> {
    let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m), sleep: &|d| std::thread::sleep(d) };
    let picture = |crop: Rect, side: u32| -> Result<(Vec<u8>, (u32, u32))> {
        let size = fitted(crop.width(), crop.height(), side);
        Ok((opaque_png(&picture_pixels(&plan.pixels, crop, size)?, size, &[])?, size))
    };
    // What the matting model is sent: at the size it renders, or as large as it is worth.
    let for_matte = |crop: Rect| -> Result<(Vec<u8>, (u32, u32))> {
        let size = bg_removal::render_size(plan.model, crop.width(), crop.height()).unwrap_or_else(|| fitted(crop.width(), crop.height(), bg_removal::MATTE_SIDE));
        Ok((opaque_png(&picture_pixels(&plan.pixels, crop, size)?, size, &[])?, size))
    };
    let area = plan.area;
    // SAM's answer, for a description or clicks.
    let target = match &plan.keep {
        Keep::Text(_) | Keep::Points { .. } => {
            ctx.progress(0.05, "Finding what to keep…");
            let (png, size) = picture(area, bg_removal::TARGET_SIDE)?;
            let answer = match &plan.keep {
                Keep::Text(phrase) => bg_removal::locate(transport, base, &png, &Target::Text(phrase), &control),
                Keep::Points { keep, drop } => {
                    let (keep, drop) = (sent_points(keep, area, size), sent_points(drop, area, size));
                    bg_removal::locate(transport, base, &png, &Target::Points { keep: &keep, drop: &drop }, &control)
                }
                _ => return Err(EngineError::Other("nothing to look for".into())),
            };
            let mask = read_mask(&answer.map_err(ai_error)?, size)?;
            let Some((x0, y0, x1, y1)) = found_box(&mask, size) else {
                return Err(EngineError::Other(match &plan.keep {
                    Keep::Text(phrase) => format!("SAM 3.1 found no \u{201c}{phrase}\u{201d} in the layer: try another word for it, or click on it"),
                    _ => "SAM 3.1 found nothing at those clicks: click on the middle of the object".to_string(),
                }));
            };
            let found = box_in_layer((x0, y0, x1, y1), size, area);
            Some((mask, size, found))
        }
        _ => None,
    };
    ctx.check()?;
    let mut crop = match (&plan.keep, &target) {
        (_, Some((_, _, found))) => padded(*found, area),
        (Keep::Selection(selection), _) => padded(selection.content_bounds().intersect(&area), area),
        _ => area,
    };
    if crop.is_empty() {
        return Err(EngineError::Other("nothing of the layer to cut out there".into()));
    }
    ctx.progress(0.4, &format!("Cutting the edge with {}…", plan.model.name));
    let (png, mut size) = for_matte(crop)?;
    let mut matte = read_mask(&bg_removal::matte(transport, base, plan.model, &png, size, &control).map_err(ai_error)?, size)?;
    ctx.check()?;
    // The model squeezes what it is shown into a square: closer in, the subject keeps more detail.
    if let (Keep::Subject, Some(closer)) = (&plan.keep, closer_crop(&matte, size, crop)) {
        ctx.progress(0.65, &format!("Refining the edge with {}…", plan.model.name));
        let (png, closer_size) = for_matte(closer)?;
        let again = read_mask(&bg_removal::matte(transport, base, plan.model, &png, closer_size, &control).map_err(ai_error)?, closer_size)?;
        ctx.check()?;
        if again.iter().any(|v| *v >= 0.5) {
            (crop, size, matte) = (closer, closer_size, again);
        }
    }
    ctx.progress(0.9, "Making the mask…");
    let (mask, matted) = match (&plan.keep, &target) {
        (_, Some((object, from, _))) => {
            let colours = picture_pixels(&plan.pixels, crop, size)?;
            let guide = RgbImage { w: size.0 as usize, h: size.1 as usize, px: colours.iter().map(|p| [p[0], p[1], p[2]]).collect() };
            cut_object(&matte, &mask_in_crop(object, *from, area, crop, size), &guide, size)
        }
        (Keep::Selection(selection), _) => (held(&matte, &coverage(selection, crop, size), size), true),
        _ => (matte, true),
    };
    if !mask.iter().any(|v| *v >= 0.5) {
        return Err(EngineError::Other(format!("{} found no subject there: select it more closely, or describe it", plan.model.name)));
    }
    let mask = mask_surface(&mask, size, crop);
    ctx.check()?;
    let cleaned = plan.clean.then(|| matting::decontaminate_surface(&plan.pixels, &mask, CLEAN_RADIUS, 100.0));
    Ok(Cut { mask, cleaned, matted })
}

/// Where a cut-out reads from: the pixels of an unlocked pixel layer inside the canvas.
fn target_layer<'a>(s: &'a Session, p: &Value) -> Result<(&'a Layer, Rect)> {
    let id = layer_param(s, p)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let layer = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    crate::cutout_cmds::check(doc, layer).map_err(EngineError::Other)?;
    let area = crate::cutout_cmds::subject_area(layer, doc.bounds(), false);
    if area.is_empty() || is_blank(layer, doc.bounds()) {
        return Err(nothing_there());
    }
    Ok((layer, area))
}

fn remove_background(s: &mut Session, p: &Value) -> Result<Value> {
    let model_id = p.get("model").map_or(Some(bg_removal::DEFAULT_MATTE), Value::as_str).unwrap_or("?");
    let model = bg_removal::find_matte(model_id).ok_or_else(|| bad(CMD, "`model` must be birefnet|lucida|rmbg-3"))?;
    let output = match p.get("output").map(|o| o.as_str().unwrap_or("?")).unwrap_or("mask") {
        "mask" => Output::Mask,
        "layer" => Output::Layer,
        "selection" => Output::Selection,
        _ => return Err(bad(CMD, "`output` must be mask|layer|selection")),
    };
    let clean = match p.get("decontaminate") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(CMD, "`decontaminate` must be true or false"))?,
    };
    if clean && output == Output::Selection {
        return Err(bad(CMD, "`decontaminate` makes new pixels: use output mask or layer"));
    }
    let transport = transport(s)?;
    let (id, area, pixels) = {
        let (layer, area) = target_layer(s, p)?;
        let pixels = layer.surface().cloned().ok_or_else(|| EngineError::Other("the layer has no pixels".into()))?;
        (layer.id, area, pixels)
    };
    let keep = keep_param(s, p, id, area)?;
    let keep_name = match keep {
        Keep::Subject => "subject",
        Keep::Selection(_) => "selection",
        Keep::Text(_) => "describe",
        Keep::Points { .. } => "points",
    };
    let phrase = if let Keep::Text(text) = &keep { Some(text.clone()) } else { None };
    refresh_engine_paths(s)?;
    let ai = settings(s);
    let server = s.ai.server.clone();
    let plan = Plan { pixels, area, keep, model, clean };
    crate::jobs::run(
        s,
        LABEL,
        true,
        move |ctx| {
            if let Some(server) = &server {
                ctx.progress(0.01, "Starting the local engine…");
                let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f.min(0.04), m), sleep: &|d| std::thread::sleep(d) };
                server.ensure(transport.as_ref(), &ai, &control).map_err(ai_error)?;
            }
            cut(transport.as_ref(), &ai.comfy_url, &plan, ctx)
        },
        move |s, done: Cut| {
            let bounds = done.mask.content_bounds();
            let depth = s.active().ok_or(EngineError::NoDocument)?.doc.depth;
            // Cleaned colours are new pixels: they go on a layer of their own, as Select and Mask does.
            let to_layer = output == Output::Layer || done.cleaned.is_some();
            let Cut { mask, cleaned, matted } = done;
            let layer = s.edit(LABEL, |doc, active| {
                if output == Output::Selection {
                    doc.selection = Some(mask.clone());
                    return Ok(id);
                }
                let layer_mask = crate::commands::mask_at_depth(LayerMask { surface: mask.clone(), ..LayerMask::reveal_all() }, depth)?;
                doc.selection = None;
                if !to_layer {
                    crate::extra_cmds::background_to_layer_for_mask(doc, id);
                    doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.mask = Some(layer_mask);
                    return Ok(id);
                }
                let source = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
                let content = match cleaned.clone() {
                    Some(pixels) => pixels,
                    None => source.surface().cloned().ok_or_else(|| EngineError::Other("the layer has no pixels".into()))?,
                };
                let name = doc.copy_name(&source.name);
                let mut copy = Layer::new(name, LayerContent::Raster(content));
                copy.mask = Some(layer_mask);
                let new = doc.insert_above(Some(id), copy);
                if let Some(l) = doc.layer_mut(id) {
                    l.visible = false;
                }
                *active = Some(new);
                Ok(new)
            })?;
            Ok(json!({
                "layer": layer.0,
                "bounds": [bounds.x0, bounds.y0, bounds.width(), bounds.height()],
                "model": model.id,
                "keep": keep_name,
                "phrase": phrase,
                "output": if to_layer { "layer" } else if output == Output::Selection { "selection" } else { "mask" },
                // false: SAM's own mask, since the matting model took something else for the subject.
                "matted": matted,
            }))
        },
    )
}

/// An engine installed before background removal was added was told where its models are without
/// this folder: its list is written again, and a running one is stopped so the next job starts it
/// reading the new one.
#[cfg(not(target_arch = "wasm32"))]
fn refresh_engine_paths(s: &mut Session) -> Result<()> {
    use photocraft_aisetup::{plan, runtime, state};
    let record = s.ai_setup.record();
    let (engine_dir, models_dir) = (record.choices.engine_dir.trim().to_string(), record.choices.models_dir.trim().to_string());
    if record.phase != state::Phase::Done || record.choices.route != plan::Route::Managed || engine_dir.is_empty() || models_dir.is_empty() {
        return Ok(());
    }
    let yaml = runtime::model_paths_yaml(std::path::Path::new(&models_dir), &record.choices.extra_models());
    let stale: Vec<std::path::PathBuf> = runtime::installed_engines(std::path::Path::new(&engine_dir))
        .into_iter()
        .map(|(root, _)| root.join(plan::MODEL_PATHS))
        .filter(|path| std::fs::read_to_string(path).ok().as_deref() != Some(yaml.as_str()))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    let running = s.ai.server.clone().filter(|server| server.state() == photocraft_aigen::ServerState::Started);
    if running.is_some() && s.has_jobs() {
        return Err(EngineError::Other("The engine is busy. Try again when what is running has finished.".into()));
    }
    for path in stale {
        std::fs::write(&path, &yaml).map_err(|e| EngineError::Other(format!("Could not write `{}`: {e}", path.display())))?;
    }
    if let Some(server) = running {
        server.stop();
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn refresh_engine_paths(_: &mut Session) -> Result<()> {
    Ok(())
}

/// Is this file in `folder` (or its older name) of one of `dirs`, or among those the engine
/// listed? With `size`, only at that size: a file shared with other models may be another
/// publisher's copy of the same weights, so it is known by its name.
#[cfg(not(target_arch = "wasm32"))]
fn has_file(folder: &str, file: &str, size: Option<u64>, dirs: &[std::path::PathBuf], server: Option<&std::collections::BTreeMap<String, Vec<String>>>) -> bool {
    let older = photocraft_aisetup::manifest::LEGACY_FOLDERS.iter().filter(|(now, _)| *now == folder).map(|(_, old)| *old);
    std::iter::once(folder).chain(older).any(|name| {
        let here = dirs.iter().any(|d| std::fs::metadata(d.join(name).join(file)).is_ok_and(|m| m.is_file() && size.is_none_or(|s| m.len() == s)));
        here || server.and_then(|f| f.get(name)).is_some_and(|names| names.iter().any(|n| n == file))
    })
}

/// Is `model` ready to run: its file, and those it shares with other models?
#[cfg(not(target_arch = "wasm32"))]
fn on_disk(model: &BgModel, dirs: &[std::path::PathBuf], server: Option<&std::collections::BTreeMap<String, Vec<String>>>) -> bool {
    has_file(model.folder, model.file, Some(model.size), dirs, server) && model.shared.iter().all(|f| has_file(f.folder, f.file, None, dirs, server))
}

/// How many bytes `model` still has to download.
#[cfg(not(target_arch = "wasm32"))]
fn left_to_fetch(model: &BgModel, dirs: &[std::path::PathBuf], server: Option<&std::collections::BTreeMap<String, Vec<String>>>) -> u64 {
    let own = if has_file(model.folder, model.file, Some(model.size), dirs, server) { 0 } else { model.size };
    own + model.shared.iter().filter(|f| !has_file(f.folder, f.file, None, dirs, server)).map(|f| f.size).sum::<u64>()
}

/// Which of the models are installed, by id. It reads the disk: not for every frame.
#[cfg(not(target_arch = "wasm32"))]
pub fn installed(s: &Session) -> Vec<(&'static str, bool)> {
    let (dirs, server) = (crate::ai_enhance_cmds::model_dirs(s), s.ai_setup.record().choices.server_files);
    bg_removal::models().iter().map(|m| (m.id, on_disk(m, &dirs, server.as_ref()))).collect()
}

/// What each model still has to download, by id. It reads the disk: not for every frame.
#[cfg(not(target_arch = "wasm32"))]
pub fn to_download(s: &Session) -> Vec<(&'static str, u64)> {
    let (dirs, server) = (crate::ai_enhance_cmds::model_dirs(s), s.ai_setup.record().choices.server_files);
    bg_removal::models().iter().map(|m| (m.id, left_to_fetch(m, &dirs, server.as_ref()))).collect()
}

/// What each model has to download, by id: all of it in the browser.
#[cfg(target_arch = "wasm32")]
pub fn to_download(_: &Session) -> Vec<(&'static str, u64)> {
    bg_removal::models().iter().map(|m| (m.id, m.size + m.shared.iter().map(|f| f.size).sum::<u64>())).collect()
}

/// Which of the models are installed, by id: none in the browser.
#[cfg(target_arch = "wasm32")]
pub fn installed(_: &Session) -> Vec<(&'static str, bool)> {
    bg_removal::models().iter().map(|m| (m.id, false)).collect()
}

fn list_models(s: &mut Session, _: &Value) -> Result<Value> {
    let have = installed(s);
    let models: Vec<Value> = bg_removal::models()
        .iter()
        .map(|m| {
            let on = have.iter().any(|(id, yes)| *id == m.id && *yes);
            let kind = if m.id == bg_removal::TARGET_MODEL.id { "target" } else { "matte" };
            json!({"id": m.id, "name": m.name, "role": m.role, "kind": kind, "file": m.file, "folder": m.folder, "size": m.size,
                "licence": m.licence, "licenceUrl": m.licence_url, "page": m.page, "installed": on})
        })
        .collect();
    Ok(json!({"models": models, "default": bg_removal::DEFAULT_MATTE}))
}

#[cfg(not(target_arch = "wasm32"))]
fn install(s: &mut Session, p: &Value) -> Result<Value> {
    use photocraft_aisetup::download::{self, Spec};
    let cmd = "ai.removeBackgroundInstall";
    let ids = bg_removal::models().map(|m| m.id).join("|");
    let id = p.get("model").and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("missing `model`: {ids}")))?;
    let model = bg_removal::find(id).ok_or_else(|| bad(cmd, format!("`model` must be {ids}")))?;
    let env = s.ai_setup.env.clone().ok_or_else(|| EngineError::Other(crate::ai_setup_cmds::NEEDS_DESKTOP.into()))?;
    let manifest = match env.manifest.clone() {
        Some(m) => m,
        None => crate::ai_setup_cmds::bundled()?,
    };
    let dirs = crate::ai_enhance_cmds::model_dirs(s);
    let Some(own) = dirs.first().cloned() else {
        return Err(EngineError::Other("Set up Local AI first (File \u{203a} Set Up Local AI): it gives the engine its models folder".into()));
    };
    let server = s.ai_setup.record().choices.server_files;
    if on_disk(model, &dirs, server.as_ref()) {
        return Ok(json!({"model": model.id, "installed": true, "downloaded": false}));
    }
    // Its own file and what it shares with other models, without what is there already.
    let mut wanted = Vec::new();
    if !has_file(model.folder, model.file, Some(model.size), &dirs, server.as_ref()) {
        wanted.push((model.folder, model.file));
    }
    wanted.extend(model.shared.iter().filter(|f| !has_file(f.folder, f.file, None, &dirs, server.as_ref())).map(|f| (f.folder, f.file)));
    let mut files = Vec::with_capacity(wanted.len());
    for (folder, name) in wanted {
        let file = manifest.files.get(&format!("{folder}/{name}")).cloned().ok_or_else(|| EngineError::Other(format!("{name} of {} has no download pinned in this build", model.name)))?;
        files.push((own.join(folder).join(name), file));
    }
    let path = own.join(model.folder).join(model.file).display().to_string();
    let token = s.ai.hf_token();
    crate::jobs::run(
        s,
        "Install Background Removal Model",
        false,
        move |ctx| {
            let total: u64 = files.iter().map(|(_, f)| f.size).sum();
            let human = photocraft_aisetup::human_bytes;
            let dl = download::Ctx { fetch: env.fetch.as_ref(), cancelled: &|| ctx.cancelled(), sleep: &|d| std::thread::sleep(d), free_space: &download::free_space };
            ctx.progress(0.0, &format!("Downloading {}\u{2026}", model.name));
            let mut arrived = 0u64;
            for (dest, file) in &files {
                if let Some(dir) = dest.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| EngineError::Other(format!("Could not create `{}`: {e}", dir.display())))?;
                }
                let spec = Spec { url: &file.url, size: file.size, sha256: &file.sha256, token: (!token.is_empty() && file.gated).then_some(token.as_str()) };
                let progress = |n: u64| {
                    let so_far = arrived.saturating_add(n);
                    ctx.progress(so_far as f32 / total.max(1) as f32, &format!("Downloading {}\u{2026} {} of {}", model.name, human(so_far), human(total)));
                };
                download::download(&dl, &spec, dest, &progress).map_err(|e| match e {
                    photocraft_aisetup::Error::Cancelled => EngineError::Cancelled,
                    e => EngineError::Other(e.to_string()),
                })?;
                arrived = arrived.saturating_add(file.size);
            }
            Ok(path)
        },
        move |_, path| Ok(json!({"model": model.id, "installed": true, "downloaded": true, "path": path})),
    )
}

#[cfg(target_arch = "wasm32")]
fn install(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("Installing models needs the desktop app".into()))
}

/// Why this session can't cut out its active layer now, or `Ok`.
pub fn can_cut(s: &Session) -> std::result::Result<(), String> {
    if s.ai.transport.is_none() {
        return Err("background removal needs the desktop app".into());
    }
    let d = s.active().ok_or("no document open")?;
    crate::cutout_cmds::check(&d.doc, crate::active_layer_of(s)?)
}

fn can_install(s: &Session) -> std::result::Result<(), String> {
    if s.ai.transport.is_none() {
        return Err("installing models needs the desktop app".into());
    }
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: CMD,
            label: "AI Remove Background",
            menu: &[],
            shortcut: None,
            params: r##"{"layer":id?,"keep":"subject|selection|describe|points"="subject" (subject: what the matting model takes for the main subject; selection: what is in the selection, a rough one will do; describe: what SAM 3.1 finds from `prompt`; points: what it finds at `points`),"prompt":str? (for describe: a short name, "couple", "woman in red"),"points":[{"x":px,"y":px,"keep":bool=true},…]? (for points, in document pixels, at most 32, at least one to keep),"model":"birefnet|lucida|rmbg-3"="birefnet" (cuts the edge: fast photo cut-outs, graphics, logos and glass, or RMBG 3.0 for the finest hair and soft detail, CC BY-NC 4.0),"output":"mask|layer|selection"="mask" (mask: a layer mask, nothing is deleted; layer: a masked copy above, the layer hidden),"decontaminate":bool=false (cleans the background's colour out of the edge: new pixels, so the result is a layer)} → {layer,bounds,model,keep,phrase,output,matted (false: SAM's own mask was used)}: runs on the local engine, which is started if need be. A background job (cancellable), one undo step. Models: ai.removeBackgroundModels"##,
            enabled: can_cut,
            run: remove_background,
            journal: false,
        },
        CommandSpec {
            id: "ai.removeBackgroundModels",
            label: "Background Removal Models",
            menu: &[],
            shortcut: None,
            params: r##"{} → {models:[{id,name,role,kind ("matte": cuts the edge; "target": finds what to keep),file,folder,size (bytes),licence,licenceUrl,page,installed}],default}"##,
            enabled: |_| Ok(()),
            run: list_models,
            journal: false,
        },
        CommandSpec {
            id: "ai.removeBackgroundInstall",
            label: "Install Background Removal Model",
            menu: &[],
            shortcut: None,
            params: r##"{"model":"birefnet|lucida|rmbg-3|sam-3-1"} → {model,installed:true,downloaded (false: it was there already),path?}: downloads a model, and any file it shares with other models that is missing, into the engine's models folder with the Local AI setup's downloader: the file is pinned with its checksum, and a stopped download resumes. A background job (cancellable)"##,
            enabled: can_install,
            run: install,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
