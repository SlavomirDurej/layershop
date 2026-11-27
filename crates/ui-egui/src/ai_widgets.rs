//! The hand-drawn egui widgets the AI Gen panel and the Local AI setup wizard are built from.
//!
//! LayerShop's AI screens were written on PhotoCraft's own widget set, which upstream replaced with
//! the craft UI framework's components (photocraft#15449). These are the pieces those screens use,
//! with the signatures they had, drawn in the theme's [`Tokens`] so they match the panels around
//! them. Each is small on purpose: moving a screen onto the framework's components retires the
//! functions it no longer calls.

use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::theme::{self, Tokens};

// ----------------------------------------------------------------------------- icons

/// The catalog name of icon `name`: the framework's catalog spells names with `_` (`chevron_down`),
/// the app's own set (`icon_data.rs`) with `-` (`triangle-alert`). Whichever is registered wins.
pub(crate) fn icon_id(name: &str) -> craft::behavior::IconId {
    use craft::behavior::IconId;
    use craft::ui::primitives::has_icon;
    let owned = |s: String| IconId::from(std::borrow::Cow::Owned(s));
    let id = owned(name.to_owned());
    if has_icon(&id) {
        return id;
    }
    let underscored = owned(name.replace('-', "_"));
    if has_icon(&underscored) {
        return underscored;
    }
    let renamed = match name {
        "settings" => "gear",
        "rotate-cw" => "refresh",
        "folder-open" => "folder",
        "pencil" => "edit",
        _ => name,
    };
    let renamed = owned(renamed.replace('-', "_"));
    if has_icon(&renamed) { renamed } else { id }
}

/// Paints icon `name` centred in `rect`, `size` points square, in `tint`.
pub fn paint_icon(ui: &Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    use craft::ui::primitives::{IconSpec, paint_icon};
    let env = craft::ui::runtime::env::Env::of(ui);
    let spec = IconSpec::svg(icon_id(name)).color(craft::ui::style::StyleColor(tint));
    let r = Rect::from_center_size(rect.center(), Vec2::splat(size));
    paint_icon(ui.painter(), &env.tokens, env.direction, r, &spec);
}

/// Icon `name` as a widget, `size` points square, in `tint`.
pub fn icon(ui: &mut Ui, name: &str, size: f32, tint: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui, rect, name, size, tint);
    resp
}

/// Square icon button, `box_size` points: transparent until hovered; `selected` gets the accent
/// treatment. `tooltip` names it for screen readers and tests, and shows on hover.
pub fn icon_button(ui: &mut Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());
    let enabled = ui.is_enabled();
    let tint = if selected {
        ui.painter().rect_filled(rect, t.radius_sm, t.accent_soft);
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.accent_border), StrokeKind::Inside);
        t.accent_text
    } else if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        t.text
    } else {
        t.icon
    };
    if resp.has_focus() {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    let tint = if enabled { tint } else { tint.gamma_multiply(0.45) };
    paint_icon(ui, rect, name, (box_size * 0.52).round(), tint);
    let label = if tooltip.is_empty() { name.to_owned() } else { tl!(tooltip).to_owned() };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    if tooltip.is_empty() { resp } else { resp.on_hover_text(label) }
}

// ----------------------------------------------------------------------------- buttons

/// What a dialog button does, which decides where the platform's button order puts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonRole {
    /// The default answer (OK, Save, Yes), drawn as the primary button.
    Default,
    /// Another answer that closes the dialog (Don't Save, No).
    Alternate,
    /// Closes the dialog without acting.
    Cancel,
    /// Acts but keeps the dialog open (Apply).
    Apply,
}

/// One button of a [`dialog_buttons`] row.
pub struct DialogButton<'a> {
    pub role: ButtonRole,
    pub label: &'a str,
    pub min_width: f32,
    pub enabled: bool,
}

impl<'a> DialogButton<'a> {
    pub fn new(role: ButtonRole, label: &'a str, min_width: f32) -> Self {
        Self { role, label, min_width, enabled: true }
    }

    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
}

const BUTTON_H: f32 = 26.0;

/// The text of `label` as a button lays it out, and the button's size.
fn button_layout(ui: &Ui, label: &str, min_width: f32, fg: Color32) -> (std::sync::Arc<egui::Galley>, Vec2) {
    let galley = ui.painter().layout_no_wrap(tl!(label).to_owned(), theme::medium(12.5), fg);
    let size = vec2((galley.size().x + 24.0).max(min_width), BUTTON_H);
    (galley, size)
}

fn button_impl(ui: &mut Ui, label: &str, min_width: f32, fill: Color32, fg: Color32, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (galley, size) = button_layout(ui, label, min_width, fg);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let fill = if !enabled {
        fill.gamma_multiply(0.5)
    } else if resp.is_pointer_button_down_on() {
        if primary { fill.gamma_multiply(0.85) } else { t.pressed }
    } else if resp.hovered() {
        if primary { fill.gamma_multiply(0.92) } else { t.hover }
    } else {
        fill
    };
    ui.painter().rect_filled(rect, t.radius_sm, fill);
    if !primary {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(1.0), t.radius_sm, Stroke::new(1.0, t.accent), StrokeKind::Outside);
    }
    let fg = if enabled { fg } else { fg.gamma_multiply(0.5) };
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tl!(label)));
    resp
}

/// The prominent button of a screen (Generate, Install).
pub fn primary_button(ui: &mut Ui, label: &str, min_width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    button_impl(ui, label, min_width, t.primary_bg, t.primary_text, true)
}

/// Any other button.
pub fn secondary_button(ui: &mut Ui, label: &str, min_width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    button_impl(ui, label, min_width, t.field, t.text, false)
}

/// The width of the widest of `labels` as a button, for a column of equal-width buttons.
pub fn button_width(ui: &Ui, labels: &[&str]) -> f32 {
    let fg = Tokens::get(ui.ctx()).text;
    labels.iter().map(|l| button_layout(ui, l, 0.0, fg).1.x).fold(0.0, f32::max)
}

/// A dialog's button row, right-aligned in the platform's order (macOS: Cancel before the default
/// answer, which is last; elsewhere the default answer first). Returns the role of the button
/// clicked this frame.
pub fn dialog_buttons(ui: &mut Ui, buttons: &[DialogButton]) -> Option<ButtonRole> {
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    let rank = |role: ButtonRole| match (role, mac) {
        (ButtonRole::Apply, _) => 0,
        (ButtonRole::Alternate, _) => 1,
        (ButtonRole::Cancel, true) => 2,
        (ButtonRole::Default, true) => 3,
        (ButtonRole::Default, false) => 2,
        (ButtonRole::Cancel, false) => 3,
    };
    let mut order: Vec<&DialogButton> = buttons.iter().collect();
    order.sort_by_key(|b| rank(b.role));
    let mut clicked = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        for b in order.iter().rev() {
            let resp = ui.add_enabled_ui(b.enabled, |ui| {
                if b.role == ButtonRole::Default { primary_button(ui, b.label, b.min_width) } else { secondary_button(ui, b.label, b.min_width) }
            });
            if resp.inner.clicked() {
                clicked = Some(b.role);
            }
        }
    });
    clicked
}

/// A checkbox with its label; clicking either toggles it.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    checkbox_named(ui, on, label, label)
}

/// [`checkbox`] whose accessible name is `name` rather than its label: for a box shown without a
/// label (a list's row box), which screen readers and tests still need to name.
pub fn checkbox_named(ui: &mut Ui, on: &mut bool, label: &str, name: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let enabled = ui.is_enabled();
    let mut resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
            let fill = if *on { t.accent } else { t.field };
            ui.painter().rect_filled(rect, 3.0, if enabled { fill } else { fill.gamma_multiply(0.5) });
            ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, if *on { t.accent } else { t.field_border }), StrokeKind::Inside);
            if *on {
                let c = rect.center();
                let stroke = Stroke::new(1.6, Color32::WHITE);
                ui.painter().line_segment([c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.6)], stroke);
                ui.painter().line_segment([c + vec2(-1.0, 2.6), c + vec2(3.6, -2.6)], stroke);
            }
            if label.is_empty() {
                return resp;
            }
            let text = egui::RichText::new(tl!(label)).color(if enabled { t.text } else { t.text_faint });
            let label_resp = ui.add(egui::Label::new(text).selectable(false).sense(Sense::click()));
            resp.union(label_resp)
        })
        .inner;
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, tl!(name)));
    resp
}

// ----------------------------------------------------------------------------- panel pieces

/// A raised (or flat) rounded fill, the look of a selected tab or a card.
pub fn surface(ui: &Ui, rect: Rect, fill: Color32, raised: bool) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, t.radius_sm, fill);
    if raised && t.bevel {
        p.line_segment([rect.left_top() + vec2(2.0, 0.5), rect.right_top() + vec2(-2.0, 0.5)], Stroke::new(1.0, Color32::from_white_alpha(18)));
    }
}

/// A tab of a row of pill tabs.
pub fn pill_tab(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let translated = tl!(label);
    let galley = ui.painter().layout_no_wrap(translated.to_string(), theme::medium(12.5), t.text);
    let size = vec2(galley.size().x + 20.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if selected {
        surface(ui, rect, t.hover, true);
        if !t.bevel {
            ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        }
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover.gamma_multiply(0.6));
    }
    let color = if selected { t.text } else { t.text_dim };
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, translated));
    resp
}

/// A small section heading.
pub fn section_label(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!(text)).font(theme::medium(11.5)).color(t.text_faint));
}

/// Hairline separator across the room there is.
pub fn hairline(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(1.0, t.separator));
}

/// Vertical hairline for horizontal layouts.
pub fn vline(ui: &mut Ui, height: f32) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(9.0, height), Sense::hover());
    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, t.separator));
}

/// Small chevron for dropdowns (`egui::ComboBox::icon`).
pub fn chevron_icon(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let c = rect.center();
    let s = 3.2;
    let stroke = Stroke::new(1.3, visuals.fg_stroke.color.gamma_multiply(0.8));
    ui.painter().line_segment([c + vec2(-s, -s * 0.5), c + vec2(0.0, s * 0.5)], stroke);
    ui.painter().line_segment([c + vec2(0.0, s * 0.5), c + vec2(s, -s * 0.5)], stroke);
}

/// A transparency checkerboard in `rect`, `cell` points a square.
pub fn checker(painter: &egui::Painter, rect: Rect, cell: f32) {
    let cell = cell.max(2.0);
    painter.rect_filled(rect, 0.0, Color32::from_gray(204));
    let (cols, rows) = ((rect.width() / cell).ceil() as i32, (rect.height() / cell).ceil() as i32);
    for y in 0..rows.min(4096) {
        for x in 0..cols.min(4096) {
            if (x + y) % 2 == 1 {
                let min = rect.min + vec2(x as f32 * cell, y as f32 * cell);
                let r = Rect::from_min_size(min, Vec2::splat(cell)).intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(255));
            }
        }
    }
}

/// `text` on one line, cut with "…" to fit `max_w`; the whole text shows on hover when cut.
pub fn elided_label(ui: &mut Ui, text: &str, max_w: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, t.text);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    let resp = ui.add(egui::Label::new(galley).selectable(false));
    if cut { resp.on_hover_text(text) } else { resp }
}

// ----------------------------------------------------------------------------- text and numbers

/// What a text field did in a pass ([`show_text`]).
pub(crate) struct Typed {
    /// The text changed in this pass.
    pub changed: bool,
    /// Enter (Ctrl+Enter in a field of several lines) submitted it, in a field made with `submits()`.
    pub submitted: bool,
    /// The keyboard focus left the field in this pass: a click elsewhere, Tab or Escape.
    pub left: bool,
    /// The field's response: its rect, and its hover for a tooltip.
    pub response: Response,
}

/// Shows `field` (a framework text field, plain text) `width` points wide, the room there is when 0, as tall as its
/// lines (`lines`, growing with its text up to `max_lines`). `key` names the field among its neighbours, so fields
/// that share a label keep their own typing.
///
/// The AI screens type into the framework's text field, never egui's own `TextEdit`: the framework keeps the letters,
/// Ctrl+Z and the other editing keys in a text field it knows, and runs them as the app's shortcuts in an egui widget
/// it doesn't (#16: F went full screen and Ctrl+Z undid the document from the prompt). Escape keeps the text, as
/// egui's field did. A field is never held taller than its lines: the framework centres the text on the field's
/// height.
pub(crate) fn show_text(ui: &mut Ui, key: impl egui::AsIdSalt, width: f32, field: craft::ui::text::TextField<'_>) -> Typed {
    use craft::ui::contract::{Props as _, Show as _, Sizing};
    let width = if width > 0.0 { width } else { ui.available_width() };
    let room = ui.available_size_before_wrap().y.max(ui.spacing().interact_size.y);
    let field = field.keeps_on_escape().id_salt(("ai-text", key)).sizing(Sizing::FillWidth);
    let out = ui.allocate_ui_with_layout(vec2(width, room), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(width);
        field.show(ui)
    });
    let shown = out.inner;
    Typed { changed: shown.value().is_some(), submitted: shown.submitted, left: shown.text_lost_focus, response: shown.response }
}

/// A one-line text field `width` points wide (the room there is when 0), named `label`, with `hint` while empty.
pub(crate) fn text_field(ui: &mut Ui, key: impl egui::AsIdSalt, label: &str, text: &mut String, hint: &str, width: f32, mono: bool) -> Typed {
    let field = plain(label, text).placeholder(tl!(hint).to_string());
    show_text(ui, key, width, if mono { field.mono() } else { field })
}

/// A plain text field named `label` (English, shown in the UI language) that edits `text` as it is typed.
pub(crate) fn plain<'a>(label: &str, text: &'a mut String) -> craft::ui::text::TextField<'a> {
    craft::ui::text::TextField::new(tl!(label).to_string(), craft::ui::contract::Bind::bound(text))
}

/// A number typed as text: digits with `.` or `,` as the decimal mark, an optional sign. Long
/// input and anything else is `None`.
pub fn parse_num(text: &str) -> Option<f64> {
    if text.len() > 1024 {
        return None;
    }
    let t = text.trim().replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number field `width` points wide: typed and committed on Enter or leaving it, clamped to
/// `range`, with `suffix` after the value.
pub fn value_field(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str, width: f32) -> Response {
    let mut v = f64::from(*value);
    let resp = ui.add_sized(
        vec2(width.max(36.0), 22.0),
        egui::DragValue::new(&mut v).range(f64::from(*range.start())..=f64::from(*range.end())).suffix(suffix).speed(0.1),
    );
    let v = v as f32;
    if v.is_finite() && v != *value {
        *value = v.clamp(*range.start(), *range.end());
    }
    resp
}

/// A thin slider over `range`.
pub fn slider(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, _gradient: Option<&[Color32]>) -> Response {
    ui.add(egui::Slider::new(value, range).show_value(false))
}

// ----------------------------------------------------------------------------- dropdowns

/// How many options a searchable dropdown has before its list shows the search field.
pub const SEARCH_FROM: usize = 10;

/// Whether `label` holds `query` (its words, any case): "krea dark" finds "krea2_darkbrush".
pub fn search_matches(label: &str, query: &str) -> bool {
    let label = label.to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| label.contains(w))
}

/// A list that scrolls when it would run past the window's bottom.
pub fn menu_scroll<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let room = (ui.ctx().content_rect().height() - 48.0).max(120.0);
    egui::ScrollArea::vertical().max_height(room).show(ui, add_contents).inner
}

/// A dropdown of `options` (value, English label, translated when shown), `width` points wide.
/// Returns whether the value changed this frame.
pub fn dropdown<T: PartialEq + Clone>(ui: &mut Ui, id: &str, name: &str, current: &mut T, options: &[(T, &str)], width: f32) -> bool {
    dropdown_with(ui, id, name, current, options, &[], width, true, false)
}

/// [`dropdown`] whose labels are shown as given (names, not built-in labels).
pub fn dropdown_literal<T: PartialEq + Clone>(ui: &mut Ui, id: &str, name: &str, current: &mut T, options: &[(T, &str)], width: f32) -> bool {
    dropdown_with(ui, id, name, current, options, &[], width, false, false)
}

/// [`dropdown_literal`] whose open list starts with a search field once it has [`SEARCH_FROM`]
/// options or more: typing narrows the list to the options whose label holds the typed text.
pub fn dropdown_search<T: PartialEq + Clone>(ui: &mut Ui, id: &str, name: &str, current: &mut T, options: &[(T, &str)], width: f32) -> bool {
    dropdown_with(ui, id, name, current, options, &[], width, false, true)
}

/// [`dropdown_search`] with headings: `(index, heading)` puts `heading` above option `index`.
/// While the search narrows the list, the headings are left out.
pub fn dropdown_sectioned_search<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: &str,
    name: &str,
    current: &mut T,
    options: &[(T, &str)],
    headings: &[(usize, &str)],
    width: f32,
) -> bool {
    dropdown_with(ui, id, name, current, options, headings, width, false, true)
}

#[allow(clippy::too_many_arguments)]
fn dropdown_with<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: &str,
    name: &str,
    current: &mut T,
    options: &[(T, &str)],
    headings: &[(usize, &str)],
    width: f32,
    translate: bool,
    search: bool,
) -> bool {
    let t = Tokens::get(ui.ctx());
    let shown = |l: &str| if translate { tl!(l).to_string() } else { l.to_string() };
    let selected = options.iter().find(|(v, _)| v == current).map(|(_, l)| shown(l)).unwrap_or_else(|| "—".into());
    let combo_id = egui::Id::unique(("ai-dropdown", id));
    let query_id = combo_id.with("query");
    let searching = search && options.len() >= SEARCH_FROM;
    let mut changed = false;
    let width = if width > 0.0 { width } else { ui.available_width() };
    // egui's combo box, as the AI screens had it: the framework's ComboBox takes another shape of
    // options (moving these screens onto it retires this function).
    #[allow(clippy::disallowed_types)]
    let resp = egui::ComboBox::from_id_salt(combo_id).selected_text(selected).width(width).icon(chevron_icon).show_ui(ui, |ui| {
        let mut query: String = ui.data(|d| d.get_temp(query_id)).unwrap_or_default();
        let mut pick_first = false;
        if searching {
            use craft::ui::contract::Props as _;
            // The list opens ready to type into.
            let field = plain("Search", &mut query).placeholder(tl!("Search").to_string()).submits();
            let field = field.request_focus(craft::ui::runtime::focus::FocusRequest::WhenNone);
            pick_first = show_text(ui, ("dropdown-search", id), width - 16.0, field).submitted;
            ui.data_mut(|d| d.insert_temp(query_id, query.clone()));
        }
        let filtering = searching && !query.trim().is_empty();
        menu_scroll(ui, |ui| {
            for (i, (value, label)) in options.iter().enumerate() {
                let label = shown(label);
                if filtering && !search_matches(&label, &query) {
                    continue;
                }
                if !filtering && let Some((_, heading)) = headings.iter().find(|(at, _)| *at == i) {
                    ui.label(egui::RichText::new(tl!(heading)).font(theme::medium(11.0)).color(t.text_faint));
                }
                let picked = ui.selectable_label(value == current, label).clicked();
                if picked || std::mem::take(&mut pick_first) {
                    if value != current {
                        *current = value.clone();
                        changed = true;
                    }
                    ui.close();
                }
            }
        });
    });
    if resp.response.clicked() {
        ui.data_mut(|d| d.remove::<String>(query_id));
    }
    resp.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, tl!(name)));
    changed
}

// ----------------------------------------------------------------------------- pictures

/// Cheap identity of a surface's pixels: its default (untouched) pixel, tile coordinates and
/// `Arc` pointers. The default pixel matters: inverting or filling a tile-less mask only changes
/// it.
pub fn surface_fingerprint(s: &photocraft_raster::Surface) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ s.tile_count() as u64;
    for &b in s.default_bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    for (c, t) in s.tiles() {
        let p = std::sync::Arc::as_ptr(t) as usize as u64;
        h = (h ^ p ^ ((c.tx as u64) << 32 | c.ty as u32 as u64)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Square thumbnail `side` pixels wide of the document area `frame`, letterboxed, sampling
/// `f(x, y)` (RGBA, 0–1) in document space.
pub fn thumb_image(frame: photocraft_geom::Rect, side: usize, mut f: impl FnMut(i32, i32) -> [f32; 4]) -> egui::ColorImage {
    let (w, h) = (frame.width().max(1) as f32, frame.height().max(1) as f32);
    let scale = w.max(h) / side.max(1) as f32;
    let (ox, oy) = ((side as f32 - w / scale) / 2.0, (side as f32 - h / scale) / 2.0);
    let mut px = vec![Color32::TRANSPARENT; side * side];
    for ty in 0..side {
        for tx in 0..side {
            let dx = (tx as f32 - ox + 0.5) * scale;
            let dy = (ty as f32 - oy + 0.5) * scale;
            if dx < 0.0 || dy < 0.0 || dx >= w || dy >= h {
                continue;
            }
            let c = f(frame.x0.saturating_add(dx as i32), frame.y0.saturating_add(dy as i32));
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            if let Some(out) = px.get_mut(ty * side + tx) {
                *out = Color32::from_rgba_unmultiplied(q(c[0]), q(c[1]), q(c[2]), q(c[3]));
            }
        }
    }
    egui::ColorImage::new([side, side], px)
}

/// A progress bar in `rect`: `frac` of it filled, or a band sweeping across while the amount is
/// unknown (`None`).
pub fn progress_bar(ui: &Ui, rect: Rect, frac: Option<f32>, t: &Tokens) {
    let p = ui.painter();
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, t.field);
    p.rect_stroke(rect, r, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    let fill = match frac {
        Some(f) => Rect::from_min_size(rect.min, vec2((rect.width() * f.clamp(0.0, 1.0)).max(rect.height()), rect.height())),
        None => {
            let time = ui.input(|i| i.time) as f32;
            let w = rect.width() * 0.3;
            let x = rect.left() + ((time * 0.8).fract() * (rect.width() + w)) - w;
            Rect::from_min_max(pos2(x.max(rect.left()), rect.top()), pos2((x + w).min(rect.right()), rect.bottom()))
        }
    };
    if fill.width() > 0.0 {
        p.rect_filled(fill, r, t.accent);
    }
    if frac.is_none() {
        ui.ctx().request_repaint();
    }
}

/// The app's mark (its icon) painted into the square `r`.
pub fn paint_mark(ui: &Ui, r: Rect) {
    let id = egui::Id::unique("layershop-ai-mark");
    let texture = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
        let texture = ui.ctx().load_texture("layershop-mark", crate::brand::icon_image(), egui::TextureOptions::LINEAR);
        ui.ctx().data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    });
    ui.painter().image(texture.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}
