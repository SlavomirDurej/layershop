//! LoRAs: small add-ons to a local model's weights that steer it to a style, a character or a
//! subject. Each model keeps the ones it offers in its settings (Preferences › Generative AI:
//! the file, as ComfyUI's `models/loras` names it, a name, the strength it starts at and its
//! trigger words; [`photocraft_aigen::Model::loras`]). The panel's Advanced section adds them
//! to the model's generations, up to [`MAX_LORAS`], each with a strength slider, an eye to leave
//! it out for a while and a bin. What was added is kept per model, so each model comes back
//! with its own; `ai.generate`'s `loras` is what is sent.
//!
//! The model's speed LoRA (its settings' Speed LoRA, the few-step schedule its steps and CFG
//! are set for) is not one of these: it always applies, and the panel doesn't show it.
//!
//! LoRAs add their changes to the weights, so the order they are listed in doesn't change the
//! picture: the panel lists them in the order they were added.

use egui::{RichText, vec2};
use photocraft_aigen::{LORA_STRENGTHS, Lora, MAX_LORAS, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{edit_label, field_width, open_settings, text_of, tr};
use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// The strengths the panel's slider runs over; the field beside it takes [`LORA_STRENGTHS`].
const SLIDER: std::ops::RangeInclusive<f32> = 0.0..=2.0;

/// A LoRA added to the panel's generations with a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PanelLora {
    /// As [`Lora::file`] names it.
    pub file: String,
    pub strength: f64,
    /// Sent with the generation; off, it stays in the list without applying.
    pub on: bool,
}

impl Default for PanelLora {
    fn default() -> Self {
        PanelLora { file: String::new(), strength: 1.0, on: true }
    }
}

/// The LoRAs added for `model`.
fn added<'a>(app: &'a PhotocraftApp, model: &Model) -> &'a [PanelLora] {
    app.ui.ai_gen.loras.get(&model.id).map_or(&[], Vec::as_slice)
}

/// The rows the LoRAs take in the Advanced section: the heading with its +, and one per LoRA.
pub(super) fn rows(ui: &super::AiGenUi, model: Option<&Model>) -> usize {
    match model.filter(|m| m.takes_loras()) {
        Some(m) => 1 + ui.loras.get(&m.id).map_or(0, |l| l.len().min(MAX_LORAS)),
        None => 0,
    }
}

/// `ai.generate`'s `loras` for `model`: those switched on, with their strengths. `None` for a
/// model that takes none, or when none is on.
pub(super) fn param(app: &PhotocraftApp, model: &Model) -> Option<Value> {
    if !model.takes_loras() {
        return None;
    }
    let on: Vec<Value> = added(app, model)
        .iter()
        .filter(|l| l.on && !l.file.trim().is_empty())
        .take(MAX_LORAS)
        .map(|l| json!({"lora": l.file, "strength": l.strength}))
        .collect();
    (!on.is_empty()).then_some(Value::Array(on))
}

/// Load Prompt: the LoRAs a layer was generated with become `model`'s in the panel.
pub(super) fn restore(app: &mut PhotocraftApp, model: &str, loras: &[photocraft_doc::GenerationLora]) {
    let list: Vec<PanelLora> = loras.iter().take(MAX_LORAS).map(|l| PanelLora { file: l.file.clone(), strength: l.strength, on: true }).collect();
    // None: the model's entry goes, rather than staying as an empty one.
    if list.is_empty() {
        app.ui.ai_gen.loras.remove(model);
    } else {
        app.ui.ai_gen.loras.insert(model.to_string(), list);
    }
}

/// What the panel calls an added LoRA: its name in the model's settings, else one made from
/// its file.
fn name_of(model: &Model, file: &str) -> String {
    model.lora(file).map_or_else(|| photocraft_aigen::model::label_of("", file), Lora::label)
}

/// Advanced › LoRAs: the heading, whose + adds one of the model's LoRAs, then a row for each
/// added: the eye, the name, the strength (slider and field) and the bin.
pub(super) fn section(app: &mut PhotocraftApp, ui: &mut egui::Ui, current: &Model, enabled: bool) {
    if !current.takes_loras() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let mut list = added(app, current).to_vec();
    list.truncate(MAX_LORAS);
    let before = list.clone();
    let mut settings = false;
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            edit_label(ui, tr("LoRAs"));
            let count = if list.is_empty() { tr("None").to_string() } else { format!("{} / {MAX_LORAS}", list.len()) };
            ui.label(RichText::new(count).color(t.text_faint).size(super::NOTE_SIZE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let full = list.len() >= MAX_LORAS;
                let tip = if full { tr("Five LoRAs at most: remove one to add another") } else { tr("Add a LoRA: one of those this model's settings offer") };
                let plus = ui.add_enabled_ui(!full, |ui| crate::icons::button(ui, "plus", 24.0, false, tip)).inner;
                egui::Popup::menu(&plus).show(|ui| {
                    widgets::menu_scroll(ui, |ui| {
                        if current.loras.is_empty() {
                            ui.label(RichText::new(tr("This model has no LoRAs set up yet.")).color(t.text_faint).size(11.0));
                        }
                        for lora in &current.loras {
                            let file = lora.file.trim();
                            if file.is_empty() {
                                continue;
                            }
                            let there = list.iter().any(|l| l.file.trim() == file);
                            let item = ui.add_enabled(!there, egui::Button::new(lora.label())).on_hover_text(file);
                            if item.clicked() {
                                list.push(PanelLora { file: file.to_string(), strength: lora.strength, on: true });
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button(tr("Manage LoRAs…")).clicked() {
                            settings = true;
                            ui.close();
                        }
                    });
                });
            });
        })
        .response
        .on_hover_text(tr(
            "LoRAs steer the model to a style, a character or a subject. Each is added with a strength: 1 as it was trained, less for a hint of it, 0 for none. A LoRA is made for one model: set them up in the model's settings",
        ));
    });
    ui.add_space(6.0);
    let mut removed = None;
    for (i, lora) in list.iter_mut().enumerate() {
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (icon, tip) = if lora.on { ("eye", "Leave this LoRA out, keeping it in the list") } else { ("eye-off", "Use this LoRA") };
                if crate::icons::button(ui, icon, 22.0, false, tr(tip)).clicked() {
                    lora.on = !lora.on;
                }
                let name = name_of(current, &lora.file);
                let color = if lora.on { t.text } else { t.text_faint };
                let name_width = (ui.available_width() * 0.36).clamp(48.0, 120.0);
                ui.allocate_ui_with_layout(vec2(name_width, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(name_width);
                    ui.add(egui::Label::new(RichText::new(&name).color(color)).truncate()).on_hover_text(lora.file.as_str());
                });
                let field = 44.0;
                let slider = (ui.available_width() - field - 22.0 - 2.0 * ui.spacing().item_spacing.x).max(30.0);
                let mut v = lora.strength as f32;
                ui.allocate_ui_with_layout(vec2(slider, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    widgets::slider(ui, &mut v, SLIDER, None).on_hover_text(tr("Strength: 1 as the LoRA was trained, 0 not at all"));
                });
                let range = *LORA_STRENGTHS.start() as f32..=*LORA_STRENGTHS.end() as f32;
                widgets::value_field(ui, &mut v, range, "", field)
                    .on_hover_text(tr("Strength. Type a value from −4 to 4; below 0 steers away from what the LoRA learnt"));
                if v.is_finite() && (f64::from(v) - lora.strength).abs() > 1e-4 {
                    lora.strength = (f64::from(v) * 100.0).round() / 100.0;
                }
                if crate::icons::button(ui, "trash", 22.0, false, tr("Remove this LoRA")).clicked() {
                    removed = Some(i);
                }
            });
        });
        ui.add_space(6.0);
    }
    if let Some(i) = removed {
        list.remove(i);
    }
    if list != before {
        app.ui.ai_gen.loras.insert(current.id.clone(), list);
    }
    if settings {
        open_settings(app, &current.id);
    }
}

/// The model settings' LoRAs, in its grid: for each, the file (the server's `models/loras`, once
/// Scan Server has read them), up / down / remove, its name, the strength it starts at and its
/// trigger words; then Add LoRA.
pub(super) fn library(ui: &mut egui::Ui, m: &mut Map<String, Value>, files: Option<&Vec<Value>>) {
    let t = Tokens::get(ui.ctx());
    let mut list: Vec<Lora> = m.get("loras").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let server: Vec<String> = files.map(|f| f.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let width = field_width(ui);
    let id = text_of(m, "id");
    let (mut moved, mut removed) = (None, None);
    let count = list.len();
    for (i, lora) in list.iter_mut().enumerate() {
        let title = if lora.file.trim().is_empty() && lora.name.trim().is_empty() { format!("{} {}", tr("LoRA"), i + 1) } else { lora.label() };
        ui.add(egui::Label::new(RichText::new(title).color(t.text_dim)).truncate());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let buttons = 3.0 * 26.0;
            let file_width = (width - buttons).max(80.0);
            if server.is_empty() {
                ui.add(egui::TextEdit::singleline(&mut lora.file).desired_width(file_width).hint_text(tr("File in models/loras")));
            } else {
                let mut names = server.clone();
                if !lora.file.is_empty() && !names.contains(&lora.file) {
                    names.insert(0, lora.file.clone());
                }
                let none = tr("(choose a file)").to_string();
                let mut options: Vec<(String, &str)> = Vec::with_capacity(names.len() + 1);
                if lora.file.is_empty() {
                    options.push((String::new(), none.as_str()));
                }
                options.extend(names.iter().map(|n| (n.clone(), n.as_str())));
                widgets::dropdown_literal(ui, &format!("ai-lora-{id}-{i}"), tr("LoRA file"), &mut lora.file, &options, file_width);
            }
            ui.add_space(4.0);
            if ui.add_enabled_ui(i > 0, |ui| crate::icons::button(ui, "chevron-up", 24.0, false, tr("Move up"))).inner.clicked() {
                moved = Some((i, i - 1));
            }
            if ui.add_enabled_ui(i + 1 < count, |ui| crate::icons::button(ui, "chevron-down", 24.0, false, tr("Move down"))).inner.clicked() {
                moved = Some((i, i + 1));
            }
            if crate::icons::button(ui, "trash", 24.0, false, tr("Remove this LoRA from the model")).clicked() {
                removed = Some(i);
            }
        });
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let derived = photocraft_aigen::model::label_of("", &lora.file);
            let name_width = (width - 6.0 - 60.0 - 50.0).max(80.0);
            ui.add(egui::TextEdit::singleline(&mut lora.name).desired_width(name_width).hint_text(derived))
                .on_hover_text(tr("The name the AI Gen panel shows. Empty: one made from the file"));
            ui.label(RichText::new(tr("Strength")).color(t.text_dim));
            let mut s = lora.strength;
            let range = LORA_STRENGTHS;
            ui.add(egui::DragValue::new(&mut s).range(range).speed(0.05).max_decimals(2).custom_parser(widgets::parse_num))
                .on_hover_text(tr("The strength it starts at when added in the AI Gen panel: 1 as it was trained"));
            if s.is_finite() {
                lora.strength = s;
            }
        });
        ui.end_row();
        ui.label("");
        ui.add(egui::TextEdit::singleline(&mut lora.trigger).desired_width(width).hint_text(tr("Trigger words, added to the prompt while it is used")))
            .on_hover_text(tr("Many LoRAs only show their effect when the prompt has the words they were trained with (their page says which). These are added to the prompt the model is given, unless the prompt has them already"));
        ui.end_row();
    }
    if let Some((from, to)) = moved {
        list.swap(from, to);
    }
    if let Some(i) = removed {
        list.remove(i);
    }
    ui.label("");
    ui.horizontal(|ui| {
        if widgets::secondary_button(ui, &format!("+ {}", tr("Add LoRA")), 110.0)
            .on_hover_text(tr(
                "Offer a LoRA in the AI Gen panel for this model (Advanced › LoRAs). A LoRA is made for one model: one made for another does nothing",
            ))
            .clicked()
        {
            // The first of the server's that the model doesn't offer yet.
            let file = server.iter().find(|f| !list.iter().any(|l| &l.file == *f)).cloned().unwrap_or_default();
            list.push(Lora { file, ..Lora::default() });
        }
        if server.is_empty() {
            ui.label(RichText::new(tr("Scan Server lists the files in ComfyUI's models/loras")).color(t.text_faint).size(11.0));
        }
    });
    ui.end_row();
    m.insert("loras".into(), json!(list));
}
