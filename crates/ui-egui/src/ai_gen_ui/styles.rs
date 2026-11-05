//! Style presets in the shell: the Style button beside Generate and the picker it opens, a grid
//! of thumbnails under a tab list of the presets' groups (All, Photography, …). The thumbnails
//! show one subject throughout, so the treatments compare; hovering one shows it larger with a
//! few words about the look. "No preset" leaves the prompt as it is.
//!
//! The presets are `photocraft_aigen::STYLES`; the panel keeps the chosen one's id
//! ([`super::AiGenUi::style`]) and `ai.generate`'s `style` adds its sentence to the prompt.

use egui::{RichText, vec2};
use photocraft_aigen::{STYLES, Style, StyleGroup};
use photocraft_codecs::{ChannelLayout, SampleType};

use super::tr;
use crate::PhotocraftApp;
use crate::theme::Tokens;

/// Choose the panel's style preset: `{"style": id|""}` (empty: none) → `{"style": id|null}`.
pub const STYLE_MENU: &str = "window.panel.aiGen.style";

/// The thumbnails, 512 px squares (`assets/ai-styles`), by preset id.
const THUMBS: [(&str, &[u8]); 24] = [
    ("natural-photo", include_bytes!("../../../../assets/ai-styles/natural-photo.jpg")),
    ("cinematic", include_bytes!("../../../../assets/ai-styles/cinematic.jpg")),
    ("studio-photo", include_bytes!("../../../../assets/ai-styles/studio-photo.jpg")),
    ("vintage-film", include_bytes!("../../../../assets/ai-styles/vintage-film.jpg")),
    ("black-and-white", include_bytes!("../../../../assets/ai-styles/black-and-white.jpg")),
    ("flat-vector", include_bytes!("../../../../assets/ai-styles/flat-vector.jpg")),
    ("storybook", include_bytes!("../../../../assets/ai-styles/storybook.jpg")),
    ("anime", include_bytes!("../../../../assets/ai-styles/anime.jpg")),
    ("comic-book", include_bytes!("../../../../assets/ai-styles/comic-book.jpg")),
    ("digital-painting", include_bytes!("../../../../assets/ai-styles/digital-painting.jpg")),
    ("watercolour", include_bytes!("../../../../assets/ai-styles/watercolour.jpg")),
    ("oil-painting", include_bytes!("../../../../assets/ai-styles/oil-painting.jpg")),
    ("pencil-sketch", include_bytes!("../../../../assets/ai-styles/pencil-sketch.jpg")),
    ("line-art", include_bytes!("../../../../assets/ai-styles/line-art.jpg")),
    ("printmaking", include_bytes!("../../../../assets/ai-styles/printmaking.jpg")),
    ("3d-cartoon", include_bytes!("../../../../assets/ai-styles/3d-cartoon.jpg")),
    ("realistic-cgi", include_bytes!("../../../../assets/ai-styles/realistic-cgi.jpg")),
    ("clay-model", include_bytes!("../../../../assets/ai-styles/clay-model.jpg")),
    ("paper-craft", include_bytes!("../../../../assets/ai-styles/paper-craft.jpg")),
    ("textile-craft", include_bytes!("../../../../assets/ai-styles/textile-craft.jpg")),
    ("pixel-art", include_bytes!("../../../../assets/ai-styles/pixel-art.jpg")),
    ("retro-poster", include_bytes!("../../../../assets/ai-styles/retro-poster.jpg")),
    ("collage", include_bytes!("../../../../assets/ai-styles/collage.jpg")),
    ("abstract", include_bytes!("../../../../assets/ai-styles/abstract.jpg")),
];

/// A tile's picture, its gap, how many tiles a row has, and how tall the grid may grow before
/// it scrolls.
const TILE: f32 = 128.0;
const GAP: f32 = 10.0;
const COLUMNS: usize = 4;
const GRID_HEIGHT: f32 = 2.0 * (TILE + 30.0) + 60.0;
/// The larger preview on hover.
const PREVIEW: f32 = 256.0;

/// The preset chosen in the panel, while it is one there is.
pub(super) fn chosen(app: &PhotocraftApp) -> Option<&'static Style> {
    photocraft_aigen::style::find(&app.ui.ai_gen.style)
}

/// `ai.generate`'s `style` for the panel: the chosen preset's id.
pub(super) fn param(app: &PhotocraftApp) -> Option<&'static str> {
    chosen(app).map(|s| s.id)
}

/// Choose `id` (empty: none). Returns the preset chosen, or an error for an id there is none of.
pub(super) fn choose(app: &mut PhotocraftApp, id: &str) -> Result<Option<&'static Style>, String> {
    if id.trim().is_empty() {
        app.ui.ai_gen.style.clear();
        return Ok(None);
    }
    let style = photocraft_aigen::style::find(id).ok_or_else(|| format!("{} `{}`", tr("No style preset"), id.chars().take(64).collect::<String>()))?;
    app.ui.ai_gen.style = style.id.to_string();
    Ok(Some(style))
}

/// `id`'s thumbnail as an egui image (one transparent pixel if it can't be decoded, which a test
/// rules out).
fn decode(bytes: &[u8]) -> egui::ColorImage {
    let rgba = photocraft_codecs::decode(bytes).ok().map(|img| img.convert(ChannelLayout::Rgba, SampleType::U8));
    match rgba {
        Some(img) if img.data().len() == img.width() as usize * img.height() as usize * 4 => {
            egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.data())
        }
        _ => egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT),
    }
}

/// `id`'s thumbnail texture, decoded on first use and kept in the context.
fn texture(ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let bytes = THUMBS.iter().find(|(i, _)| *i == id)?.1;
    let key = egui::Id::unique("ai-style-thumb").with(id);
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(tex);
    }
    let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
    let tex = ctx.load_texture(format!("ai-style-{id}"), decode(bytes), options);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

/// The Style button's width: the palette, and the chosen preset's name (or "Style") with `words`.
pub(super) fn button_width(ui: &egui::Ui, app: &PhotocraftApp, words: bool) -> f32 {
    if !words {
        return 26.0;
    }
    let label = chosen(app).map_or(tr("Style"), |s| s.name);
    let text = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE).size().x;
    (8.0 + 16.0 + 6.0 + text + 8.0).min(MAX_BUTTON)
}

/// The widest the Style button grows for a long preset name (cut short beyond).
const MAX_BUTTON: f32 = 128.0;

/// The Style button, and the picker it opens. With `words` it says which preset is chosen;
/// without, the palette alone (accented while one is).
pub(super) fn button(app: &mut PhotocraftApp, ui: &mut egui::Ui, enabled: bool, words: bool) {
    let t = Tokens::get(ui.ctx());
    let style = chosen(app);
    let size = vec2(button_width(ui, app, words), ui.spacing().interact_size.y.max(24.0));
    let resp = ui.add_enabled_ui(enabled, |ui| {
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let on = ui.is_enabled();
        let label = style.map_or(tr("Style"), |s| s.name);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, on, tr("Style")));
        let fill = if on && resp.hovered() { t.hover } else { t.field };
        ui.painter().rect_filled(rect, t.radius_sm, fill);
        let border = if style.is_some() { t.accent } else { t.field_border };
        ui.painter().rect_stroke(rect, t.radius_sm, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
        let color = if on { t.text } else { t.text_faint };
        let icon_color = if on && style.is_some() { t.accent } else { color };
        let icon = if words { egui::Rect::from_min_size(rect.left_center() + vec2(8.0, -8.0), vec2(16.0, 16.0)) } else { egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0)) };
        crate::icons::paint(ui, icon, "palette", 14.0, icon_color);
        if words {
            let room = rect.right() - 8.0 - (icon.right() + 6.0);
            let painter = ui.painter_at(rect.shrink(1.0));
            let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), egui::FontId::proportional(12.0), color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
            let galley = ui.painter().layout_job(job);
            painter.galley(egui::pos2(icon.right() + 6.0, rect.center().y - galley.size().y / 2.0), galley, color);
        }
        resp
    });
    let tip = match style {
        Some(s) => format!("{}: {}. {}", tr("Style"), s.name, tr("Choose another, or No preset to rely on the prompt alone")),
        None => tr("Choose a style preset: a look added to the prompt (photography, illustration, painting, 3D, graphic)").to_string(),
    };
    let resp = resp.inner.on_hover_text(tip);
    egui::Popup::from_toggle_button_response(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_END)
        .align_alternatives(&[egui::RectAlign::BOTTOM_END, egui::RectAlign::TOP_START, egui::RectAlign::BOTTOM_START])
        .show(|ui| {
            if let Some(picked) = picker(app, ui) {
                app.ui.ai_gen.style = picked.map(|s| s.id.to_string()).unwrap_or_default();
                ui.close();
            }
        });
}

/// The picker: the groups as tabs, then the tiles of the tab in front ("No preset" first).
/// Returns the tile clicked: `Some(None)` for No preset.
fn picker(app: &mut PhotocraftApp, ui: &mut egui::Ui) -> Option<Option<&'static Style>> {
    let t = Tokens::get(ui.ctx());
    let width = COLUMNS as f32 * TILE + (COLUMNS as f32 - 1.0) * GAP;
    ui.set_width(width);
    ui.label(RichText::new(tr("Style Presets")).font(crate::theme::medium(13.0)).color(t.text));
    ui.add_space(4.0);
    let mut tab = app.ui.ai_gen.style_tab.min(StyleGroup::ALL.len());
    tabs(ui, &mut tab);
    app.ui.ai_gen.style_tab = tab;
    ui.add_space(8.0);
    let group = tab.checked_sub(1).and_then(|i| StyleGroup::ALL.get(i).copied());
    let shown: Vec<&'static Style> = STYLES.iter().filter(|s| group.is_none_or(|g| s.group == g)).collect();
    let current = chosen(app).map(|s| s.id);
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("ai-style-grid").max_height(GRID_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        let tiles: Vec<Option<&'static Style>> = std::iter::once(None).chain(shown.iter().map(|s| Some(*s))).collect();
        for row in tiles.chunks(COLUMNS) {
            ui.horizontal_top(|ui| {
                for style in row {
                    let on = style.map(|s| s.id) == current;
                    if tile(ui, *style, on).clicked() {
                        picked = Some(*style);
                    }
                }
            });
        }
    });
    picked
}

/// The tab list: All, then each group; the one in front underlined in the accent.
fn tabs(ui: &mut egui::Ui, tab: &mut usize) {
    let t = Tokens::get(ui.ctx());
    let names: Vec<&str> = std::iter::once(tr("All")).chain(StyleGroup::ALL.iter().map(|g| tr(g.label()))).collect();
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for (i, name) in names.iter().enumerate() {
            let on = *tab == i;
            let color = if on { t.text } else { t.text_dim };
            let galley = ui.painter().layout_no_wrap((*name).to_string(), crate::theme::medium(11.5), color);
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x, 22.0), egui::Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, *name));
            let color = if !on && resp.hovered() { t.text } else { color };
            ui.painter().galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0 - 1.0), galley, color);
            if on {
                let line = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom());
                ui.painter().rect_filled(line, 1.0, t.accent);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *tab = i;
            }
        }
    });
    let y = ui.cursor().top().max(top + 22.0);
    let r = ui.max_rect();
    ui.painter().line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], egui::Stroke::new(1.0, t.separator));
}

/// One tile: the thumbnail with its name under it; the chosen one framed in the accent with a
/// check. `None` is No preset. Hovering shows the picture larger, with what the look is.
fn tile(ui: &mut egui::Ui, style: Option<&'static Style>, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(TILE, TILE + 22.0), egui::Sense::click());
    let name = style.map_or(tr("No preset"), |s| s.name);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    let picture = egui::Rect::from_min_size(rect.min, vec2(TILE, TILE));
    let radius = t.radius_sm + 2.0;
    let texture = style.and_then(|s| texture(ui.ctx(), s.id));
    match &texture {
        Some(tex) => {
            egui::Image::new((tex.id(), vec2(TILE, TILE))).corner_radius(radius).paint_at(ui, picture);
        }
        None => {
            // No preset: a plain tile.
            ui.painter().rect_filled(picture, radius, t.field);
            crate::icons::paint(ui, egui::Rect::from_center_size(picture.center(), vec2(28.0, 28.0)), "ban", 26.0, t.text_faint);
        }
    }
    let border = if on {
        egui::Stroke::new(2.5, t.accent)
    } else if resp.hovered() {
        egui::Stroke::new(1.5, t.text_dim)
    } else {
        egui::Stroke::new(1.0, t.field_border)
    };
    ui.painter().rect_stroke(picture, radius, border, egui::StrokeKind::Inside);
    if on {
        // The check in a badge at the top right.
        let badge = egui::Rect::from_center_size(picture.right_top() + vec2(-14.0, 14.0), vec2(20.0, 20.0));
        ui.painter().circle_filled(badge.center(), 10.0, t.accent);
        crate::icons::paint(ui, badge.shrink(3.0), "check", 13.0, t.primary_text);
    }
    let color = if on { t.text } else { t.text_dim };
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), egui::FontId::proportional(12.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(TILE);
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(egui::pos2(rect.center().x - galley.size().x / 2.0, picture.bottom() + 4.0), galley, color);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    match (style, texture) {
        (Some(s), Some(tex)) => resp.on_hover_ui(|ui| {
            ui.set_max_width(PREVIEW);
            ui.add(egui::Image::new((tex.id(), vec2(PREVIEW, PREVIEW))).corner_radius(radius));
            ui.add_space(4.0);
            ui.label(RichText::new(s.name).font(crate::theme::medium(12.5)).color(t.text));
            ui.label(RichText::new(tr(s.description)).color(t.text_dim).size(11.5));
        }),
        _ => resp.on_hover_text(tr("No preset: the prompt alone says how the image looks")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_a_thumbnail_that_decodes_to_a_512_px_square() {
        assert_eq!(THUMBS.len(), STYLES.len());
        for s in &STYLES {
            let (_, bytes) = THUMBS.iter().find(|(id, _)| *id == s.id).unwrap_or_else(|| panic!("{} has no thumbnail", s.id));
            let img = decode(bytes);
            assert_eq!(img.size, [512, 512], "{}", s.id);
        }
    }
}
