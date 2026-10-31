//! Create › Regions: the scene laid out in boxes, for models trained on boxes
//! ([`photocraft_aigen::Model::layout`]). The boxes are the document's
//! ([`photocraft_doc::Document::ai_regions`]); every change goes through the engine's
//! `ai.setRegions` and `ai.region.set`, so it is a History step and agents see the same.
//!
//! - **In the panel** ([`section`]): `Regions (N)` with help, copy (what the model is sent),
//!   clear and add; one row per region, front to back. Dragging a row changes its depth; the
//!   pencil opens its type, words, colour and fill; the bin removes it.
//! - **On the canvas** ([`canvas`], [`pointer`]): while the tab is in front the boxes are drawn
//!   over the picture, and the pointer edits them: drag on empty canvas to draw one, drag a box
//!   to move it, a corner to resize it, double-click to type its description (a new box opens
//!   for typing at once). Opening the tab picks the Move tool for this; picking any other tool
//!   hands the pointer back to it (the boxes stay in view until the tab is left).

use egui::{Color32, Pos2, Rect, RichText, Stroke, vec2};
use photocraft_doc::{AiRegion, AiRegionKind};
use serde_json::{Value, json};

use super::{CreateTask, Mode, tr};
use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::state::Tool;
use crate::theme::Tokens;

/// How near a corner (screen pixels) a press grabs it to resize.
const HANDLE_REACH: f32 = 7.0;
/// The smallest drag (screen pixels) that draws a box; less is a click.
const MIN_DRAG: f32 = 6.0;
/// A row of the list.
const ROW_HEIGHT: f32 = 24.0;

const HELP: &str = "Placing things with regions\n\n\
1. Describe the whole picture in the Scene box.\n\
2. Drag on the canvas to draw a box, then type what goes in it (double-click a box to type again). Click a box to select it; drag the selected box to move it, its corners to resize it. A drag anywhere else draws a new box, over other boxes too.\n\
3. The pencil sets a region's type (Object, Character, Text), colour and fill. A Text region renders the exact words you give it.\n\
4. Drag rows to set depth: the top row is in front.\n\n\
Boxes guide the model, they don't clip it, and very small boxes may be dropped. Where boxes overlap, say how in the description: \u{201c}cat asleep on the armchair cushion\u{201d}.";

/// The Regions tab's own state (not saved: the regions themselves are the document's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionsUi {
    /// The tool the canvas editing was armed with (the Move tool); picking another ends it.
    armed: Option<Tool>,
    /// The tab was in front last frame: coming to the front arms the canvas.
    was_open: bool,
    drag: Option<Drag>,
    /// The region whose description is being typed on the canvas, and whether its field still
    /// has to take the keyboard focus.
    pub(super) typing: Option<u32>,
    focus: bool,
    /// The region whose properties are open under its row (the pencil).
    pub(super) editor: Option<u32>,
    /// A row being dragged to another depth: its region and where it would drop.
    row_drag: Option<(u32, usize)>,
}

/// A pointer gesture on the canvas, in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// `hit`: the box pressed on, which a click (no drag) selects.
    New {
        start: [f64; 2],
        cur: [f64; 2],
        hit: Option<u32>,
    },
    Move {
        id: u32,
        start: [f64; 2],
        cur: [f64; 2],
        rect: photocraft_geom::Rect,
    },
    /// `corner`: which edges move, −1 the left or top, +1 the right or bottom.
    Resize {
        id: u32,
        corner: [i8; 2],
        start: [f64; 2],
        cur: [f64; 2],
        rect: photocraft_geom::Rect,
    },
}

/// Is the Regions tab in front, with a model that takes regions?
pub fn tab_open(app: &PhotocraftApp) -> bool {
    let ui = &app.ui.ai_gen;
    ui.mode == Mode::Create && ui.create_task == CreateTask::Regions && super::current_model(app).is_some_and(|m| m.layout().is_some())
}

/// Are the boxes on the canvas: the panel showing with the Regions tab in front?
fn on_canvas(app: &PhotocraftApp) -> bool {
    super::shown(app) && tab_open(app) && app.session.active().is_some()
}

/// Does the pointer edit the boxes now (the canvas was armed, and no other tool picked since)?
pub fn editing(app: &PhotocraftApp) -> bool {
    on_canvas(app) && app.ui.ai_gen.regions.armed.is_some_and(|t| t == app.ui.tool)
}

/// Give the pointer to the boxes: the Move tool, which draws nothing of its own over them.
pub(super) fn arm(app: &mut PhotocraftApp) {
    app.ui.tool = Tool::Move;
    app.ui.ai_gen.regions.armed = Some(Tool::Move);
}

/// Once a frame from the panel: arm the canvas when the tab comes to the front, and let go of it
/// when another tool was picked.
pub(super) fn follow(app: &mut PhotocraftApp) {
    let open = tab_open(app);
    if open && !app.ui.ai_gen.regions.was_open {
        arm(app);
    }
    app.ui.ai_gen.regions.was_open = open;
    let r = &mut app.ui.ai_gen.regions;
    if !open || r.armed.is_some_and(|t| t != app.ui.tool) {
        r.armed = None;
        r.drag = None;
        r.typing = None;
    }
}

fn regions_of(app: &PhotocraftApp) -> Vec<AiRegion> {
    app.session.active().map(|d| d.doc.ai_regions.clone()).unwrap_or_default()
}

fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn kind_label(k: AiRegionKind) -> &'static str {
    match k {
        AiRegionKind::Object => "Object",
        AiRegionKind::Character => "Character",
        AiRegionKind::Text => "Text",
    }
}

fn kind_icon(k: AiRegionKind) -> &'static str {
    match k {
        AiRegionKind::Object => "square",
        AiRegionKind::Character => "circle",
        AiRegionKind::Text => "type",
    }
}

/// Change one region (`ai.region.set`); `coalesce` merges a run (typing) into one History step.
fn set(app: &mut PhotocraftApp, id: u32, mut fields: Value, coalesce: Option<String>) {
    fields["id"] = json!(id);
    if let Some(key) = coalesce {
        fields["coalesce"] = json!(key);
    }
    if let Err(e) = app.run("ai.region.set", fields) {
        crate::notices::error(app, format!("{}: {e}", tr("Regions")));
    }
}

/// Replace the regions (`ai.setRegions`); `entries` name the ones kept by id. Returns the list.
fn set_all(app: &mut PhotocraftApp, entries: Vec<Value>) -> Option<Value> {
    match app.run("ai.setRegions", json!({"regions": entries})) {
        Ok(v) => Some(v),
        Err(e) => {
            crate::notices::error(app, format!("{}: {e}", tr("Regions")));
            None
        }
    }
}

fn ids(regions: &[AiRegion]) -> Vec<Value> {
    regions.iter().map(|r| json!({"id": r.id})).collect()
}

fn rect_json(r: photocraft_geom::Rect) -> Value {
    json!([r.x0, r.y0, r.width(), r.height()])
}

/// Add a region in front of the others, select it and open it for typing.
fn add(app: &mut PhotocraftApp, rect: photocraft_geom::Rect) {
    let regions = regions_of(app);
    let mut entries = vec![json!({"rect": rect_json(rect)})];
    entries.extend(ids(&regions));
    let Some(v) = set_all(app, entries) else { return };
    let id = v["regions"][0]["id"].as_u64().and_then(|i| u32::try_from(i).ok());
    app.ui.ai_gen.region = id;
    let r = &mut app.ui.ai_gen.regions;
    r.typing = id;
    r.focus = id.is_some();
}

fn remove(app: &mut PhotocraftApp, id: u32) {
    let regions: Vec<AiRegion> = regions_of(app).into_iter().filter(|r| r.id != id).collect();
    if set_all(app, ids(&regions)).is_some() {
        if app.ui.ai_gen.region == Some(id) {
            app.ui.ai_gen.region = None;
        }
        let r = &mut app.ui.ai_gen.regions;
        r.typing = r.typing.filter(|t| *t != id);
        r.editor = r.editor.filter(|t| *t != id);
    }
}

/// Copy what the chosen model would be sent for the scene and the regions.
fn copy_prompt(app: &mut PhotocraftApp, ctx: &egui::Context, model: &str) {
    let params = json!({"prompt": app.ui.ai_gen.prompt, "model": model});
    match app.run("ai.layoutPrompt", params) {
        Ok(v) => {
            ctx.copy_text(v["text"].as_str().unwrap_or_default().to_string());
            app.ui.status = tr("Copied the layout prompt").to_string();
            app.ui.status_error = false;
        }
        Err(e) => crate::notices::error(app, format!("{}: {e}", tr("Copy layout prompt"))),
    }
}

/// The panel's Regions section, under the scene. `width` is the panel's.
pub(super) fn section(app: &mut PhotocraftApp, ui: &mut egui::Ui, busy: bool, model: &str) {
    let t = Tokens::get(ui.ctx());
    let regions = regions_of(app);
    let width = ui.available_width();
    // The heading: the count, help, then copy, clear and add on the right.
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} ({})", tr("Regions"), regions.len())).font(crate::theme::medium(12.0)).color(t.text));
        crate::icons::button(ui, "info", 20.0, false, "").on_hover_ui(|ui| {
            ui.set_max_width(300.0);
            ui.label(RichText::new(tr(HELP)).size(11.5));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if crate::icons::button(ui, "plus", 22.0, false, tr("Add region")).clicked() {
                    let canvas = app.session.active().map(|d| d.doc.bounds()).unwrap_or_default();
                    let third = |v: u32| i32::try_from(v / 3).unwrap_or(1).max(1);
                    let (w, h) = (third(canvas.width()), third(canvas.height()));
                    add(
                        app,
                        photocraft_geom::Rect::new(
                            canvas.x0.saturating_add(w),
                            canvas.y0.saturating_add(h),
                            canvas.x0.saturating_add(2 * w),
                            canvas.y0.saturating_add(2 * h),
                        ),
                    );
                    arm(app);
                }
                let clear = ui.add_enabled_ui(!regions.is_empty(), |ui| crate::icons::button(ui, "trash", 22.0, false, tr("Clear all regions"))).inner;
                if clear.clicked() {
                    set_all(app, Vec::new());
                    app.ui.ai_gen.region = None;
                }
                if crate::icons::button(ui, "copy", 22.0, false, tr("Click to copy JSON output")).clicked() {
                    copy_prompt(app, ui.ctx(), model);
                }
            });
        });
    });
    if regions.is_empty() {
        ui.label(RichText::new(tr("Drag on the canvas to draw a region, or press +.")).color(t.text_faint).size(11.5));
        return;
    }
    // The rows, front to back. A drag on a row moves it to another depth.
    let mut centres: Vec<f32> = Vec::with_capacity(regions.len());
    let mut stopped = None;
    let selected = app.ui.ai_gen.region;
    let row_drag = app.ui.ai_gen.regions.row_drag;
    let mut line_at: Option<f32> = None;
    let mut tops: Vec<(f32, f32)> = Vec::with_capacity(regions.len());
    // The open editor and its row's pencil: a press anywhere else closes it.
    let mut editor_rect = None;
    let mut pencil_rect = None;
    for (i, r) in regions.iter().enumerate() {
        let mut edit = false;
        let mut delete = false;
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let main_w = (width - 2.0 * 24.0 - 6.0).max(60.0);
            let sense = if busy { egui::Sense::hover() } else { egui::Sense::click_and_drag() };
            let (rect, resp) = ui.allocate_exact_size(vec2(main_w, ROW_HEIGHT), sense);
            let on = selected == Some(r.id);
            if on {
                ui.painter().rect_filled(rect, t.radius_sm, t.row_selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, t.radius_sm, t.hover.gamma_multiply(0.6));
            }
            let dragged = row_drag.is_some_and(|(id, _)| id == r.id);
            let fade = if dragged { 0.45 } else { 1.0 };
            // The number (depth) on the region's colour, the type, the description.
            let chip = Rect::from_min_size(rect.left_center() + vec2(4.0, -9.0), vec2(18.0, 18.0));
            ui.painter().rect_filled(chip, 3.0, color32(r.color).gamma_multiply(fade));
            ui.painter().text(chip.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), crate::theme::medium(11.0), Color32::from_gray(20));
            crate::icons::paint(
                ui,
                Rect::from_min_size(chip.right_top() + vec2(5.0, 1.0), vec2(16.0, 16.0)),
                kind_icon(r.kind),
                13.0,
                t.text_dim.gamma_multiply(fade),
            );
            let label = match r.kind {
                AiRegionKind::Text => format!("\u{201c}{}\u{201d} {}", r.text, r.caption),
                _ if r.caption.is_empty() => tr("Untitled region").to_string(),
                _ => r.caption.clone(),
            };
            let text_rect = Rect::from_min_max(egui::pos2(chip.right() + 26.0, rect.top()), rect.right_bottom() - vec2(4.0, 0.0));
            let galley = ui.painter().layout(label.replace('\n', " "), egui::FontId::proportional(12.0), t.text.gamma_multiply(fade), f32::INFINITY);
            ui.painter().with_clip_rect(text_rect).galley(egui::pos2(text_rect.left(), rect.center().y - galley.size().y / 2.0), galley, t.text);
            if resp.clicked() {
                app.ui.ai_gen.region = Some(r.id);
                if !editing(app) {
                    arm(app);
                }
            }
            if resp.drag_started() {
                app.ui.ai_gen.regions.row_drag = Some((r.id, i));
                app.ui.ai_gen.region = Some(r.id);
            }
            if resp.drag_stopped() {
                stopped = Some(r.id);
            }
            let tip = if busy { "" } else { "Drag to change depth: the top row is in front" };
            let _ = resp.on_hover_text(tr(tip));
            ui.add_enabled_ui(!busy, |ui| {
                let open = app.ui.ai_gen.regions.editor == Some(r.id);
                let pencil = crate::icons::button(ui, "pencil", 22.0, open, tr("Edit region"));
                if open {
                    pencil_rect = Some(pencil.rect);
                }
                edit = pencil.clicked();
                delete = crate::icons::button(ui, "trash", 22.0, false, tr("Remove region")).clicked();
            });
            rect
        });
        let rect = row.inner;
        centres.push(rect.center().y);
        tops.push((rect.top(), rect.bottom()));
        if edit {
            let e = &mut app.ui.ai_gen.regions.editor;
            *e = if *e == Some(r.id) { None } else { Some(r.id) };
            app.ui.ai_gen.region = Some(r.id);
        }
        if delete {
            remove(app, r.id);
            return;
        }
        if app.ui.ai_gen.regions.editor == Some(r.id) {
            editor_rect = Some(editor(app, ui, r, busy, width));
        }
    }
    // The editor belongs to the selected region: it closes when another is selected, or on a
    // press outside it (not in a colour popup it opened, nor on its pencil, which toggles it).
    if let Some(id) = app.ui.ai_gen.regions.editor {
        let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None });
        let outside = press.is_some_and(|p| !editor_rect.is_some_and(|r: Rect| r.contains(p)) && !pencil_rect.is_some_and(|r: Rect| r.contains(p)));
        if app.ui.ai_gen.region != Some(id) || (outside && !egui::Popup::is_any_open(ui.ctx())) {
            app.ui.ai_gen.regions.editor = None;
        }
    }
    // Where a dragged row would go: before the first row whose middle is below the pointer.
    if let Some((id, _)) = app.ui.ai_gen.regions.row_drag {
        let y = ui.ctx().pointer_latest_pos().map_or(f32::NAN, |p| p.y);
        let drop = centres.iter().filter(|c| **c < y).count();
        app.ui.ai_gen.regions.row_drag = Some((id, drop));
        let from = regions.iter().position(|r| r.id == id);
        if from.is_some_and(|f| drop != f && drop != f + 1) {
            line_at = match (drop.checked_sub(1).and_then(|i| tops.get(i)), tops.get(drop)) {
                (Some((_, above)), Some((below, _))) => Some((above + below) / 2.0),
                (Some((_, above)), None) => Some(above + 1.0),
                (None, Some((below, _))) => Some(below - 1.0),
                (None, None) => None,
            };
        }
        if let Some(y) = line_at {
            let x = ui.min_rect().left();
            ui.painter().line_segment([egui::pos2(x + 2.0, y), egui::pos2(x + width - 2.0, y)], Stroke::new(2.0, t.accent));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if let Some(id) = stopped
        && let Some((_, drop)) = app.ui.ai_gen.regions.row_drag.take()
        && let Some(from) = regions.iter().position(|r| r.id == id)
    {
        let to = if drop > from { drop - 1 } else { drop };
        if to != from {
            let mut order = regions.clone();
            let moved = order.remove(from);
            order.insert(to.min(order.len()), moved);
            set_all(app, ids(&order));
        }
    }
    if !ui.input(|i| i.pointer.any_down()) && stopped.is_none() {
        app.ui.ai_gen.regions.row_drag = None;
    }
}

/// A region's properties, under its row, one per line: type, the words of a text region,
/// colour, fill. `width` is the section's. Returns the editor's rect.
fn editor(app: &mut PhotocraftApp, ui: &mut egui::Ui, r: &AiRegion, busy: bool, width: f32) -> Rect {
    let t = Tokens::get(ui.ctx());
    let label_w = 52.0;
    let label = |ui: &mut egui::Ui, text: &str| {
        let (rect, _) = ui.allocate_exact_size(vec2(label_w, 22.0), egui::Sense::hover());
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, tr(text), egui::FontId::proportional(11.5), t.text_dim);
    };
    let margin = 8.0;
    let frame = egui::Frame::new().fill(t.field).stroke(Stroke::new(1.0, t.field_border)).corner_radius(t.radius_sm).inner_margin(margin);
    // Indented under the row's chip, and inside the section: never wider than the panel.
    let indent = 24.0;
    let inner_w = (width - indent - 2.0 * margin - 2.0).max(120.0);
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            frame
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(inner_w);
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.horizontal(|ui| {
                            label(ui, "Type");
                            // Three equal cells in the width left, drawn here so no label can widen
                            // them (and the panel) past it.
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let cell = ((ui.available_width() - 4.0) / 3.0).max(30.0);
                            for k in AiRegionKind::ALL {
                                let (rect, resp) = ui.allocate_exact_size(vec2(cell, 22.0), egui::Sense::click());
                                let on = r.kind == k;
                                if on {
                                    ui.painter().rect_filled(rect, t.radius_sm, t.accent);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                                }
                                let color = if on { Color32::WHITE } else { t.text };
                                let galley = ui.painter().layout_no_wrap(tr(kind_label(k)).to_string(), egui::FontId::proportional(11.5), color);
                                let at = egui::pos2((rect.center().x - galley.size().x / 2.0).max(rect.left() + 2.0), rect.center().y - galley.size().y / 2.0);
                                ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).galley(at, galley, color);
                                if resp.clicked() && !on {
                                    let mut fields = json!({"kind": k.key()});
                                    if k == AiRegionKind::Text && r.text.trim().is_empty() {
                                        fields["text"] = json!("TITLE");
                                    }
                                    set(app, r.id, fields, None);
                                }
                            }
                        });
                        if r.kind == AiRegionKind::Text {
                            ui.horizontal(|ui| {
                                label(ui, "Words");
                                let mut words = r.text.clone();
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut words)
                                        .id(egui::Id::unique("ai-region-words").with(r.id))
                                        .hint_text(tr("The exact words"))
                                        .desired_width(f32::INFINITY),
                                );
                                if field.changed() {
                                    set(app, r.id, json!({"text": words}), Some(format!("ai-region-words-{}", r.id)));
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            label(ui, "Colour");
                            // The swatches and the picker share the width left (8 + 1 cells).
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let swatch = ((ui.available_width() - 9.0 * 3.0) / 9.0).clamp(10.0, 16.0);
                            for c in photocraft_engine::ai_regions_cmds::PALETTE {
                                let (rect, resp) = ui.allocate_exact_size(vec2(swatch, swatch), egui::Sense::click());
                                ui.painter().rect_filled(rect, 3.0, color32(c));
                                if c == r.color {
                                    ui.painter().rect_stroke(rect.expand(2.0), 4.0, Stroke::new(1.5, t.text), egui::StrokeKind::Inside);
                                }
                                if resp.on_hover_text(tr("Outline colour")).clicked() && c != r.color {
                                    set(app, r.id, json!({"color": hex(c)}), None);
                                }
                            }
                            let mut custom = r.color;
                            ui.spacing_mut().interact_size = vec2(swatch + 4.0, swatch);
                            if egui::color_picker::color_edit_button_srgb(ui, &mut custom).changed() && custom != r.color {
                                set(app, r.id, json!({"color": hex(custom)}), Some(format!("ai-region-colour-{}", r.id)));
                            }
                        });
                        ui.horizontal(|ui| {
                            let mut fill = r.fill;
                            let (rect, _) = ui.allocate_exact_size(vec2(label_w, 22.0), egui::Sense::hover());
                            let check = ui.put(rect, |ui: &mut egui::Ui| crate::widgets::checkbox(ui, &mut fill, tr("Fill")));
                            if check.changed() {
                                set(app, r.id, json!({"fill": fill}), None);
                            }
                            // The percentage on the right, the slider in between (it takes the width left).
                            ui.add_enabled_ui(r.fill, |ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let pct = RichText::new(format!("{}%", (r.fill_opacity * 100.0).round())).color(t.text_dim).size(11.5);
                                    ui.add_sized(vec2(34.0, 18.0), egui::Label::new(pct));
                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        let mut v = r.fill_opacity.clamp(0.0, 1.0) * 100.0;
                                        let resp = crate::widgets::slider(ui, &mut v, 0.0..=60.0, None).on_hover_text(tr("Fill opacity"));
                                        if resp.changed() {
                                            set(
                                                app,
                                                r.id,
                                                json!({"fillOpacity": (f64::from(v.round()) / 100.0).clamp(0.0, 1.0)}),
                                                Some(format!("ai-region-fill-{}", r.id)),
                                            );
                                        }
                                    });
                                });
                            });
                        });
                    });
                })
                .response
                .rect
        })
        .inner
    })
    .inner
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// How many rows the section adds to the panel's wanted height.
pub(super) fn rows(app: &PhotocraftApp) -> usize {
    if !tab_open(app) {
        return 0;
    }
    let n = app.session.active().map_or(0, |d| d.doc.ai_regions.len());
    // The heading, the rows (or the hint), an open editor (type, colour, fill and its margins;
    // the words too for a text region).
    let words = |id: u32| app.session.active().is_some_and(|d| d.doc.ai_regions.iter().any(|r| r.id == id && r.kind == AiRegionKind::Text));
    1 + n.max(1) + app.ui.ai_gen.regions.editor.map_or(0, |id| 4 + usize::from(words(id)))
}

fn screen_rect(xf: &ViewXform, r: photocraft_geom::Rect) -> Rect {
    Rect::from_two_pos(xf.to_screen(r.x0 as f32, r.y0 as f32), xf.to_screen(r.x1 as f32, r.y1 as f32))
}

fn rect_from(a: [f64; 2], b: [f64; 2]) -> photocraft_geom::Rect {
    let r = |v: f64| v.round().clamp(-1e7, 1e7) as i32;
    photocraft_geom::Rect::new(r(a[0].min(b[0])), r(a[1].min(b[1])), r(a[0].max(b[0])), r(a[1].max(b[1])))
}

/// The rect a gesture leaves `drag`'s region with, kept inside `canvas` (a move keeps its size).
fn dragged(drag: &Drag, canvas: photocraft_geom::Rect) -> Option<(Option<u32>, photocraft_geom::Rect)> {
    match *drag {
        Drag::New { start, cur, .. } => Some((None, rect_from(start, cur).intersect(&canvas))),
        Drag::Move { id, start, cur, rect } => {
            let dx = (cur[0] - start[0]).round().clamp(-1e7, 1e7) as i32;
            let dy = (cur[1] - start[1]).round().clamp(-1e7, 1e7) as i32;
            // Kept on the canvas: as far as it can go, the size it has.
            let dx = dx.clamp(canvas.x0.saturating_sub(rect.x0).min(0), canvas.x1.saturating_sub(rect.x1).max(0));
            let dy = dy.clamp(canvas.y0.saturating_sub(rect.y0).min(0), canvas.y1.saturating_sub(rect.y1).max(0));
            Some((Some(id), rect.translate(dx, dy)))
        }
        Drag::Resize { id, corner, start, cur, rect } => {
            let (dx, dy) = (cur[0] - start[0], cur[1] - start[1]);
            let (mut l, mut t, mut r, mut b) = (f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1));
            match corner[0] {
                -1 => l += dx,
                1 => r += dx,
                _ => {}
            }
            match corner[1] {
                -1 => t += dy,
                1 => b += dy,
                _ => {}
            }
            let out = rect_from([l, t], [r, b]).intersect(&canvas);
            (out.width() >= 1 && out.height() >= 1).then_some((Some(id), out))
        }
    }
}

/// The corner of `r` (screen) within reach of `p`.
fn corner_at(r: Rect, p: Pos2) -> Option<[i8; 2]> {
    [([-1, -1], r.left_top()), ([1, -1], r.right_top()), ([-1, 1], r.left_bottom()), ([1, 1], r.right_bottom())]
        .into_iter()
        .find(|(_, c)| c.distance(p) <= HANDLE_REACH)
        .map(|(k, _)| k)
}

/// The corner of `r` (document pixels) within `reach` of `(x, y)`.
fn corner_near(r: photocraft_geom::Rect, x: f64, y: f64, reach: f64) -> Option<[i8; 2]> {
    let (l, t, rt, b) = (f64::from(r.x0), f64::from(r.y0), f64::from(r.x1), f64::from(r.y1));
    [([-1, -1], l, t), ([1, -1], rt, t), ([-1, 1], l, b), ([1, 1], rt, b)].into_iter().find(|(_, cx, cy)| (x - cx).hypot(y - cy) <= reach).map(|(k, ..)| k)
}

fn contains(r: photocraft_geom::Rect, x: f64, y: f64) -> bool {
    x >= f64::from(r.x0) && x < f64::from(r.x1) && y >= f64::from(r.y0) && y < f64::from(r.y1)
}

/// Pointer input on the canvas while the boxes are being edited ([`editing`]). Returns true
/// when the event was theirs: then no tool sees it.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent) -> bool {
    if !editing(app) {
        return false;
    }
    let Some(d) = app.session.active() else { return true };
    let canvas = d.doc.bounds();
    let regions = d.doc.ai_regions.clone();
    let zoom = f64::from(app.point_zoom().max(0.01));
    match ev {
        ToolEvent::Down { x, y, .. } => {
            let st = &mut app.ui.ai_gen.regions;
            st.typing = None;
            // A corner of the selected box resizes it, a press in it moves it; anywhere else a
            // drag draws a new box, over other boxes too (so a canvas full of them still takes
            // one), and a click selects the box under it (the front one first).
            let selected = app.ui.ai_gen.region.and_then(|id| regions.iter().find(|r| r.id == id));
            let reach = f64::from(HANDLE_REACH) / zoom;
            let grabbed = selected.and_then(|r| corner_near(r.rect, x, y, reach).map(|c| (r, c)));
            let drag = if let Some((r, corner)) = grabbed {
                Drag::Resize { id: r.id, corner, start: [x, y], cur: [x, y], rect: r.rect }
            } else if let Some(r) = selected.filter(|r| contains(r.rect, x, y)) {
                Drag::Move { id: r.id, start: [x, y], cur: [x, y], rect: r.rect }
            } else {
                Drag::New { start: [x, y], cur: [x, y], hit: regions.iter().find(|r| contains(r.rect, x, y)).map(|r| r.id) }
            };
            app.ui.ai_gen.regions.drag = Some(drag);
        }
        ToolEvent::Move { x, y, .. } => match &mut app.ui.ai_gen.regions.drag {
            Some(Drag::New { cur, .. } | Drag::Move { cur, .. } | Drag::Resize { cur, .. }) => *cur = [x, y],
            None => {}
        },
        ToolEvent::Up { x, y } => {
            let Some(mut drag) = app.ui.ai_gen.regions.drag.take() else { return true };
            let (Drag::New { cur, .. } | Drag::Move { cur, .. } | Drag::Resize { cur, .. }) = &mut drag;
            *cur = [x, y];
            match (drag, dragged(&drag, canvas)) {
                (Drag::New { start, cur, hit }, Some((_, rect))) => {
                    let far = (cur[0] - start[0]).abs().max((cur[1] - start[1]).abs()) * zoom >= f64::from(MIN_DRAG);
                    if far && rect.width() >= 1 && rect.height() >= 1 {
                        add(app, rect);
                    } else {
                        // A click: the box under it selected, or none on empty canvas.
                        app.ui.ai_gen.region = hit;
                    }
                }
                (Drag::Move { rect: before, .. } | Drag::Resize { rect: before, .. }, Some((Some(id), rect))) if rect != before => {
                    set(app, id, json!({"rect": rect_json(rect)}), None);
                }
                _ => {}
            }
        }
    }
    true
}

/// A double-click on the canvas: on a box being edited, type its description.
pub fn double_click(app: &mut PhotocraftApp, x: f64, y: f64) -> bool {
    if !editing(app) {
        return false;
    }
    let Some(id) = regions_of(app).iter().find(|r| contains(r.rect, x, y)).map(|r| r.id) else { return false };
    app.ui.ai_gen.region = Some(id);
    let st = &mut app.ui.ai_gen.regions;
    st.typing = Some(id);
    st.focus = true;
    true
}

/// The boxes over the canvas while the Regions tab is in front, the gesture in progress, the
/// description being typed, and the pointer's shape over them.
pub fn canvas(app: &mut PhotocraftApp, ui: &mut egui::Ui, painter: &egui::Painter, xf: &ViewXform) {
    if !on_canvas(app) {
        return;
    }
    let editing = editing(app);
    let Some(canvas) = app.session.active().map(|d| d.doc.bounds()) else { return };
    let regions = regions_of(app);
    let drag = app.ui.ai_gen.regions.drag;
    let moving = drag.as_ref().and_then(|d| dragged(d, canvas));
    let selected = app.ui.ai_gen.region;
    let typing = app.ui.ai_gen.regions.typing.filter(|_| editing);
    let clip = painter.clip_rect();
    // Back to front, so the one in front is drawn last.
    for (i, r) in regions.iter().enumerate().rev() {
        let rect = match moving {
            Some((Some(id), rect)) if id == r.id => rect,
            _ => r.rect,
        };
        let sr = screen_rect(xf, rect);
        let color = color32(r.color);
        let on = selected == Some(r.id) && editing;
        if r.fill {
            painter.rect_filled(sr, 0.0, color.gamma_multiply(r.fill_opacity.clamp(0.0, 1.0)));
        }
        painter.rect_stroke(sr, 0.0, Stroke::new(if on { 2.0 } else { 1.5 }, color), egui::StrokeKind::Inside);
        // The label: depth and type, above the box (inside it at the top of the view).
        let chip_text = painter.layout_no_wrap(format!("{}  {}", i + 1, tr(kind_label(r.kind))), crate::theme::medium(11.0), Color32::from_gray(20));
        let chip_size = chip_text.size() + vec2(10.0, 6.0);
        let inside = sr.top() - chip_size.y < clip.top();
        let chip = Rect::from_min_size(if inside { sr.left_top() + vec2(2.0, 2.0) } else { sr.left_top() - vec2(0.0, chip_size.y) }, chip_size);
        painter.rect_filled(chip, 3.0, color);
        painter.galley(chip.min + vec2(5.0, 3.0), chip_text, Color32::from_gray(20));
        let inner = painter.with_clip_rect(sr.shrink(1.0).intersect(clip));
        let top = if inside { chip.bottom() + 4.0 } else { sr.top() + 5.0 };
        if r.kind == AiRegionKind::Text && !r.text.is_empty() {
            // The words, at a size that fits the box: where and how large they will be.
            let chars = r.text.chars().count().max(1) as f32;
            let size = (sr.height() * 0.5).min((sr.width() - 12.0) / (chars * 0.62)).clamp(8.0, 96.0);
            let words = painter.layout_no_wrap(r.text.clone(), egui::FontId::proportional(size), Color32::from_rgb(0xf6, 0xea, 0xd0));
            inner.galley(sr.center() - words.size() / 2.0, words, Color32::WHITE);
        } else if typing != Some(r.id) && !r.caption.is_empty() && sr.width() > 24.0 && sr.height() > 18.0 {
            let galley = painter.layout(r.caption.clone(), egui::FontId::proportional(11.5), Color32::from_gray(240), (sr.width() - 22.0).max(20.0));
            let card = Rect::from_min_size(egui::pos2(sr.left() + 5.0, top), galley.size() + vec2(10.0, 6.0));
            inner.rect_filled(card, 3.0, Color32::from_black_alpha(190));
            inner.galley(card.min + vec2(5.0, 3.0), galley, Color32::WHITE);
        }
        if on {
            for c in [sr.left_top(), sr.right_top(), sr.left_bottom(), sr.right_bottom()] {
                let h = Rect::from_center_size(c, vec2(9.0, 9.0));
                painter.rect_filled(h, 0.0, Color32::WHITE);
                painter.rect_stroke(h, 0.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
            }
        }
    }
    // A box being drawn: in the colour it will get (the engine's next one), dashed light over
    // dark so it shows on any picture, white canvas included, with its size.
    if let Some((None, rect)) = moving {
        let sr = screen_rect(xf, rect);
        let next = regions.iter().map(|r| r.id).max().unwrap_or(0).saturating_add(1);
        let palette = photocraft_engine::ai_regions_cmds::PALETTE;
        let color = color32(palette.get(next as usize % palette.len()).copied().unwrap_or(palette[0]));
        painter.rect_filled(sr, 0.0, color.gamma_multiply(0.18));
        painter.rect_stroke(sr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(200)), egui::StrokeKind::Outside);
        painter.rect_stroke(sr, 0.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
        let pts = [sr.left_top(), sr.right_top(), sr.right_bottom(), sr.left_bottom(), sr.left_top()];
        painter.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0, Color32::WHITE), 4.0, 4.0));
        let size = painter.layout_no_wrap(format!("{} \u{d7} {} px", rect.width(), rect.height()), egui::FontId::proportional(11.0), Color32::WHITE);
        let tag = Rect::from_min_size(sr.right_bottom() + vec2(6.0, 6.0), size.size() + vec2(10.0, 6.0));
        painter.rect_filled(tag, 3.0, Color32::from_black_alpha(200));
        painter.galley(tag.min + vec2(5.0, 3.0), size, Color32::WHITE);
    }
    if let Some(id) = typing
        && let Some(r) = regions.iter().find(|r| r.id == id)
    {
        type_caption(app, ui.ctx(), xf, r);
    }
    // The pointer's shape: what a press there would do.
    if editing
        && drag.is_none()
        && let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| xf.rect.contains(*p))
    {
        let d = xf.to_doc(p);
        let sel = selected.and_then(|id| regions.iter().find(|r| r.id == id));
        let icon = match sel.and_then(|r| corner_at(screen_rect(xf, r.rect), p)) {
            Some([-1, -1] | [1, 1]) => egui::CursorIcon::ResizeNwSe,
            Some(_) => egui::CursorIcon::ResizeNeSw,
            None if sel.is_some_and(|r| contains(r.rect, d[0], d[1])) => egui::CursorIcon::Move,
            None => egui::CursorIcon::Crosshair,
        };
        ui.ctx().set_cursor_icon(icon);
    }
    if drag.is_some() {
        ui.ctx().request_repaint();
    }
}

/// The description of `r` typed in place, over its box.
fn type_caption(app: &mut PhotocraftApp, ctx: &egui::Context, xf: &ViewXform, r: &AiRegion) {
    let t = Tokens::get(ctx);
    let sr = screen_rect(xf, r.rect);
    let width = (sr.width() - 10.0).clamp(140.0, 420.0);
    let mut caption = r.caption.clone();
    let focus = std::mem::take(&mut app.ui.ai_gen.regions.focus);
    let mut done = false;
    let mut changed = false;
    egui::Area::new(egui::Id::unique("ai-region-typing").with(r.id)).order(egui::Order::Foreground).fixed_pos(sr.left_top() + vec2(5.0, 5.0)).show(ctx, |ui| {
        egui::Frame::new().fill(Color32::from_black_alpha(225)).stroke(Stroke::new(1.0, color32(r.color))).corner_radius(3.0).inner_margin(4.0).show(
            ui,
            |ui| {
                let field = ui.add(
                    egui::TextEdit::multiline(&mut caption)
                        .id(egui::Id::unique("ai-region-caption").with(r.id))
                        .hint_text(tr("Describe what goes here"))
                        .desired_width(width)
                        .desired_rows(2)
                        .frame(egui::Frame::NONE)
                        .text_color(Color32::from_gray(245)),
                );
                if focus {
                    field.request_focus();
                }
                changed = field.changed();
                let keys = ui.input(|i| i.key_pressed(egui::Key::Escape) || (i.key_pressed(egui::Key::Enter) && i.modifiers.command));
                done = keys || (field.lost_focus() && !focus);
                let hint = egui::RichText::new(tr("Esc or Ctrl+Enter when done")).size(10.5).color(t.text_faint);
                ui.label(hint);
            },
        );
    });
    if changed {
        set(app, r.id, json!({"caption": caption}), Some(format!("ai-region-caption-{}", r.id)));
    }
    if done {
        app.ui.ai_gen.regions.typing = None;
    }
}

/// Load Prompt of a layer made with regions: they become the document's again, and the tab
/// comes to the front.
pub(super) fn restore(app: &mut PhotocraftApp, regions: &[AiRegion]) {
    let entries: Vec<Value> = regions.iter().map(photocraft_engine::ai_regions_cmds::region_json).collect();
    if set_all(app, entries).is_some() {
        app.ui.ai_gen.mode = Mode::Create;
        app.ui.ai_gen.create_task = CreateTask::Regions;
        app.ui.ai_gen.region = None;
    }
}
