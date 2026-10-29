//! `photocraft.exe --uninstall-cleanup`: what the Windows installer runs when LayerShop is
//! uninstalled (never on an upgrade), before its own files go.
//!
//! The app's data outlives its program files unless it is removed here:
//! - **the local AI engine**: ComfyUI with its own Python (`python_embeded`; LayerShop never
//!   installs a Python of the system's), several GB;
//! - **the AI models** the Local AI setup downloaded, often tens of GB;
//!   both in `%LOCALAPPDATA%\LayerShop` unless the user chose other folders (the setup's record,
//!   `ai-setup.json`, says which);
//! - **the settings**: preferences, presets, API keys and the Hugging Face sign-in, recovery
//!   files, in `%APPDATA%\LayerShop`.
//!
//! A small window asks, one box each, with its size (engine and models selected, settings not),
//! and removes what is selected. Model files in a folder the user chose go only where the setup put
//! them (the same code as Remove Local AI in the app), and models downloaded into the user's own
//! ComfyUI are never touched. With the settings kept, they are told the engine is gone.
//!
//! The installer passes the user's folders (`--local "[LocalAppDataFolder]."`,
//! `--roaming "[AppDataFolder]."`): it runs this as the user, but its environment need not be the
//! user's. `--remove none|ai|all` answers without asking (a silent uninstall); a process with no
//! desktop to show the window on never asks, and keeps everything.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use photocraft_aisetup::Manifest;
use photocraft_aisetup::manifest::split_key;
use photocraft_aisetup::plan::Route;
use photocraft_aisetup::state::{self, SetupState};
use serde_json::json;

/// What was asked for on the command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub local: Option<PathBuf>,
    pub roaming: Option<PathBuf>,
    /// Answered without asking: `none`, `ai` (engine and models) or `all`.
    pub remove: Option<String>,
}

pub fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Args {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--local") => out.local = args.next().map(|p| tidy(PathBuf::from(p))).filter(|p| !p.as_os_str().is_empty()),
            Some("--roaming") => out.roaming = args.next().map(|p| tidy(PathBuf::from(p))).filter(|p| !p.as_os_str().is_empty()),
            Some("--remove") => out.remove = args.next().map(|v| v.to_string_lossy().trim().to_ascii_lowercase()),
            _ => {}
        }
    }
    out
}

/// A folder (or the setup's files in one) that can be removed, and how much room it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub bytes: u64,
}

/// What LayerShop left on this PC.
#[derive(Clone, Debug, Default)]
pub struct Survey {
    /// The settings folder, when there is one.
    pub settings: Option<Item>,
    /// `%LOCALAPPDATA%\LayerShop`, the default home of the engine and the models.
    pub home: Option<PathBuf>,
    /// The engine: its folder in the home, and the setup's engine folder elsewhere.
    pub engine: Vec<Item>,
    /// The models: their folder in the home, and the setup's files in a models folder elsewhere.
    pub models: Vec<Item>,
    /// The setup's record (empty without one).
    pub record: SetupState,
    /// The setup downloaded models into the user's own ComfyUI: they stay.
    pub own_comfyui: Option<PathBuf>,
}

impl Survey {
    pub fn engine_bytes(&self) -> u64 {
        self.engine.iter().map(|i| i.bytes).sum()
    }

    pub fn models_bytes(&self) -> u64 {
        self.models.iter().map(|i| i.bytes).sum()
    }

    /// Anything to ask about?
    pub fn anything(&self) -> bool {
        self.settings.is_some() || !self.engine.is_empty() || !self.models.is_empty()
    }
}

/// The bytes under `path` (files only, links not followed, at most 64 levels and a million
/// entries: an estimate for a question, not an audit).
pub fn size_of(path: &Path) -> u64 {
    fn walk(path: &Path, depth: u32, seen: &mut u32) -> u64 {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
        if meta.is_file() {
            return meta.len();
        }
        if !meta.is_dir() || depth > 64 || *seen > 1_000_000 {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(path) else { return 0 };
        let mut total = 0u64;
        for entry in entries.flatten() {
            *seen += 1;
            total = total.saturating_add(walk(&entry.path(), depth + 1, seen));
        }
        total
    }
    walk(path, 0, &mut 0)
}

/// `path` without its `.` parts (the installer writes `C:\…\Local\.`).
fn tidy(path: PathBuf) -> PathBuf {
    path.components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect()
}

/// Is `path` `folder` or in it, however each is written?
fn inside(path: &Path, folder: &Path) -> bool {
    let key = |p: &Path| tidy(p.to_path_buf()).to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (p, f) = (key(path), key(folder));
    p == f || p.starts_with(&format!("{f}\\"))
}

/// Look at what is there: `local` and `roaming` are the user's `%LOCALAPPDATA%` and `%APPDATA%`;
/// `name` the product's folder name in each (`LayerShop`).
pub fn survey(local: Option<&Path>, roaming: Option<&Path>, name: &str) -> Survey {
    let mut out = Survey::default();
    let settings = roaming.map(|r| r.join(name)).filter(|d| d.is_dir());
    out.record = settings.as_ref().map(|d| state::load(&d.join("ai-setup.json"))).unwrap_or_default();
    out.settings = settings.map(|path| Item { bytes: size_of(&path), path });
    let home = local.map(|l| l.join(name)).filter(|h| h.is_dir());
    if let Some(home) = &home {
        for (sub, into) in [("engine", &mut out.engine), ("models", &mut out.models)] {
            let path = home.join(sub);
            if path.is_dir() {
                into.push(Item { bytes: size_of(&path), path });
            }
        }
    }
    let choices = &out.record.choices;
    let elsewhere =
        |dir: &str| Some(PathBuf::from(dir.trim())).filter(|d| !d.as_os_str().is_empty() && d.is_dir() && home.as_ref().is_none_or(|h| !inside(d, h)));
    match choices.route {
        Route::Managed => {
            if let Some(engine) = elsewhere(&choices.engine_dir) {
                let bytes = photocraft_aisetup::runtime::installed_engines(&engine).iter().map(|(p, _)| size_of(p)).sum();
                if bytes > 0 {
                    out.engine.push(Item { path: engine, bytes });
                }
            }
            if let Some(models) = elsewhere(&choices.models_dir) {
                // Only the files the setup put there count: anything else in the folder stays.
                let bytes = setup_files(&models, &out.record).iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
                if bytes > 0 {
                    out.models.push(Item { path: models, bytes });
                }
            }
        }
        Route::Existing => out.own_comfyui = Some(PathBuf::from(choices.models_dir.trim())).filter(|d| !d.as_os_str().is_empty()),
    }
    out.home = home;
    out
}

/// The model files the setup put in `models` (its installed models' files that are there).
fn setup_files(models: &Path, record: &SetupState) -> Vec<PathBuf> {
    let Ok(manifest) = Manifest::bundled() else { return Vec::new() };
    let installed = manifest.models.iter().filter(|m| record.installed.contains(&m.id));
    manifest.file_keys(installed).into_iter().filter_map(split_key).map(|(folder, name)| models.join(folder).join(name)).filter(|p| p.is_file()).collect()
}

/// What to remove.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub engine: bool,
    pub models: bool,
    pub settings: bool,
}

/// Remove what `choice` selects. Returns what could not be removed (files in use, say), one line
/// each.
pub fn remove(found: &Survey, choice: Choice) -> Vec<String> {
    let mut failed = Vec::new();
    let settings = found.settings.as_ref().map(|s| s.path.clone());
    // A setup in folders the user chose: the app's own removal, which takes only what the setup
    // installed and tells the settings the engine is gone.
    if (choice.engine || choice.models)
        && found.record.choices.route == Route::Managed
        && found.record.phase != state::Phase::None
        && let Some(settings) = &settings
    {
        failed.extend(remove_through_the_app(settings, found, choice));
    }
    let mut delete = |path: &Path| {
        if path.exists()
            && let Err(e) = std::fs::remove_dir_all(path)
        {
            failed.push(format!("{}: {e}", path.display()));
        }
    };
    if let Some(home) = &found.home {
        if choice.engine {
            delete(&home.join("engine"));
        }
        if choice.models {
            delete(&home.join("models"));
        }
        // Nothing of LayerShop's left in it (its downloads, a record): it goes too.
        if choice.engine && choice.models {
            delete(home);
        } else if std::fs::read_dir(home).is_ok_and(|mut d| d.next().is_none()) {
            let _ = std::fs::remove_dir(home);
        }
    }
    if choice.settings
        && let Some(settings) = &settings
    {
        delete(settings);
    }
    failed
}

/// `ai.setup.remove` through a session with the user's settings: the engine and only the model
/// files the setup installed, wherever they are; the settings saved after (unless they are being
/// removed too).
fn remove_through_the_app(settings: &Path, found: &Survey, choice: Choice) -> Vec<String> {
    let mut s = photocraft_engine::Session::new();
    let prefs = settings.join("preferences.json");
    if let Ok(text) = std::fs::read_to_string(&prefs) {
        let _ = s.load_prefs_json(&text);
    }
    s.ai.transport = Some(Arc::new(photocraft_aigen::UreqTransport::new()));
    s.ai_setup.env = Some(photocraft_engine::ai_setup_cmds::SetupEnv {
        fetch: Arc::new(photocraft_aisetup::fetch::UreqFetch::default()),
        state_file: settings.join("ai-setup.json"),
        engine_dir: found.record.choices.engine_dir.clone(),
        models_dir: found.record.choices.models_dir.clone(),
        manifest: None,
        hf_client_id: String::new(),
    });
    let models = if choice.models { json!(true) } else { json!(false) };
    let mut failed = Vec::new();
    match s.execute("ai.setup.remove", json!({"engine": choice.engine, "models": models})) {
        Ok(_) if !choice.settings => {
            if let Err(e) = photocraft_format::atomic_write(&prefs, s.prefs_to_json().as_bytes()) {
                failed.push(format!("{}: {e}", prefs.display()));
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("uninstall: the local AI setup could not be removed through the app: {e}"),
    }
    failed
}

/// Run the cleanup; the process's exit status.
pub fn run(args: Args) -> i32 {
    let env_dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let local = args.local.clone().or_else(|| env_dir("LOCALAPPDATA"));
    let roaming = args.roaming.clone().or_else(|| env_dir("APPDATA"));
    let found = survey(local.as_deref(), roaming.as_deref(), crate::app_dirs::DIR_NAME);
    if !found.anything() {
        return 0;
    }
    let choice = match args.remove.as_deref() {
        Some("all") => Some(Choice { engine: true, models: true, settings: true }),
        Some("ai") => Some(Choice { engine: true, models: true, settings: false }),
        Some(_) => Some(Choice::default()),
        // No desktop to ask on (a service account's): keep everything.
        None if env_dir("USERPROFILE").is_some_and(|p| p.to_string_lossy().to_lowercase().contains("systemprofile")) => Some(Choice::default()),
        None => None,
    };
    if let Some(choice) = choice {
        let failed = remove(&found, choice);
        for line in &failed {
            log::warn!("uninstall: could not remove {line}");
        }
        return 0;
    }
    if ask_in_a_window(found.clone()).is_err() {
        // No window could be opened: the plainest question there is.
        let choice = ask_in_message_boxes(&found);
        let failed = remove(&found, choice);
        if !failed.is_empty() {
            let _ = rfd::MessageDialog::new()
                .set_title(format!("Uninstall {}", photocraft_ui_egui::brand::NAME))
                .set_description(format!("Some files could not be removed (in use?):\n\n{}\n\nYou can delete them yourself.", failed.join("\n")))
                .set_level(rfd::MessageLevel::Warning)
                .show();
        }
    }
    0
}

fn gb(bytes: u64) -> String {
    photocraft_aisetup::human_bytes(bytes)
}

/// Two Yes/No questions, when the window can't be shown.
fn ask_in_message_boxes(found: &Survey) -> Choice {
    let name = photocraft_ui_egui::brand::NAME;
    let yes = |title: String, text: String| {
        rfd::MessageDialog::new().set_title(title).set_description(text).set_buttons(rfd::MessageButtons::YesNo).set_level(rfd::MessageLevel::Info).show()
            == rfd::MessageDialogResult::Yes
    };
    let mut choice = Choice::default();
    let ai = found.engine_bytes() + found.models_bytes();
    if !found.engine.is_empty() || !found.models.is_empty() {
        let places: Vec<String> = found.engine.iter().chain(&found.models).map(|i| i.path.display().to_string()).collect();
        let text = format!(
            "Also remove {name}'s local AI?\n\nThe engine (ComfyUI with its own Python) and the AI models it downloaded take {} in:\n{}\n\nNo keeps them for a later install.",
            gb(ai),
            places.join("\n")
        );
        let all = yes(format!("Uninstall {name}"), text);
        (choice.engine, choice.models) = (all, all);
    }
    if let Some(settings) = &found.settings {
        let text = format!(
            "Also remove your {name} settings?\n\nPreferences, brush presets, API keys and the Hugging Face sign-in, in:\n{}\n\nNo keeps them for a later install.",
            settings.path.display()
        );
        choice.settings = yes(format!("Uninstall {name}"), text);
    }
    choice
}

/// The question as a small window of its own, above the installer's.
fn ask_in_a_window(found: Survey) -> eframe::Result {
    let name = photocraft_ui_egui::brand::NAME;
    // Nothing of the window's own is kept: without a file of its own, eframe would leave a
    // "LayerShop uninstall" folder in %APPDATA% behind the uninstall.
    let scratch = std::env::temp_dir().join(format!("{}-uninstall-{}.ron", crate::app_dirs::DIR_NAME, std::process::id()));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Uninstall {name}"))
            // Fitted to the content on the first frame (`Dialog::content_height`).
            .with_inner_size([WIDTH, 340.0])
            .with_resizable(false)
            .with_minimize_button(false)
            .with_maximize_button(false)
            .with_always_on_top()
            .with_active(true)
            .with_icon(crate::app_icon::window_icon()),
        centered: true,
        persist_window: false,
        persistence_path: Some(scratch.clone()),
        ..Default::default()
    };
    let shown = eframe::run_native(
        &format!("{name} uninstall"),
        options,
        Box::new(move |cc| {
            photocraft_ui_egui::PhotocraftApp::setup_context(&cc.egui_ctx, photocraft_ui_egui::theme::ThemeKind::ALL[0]);
            Ok(Box::new(Dialog::new(found)))
        }),
    );
    let _ = std::fs::remove_file(&scratch);
    shown
}

/// The window's state.
pub struct Dialog {
    pub found: Survey,
    pub choice: Choice,
    /// After removing: what could not be removed (empty: all went), or `None` before.
    pub done: Option<Vec<String>>,
    /// The room the selected items took: what removing them freed.
    pub freed: u64,
    /// The height the content took when last drawn: the window is fitted to it.
    pub content_height: f32,
    /// The window height last asked for, so it is asked once.
    asked_height: f32,
}

/// The window's width, and the room around its content.
const WIDTH: f32 = 560.0;
const MARGIN: i8 = 20;

impl Dialog {
    pub fn new(found: Survey) -> Self {
        let choice = Choice { engine: !found.engine.is_empty(), models: !found.models.is_empty(), settings: false };
        Self { found, choice, done: None, freed: 0, content_height: 0.0, asked_height: 0.0 }
    }

    /// Remove what is selected, and keep what came of it for the window to say.
    pub fn remove_selected(&mut self) {
        let c = self.choice;
        self.freed =
            [(c.engine, self.found.engine_bytes()), (c.models, self.found.models_bytes()), (c.settings, self.found.settings.as_ref().map_or(0, |s| s.bytes))]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, bytes)| bytes)
                .sum();
        self.done = Some(remove(&self.found, c));
    }

    /// What removing came to, in the middle of the window: a mark, a title, a line, Close.
    fn draw_done(&mut self, ui: &mut egui::Ui, failed: &[String]) -> bool {
        use photocraft_ui_egui::widgets;
        let t = photocraft_ui_egui::theme::Tokens::get(ui.ctx());
        let (title, text) = if failed.is_empty() {
            (
                "Selected items removed".to_string(),
                // The size on a line of its own, never split from its unit.
                format!("All the selected items were removed from your hard drive.\nSpace freed: {}", gb(self.freed).replace(' ', "\u{a0}")),
            )
        } else {
            (
                "Some items could not be removed".to_string(),
                format!(
                    "These are still on your hard drive, most likely because a program is using them. You can delete them yourself once the uninstall has finished:\n\n{}",
                    failed.join("\n")
                ),
            )
        };
        // Centred top to bottom: the block's height is estimated, and the room above it is half of what is left.
        let block = 170.0 + if failed.is_empty() { 0.0 } else { 40.0 + 18.0 * failed.len().min(10) as f32 };
        // The window grows when the list needs more room than the question had, and never shrinks.
        self.content_height = self.content_height.max(block + 16.0);
        ui.add_space(((ui.available_height() - block) / 2.0).max(8.0));
        let mut close = false;
        ui.vertical_centered(|ui| {
            let (mark, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            if failed.is_empty() {
                ui.painter().circle_filled(mark.center(), 22.0, t.accent);
                photocraft_ui_egui::icons::paint(ui, mark, "check", 26.0, t.primary_text);
            } else {
                photocraft_ui_egui::icons::paint(ui, mark, "triangle-alert", 34.0, t.warning);
            }
            ui.add_space(12.0);
            ui.label(egui::RichText::new(title).size(18.0).strong().color(t.text));
            ui.add_space(8.0);
            ui.scope(|ui| {
                ui.set_max_width(420.0);
                ui.add(egui::Label::new(egui::RichText::new(text).color(t.text_dim)).wrap());
            });
            ui.add_space(20.0);
            close = widgets::primary_button(ui, "Close", 110.0).clicked();
        });
        close
    }

    /// The window's content; returns whether it should close.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> bool {
        use photocraft_ui_egui::widgets;
        let t = photocraft_ui_egui::theme::Tokens::get(ui.ctx());
        let name = photocraft_ui_egui::brand::NAME;
        if let Some(failed) = self.done.clone() {
            return self.draw_done(ui, &failed);
        }
        let mut close = false;
        let top = ui.cursor().top();
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Uninstall {name}")).size(18.0).strong().color(t.text));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("{name} is being uninstalled. The following items will be kept on this PC unless they are selected for removal:"))
                    .color(t.text_dim),
            )
            .wrap(),
        );
        ui.add_space(10.0);
        let row = |ui: &mut egui::Ui, on: &mut bool, title: &str, items: &[Item], note: &str| {
            let bytes: u64 = items.iter().map(|i| i.bytes).sum();
            ui.horizontal(|ui| {
                widgets::checkbox(ui, on, title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(gb(bytes)).monospace().color(t.text_dim));
                });
            });
            for item in items {
                ui.label(egui::RichText::new(item.path.display().to_string()).size(11.0).color(t.text_faint));
            }
            if !note.is_empty() {
                ui.label(egui::RichText::new(note).size(11.0).color(t.text_faint));
            }
            ui.add_space(8.0);
        };
        if !self.found.engine.is_empty() {
            row(ui, &mut self.choice.engine, "Local AI engine (ComfyUI and its bundled Python)", &self.found.engine.clone(), "");
        }
        if !self.found.models.is_empty() {
            row(ui, &mut self.choice.models, "Downloaded AI models", &self.found.models.clone(), "");
        }
        if let Some(settings) = self.found.settings.clone() {
            row(ui, &mut self.choice.settings, "Settings, presets, API keys and Hugging Face sign-in", std::slice::from_ref(&settings), "");
        }
        if let Some(own) = &self.found.own_comfyui {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Models downloaded into your existing ComfyUI installation ({}) are not removed.", own.display()))
                        .size(11.0)
                        .color(t.text_faint),
                )
                .wrap(),
            );
            ui.add_space(8.0);
        }
        ui.add_space(6.0);
        let buttons = ui.horizontal(|ui| {
            let any = self.choice.engine || self.choice.models || self.choice.settings;
            if ui.add_enabled_ui(any, |ui| widgets::primary_button(ui, "Remove Selected", 130.0)).inner.clicked() {
                self.remove_selected();
            }
            if widgets::secondary_button(ui, "Keep Everything", 130.0).clicked() {
                close = true;
            }
        });
        // To the buttons' foot (the panel's own rect fills the window, whatever its content).
        self.content_height = buttons.response.rect.bottom() - top;
        close
    }
}

impl eframe::App for Dialog {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = photocraft_ui_egui::theme::Tokens::get(ui.ctx());
        let close = egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.card).inner_margin(MARGIN)).show(ui, |ui| self.draw(ui)).inner;
        // The window fits its content: no empty band below the buttons.
        let height = (self.content_height + 2.0 * f32::from(MARGIN)).ceil();
        if self.content_height > 0.0 && height != self.asked_height {
            self.asked_height = height;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(WIDTH, height)));
        }
        if close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_aisetup::plan::Choices;

    /// A folder of the test's own under the system's temp folder, removed again on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("layershop-uninstall-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn the_installer_passes_the_users_folders() {
        let os = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let a = parse(os(&["--local", r"C:\Users\a\AppData\Local\.", "--roaming", r"C:\Users\a\AppData\Roaming\.", "--remove", " AI "]));
        // The installer's "\." (it keeps a trailing backslash from escaping the quote) is dropped.
        assert_eq!(a.local.as_deref(), Some(Path::new(r"C:\Users\a\AppData\Local")));
        assert_eq!((a.roaming.is_some(), a.remove.as_deref()), (true, Some("ai")));
        assert_eq!(parse(os(&["--local"])), Args::default());
        assert_eq!(parse(os(&["--local", "", "junk"])), Args::default());
    }

    #[test]
    fn nothing_there_asks_nothing_and_removes_nothing() {
        let s = Scratch::new("empty");
        let found = survey(Some(&s.0.join("Local")), Some(&s.0.join("Roaming")), "LayerShop");
        assert!(!found.anything());
        assert!(remove(&found, Choice { engine: true, models: true, settings: true }).is_empty());
        assert!(!survey(None, None, "LayerShop").anything());
    }

    #[test]
    fn the_engine_and_models_go_as_selected_and_the_settings_stay() {
        let s = Scratch::new("default");
        let (local, roaming) = (s.0.join("Local"), s.0.join("Roaming"));
        let home = local.join("LayerShop");
        write(&home.join("engine/0.39.0/python_embeded/python.exe"), 3000);
        write(&home.join("models/diffusion_models/a.safetensors"), 5000);
        write(&roaming.join("LayerShop/preferences.json"), 20);
        // A record pointing at the default folders: counted once, also through "\."-ended paths.
        let record = SetupState {
            phase: state::Phase::Done,
            choices: Choices {
                route: Route::Managed,
                engine_dir: home.join("engine").to_string_lossy().into_owned(),
                models_dir: home.join("models").to_string_lossy().into_owned(),
                ..Choices::default()
            },
            ..SetupState::default()
        };
        state::save(&roaming.join("LayerShop/ai-setup.json"), &record).unwrap();
        let dotted = parse([std::ffi::OsString::from("--local"), local.join(".").into_os_string(), "--roaming".into(), roaming.join(".").into_os_string()]);
        let found = survey(dotted.local.as_deref(), dotted.roaming.as_deref(), "LayerShop");
        assert_eq!((found.engine.len(), found.models.len()), (1, 1), "{found:?}");
        let found = survey(Some(&local), Some(&roaming), "LayerShop");
        assert_eq!((found.engine_bytes(), found.models_bytes()), (3000, 5000));
        assert!(found.settings.as_ref().is_some_and(|i| i.bytes >= 20));
        // Selected by default: engine and models, not the settings.
        let dialog = Dialog::new(found.clone());
        assert_eq!(dialog.choice, Choice { engine: true, models: true, settings: false });
        // The engine alone: the models and the home stay.
        assert!(remove(&found, Choice { engine: true, ..Choice::default() }).is_empty());
        assert!(!home.join("engine").exists() && home.join("models/diffusion_models/a.safetensors").is_file());
        // The models too: the home goes with them (it was LayerShop's); the settings stay.
        let found = survey(Some(&local), Some(&roaming), "LayerShop");
        assert!(remove(&found, dialog.choice).is_empty());
        assert!(!home.exists() && roaming.join("LayerShop/preferences.json").is_file());
        // And the settings, when selected.
        let found = survey(Some(&local), Some(&roaming), "LayerShop");
        assert!(remove(&found, Choice { settings: true, ..Choice::default() }).is_empty());
        assert!(!roaming.join("LayerShop").exists());
    }

    #[test]
    fn a_setup_in_folders_of_the_users_choice_loses_only_what_it_installed() {
        let s = Scratch::new("chosen");
        let (local, roaming) = (s.0.join("Local"), s.0.join("Roaming"));
        let (engine, models) = (s.0.join("D/engine"), s.0.join("D/models"));
        // An engine the setup installed (its marker), and one model's files in a shared folder,
        // beside a file of the user's own.
        let root = engine.join("0.39.0");
        write(&root.join("python_embeded/python.exe"), 4000);
        photocraft_aisetup::runtime::write_marker(
            &root,
            &photocraft_aisetup::runtime::Marker { version: "0.39.0".into(), variant: "nvidia".into(), sha256: "ab".into() },
        )
        .unwrap();
        let manifest = Manifest::bundled().unwrap();
        let qwen = manifest.model("qwen-image-2-1").unwrap();
        for key in manifest.file_keys([qwen]) {
            let (folder, name) = split_key(key).unwrap();
            write(&models.join(folder).join(name), 1000);
        }
        write(&models.join("checkpoints/mine.safetensors"), 777);
        let record = SetupState {
            phase: state::Phase::Done,
            choices: Choices {
                route: Route::Managed,
                models: vec!["qwen-image-2-1".into()],
                engine_dir: engine.to_string_lossy().into_owned(),
                models_dir: models.to_string_lossy().into_owned(),
                variant: "nvidia".into(),
                ..Choices::default()
            },
            port: 8190,
            installed: vec!["qwen-image-2-1".into()],
            ..SetupState::default()
        };
        state::save(&roaming.join("LayerShop/ai-setup.json"), &record).unwrap();
        // Settings that start that engine.
        let mut session = photocraft_engine::Session::new();
        session.edit_prefs(|p| {
            p.integrations.ai.comfy_autostart = true;
            p.integrations.ai.comfy_launch.python = root.join("python_embeded/python.exe").to_string_lossy().into_owned();
            p.integrations.ai.comfy_launch.folder = root.to_string_lossy().into_owned();
        });
        std::fs::write(roaming.join("LayerShop/preferences.json"), session.prefs_to_json()).unwrap();
        let found = survey(Some(&local), Some(&roaming), "LayerShop");
        assert!(found.home.is_none());
        assert_eq!(found.engine.len(), 1);
        assert_eq!(found.models_bytes(), 3000, "the setup's three files, not the user's");
        assert!(remove(&found, Choice { engine: true, models: true, settings: false }).is_empty());
        assert!(!root.exists() && models.join("checkpoints/mine.safetensors").is_file());
        assert!(manifest.file_keys([qwen]).into_iter().all(|key| {
            let (folder, name) = split_key(key).unwrap();
            !models.join(folder).join(name).exists()
        }));
        // The settings that stay no longer start an engine that is gone.
        let mut after = photocraft_engine::Session::new();
        after.load_prefs_json(&std::fs::read_to_string(roaming.join("LayerShop/preferences.json")).unwrap()).unwrap();
        let ai = &after.prefs().integrations.ai;
        assert!(!ai.comfy_autostart && ai.comfy_launch.python.is_empty(), "{:?}", ai.comfy_launch);
    }

    #[test]
    fn models_in_the_users_own_comfyui_are_never_touched() {
        let s = Scratch::new("own");
        let (local, roaming) = (s.0.join("Local"), s.0.join("Roaming"));
        let theirs = s.0.join("ComfyUI/models");
        write(&theirs.join("diffusion_models/qwen_image_2.1_bf16.safetensors"), 1000);
        let record = SetupState {
            phase: state::Phase::Done,
            choices: Choices { route: Route::Existing, models_dir: theirs.to_string_lossy().into_owned(), ..Choices::default() },
            installed: vec!["qwen-image-2-1".into()],
            ..SetupState::default()
        };
        state::save(&roaming.join("LayerShop/ai-setup.json"), &record).unwrap();
        let found = survey(Some(&local), Some(&roaming), "LayerShop");
        assert!(found.engine.is_empty() && found.models.is_empty() && found.own_comfyui.is_some());
        assert!(remove(&found, Choice { engine: true, models: true, settings: false }).is_empty());
        assert!(theirs.join("diffusion_models/qwen_image_2.1_bf16.safetensors").is_file());
    }

    #[test]
    fn the_window_draws_asks_and_says_what_it_did() {
        let s = Scratch::new("window");
        let (local, roaming) = (s.0.join("Local"), s.0.join("Roaming"));
        write(&local.join("LayerShop/models/vae/v.safetensors"), 100);
        write(&roaming.join("LayerShop/preferences.json"), 20);
        let mut dialog = Dialog::new(survey(Some(&local), Some(&roaming), "LayerShop"));
        let ctx = egui::Context::default();
        photocraft_ui_egui::PhotocraftApp::setup_context(&ctx, photocraft_ui_egui::theme::ThemeKind::ALL[0]);
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| {
                dialog.draw(ui);
            })
            .textures_delta
            .clear();
        }
        // The window is fitted to the question: two rows and the buttons, well short of a screen.
        assert!((100.0..300.0).contains(&dialog.content_height), "{}", dialog.content_height);
        dialog.remove_selected();
        ctx.run_ui(Default::default(), |ui| {
            dialog.draw(ui);
        })
        .textures_delta
        .clear();
        // The models (selected) went, the settings (not selected) stayed; what it freed is theirs.
        assert_eq!((dialog.done.clone(), dialog.freed), (Some(Vec::new()), 100));
        assert!(!local.join("LayerShop").exists() && roaming.join("LayerShop").exists());
        // What could not be removed is listed, and draws too.
        dialog.done = Some(vec![r"C:\x\in-use.safetensors: Access is denied. (os error 5)".into()]);
        ctx.run_ui(Default::default(), |ui| {
            dialog.draw(ui);
        })
        .textures_delta
        .clear();
    }
}
