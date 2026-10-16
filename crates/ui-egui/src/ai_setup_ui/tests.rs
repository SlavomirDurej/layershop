use super::*;
use photocraft_engine::Session;
use std::time::{Duration, Instant};

/// An app with the stand-in network and engine, its folders in a folder of the test's own.
fn app(scale: demo::Scale) -> (PhotocraftApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    demo::install(&mut app, dir.path(), scale);
    // The walkthrough's machine, so no test asks this one what it has.
    app.ui.ai_setup.probe = Some(demo::probe(dir.path()));
    (app, dir)
}

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    ctx
}

/// Draw the wizard for a few frames (fonts load on the first).
fn draw(app: &mut PhotocraftApp, ctx: &egui::Context) {
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| window(app, ui.ctx())).textures_delta.clear();
    }
}

/// Keep the frames coming until the running setup has ended and been applied.
fn finish(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let began = Instant::now();
    while running(app) {
        assert!(began.elapsed() < Duration::from_secs(30), "the setup did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
    draw(app, ctx);
}

#[test]
fn the_wizard_is_in_the_file_menu_of_the_desktop_app_only() {
    let (mut with, _dir) = app(demo::Scale::Small);
    let items = crate::menus::menu_items(&with);
    let at = items.iter().position(|i| i.id == OPEN).expect("File \u{203a} Set Up Local AI\u{2026}");
    assert_eq!((items[at].label.as_str(), items[at].path.clone(), items[at].enabled), ("Set Up Local AI\u{2026}", vec!["File".to_string()], true));
    // A group of its own, above File Info.
    assert_eq!((items[at - 1].id.as_str(), items[at + 1].id.as_str(), items[at + 2].id.as_str()), ("---", "---", "file.fileInfo"));
    assert_eq!(items.iter().filter(|i| i.id == OPEN).count(), 1);
    assert!(crate::menus::is_live(OPEN));
    // Chosen from the menu, it opens on the first screen.
    let c = ctx();
    let r = crate::menus::invoke(&mut with, &c, OPEN, json!({})).unwrap();
    assert_eq!((r["open"].as_bool(), r["page"].as_str(), with.ui.ai_setup.open), (Some(true), Some("pc"), true));
    assert_eq!(crate::menus::invoke(&mut with, &c, OPEN, json!({"open": false})).unwrap()["open"], false);
    assert!(!with.ui.ai_setup.open);
    let r = crate::menus::invoke(&mut with, &c, OPEN, json!({"page": "models", "route": "existing"})).unwrap();
    assert_eq!((r["page"].as_str(), with.ui.ai_setup.existing), (Some("models"), true));
    // Without the desktop app's network and folders the item is there, greyed, and says why.
    let mut without = PhotocraftApp::new(Session::new(), crate::Services::default());
    let item = crate::menus::menu_items(&without).into_iter().find(|i| i.id == OPEN).unwrap();
    assert!(!item.enabled);
    assert!(crate::menus::invoke(&mut without, &c, OPEN, json!({})).unwrap_err().contains("desktop app"));
    assert!(!without.ui.ai_setup.open);
}

#[test]
fn opening_fills_in_the_folders_and_the_recommended_models() {
    let (mut app, dir) = app(demo::Scale::Real);
    open(&mut app);
    let s = &app.ui.ai_setup;
    assert!(s.open && s.page == Page::Pc && !s.existing);
    assert_eq!(s.engine_dir, dir.path().join("engine").to_string_lossy());
    assert_eq!(s.models_dir, dir.path().join("models").to_string_lossy());
    assert_eq!(
        (s.preset.as_str(), s.models.clone()),
        ("recommended", vec!["qwen-image-2-1".to_string(), "z-image-turbo".to_string(), "flux-2-klein-9b".to_string()])
    );
    assert!(s.probe_job.is_none(), "an answer that is there is not asked for again");
    // The presets: ticking by hand names the preset that results, or none.
    toggle_model(&mut app, "flux-2-klein-9b", false);
    toggle_model(&mut app, "z-image-turbo", false);
    assert_eq!(app.ui.ai_setup.preset, "starter");
    toggle_model(&mut app, "krea-2-turbo", true);
    assert_eq!((app.ui.ai_setup.preset.as_str(), app.ui.ai_setup.models.len()), ("custom", 2));
    toggle_model(&mut app, "krea-2-turbo", true);
    assert_eq!(app.ui.ai_setup.models.len(), 2, "ticked once");
    // Every screen draws, in every state it has here.
    let c = ctx();
    for existing in [false, true] {
        app.ui.ai_setup.existing = existing;
        for page in Page::ALL {
            app.ui.ai_setup.page = page;
            draw(&mut app, &c);
        }
    }
    // The plan for the choices was asked of the engine, and names what is left out.
    app.ui.ai_setup.existing = false;
    app.ui.ai_setup.page = Page::Models;
    app.ui.ai_setup.models = preset_models(&app, "recommended");
    draw(&mut app, &c);
    let plan = app.ui.ai_setup.plan.clone().unwrap();
    assert_eq!(plan["models"].as_array().unwrap().iter().filter(|m| !m["skipped"].is_null()).count(), 1, "FLUX.2 Klein needs a sign-in");
    assert!(plan["downloadBytes"].as_u64().unwrap() > 30_000_000_000);
    assert!(!dir.path().join("engine").exists(), "looking changes nothing");
}

#[test]
fn a_setup_runs_from_the_wizard_to_its_test_image() {
    let (mut app, dir) = app(demo::Scale::Small);
    let c = ctx();
    open(&mut app);
    // The user's own ComfyUI, which has none of the files yet.
    let s = &mut app.ui.ai_setup;
    (s.existing, s.url, s.page) = (true, "http://127.0.0.1:8188".into(), Page::Location);
    connect(&mut app);
    assert!(app.ui.ai_setup.note.is_empty(), "{}", app.ui.ai_setup.note);
    let server = app.ui.ai_setup.server.clone().unwrap();
    assert_eq!((server["local"].as_bool(), server["folders"].is_object()), (Some(true), true));
    app.ui.ai_setup.models_dir = dir.path().join("models").to_string_lossy().into_owned();
    app.ui.ai_setup.models = vec!["qwen-image-2-1".into()];
    app.ui.ai_setup.page = Page::Models;
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.plan.as_ref().unwrap()["downloadBytes"], 24_000);
    install(&mut app);
    assert!(app.ui.ai_setup.note.is_empty(), "{}", app.ui.ai_setup.note);
    assert_eq!(app.ui.ai_setup.page, Page::Install);
    finish(&mut app, &c);
    // It ended on the last screen, with the image the engine rendered.
    assert_eq!(app.ui.ai_setup.page, Page::Ready);
    let result = app.ui.ai_setup.status.as_ref().unwrap()["result"].clone();
    assert_eq!(
        (result["ok"].as_bool(), result["installed"][0].as_str(), result["testImage"]["modelName"].as_str()),
        (Some(true), Some("qwen-image-2-1"), Some("Qwen Image 2.1"))
    );
    assert_eq!(test_image(&app, &c).map(|t| t.size()), Some([128, 128]));
    assert!(dir.path().join("models").join("vae").join("qwen_image_2.1_vae_bf16.safetensors").is_file());
    // The settings were written, and the panel no longer offers the setup.
    let ai = &app.session.prefs().integrations.ai;
    assert_eq!(ai.comfy_url, "http://127.0.0.1:8188");
    assert!(ai.model("qwen-image-2-1").is_some_and(|m| m.enabled));
    assert!(set_up(&app) && progress_line(&app).is_none());
    assert_eq!(app.ui.status, "Local AI is set up");
    // Open AI Gen leaves the wizard for the panel.
    draw(&mut app, &c);
    let summary = summary(&app);
    assert_eq!((summary["installed"].as_bool(), summary["managed"].as_bool()), (Some(true), Some(false)));
    assert!(summary["text"].as_str().unwrap().contains("your own ComfyUI"));
}

#[test]
fn a_running_setup_shows_in_the_status_bar_and_can_be_paused_and_continued() {
    let (mut app, _dir) = app(demo::Scale::Real);
    let c = ctx();
    open(&mut app);
    app.ui.ai_setup.models = vec!["qwen-image-2-1".into()];
    app.ui.ai_setup.page = Page::Models;
    draw(&mut app, &c);
    install(&mut app);
    assert!(app.ui.ai_setup.note.is_empty(), "{}", app.ui.ai_setup.note);
    assert!(running(&app));
    // The window can be closed: the setup carries on, and says so where it will be seen.
    app.ui.ai_setup.open = false;
    let began = Instant::now();
    loop {
        draw(&mut app, &c);
        let arrived = app.ui.ai_setup.status.as_ref().map_or(0, |s| number(s, "/progress/bytesDone"));
        if arrived > 0 {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(20), "nothing arrived");
        std::thread::sleep(Duration::from_millis(50));
    }
    let (fraction, line) = progress_line(&app).unwrap();
    assert!(fraction > 0.0 && line.starts_with("Setting up local AI:") && line.contains("GB"), "{line}");
    assert!(!set_up(&app), "not until it has finished");
    // Pause: it stops, what arrived is kept, and nothing is written to the settings.
    app.session.execute("ai.setup.cancel", json!({})).unwrap();
    finish(&mut app, &c);
    let status = app.ui.ai_setup.status.clone().unwrap();
    assert_eq!((status["state"].as_str(), status["result"]["cancelled"].as_bool()), (Some("paused"), Some(true)));
    assert_eq!(app.session.prefs().integrations.ai.comfy_url, photocraft_aigen::Settings::default().comfy_url);
    assert!(progress_line(&app).is_none());
    assert!(summary(&app)["text"].as_str().unwrap().contains("interrupted"));
    // Opened again, the first screen offers to continue it; Continue runs it without asking.
    open(&mut app);
    assert_eq!(app.ui.ai_setup.page, Page::Pc);
    draw(&mut app, &c);
    resume(&mut app);
    assert!(running(&app) && app.ui.ai_setup.page == Page::Install);
    app.session.execute("ai.setup.cancel", json!({})).unwrap();
    finish(&mut app, &c);
}

#[test]
fn the_wizard_is_offered_once_at_first_launch_and_again_only_to_continue() {
    let (mut app, _dir) = app(demo::Scale::Small);
    offer_at_first_launch(&mut app);
    assert!(app.ui.ai_setup.open, "nothing is set up: offered");
    assert!(app.session.ai_setup.record().offered);
    // Closed, and the app started again: it is not offered a second time.
    app.ui.ai_setup = AiSetupUi::default();
    offer_at_first_launch(&mut app);
    assert!(!app.ui.ai_setup.open);
    // An app whose local AI the user set up by hand is not asked at all.
    let (mut configured, _dir2) = self::app(demo::Scale::Small);
    configured.run("prefs.set", json!({"path": "integrations.ai.comfyAutostart", "value": true})).unwrap();
    offer_at_first_launch(&mut configured);
    assert!(!configured.ui.ai_setup.open && configured.session.ai_setup.record().offered);
    // Without the desktop app there is nothing to offer.
    let mut plain = PhotocraftApp::new(Session::new(), crate::Services::default());
    offer_at_first_launch(&mut plain);
    assert!(!plain.ui.ai_setup.open);
}

#[test]
fn the_preferences_page_hands_over_to_the_wizard_after_saving_itself() {
    let (mut app, _dir) = app(demo::Scale::Small);
    let c = ctx();
    let id = crate::prefs_ui::open_preferences(&mut app, photocraft_engine::prefs::GENERATIVE_AI.0);
    // A change made in the dialog before the button is pressed.
    app.ui.dialog_mut(id).unwrap().fields["values"]["integrations"]["ai"]["timeoutSeconds"] = json!(777);
    request_from_prefs(&mut app, "open");
    draw(&mut app, &c);
    assert!(app.ui.dialogs.is_empty(), "the dialog was confirmed, not left behind the wizard");
    assert_eq!(app.session.prefs().integrations.ai.timeout_seconds, 777);
    assert!(app.ui.ai_setup.open && app.ui.ai_setup.page == Page::Pc);
    // Remove asks first; Repair with nothing set up says so on the screen.
    request_from_prefs(&mut app, "remove");
    draw(&mut app, &c);
    assert!(app.ui.ai_setup.confirm_remove);
    request_from_prefs(&mut app, "repair");
    draw(&mut app, &c);
    assert!(app.ui.ai_setup.note.contains("no setup to repair"), "{}", app.ui.ai_setup.note);
    // The page's own line about what is installed.
    assert!(summary(&app)["text"].as_str().unwrap().contains("Not set up yet"));
    assert_eq!(summary(&app)["available"], true);
    let plain = PhotocraftApp::new(Session::new(), crate::Services::default());
    assert_eq!(summary(&plain)["available"], false);
}

#[test]
fn the_token_is_saved_with_the_keys_and_never_shown_in_the_state() {
    let (mut app, _dir) = app(demo::Scale::Small);
    let saved = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = saved.clone();
    app.services.save_ai_keys = Some(Box::new(move |text: &str| {
        *sink.lock().unwrap() = text.to_string();
        Ok(())
    }));
    open(&mut app);
    app.ui.ai_setup.token = "hf_secret_token".into();
    assert!(!serde_json::to_string(&app.ui.ai_setup).unwrap().contains("hf_secret_token"), "what ui.inspect reports");
    save_token(&mut app);
    assert!(app.ui.ai_setup.token.is_empty() && app.ui.ai_setup.note.is_empty());
    assert!(saved.lock().unwrap().contains("hf_secret_token"));
    assert_eq!(app.ui.ai_setup.status.as_ref().unwrap()["token"], true);
    assert!(!serde_json::to_string(&app.ui).unwrap().contains("hf_secret_token"));
    // With it, the plan no longer leaves the gated model out.
    let c = ctx();
    app.ui.ai_setup.page = Page::Models;
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.plan.as_ref().unwrap()["token"], true);
}

#[test]
fn a_comfyui_that_does_not_answer_is_said_on_the_screen() {
    let (mut app, _dir) = app(demo::Scale::Small);
    struct Down;
    impl photocraft_aigen::Transport for Down {
        fn send(&self, _: &photocraft_aigen::Request) -> photocraft_aigen::Result<photocraft_aigen::Response> {
            Err(photocraft_aigen::Error::Network("refused".into()))
        }
    }
    app.session.ai.transport = Some(std::sync::Arc::new(Down));
    open(&mut app);
    app.ui.ai_setup.existing = true;
    app.ui.ai_setup.url = "http://127.0.0.1:8188".into();
    connect(&mut app);
    assert!(app.ui.ai_setup.server.is_none());
    assert!(!app.ui.ai_setup.note.is_empty());
    // Odd state from outside (a script, a later version) draws, and asks for nothing absurd.
    let c = ctx();
    let odd: AiSetupUi = serde_json::from_value(json!({
        "open": true, "page": "install", "existing": true, "preset": "nonsense", "models": ["no-such-model", ""],
        "engineDir": "relative", "modelsDir": "\u{0}", "url": "not a url",
        "probe": {"checks": [{"level": 7}, "x"], "vramGb": "lots"}, "manifest": [1, 2], "plan": "x", "status": {"progress": {"steps": [1, {"total": -5}]}},
        "server": 7, "note": "x".repeat(5000)
    }))
    .unwrap();
    app.ui.ai_setup = odd;
    for page in Page::ALL {
        app.ui.ai_setup.page = page;
        draw(&mut app, &c);
    }
    assert!(!running(&app));
    assert_eq!(serde_json::from_value::<AiSetupUi>(json!({})).unwrap(), AiSetupUi::default());
}

#[test]
fn an_installed_setup_can_lose_a_model_or_everything_without_being_taken_for_a_new_one() {
    let (mut app, dir) = app(demo::Scale::Small);
    let root = demo::pretend_installed(&app, dir.path()).unwrap();
    let c = ctx();
    open(&mut app);
    assert!(set_up(&app), "the record says it is installed");
    assert_eq!(crate::menus::menu_items(&app).into_iter().find(|i| i.id == OPEN).unwrap().label, "Local AI Setup\u{2026}");
    let summary = summary(&app);
    assert!(summary["text"].as_str().unwrap().contains("Installed: ComfyUI 0.39.0 \u{b7} 2 models"), "{summary}");
    assert_eq!((summary["installed"].as_bool(), summary["managed"].as_bool()), (Some(true), Some(true)));
    // The first screen says what is there, and the Models screen marks both installed.
    for page in [Page::Pc, Page::Models] {
        app.ui.ai_setup.page = page;
        draw(&mut app, &c);
    }
    let installed =
        |app: &PhotocraftApp| list(app.ui.ai_setup.manifest.as_ref().unwrap(), "/models").iter().map(|m| m["installed"].as_bool().unwrap()).collect::<Vec<_>>();
    assert_eq!(installed(&app), [true, true]);
    // One model's files go. It is not a setup that finished: the screen stays where it is.
    remove_model(&mut app, "z-image-turbo");
    draw(&mut app, &c);
    assert!(app.ui.ai_setup.note.contains("Space freed: 24.0 KB"), "{}", app.ui.ai_setup.note);
    assert_eq!((app.ui.ai_setup.page, installed(&app)), (Page::Models, vec![true, false]));
    assert!(app.ui.ai_setup.remove_model.is_none() && !app.ui.ai_setup.models.contains(&"z-image-turbo".to_string()));
    assert!(root.join("python_embeded").join("python.exe").is_file());
    // Everything goes: back on the first screen, which says how much room that made.
    app.ui.ai_setup.confirm_remove = true;
    remove(&mut app, true);
    draw(&mut app, &c);
    assert!(!root.exists() && app.ui.ai_setup.page == Page::Pc && !app.ui.ai_setup.confirm_remove);
    assert!(app.ui.ai_setup.note.starts_with("Local AI was removed. Space freed:"), "{}", app.ui.ai_setup.note);
    assert!(!set_up(&app));
    assert_eq!(installed(&app), [false, false]);
}

#[test]
fn a_comfyui_on_this_pc_is_a_choice_of_two_buttons_and_its_models_folder_is_filled_in() {
    let (mut app, dir) = app(demo::Scale::Small);
    app.ui.ai_setup.probe = Some(demo::probe_with_comfy(dir.path()));
    let folders = demo::comfy_folders(dir.path());
    let c = ctx();
    open(&mut app);
    // The first screen says it was found and where its models are, and asks which to use with
    // the two buttons in place of Next.
    let checks = app.ui.ai_setup.probe.clone().unwrap()["checks"].clone();
    let comfy = list(&checks, "").iter().find(|c| c["id"] == "comfy").cloned().unwrap();
    assert_eq!((comfy["title"].as_str(), comfy["value"].as_str()), (Some("ComfyUI detected"), Some("installed")));
    assert!(comfy["detail"].as_str().unwrap().contains(&folders[0]), "{comfy}");
    assert!(offer_choice(&app));
    draw(&mut app, &c);
    // Without one, it is the usual Next.
    let (mut plain, _dir2) = self::app(demo::Scale::Small);
    open(&mut plain);
    assert!(!offer_choice(&plain));
    // Continue with My ComfyUI: its models folder is filled in, the one it saves new models to
    // first, and the others are offered.
    use_existing(&mut app);
    let s = &app.ui.ai_setup;
    assert!(s.existing && s.page == Page::Location && s.note.is_empty(), "{}", s.note);
    assert_eq!((s.models_dir.as_str(), s.folders.clone()), (folders[0].as_str(), folders.clone()));
    assert!(s.server.is_some(), "connected at once");
    draw(&mut app, &c);
    // Another of its folders, picked from the list, stays when it connects again; so does one typed in.
    app.ui.ai_setup.models_dir = folders[1].clone();
    connect(&mut app);
    assert_eq!(app.ui.ai_setup.models_dir, folders[1]);
    let typed = dir.path().join("elsewhere").to_string_lossy().into_owned();
    app.ui.ai_setup.models_dir = typed.clone();
    connect(&mut app);
    assert_eq!(app.ui.ai_setup.models_dir, typed);
    draw(&mut app, &c);
    // Changing one's mind: a new ComfyUI after all, with the app's own models folder; and back.
    use_managed(&mut app);
    let s = &app.ui.ai_setup;
    assert!(!s.existing && s.page == Page::Location);
    assert_eq!(s.models_dir, dir.path().join("models").to_string_lossy());
    draw(&mut app, &c);
    use_existing(&mut app);
    assert_eq!(app.ui.ai_setup.models_dir, folders[0]);
    // A folder chosen for the new ComfyUI stays when Next is pressed again.
    use_managed(&mut app);
    app.ui.ai_setup.models_dir = typed.clone();
    app.ui.ai_setup.page = Page::Pc;
    use_managed(&mut app);
    assert_eq!(app.ui.ai_setup.models_dir, typed);
    // Set up already: no question.
    let (mut installed, dir3) = self::app(demo::Scale::Small);
    installed.ui.ai_setup.probe = Some(demo::probe_with_comfy(dir3.path()));
    demo::pretend_installed(&installed, dir3.path()).unwrap();
    open(&mut installed);
    assert!(!offer_choice(&installed));
}

#[test]
fn where_a_comfyui_keeps_its_models_is_read_from_its_answer_or_its_settings() {
    let (mut app, dir) = app(demo::Scale::Small);
    open(&mut app);
    // Nothing found yet: the screen says where to look, and the folder is left to choose.
    app.ui.ai_setup.probe = Some(demo::probe(dir.path()));
    struct Older;
    impl photocraft_aigen::Transport for Older {
        fn send(&self, r: &photocraft_aigen::Request) -> photocraft_aigen::Result<photocraft_aigen::Response> {
            // A ComfyUI from before `/experiment/models`.
            let body: &[u8] = if r.url.ends_with("/system_stats") {
                br#"{"system":{"argv":["main.py"]}}"#
            } else if r.url.contains("/models/") {
                b"[]"
            } else {
                return Ok(photocraft_aigen::Response { status: 404, body: Vec::new() });
            };
            Ok(photocraft_aigen::Response { status: 200, body: body.to_vec() })
        }
    }
    app.session.ai.transport = Some(std::sync::Arc::new(Older));
    use_existing(&mut app);
    let s = &app.ui.ai_setup;
    assert!(s.server.is_some() && s.folders.is_empty() && s.models_dir.is_empty(), "{:?}", (&s.folders, &s.models_dir));
    let c = ctx();
    draw(&mut app, &c);
    // A remote ComfyUI's folders are not this computer's.
    let mut found = json!({"folders": {}, "modelsFolders": ["/srv/models"]});
    app.ui.ai_setup.url = "http://192.168.1.20:8188".into();
    connected(&mut app, Some(found.clone()));
    assert!(app.ui.ai_setup.folders.is_empty());
    // A local one's answer is taken.
    app.ui.ai_setup.url = "http://127.0.0.1:8188".into();
    found["modelsFolders"] = json!(["", "/srv/models", 7]);
    connected(&mut app, Some(found));
    assert_eq!((app.ui.ai_setup.models_dir.as_str(), app.ui.ai_setup.folders.clone()), ("/srv/models", vec!["/srv/models".to_string()]));
    assert!(same_folder(r"C:\Users\a\models\", "c:/users/a/models") && !same_folder("C:/a", "C:/b"));
}

#[test]
fn a_local_generation_that_worked_ends_the_offer() {
    let (mut app, _dir) = app(demo::Scale::Small);
    assert!(!set_up(&app));
    // A cloud model's image says nothing about local AI.
    crate::ai_gen_ui::finished(&mut app, &json!({"model": "gpt-image-2-5-sunburst", "seed": 1}));
    assert!(!set_up(&app));
    crate::ai_gen_ui::finished(&mut app, &json!({"model": "qwen-image-2-1", "seed": 1}));
    assert!(set_up(&app), "the user's own ComfyUI rendered: there is nothing to set up");
    for odd in [json!({}), json!({"model": 7}), json!({"model": "no-such-model"}), json!({"model": ""})] {
        let mut fresh = self::app(demo::Scale::Small).0;
        crate::ai_gen_ui::finished(&mut fresh, &odd);
        assert!(!set_up(&fresh), "{odd}");
    }
}

/// A frame with the app's window focused, or not (the user away in the browser).
fn draw_focused(app: &mut PhotocraftApp, ctx: &egui::Context, focused: bool) {
    let mut input = egui::RawInput::default();
    input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
    ctx.run_ui(input, |ui| window(app, ui.ctx())).textures_delta.clear();
}

/// An app whose stand-in has FLUX.2 Klein 9B (gated) ticked beside Qwen Image 2.1, on the Models
/// screen; the token its file opens to; and what the app saved as its keys file.
fn gated_app() -> (PhotocraftApp, tempfile::TempDir, std::sync::Arc<std::sync::Mutex<String>>, std::sync::Arc<std::sync::Mutex<String>>) {
    let (mut app, dir) = app(demo::Scale::Small);
    let accepted = demo::install(&mut app, dir.path(), demo::Scale::Gated);
    let saved = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = saved.clone();
    app.services.save_ai_keys = Some(Box::new(move |text: &str| {
        *sink.lock().unwrap() = text.to_string();
        Ok(())
    }));
    open(&mut app);
    // The user's own ComfyUI (the stand-in), which has none of the files: no engine to download,
    // so a setup runs to its end in moments.
    let s = &mut app.ui.ai_setup;
    (s.existing, s.url) = (true, "http://127.0.0.1:8188".into());
    s.server = Some(json!({"folders": {}, "launch": null, "local": true}));
    s.models_dir = dir.path().join("models").to_string_lossy().into_owned();
    s.models = vec!["qwen-image-2-1".into(), "flux-2-klein-9b".into()];
    s.page = Page::Models;
    (app, dir, accepted, saved)
}

#[test]
fn a_gated_model_is_sign_in_then_licence_then_ready_and_installs_only_then() {
    let (mut app, dir, accepted, saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    // Not signed in: the plan says FLUX.2 Klein needs it, and its row asks to sign in.
    assert_eq!(needs_sign_in(&app).iter().map(|m| text(m, "/id").to_string()).collect::<Vec<_>>(), ["flux-2-klein-9b"]);
    assert_eq!(access_of(&app, "flux-2-klein-9b"), "signIn");
    assert!(hf_flag(&app, "canSignIn") && !hf_flag(&app, "signedIn"));
    // Install asks first, and starts nothing.
    install_clicked(&mut app);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::SignIn));
    assert!(!running(&app));
    draw(&mut app, &c);
    // Install in the question does nothing yet.
    sign_in_answered(&mut app, widgets::ButtonRole::Default);
    assert!(app.ui.ai_setup.ask == Some(Ask::SignIn) && !running(&app));
    // Signed in (as Sign in with Hugging Face leaves it): saved with the keys, step 2.
    *accepted.lock().unwrap() = "someone-elses".into();
    demo::sign_in(&mut app);
    draw(&mut app, &c);
    assert!(saved.lock().unwrap().contains(demo::TOKEN), "the keys file has the sign-in");
    refresh_hf(&mut app);
    check_access(&mut app);
    assert_eq!(access_of(&app, "flux-2-klein-9b"), "licence");
    assert_eq!(text(app.ui.ai_setup.hf.as_ref().unwrap(), "/user"), "pixel-painter");
    draw(&mut app, &c);
    // The licence is accepted in the browser; back in the app, it is asked again by itself.
    *accepted.lock().unwrap() = demo::TOKEN.into();
    draw_focused(&mut app, &c, false);
    assert_eq!(access_of(&app, "flux-2-klein-9b"), "licence", "not while away");
    draw_focused(&mut app, &c, true);
    assert_eq!(access_of(&app, "flux-2-klein-9b"), "ready");
    // Install, from the question: everything installs, FLUX.2 Klein too.
    sign_in_answered(&mut app, widgets::ButtonRole::Default);
    assert!(app.ui.ai_setup.ask.is_none() && app.ui.ai_setup.page == Page::Install, "{}", app.ui.ai_setup.note);
    finish(&mut app, &c);
    let result = app.ui.ai_setup.status.as_ref().unwrap()["result"].clone();
    assert_eq!(result["ok"], true, "{result}");
    assert!(dir.path().join("models").join("diffusion_models").join("flux-2-klein-9b-fp8.safetensors").is_file());
    // Nothing the wizard shows or reports carries a token.
    let state = serde_json::to_string(&app.ui).unwrap();
    assert!(!state.contains(demo::TOKEN) && !state.contains("hf_demo_refresh"));
    // Signing out forgets it, and saves that.
    app.ui.ai_setup.page = Page::Models;
    sign_out(&mut app);
    assert!(!hf_flag(&app, "signedIn") && !saved.lock().unwrap().contains(demo::TOKEN));
}

#[test]
fn install_without_leaves_the_gated_model_out() {
    let (mut app, dir, _accepted, _saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    install_clicked(&mut app);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::SignIn));
    sign_in_answered(&mut app, widgets::ButtonRole::Alternate);
    assert!(app.ui.ai_setup.ask.is_none() && !app.ui.ai_setup.models.contains(&"flux-2-klein-9b".to_string()));
    assert_eq!(app.ui.ai_setup.models, ["qwen-image-2-1"]);
    finish(&mut app, &c);
    assert_eq!(app.ui.ai_setup.status.as_ref().unwrap()["result"]["ok"], true);
    assert!(!dir.path().join("models").join("diffusion_models").join("flux-2-klein-9b-fp8.safetensors").exists());
    // Cancel just closes the question.
    let (mut other, _dir2, _a, _s) = gated_app();
    draw(&mut other, &c);
    install_clicked(&mut other);
    sign_in_answered(&mut other, widgets::ButtonRole::Cancel);
    assert!(other.ui.ai_setup.ask.is_none() && !running(&other) && other.ui.ai_setup.models.len() == 2);
}

#[test]
fn the_access_token_is_the_fallback_and_a_token_on_this_pc_is_never_used_unasked() {
    let (mut app, _dir, accepted, saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    // Whatever is on this PC, nothing is signed in until the user says so.
    assert!(!hf_flag(&app, "signedIn") && app.session.ai.hf_token().is_empty());
    // The token, pasted: saved with the keys, checked like a sign-in.
    *accepted.lock().unwrap() = "hf_pasted".into();
    app.ui.ai_setup.token_open = true;
    draw(&mut app, &c);
    app.ui.ai_setup.token = "hf_pasted".into();
    save_token(&mut app);
    assert!(saved.lock().unwrap().contains("hf_pasted") && !app.ui.ai_setup.token_open);
    draw(&mut app, &c);
    assert_eq!(access_of(&app, "flux-2-klein-9b"), "ready");
    assert_eq!(text(app.ui.ai_setup.hf.as_ref().unwrap(), "/via"), "token");
    install_clicked(&mut app);
    assert!(app.ui.ai_setup.ask.is_none() && running(&app), "ready: Install installs at once");
    finish(&mut app, &c);
}

#[test]
fn closing_while_downloading_asks_and_the_status_bar_brings_it_back() {
    let (mut app, _dir) = app(demo::Scale::Real);
    let c = ctx();
    open(&mut app);
    app.ui.ai_setup.models = vec!["qwen-image-2-1".into()];
    app.ui.ai_setup.page = Page::Models;
    draw(&mut app, &c);
    install(&mut app);
    assert!(running(&app));
    // Close (or the title bar's ×) asks; Cancel goes back to the wizard.
    app.ui.ai_setup.ask = Some(Ask::Close);
    draw(&mut app, &c);
    close_answered(&mut app, widgets::ButtonRole::Cancel);
    assert!(app.ui.ai_setup.open && app.ui.ai_setup.ask.is_none() && running(&app));
    // Keep downloading: the window closes, the setup carries on, and the status bar shows it.
    close_answered(&mut app, widgets::ButtonRole::Default);
    assert!(!app.ui.ai_setup.open && running(&app));
    draw(&mut app, &c);
    assert!(progress_line(&app).is_some());
    // Clicking the status bar's readout brings the wizard back, on the install step.
    app.ui.ai_setup.page = Page::Models;
    show_progress(&mut app);
    assert!(app.ui.ai_setup.open && app.ui.ai_setup.page == Page::Install);
    // Stop Download: it pauses, what arrived is kept, and the window closes.
    close_answered(&mut app, widgets::ButtonRole::Alternate);
    assert!(!app.ui.ai_setup.open);
    finish(&mut app, &c);
    assert_eq!(app.ui.ai_setup.status.as_ref().unwrap()["state"], "paused");
    // Not running: × closes without asking.
    app.ui.ai_setup.open = true;
    app.ui.ai_setup.ask = Some(Ask::Close);
    draw(&mut app, &c);
    assert!(!app.ui.ai_setup.open && app.ui.ai_setup.ask.is_none(), "nothing to decide once it has stopped");
}
