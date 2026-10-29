//! A change by instruction's references: the pictures the active document keeps for its edits
//! to refer to (`ai.references`, saved with it in PSD and `.pcraft` files), shown as chips under
//! the source and named as the prompt calls them. A chip named Luna is `@Luna` in the prompt,
//! which shows the mention in the accent colour while it names a chip; typing `@` offers the
//! names (Tab takes the first). When the prompt mentions any chip only the mentioned ones are
//! sent (the others show faint); a prompt that mentions none sends them all, as image 2, image 3,
//! … (the engine's [`photocraft_engine::ai_refs`]).
//!
//! A click on a chip opens its name and its picture under the chips; its × takes it out; + adds
//! one, named after the picture it starts with. A picture is a layer of this document or the
//! document itself (read when an edit runs), or another open document or a layer of one, which
//! the document keeps a copy of. Every change is `ai.setReferences`: one History step, and the
//! document has changes to save.

use std::collections::HashMap;
use std::ops::Range;

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use photocraft_doc::{AiPicture, AiReference};
use photocraft_engine::Session;
use photocraft_engine::ai_cmds::SourceRef;
use photocraft_engine::ai_refs::{self, MAX_NAME};
use serde_json::{Value, json};

use super::{edit_label, edit_source, tr};
use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// `source` with a layer of the active document named by its document too
/// ([`SourceRef::DocumentLayer`]), as the panel's lists name it.
pub(super) fn absolute(s: &Session, source: SourceRef) -> SourceRef {
    match (source, s.active()) {
        (SourceRef::Layer(layer), Some(d)) => SourceRef::DocumentLayer { document: d.doc.id.0, layer },
        (other, _) => other,
    }
}

/// The pictures a reference can be now, each named by [`absolute`], with its label.
pub(super) fn offered(s: &Session) -> Vec<(SourceRef, String)> {
    photocraft_engine::ai_cmds::reference_sources(s).into_iter().map(|(r, label)| (absolute(s, r), label)).collect()
}

/// The references the active document keeps.
pub(super) fn kept(s: &Session) -> Vec<AiReference> {
    s.active().map(|d| d.doc.ai_references.clone()).unwrap_or_default()
}

/// The pictures a kept reference is, as [`offered`] names it (`None`: a copy, which is no
/// picture that is open).
fn source_of(s: &Session, picture: &AiPicture) -> Option<SourceRef> {
    let here = s.active()?.doc.id.0;
    match picture {
        AiPicture::Layer(layer) => Some(SourceRef::DocumentLayer { document: here, layer: *layer }),
        AiPicture::Document => Some(SourceRef::Document(here)),
        AiPicture::Image { .. } => None,
    }
}

/// The rows the references take: the chips, and the name and picture of the one open.
pub(super) fn rows(editing: Option<usize>, count: usize) -> usize {
    1 + if editing.is_some_and(|i| i < count) { 2 } else { 0 }
}

/// `name`, or with a number after it when one of `taken` has it already (Luna, Luna2, …), within
/// [`MAX_NAME`]. Empty stays empty.
pub(super) fn unique(name: &str, taken: &[&str]) -> String {
    let clash = |n: &str| taken.iter().any(|t| t.to_lowercase() == n.to_lowercase());
    if name.is_empty() || !clash(name) {
        return name.to_string();
    }
    (2..100)
        .map(|k| {
            let suffix = k.to_string();
            let stem: String = name.chars().take(MAX_NAME.saturating_sub(suffix.len())).collect();
            format!("{stem}{suffix}")
        })
        .find(|n| !clash(n))
        .unwrap_or_default()
}

/// What a chip's picture does with the prompt as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sent {
    /// The prompt mentions it: it is sent.
    Mentioned,
    /// The prompt mentions none: every reference is sent.
    All,
    /// The prompt mentions others: this one stays back.
    No,
}

/// What each reference named `names` does with `prompt`, and the image number each sent one is
/// to the model.
pub(super) fn sent(prompt: &str, names: &[&str]) -> Vec<(Sent, Option<usize>)> {
    let resolved = ai_refs::resolve(prompt, names);
    let any = !ai_refs::mentions(prompt, names).is_empty();
    (0..names.len())
        .map(|i| {
            let number = resolved.sent.iter().position(|s| *s == i).map(|p| p + 2);
            let state = match (any, number) {
                (false, _) => Sent::All,
                (true, Some(_)) => Sent::Mentioned,
                (true, None) => Sent::No,
            };
            (state, number)
        })
        .collect()
}

/// A reference as `ai.setReferences` is asked for it: one the document keeps (by index), or a
/// picture to take.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Keep(usize, String),
    New(SourceRef, String),
}

impl Entry {
    fn name(&self) -> &str {
        match self {
            Entry::Keep(_, name) | Entry::New(_, name) => name,
        }
    }

    fn json(&self) -> Value {
        match self {
            Entry::Keep(i, name) => json!({"keep": i, "name": name}),
            Entry::New(source, name) => {
                let mut v = json!(source);
                v["name"] = json!(name);
                v
            }
        }
    }
}

/// Set the active document's references to `entries` (`ai.setReferences`); `coalesce` merges a
/// run of changes (a name being typed) into one History step. A refusal is a notice.
pub(super) fn set(app: &mut PhotocraftApp, entries: &[Entry], coalesce: Option<String>) -> bool {
    let mut params = json!({"references": entries.iter().map(Entry::json).collect::<Vec<_>>()});
    if let Some(key) = coalesce {
        params["coalesce"] = json!(key);
    }
    match app.run("ai.setReferences", params) {
        Ok(_) => true,
        Err(e) => {
            crate::notices::error(app, format!("{}: {e}", tr("References")));
            false
        }
    }
}

/// The document's references kept as they are, with `change` (an index and what goes there:
/// past the end, added).
fn keeping(refs: &[AiReference], change: Option<(usize, Entry)>) -> Vec<Entry> {
    let mut out: Vec<Entry> = refs.iter().enumerate().map(|(i, r)| Entry::Keep(i, r.name.clone())).collect();
    if let Some((i, entry)) = change {
        match out.get_mut(i) {
            Some(slot) => *slot = entry,
            None => out.push(entry),
        }
    }
    out
}

/// The chips, with + to add one, and under them the name and picture of the one open.
pub(super) fn chips(app: &mut PhotocraftApp, ui: &mut egui::Ui, busy: bool, max: usize) {
    let t = Tokens::get(ui.ctx());
    let offered = offered(&app.session);
    let refs = kept(&app.session);
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    let states = sent(&app.ui.ai_gen.prompt, &names);
    let mut open = app.ui.ai_gen.editing_ref.filter(|i| *i < refs.len());
    let mut remove = None;
    let mut add = false;
    let thumbs = thumbs(&app.session, ui.ctx(), &refs);
    let has_doc = app.session.active().is_some();
    ui.add_enabled_ui(!busy && has_doc, |ui| {
        ui.horizontal(|ui| {
            edit_label(ui, tr("Refs"));
            ui.spacing_mut().item_spacing.x = 4.0;
            // The chips share the row (with room for + while there is one to add): a long name is
            // shortened rather than pushing the panel wider.
            let addable = refs.len() < max && !offered.is_empty();
            let plus = if addable { 22.0 + 4.0 } else { 0.0 };
            let each = ((ui.available_width() - plus) / refs.len().max(1) as f32 - 4.0).max(40.0);
            for (i, r) in refs.iter().enumerate() {
                let (state, number) = states.get(i).copied().unwrap_or((Sent::All, None));
                let label = if r.name.is_empty() { format!("{} {}", tr("Image"), number.unwrap_or(i + 2)) } else { format!("@{}", r.name) };
                let picture = photocraft_engine::ai_refs_cmds::describe(&app.session, &r.picture);
                let (body, close) = chip(ui, &label, thumbs.get(i).copied().flatten(), state, open == Some(i), each);
                if close {
                    remove = Some(i);
                } else if body.clicked() {
                    open = if open == Some(i) { None } else { Some(i) };
                }
                body.on_hover_text(chip_tip(&r.name, &picture, state, number));
            }
            if addable
                && crate::icons::button(ui, "plus", 22.0, false, tr("Add a picture to refer to: a layer, a document, or a layer of another open document"))
                    .clicked()
            {
                add = true;
            }
            if refs.is_empty() {
                ui.add(egui::Label::new(RichText::new(tr("Characters, objects, places…")).color(t.text_faint).size(super::NOTE_SIZE)).truncate());
            }
        });
    });
    if let Some(i) = remove {
        let entries: Vec<Entry> = keeping(&refs, None).into_iter().enumerate().filter(|(j, _)| *j != i).map(|(_, e)| e).collect();
        if set(app, &entries, None) {
            open = match open {
                Some(o) if o == i => None,
                Some(o) if o > i => Some(o - 1),
                o => o,
            };
        }
    }
    if add {
        // A picture not in use yet, and not the one being changed (nor this whole document, which
        // holds it), first.
        let changing = edit_source(app).map(|s| absolute(&app.session, s));
        let here = app.session.active().map(|d| SourceRef::Document(d.doc.id.0));
        let used: Vec<SourceRef> = refs.iter().filter_map(|r| source_of(&app.session, &r.picture)).collect();
        let unused = |o: &SourceRef| Some(*o) != changing && !used.contains(o);
        let candidates = offered.iter().map(|(o, _)| *o);
        let fresh = candidates.clone().find(|o| unused(o) && Some(*o) != here).or_else(|| candidates.clone().find(|o| unused(o)));
        if let Some(source) = fresh.or_else(|| offered.first().map(|(o, _)| *o)) {
            let name = unique(&photocraft_engine::ai_cmds::reference_name(&app.session, source), &names);
            if set(app, &keeping(&refs, Some((refs.len(), Entry::New(source, name)))), None) {
                open = Some(refs.len());
            }
        }
    }
    if let Some(i) = open {
        ui.add_space(6.0);
        if !editor(app, ui, i, &offered, busy) {
            open = None;
        }
    }
    ui.add_space(6.0);
    app.ui.ai_gen.editing_ref = open;
}

/// What a chip's tooltip says: the picture, and what the prompt does with it.
fn chip_tip(name: &str, picture: &str, state: Sent, number: Option<usize>) -> String {
    let n = number.map_or_else(String::new, |n| n.to_string());
    let what = match (state, name.is_empty()) {
        (Sent::Mentioned, _) => format!("{} {n}: {} @{name}.", tr("Sent as image"), tr("the prompt mentions")),
        (Sent::All, false) => format!("{} {n}. {} @{name} {}", tr("Sent as image"), tr("Type"), tr("in the prompt to call it by name.")),
        (Sent::All, true) => format!("{} {n}. {}", tr("Sent as image"), tr("Name it to call it by name in the prompt.")),
        (Sent::No, false) => format!("{} @{name} {}", tr("Not sent: the prompt mentions other references. Type"), tr("to send it too.")),
        (Sent::No, true) => tr("Not sent: the prompt mentions references by name. Name this one to use it.").to_string(),
    };
    format!("{picture}\n{what}\n{}", tr("Kept with the document. Click to rename it or choose another picture; × takes it out."))
}

/// One chip, `max` wide at most: the picture's thumbnail, the name (shortened with … to fit) and
/// a × at its end. Returns the chip's response and whether the × was clicked.
fn chip(ui: &mut egui::Ui, label: &str, thumb: Option<egui::TextureId>, state: Sent, open: bool, max: f32) -> (egui::Response, bool) {
    const H: f32 = 22.0;
    const THUMB: f32 = 16.0;
    const CLOSE: f32 = 14.0;
    let t = Tokens::get(ui.ctx());
    let color = match state {
        Sent::Mentioned => t.accent,
        Sent::All => t.text,
        Sent::No => t.text_faint,
    };
    // Short of room the thumbnail goes first, so the name keeps a few letters.
    let thumbed = max >= 3.0 + THUMB + 5.0 + 3.0 + CLOSE + 3.0 + 34.0;
    let lead = if thumbed { 3.0 + THUMB + 5.0 } else { 6.0 };
    let frame = lead + 3.0 + CLOSE + 3.0;
    let font = egui::FontId::proportional(12.0);
    // Without its thumbnail the chip is short of room: the name keeps the letters the @ took.
    let label = if thumbed { label } else { label.strip_prefix('@').unwrap_or(label) };
    let chars = label.chars().count();
    // 14 characters at most, fewer when the room is short.
    let shown = |keep: usize| if keep >= chars { label.to_string() } else { label.chars().take(keep).chain(['…']).collect() };
    let mut keep = chars.min(13 + usize::from(chars == 14));
    let mut galley = ui.painter().layout_no_wrap(shown(keep), font.clone(), color);
    while keep > 1 && frame + galley.size().x > max {
        keep -= 1;
        galley = ui.painter().layout_no_wrap(shown(keep), font.clone(), color);
    }
    let width = frame + galley.size().x;
    let (rect, body) = ui.allocate_exact_size(vec2(width, H), Sense::click());
    let close_rect = Rect::from_center_size(pos2(rect.right() - 3.0 - CLOSE / 2.0, rect.center().y), vec2(CLOSE, CLOSE));
    let close = ui.interact(close_rect, body.id.with("x"), Sense::click()).on_hover_text(tr("Take it out"));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let radius = t.radius_sm.max(3.0);
        let fill = if open {
            t.accent_soft
        } else if body.hovered() {
            t.hover
        } else {
            t.field
        };
        p.rect_filled(rect, radius, fill);
        let edge = if open || state == Sent::Mentioned { t.accent } else { t.field_border };
        p.rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        if thumbed {
            let thumb_rect = Rect::from_min_size(pos2(rect.left() + 3.0, rect.center().y - THUMB / 2.0), vec2(THUMB, THUMB));
            match thumb {
                Some(tex) => {
                    widgets::checker(p, thumb_rect, 4.0);
                    p.image(tex, thumb_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => crate::icons::paint(ui, thumb_rect, "image", 13.0, t.icon),
            }
        }
        let text_pos = pos2(rect.left() + lead, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, color);
        if close.hovered() {
            ui.painter().rect_filled(close_rect, CLOSE / 2.0, t.hover);
        }
        crate::icons::paint(ui, close_rect, "x", 10.0, if close.hovered() { t.text } else { t.text_dim });
    }
    let clicked = close.clicked();
    (body, clicked)
}

/// The open chip's name and picture, as two of the panel's labelled rows. `false` when it was
/// closed (✓).
fn editor(app: &mut PhotocraftApp, ui: &mut egui::Ui, i: usize, offered: &[(SourceRef, String)], busy: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let refs = kept(&app.session);
    let Some(r) = refs.get(i).cloned() else { return false };
    let others: Vec<&str> = refs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, r)| r.name.as_str()).collect();
    let mut keep = true;
    let mut change: Option<(Entry, Option<String>)> = None;
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            edit_label(ui, tr("Name"));
            let mut text = r.name.clone();
            let id = egui::Id::unique(("ai-gen-ref-name", i));
            let width = (ui.available_width() - 30.0).max(60.0);
            let field = widgets::text_field_ex(ui, id, &mut text, tr("e.g. Luna"), width, false, Some(MAX_NAME + 4))
                .on_hover_text(tr("The name the prompt calls it by: type @ and the name. Letters, digits, _ and -"));
            if field.changed() {
                // A space typed is a `_` (names are one word); what a name can't hold is left out.
                let name = ai_refs::clean_name(&text.replace(char::is_whitespace, "_"));
                // A name another reference has is shown, not set: the document can't keep two.
                if name != r.name && !others.iter().any(|o| !name.is_empty() && o.to_lowercase() == name.to_lowercase()) {
                    change = Some((Entry::Keep(i, name), Some(format!("ai-ref-name-{i}"))));
                }
            }
            if crate::icons::button(ui, "check", 22.0, false, tr("Done")).clicked() || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                keep = false;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // A long picture name is cut short, not let to push the panel wider.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            edit_label(ui, tr("Picture"));
            // A copy is no picture that is open: it is listed first, as what the chip has.
            let copy = matches!(r.picture, AiPicture::Image { .. }).then(|| photocraft_engine::ai_refs_cmds::describe(&app.session, &r.picture));
            let current = source_of(&app.session, &r.picture);
            let mut options: Vec<(Option<SourceRef>, &str)> = copy.iter().map(|label| (None, label.as_str())).collect();
            options.extend(offered.iter().map(|(s, label)| (Some(*s), label.as_str())));
            let mut chosen = current;
            if widgets::dropdown_literal(ui, &format!("ai-gen-ref-picture-{i}"), crate::i18n::t("Picture"), &mut chosen, &options, (ui.available_width() - 2.0).max(80.0))
                && chosen != current
                && let Some(source) = chosen
            {
                // A name that was the picture's own follows the new picture.
                let was = current.map(|c| photocraft_engine::ai_cmds::reference_name(&app.session, c)).unwrap_or_default();
                let name = if r.name.is_empty() || r.name == was {
                    unique(&photocraft_engine::ai_cmds::reference_name(&app.session, source), &others)
                } else {
                    r.name.clone()
                };
                change = Some((Entry::New(source, name), None));
            }
        })
        .response
        .on_hover_text(tr(
            "The picture: a layer, this document, another open document or a layer of one. Another document's picture is copied into this one, so it is kept with it",
        ));
    });
    if let Some((entry, coalesce)) = change {
        let old = r.name.clone();
        let new = entry.name().to_string();
        if set(app, &keeping(&refs, Some((i, entry))), coalesce) {
            // The prompt's mentions follow the name.
            app.ui.ai_gen.prompt = ai_refs::rename(&app.ui.ai_gen.prompt, &old, &new);
        }
    }
    if refs.iter().enumerate().any(|(j, o)| j != i && !r.name.is_empty() && o.name.to_lowercase() == r.name.to_lowercase()) {
        ui.label(RichText::new(tr("Another reference has this name")).color(t.danger).size(super::NOTE_SIZE));
    }
    keep
}

/// The cached thumbnails of the pictures the references show, by what they show.
type ThumbCache = HashMap<String, (u64, f64, TextureHandle)>;

/// A thumbnail for each of `refs` (`None`: none to show).
fn thumbs(s: &Session, ctx: &egui::Context, refs: &[AiReference]) -> Vec<Option<egui::TextureId>> {
    const SIDE: usize = 32;
    let id = egui::Id::unique("ai-gen-ref-thumbs");
    let now = ctx.input(|i| i.time);
    let mut cache: ThumbCache = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let keys: Vec<String> = refs.iter().map(|r| thumb_key(&r.picture)).collect();
    cache.retain(|k, _| keys.contains(k));
    let out = refs
        .iter()
        .zip(&keys)
        .map(|(r, cache_key)| {
            let (key, image) = thumb_source(s, &r.picture)?;
            if let Some((k, made, tex)) = cache.get(cache_key) {
                // A document is composited anew at most once a second while it changes.
                if *k == key || (matches!(image, Thumb::Document(_)) && now - made < 1.0) {
                    return Some(tex.id());
                }
            }
            let img = image.render(SIDE)?;
            match cache.get_mut(cache_key) {
                Some((k, made, tex)) => {
                    tex.set(img, egui::TextureOptions::LINEAR);
                    (*k, *made) = (key, now);
                    Some(tex.id())
                }
                None => {
                    let tex = ctx.load_texture(format!("ai-gen-ref-{cache_key}"), img, egui::TextureOptions::LINEAR);
                    let tid = tex.id();
                    cache.insert(cache_key.clone(), (key, now, tex));
                    Some(tid)
                }
            }
        })
        .collect();
    ctx.data_mut(|d| d.insert_temp(id, cache));
    out
}

/// What a thumbnail of `picture` is cached under.
fn thumb_key(picture: &AiPicture) -> String {
    match picture {
        AiPicture::Layer(id) => format!("layer-{id}"),
        AiPicture::Document => "document".into(),
        // The same copy, the same bytes: kept in an `Arc` until it is replaced.
        AiPicture::Image { png, .. } => format!("copy-{:p}", std::sync::Arc::as_ptr(png)),
    }
}

/// The pixels a thumbnail shows.
enum Thumb<'a> {
    Layer(&'a photocraft_raster::Surface, photocraft_geom::Rect),
    Document(&'a photocraft_doc::Document),
    Png(&'a [u8]),
}

impl Thumb<'_> {
    fn render(&self, side: usize) -> Option<egui::ColorImage> {
        let square = |w: usize, h: usize, rgba: &[u8]| {
            let mut px = vec![Color32::TRANSPARENT; side * side];
            let (ox, oy) = (side.saturating_sub(w) / 2, side.saturating_sub(h) / 2);
            for (i, &[r, g, b, a]) in rgba.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (i % w.max(1) + ox, i / w.max(1) + oy);
                if x < side
                    && let Some(out) = px.get_mut(y * side + x)
                {
                    *out = Color32::from_rgba_unmultiplied(r, g, b, a);
                }
            }
            egui::ColorImage::new([side, side], px)
        };
        match self {
            Thumb::Layer(px, frame) => {
                let mut v = [0.0f32; 8];
                let n = px.channels().min(v.len());
                Some(crate::app::thumb_image(*frame, side, |x, y| {
                    let Some(buf) = v.get_mut(..n) else { return [0.0; 4] };
                    px.read_pixel(x, y, buf);
                    photocraft_raster::to_rgba(&px.format(), buf)
                }))
            }
            Thumb::Document(doc) => {
                let comp = photocraft_compose::thumbnail(doc, side as u32);
                Some(square(comp.width as usize, comp.height as usize, &comp.pixels))
            }
            Thumb::Png(bytes) => {
                // Scaled to fit the square by sampling; a picture that does not decode has none.
                let img = photocraft_codecs::decode(bytes).ok()?;
                let (w, h) = (img.width() as usize, img.height() as usize);
                if w == 0 || h == 0 {
                    return None;
                }
                let rgba = img.to_rgba8();
                let scale = (w.max(h) as f32 / side as f32).max(1.0);
                let (tw, th) = (((w as f32 / scale) as usize).clamp(1, side), ((h as f32 / scale) as usize).clamp(1, side));
                let mut small = Vec::with_capacity(tw * th * 4);
                for y in 0..th {
                    for x in 0..tw {
                        let (sx, sy) = (((x as f32 + 0.5) * scale) as usize, ((y as f32 + 0.5) * scale) as usize);
                        let at = (sy.min(h - 1) * w + sx.min(w - 1)) * 4;
                        small.extend(rgba.get(at..at + 4).unwrap_or(&[0, 0, 0, 0]));
                    }
                }
                Some(square(tw, th, &small))
            }
        }
    }
}

/// What the thumbnail of `picture` shows, and a key that changes when that does: the layer's
/// pixels, the document's revision, or the copy (which does not change).
fn thumb_source<'a>(s: &'a Session, picture: &'a AiPicture) -> Option<(u64, Thumb<'a>)> {
    let d = s.active()?;
    match picture {
        AiPicture::Layer(id) => {
            let px = d.doc.layer(photocraft_doc::LayerId(*id))?.surface()?;
            let frame = px.content_bounds().intersect(&d.doc.bounds());
            Some((crate::surface_fingerprint(px), Thumb::Layer(px, frame)))
        }
        AiPicture::Document => Some((d.revision, Thumb::Document(&d.doc))),
        AiPicture::Image { png, .. } => Some((0, Thumb::Png(png))),
    }
}

// ---------- The prompt ----------

/// The prompt laid out with its mentions of `names` in `mention`, on a tint of it.
pub(super) fn prompt_job(text: &str, names: &[&str], font: egui::FontId, color: Color32, mention: Color32, wrap: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping { max_width: wrap, ..Default::default() }, ..Default::default() };
    let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let marked = egui::TextFormat { font_id: font, color: mention, background: mention.gamma_multiply(0.18), ..Default::default() };
    let mut from = 0;
    for m in ai_refs::mentions(text, names) {
        job.append(text.get(from..m.range.start).unwrap_or(""), 0.0, plain.clone());
        job.append(text.get(m.range.clone()).unwrap_or(""), 0.0, marked.clone());
        from = m.range.end;
    }
    job.append(text.get(from..).unwrap_or(""), 0.0, plain);
    job
}

/// The `@` being typed before the cursor (`cursor`, in characters) and the names it may become:
/// the bytes of `@` and what follows it up to the cursor, and the indices of the names that
/// begin with that, but not a name typed out already. `None` when the cursor is not after one.
pub(super) fn completion_at(text: &str, cursor: usize, names: &[&str]) -> Option<(Range<usize>, Vec<usize>)> {
    let end = text.char_indices().nth(cursor).map_or(text.len(), |(i, _)| i);
    let before = text.get(..end)?;
    let (at, partial) = before.rfind('@').map(|at| (at, before.get(at + 1..).unwrap_or("")))?;
    if !partial.chars().all(ai_refs::name_char) || before.get(..at).and_then(|b| b.chars().next_back()).is_some_and(ai_refs::name_char) {
        return None;
    }
    // A name continues past the cursor: the word is not being typed at its end.
    if text.get(end..).and_then(|r| r.chars().next()).is_some_and(ai_refs::name_char) {
        return None;
    }
    let lower = partial.to_lowercase();
    let fits: Vec<usize> =
        names.iter().enumerate().filter(|(_, n)| !n.is_empty() && n.to_lowercase().starts_with(&lower) && n.to_lowercase() != lower).map(|(i, _)| i).collect();
    (!fits.is_empty()).then_some((at..end, fits))
}

/// `text` with `range` (an `@` being typed) replaced by `@name` and a space, and the cursor's
/// place after them, in characters.
pub(super) fn complete(text: &str, range: Range<usize>, name: &str) -> (String, usize) {
    let (head, tail) = (text.get(..range.start).unwrap_or(""), text.get(range.end..).unwrap_or(""));
    let space = if tail.starts_with(char::is_whitespace) { "" } else { " " };
    let inserted = format!("@{name}{space}");
    let cursor = head.chars().count() + inserted.chars().count();
    (format!("{head}{inserted}{tail}"), cursor)
}

/// The prompt's id, so a completion can put the cursor after the name it typed.
pub(super) fn prompt_id() -> egui::Id {
    egui::Id::unique("ai-gen-prompt-text")
}

/// A completion to apply to the prompt: from Tab on the frame it was offered, or a click on it.
#[derive(Clone, Default)]
struct Pending {
    range: Range<usize>,
    name: String,
}

fn pending_id() -> egui::Id {
    prompt_id().with("complete")
}

fn offered_id() -> egui::Id {
    prompt_id().with("offered")
}

/// Are names being offered for an `@` in the prompt? Then Tab takes the first, and the prompt
/// keeps the focus (its event filter takes Tab).
pub(super) fn offering(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Pending>(offered_id())).is_some()
}

/// Before the prompt is drawn: apply a completion clicked last frame, or one Tab takes now (the
/// key is the completion's, not focus'), putting the cursor after it.
pub(super) fn before_prompt(ui: &egui::Ui, prompt: &mut String) {
    let ctx = ui.ctx();
    let mut pending: Option<Pending> = ctx.data_mut(|d| d.remove_temp(pending_id()));
    let focused = ctx.memory(|m| m.has_focus(prompt_id()));
    if pending.is_none()
        && focused
        && let Some(first) = ctx.data(|d| d.get_temp::<Pending>(offered_id()))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
    {
        pending = Some(first);
    }
    let Some(p) = pending else { return };
    if prompt.get(p.range.clone()).is_none_or(|s| !s.starts_with('@')) {
        return;
    }
    let (text, cursor) = complete(prompt, p.range, &p.name);
    *prompt = text;
    let mut state = egui::TextEdit::load_state(ctx, prompt_id()).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(cursor))));
    state.store(ctx, prompt_id());
    ctx.memory_mut(|m| m.request_focus(prompt_id()));
}

/// After the prompt is drawn: offer the names the `@` before the cursor may become, under it.
pub(super) fn after_prompt(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, prompt: &str, names: &[&str]) {
    let ctx = ui.ctx();
    let cursor = output.cursor_range.filter(|r| r.is_empty() && output.response.has_focus()).map(|r| r.primary.index.0);
    let found = cursor.and_then(|c| completion_at(prompt, c, names).map(|f| (c, f)));
    let Some((c, (range, fits))) = found else {
        ctx.data_mut(|d| d.remove::<Pending>(offered_id()));
        return;
    };
    let first = fits.first().and_then(|i| names.get(*i)).map(|n| Pending { range: range.clone(), name: n.to_string() });
    ctx.data_mut(|d| match first {
        Some(p) => {
            d.insert_temp(offered_id(), p);
        }
        None => d.remove::<Pending>(offered_id()),
    });
    let t = Tokens::get(ctx);
    let at = output.galley_pos + output.galley.pos_from_cursor(egui::text::CCursor::new(c)).left_bottom().to_vec2();
    egui::Area::new(prompt_id().with("names")).order(egui::Order::Foreground).fixed_pos(at + vec2(0.0, 2.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
            for (k, i) in fits.iter().enumerate() {
                let Some(name) = names.get(*i) else { continue };
                let text = RichText::new(format!("@{name}")).color(t.accent);
                let label = if k == 0 { format!("{}  ", tr("Tab")) } else { String::new() };
                let row = ui.horizontal(|ui| {
                    let r = ui.add(egui::Button::new(text).frame(false));
                    if !label.is_empty() {
                        ui.label(RichText::new(label).color(t.text_faint).size(super::NOTE_SIZE));
                    }
                    r
                });
                if row.inner.clicked() {
                    ctx.data_mut(|d| d.insert_temp(pending_id(), Pending { range: range.clone(), name: name.to_string() }));
                    ctx.request_repaint();
                }
            }
        });
    });
}
