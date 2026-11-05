//! Preferences › Generative AI › Model Folders: every models folder local models are read from,
//! each as a row saying whose it is. The setup's own (PhotoCraft downloads into it and deletes
//! from it) comes first; then the folders the user added, which the engine PhotoCraft installed
//! only reads. Each row opens to list the model files it has, by kind, and has Refresh, Show in
//! the file manager and (for an added folder) Remove. With the user's own ComfyUI there is only
//! that ComfyUI's folder: its own settings name any others.
//!
//! What is listed comes from `ai.setup.modelFolders {list: true}`; adding or removing a folder is
//! `ai.setup.modelFolders {folders}`, after which the page scans as Scan Server does.

use egui::{RichText, vec2};
use serde_json::{Map, Value, json};

use super::{ACTION, FILES, SELECTED, SETUP, STATUS, Standing, failed, model_of, status_line, tr};
use crate::PhotocraftApp;
use crate::theme::Tokens;
use crate::widgets;

/// Dialog fields: what `ai.setup.modelFolders` listed (`[{path, kind, found, files}]`), and the
/// folders opened to show their files (by path).
const LISTING: &str = "__aiFolderListing";
const OPENED: &str = "__aiFolderOpened";
/// The folder whose Remove is waiting for the user to confirm it.
const CONFIRM: &str = "__aiFolderConfirm";

/// What the rows ask [`act`] for.
const LIST: &str = "folderList";
pub(super) const ADD: &str = "ownFolderAdd";
pub(super) const REMOVE: &str = "ownFolderRemove:";
const SHOW: &str = "folderShow:";

/// The kinds of model file a folder holds, as ComfyUI names their folders, in the order a row
/// lists them, with what to call them (one, many, and as the heading of an opened row's list).
const KINDS: [(&str, &str, &str, &str); 6] = [
    ("diffusion_models", "diffusion model", "diffusion models", "Diffusion models"),
    ("checkpoints", "checkpoint", "checkpoints", "Checkpoints"),
    ("loras", "LoRA", "LoRAs", "LoRAs"),
    ("text_encoders", "text encoder", "text encoders", "Text encoders"),
    ("vae", "VAE", "VAEs", "VAEs"),
    ("controlnet", "ControlNet", "ControlNets", "ControlNets"),
];

/// The most files a row lists of one kind; the rest are counted.
const MAX_SHOWN: usize = 400;

/// "37 diffusion models · 33 LoRAs · …": what `files` has, the kinds with none left out.
fn counts(files: &Map<String, Value>) -> String {
    let parts: Vec<String> = KINDS
        .iter()
        .filter_map(|(key, one, many, _)| {
            let n = files.get(*key).and_then(Value::as_array).map_or(0, Vec::len);
            (n > 0).then(|| format!("{n} {}", tr(if n == 1 { one } else { many })))
        })
        .collect();
    if parts.is_empty() { tr("No model files").to_string() } else { parts.join(" · ") }
}

/// A small rounded label beside a folder's (or the engine's) name saying whose it is, on a muted
/// fill of `tone`, in a lighter `tone`: the accent for PhotoCraft's own, amber for the user's.
pub(super) fn badge(ui: &mut egui::Ui, text: &str, tone: egui::Color32) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::new()
        .fill(tone.gamma_multiply(0.22))
        .corner_radius(t.radius_sm)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| ui.label(RichText::new(text).color(tone.lerp_to_gamma(egui::Color32::WHITE, 0.35)).size(10.5)));
}

/// The page. `setup` is what [`crate::ai_setup_ui::summary`] says; `busy` while a Connect or
/// Scan runs.
pub(super) fn page(ui: &mut egui::Ui, f: &mut Map<String, Value>, busy: bool) {
    let t = Tokens::get(ui.ctx());
    let setup = f.get(SETUP).cloned().unwrap_or(Value::Null);
    // As wide as the dialog lets it be: it follows the dialog when that is resized.
    let width = ui.available_width();
    let route = setup.get("route").and_then(Value::as_str).unwrap_or("");
    let managed = setup.get("managed").and_then(Value::as_bool) == Some(true);
    ui.scope(|ui| {
        ui.set_max_width(width);
        let intro = if route == "existing" {
            tr("Your own ComfyUI reads the models folders its own settings name (its extra_model_paths.yaml). The setup put the models it downloaded in the folder below.")
        } else {
            tr("Local models and LoRAs are read from these folders. PhotoCraft downloads into its own folder; folders you add are only read: nothing in them is changed or deleted.")
        };
        ui.add(egui::Label::new(RichText::new(intro).color(t.text_dim)).wrap());
    });
    ui.add_space(10.0);
    if setup.get("available").and_then(Value::as_bool) != Some(true) || route.is_empty() {
        ui.scope(|ui| {
            ui.set_max_width(width);
            ui.add(
                egui::Label::new(RichText::new(tr("Local AI isn't set up yet. The setup installs the engine and the models you choose, and can read models folders you already have.")).color(t.text_faint).size(11.5))
                    .wrap(),
            );
        });
        ui.add_space(6.0);
        if widgets::secondary_button(ui, tr("Set Up Local AI…"), 150.0).clicked() {
            f.insert(ACTION.into(), json!("setup"));
        }
        return;
    }
    let Some(listing) = f.get(LISTING).and_then(Value::as_array).cloned() else {
        // Read when the page is first shown.
        if !f.contains_key(ACTION) {
            f.insert(ACTION.into(), json!(LIST));
        }
        ui.label(RichText::new(tr("Reading the folders…")).color(t.text_faint).size(11.5));
        return;
    };
    let mut opened: Vec<String> =
        f.get(OPENED).and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let mut yours = 0usize;
    for entry in &listing {
        let path = entry.get("path").and_then(Value::as_str).unwrap_or("");
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        yours += usize::from(kind == "yours");
        let open = opened.iter().any(|p| p == path);
        if let Some(toggled) = row(ui, f, entry, kind == "yours", open, width, busy) {
            if toggled {
                opened.retain(|p| p != path);
            } else {
                opened.push(path.to_string());
            }
        }
        ui.add_space(6.0);
    }
    f.insert(OPENED.into(), json!(opened));
    confirm_remove(ui.ctx(), f);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if managed {
            let full = yours >= photocraft_aisetup::plan::MAX_EXTRA_MODELS_DIRS;
            let add = ui.add_enabled_ui(!busy && !full, |ui| widgets::secondary_button(ui, tr("Add Folder…"), 120.0)).inner;
            if add
                .on_hover_text(tr("A models folder you already have, such as another ComfyUI's: its models and LoRAs are listed beside PhotoCraft's own"))
                .on_disabled_hover_text(tr("Sixteen folders at most"))
                .clicked()
            {
                f.insert(ACTION.into(), json!(ADD));
            }
        }
        if widgets::secondary_button(ui, tr("Download Models…"), 150.0)
            .on_hover_text(tr("Opens the setup wizard to download more of the recommended models. These preferences are saved first"))
            .clicked()
        {
            f.insert(ACTION.into(), json!("setup"));
        }
    });
    if f.get(STATUS).is_some() {
        ui.add_space(4.0);
        status_line(ui, f, "", width);
    }
}

/// One folder: its icon, whose it is, its path and what it has, with Refresh, Open and (for one
/// the user added, `yours`) Remove; opened, its files. Returns
/// `Some(was_open)` when its header was clicked.
fn row(ui: &mut egui::Ui, f: &mut Map<String, Value>, entry: &Value, yours: bool, open: bool, width: f32, busy: bool) -> Option<bool> {
    let t = Tokens::get(ui.ctx());
    let path = entry.get("path").and_then(Value::as_str).unwrap_or("");
    let found = entry.get("found").and_then(Value::as_bool).unwrap_or(false);
    let files = entry.get("files").and_then(Value::as_object).cloned().unwrap_or_default();
    let (title, whose, whose_color, tip) = match entry.get("kind").and_then(Value::as_str) {
        Some("setup") => (
            tr("PhotoCraft's models"),
            tr("Internal"),
            t.accent,
            tr("The folder the setup downloads models into. Removing models is done in the setup wizard"),
        ),
        Some("comfy") => (tr("Your ComfyUI's models"), tr("External"), t.warning, tr("Where your own ComfyUI keeps its models; the setup downloaded into it")),
        _ => (tr("Your models folder"), tr("External (read only)"), t.warning, tr("Read by PhotoCraft's engine; nothing in it is changed or deleted")),
    };
    let mut clicked = None;
    egui::Frame::new().fill(t.card).stroke(egui::Stroke::new(1.0, t.card_border)).corner_radius(t.radius).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(width - 22.0);
        // The buttons sit on the path's line, centred on its field. The rest of the header (icon,
        // name, path, counts) opens and closes the list, without lying over them.
        let mut header = egui::Rect::NOTHING;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            header = header.union(ui.add(crate::icons::image("folder", 22.0, if found { t.icon } else { t.text_faint })).rect);
            ui.vertical(|ui| {
                let name = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(title).font(crate::theme::medium(13.0)).color(t.text));
                    badge(ui, whose, whose_color);
                });
                header = header.union(name.response.rect);
                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    // Room for all four buttons on every row, so the paths line up whether a row
                    // has a bin or not.
                    let buttons = 4.0 * 26.0;
                    let field_width = (ui.available_width() - buttons - 8.0).max(120.0);
                    let field = ui.allocate_ui_with_layout(vec2(field_width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| path_field(ui, path)).response;
                    header = header.union(field.rect);
                    let room = vec2(ui.available_width(), field.rect.height());
                    ui.allocate_ui_with_layout(room, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let chevron = crate::icons::button(ui, if open { "chevron-down" } else { "chevron-right" }, 24.0, false, if open { tr("Hide its files") } else { tr("List its files") });
                        if chevron.clicked() {
                            clicked = Some(open);
                        }
                        if yours {
                            let remove = ui.add_enabled_ui(!busy, |ui| crate::icons::button(ui, "trash", 24.0, false, tr("Stop reading this folder (nothing in it is deleted)"))).inner;
                            if remove.clicked() {
                                f.insert(CONFIRM.into(), json!(path));
                            }
                        }
                        let show = ui.add_enabled_ui(found, |ui| crate::icons::button(ui, "folder-open", 24.0, false, tr(open_label()))).inner;
                        if show.clicked() {
                            f.insert(ACTION.into(), json!(format!("{SHOW}{path}")));
                        }
                        if crate::icons::button(ui, "rotate-cw", 24.0, false, tr("Read the folder again")).clicked() {
                            f.insert(ACTION.into(), json!(LIST));
                        }
                    });
                });
                ui.add_space(2.0);
                let (what, color) = if found { (counts(&files), t.text_faint) } else { (tr("Not found: is its drive plugged in?").to_string(), t.warning) };
                header = header.union(ui.label(RichText::new(what).color(color).size(11.0)).rect);
            });
        });
        let header = ui.interact(header, ui.scope_id().with(("folder-row", path)), egui::Sense::click()).on_hover_text(format!("{path}
{tip}"));
        if header.clicked() {
            clicked = Some(open);
        }
        if open && found {
            ui.add_space(6.0);
            files_list(ui, path, &files);
        }
    });
    clicked
}

/// Remove's question: what stops working when the folder is no longer read, and that nothing in
/// it is deleted. Remove asks [`act`] to do it; Cancel, Escape or a click outside drop it.
fn confirm_remove(ctx: &egui::Context, f: &mut Map<String, Value>) {
    use widgets::{ButtonRole, DialogButton};
    let Some(path) = f.get(CONFIRM).and_then(Value::as_str).map(str::to_string) else { return };
    let mut answer = None;
    let buttons = [DialogButton::new(ButtonRole::Default, tr("Remove"), 100.0), DialogButton::new(ButtonRole::Cancel, tr("Cancel"), 84.0)];
    let modal = egui::Modal::new(egui::Id::unique("ai-folder-remove")).show(ctx, |ui| {
        let t = Tokens::get(ui.ctx());
        ui.set_max_width(460.0);
        ui.label(RichText::new(tr("Remove this models folder?")).font(crate::theme::semibold(15.0)));
        ui.add_space(4.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(&path).font(crate::theme::mono(12.0)).color(t.text)).wrap());
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(tr(
                "PhotoCraft will stop reading this folder. The models and LoRAs that are only in it are taken out of Manage Models and the AI Gen panel (a recommended model stays, switched off, so it can be installed). Adding the folder again and pressing Scan Server brings them back.",
            ))
            .wrap(),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(tr("No files are deleted: the folder and everything in it stay as they are.")).color(t.text_dim)).wrap());
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            answer = widgets::dialog_buttons(ui, &buttons);
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(ButtonRole::Cancel));
    }
    match answer {
        Some(ButtonRole::Default) => {
            f.remove(CONFIRM);
            f.insert(ACTION.into(), json!(format!("{REMOVE}{path}")));
        }
        Some(_) => {
            f.remove(CONFIRM);
        }
        None => {}
    }
}

/// A folder's path, left-aligned in a field of its own (cut short with "…" when it doesn't fit).
fn path_field(ui: &mut egui::Ui, path: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::new().fill(t.field).stroke(egui::Stroke::new(1.0, t.field_border)).corner_radius(t.radius_sm).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
        ui.set_width((ui.available_width() - 12.0).max(60.0));
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(path).font(crate::theme::mono(11.5)).color(t.text_dim)).truncate().selectable(false));
        });
    });
}

/// What opening a folder in the file manager is called on this platform.
fn open_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Open in Finder"
    } else if cfg!(windows) {
        "Open in Explorer"
    } else {
        "Open Folder"
    }
}

/// An opened folder's files, by kind, in a box of their own that scrolls.
fn files_list(ui: &mut egui::Ui, path: &str, files: &Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    widgets::hairline(ui);
    ui.add_space(4.0);
    if files.values().filter_map(Value::as_array).all(Vec::is_empty) {
        ui.label(RichText::new(tr("No model files in its model folders (checkpoints, diffusion_models, loras, …).")).color(t.text_faint).size(11.0));
        return;
    }
    egui::ScrollArea::vertical().id_salt(("folder-files", path)).max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        for (key, _, _, heading) in KINDS {
            let Some(names) = files.get(key).and_then(Value::as_array).filter(|l| !l.is_empty()) else { continue };
            ui.label(RichText::new(format!("{} ({})", tr(heading), names.len())).font(crate::theme::medium(11.5)).color(t.text_dim));
            for name in names.iter().filter_map(Value::as_str).take(MAX_SHOWN) {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.add(egui::Label::new(RichText::new(name).font(crate::theme::mono(11.5)).color(t.text)).truncate());
                });
            }
            if names.len() > MAX_SHOWN {
                ui.label(RichText::new(format!("… {} {}", names.len() - MAX_SHOWN, tr("more"))).color(t.text_faint).size(11.0));
            }
            ui.add_space(4.0);
        }
    });
}

/// Read every folder's files again (`ai.setup.modelFolders {list: true}`).
pub(super) fn list(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    match app.session.execute("ai.setup.modelFolders", json!({"list": true})) {
        Ok(r) => {
            f.insert(LISTING.into(), r.get("listing").cloned().unwrap_or_else(|| json!([])));
        }
        Err(e) => {
            f.insert(LISTING.into(), json!([]));
            failed(f, &e.to_string());
        }
    }
}

/// A models folder was added (`folders` is the whole list now): saved, listed again, and the
/// models in it added to the draft as Scan Server would, from the files the folders have. The
/// engine isn't started for it: it reads the new folder the next time a local model needs it.
pub(crate) fn added(app: &mut PhotocraftApp, f: &mut Map<String, Value>, folders: &[String]) {
    match crate::ai_setup_ui::set_own_folders(app, folders) {
        Ok(_) => {
            f.insert(SETUP.into(), crate::ai_setup_ui::summary(app));
            list(app, f);
            let files = json!(files_of(f));
            let (models, repaired, matched) = super::add_detected(f, &files);
            f.insert(FILES.into(), files);
            let mut text = format!("{} {models} {}", tr("Folder added:"), tr(if models == 1 { "model added" } else { "models added" }));
            if repaired + matched > 0 {
                text = format!("{text}, {} {}", repaired + matched, tr("pointed at its files"));
            }
            f.insert(STATUS.into(), json!({"error": false, "text": text}));
        }
        Err(e) => failed(f, &e),
    }
}

/// Read the folders for the page, once a dialog: Manage Models marks each model with the folder
/// it is in (internal or external), so it needs them as much as this page does.
pub(super) fn ensure_listed(app: &mut PhotocraftApp, f: &mut Map<String, Value>) {
    let set_up = f.get(SETUP).and_then(|s| s.get("route")).and_then(Value::as_str).is_some_and(|r| !r.is_empty());
    if set_up && !f.contains_key(LISTING) {
        list(app, f);
    }
}

/// The model files of PhotoCraft's own folder (`internal`) and of the user's (`external`: those
/// added, and the user's own ComfyUI's), each by kind, from the folders the page listed.
pub(super) fn files_by_origin(f: &Map<String, Value>) -> (std::collections::BTreeMap<String, Vec<String>>, std::collections::BTreeMap<String, Vec<String>>) {
    let (mut internal, mut external): (std::collections::BTreeMap<String, Vec<String>>, std::collections::BTreeMap<String, Vec<String>>) = Default::default();
    for entry in f.get(LISTING).and_then(Value::as_array).into_iter().flatten() {
        let to = if entry.get("kind").and_then(Value::as_str) == Some("setup") { &mut internal } else { &mut external };
        for (kind, names) in entry.get("files").and_then(Value::as_object).into_iter().flatten() {
            to.entry(kind.clone()).or_default().extend(names.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        }
    }
    (internal, external)
}

/// The model files of every folder the page lists, by kind, merged.
pub(super) fn files_of(f: &Map<String, Value>) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for entry in f.get(LISTING).and_then(Value::as_array).into_iter().flatten() {
        for (kind, names) in entry.get("files").and_then(Value::as_object).into_iter().flatten() {
            let have = out.entry(kind.clone()).or_default();
            for name in names.as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !have.iter().any(|h| h == name) {
                    have.push(name.to_string());
                }
            }
        }
    }
    out
}

/// A models folder went: take out of the draft what could run (`had`) only thanks to it and can't
/// now (`left`). A model Scan Server added for one of its files goes; a recommended one (one of
/// [`photocraft_aigen::Model::defaults`], or the wizard's, by id) stays, unticked, to show as not
/// installed. A LoRA whose file was only
/// there goes from its model's settings and from the AI Gen panel. Returns how many models and
/// LoRAs went.
fn forget(
    app: &mut PhotocraftApp,
    f: &mut Map<String, Value>,
    had: &std::collections::BTreeMap<String, Vec<String>>,
    left: &std::collections::BTreeMap<String, Vec<String>>,
) -> (usize, usize) {
    use photocraft_aigen::detect::on_server;
    let Some(mut ai) = f.get("values").and_then(|v| v.pointer("/integrations/ai")).and_then(Value::as_object).cloned() else { return (0, 0) };
    let lost_lora = |file: &str| {
        let file = file.trim();
        let in_ = |m: &std::collections::BTreeMap<String, Vec<String>>| m.get("loras").is_some_and(|l| l.iter().any(|x| x == file));
        !file.is_empty() && in_(had) && !in_(left)
    };
    let standing = Standing::of(&ai, f);
    let (mut models, mut loras) = (0, 0);
    let mut kept = Vec::new();
    for v in ai.get("models").and_then(Value::as_array).cloned().unwrap_or_default() {
        let Some(mut m) = model_of(&v) else {
            kept.push(v);
            continue;
        };
        let lost = m.provider.is_local() && on_server(had, &m) && !on_server(left, &m);
        // The recommended entry itself (by id: the recommended and the wizard's models keep their
        // own), not one Scan Server made for a file of the same model (`krea2_turbo_fp8_scaled`
        // beside Krea 2 Turbo, or even its very file).
        let recommended = photocraft_aigen::Model::defaults().iter().any(|d| d.id == m.id) || standing.catalog.iter().any(|w| w.get("id").and_then(Value::as_str) == Some(m.id.as_str()));
        if lost && !recommended {
            models += 1;
            continue;
        }
        if lost {
            m.enabled = false;
        }
        let before = m.loras.len();
        m.loras.retain(|l| !lost_lora(&l.file));
        loras += before - m.loras.len();
        kept.push(serde_json::to_value(&m).unwrap_or(v));
    }
    ai.insert("models".into(), Value::Array(kept));
    if let Some(slot) = f.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai")) {
        *slot = Value::Object(ai);
    }
    if models > 0 {
        f.remove(SELECTED);
    }
    // The panel's LoRAs: those added for a model, from the same files.
    for list in app.ui.ai_gen.loras.values_mut() {
        list.retain(|l| !lost_lora(&l.file));
    }
    app.ui.ai_gen.loras.retain(|_, list| !list.is_empty());
    (models, loras)
}

/// The user's own models folders as saved in the setup's record: what Add and Remove change.
/// (The wizard's own copy is only read when the wizard opens, so after a restart it is empty.)
fn saved(app: &mut PhotocraftApp) -> Vec<String> {
    match app.session.execute("ai.setup.modelFolders", json!({})) {
        Ok(r) => r.get("folders").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        Err(_) => app.ui.ai_setup.extra_models_dirs.clone(),
    }
}

/// Do what a row asked for; `false` when `action` isn't one of this page's.
pub(super) fn act(app: &mut PhotocraftApp, f: &mut Map<String, Value>, action: &str) -> bool {
    match action {
        LIST => list(app, f),
        ADD => {
            let asked = app.ask_file(crate::file_dialog::FileDialogRequest::Folder { initial_dir: None }, |app, answer| {
                let crate::file_dialog::FileDialogAnswer::FolderTo(path) = answer else { return Ok(Value::Null) };
                let mut folders = saved(app);
                let picked = crate::ai_setup_ui::own_folder(&path);
                let Some(d) = app.ui.dialogs.iter().position(|d| crate::prefs_ui::is_preferences(&d.fields)) else { return Ok(Value::Null) };
                let mut fields = std::mem::take(&mut app.ui.dialogs[d].fields);
                match picked {
                    // Picked again: nothing changes.
                    Ok(dir) if folders.contains(&dir) => {}
                    Ok(dir) => {
                        folders.push(dir);
                        added(app, &mut fields, &folders);
                    }
                    Err(why) => failed(&mut fields, &why),
                }
                if let Some(dialog) = app.ui.dialogs.get_mut(d) {
                    dialog.fields = fields;
                }
                Ok(Value::Null)
            });
            if let Err(e) = asked {
                failed(f, &format!("{}: {e}", tr("Could not open the folder dialog")));
            }
        }
        _ if action.starts_with(REMOVE) => {
            let gone = action.trim_start_matches(REMOVE);
            let mut folders = saved(app);
            let before = folders.len();
            folders.retain(|d| d != gone);
            if folders.len() < before {
                if let Some(opened) = f.get_mut(OPENED).and_then(Value::as_array_mut) {
                    opened.retain(|p| p.as_str() != Some(gone));
                }
                let had = files_of(f);
                match crate::ai_setup_ui::set_own_folders(app, &folders) {
                    Ok(_) => {
                        f.insert(SETUP.into(), crate::ai_setup_ui::summary(app));
                        list(app, f);
                        let left = files_of(f);
                        let (models, loras) = forget(app, f, &had, &left);
                        // What the engine has now is what Manage Models and the LoRA lists offer
                        // (no scan: a folder going adds nothing, and starting the engine is slow).
                        f.insert(FILES.into(), json!(left));
                        let text = if models + loras == 0 {
                            tr("Folder removed. Nothing listed came from it alone.").to_string()
                        } else {
                            let n = |n: usize, one: &str, many: &str| format!("{n} {}", tr(if n == 1 { one } else { many }));
                            format!("{} {}, {}", tr("Folder removed, with what was only in it:"), n(models, "model", "models"), n(loras, "LoRA", "LoRAs"))
                        };
                        f.insert(STATUS.into(), json!({"error": false, "text": text}));
                    }
                    Err(e) => failed(f, &e),
                }
            }
        }
        _ if action.starts_with(SHOW) => {
            if let Err(e) = app.session.execute("ai.setup.openModelFolder", json!({"folder": action.trim_start_matches(SHOW)})) {
                failed(f, &e.to_string());
            }
        }
        _ => return false,
    }
    true
}
