use super::*;
use photocraft_aigen::{Request, Response, Settings, Transport};
use photocraft_engine::Session;
use std::sync::{Arc, Mutex};

/// A ComfyUI that lists two model files per folder and renders a green square.
struct Fake;

impl Transport for Fake {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        let body = if r.url.ends_with("/prompt") {
            br#"{"prompt_id":"p1"}"#.to_vec()
        } else if r.url.contains("/history/") {
            br#"{"p1":{"outputs":{"9":{"images":[{"filename":"o.png","subfolder":"","type":"temp"}]}},"status":{"completed":true}}}"#.to_vec()
        } else if r.url.contains("/view?") {
            let img = photocraft_codecs::Image::from_u8(8, 8, photocraft_codecs::ChannelLayout::Rgba, [0u8, 255, 0, 255].repeat(64)).unwrap();
            photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap()
        } else if r.url.ends_with("/upload/image") {
            br#"{"name":"uploaded.png","subfolder":"","type":"input"}"#.to_vec()
        } else if r.url.ends_with("/models/diffusion_models") {
            br#"["z_image_turbo_bf16.safetensors","flux1-dev.safetensors","qwen_image_edit_2511_bf16.safetensors"]"#.to_vec()
        } else if r.url.contains("/models/") {
            br#"["ae.safetensors","qwen_3_4b.safetensors"]"#.to_vec()
        } else {
            return Ok(Response { status: 404, body: Vec::new() });
        };
        Ok(Response { status: 200, body })
    }
}

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.ai.transport = Some(Arc::new(Fake));
    app
}

fn add_model(app: &mut PhotocraftApp, model: Model) {
    let mut models = app.session.prefs().integrations.ai.models.clone();
    models.push(model);
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": models})).unwrap();
}

fn z_image(id: &str) -> Model {
    Model { id: id.into(), ..Family::ZImage.preset() }
}

/// Draw the panel for a few frames (fonts load on the first).
fn draw_panel(app: &mut PhotocraftApp) {
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| panel(app, ui, true)).textures_delta.clear();
        ctx.run_ui(Default::default(), |ui| panel(app, ui, false)).textures_delta.clear();
    }
}

/// Draw the Integrations section over the dialog's draft for a few frames.
fn draw_section(f: &mut Map<String, Value>) {
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    let before = f.get(PAGE).cloned();
    for page in PAGES.iter().map(|(key, _)| *key).cycle().take(PAGES.len() * 3) {
        f.insert(PAGE.into(), json!(page));
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            let obj = values.get_mut("integrations").and_then(Value::as_object_mut).unwrap();
            prefs_section(ui, obj, f);
            f.insert("values".into(), values);
        })
        .textures_delta
        .clear();
    }
    // Leave the dialog on the page it was on (drawing the local page may ask to read the server).
    match before {
        Some(page) => f.insert(PAGE.into(), page),
        None => f.remove(PAGE),
    };
    f.remove(ACTION);
}

fn dialog_fields(app: &mut PhotocraftApp) -> (u64, Map<String, Value>) {
    let id = open_settings(app, "");
    (id, app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone())
}

#[test]
fn the_window_menu_item_shows_the_tab_and_reports_it() {
    let mut app = app();
    let ctx = egui::Context::default();
    assert!(crate::menus::is_live(PANEL));
    assert!(crate::menu_catalog::CATALOG.iter().any(|m| m.3 == PANEL && m.1 == "AI Gen" && m.0 == ["Window"]));
    app.ui.panels.properties = false;
    assert_eq!(checked(&app, PANEL), Some(false));
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert!(app.ui.panels.properties);
    assert_eq!(app.ui.dock_tabs.properties, TAB);
    assert_eq!(checked(&app, PANEL), Some(true));
    // Another tab of the group in front: the item is unchecked, and choosing it brings AI Gen back.
    app.ui.dock_tabs.properties = 0;
    assert_eq!(checked(&app, PANEL), Some(false));
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert_eq!((app.ui.panels.properties, app.ui.dock_tabs.properties), (true, TAB));
    // A collapsed group is expanded.
    app.ui.dock.set_collapsed(crate::dock::Group::Properties, true);
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert!(!app.ui.dock.is_collapsed(crate::dock::Group::Properties));
    // Showing: the item hides the group, like the other Window › <panel> items.
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert!(!app.ui.panels.properties);
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({"show": true})).unwrap();
    assert!(app.ui.panels.properties);
    assert_eq!(crate::dock::Group::Properties.tabs(true).get(TAB), Some(&"AI Gen"));
}

#[test]
fn themes_that_float_properties_get_a_floating_panel() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.ui.theme = crate::theme::ThemeKind::Studio;
    let before = (app.ui.panels.properties, app.ui.dock_tabs.properties);
    assert_eq!(checked(&app, PANEL), Some(false));
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert!(app.ui.ai_gen.floating);
    assert_eq!(checked(&app, PANEL), Some(true));
    // The dock is left alone.
    assert_eq!((app.ui.panels.properties, app.ui.dock_tabs.properties), before);
    PhotocraftApp::setup_context(&ctx, crate::theme::ThemeKind::Studio);
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| window(&mut app, ui.ctx())).textures_delta.clear();
    }
    crate::menus::invoke(&mut app, &ctx, PANEL, json!({})).unwrap();
    assert!(!app.ui.ai_gen.floating);
}

#[test]
fn the_panel_draws_in_every_state() {
    // No models, no document.
    let mut app = app();
    draw_panel(&mut app);
    // Models but no document; a stale model id falls back to the first enabled model.
    add_model(&mut app, Model { enabled: false, ..z_image("off") });
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 1.5", "gpt-image-1.5", true) });
    add_model(&mut app, Model { id: "nb".into(), ..Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false) });
    app.ui.ai_gen.model = "gone".into();
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.model, "z");
    // With a document and a selection, for a local, an OpenAI and a Gemini model.
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 10})).unwrap();
    for id in ["z", "gpt", "nb"] {
        app.ui.ai_gen.model = id.into();
        app.ui.ai_gen.alpha = true;
        draw_panel(&mut app);
        // The choice is remembered across models; one without alpha just doesn't use it.
        assert!(app.ui.ai_gen.alpha, "{id}");
    }
    assert_eq!(target_size(&app), Some((20, 10, true)));
    assert_eq!(render_size(&z_image("z"), 500, 500), (1008, 1008));
}

#[test]
fn openai_models_show_an_estimate_and_the_last_billed_cost() {
    let mut app = app();
    app.run("file.new", json!({"width": 600, "height": 600})).unwrap();
    let flare = Model { quality: "medium".into(), ..Model::cloud(Provider::Openai, "GPT Image 2.5 Flare", "gpt-image-2.5-flare", true) };
    let (text, tip) = price_note(&app, &flare);
    assert_eq!(text, "Est. cost ≈ $0.041–$0.053 (medium)");
    assert!(tip.contains("GPT Image 2.5"));
    // A published size of a model with published prices: exact, no ≈.
    let old = Model { quality: "high".into(), ..Model::cloud(Provider::Openai, "GPT Image 1.5", "gpt-image-1.5", true) };
    assert_eq!(price_note(&app, &old).0, "Est. cost $0.133 (high)");
    // A quality without published figures says so instead of guessing.
    let max = Model { quality: "max".into(), ..flare.clone() };
    assert!(price_note(&app, &max).0.starts_with("No published price; $30 / 1M output tokens"));
    // The billed cost of the last generation follows it; a result without one clears it.
    finished(&mut app, &json!({"layer": 1, "cost": 0.0534}));
    assert!(price_note(&app, &flare).0.ends_with("· last $0.053"));
    for junk in [json!({"cost": null}), json!({"cost": "free"}), json!({"cost": -1.0}), json!(null)] {
        finished(&mut app, &junk);
        assert_eq!(app.ui.ai_gen.last_cost, None, "{junk}");
    }
    assert!(crate::dock::Group::Properties.scrolls_itself(TAB));
}

#[test]
fn generate_is_blocked_until_it_can_run() {
    let mut app = app();
    let z = z_image("z");
    add_model(&mut app, z.clone());
    assert!(generate_blocked(&app, &z).unwrap().contains("Open a document"));
    app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
    assert!(generate_blocked(&app, &z).unwrap().contains("prompt"));
    app.ui.ai_gen.prompt = "a green field".into();
    assert_eq!(generate_blocked(&app, &z), None);
    let cloud = Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false);
    if app.session.ai.key(Provider::Openai).is_empty() {
        assert!(generate_blocked(&app, &cloud).unwrap().contains("API key"));
    }
    app.session.ai.transport = None;
    assert!(generate_blocked(&app, &z).unwrap().contains("desktop app"));
}

#[test]
fn generating_from_the_panel_adds_a_layer() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
    app.ui.ai_gen.model = "z".into();
    app.ui.ai_gen.prompt = "a green field".into();
    let before = app.session.active().unwrap().doc.layers.len();
    let r = generate(&mut app).unwrap();
    assert_eq!(r["model"], "z");
    assert_eq!(app.session.active().unwrap().doc.layers.len(), before + 1);
    // The panel's state is readable and drivable like the rest of the UI state.
    let state = serde_json::to_value(&app.ui).unwrap();
    assert_eq!(state["ai_gen"]["prompt"], "a green field");
    let old: AiGenUi = serde_json::from_value(json!({"prompt": "x", "unknown": 1})).unwrap();
    assert_eq!(old, AiGenUi { prompt: "x".into(), ..Default::default() });
}

#[test]
fn the_settings_page_edits_the_draft() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    let (_, mut f) = dialog_fields(&mut app);
    assert_eq!(f.get("section"), Some(&json!("integrations")));
    draw_section(&mut f);
    // Drawing changes nothing.
    assert_eq!(f["values"]["integrations"]["ai"], serde_json::to_value(&app.session.prefs().integrations.ai).unwrap());
    // Every family and cloud preset can be added, drawn and saved.
    for family in Family::ALL {
        let mut ai = f["values"]["integrations"]["ai"].as_object().cloned().unwrap();
        push_model(&mut ai, &mut f, family.preset());
        f["values"]["integrations"]["ai"] = Value::Object(ai);
        draw_section(&mut f);
    }
    for m in Model::cloud_presets() {
        let mut ai = f["values"]["integrations"]["ai"].as_object().cloned().unwrap();
        push_model(&mut ai, &mut f, m);
        f["values"]["integrations"]["ai"] = Value::Object(ai);
        draw_section(&mut f);
    }
    crate::prefs_ui::confirm(&mut app, &f).unwrap();
    let models = &app.session.prefs().integrations.ai.models;
    assert_eq!(models.len(), 1 + Family::ALL.len() + Model::cloud_presets().len());
    let mut ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), models.len(), "ids are unique");
    // A hostile draft is drawn without a panic.
    for junk in [json!(null), json!([]), json!({"models": 7}), json!({"models": [1, "a", {"provider": "nope", "family": 3, "steps": "x"}], "comfyUrl": 9})] {
        let mut f = f.clone();
        f["values"]["integrations"]["ai"] = junk;
        f.insert(SELECTED.into(), json!(99));
        draw_section(&mut f);
    }
}

#[test]
fn the_gear_opens_the_models_own_page_and_each_page_selects_its_own_kind() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2.5 Flare", "gpt-image-2.5-flare", true) });
    add_model(&mut app, z_image("z2"));
    let fields = |app: &mut PhotocraftApp, model: &str| {
        let id = open_settings(app, model);
        app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone()
    };
    let f = fields(&mut app, "gpt");
    assert_eq!((f[PAGE].as_str(), f[SELECTED].as_u64()), (Some("cloud"), Some(1)));
    let mut f = fields(&mut app, "z2");
    assert_eq!((f[PAGE].as_str(), f[SELECTED].as_u64()), (Some("local"), Some(2)));
    // The cloud page doesn't show a local model as selected: it takes its first model.
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    f.insert(PAGE.into(), json!("cloud"));
    for _ in 0..2 {
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f);
            f.insert("values".into(), values);
        })
        .textures_delta
        .clear();
    }
    assert_eq!(f[SELECTED], 1);
    // The local page reads a server on this machine once, without being asked.
    let mut f = fields(&mut app, "z");
    f.remove(ACTION);
    ctx.run_ui(Default::default(), |ui| {
        let mut values = f.get("values").cloned().unwrap();
        prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f);
    })
    .textures_delta
    .clear();
    assert_eq!(f.get(ACTION), Some(&json!("peek")));
    after_body(&mut app, &mut f);
    assert!(f.get(FILES).is_some() && f.get(ACTION).is_none());
    // An unknown page falls back to the first.
    f.insert(PAGE.into(), json!("nope"));
    draw_section(&mut f);
}

/// A launcher that records what it was asked and "starts" at once.
#[derive(Default)]
struct FakeServer {
    started: std::sync::atomic::AtomicBool,
    ensured: Mutex<Vec<bool>>,
}

impl photocraft_aigen::LocalServer for FakeServer {
    fn ensure(&self, _: &dyn Transport, settings: &Settings, _: &photocraft_aigen::Control<'_>) -> photocraft_aigen::Result<()> {
        self.ensured.lock().unwrap().push(settings.comfy_autostart);
        if settings.comfy_autostart {
            self.started.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }
    fn stop(&self) -> bool {
        self.started.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
    fn state(&self) -> photocraft_aigen::ServerState {
        if self.started.load(std::sync::atomic::Ordering::Relaxed) { photocraft_aigen::ServerState::Started } else { photocraft_aigen::ServerState::NotStarted }
    }
    fn find(&self, _: &dyn Transport, _: &str) -> photocraft_aigen::Result<photocraft_aigen::Launch> {
        Ok(photocraft_aigen::Launch { python: "C:/comfy/.venv/Scripts/python.exe".into(), folder: "C:/comfy".into(), args: "main.py --enable-manager".into() })
    }
}

#[test]
fn the_server_page_finds_starts_and_stops_comfyui() {
    let mut app = app();
    let server = Arc::new(FakeServer::default());
    app.session.ai.server = Some(server.clone());
    let (_, mut f) = dialog_fields(&mut app);
    // Find Installation fills the launch settings in and switches starting on.
    f.insert(ACTION.into(), json!("find"));
    after_body(&mut app, &mut f);
    let ai = &f["values"]["integrations"]["ai"];
    assert_eq!(ai["comfyLaunch"]["python"], "C:/comfy/.venv/Scripts/python.exe");
    assert_eq!(ai["comfyLaunch"]["args"], "main.py --enable-manager");
    assert_eq!(ai["comfyAutostart"], true);
    // The result is reported under the button that was pressed.
    assert_eq!(f[LAUNCH_STATUS]["error"], false);
    assert!(f[LAUNCH_STATUS]["text"].as_str().unwrap().contains("Press OK"));
    draw_section(&mut f);
    // Opening the Local Models page only looks: it must not start a server, or even ask to.
    let asked = server.ensured.lock().unwrap().len();
    f.insert(ACTION.into(), json!("peek"));
    after_body(&mut app, &mut f);
    assert_eq!(server.ensured.lock().unwrap().len(), asked);
    assert_eq!(f[STARTED], false);
    // With no server there, that look leaves a note about what will happen, not an error.
    let up = app.session.ai.transport.take();
    f.insert(ACTION.into(), json!("peek"));
    after_body(&mut app, &mut f);
    assert_eq!(f[STATUS]["error"], false);
    assert!(f[STATUS]["text"].as_str().unwrap().contains("PhotoCraft will start it"), "{}", f[STATUS]);
    f["values"]["integrations"]["ai"]["comfyAutostart"] = json!(false);
    f.insert(ACTION.into(), json!("peek"));
    after_body(&mut app, &mut f);
    assert!(f[STATUS]["text"].as_str().unwrap().contains("Local Server page"), "{}", f[STATUS]);
    // A long status wraps inside the page instead of running off it.
    f.insert(STATUS.into(), json!({"error": true, "text": "word ".repeat(120)}));
    draw_section(&mut f);
    f["values"]["integrations"]["ai"]["comfyAutostart"] = json!(true);
    app.session.ai.transport = up;
    // Connect starts it (the draft says so), and says who did.
    f.insert(ACTION.into(), json!("connect"));
    after_body(&mut app, &mut f);
    assert_eq!(server.ensured.lock().unwrap().last(), Some(&true));
    assert!(f[STATUS]["text"].as_str().unwrap().contains("started by PhotoCraft"), "{}", f[STATUS]);
    after_body(&mut app, &mut f);
    assert_eq!(f[STARTED], true);
    draw_section(&mut f);
    // Stop Server stops ours; a second press has nothing to stop and says so.
    f.insert(ACTION.into(), json!("stop"));
    after_body(&mut app, &mut f);
    assert_eq!((f[STARTED].clone(), f[LAUNCH_STATUS]["error"].clone()), (json!(false), json!(false)));
    f.insert(ACTION.into(), json!("stop"));
    after_body(&mut app, &mut f);
    assert_eq!(f[LAUNCH_STATUS]["error"], true);
    // The launch settings are saved with OK and survive a round trip.
    crate::prefs_ui::confirm(&mut app, &f).unwrap();
    let saved = &app.session.prefs().integrations.ai;
    assert!(saved.comfy_autostart);
    assert_eq!(saved.comfy_launch.folder, "C:/comfy");
    // Without a launcher (web, tests) Find says so instead of failing oddly.
    app.session.ai.server = None;
    f.insert(ACTION.into(), json!("find"));
    after_body(&mut app, &mut f);
    assert!(f[LAUNCH_STATUS]["text"].as_str().unwrap().contains("desktop app"));
    draw_section(&mut f);
}

#[test]
fn scanning_repairs_a_model_set_up_as_the_wrong_architecture() {
    /// A server with the Qwen Image 2.1 files.
    struct Qwen;
    impl Transport for Qwen {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            let body: &[u8] = if r.url.ends_with("/models/diffusion_models") {
                br#"["qwen_image_2.1_int8_convrot.safetensors"]"#
            } else if r.url.ends_with("/models/text_encoders") {
                br#"["qwen_2.5_vl_7b_fp8_scaled.safetensors","qwen3vl_8b_int8_convrot.safetensors"]"#
            } else if r.url.ends_with("/models/vae") {
                br#"["qwen_image_vae.safetensors","qwen_image_2.1_vae_bf16.safetensors"]"#
            } else {
                b"[]"
            };
            Ok(Response { status: 200, body: body.to_vec() })
        }
    }
    let mut app = app();
    app.session.ai.transport = Some(Arc::new(Qwen));
    // As an older scan left it, with the user's own name and seed.
    add_model(
        &mut app,
        Model { id: "q".into(), name: "my qwen".into(), seed: 7, unet: "qwen_image_2.1_int8_convrot.safetensors".into(), ..Family::QwenImage.preset() },
    );
    // Generating with it says what is wrong instead of failing in the sampler.
    app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
    let e = app.run("ai.generate", json!({"prompt": "x", "model": "q"})).unwrap_err();
    assert!(e.contains("Qwen Image 2.1 model") && e.contains("Scan Server"), "{e}");
    // The panel says so under a disabled Generate, before anything is started.
    app.ui.ai_gen.prompt = "x".into();
    let broken = app.session.prefs().integrations.ai.models[0].clone();
    assert!(generate_blocked(&app, &broken).unwrap().contains("Scan Server"));
    // Scan Server fixes that entry in place: no second one beside it.
    let (_, mut f) = dialog_fields(&mut app);
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    let models = f["values"]["integrations"]["ai"]["models"].as_array().unwrap().clone();
    assert_eq!(models.len(), 1, "{models:?}");
    assert_eq!((models[0]["id"].as_str(), models[0]["name"].as_str(), models[0]["seed"].as_i64()), (Some("q"), Some("my qwen"), Some(7)));
    assert_eq!((models[0]["family"].as_str(), models[0]["clip"].as_str()), (Some("qwenImage21"), Some("qwen3vl_8b_int8_convrot.safetensors")));
    assert_eq!(models[0]["vae"], "qwen_image_2.1_vae_bf16.safetensors");
    assert!(f[STATUS]["text"].as_str().unwrap().contains("1 repaired"), "{}", f[STATUS]);
    // A second scan has nothing left to do.
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 1);
    assert!(!f[STATUS]["text"].as_str().unwrap().contains("repaired"));
}

#[test]
fn the_page_divides_whatever_width_the_dialog_gives_it() {
    assert_eq!(page_widths(540.0), (180.0, 200.0));
    let (list, field) = page_widths(830.0);
    assert!(list > 250.0 && field > 380.0 && list + field + 160.0 <= 830.0, "{list} {field}");
    for odd in [0.0, -5.0, f32::NAN, f32::INFINITY, 1e9] {
        let (list, field) = page_widths(odd);
        assert!((180.0..=280.0).contains(&list) && (200.0..=420.0).contains(&field), "{odd}: {list} {field}");
    }
}

#[test]
fn browse_fills_the_launch_settings_from_the_chosen_file() {
    let mut app = app();
    let (_, mut f) = dialog_fields(&mut app);
    // Without a file dialog (tests, the web) Browse says so instead of doing nothing.
    f.insert(ACTION.into(), json!("browseScript"));
    after_body(&mut app, &mut f);
    assert_eq!(f[LAUNCH_STATUS]["error"], true);
    // A chosen main.py sets the folder and the script, and asks for the other half.
    picked(&mut f, "C:/nowhere/ComfyUI/main.py");
    let launch = f["values"]["integrations"]["ai"]["comfyLaunch"].clone();
    assert_eq!(launch["args"], "main.py");
    assert!(launch["folder"].as_str().unwrap().replace('\\', "/").ends_with("nowhere/ComfyUI"));
    assert!(f[LAUNCH_STATUS]["text"].as_str().unwrap().contains("Python"));
    // Then the Python: both set, nothing else changed.
    picked(&mut f, "C:/nowhere/ComfyUI/.venv/Scripts/python.exe");
    let launch = f["values"]["integrations"]["ai"]["comfyLaunch"].clone();
    assert!(launch["python"].as_str().unwrap().ends_with("python.exe"));
    assert_eq!(launch["args"], "main.py");
    assert_eq!(f[LAUNCH_STATUS]["error"], false);
    assert!(f[LAUNCH_STATUS]["text"].as_str().unwrap().contains("Python and folder are set"));
    draw_section(&mut f);
    // With a dialog service the request asks for the right kind of file.
    let asked: std::rc::Rc<std::cell::RefCell<Vec<crate::file_dialog::FileDialogRequest>>> = Default::default();
    let log = asked.clone();
    app.services.file_dialog = Some(Box::new(move |request, _, _| log.borrow_mut().push(request)));
    f.insert(ACTION.into(), json!("browsePython"));
    after_body(&mut app, &mut f);
    let ctx = egui::Context::default();
    app.poll_file_dialog(&ctx, None);
    assert!(
        matches!(asked.borrow().first(), Some(crate::file_dialog::FileDialogRequest::Open { multiple: false, extensions: Some(e), .. }) if e == &["exe"]),
        "{:?}",
        asked.borrow()
    );
}

#[test]
fn a_background_connect_reports_through_the_dialog() {
    let mut app = app();
    app.background_jobs = true;
    let (_, mut f) = dialog_fields(&mut app);
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    assert!(f.get(JOB).is_some());
    draw_section(&mut f);
    // The job ends on its own thread; the dialog picks the result up when it has been applied.
    for _ in 0..400 {
        app.session.poll_jobs();
        after_body(&mut app, &mut f);
        if f.get(JOB).is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(f.get(JOB).is_none(), "the connect never finished");
    assert_eq!(f[STATUS]["error"], false, "{}", f[STATUS]);
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 2);
}

#[test]
fn connect_reads_the_servers_files_and_detect_adds_its_models() {
    let mut app = app();
    let (_, mut f) = dialog_fields(&mut app);
    f.insert(ACTION.into(), json!("connect"));
    after_body(&mut app, &mut f);
    assert_eq!(f[STATUS]["error"], false, "{}", f[STATUS]);
    assert_eq!(f[FILES]["vae"], json!(["ae.safetensors", "qwen_3_4b.safetensors"]));
    assert!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().is_empty());
    draw_section(&mut f);
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    let models = f["values"]["integrations"]["ai"]["models"].as_array().unwrap().clone();
    // The edit model is not a text-to-image model.
    assert_eq!(models.len(), 2, "{models:?}");
    assert_eq!(models[0]["unet"], "z_image_turbo_bf16.safetensors");
    assert_eq!(models[0]["clip"], "qwen_3_4b.safetensors");
    // Detecting again adds nothing twice.
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 2);
    draw_section(&mut f);
    // A server that isn't there is reported in the dialog, not thrown.
    app.session.ai.transport = None;
    f.insert(ACTION.into(), json!("connect"));
    after_body(&mut app, &mut f);
    assert_eq!(f[STATUS]["error"], true);
    draw_section(&mut f);
}

#[test]
fn api_keys_are_saved_apart_from_the_preferences() {
    let keys: Arc<Mutex<Option<String>>> = Arc::default();
    let prefs: Arc<Mutex<Option<String>>> = Arc::default();
    let (k1, k2, p1) = (keys.clone(), keys.clone(), prefs.clone());
    let services = crate::Services {
        load_ai_keys: Some(Box::new(move || k1.lock().unwrap().clone())),
        save_ai_keys: Some(Box::new(move |s: &str| {
            *k2.lock().unwrap() = Some(s.to_string());
            Ok(())
        })),
        save_prefs: Some(Box::new(move |s: &str| {
            *p1.lock().unwrap() = Some(s.to_string());
            Ok(())
        })),
        ..Default::default()
    };
    let mut app = PhotocraftApp::new(Session::new(), services);
    let (id, mut f) = dialog_fields(&mut app);
    f[KEYS]["gemini"] = json!("g-secret-1");
    draw_section(&mut f);
    crate::prefs_ui::confirm(&mut app, &f).unwrap();
    assert_eq!(app.session.ai.key(Provider::Gemini), "g-secret-1");
    assert!(keys.lock().unwrap().as_deref().unwrap().contains("g-secret-1"));
    assert!(!app.session.prefs_to_json().contains("g-secret"));
    // The open dialog never holds a saved key, only that there is one.
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields = f.clone();
    }
    let (_, reopened) = dialog_fields(&mut app);
    assert_eq!(reopened[KEYS]["geminiSaved"], true);
    assert!(!Value::Object(reopened.clone()).to_string().contains("g-secret"));
    // A new launch restores it from the keys file.
    let k3 = keys.clone();
    let mut again =
        PhotocraftApp::new(Session::new(), crate::Services { load_ai_keys: Some(Box::new(move || k3.lock().unwrap().clone())), ..Default::default() });
    load_keys(&mut again);
    assert_eq!(again.session.ai.key(Provider::Gemini), "g-secret-1");
    // Remove.
    let mut f = reopened;
    f[KEYS]["geminiRemove"] = json!(true);
    crate::prefs_ui::confirm(&mut app, &f).unwrap();
    assert!(!keys.lock().unwrap().as_deref().unwrap().contains("g-secret"));
}

#[test]
fn a_long_reason_under_generate_fits_in_the_panel() {
    let mut app = app();
    // A model whose set-up can't run: the reason Generate is off is several lines long.
    add_model(
        &mut app,
        Model {
            id: "q".into(),
            name: "qwen image 2.1 int8 convrot".into(),
            unet: "qwen_image_2.1_int8_convrot.safetensors".into(),
            ..Family::QwenImage.preset()
        },
    );
    app.run("file.new", json!({"width": 64, "height": 64})).unwrap();
    app.ui.ai_gen.model = "q".into();
    app.ui.ai_gen.prompt = "a long prompt\n".repeat(60);
    let broken = app.session.prefs().integrations.ai.models[0].clone();
    assert!(generate_blocked(&app, &broken).unwrap().len() > 150);
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    // Panels of several sizes, down to a short narrow one: the notes always end inside.
    for (w, h) in [(300.0, 420.0), (260.0, 300.0), (420.0, 700.0)] {
        let mut overflow = 0.0f32;
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |ui| {
                let area = egui::Rect::from_min_size(ui.max_rect().min, vec2(w, h));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
                // The content itself, as the dock group lays it out (there it scrolls only when
                // even a two-line prompt box doesn't fit).
                panel_body(&mut app, &mut child, Some(h));
                overflow = child.min_rect().bottom() - area.bottom();
            })
            .textures_delta
            .clear();
        }
        assert!(overflow <= 1.0, "{w}x{h}: the panel's content runs {overflow} points past its bottom");
    }
    // The same on the InPaint tab, which has a row more.
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    app.ui.ai_gen.mode = Mode::Inpaint;
    for (w, h) in [(300.0, 420.0), (260.0, 340.0)] {
        let mut overflow = 0.0f32;
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |ui| {
                let area = egui::Rect::from_min_size(ui.max_rect().min, vec2(w, h));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
                panel_body(&mut app, &mut child, Some(h));
                overflow = child.min_rect().bottom() - area.bottom();
            })
            .textures_delta
            .clear();
        }
        assert!(overflow <= 1.0, "inpaint {w}x{h}: the panel's content runs {overflow} points past its bottom");
    }
}

#[test]
fn the_inpaint_tab_lists_the_models_that_can_inpaint_and_keeps_its_own_choice() {
    let mut app = app();
    // Text-to-image models are not told where a selection is: they are offered for inpainting
    // only when their box is ticked. FLUX.2 is, and so are the models made for inpainting, which
    // in turn are not offered for text to image.
    add_model(&mut app, Model { id: "ernie".into(), ..Family::Ernie.preset() });
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "ticked".into(), inpaint: Some(true), ..Family::ZImage.preset() });
    add_model(&mut app, Model { id: "flux".into(), ..Family::Flux2.preset() });
    add_model(&mut app, Model { id: "off".into(), inpaint: Some(false), ..Family::Flux2.preset() });
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    add_model(&mut app, Model { id: "qwen".into(), ..Family::QwenInpaint.preset() });
    let ids = |app: &PhotocraftApp| enabled_models(app).into_iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(&app), ["ernie", "z", "ticked", "flux", "off"]);
    app.ui.ai_gen.model = "ernie".into();
    app.ui.ai_gen.mode = Mode::Inpaint;
    assert_eq!(ids(&app), ["ticked", "flux", "fill", "qwen"]);
    draw_panel(&mut app);
    // Each tab has its own model: choosing one for inpainting leaves the other tab's alone.
    assert_eq!((app.ui.ai_gen.inpaint_model.as_str(), app.ui.ai_gen.model.as_str()), ("ticked", "ernie"));
    app.ui.ai_gen.inpaint_model = "fill".into();
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.chosen(), "fill");
    app.ui.ai_gen.mode = Mode::Txt2img;
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.chosen(), "ernie");
    // No model that can: the tab says so instead of offering a list of nothing.
    let only = vec![Model { id: "ernie".into(), ..Family::Ernie.preset() }];
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": only})).unwrap();
    app.ui.ai_gen.mode = Mode::Inpaint;
    assert!(ids(&app).is_empty());
    draw_panel(&mut app);
    // A tab the engine can't run yet (set from outside: the panel doesn't offer it) is Txt2Img.
    app.ui.ai_gen.mode = Mode::ImgEdit;
    assert!(!Mode::ImgEdit.available() && app.ui.ai_gen.mode() == Mode::Txt2img);
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.mode, Mode::Txt2img);
    // The state reads and writes like the engine names the modes.
    assert_eq!(serde_json::to_value(Mode::ALL).unwrap(), json!(["txt2img", "inpaint", "img2img", "imgEdit"]));
    assert_eq!(Mode::ALL.map(Mode::key), ["txt2img", "inpaint", "img2img", "imgEdit"]);
    let old: AiGenUi = serde_json::from_value(json!({"prompt": "x"})).unwrap();
    assert_eq!((old.mode, old.strength, old.context), (Mode::Txt2img, 1.0, 0.25));
    let hostile = AiGenUi { strength: f64::NAN, context: -7.0, ..Default::default() };
    assert_eq!((hostile.strength(), hostile.context()), (1.0, 0.0));
}

#[test]
fn inpainting_from_the_panel_needs_a_selection_on_a_pixel_layer_and_adds_a_masked_layer() {
    let mut app = app();
    let z = Model { inpaint: Some(true), ..z_image("z") };
    add_model(&mut app, z.clone());
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    app.ui.ai_gen.mode = Mode::Inpaint;
    app.ui.ai_gen.prompt = "a pond".into();
    // No selection: Generate is off and says what to do.
    assert!(generate_blocked(&app, &z).unwrap().contains("needs a selection"));
    assert_eq!(target_size(&app), Some((64, 48, false)));
    draw_panel(&mut app);
    app.run("select.rect", json!({"x": 24, "y": 16, "width": 16, "height": 16})).unwrap();
    assert_eq!(generate_blocked(&app, &z), None);
    // The model renders the selection with a quarter of its edge of context on every side.
    assert_eq!(target_size(&app), Some((24, 24, true)));
    app.ui.ai_gen.context = 0.0;
    assert_eq!(target_size(&app), Some((16, 16, true)));
    app.ui.ai_gen.context = 0.5;
    // An empty layer has nothing to repaint.
    app.run("layer.new.layer", json!({})).unwrap();
    assert!(generate_blocked(&app, &z).unwrap().contains("is empty inside the selection"));
    draw_panel(&mut app);
    app.run("edit.undo", json!({})).unwrap();
    // A custom workflow ticked for inpainting but without an image to load can't.
    let custom = Model {
        family: Family::Custom,
        name: "Mine".into(),
        inpaint: Some(true),
        workflow: r#"{"1":{"class_type":"X","inputs":{"t":"%prompt%"}}}"#.into(),
        ..Model::default()
    };
    assert!(generate_blocked(&app, &custom).unwrap().contains("%image%"));
    app.ui.ai_gen.strength = 0.6;
    let before = app.session.active().unwrap().doc.layers.len();
    draw_panel(&mut app);
    let r = generate(&mut app).unwrap();
    assert_eq!((r["mode"].as_str(), r["model"].as_str()), (Some("inpaint"), Some("z")));
    assert_eq!(r["bounds"], json!([16, 8, 32, 32]));
    let d = app.session.active().unwrap();
    assert_eq!(d.doc.layers.len(), before + 1);
    assert!(d.doc.layer(d.active_layer.unwrap()).unwrap().mask.is_some());
    // The same button on the other tab is text to image, as before.
    app.ui.ai_gen.mode = Mode::Txt2img;
    assert_eq!(generate(&mut app).unwrap()["mode"], "txt2img");
}

#[test]
fn the_inpainting_box_follows_the_architecture_until_it_is_used() {
    let mut app = app();
    add_model(&mut app, Model { id: "flux".into(), ..Family::Flux2.preset() });
    add_model(&mut app, Model { id: "ernie".into(), ..Family::Ernie.preset() });
    let (_, mut f) = dialog_fields(&mut app);
    draw_section(&mut f);
    // Drawing the settings stores no choice: the models keep following their architecture.
    let models: Vec<Model> = serde_json::from_value(f["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!(models.iter().map(|m| (m.inpaint, m.can_inpaint())).collect::<Vec<_>>(), [(None, true), (None, false)]);
}

#[test]
fn the_dock_gives_the_panel_room_for_its_generate_button() {
    use crate::dock::{DockLayout, Group};
    let shown = [Group::Color, Group::Properties, Group::Layers];
    let layout = DockLayout::default();
    // A 900-point window: Properties is squeezed to its compact height, less than the panel needs.
    let plain = layout.heights_for(&shown, 800.0, 28.0);
    let mut app = app();
    let want = 28.0 + wanted_height(&app);
    assert!(plain[1].1 < want, "{plain:?}");
    let roomy = layout.with_room(plain.clone(), Group::Properties, want);
    assert_eq!(roomy[1].1, want);
    // The room comes from Layers, which keeps its compact height; the column is as tall as before.
    assert!(roomy[2].1 >= Group::Layers.compact_height());
    assert_eq!(roomy.iter().map(|(_, h)| h).sum::<f32>(), plain.iter().map(|(_, h)| h).sum::<f32>());
    // InPaint has a row more.
    app.ui.ai_gen.mode = Mode::Inpaint;
    assert_eq!(wanted_height(&app), want - 28.0 + INPAINT_ROW);
    // A column with nothing to spare gives nothing; a height the user dragged is theirs.
    let tight = layout.heights_for(&shown, 420.0, 28.0);
    assert_eq!(layout.with_room(tight.clone(), Group::Properties, want), tight);
    let mut mine = DockLayout::default();
    mine.heights.insert(Group::Properties, 120.0);
    let dragged = mine.heights_for(&shown, 800.0, 28.0);
    assert_eq!(mine.with_room(dragged.clone(), Group::Properties, want), dragged);
    for hostile in [f32::NAN, f32::INFINITY, -5.0] {
        assert_eq!(layout.with_room(plain.clone(), Group::Properties, hostile), plain);
    }
    assert!(layout.with_room(Vec::new(), Group::Properties, want).is_empty());
}

#[test]
fn inpainting_models_have_their_own_settings_page() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) });
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    add_model(&mut app, Model { id: "qwen".into(), ..Family::QwenInpaint.preset() });
    assert!(PAGES.iter().any(|(key, title)| (*key, *title) == ("inpaint", "Inpainting Models")));
    // Each model is listed on one page: by where it runs, and whether it is made for inpainting.
    let draft = serde_json::to_value(&app.session.prefs().integrations.ai.models).unwrap();
    let kinds: Vec<Kind> = draft.as_array().unwrap().iter().map(Kind::of).collect();
    assert_eq!(kinds, [Kind::Local, Kind::Cloud, Kind::Inpaint, Kind::Inpaint]);
    assert_eq!(app.session.prefs().integrations.ai.models.iter().map(Kind::of_model).collect::<Vec<_>>(), kinds);
    // The gear on an inpainting model opens its page with it selected.
    let id = open_settings(&mut app, "qwen");
    let mut f = app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone();
    assert_eq!((f[PAGE].as_str(), f[SELECTED].as_u64()), (Some("inpaint"), Some(3)));
    // Drawing every page keeps the models as they are (the ControlNet row included).
    draw_section(&mut f);
    let after: Vec<Model> = serde_json::from_value(f["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!(after, app.session.prefs().integrations.ai.models);
    assert_eq!(after[3].controlnet, "Qwen-Image-InstantX-ControlNet-Inpainting.safetensors");
    // Add Model offers each page its own: the three models of ComfyUI's inpainting templates
    // there, the text-to-image architectures on the local page.
    let inpaint = add_presets(Kind::Inpaint);
    assert_eq!(inpaint.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["FLUX.1 Fill Dev", "FLUX.1 Fill OneReward", "Qwen Image InstantX Inpainting"]);
    assert!(inpaint.iter().all(|m| Kind::of_model(m) == Kind::Inpaint && m.can_inpaint() && !m.can_generate()));
    assert!(add_presets(Kind::Local).iter().all(|m| Kind::of_model(m) == Kind::Local));
    assert_eq!(add_presets(Kind::Local).len(), Family::ALL.len() - 2);
    assert!(add_presets(Kind::Cloud).iter().all(|m| Kind::of_model(m) == Kind::Cloud));
    // A scan adds the server's inpainting models next to its text-to-image ones.
    let folders = json!({
        "diffusion_models": ["z_image_turbo_bf16.safetensors", "flux1-fill-dev.safetensors", "qwen_image_fp8_e4m3fn.safetensors"],
        "controlnet": ["Qwen-Image-InstantX-ControlNet-Inpainting.safetensors"],
    });
    let (_, mut fresh) = dialog_fields(&mut app);
    *fresh.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai/models")).unwrap() = json!([]);
    let (added, _) = add_detected(&mut fresh, &folders);
    let found: Vec<Model> = serde_json::from_value(fresh["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!(added, found.len());
    assert_eq!(found.iter().filter(|m| Kind::of_model(m) == Kind::Inpaint).map(|m| m.family).collect::<Vec<_>>(), [Family::Flux1Fill, Family::QwenInpaint]);
    // With no inpainting model the InPaint tab says how to get one (and draws).
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": [z_image("z")]})).unwrap();
    app.ui.ai_gen.mode = Mode::Inpaint;
    assert!(enabled_models(&app).is_empty());
    draw_panel(&mut app);
}

#[test]
fn image_to_image_from_the_panel_picks_a_layer_or_another_document() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    app.ui.ai_gen.mode = Mode::Img2img;
    app.ui.ai_gen.prompt = "as a watercolour".into();
    // The models that generate are offered; one made for inpainting is not.
    assert_eq!(enabled_models(&app).into_iter().map(|m| m.id).collect::<Vec<_>>(), ["z"]);
    let z = z_image("z");
    // No document, then one with nothing in it: Generate is off and says what is missing.
    draw_panel(&mut app);
    app.run("file.new", json!({"width": 40, "height": 30, "background": "transparent"})).unwrap();
    assert!(generate_blocked(&app, &z).unwrap().contains("needs a picture to start from"));
    assert_eq!((img_source(&app), target_size(&app)), (None, None));
    draw_panel(&mut app);
    // A second document with an image: the first now has a source, the other document.
    let empty_doc = app.session.active().unwrap().doc.id.0;
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    let pictured = app.session.active().unwrap().doc.id.0;
    let background = app.session.active().unwrap().active_layer.unwrap().0;
    // Here the active layer has pixels: it is the source, redrawn over its own extent.
    assert_eq!(img_source(&app), Some(SourceRef::Layer(background)));
    assert_eq!((generate_blocked(&app, &z), target_size(&app)), (None, Some((64, 48, false))));
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.img_source, Some(SourceRef::Layer(background)));
    // The other document is empty: it is no picture, and is not offered.
    let offered = photocraft_engine::ai_cmds::img2img_sources(&app.session);
    assert_eq!(offered.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [SourceRef::Layer(background)]);
    assert!(offered[0].1.starts_with("Layer: "));
    assert!(photocraft_engine::ai_cmds::img2img_plan(&app.session, Some(SourceRef::Document(empty_doc)), None).err().unwrap().contains("is empty"));
    // A selection narrows the area, and masks the result.
    app.run("select.rect", json!({"x": 8, "y": 4, "width": 20, "height": 10})).unwrap();
    assert_eq!(target_size(&app), Some((20, 10, true)));
    app.ui.ai_gen.img_strength = 0.4;
    let before = app.session.active().unwrap().doc.layers.len();
    draw_panel(&mut app);
    let r = generate(&mut app).unwrap();
    assert_eq!((r["mode"].as_str(), r["bounds"].clone()), (Some("img2img"), json!([8, 4, 20, 10])));
    assert_eq!(app.session.active().unwrap().doc.layers.len(), before + 1);
    // Back in the first document the chosen layer is not there: the panel falls back to what
    // that document offers, the other document, and generating uses it.
    let first = app.session.documents().iter().position(|d| d.doc.id.0 == empty_doc).unwrap();
    app.run("document.activate", json!({"document": first})).unwrap();
    assert_eq!(app.session.active().unwrap().doc.id.0, empty_doc);
    assert_eq!(img_source(&app), Some(SourceRef::Document(pictured)));
    assert_eq!(target_size(&app), Some((40, 30, false)));
    draw_panel(&mut app);
    let r = generate(&mut app).unwrap();
    assert_eq!((r["mode"].as_str(), r["bounds"].clone()), (Some("img2img"), json!([0, 0, 40, 30])));
    // The state reads and writes like the engine names a source; hostile values are tamed.
    let state = serde_json::to_value(&app.ui.ai_gen).unwrap();
    assert_eq!(state["imgSource"], json!({"document": pictured}));
    let old: AiGenUi = serde_json::from_value(json!({"prompt": "x"})).unwrap();
    assert_eq!((old.img_source, old.img_strength), (None, 0.6));
    assert_eq!(AiGenUi { img_strength: f64::NAN, ..Default::default() }.img_strength(), 0.6);
    // The dock gives the tab the row its Source picker needs.
    assert_eq!(
        wanted_height(&app),
        {
            app.ui.ai_gen.mode = Mode::Txt2img;
            wanted_height(&app)
        } + INPAINT_ROW
    );
}
