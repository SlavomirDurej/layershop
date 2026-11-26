//! Style presets in the shell: the Style button beside Generate and the picker it opens, a grid
//! of thumbnails under a tab list of the presets' groups (All, Photography, …). The picker is a
//! craft layer: dragging its header moves it, its edges resize it (it stays as left for the session), the grid
//! reflows to the width and scrolls under a framework scroll bar in the canvas's docked look. The thumbnails
//! show one subject throughout, so the treatments compare; hovering one shows it larger with a
//! few words about the look. "No preset" leaves the prompt as it is.
//!
//! The presets are `photocraft_aigen::STYLES`; the panel keeps the chosen one's id
//! ([`super::AiGenUi::style`]) and `ai.generate`'s `style` adds its sentence to the prompt.

use std::cell::Cell;

use craft::behavior::overlay::{Align, Side};
use craft::behavior::scroll::ScrollSpan;
use craft::behavior::{Axis, Metrics, PartKey, Sense};
use craft::ui::contract::overlay::{Anchor, OverlayFit, Place, PlacementChange};
use craft::ui::contract::{
    Access, Bind, BindMut, BodyCx, Common, Component, Container, ContainerProps, Cx, DragBand, Edit as _, EditResponse, HeaderRegion, Mount, NoValue, Outcome,
    Overlay, POPUP_PAD, PaintUi, PartsCx, Pointer as _, Props, Role, Show as _, ShowContainer as _, Trigger,
};
use craft::ui::runtime::access::{Name, Numeric};
use craft::ui::style::{ComponentStyle, ScrollBarKind, ScrollBarLook, StyleCx, SurfaceKind};
use egui::{RichText, pos2, vec2};
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

/// A tile's picture, the gap between tiles, and how many tiles a row has until the user resizes the picker.
const TILE: f32 = 128.0;
const GAP: f32 = 10.0;
const COLUMNS: usize = 4;
/// The least size the picker window opens at, and the least it may be resized to (one column, one row).
const HEIGHT: f32 = 260.0;
/// The height of tiles the window opens at: two rows and the top of the third, so it shows it scrolls.
const GRID_HEIGHT: f32 = 2.0 * (TILE + 22.0 + GAP) + 60.0;
const MIN_WIDTH: f32 = TILE + 40.0;
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
        crate::ai_widgets::paint_icon(ui, icon, "palette", 14.0, icon_color);
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
    if resp.clicked() {
        app.ui.ai_gen.style_open = !app.ui.ai_gen.style_open;
    }
    picker(app, ui, &resp);
}

/// The picker, while it is open: a window that hangs from the Style button, moved by its header (the title and the
/// tabs) and resized by its edges, with the tiles of the tab in front ("No preset" first) scrolling under the header.
/// Choosing a tile closes it.
fn picker(app: &mut PhotocraftApp, ui: &mut egui::Ui, opener: &egui::Response) {
    let current = chosen(app).map(|s| s.id);
    let tab = Cell::new(app.ui.ai_gen.style_tab.min(StyleGroup::ALL.len()));
    let placed = Cell::new(app.ui.ai_gen.style_rect);
    let mut picked = None;
    PresetsWindow::new(Bind::bound(&mut app.ui.ai_gen.style_open), &tab, &placed, opener).show(ui, |ui, _| {
        picked = grid(ui, tab.get(), current);
    });
    app.ui.ai_gen.style_tab = tab.get();
    app.ui.ai_gen.style_rect = placed.get();
    if let Some(style) = picked {
        app.ui.ai_gen.style = style.map(|s| s.id.to_string()).unwrap_or_default();
        app.ui.ai_gen.style_open = false;
    }
}

/// The tiles of tab `tab` in as many columns as the window's width holds, scrolling under a [`GridBar`] at the right
/// edge while they are taller than the window. Returns the tile clicked: `Some(None)` for No preset.
///
/// The layer measures its body against the whole screen's width, so a measuring pass lays the tiles out in the
/// columns the last real pass found room for ([`COLUMNS`] at first): the window opens at that width and keeps it
/// until the user resizes it, and the real pass reflows to the width the window has. The bar's width is kept beside
/// the tiles whether it shows or not, so the columns don't change from tab to tab.
fn grid(ui: &mut egui::Ui, tab: usize, current: Option<&str>) -> Option<Option<&'static Style>> {
    let group = tab.checked_sub(1).and_then(|i| StyleGroup::ALL.get(i).copied());
    let tiles: Vec<Option<&'static Style>> = std::iter::once(None).chain(STYLES.iter().filter(|s| group.is_none_or(|g| s.group == g)).map(Some)).collect();
    let key = egui::Id::unique("ai-style-grid-columns");
    let bar = StyleCx::with_root(ui.ctx(), |cx| cx.scroll_bar_of(ScrollBarKind::Docked)).thickness;
    let rows_height = |columns: usize| tiles.len().div_ceil(columns) as f32 * (TILE + 22.0 + GAP) - GAP;
    if ui.is_sizing_pass() {
        // The size the window opens at: the columns' width and the bar's, and at most GRID_HEIGHT of rows.
        let columns = ui.ctx().data(|d| d.get_temp::<usize>(key)).unwrap_or(COLUMNS);
        ui.allocate_space(vec2(columns as f32 * (TILE + GAP) - GAP + bar, rows_height(columns).min(GRID_HEIGHT)));
        return None;
    }
    let area = ui.available_rect_before_wrap();
    let columns = (((area.width() - bar + GAP) / (TILE + GAP)).floor() as usize).max(1);
    ui.ctx().data_mut(|d| d.insert_temp(key, columns));
    let content = rows_height(columns);
    // The window shows the body down to its clip, which may end before the room the body is offered.
    let shown = area.height().min(ui.clip_rect().bottom() - area.top());
    let height = if shown.is_finite() && shown > 0.0 { shown } else { content.min(GRID_HEIGHT) };
    let area = egui::Rect::from_min_size(area.min, vec2(area.width(), height));
    let track = egui::Rect::from_min_max(pos2(area.right() - bar, area.top()), area.max);
    let view = egui::Rect::from_min_max(area.min, pos2(track.left(), area.bottom()));
    // Each tab keeps its own place in the list. The scroll area moves it by the wheel and to a tile focused by key;
    // the bar by its thumb and its track.
    let offset_key = egui::Id::unique("ai-style-grid-offset").with(tab);
    let room = (content - height).max(0.0);
    let mut offset = ui.ctx().data(|d| d.get_temp::<f32>(offset_key)).unwrap_or(0.0).clamp(0.0, room);
    let before = offset;
    if room > 0.0 {
        GridBar::new(Bind::bound(&mut offset), content, height).show_at(ui, track);
    }
    let mut picked = None;
    let scrolled = ui.scope_builder(egui::UiBuilder::new().max_rect(view), |ui| {
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(("ai-style-grid", tab))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false, false])
            .max_height(height);
        if offset != before {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            for row in tiles.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for style in row {
                        let on = style.map(|s| s.id) == current;
                        if tile(ui, *style, on).clicked() {
                            picked = Some(*style);
                        }
                    }
                });
            }
        })
    });
    let offset = scrolled.inner.state.clamped_offset().y;
    ui.ctx().data_mut(|d| d.insert_temp(offset_key, offset));
    ui.advance_cursor_after_rect(area);
    picked
}

/// The grid's bar's track; its thumb is `BAR.at(THUMB)`.
const BAR: PartKey = PartKey::new("bar");

/// The style of the grid's bar: the canvas's look (docked), at its full thickness on its own track in every state.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
struct GridBarStyle {
    #[style(metrics)]
    metrics: Metrics,
    look: ScrollBarLook,
}

/// The vertical scroll bar beside the tiles, bound to how far down the list is scrolled (points from its top): the
/// thumb drags, and a press on the track pages towards it. The wheel and the keys scroll the list itself.
#[derive(Component)]
#[component(unregistered)]
struct GridBar<'a> {
    #[common]
    common: Common,
    offset: Bind<'a, f32>,
    /// The height of every row, and of the part of them in view.
    content: f32,
    view: f32,
}

impl<'a> GridBar<'a> {
    fn new(offset: Bind<'a, f32>, content: f32, view: f32) -> Self {
        GridBar { common: Common::new(), offset, content, view }
    }

    /// The rows, and the part in view at `offset`.
    fn span(&self, offset: f32) -> ScrollSpan {
        let start = f64::from(offset);
        ScrollSpan::new(0.0..f64::from(self.content.max(self.view)), start..start + f64::from(self.view))
    }

    /// The furthest the rows scroll.
    fn room(&self) -> f32 {
        (self.content - self.view).max(0.0)
    }
}

impl Component for GridBar<'_> {
    const ROLE: Role = Role::GROUP;
    const STOPS: bool = false;
    type Style = GridBarStyle;
    type Output = EditResponse<f32>;

    fn style(cx: &StyleCx<'_>, _: GridBarStyleKey) -> GridBarStyle {
        GridBarStyle { metrics: Metrics::new(), look: cx.scroll_bar_of(ScrollBarKind::Docked) }
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn bind(&mut self) -> Option<BindMut<'_, f32>> {
        Some(self.offset.reborrow())
    }

    fn parts(&mut self, rect: egui::Rect, cx: &mut PartsCx<'_, Self>) {
        let Some(offset) = cx.value().copied() else { return };
        cx.scroll(BAR, rect, Axis::Y, self.span(offset));
    }

    fn update(&mut self, cx: &mut Cx<'_, Self>) {
        if let Some(range) = cx.scroll_moved(BAR) {
            let to = (range.start as f32).clamp(0.0, self.room());
            if cx.value().copied() != Some(to) {
                cx.set(to);
            }
        }
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let Some(offset) = cx.value().copied() else { return };
        ui.draw().scroll_bar(&cx.style.look, cx.rect, Axis::Y, false, &self.span(offset), &cx.scroll_bar_state(BAR));
    }

    fn access(&self, access: &mut Access, cx: &Cx<'_, Self>) {
        let Some(offset) = cx.value().copied() else { return };
        let node = access.node(BAR, Role::SCROLL_BAR);
        node.name = Name::Text(tr("Style presets scroll bar").to_owned());
        node.numeric = Some(Numeric::new(f64::from(offset), 0.0..=f64::from(self.room())));
    }

    fn finish(self, cx: Cx<'_, Self>) -> EditResponse<f32> {
        cx.edit_response()
    }
}

/// The style of the picker window: its surface and padding come from the container.
#[derive(Clone, Debug, Default, PartialEq, ComponentStyle)]
#[non_exhaustive]
struct PresetsWindowStyle {
    #[style(metrics)]
    metrics: Metrics,
}

/// What the picker window reports: its response.
#[derive(Debug)]
struct PresetsWindowOutput {
    response: egui::Response,
}

impl Outcome for PresetsWindowOutput {
    type Value = NoValue;

    fn response(&self) -> &egui::Response {
        &self.response
    }

    fn unshown(response: egui::Response) -> Option<Self> {
        Some(PresetsWindowOutput { response })
    }
}

/// The picker's layer: a popup surface that hangs from the Style button until the user moves it by its header, with
/// grips on its edges. Its header is the title and the tab list; the tiles scroll under it, beside their own bar.
#[derive(Component, Props)]
#[component(container, unregistered)]
struct PresetsWindow<'a> {
    #[common]
    common: Common,
    #[container]
    props: ContainerProps,
    open: Bind<'a, bool>,
    tab: &'a Cell<usize>,
    placed: &'a Cell<Option<egui::Rect>>,
}

impl<'a> PresetsWindow<'a> {
    /// Open while `open` holds, at `placed` once the user moved or resized it (hanging from `opener` until then),
    /// showing tab `tab`.
    fn new(open: Bind<'a, bool>, tab: &'a Cell<usize>, placed: &'a Cell<Option<egui::Rect>>, opener: &egui::Response) -> Self {
        // Above the button, its right edges aligned, as the button sits low in the panel; the layer turns below
        // where there is no room above.
        let mut overlay = Overlay::popover()
            .place(Place::anchored(Anchor::Response, Side::Above, Align::End))
            .opened_by(opener, Trigger::Manual)
            .movable(true)
            .resizable(true)
            .fit(OverlayFit::Resizable)
            .size(vec2(MIN_WIDTH, HEIGHT))
            .min_size(vec2(MIN_WIDTH, HEIGHT))
            // The tiles scroll under their own bar, in the canvas's look, rather than the body's.
            .body_scroll(false);
        if let Some(rect) = placed.get() {
            overlay = overlay.placed(rect);
        }
        let props = ContainerProps::new()
            .surface(SurfaceKind::Popup)
            .pad(POPUP_PAD)
            .mount(Mount::Layer(overlay))
            .header(HeaderRegion::new().band(DragBand::Move));
        PresetsWindow { common: Common::new(), props, open, tab, placed }.label(tr("Style Presets"))
    }
}

impl Component for PresetsWindow<'_> {
    const ROLE: Role = Role::DIALOG;
    const PARTS: bool = false;
    const STOPS: bool = false;
    type Style = PresetsWindowStyle;
    type Output = PresetsWindowOutput;

    fn style(_: &StyleCx<'_>, _: PresetsWindowStyleKey) -> PresetsWindowStyle {
        PresetsWindowStyle::default()
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn finish(self, cx: Cx<'_, Self>) -> PresetsWindowOutput {
        PresetsWindowOutput { response: cx.response.clone() }
    }
}

impl Container for PresetsWindow<'_> {
    fn open(&mut self) -> Option<BindMut<'_, bool>> {
        Some(self.open.reborrow())
    }

    /// The user moved or resized it: it stays where they leave it, as large as they leave it.
    fn placement_changed(&mut self, change: &PlacementChange) {
        self.placed.set(Some(change.rect));
    }

    /// The title and the tab list: the band that moves the window lies under them, so the tabs keep their clicks.
    fn header(&mut self, ui: &mut egui::Ui, _: &mut BodyCx<'_>) {
        let t = Tokens::get(ui.ctx());
        // The title, painted: it takes no press, so a drag on it reaches the move band under it, and it measures as
        // tall as it shows (a label measures differently in a measuring pass).
        let title = ui.painter().layout_no_wrap(tr("Style Presets").to_string(), crate::theme::medium(13.0), t.text);
        let (rect, resp) = ui.allocate_exact_size(title.size(), egui::Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, tr("Style Presets")));
        ui.painter().galley(rect.min, title, t.text);
        ui.add_space(4.0);
        // A measuring pass wraps the tabs at the width the window had in the last real pass, as the grid lays out
        // its columns; the real pass wraps them at the width it has.
        let key = egui::Id::unique("ai-style-header-width");
        let width = if ui.is_sizing_pass() {
            ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(f32::INFINITY)
        } else {
            let width = ui.available_width();
            ui.ctx().data_mut(|d| d.insert_temp(key, width));
            width
        };
        let mut tab = self.tab.get();
        tabs(ui, &mut tab, width);
        self.tab.set(tab);
        ui.add_space(8.0);
    }
}

/// The height of a tab, and the space between two lines of tabs.
const TAB_HEIGHT: f32 = 22.0;
const TAB_LINE_GAP: f32 = 6.0;
const TAB_GAP: f32 = 14.0;

/// The tab list, in as many lines as `width` needs: All, then each group; the one in front underlined in the accent,
/// a rule under them. The lines are laid out here rather than by egui's wrapping, which wraps differently in a
/// measuring pass, so the header measures as tall as it is.
fn tabs(ui: &mut egui::Ui, tab: &mut usize, width: f32) {
    let t = Tokens::get(ui.ctx());
    let names: Vec<&str> = std::iter::once(tr("All")).chain(StyleGroup::ALL.iter().map(|g| tr(g.label()))).collect();
    let galleys: Vec<_> = names.iter().map(|name| ui.painter().layout_no_wrap((*name).to_string(), crate::theme::medium(11.5), t.text)).collect();
    // Each tab's offset from the list's top left: a tab that would cross `width` starts the next line.
    let (mut x, mut y, mut used) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut at = Vec::with_capacity(galleys.len());
    for galley in &galleys {
        let w = galley.size().x;
        if x > 0.0 && x + w > width {
            (x, y) = (0.0, y + TAB_HEIGHT + TAB_LINE_GAP);
        }
        at.push(vec2(x, y));
        used = used.max(x + w);
        x += w + TAB_GAP;
    }
    let (list, _) = ui.allocate_exact_size(vec2(used.min(width), y + TAB_HEIGHT), egui::Sense::hover());
    for (i, ((name, galley), offset)) in names.iter().zip(galleys).zip(at).enumerate() {
        let on = *tab == i;
        let rect = egui::Rect::from_min_size(list.min + offset, vec2(galley.size().x, TAB_HEIGHT));
        let resp = ui.interact(rect, ui.scope_id().with(("ai-style-tab", i)), egui::Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, *name));
        let color = if on || resp.hovered() { t.text } else { t.text_dim };
        ui.painter().galley_with_override_text_color(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0 - 1.0), galley, color);
        if on {
            let line = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom());
            ui.painter().rect_filled(line, 1.0, t.accent);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            *tab = i;
        }
    }
    let r = ui.max_rect();
    ui.painter().line_segment([egui::pos2(r.left(), list.bottom()), egui::pos2(r.right(), list.bottom())], egui::Stroke::new(1.0, t.separator));
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
            crate::ai_widgets::paint_icon(ui, egui::Rect::from_center_size(picture.center(), vec2(28.0, 28.0)), "ban", 26.0, t.text_faint);
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
        crate::ai_widgets::paint_icon(ui, badge.shrink(3.0), "check", 13.0, t.primary_text);
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
