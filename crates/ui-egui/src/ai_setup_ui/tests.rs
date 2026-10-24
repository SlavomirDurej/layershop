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
        ("recommended", vec!["qwen-image-2-1".to_string(), "z-image-turbo".to_string(), "flux-3-klein".to_string()])
    );
    assert!(s.probe_job.is_none(), "an answer that is there is not asked for again");
    // The presets: ticking by hand names the preset that results, or none.
    toggle_model(&mut app, "flux-3-klein", false);
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
    assert_eq!(plan["models"].as_array().unwrap().iter().filter(|m| !m["skipped"].is_null()).count(), 1, "FLUX.3 Klein needs a sign-in");
    assert!(plan["downloadBytes"].as_u64().unwrap() > 30_000_000_000);
    assert!(!dir.path().join("engine").exists(), "looking changes nothing");
}

/// A card with little memory starts on Light (MOSS Image), whether the probe has answered when
/// the wizard opens or answers after; a selection the user made stays theirs.
#[test]
fn a_small_card_starts_on_the_light_preset() {
    let small = |dir: &std::path::Path| {
        let mut found = demo::probe(dir);
        found["vramGb"] = json!(8);
        found
    };
    let (mut app, dir) = app(demo::Scale::Real);
    app.ui.ai_setup.probe = Some(small(dir.path()));
    open(&mut app);
    assert_eq!((app.ui.ai_setup.preset.as_str(), app.ui.ai_setup.models.clone()), ("light", vec!["moss-image".to_string()]));
    assert_eq!(list(app.ui.ai_setup.manifest.as_ref().unwrap(), "/models")[0]["id"], "moss-image");
    let c = ctx();
    app.ui.ai_setup.page = Page::Models;
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.plan.as_ref().unwrap()["models"][0]["id"], "moss-image");

    // Opened before the probe answered: Recommended, until the answer says the card is small.
    let (mut app, dir) = app_without_probe();
    open(&mut app);
    assert_eq!(app.ui.ai_setup.preset, "recommended");
    probed(&mut app, small(dir.path()));
    assert_eq!((app.ui.ai_setup.preset.as_str(), app.ui.ai_setup.models.clone()), ("light", vec!["moss-image".to_string()]));

    // Ticked by hand first: the answer changes nothing.
    let (mut app, dir) = app_without_probe();
    open(&mut app);
    toggle_model(&mut app, "krea-2-turbo", true);
    let chosen = app.ui.ai_setup.models.clone();
    probed(&mut app, small(dir.path()));
    assert_eq!((app.ui.ai_setup.preset.as_str(), app.ui.ai_setup.models.clone()), ("custom", chosen));
}

/// [`app`] before this PC was looked at.
fn app_without_probe() -> (PhotocraftApp, tempfile::TempDir) {
    let (mut app, dir) = app(demo::Scale::Real);
    app.ui.ai_setup.probe = None;
    (app, dir)
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
    // Neither port answered: a popup asks to start ComfyUI, and it draws.
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::NotRunning));
    draw(&mut app, &ctx());
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::NotRunning));
    // ComfyUI Desktop started meanwhile, on its own port: Try Again finds it and fixes the address.
    struct DesktopUp;
    impl photocraft_aigen::Transport for DesktopUp {
        fn send(&self, r: &photocraft_aigen::Request) -> photocraft_aigen::Result<photocraft_aigen::Response> {
            if !r.url.starts_with("http://127.0.0.1:8000/") {
                return Err(photocraft_aigen::Error::Network("refused".into()));
            }
            let body: &[u8] = if r.url.ends_with("/system_stats") { br#"{"system":{"argv":["main.py"]}}"# } else { b"[]" };
            Ok(photocraft_aigen::Response { status: 200, body: body.to_vec() })
        }
    }
    app.session.ai.transport = Some(std::sync::Arc::new(DesktopUp));
    app.ui.ai_setup.ask = None;
    connect(&mut app);
    assert_eq!(app.ui.ai_setup.ask, None);
    assert_eq!(app.ui.ai_setup.url, "http://127.0.0.1:8000");
    assert!(app.ui.ai_setup.server.is_some());
    app.session.ai.transport = Some(std::sync::Arc::new(Down));
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
    assert!(summary["text"].as_str().unwrap().contains("Installed: ComfyUI 0.42.1 \u{b7} 2 models"), "{summary}");
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
fn without_a_graphics_card_a_new_comfyui_installs_only_on_the_processor_when_asked() {
    let (mut app, dir) = app(demo::Scale::Small);
    app.ui.ai_setup.probe = Some(demo::probe_without_card(dir.path()));
    let c = ctx();
    open(&mut app);
    // The card's row stops local models, and nothing can be installed yet.
    let checks = app.ui.ai_setup.probe.clone().unwrap()["checks"].clone();
    let gpu = list(&checks, "").iter().find(|c| c["id"] == "gpu").cloned().unwrap();
    assert_eq!((gpu["level"].as_str(), gpu["title"].as_str()), (Some("stop"), Some("VMware SVGA 3D")));
    assert!(!gpu_usable(&app) && !can_install_new(&app));
    assert!(cant_install_why(&app).contains("Install anyway"));
    draw(&mut app, &c);
    // Install anyway: on the processor, which the setup is told.
    app.ui.ai_setup.cpu = true;
    assert!(can_install_new(&app));
    use_managed(&mut app);
    assert_eq!(params(&app)["cpu"], json!(true));
    draw(&mut app, &c);
    // With a card, the processor is never asked for, even when the box was ticked before.
    let (mut card, _dir2) = self::app(demo::Scale::Small);
    card.ui.ai_setup.cpu = true;
    open(&mut card);
    assert!(gpu_usable(&card) && can_install_new(&card));
    assert_eq!(params(&card)["cpu"], json!(false));
    // Continue Setup goes by this PC too: a setup begun before the missing card was noticed
    // continues on the processor; with a card, on it; before the PC is checked, as it was.
    assert_eq!(resume_params(&app), json!({"cpu": true}));
    assert_eq!(resume_params(&card), json!({"cpu": false}));
    card.ui.ai_setup.probe = None;
    assert_eq!(resume_params(&card), json!({}));
}

#[test]
fn an_engine_slow_to_start_says_what_it_is_doing_and_shows_its_own_lines() {
    let (mut app, dir) = app(demo::Scale::Small);
    assert!(demo::slow_engine(&mut app, dir.path(), false), "the setup reached the engine's step");
    let c = ctx();
    draw(&mut app, &c);
    // The step says where the engine is, from what it printed, and for how long.
    let status = app.ui.ai_setup.status.clone().unwrap();
    let verify = list(&status, "/progress/steps").iter().find(|s| text(s, "/key") == "verify").cloned().unwrap();
    let words = step_words(&verify);
    assert!(words.starts_with("Starting ComfyUI: loading PyTorch · 3:1"), "{words}");
    // What's happening? shows the engine's own lines, without their colour codes, and its log.
    let lines = list(&status, "/engine/output");
    assert!(lines.len() == 6 && lines.iter().all(|l| !l.as_str().unwrap().contains('\u{1b}')), "{lines:?}");
    assert!(text(&status, "/engine/log").ends_with("comfyui.log"));
    app.ui.ai_setup.ask = Some(Ask::Activity);
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::Activity), "it stays open while the setup runs");
    // Stopped: the setup ends, and the window says the engine is not working any more.
    let _ = app.session.execute("ai.setup.cancel", json!({}));
    finish(&mut app, &c);
    assert!(!running(&app));
    draw(&mut app, &c);
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
    // Without one, the same two buttons: one this look missed can still be connected.
    let (mut plain, _dir2) = self::app(demo::Scale::Small);
    open(&mut plain);
    assert!(offer_choice(&plain) && !detected(&plain));
    draw(&mut plain, &c);
    // Before the look has answered, there is nothing to choose yet.
    plain.ui.ai_setup.probe = None;
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

/// An app whose stand-in has FLUX.3 Klein (gated) ticked beside Qwen Image 2.1, on the Models
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
    s.models = vec!["qwen-image-2-1".into(), "flux-3-klein".into()];
    s.page = Page::Models;
    (app, dir, accepted, saved)
}

#[test]
fn a_gated_model_is_sign_in_then_licence_then_ready_and_installs_only_then() {
    let (mut app, dir, accepted, saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    // Not signed in: the plan says FLUX.3 Klein needs it, and its row asks to sign in.
    assert_eq!(needs_sign_in(&app).iter().map(|m| text(m, "/id").to_string()).collect::<Vec<_>>(), ["flux-3-klein"]);
    assert_eq!(access_of(&app, "flux-3-klein"), "signIn");
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
    assert_eq!(access_of(&app, "flux-3-klein"), "licence");
    assert_eq!(text(app.ui.ai_setup.hf.as_ref().unwrap(), "/user"), "pixel-painter");
    draw(&mut app, &c);
    // The licence is accepted in the browser; back in the app, it is asked again by itself.
    *accepted.lock().unwrap() = demo::TOKEN.into();
    draw_focused(&mut app, &c, false);
    assert_eq!(access_of(&app, "flux-3-klein"), "licence", "not while away");
    draw_focused(&mut app, &c, true);
    assert_eq!(access_of(&app, "flux-3-klein"), "ready");
    // Install, from the question: everything installs, FLUX.3 Klein too.
    sign_in_answered(&mut app, widgets::ButtonRole::Default);
    assert!(app.ui.ai_setup.ask.is_none() && app.ui.ai_setup.page == Page::Install, "{}", app.ui.ai_setup.note);
    finish(&mut app, &c);
    let result = app.ui.ai_setup.status.as_ref().unwrap()["result"].clone();
    assert_eq!(result["ok"], true, "{result}");
    assert!(dir.path().join("models").join("diffusion_models").join("flux3_image_klein_base_int8_convrot.safetensors").is_file());
    // Nothing the wizard shows or reports carries a token.
    let state = serde_json::to_string(&app.ui).unwrap();
    assert!(!state.contains(demo::TOKEN) && !state.contains("hf_demo_refresh"));
    // Signing out forgets it, and saves that.
    app.ui.ai_setup.page = Page::Models;
    sign_out(&mut app);
    assert!(!hf_flag(&app, "signedIn") && !saved.lock().unwrap().contains(demo::TOKEN));
}

#[test]
fn a_moved_file_or_a_refused_sign_in_is_said_as_it_is() {
    let (mut app, _dir, accepted, saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    // Signed in, and the maker moved the file: that model can't come. The sign-in stays signed in,
    // nothing reads as a sign-in error, and the question offers to install without it.
    *accepted.lock().unwrap() = "moved".into();
    demo::sign_in(&mut app);
    refresh_hf(&mut app);
    check_access(&mut app);
    draw(&mut app, &c);
    assert_eq!(access_of(&app, "flux-3-klein"), "unavailable");
    assert!(hf_flag(&app, "signedIn") && app.ui.ai_setup.note.is_empty(), "{}", app.ui.ai_setup.note);
    assert!(detail_of(&app, "flux-3-klein").contains("newer version"));
    install_clicked(&mut app);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::SignIn));
    draw(&mut app, &c);
    sign_in_answered(&mut app, widgets::ButtonRole::Default);
    assert!(app.ui.ai_setup.ask.is_some() && !running(&app), "Install stays off");
    sign_in_answered(&mut app, widgets::ButtonRole::Alternate);
    assert_eq!(app.ui.ai_setup.models, ["qwen-image-2-1"]);
    // A sign-in Hugging Face refuses (disconnected in Connected apps): not "Signed in as" any
    // more. Forgotten, saved so, and the panel says why.
    let (mut app, _dir, _accepted, saved2) = gated_app();
    draw(&mut app, &c);
    demo::sign_in(&mut app);
    app.session.ai.set_hf(&photocraft_aisetup::hf::Credentials { access: "revoked-token".into(), ..app.session.ai.hf() });
    refresh_hf(&mut app);
    assert!(hf_flag(&app, "signedIn"));
    check_access(&mut app);
    draw(&mut app, &c);
    assert!(!hf_flag(&app, "signedIn"));
    assert_eq!(access_of(&app, "flux-3-klein"), "signIn");
    assert!(text(app.ui.ai_setup.access.as_ref().unwrap(), "/note").contains("no longer accepts"));
    assert!(!saved2.lock().unwrap().contains("revoked-token"), "the keys file forgets it");
    assert!(!saved.lock().unwrap().contains("revoked-token"));
}

#[test]
fn install_without_leaves_the_gated_model_out() {
    let (mut app, dir, _accepted, _saved) = gated_app();
    let c = ctx();
    draw(&mut app, &c);
    install_clicked(&mut app);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::SignIn));
    sign_in_answered(&mut app, widgets::ButtonRole::Alternate);
    assert!(app.ui.ai_setup.ask.is_none() && !app.ui.ai_setup.models.contains(&"flux-3-klein".to_string()));
    assert_eq!(app.ui.ai_setup.models, ["qwen-image-2-1"]);
    finish(&mut app, &c);
    assert_eq!(app.ui.ai_setup.status.as_ref().unwrap()["result"]["ok"], true);
    assert!(!dir.path().join("models").join("diffusion_models").join("flux3_image_klein_base_int8_convrot.safetensors").exists());
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
    assert_eq!(access_of(&app, "flux-3-klein"), "ready");
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

#[test]
fn the_install_rows_say_what_is_being_done_not_what_to_do() {
    let row = |key: &str, label: &str, state: &str, note: &str| step_words(&json!({"key": key, "label": label, "state": state, "note": note}));
    // The last step, from waiting to done: the engine's own words while it works.
    assert_eq!(row("verify", "Start the engine and render a test image", "pending", ""), "Engine start and a test image");
    assert_eq!(row("verify", "", "active", ""), "Starting the engine\u{2026}");
    assert_eq!(row("verify", "", "active", "Starting ComfyUI\u{2026}"), "Starting ComfyUI\u{2026}");
    // A running clock says it is busy: no dots after it.
    assert_eq!(row("verify", "", "active", "Starting ComfyUI: loading PyTorch \u{b7} 3:12"), "Starting ComfyUI: loading PyTorch \u{b7} 3:12");
    assert_eq!(row("verify", "", "active", "Generating with X: step 7 of 20 \u{b7} 1:02:03"), "Generating with X: step 7 of 20 \u{b7} 1:02:03");
    assert_eq!(row("verify", "", "active", "Rendering a test image with Qwen Image 2.1"), "Rendering a test image with Qwen Image 2.1\u{2026}");
    assert_eq!(row("verify", "", "done", ""), "Engine started, test image rendered");
    assert_eq!(row("verify", "", "done", "The test image did not render"), "Engine started \u{b7} The test image did not render");
    // Downloads, and the steps between.
    assert_eq!(row("file:vae/v.safetensors", "v.safetensors", "active", ""), "Downloading v.safetensors\u{2026}");
    assert_eq!(row("file:vae/v.safetensors", "v.safetensors", "active", "Checking what arrived"), "Checking what arrived: v.safetensors\u{2026}");
    assert_eq!(row("file:vae/v.safetensors", "v.safetensors", "skipped", "needs a Hugging Face sign-in"), "v.safetensors \u{b7} needs a Hugging Face sign-in");
    assert_eq!(row("file:vae/v.safetensors", "v.safetensors", "done", ""), "v.safetensors");
    assert_eq!(row("unpack", "Unpack the engine", "active", ""), "Unpacking the engine\u{2026}");
    assert_eq!(row("paths", "Point the engine at your models", "pending", ""), "The engine's model folders");
    assert_eq!(row("configure", "Set up the models", "done", ""), "Models set up");
    // Anything else falls back to its own label.
    assert_eq!(row("odd", "Something", "pending", ""), "Something");
}

#[test]
fn too_little_room_for_the_models_does_not_stop_the_location_page() {
    let (mut app, _dir) = app(demo::Scale::Small);
    let c = ctx();
    open(&mut app);
    app.ui.ai_setup.page = Page::Location;
    draw(&mut app, &c);
    assert_eq!(location_blocked(&app), None);
    // A small drive: the recommended models don't fit, but fewer can be ticked on the next page.
    let short = "Not enough room: 40.0 GB is free, and this needs 42.6 GB. Choose fewer models or another folder.";
    let plan = app.ui.ai_setup.plan.as_mut().unwrap();
    (plan["problems"], plan["modelProblems"]) = (json!([short]), json!([short]));
    draw(&mut app, &c);
    assert_eq!(location_blocked(&app), None);
    // The engine alone does not fit: only another folder helps.
    let plan = app.ui.ai_setup.plan.as_mut().unwrap();
    (plan["folderProblems"], plan["modelProblems"]) = (json!(["Not enough room for the engine"]), json!([]));
    draw(&mut app, &c);
    assert_eq!(location_blocked(&app), Some("Choose folders that have the room"));
    // No plan yet, or the user's own ComfyUI not connected: wait.
    app.ui.ai_setup.plan = None;
    assert!(location_blocked(&app).is_some());
    app.ui.ai_setup.existing = true;
    assert_eq!(location_blocked(&app), Some("Connect to your ComfyUI first"));
}

#[test]
fn a_setup_that_stops_with_an_error_shows_all_of_it_in_a_window_to_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    let c = ctx();
    // The user's own ComfyUI doesn't answer: the setup stops at its last step, with the wizard open.
    demo::fail(&mut app, dir.path(), None);
    draw(&mut app, &c);
    let status = app.ui.ai_setup.status.clone().unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(app.ui.ai_setup.page, Page::Install);
    let verify = status["progress"]["steps"].as_array().unwrap().iter().find(|s| s["key"] == "verify").cloned().unwrap();
    assert_eq!(verify["state"], "failed", "the row keeps its red \u{d7}");
    // The window opened by itself, with the whole of what went wrong.
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::Error));
    let said = failure(&app);
    assert!(said.contains("not answering"), "{said}");
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::Error), "it stays open until it is closed");
    // Copy puts all of it on the clipboard, and keeps the window open.
    error_answered(&mut app, &c, widgets::ButtonRole::Apply);
    let copied = c.output(|o| o.commands.clone());
    assert!(copied.contains(&egui::OutputCommand::CopyText(said.clone())), "{copied:?}");
    assert_eq!(app.ui.ai_setup.ask, Some(Ask::Error));
    // Close closes it, and it doesn't open again by itself.
    error_answered(&mut app, &c, widgets::ButtonRole::Cancel);
    draw(&mut app, &c);
    assert_eq!(app.ui.ai_setup.ask, None);
    // A long one (a Python traceback) is kept whole, however little room the screen has.
    let other = tempfile::tempdir().unwrap();
    let mut long = PhotocraftApp::new(Session::new(), crate::Services::default());
    demo::fail(&mut long, other.path(), Some(demo::TRACEBACK));
    draw(&mut long, &c);
    assert_eq!(failure(&long), demo::TRACEBACK);
}

/// The first look at this PC is its steps in the middle of the screen, ticked as the job reports
/// each, with no Next under them; a setup that is there already keeps its rows while it looks.
#[test]
fn the_first_look_at_this_pc_shows_its_steps() {
    let (mut app, dir0) = app_without_probe();
    // A look that is under way (its job stands in here: no test asks this PC what it has).
    app.ui.ai_setup.probe_job = Some(u64::MAX);
    open(&mut app);
    assert!(first_look(&app) && !offer_choice(&app));
    assert_eq!(probe_stage(&app), Stage::Hardware, "a job that has said nothing yet is on its first step");
    for step in Stage::ALL {
        assert_eq!(stage_of(step.message()), step);
    }
    assert_eq!(stage_of("something else"), Stage::Hardware);
    let c = ctx();
    for _ in 0..3 {
        c.run_ui(Default::default(), |ui| page_checking(&app, ui)).textures_delta.clear();
    }
    // Answered: the rows and the two buttons.
    app.ui.ai_setup.probe_job = None;
    app.ui.ai_setup.probe = Some(demo::probe(dir0.path()));
    assert!(!first_look(&app) && offer_choice(&app));
    // Set up already: Check Again looks with the setup's own rows in place, not the steps.
    let (mut installed, dir) = self::app(demo::Scale::Small);
    demo::pretend_installed(&installed, dir.path()).unwrap();
    (installed.ui.ai_setup.probe, installed.ui.ai_setup.probe_job) = (None, Some(u64::MAX));
    open(&mut installed);
    assert!(!first_look(&installed));
}
