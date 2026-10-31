use super::*;
use photocraft_aigen::{Request, Response, Settings, Transport};
use photocraft_doc::AiPicture;
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

/// An app with the fake server and no models: each test lists its own, not the recommended
/// ones a new installation starts with.
fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.ai.transport = Some(Arc::new(Fake));
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": []})).unwrap();
    // Local AI works here (the fake server renders), and the panel has already asked it: each
    // test lists its own models, whatever files the server has. What is offered when is
    // `only_models_that_can_run_are_offered`'s.
    app.ui.ai_setup.local_worked = true;
    app.ui.ai_gen.server_asked = 0.0;
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

/// Draw every page of the Generative AI section over the dialog's draft for a few frames.
fn draw_section(f: &mut Map<String, Value>) {
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    let before = f.get(PAGE).cloned();
    for page in PAGES.iter().map(|(key, _)| *key).cycle().take(PAGES.len() * 3) {
        f.insert(PAGE.into(), json!(page));
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            let obj = values.get_mut("integrations").and_then(Value::as_object_mut).unwrap();
            prefs_section(ui, obj, f, 480.0);
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
    // Models but no document; a stale model id falls back to the first model offered, which is
    // the first cloud model: the panel lists those before the local ones.
    add_model(&mut app, Model { enabled: false, ..z_image("off") });
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 1.5", "gpt-image-1.5", true) });
    add_model(&mut app, Model { id: "nb".into(), ..Model::cloud(Provider::Gemini, "Nano Banana 2", "gemini-3.1-flash-image-preview", false) });
    // Cloud models are offered once their key is there.
    app.session.ai.set_key(Provider::Openai, "sk-test");
    app.session.ai.set_key(Provider::Gemini, "g-test");
    app.ui.ai_gen.model = "gone".into();
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.model, "gpt");
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
    assert_eq!(f.get("section"), Some(&json!("generativeAi")));
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
fn the_gear_opens_manage_models_on_the_model_and_a_filter_selects_from_what_it_lists() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2.5 Flare", "gpt-image-2.5-flare", true) });
    add_model(&mut app, z_image("z2"));
    let fields = |app: &mut PhotocraftApp, model: &str| {
        let id = open_settings(app, model);
        app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone()
    };
    let f = fields(&mut app, "gpt");
    assert_eq!(
        (f["section"].as_str(), f[PAGE].as_str(), f[FILTER].as_str(), f[SELECTED].as_u64()),
        (Some("generativeAi"), Some("models"), Some("all"), Some(1))
    );
    let mut f = fields(&mut app, "z2");
    assert_eq!((f[PAGE].as_str(), f[SELECTED].as_u64()), (Some("models"), Some(2)));
    // Narrowed to Editing the list doesn't show Z-Image: it takes its first model, the cloud one.
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    f.insert(FILTER.into(), json!("edit"));
    for _ in 0..2 {
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f, 480.0);
            f.insert("values".into(), values);
        })
        .textures_delta
        .clear();
    }
    assert_eq!((f[FILTER].as_str(), f[SELECTED].as_u64()), (Some("edit"), Some(1)));
    // A filter that lists nothing says so, and selects nothing.
    f.insert(FILTER.into(), json!("inpaint"));
    ctx.run_ui(Default::default(), |ui| {
        let mut values = f.get("values").cloned().unwrap();
        prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f, 480.0);
    })
    .textures_delta
    .clear();
    assert_eq!(f[SELECTED], 1, "the selection is kept for when the list shows it again");
    // The local page reads a server on this machine once, without being asked.
    let mut f = fields(&mut app, "z");
    f.remove(ACTION);
    ctx.run_ui(Default::default(), |ui| {
        let mut values = f.get("values").cloned().unwrap();
        prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f, 480.0);
    })
    .textures_delta
    .clear();
    assert_eq!(f.get(ACTION), Some(&json!("peek")));
    after_body(&mut app, &mut f);
    assert!(f.get(FILES).is_some() && f.get(ACTION).is_none());
    // An unknown page or filter falls back to the first; any height is one it can be drawn in.
    f.insert(PAGE.into(), json!("nope"));
    f.insert(FILTER.into(), json!("nope"));
    draw_section(&mut f);
    for height in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e9] {
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            prefs_section(ui, values.get_mut("integrations").and_then(Value::as_object_mut).unwrap(), &mut f, height);
        })
        .textures_delta
        .clear();
    }
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
    // Opening the Manage Models page only looks: it must not start a server, or even ask to.
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
    assert!(f[STATUS]["text"].as_str().unwrap().contains(crate::brand::display("PhotoCraft will start it")), "{}", f[STATUS]);
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
    assert!(f[STATUS]["text"].as_str().unwrap().contains(crate::brand::display("started by PhotoCraft")), "{}", f[STATUS]);
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
    let (list, field) = page_widths(540.0);
    assert!((list - 216.0).abs() < 0.01 && field == 170.0, "{list} {field}");
    let (list, field) = page_widths(830.0);
    assert!(list > 300.0 && field > 300.0 && list + field + 160.0 <= 830.01, "{list} {field}");
    for odd in [0.0, -5.0, f32::NAN, f32::INFINITY, 1e9] {
        let (list, field) = page_widths(odd);
        assert!((210.0..=340.0).contains(&list) && (170.0..=420.0).contains(&field), "{odd}: {list} {field}");
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
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 3);
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
    assert_eq!(models.len(), 3, "{models:?}");
    assert_eq!(models[0]["unet"], "z_image_turbo_bf16.safetensors");
    assert_eq!(models[0]["clip"], "qwen_3_4b.safetensors");
    // The edit model is detected as one, not as a text-to-image model.
    assert_eq!((&models[2]["unet"], &models[2]["family"]), (&json!("qwen_image_edit_2511_bf16.safetensors"), &json!("qwenEdit")));
    // Detecting again adds nothing twice.
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 3);
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
            // Ticked for inpainting, so a change offers it too.
            inpaint: Some(true),
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
    // Panels of several sizes, down to a short narrow one (the tabs, Prompt | Regions, the
    // model, a two-line prompt, the Advanced heading, Generate and this reason): the notes
    // always end inside.
    for (w, h) in [(300.0, 420.0), (260.0, 360.0), (420.0, 700.0)] {
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
    // The same for a change, which has rows more (its tasks, the mask tools), and with the
    // Advanced section open.
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    app.ui.ai_gen.mode = Mode::Edit;
    assert_eq!(technique(&app.ui.ai_gen, Some(&broken)), GenerationMode::Inpaint);
    for (w, h, advanced) in [(300.0, 420.0, false), (260.0, 390.0, false), (300.0, 500.0, true)] {
        app.ui.ai_gen.advanced = advanced;
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
        assert!(overflow <= 1.0, "change {w}x{h}: the panel's content runs {overflow} points past its bottom");
    }
}

#[test]
fn a_change_lists_the_models_that_edit_or_inpaint_and_keeps_its_own_choice() {
    let mut app = app();
    // Text-to-image models are not told where a selection is: they are offered for a change
    // only when their inpainting box is ticked. FLUX.2 edits, and inpaints unless its box is
    // unticked; the models made for inpainting are offered too, and in turn not to create.
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
    app.ui.ai_gen.mode = Mode::Edit;
    assert_eq!(ids(&app), ["ticked", "flux", "off", "fill", "qwen"]);
    draw_panel(&mut app);
    // A change has its own model: choosing one leaves the model of Create alone.
    assert_eq!((app.ui.ai_gen.edit_model.as_str(), app.ui.ai_gen.model.as_str()), ("ticked", "ernie"));
    app.ui.ai_gen.edit_model = "fill".into();
    draw_panel(&mut app);
    assert_eq!((app.ui.ai_gen.chosen(), current_model(&app).map(|m| m.id.as_str())), ("fill", Some("fill")));
    // Restyle lists the models that generate, and shares the model of Create.
    app.ui.ai_gen.task = EditTask::Restyle;
    assert_eq!(ids(&app), ["ernie", "z", "ticked", "flux", "off"]);
    app.ui.ai_gen.mode = Mode::Create;
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.chosen(), "ernie");
    // What the engine is asked for follows from the task and from what the model can do.
    let model = |id: &str| app.session.prefs().integrations.ai.models.iter().find(|m| m.id == id).cloned().unwrap();
    let mut ui = AiGenUi::default();
    assert_eq!(technique(&ui, Some(&model("fill"))), GenerationMode::Txt2img);
    ui.mode = Mode::Edit;
    let asked = |ui: &AiGenUi| ["ticked", "flux", "off", "fill", "qwen"].map(|id| technique(ui, Some(&model(id))));
    use GenerationMode::{Img2img, ImgEdit, Inpaint};
    // An edit model is given an instruction, a model that only inpaints the selection as its mask.
    assert_eq!(asked(&ui), [Inpaint, ImgEdit, ImgEdit, Inpaint, Inpaint]);
    // The method is a choice only for a model that does both (FLUX.2 with its box on).
    assert_eq!(["ticked", "flux", "off", "fill", "qwen"].map(|id| both_ways(&model(id))), [false, true, false, false, false]);
    ui.method = Method::Fill;
    assert_eq!(asked(&ui), [Inpaint, Inpaint, ImgEdit, Inpaint, Inpaint]);
    assert_eq!(technique(&ui, None), ImgEdit, "no model to ask: as an instruction");
    ui.task = EditTask::Restyle;
    assert_eq!(asked(&ui), [Img2img; 5]);
    // No model that can: the panel says so instead of offering a list of nothing.
    let only = vec![Model { id: "ernie".into(), ..Family::Ernie.preset() }];
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": only})).unwrap();
    app.ui.ai_gen.mode = Mode::Edit;
    app.ui.ai_gen.task = EditTask::Change;
    assert!(ids(&app).is_empty() && current_model(&app).is_none());
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.mode, Mode::Edit);
    assert!(generate(&mut app).unwrap_err().contains("No AI model"));
    // The state names the tabs and tasks; the tabs of an earlier build, one per technique, load.
    assert_eq!(serde_json::to_value(Mode::ALL).unwrap(), json!(["create", "edit"]));
    assert_eq!(serde_json::to_value((EditTask::ALL, [Method::Instruct, Method::Fill])).unwrap(), json!([["change", "restyle"], ["instruct", "fill"]]));
    for (saved, mode) in [("txt2img", Mode::Create), ("inpaint", Mode::Edit), ("img2img", Mode::Edit), ("imgEdit", Mode::Edit), ("edit", Mode::Edit)] {
        let old: AiGenUi = serde_json::from_value(json!({"mode": saved, "inpaintModel": "gone"})).unwrap();
        assert_eq!(old, AiGenUi { mode, ..Default::default() }, "{saved}");
    }
    let old: AiGenUi = serde_json::from_value(json!({"prompt": "x"})).unwrap();
    assert_eq!((old.mode, old.task, old.method, old.strength, old.context), (Mode::Create, EditTask::Change, Method::Instruct, 1.0, 0.25));
    let hostile = AiGenUi { strength: f64::NAN, context: -7.0, ..Default::default() };
    assert_eq!((hostile.strength(), hostile.context()), (1.0, 0.0));
}

#[test]
fn inpainting_from_the_panel_needs_a_selection_on_a_pixel_layer_and_adds_a_masked_layer() {
    let mut app = app();
    let z = Model { inpaint: Some(true), ..z_image("z") };
    add_model(&mut app, z.clone());
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    // A model that inpaints and does not edit: a change with it is an inpainting.
    app.ui.ai_gen.mode = Mode::Edit;
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
    app.ui.ai_gen.mode = Mode::Create;
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

/// The height the docked panel asks for (`panel_docking`'s `dock_heights`) grows with the rows it
/// shows, so its Generate button stays in view.
#[test]
fn the_panel_asks_for_room_for_its_generate_button() {
    let mut app = app();
    let base = wanted_height(&app);
    assert!(base.is_finite() && base > crate::dock::Group::Properties.compact_height(), "{base}");
    // A change has a row more: the mask tools (Edit's tasks take the place of Create's Prompt |
    // Regions). Advanced, open, adds its own.
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    app.ui.ai_gen.mode = Mode::Edit;
    assert_eq!(wanted_height(&app), base + ROW);
    app.ui.ai_gen.advanced = true;
    assert_eq!(wanted_height(&app), base + 3.0 * ROW);
    // A model that changes a picture both ways has the method to choose as well.
    add_model(&mut app, Model { id: "flux".into(), ..Family::Flux2.preset() });
    app.ui.ai_gen.edit_model = "flux".into();
    app.ui.ai_gen.method = Method::Fill;
    assert_eq!(wanted_height(&app), base + 4.0 * ROW);
    app.ui.ai_gen.mode = Mode::Create;
    assert_eq!(wanted_height(&app), base + ROW);
}

#[test]
fn the_list_says_what_each_model_is_used_for_and_can_be_narrowed_to_it() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) });
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    add_model(&mut app, Model { id: "qwen".into(), ..Family::QwenInpaint.preset() });
    add_model(&mut app, Model { id: "flux".into(), ..Family::Flux2.preset() });
    // One page lists every model; the pages for local, inpainting and cloud models are gone.
    assert_eq!(PAGES.map(|(key, title)| (key, title)), [("models", "Manage Models"), ("keys", "API Keys"), ("server", "Local Server")]);
    let models = app.session.prefs().integrations.ai.models.clone();
    let listed = |filter: Filter| models.iter().filter(|m| filter.shows(Some(m))).map(|m| m.id.as_str()).collect::<Vec<_>>();
    assert_eq!(listed(Filter::All), ["z", "gpt", "fill", "qwen", "flux"]);
    assert_eq!(listed(Filter::Create), ["z", "gpt", "flux"]);
    assert_eq!(listed(Filter::Edit), ["gpt", "flux"]);
    assert_eq!(listed(Filter::Inpaint), ["fill", "qwen", "flux"]);
    assert_eq!(
        models.iter().map(|m| uses(m).join(", ")).collect::<Vec<_>>(),
        ["Text to image", "Text to image, Editing", "Inpainting", "Inpainting", "Text to image, Editing, Inpainting"]
    );
    // Ticking Supports inpainting moves a model under that filter too.
    assert!(Filter::Inpaint.shows(Some(&Model { inpaint: Some(true), ..z_image("z") })));
    // An entry that doesn't read as a model is listed under All only, where it can be removed.
    assert!(model_of(&json!({"provider": "nope"})).is_none() && model_of(&json!(7)).is_none());
    assert!(Filter::All.shows(None) && Filter::ALL.into_iter().filter(|x| x.shows(None)).count() == 1);
    for x in Filter::ALL {
        assert_eq!(Filter::from_key(x.key()), x);
        assert!(!x.title().is_empty() && !x.hint().is_empty());
    }
    assert_eq!(Filter::from_key("?"), Filter::All);
    // The gear on a model opens the list with it selected, whatever it is used for.
    let id = open_settings(&mut app, "qwen");
    let mut f = app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone();
    assert_eq!((f[PAGE].as_str(), f[FILTER].as_str(), f[SELECTED].as_u64()), (Some("models"), Some("all"), Some(3)));
    // Drawing under every filter keeps the models as they are (the ControlNet row included).
    for x in Filter::ALL {
        f.insert(FILTER.into(), json!(x.key()));
        draw_section(&mut f);
    }
    let after: Vec<Model> = serde_json::from_value(f["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!(after, app.session.prefs().integrations.ai.models);
    assert_eq!(after[3].controlnet, "Qwen-Image-InstantX-ControlNet-Inpainting.safetensors");
    // Add Model offers everything in one menu, grouped: cloud models, the local architectures,
    // and the three models of ComfyUI's inpainting templates.
    let groups = add_presets();
    assert_eq!(groups.iter().map(|(heading, _)| *heading).collect::<Vec<_>>(), ["Cloud", "Local", "Local, made for inpainting"]);
    assert_eq!((groups[0].1.len(), groups[1].1.len()), (Model::cloud_presets().len(), Family::ALL.len() - 2));
    assert!(groups[0].1.iter().all(|(_, m)| !m.provider.is_local()) && groups[1].1.iter().all(|(_, m)| m.provider.is_local() && !m.family.inpaint_only()));
    assert_eq!(
        groups[2].1.iter().map(|(title, _)| title.as_str()).collect::<Vec<_>>(),
        ["FLUX.1 Fill Dev", "FLUX.1 Fill OneReward", "Qwen Image InstantX Inpainting"]
    );
    assert!(groups[2].1.iter().all(|(_, m)| m.can_inpaint() && !m.can_generate()));
    // Nano Banana 2.1 and GPT Image 2 are among the cloud models on offer.
    let offered: Vec<(&str, &str)> = groups[0].1.iter().map(|(title, m)| (title.as_str(), m.api_model.as_str())).collect();
    assert!(
        offered.contains(&("Nano Banana 2.1 (Google Gemini)", "gemini-nano-banana-2.1")) && offered.contains(&("GPT Image 2 (OpenAI)", "gpt-image-2")),
        "{offered:?}"
    );
    // A scan adds the server's inpainting models next to its text-to-image ones.
    let folders = json!({
        "diffusion_models": ["z_image_turbo_bf16.safetensors", "flux1-fill-dev.safetensors", "qwen_image_fp8_e4m3fn.safetensors"],
        "controlnet": ["Qwen-Image-InstantX-ControlNet-Inpainting.safetensors"],
    });
    let (_, mut fresh) = dialog_fields(&mut app);
    *fresh.get_mut("values").and_then(|v| v.pointer_mut("/integrations/ai/models")).unwrap() = json!([]);
    let (added, repaired, matched) = add_detected(&mut fresh, &folders);
    let found: Vec<Model> = serde_json::from_value(fresh["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!((added, repaired, matched), (found.len(), 0, 0));
    assert_eq!(found.iter().filter(|m| m.family.inpaint_only()).map(|m| m.family).collect::<Vec<_>>(), [Family::Flux1Fill, Family::QwenInpaint]);
    // With no model that edits or inpaints, a change says how to get one (and draws).
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": [z_image("z")]})).unwrap();
    app.ui.ai_gen.mode = Mode::Edit;
    assert!(enabled_models(&app).is_empty());
    draw_panel(&mut app);
}

#[test]
fn image_to_image_from_the_panel_picks_a_layer_or_another_document() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    app.ui.ai_gen.mode = Mode::Edit;
    app.ui.ai_gen.task = EditTask::Restyle;
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
    // The dock gives the task its rows: the Source picker (Edit's tasks take the place of Create's row).
    assert_eq!(
        wanted_height(&app),
        {
            app.ui.ai_gen.mode = Mode::Create;
            wanted_height(&app)
        } + ROW
    );
}

#[test]
fn the_mask_tools_mark_what_to_repaint_from_the_panel() {
    use crate::state::Tool;
    let mut app = app();
    add_model(&mut app, Model { inpaint: Some(true), ..z_image("z") });
    app.ui.ai_gen.mode = Mode::Edit;
    app.ui.ai_gen.prompt = "a boat".into();
    // No document: the bar draws, disabled.
    draw_panel(&mut app);
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    draw_panel(&mut app);
    let z = Model { inpaint: Some(true), ..z_image("z") };
    assert!(generate_blocked(&app, &z).unwrap().contains("mask tools under the prompt"));
    // Outside the brush mode a stroke is the user's own: their colours, their eraser.
    let mut stroke = json!({"points": [[1, 1]], "erase": true});
    mask_stroke(&app, true, &mut stroke);
    assert_eq!(stroke, json!({"points": [[1, 1]], "erase": true}));
    // The brush: what the bar's button does.
    app.run("ai.mask", json!({"paint": true})).unwrap();
    app.ui.ai_gen.mask_brush = true;
    app.ui.tool = Tool::Brush;
    assert!(mask_brushing(&app));
    draw_panel(&mut app);
    assert!(app.ui.ai_gen.mask_brush, "the mode survives drawing");
    // Now the Brush paints the mask in and the Eraser paints it out, whatever the colours are.
    let mut add = json!({"points": [[20, 20], [40, 20]], "erase": false, "target": "quickMask", "size": 12});
    mask_stroke(&app, false, &mut add);
    assert_eq!((add["color"].as_str(), add["erase"].as_bool()), (Some("#ffffff"), Some(false)));
    let mut take = json!({"points": [[20, 20]], "erase": true});
    mask_stroke(&app, true, &mut take);
    assert_eq!((take["color"].as_str(), take["erase"].as_bool()), (Some("#000000"), Some(false)));
    app.run("paint.stroke", add).unwrap();
    assert!(photocraft_engine::ai_cmds::inpaint_marked(&app.session));
    assert_eq!(generate_blocked(&app, &z), None);
    draw_panel(&mut app);
    // The note sizes what will be rendered from the painted mask.
    let (w, h, selection) = target_size(&app).unwrap();
    assert!(w >= 28 && h >= 8 && !selection, "{w}x{h}");
    // Generate turns the mask into the selection and the new layer's mask, and ends the mode.
    let before = app.session.active().unwrap().doc.layers.len();
    let r = generate(&mut app).unwrap();
    assert_eq!(r["mode"], "inpaint");
    let d = app.session.active().unwrap();
    assert_eq!(d.doc.layers.len(), before + 1);
    assert!(d.doc.quick_mask.is_none() && d.doc.selection.is_some() && d.doc.layer(d.active_layer.unwrap()).unwrap().mask.is_some());
    draw_panel(&mut app);
    assert!(!app.ui.ai_gen.mask_brush && !mask_brushing(&app));
    // Quick Mask the user entered themselves is theirs: the panel's brush mode is not on.
    app.run("select.editInQuickMaskMode", json!({"on": true})).unwrap();
    app.ui.ai_gen.mask_brush = true;
    assert!(!mask_brushing(&app));
    // The dock makes room for the bar (Edit's tasks take the place of Create's row).
    assert_eq!(
        wanted_height(&app),
        {
            app.ui.ai_gen.mode = Mode::Create;
            wanted_height(&app)
        } + ROW
    );
}

#[test]
fn editing_from_the_panel_picks_a_source_references_and_a_quality() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "q21".into(), ..Family::QwenImage21.preset() });
    add_model(&mut app, Model { id: "edit".into(), ..Family::QwenEdit.preset() });
    add_model(&mut app, Model { id: "gpt-image-2".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) });
    app.session.ai.set_key(Provider::Openai, "sk-test");
    app.ui.ai_gen.mode = Mode::Edit;
    app.ui.ai_gen.prompt = "make the sky stormy".into();
    // The models that edit are offered, local and cloud; one that only generates is not, and an
    // edit model is offered nowhere else.
    let ids = |app: &PhotocraftApp| enabled_models(app).into_iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(&app), ["gpt-image-2", "q21", "edit"]);
    for (mode, task) in [(Mode::Create, EditTask::Change), (Mode::Edit, EditTask::Restyle)] {
        (app.ui.ai_gen.mode, app.ui.ai_gen.task) = (mode, task);
        assert_eq!(ids(&app), ["gpt-image-2", "z", "q21"], "{mode:?} {task:?}");
    }
    (app.ui.ai_gen.mode, app.ui.ai_gen.task) = (Mode::Edit, EditTask::Change);
    let q21 = Model { id: "q21".into(), ..Family::QwenImage21.preset() };
    // None chosen: Change runs Qwen Image 2.1, the recommended edit model, though a cloud model
    // is listed first; Create runs the first.
    app.ui.ai_gen.edit_model = String::new();
    assert_eq!(current_model(&app).map(|m| m.id.as_str()), Some("q21"));
    app.ui.ai_gen.mode = Mode::Create;
    assert_eq!(current_model(&app).map(|m| m.id.as_str()), Some("gpt-image-2"));
    app.ui.ai_gen.mode = Mode::Edit;
    // No document, then one with nothing in it: Generate is off and says what is missing.
    app.ui.ai_gen.edit_model = "q21".into();
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.edit_model, "q21", "a change keeps its own choice of model");
    app.run("file.new", json!({"width": 40, "height": 30, "background": "transparent"})).unwrap();
    assert!(generate_blocked(&app, &q21).unwrap().contains("Editing needs a picture"));
    assert_eq!((edit_source(&app), target_size(&app)), (None, None));
    draw_panel(&mut app);
    // A model that can't edit says so (a state set from outside may name one).
    assert!(generate_blocked(&app, &z_image("z")).is_some());
    // A document with an image: its active layer is the source, the document itself is offered.
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    let doc = app.session.active().unwrap().doc.id.0;
    let background = app.session.active().unwrap().active_layer.unwrap().0;
    assert_eq!(edit_source(&app), Some(SourceRef::Layer(background)));
    assert_eq!((generate_blocked(&app, &q21), target_size(&app)), (None, Some((64, 48, false))));
    let offered: Vec<SourceRef> = photocraft_engine::ai_cmds::edit_sources(&app.session).into_iter().map(|(s, _)| s).collect();
    assert_eq!(offered, [SourceRef::Document(doc), SourceRef::Layer(background)]);
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.edit_source, Some(SourceRef::Layer(background)));
    // A selection scopes the edit: the model is sent it with a quarter of its edge around it.
    app.run("select.rect", json!({"x": 16, "y": 12, "width": 16, "height": 16})).unwrap();
    assert_eq!(target_size(&app), Some((24, 24, true)));
    // References: the document's own (ai.setReferences), sent when the prompt names none.
    app.run("ai.setReferences", json!({"references": [{"document": doc}, {"layer": background}, {"document": doc}]})).unwrap();
    draw_panel(&mut app);
    assert_eq!(refs::kept(&app.session).len(), 3);
    // Qwen Image Edit takes two: Generate says to name the ones to send.
    app.ui.ai_gen.edit_model = "edit".into();
    let edit = current_model(&app).cloned().unwrap();
    assert!(generate_blocked(&app, &edit).unwrap().contains("takes fewer references"), "{:?}", generate_blocked(&app, &edit));
    draw_panel(&mut app);
    app.run("ai.setReferences", json!({"references": [{"keep": 0}, {"keep": 1}]})).unwrap();
    app.ui.ai_gen.edit_model = "q21".into();
    app.ui.ai_gen.edit_quality = GenerationQuality::Draft;
    let before = app.session.active().unwrap().doc.layers.len();
    let r = generate(&mut app).unwrap();
    assert_eq!((r["mode"].as_str(), r["model"].as_str(), r["bounds"].clone()), (Some("imgEdit"), Some("q21"), json!([12, 8, 24, 24])));
    assert_eq!(app.session.active().unwrap().doc.layers.len(), before + 1);
    let made = {
        let d = app.session.active().unwrap();
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert!(layer.mask.is_some(), "the selection becomes the layer's mask");
        layer.generation.clone().unwrap()
    };
    // The layer keeps its own document's layer as one (what a saved file keeps).
    assert_eq!(made.references, [photocraft_doc::GenerationSource::Document(doc).into(), photocraft_doc::GenerationSource::Layer(background).into()]);
    assert_eq!((made.quality, made.prompt.as_str()), (GenerationQuality::Draft, "make the sky stormy"));
    // Load Prompt puts the edit's prompt back, and only that: the tab, model, source,
    // references and quality stay as they are.
    let layer = r["layer"].as_u64().unwrap();
    (app.ui.ai_gen.prompt, app.ui.ai_gen.mode, app.ui.ai_gen.edit_model) = ("something else".into(), Mode::Create, "z".into());
    let kept = app.ui.ai_gen.clone();
    assert!(load_prompt(&mut app, layer));
    assert_eq!(app.ui.ai_gen, AiGenUi { prompt: "make the sky stormy".into(), ..kept });
    (app.ui.ai_gen.mode, app.ui.ai_gen.edit_model) = (Mode::Edit, "q21".into());
    draw_panel(&mut app);
    // A cloud model edits too (its quality is its own setting); so does every tab's draw.
    app.ui.ai_gen.edit_model = "gpt-image-2".into();
    draw_panel(&mut app);
    // The state reads and writes like the engine names things; an old state has none of it, and
    // references an old state kept are no longer the panel's.
    let state = serde_json::to_value(&app.ui.ai_gen).unwrap();
    assert_eq!((&state["editSource"], &state["editQuality"]), (&json!({"layer": background}), &json!("draft")));
    let old: AiGenUi = serde_json::from_value(json!({"prompt": "x", "mode": "imgEdit", "editReferences": [{"layer": 3}]})).unwrap();
    assert_eq!((old.mode, old.edit_source, old.edit_quality), (Mode::Edit, None, GenerationQuality::Standard));
    // The dock gives the task its rows: the source, the chips and the mask tools (Edit's tasks
    // take the place of Create's row); a chip open shows its name and picture too.
    let tall = wanted_height(&app);
    app.ui.ai_gen.editing_ref = Some(0);
    assert_eq!(wanted_height(&app), tall + 2.0 * ROW);
    app.ui.ai_gen.editing_ref = Some(5);
    assert_eq!(wanted_height(&app), tall, "a chip that is gone is not open");
    app.ui.ai_gen.mode = Mode::Create;
    assert_eq!(tall, wanted_height(&app) + 3.0 * ROW);
}

#[test]
fn references_are_named_chips_the_document_keeps() {
    let mut app = app();
    add_model(&mut app, Model { id: "q21".into(), ..Family::QwenImage21.preset() });
    // A picture to change, a layer to refer to, and another document with a layer of its own.
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    let here = app.session.active().unwrap().doc.id.0;
    let background = app.session.active().unwrap().active_layer.unwrap().0;
    app.run("layer.new.layer", json!({"name": "Max"})).unwrap();
    app.run("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    app.run("select.deselect", json!({})).unwrap();
    let max = app.session.active().unwrap().active_layer.unwrap().0;
    app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
    let there = app.session.active().unwrap().doc.id.0;
    app.run("layer.new.layer", json!({"name": "Luna"})).unwrap();
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 8, "height": 20})).unwrap();
    app.run("edit.fill", json!({"color": "#0000ff"})).unwrap();
    app.run("select.deselect", json!({})).unwrap();
    let luna = app.session.active().unwrap().active_layer.unwrap().0;
    assert!(app.session.set_active(0));
    app.run("layer.select", json!({"layer": background})).unwrap();
    (app.ui.ai_gen.mode, app.ui.ai_gen.task, app.ui.ai_gen.edit_model) = (Mode::Edit, EditTask::Change, "q21".into());
    // The other document's layer is on offer, after its document.
    let offered = refs::offered(&app.session);
    let theirs = SourceRef::DocumentLayer { document: there, layer: luna };
    let at = offered.iter().position(|(r, _)| *r == SourceRef::Document(there)).unwrap();
    assert_eq!(offered[at + 1].0, theirs);
    // Chips are named after their pictures, and names don't clash.
    assert_eq!(photocraft_engine::ai_cmds::reference_name(&app.session, theirs), "Luna");
    assert_eq!(
        (refs::unique("Luna", &["luna", "Luna2"]), refs::unique("Luna", &["Max"]), refs::unique("", &[""])),
        ("Luna3".into(), "Luna".into(), String::new())
    );
    assert_eq!(refs::unique(&"x".repeat(24), &[&"x".repeat(24)]).chars().count(), 24);
    // Setting them from the panel: another document's layer is copied in, this one's is kept as
    // a layer.
    let entries = [
        refs::Entry::New(theirs, "Luna".into()),
        refs::Entry::New(SourceRef::DocumentLayer { document: here, layer: max }, "Max".into()),
        refs::Entry::New(SourceRef::Document(there), String::new()),
    ];
    assert!(refs::set(&mut app, &entries, None));
    let kept = refs::kept(&app.session);
    assert!(matches!(kept[0].picture, AiPicture::Image { .. }) && kept[1].picture == AiPicture::Layer(max), "{kept:?}");
    draw_panel(&mut app);
    assert_eq!(refs::kept(&app.session).len(), 3);
    // Generating sends the document's references; the engine sends the mentioned ones, the copy
    // as it was kept.
    app.ui.ai_gen.prompt = "@Luna sits beside @Max".into();
    let q21 = current_model(&app).cloned().unwrap();
    assert_eq!(generate_blocked(&app, &q21), None);
    let r = generate(&mut app).unwrap();
    assert_eq!(r["modelPrompt"], "Luna (image 2) sits beside Max (image 3)");
    // Two alike can't be set: the document keeps its references as they were.
    let twice = [refs::Entry::Keep(0, "Luna".into()), refs::Entry::Keep(1, "LUNA".into())];
    assert!(!refs::set(&mut app, &twice, None));
    assert_eq!(refs::kept(&app.session).len(), 3);
    // The other document closed: the copy stays (it is this document's), as do the others.
    assert!(app.session.close(1).is_some());
    draw_panel(&mut app);
    assert_eq!(refs::kept(&app.session).iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Luna", "Max", ""]);
    // An open chip draws its name and picture; taking one out and undoing it are History steps.
    app.ui.ai_gen.editing_ref = Some(1);
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.editing_ref, Some(1));
    assert!(refs::set(&mut app, &[refs::Entry::Keep(1, "Max".into())], None));
    assert_eq!(refs::kept(&app.session).len(), 1);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(refs::kept(&app.session).len(), 3);
    // A rename typed letter by letter is one step.
    for name in ["M", "Ma", "Mal"] {
        assert!(refs::set(
            &mut app,
            &[refs::Entry::Keep(0, "Luna".into()), refs::Entry::Keep(1, name.into()), refs::Entry::Keep(2, String::new())],
            Some("ai-ref-name-1".into())
        ));
    }
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(refs::kept(&app.session)[1].name, "Max");
}

/// The prompt names references with @: what it sends, how it shows them, and the names it offers.
#[test]
fn the_prompt_mentions_references_by_name_and_offers_them_after_an_at() {
    // What the prompt sends: all of them while it mentions none, then those it mentions.
    let names = ["Luna", "Max", ""];
    use refs::Sent::{All, Mentioned, No};
    assert_eq!(refs::sent("put image 4 behind them", &names), [(All, Some(2)), (All, Some(3)), (All, Some(4))]);
    assert_eq!(refs::sent("@max hugs @Luna", &names), [(Mentioned, Some(2)), (Mentioned, Some(3)), (No, None)]);
    assert_eq!(refs::sent("@Max waves", &names), [(No, None), (Mentioned, Some(2)), (No, None)]);
    // The prompt shows a mention in the accent colour, on a tint; other @words stay plain.
    let job = refs::prompt_job("@Luna meets @Bob", &["Luna", "Max"], egui::FontId::proportional(12.0), egui::Color32::WHITE, egui::Color32::BLUE, 100.0);
    let colored: Vec<(&str, egui::Color32)> =
        job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color)).filter(|(t, _)| !t.is_empty()).collect();
    assert_eq!(colored, [("@Luna", egui::Color32::BLUE), (" meets @Bob", egui::Color32::WHITE)]);
    // Typing @ offers the names; Tab or a click puts the one taken in, and a space after it.
    let names = ["Luna", "Max", "Lucas"];
    assert_eq!(refs::completion_at("hug @", 5, &names), Some((4..5, vec![0, 1, 2])));
    assert_eq!(refs::completion_at("hug @lu", 7, &names), Some((4..7, vec![0, 2])));
    assert_eq!(refs::completion_at("hug @Luna", 9, &names), None, "typed out already");
    assert_eq!(refs::completion_at("me@lu", 5, &names), None, "inside a word");
    assert_eq!(refs::completion_at("@lu x", 3, &names), Some((0..3, vec![0, 2])));
    assert_eq!(refs::completion_at("@lux", 3, &names), None, "the word goes on past the cursor");
    assert_eq!(refs::completion_at("hug @zz", 7, &names), None);
    assert_eq!(refs::completion_at("🦆 @L", 4, &names), Some(("🦆 ".len().."🦆 @L".len(), vec![0, 2])));
    assert_eq!(refs::completion_at("x", 99, &names), None);
    assert_eq!(refs::complete("hug @lu now", 4..7, "Luna"), ("hug @Luna now".into(), 9));
    assert_eq!(refs::complete("hug @lu", 4..7, "Luna"), ("hug @Luna ".into(), 10));
}

#[test]
fn a_painted_mask_scopes_a_change_by_instruction() {
    let mut app = app();
    let q21 = Model { id: "q21".into(), ..Family::QwenImage21.preset() };
    add_model(&mut app, q21.clone());
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    app.ui.ai_gen.mode = Mode::Edit;
    app.ui.ai_gen.prompt = "make it a pond".into();
    app.ui.ai_gen.context = 0.0;
    // An edit model: the change is an instruction, and nothing marked means the whole layer.
    assert_eq!(technique(&app.ui.ai_gen, current_model(&app)), GenerationMode::ImgEdit);
    assert_eq!((generate_blocked(&app, &q21), target_size(&app)), (None, Some((64, 48, false))));
    // The mask tools are there for an instruction too: what is painted scopes the change.
    app.run("ai.mask", json!({"paint": true})).unwrap();
    app.ui.ai_gen.mask_brush = true;
    app.ui.tool = crate::state::Tool::Brush;
    app.run("paint.stroke", json!({"points": [[20, 20], [40, 20]], "size": 12, "hardness": 1.0, "color": "#ffffff", "target": "quickMask"})).unwrap();
    draw_panel(&mut app);
    assert!(app.ui.ai_gen.mask_brush, "the mode survives drawing");
    let (w, h, marked) = target_size(&app).unwrap();
    assert!(marked && (28..64).contains(&w) && (8..48).contains(&h), "{w}x{h}");
    // Generate turns the mask into the selection and the new layer's mask, and ends the mode.
    let before = app.session.active().unwrap().doc.layers.len();
    let r = generate(&mut app).unwrap();
    assert_eq!((r["mode"].as_str(), r["bounds"][2].as_u64(), r["bounds"][3].as_u64()), (Some("imgEdit"), Some(u64::from(w)), Some(u64::from(h))));
    let d = app.session.active().unwrap();
    assert_eq!(d.doc.layers.len(), before + 1);
    assert!(d.doc.quick_mask.is_none() && d.doc.selection.is_some() && d.doc.layer(d.active_layer.unwrap()).unwrap().mask.is_some());
    draw_panel(&mut app);
    assert!(!app.ui.ai_gen.mask_brush && !mask_brushing(&app));
}

#[test]
fn the_advanced_section_opens_the_settings_of_the_task_in_front() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "flux".into(), ..Family::Flux2.preset() });
    add_model(&mut app, Model { id: "fill".into(), ..Family::Flux1Fill.preset() });
    add_model(&mut app, Model { id: "q21".into(), ..Family::QwenImage21.preset() });
    add_model(&mut app, Model { id: "gpt".into(), inpaint: Some(true), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) });
    app.session.ai.set_key(Provider::Openai, "sk-test");
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    app.run("select.rect", json!({"x": 16, "y": 12, "width": 16, "height": 16})).unwrap();
    let rows = |app: &PhotocraftApp| {
        let model = current_model(app);
        advanced_rows(model, technique(&app.ui.ai_gen, model))
    };
    // Closed, it is its heading; open, the rows of the task: the seed, for a change also its
    // strength or quality with the context, and the method when the model leaves that open.
    let closed = wanted_height(&app);
    app.ui.ai_gen.advanced = true;
    app.ui.ai_gen.seed = Some(7);
    assert_eq!((rows(&app), wanted_height(&app)), (1, closed + ROW));
    draw_panel(&mut app);
    (app.ui.ai_gen.mode, app.ui.ai_gen.task) = (Mode::Edit, EditTask::Restyle);
    assert_eq!(rows(&app), 1);
    draw_panel(&mut app);
    app.ui.ai_gen.task = EditTask::Change;
    for (id, method, want) in [
        ("flux", Method::Instruct, 3),
        ("flux", Method::Fill, 3),
        ("fill", Method::Instruct, 2),
        ("q21", Method::Fill, 2),
        ("gpt", Method::Instruct, 3),
        ("gpt", Method::Fill, 3),
    ] {
        app.ui.ai_gen.edit_model = id.into();
        app.ui.ai_gen.method = method;
        assert_eq!(rows(&app), want, "{id} {method:?}");
        draw_panel(&mut app);
        // Drawing leaves what was chosen as it is, for a local and for a cloud model.
        let ui = &app.ui.ai_gen;
        assert_eq!(
            (ui.edit_model.as_str(), ui.method, ui.strength, ui.context, ui.edit_quality, ui.seed),
            (id, method, 1.0, 0.25, GenerationQuality::Standard, Some(7)),
            "{id}"
        );
    }
    // The heading is drawn closed as well, in the floating panel's layout too.
    app.ui.ai_gen.advanced = false;
    draw_panel(&mut app);
}

#[test]
fn the_seed_of_the_last_image_can_be_kept() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
    app.ui.ai_gen.prompt = "a green field".into();
    // No seed asked for: the model's own setting, a new one each time. The panel remembers which.
    let first = generate(&mut app).unwrap()["seed"].as_u64().unwrap();
    assert_eq!((app.ui.ai_gen.seed, app.ui.ai_gen.last_seed), (None, Some(first)));
    // Kept (the lock in Advanced): the next image renders with it, and its layer says so.
    app.ui.ai_gen.seed = app.ui.ai_gen.last_seed;
    let r = generate(&mut app).unwrap();
    assert_eq!(r["seed"].as_u64(), Some(first));
    let d = app.session.active().unwrap();
    assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().generation.as_ref().map(|g| g.seed), Some(first));
    // A typed seed: its digits; nothing typed, none; more than a seed holds, what was there.
    assert_eq!((typed_seed("42", None), typed_seed(" 4 2x", Some(1)), typed_seed("-7", None)), (Some(42), Some(42), Some(7)));
    assert_eq!((typed_seed("", Some(5)), typed_seed("seed", Some(5))), (None, None));
    assert_eq!(typed_seed(&SEED_MASK.to_string(), None), Some(SEED_MASK));
    assert_eq!((typed_seed("281474976710656", Some(9)), typed_seed(&"9".repeat(40), None)), (Some(9), None));
    app.ui.ai_gen.seed = typed_seed("42", None);
    assert_eq!(generate(&mut app).unwrap()["seed"], 42);
    // A state set from outside with a seed too large is brought into range, not refused.
    app.ui.ai_gen.seed = Some(u64::MAX);
    assert_eq!(generate(&mut app).unwrap()["seed"].as_u64(), Some(SEED_MASK));
    let state = serde_json::to_value(&app.ui.ai_gen).unwrap();
    assert_eq!((&state["seed"], &state["lastSeed"], &state["advanced"]), (&json!(u64::MAX), &json!(SEED_MASK), &json!(false)));
    // A result without a seed forgets the last one.
    finished(&mut app, &json!({"seed": "x"}));
    assert_eq!(app.ui.ai_gen.last_seed, None);
}

/// A document with one generation of each kind, made from the panel: the ids of the Background
/// and of the created, the restyled and the filled layer.
fn generated_layers(app: &mut PhotocraftApp) -> [u64; 4] {
    add_model(app, z_image("z"));
    add_model(app, Model { id: "z2".into(), inpaint: Some(true), ..Family::ZImage.preset() });
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    let background = app.session.active().unwrap().active_layer.unwrap().0;
    let made = |app: &mut PhotocraftApp| {
        let r = generate(app).unwrap();
        r["layer"].as_u64().unwrap()
    };
    app.ui.ai_gen = AiGenUi { model: "z2".into(), prompt: "a green field".into(), alpha: false, ..Default::default() };
    let txt = made(app);
    let restyle = (Mode::Edit, EditTask::Restyle);
    app.ui.ai_gen =
        AiGenUi { mode: restyle.0, task: restyle.1, model: "z".into(), prompt: "as a watercolour".into(), img_strength: 0.35, ..Default::default() };
    app.ui.ai_gen.img_source = Some(SourceRef::Layer(background));
    let img = made(app);
    app.run("select.rect", json!({"x": 8, "y": 4, "width": 20, "height": 10})).unwrap();
    app.ui.ai_gen = AiGenUi { mode: Mode::Edit, edit_model: "z2".into(), prompt: "a pond".into(), strength: 0.75, context: 0.5, ..Default::default() };
    let inpaint = made(app);
    [background, txt, img, inpaint]
}

#[test]
fn load_prompt_puts_a_generated_layers_prompt_in_the_box_and_nothing_else() {
    let mut app = app();
    let [background, txt, img, inpaint] = generated_layers(&mut app);
    // Something else in the panel: only its prompt is replaced.
    let other = AiGenUi { mode: Mode::Create, model: "z".into(), edit_model: "z".into(), prompt: "something else".into(), ..Default::default() };
    app.ui.ai_gen = other.clone();
    // A layer that was not generated, and one that is not there, change nothing.
    assert!(!load_prompt(&mut app, background));
    assert!(!load_prompt(&mut app, u64::MAX));
    assert_eq!(app.ui.ai_gen, other);
    // A fill, a restyle and a creation: the prompt, and the tab, task, model and settings stay.
    for (layer, prompt) in [(inpaint, "a pond"), (img, "as a watercolour"), (txt, "a green field")] {
        assert!(load_prompt(&mut app, layer));
        assert_eq!(app.ui.ai_gen, AiGenUi { prompt: prompt.into(), ..other.clone() });
    }
    draw_panel(&mut app);
    assert_eq!((app.ui.ai_gen.mode, app.ui.ai_gen.model.as_str()), (Mode::Create, "z"));
    // The command does the same, for the active layer when none is named.
    app.ui.ai_gen = other.clone();
    assert_eq!(menu(&mut app, LOAD_PROMPT, &json!({"layer": inpaint})), Some(Ok(json!({"loaded": true}))));
    assert_eq!(app.ui.ai_gen, AiGenUi { prompt: "a pond".into(), ..other.clone() });
    app.run("layer.select", json!({"layer": txt})).unwrap();
    assert_eq!(menu(&mut app, LOAD_PROMPT, &json!({})), Some(Ok(json!({"loaded": true}))));
    assert_eq!(app.ui.ai_gen.prompt, "a green field");
    assert_eq!(menu(&mut app, LOAD_PROMPT, &json!({"layer": background})), Some(Ok(json!({"loaded": false}))));
    assert!(menu(&mut app, LOAD_PROMPT, &json!({"layer": "x"})).unwrap().is_err());
    assert!(menu(&mut app, LOAD_PROMPT, &json!({"layer": -1})).unwrap().is_err());
    // A generated layer whose prompt is empty has nothing to load.
    let d = app.session.active_mut().unwrap();
    std::sync::Arc::make_mut(&mut d.doc).layer_mut(photocraft_doc::LayerId(txt)).unwrap().generation.as_mut().unwrap().prompt = "  ".into();
    assert!(!load_prompt(&mut app, txt));
    assert_eq!(app.ui.ai_gen.prompt, "a green field");
}

mod clicks {
    use super::*;
    use crate::layer_row_ui::recorded;
    use egui::{Modifiers, PointerButton, Pos2, vec2};
    use egui_kittest::{Harness, kittest::Queryable};

    fn click(h: &mut Harness<'_, PhotocraftApp>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    }

    #[test]
    fn clicking_a_generated_layer_leaves_the_panel_and_load_prompt_loads_its_prompt() {
        const LOAD: &str = "Load the prompt the selected layer was generated with";
        const NOTHING: &str = "Load prompt: select a layer made with AI Gen";
        let mut app = app();
        let [background, _, _, inpaint] = generated_layers(&mut app);
        app.ui.ai_gen = AiGenUi { model: "z".into(), prompt: "something else".into(), ..Default::default() };
        menu(&mut app, PANEL, &json!({"show": true})).unwrap().unwrap();
        let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
            app
        });
        h.run_steps(8);
        let row = |h: &Harness<'_, PhotocraftApp>, layer: u64| recorded(&h.ctx).into_iter().find(|r| r.layer == layer).unwrap();
        // Clicking the layer selects it and leaves the panel as it is.
        let at = row(&h, inpaint).name.unwrap().center();
        click(&mut h, at);
        assert_eq!(h.state().session.active().unwrap().active_layer, Some(photocraft_doc::LayerId(inpaint)));
        let before = h.state().ui.ai_gen.clone();
        assert_eq!((before.mode, before.model.as_str(), before.prompt.as_str()), (Mode::Create, "z", "something else"));
        // Load Prompt puts its prompt in the box: no other tab, no other model.
        let at = h.get_by_label(LOAD).rect().center();
        click(&mut h, at);
        assert_eq!(h.state().ui.ai_gen, AiGenUi { prompt: "a pond".into(), ..before });
        // A layer that was not generated has no prompt to load: the button is off.
        let at = row(&h, background).name.unwrap().center();
        click(&mut h, at);
        assert_eq!(h.state().session.active().unwrap().active_layer, Some(photocraft_doc::LayerId(background)));
        assert!(h.query_by_label(LOAD).is_none());
        let at = h.get_by_label(NOTHING).rect().center();
        click(&mut h, at);
        assert_eq!(h.state().ui.ai_gen.prompt, "a pond");
    }
}

#[test]
fn a_new_installation_lists_the_recommended_models_and_a_scan_points_them_at_the_servers_files() {
    // Not `app()`, which empties the list.
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.ai.transport = Some(Arc::new(Fake));
    assert_eq!(app.session.prefs().integrations.ai.models, Model::defaults());
    let (_, mut f) = dialog_fields(&mut app);
    assert_eq!(f.get("section"), Some(&json!("generativeAi")));
    draw_section(&mut f);
    // Restore Defaults has nothing to add until some are removed; then it adds those, and only those.
    let mut ai = f["values"]["integrations"]["ai"].as_object().cloned().unwrap();
    assert_eq!(restore_defaults(&mut ai, &mut f), 0);
    ai.get_mut("models").and_then(Value::as_array_mut).unwrap().retain(|m| m["id"] != "nano-banana-pro" && m["id"] != "krea-2-turbo");
    assert_eq!(restore_defaults(&mut ai, &mut f), 2);
    let ids: Vec<&str> = ai["models"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!((ids.len(), &ids[15..]), (17, &["nano-banana-pro", "krea-2-turbo"][..]));
    // The same model stored another way, or under the user's own name, counts as listed.
    for m in ai.get_mut("models").and_then(Value::as_array_mut).unwrap() {
        if m["id"] == "z-image-turbo" {
            m["unet"] = json!("z_image_turbo_bf16.safetensors");
            m["name"] = json!("mine");
        }
    }
    assert_eq!(restore_defaults(&mut ai, &mut f), 0);
    // Emptied, or unreadable: all seventeen come back.
    let mut none = Map::new();
    assert_eq!((restore_defaults(&mut none, &mut f), none["models"].as_array().unwrap().len()), (17, 17));
    let mut junk: Map<String, Value> = serde_json::from_value(json!({"models": [1, "a", {"provider": "nope"}]})).unwrap();
    assert_eq!(restore_defaults(&mut junk, &mut f), 17);

    // The fake server has Z-Image Turbo as bf16, not the int8 file the recommended entry names:
    // a scan points that entry at the server's file instead of listing the model twice, and adds
    // the two models that were not listed.
    let (_, mut f) = dialog_fields(&mut app);
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    let models: Vec<Model> = serde_json::from_value(f["values"]["integrations"]["ai"]["models"].clone()).unwrap();
    assert_eq!(models.len(), 19, "{:?}", models.iter().map(|m| &m.id).collect::<Vec<_>>());
    let z = models.iter().find(|m| m.id == "z-image-turbo").unwrap();
    assert_eq!((z.name.as_str(), z.unet.as_str(), z.cfg, z.steps), ("Z-Image Turbo", "z_image_turbo_bf16.safetensors", 2.0, 8));
    let status = f[STATUS]["text"].as_str().unwrap().to_string();
    assert!(status.contains("2 models added") && status.contains("1 matched to the server's files"), "{status}");
    // A second scan has nothing left to do.
    f.insert(ACTION.into(), json!("detect"));
    after_body(&mut app, &mut f);
    assert_eq!(f["values"]["integrations"]["ai"]["models"].as_array().unwrap().len(), 19);
    assert!(!f[STATUS]["text"].as_str().unwrap().contains("matched"));
    // With the server's files read, the list marks the models whose file it doesn't have.
    let listing: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_value(f[FILES].clone()).unwrap();
    let lost: Vec<&str> = models.iter().filter(|m| !photocraft_aigen::detect::on_server(&listing, m)).map(|m| m.id.as_str()).collect();
    assert_eq!(
        lost,
        ["qwen-image-2-1", "krea-2-turbo", "flux-3-klein", "moss-image", "fibo-2-dev", "ideogram-4", "hidream-o1", "ernie-image-turbo", "ming-image-0-1-design"]
    );
    for x in Filter::ALL {
        f.insert(FILTER.into(), json!(x.key()));
        draw_section(&mut f);
    }
}

#[test]
fn the_newer_cloud_providers_take_a_key_and_are_drawn_sized_and_priced() {
    let mut app = app();
    for m in Model::cloud_presets().into_iter().filter(|m| matches!(m.provider, Provider::Xai | Provider::Meta | Provider::Bfl)) {
        add_model(&mut app, Model { id: m.provider.key().into(), ..m });
    }
    let by = |app: &PhotocraftApp, id: &str| app.session.prefs().integrations.ai.models.iter().find(|m| m.id == id).cloned().unwrap();
    assert_eq!(by(&app, "bfl").api_model, "flux-3-image");
    // Each has a row on the API Keys page; a key typed there is saved like the others.
    let (_, mut f) = dialog_fields(&mut app);
    for provider in ["xai", "meta", "bfl"] {
        f[KEYS][provider] = json!(format!("{provider}-secret"));
    }
    draw_section(&mut f);
    prefs_confirmed(&mut app, &f).unwrap();
    let keys = app.session.execute("ai.keys", json!({})).unwrap();
    assert_eq!((&keys["xai"], &keys["meta"], &keys["bfl"]), (&json!(true), &json!(true), &json!(true)));
    assert!(!app.session.prefs_to_json().contains("-secret"));
    let e = app.session.execute("ai.setKey", json!({"provider": "nope", "key": "x"})).unwrap_err().to_string();
    assert!(e.contains("openai|gemini|xai|meta|bfl"), "{e}");
    // xAI and Black Forest Labs are asked for a shape, Muse Image for a size that only gives one.
    assert_eq!((render_size(&by(&app, "xai"), 1920, 1080), render_size(&by(&app, "bfl"), 1920, 1080)), ((16, 9), (16, 9)));
    let (w, h) = render_size(&by(&app, "meta"), 1920, 1080);
    assert!(w > h && w >= 1024, "{w}x{h}");
    // Muse Image has one price; the other two say what the last image cost once there was one.
    assert_eq!(reported_price_note(&app, &by(&app, "meta")).map(|(text, _)| text).as_deref(), Some("Cost $0.010"));
    assert!(reported_price_note(&app, &by(&app, "xai")).is_none() && reported_price_note(&app, &by(&app, "bfl")).is_none());
    app.ui.ai_gen.last_cost = Some(0.048);
    assert_eq!(reported_price_note(&app, &by(&app, "bfl")).map(|(text, _)| text).as_deref(), Some("Last image $0.048"));
    // The panel and every settings page draw with each of them chosen.
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    for id in ["xai", "meta", "bfl"] {
        app.ui.ai_gen.model = id.into();
        app.ui.ai_gen.edit_model = id.into();
        for mode in Mode::ALL {
            app.ui.ai_gen.mode = mode;
            draw_panel(&mut app);
        }
        let dialog = open_settings(&mut app, id);
        let mut f = app.ui.dialogs.iter().find(|d| d.id == dialog).unwrap().fields.clone();
        draw_section(&mut f);
        // Drawing the settings changes nothing about the model.
        let drawn: Vec<Model> = serde_json::from_value(f["values"]["integrations"]["ai"]["models"].clone()).unwrap();
        assert_eq!(drawn, app.session.prefs().integrations.ai.models, "{id}");
    }
}

#[test]
fn the_order_of_the_list_is_the_panels_and_each_kind_can_be_left_out_of_it() {
    let mut app = app();
    let cloud = |id: &str, api: &str| Model { id: id.into(), ..Model::cloud(Provider::Openai, id, api, false) };
    // As a user's list may be: local and cloud mixed, in no particular order.
    add_model(&mut app, Model { id: "ernie".into(), ..Family::Ernie.preset() });
    add_model(&mut app, cloud("pro", "gemini-3-pro-image-preview"));
    add_model(&mut app, z_image("z"));
    add_model(&mut app, cloud("sunburst", "gpt-image-2.5-sunburst"));
    add_model(&mut app, Model { id: "q21".into(), ..Family::QwenImage21.preset() });
    app.session.ai.set_key(Provider::Openai, "sk-test");
    let panel = |app: &PhotocraftApp| enabled_models(app).into_iter().map(|m| m.id).collect::<Vec<_>>();
    // The panel groups them: cloud first, each kind in the order of the settings.
    assert_eq!(panel(&app), ["pro", "sunburst", "ernie", "z", "q21"]);
    assert_eq!(current_model(&app).map(|m| m.id.as_str()), Some("pro"));

    // Moving a model past its neighbour moves it in the settings, whatever lies between them there.
    let (_, mut f) = dialog_fields(&mut app);
    let ids = |models: &[Value]| models.iter().map(|m| m["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let mut models = f["values"]["integrations"]["ai"]["models"].as_array().cloned().unwrap();
    move_model(&mut models, 3, 1);
    assert_eq!(ids(&models), ["ernie", "sunburst", "pro", "z", "q21"]);
    move_model(&mut models, 0, 3);
    assert_eq!(ids(&models), ["sunburst", "pro", "z", "ernie", "q21"]);
    for (from, to) in [(0, 0), (9, 0), (0, 9), (usize::MAX, 1)] {
        let before = models.clone();
        move_model(&mut models, from, to);
        assert_eq!(models, before, "{from} -> {to}");
    }
    // Recommended Order: cloud before local, each kind best and newest first; an entry that
    // doesn't read as a model stays, at the end.
    models.insert(0, json!({"id": "junk", "provider": "nope"}));
    sort_recommended(&mut models);
    assert_eq!(ids(&models), ["sunburst", "pro", "q21", "z", "ernie", "junk"]);
    models.pop();
    f["values"]["integrations"]["ai"]["models"] = Value::Array(models);

    // Each kind has a switch on the page; switched off, the draft says so and draws.
    for x in Filter::ALL {
        f.insert(FILTER.into(), json!(x.key()));
        draw_section(&mut f);
    }
    assert_eq!((&f["values"]["integrations"]["ai"]["showCloud"], &f["values"]["integrations"]["ai"]["showLocal"]), (&json!(true), &json!(true)));
    f["values"]["integrations"]["ai"]["showCloud"] = json!(false);
    draw_section(&mut f);
    crate::prefs_ui::confirm(&mut app, &f).unwrap();
    let saved = app.session.prefs().integrations.ai.clone();
    assert!(!saved.show_cloud && saved.show_local);
    assert_eq!(saved.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["sunburst", "pro", "q21", "z", "ernie"]);
    // The panel offers the local models only, in their new order, and falls back to its first.
    assert_eq!(panel(&app), ["q21", "z", "ernie"]);
    app.ui.ai_gen.model = "sunburst".into();
    assert_eq!(current_model(&app).map(|m| m.id.as_str()), Some("q21"));
    draw_panel(&mut app);
    // `ai.models` says which are offered, in the panel's order.
    let listed = app.session.execute("ai.models", json!({})).unwrap();
    assert_eq!(listed["listed"], json!(["q21", "z", "ernie"]));
    assert_eq!((&listed["models"][0]["id"], &listed["models"][0]["listed"]), (&json!("sunburst"), &json!(false)));
    // Both off: nothing is offered, and the panel says how to get models.
    app.run("prefs.set", json!({"path": "integrations.ai.showLocal", "value": false})).unwrap();
    assert!(panel(&app).is_empty() && current_model(&app).is_none());
    draw_panel(&mut app);
    assert!(generate(&mut app).is_err());
}

#[test]
fn the_panels_model_list_is_in_sections_and_names_the_models_plainly() {
    let cloud = |id: &str| Model { id: id.into(), ..Model::cloud(Provider::Openai, id, "gpt-image-2", false) };
    // As the panel lists them: cloud first. Each kind that is offered gets its heading.
    let both = [cloud("a"), cloud("b"), z_image("z"), z_image("y")];
    assert_eq!(model_headings(&both), [(0, "Cloud API"), (2, "Local Models")]);
    assert_eq!(model_headings(&both[..2]), [(0, "Cloud API")]);
    assert_eq!(model_headings(&both[2..]), [(0, "Local Models")]);
    assert!(model_headings(&[]).is_empty());
    // The list and the layer a generation lands on name the model, not where it runs.
    let mut app = app();
    add_model(&mut app, Model { name: "GPT Image 2".into(), ..cloud("gpt") });
    add_model(&mut app, z_image("z"));
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    for mode in Mode::ALL {
        app.ui.ai_gen.mode = mode;
        draw_panel(&mut app);
    }
    app.ui.ai_gen.mode = Mode::Create;
    app.ui.ai_gen.model = "z".into();
    app.ui.ai_gen.prompt = "a frog".into();
    let r = generate(&mut app).unwrap();
    let layer = r["layer"].as_u64().unwrap();
    let doc = &app.session.active().unwrap().doc;
    let name = doc.layers.iter().find(|l| l.id.0 == layer).map(|l| l.name.to_lowercase()).unwrap();
    assert!(name.starts_with("[z_image_turbo]") && !name.contains("local") && !name.contains("cloud"), "{name}");
}

/// A ComfyUI that has one model file: Z-Image Turbo as the recommended list names it.
struct OneModel;

impl Transport for OneModel {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        let body: &[u8] = if r.url.ends_with("/models/diffusion_models") {
            br#"["z_image_turbo_int8_convrot.safetensors"]"#
        } else if r.url.contains("/models/") {
            b"[]"
        } else {
            return Ok(Response { status: 404, body: Vec::new() });
        };
        Ok(Response { status: 200, body: body.to_vec() })
    }
}

/// Nothing answers: no ComfyUI here.
struct Nobody;

impl Transport for Nobody {
    fn send(&self, _: &Request) -> photocraft_aigen::Result<Response> {
        Err(photocraft_aigen::Error::Network("connection refused".into()))
    }
}

fn offered(app: &PhotocraftApp) -> Vec<String> {
    enabled_models(app).into_iter().map(|m| m.name).collect()
}

#[test]
fn only_models_that_can_run_are_offered() {
    // A new installation: the recommended models are ticked, but nothing can run yet: no API
    // key, no ComfyUI. The panel says so, and offers the ways to one.
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.ai.transport = Some(Arc::new(Nobody));
    draw_panel(&mut app);
    assert!(offered(&app).is_empty(), "{:?}", offered(&app));
    assert_eq!(local_state(&app), Local::NotSetUp);
    assert!(app.ui.notices.is_empty(), "a ComfyUI that isn't there is not an error");
    assert_eq!(generate(&mut app).unwrap_err(), "No AI model is set up for this.");
    // An OpenAI key: its models are offered, and only those.
    app.session.ai.set_key(Provider::Openai, "sk-test");
    let cloud = offered(&app);
    assert_eq!(cloud, ["GPT Image 2.5 Sunburst", "GPT Image 2.5 Flare"]);
    // A ComfyUI that answers, asked again when the app gets the focus back: the local models
    // whose file it has are offered, after the cloud ones; the others are not.
    app.session.ai.transport = Some(Arc::new(OneModel));
    app.ui.ai_gen.server_asked = -LOOK_AGAIN - 1.0;
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    for focused in [false, true] {
        let mut input = egui::RawInput::default();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
        ctx.run_ui(input, |ui| panel(&mut app, ui, true)).textures_delta.clear();
    }
    assert_eq!(local_state(&app), Local::Ready);
    assert_eq!(offered(&app), ["GPT Image 2.5 Sunburst", "GPT Image 2.5 Flare", "Z-Image Turbo"]);
    // A server that stops answering leaves what it said: its files are still there.
    app.session.ai.transport = Some(Arc::new(Nobody));
    app.ui.ai_gen.server_asked = f64::NEG_INFINITY;
    draw_panel(&mut app);
    assert!(offered(&app).contains(&"Z-Image Turbo".to_string()));
}

#[test]
fn while_local_ai_is_being_set_up_its_models_wait_and_the_panel_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    // The stand-in downloads never finish: the setup stays under way.
    crate::ai_setup_ui::demo::install(&mut app, dir.path(), crate::ai_setup_ui::demo::Scale::Real);
    app.ui.ai_setup.local_worked = true;
    app.session.execute("ai.setup.start", json!({"route": "managed", "models": ["qwen-image-2-1"], "testImage": false})).unwrap();
    assert!(crate::ai_setup_ui::running(&app));
    draw_panel(&mut app);
    assert_eq!(local_state(&app), Local::Installing);
    assert!(offered(&app).is_empty(), "{:?}", offered(&app));
    // A cloud model with its key still works meanwhile; the line under Generate says what waits.
    app.session.ai.set_key(Provider::Gemini, "g-test");
    assert_eq!(offered(&app), ["Nano Banana 2", "Nano Banana Pro"]);
    draw_panel(&mut app);
    // The status the panel reads is the wizard's; once it has some, the line names the progress.
    let line = setup_line(&app).unwrap_or_default();
    assert!(line.is_empty() || line.contains("Local models appear here when it has finished"), "{line}");
    app.session.execute("ai.setup.cancel", json!({})).unwrap();
    let began = std::time::Instant::now();
    while crate::ai_setup_ui::running(&app) {
        assert!(began.elapsed() < std::time::Duration::from_secs(20));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_panels_own_look_at_the_server_ends_quietly() {
    use photocraft_engine::jobs::{JobEvent, JobId, JobOutcome};
    let mut app = app();
    let event = |id: u64, outcome: JobOutcome| JobEvent {
        id: JobId(id),
        command: "ai.comfyModels".into(),
        label: "Connect to ComfyUI".into(),
        document: None,
        outcome,
    };
    // Someone else's job is not the panel's.
    assert!(!on_job_event(&mut app, &event(4, JobOutcome::Failed("refused".into()))));
    app.ui.ai_gen.server_job = Some(9);
    assert!(on_job_event(&mut app, &event(9, JobOutcome::Failed("refused".into()))));
    assert!(app.ui.ai_gen.server_job.is_none() && app.ui.ai_gen.server_files.is_none() && app.ui.notices.is_empty());
    app.ui.ai_gen.server_job = Some(10);
    assert!(on_job_event(&mut app, &event(10, JobOutcome::Done(json!({"folders": {"diffusion_models": ["a.safetensors"]}})))));
    assert_eq!(app.ui.ai_gen.server_files.as_ref().unwrap()["diffusion_models"], ["a.safetensors"]);
}

#[test]
fn a_narrow_panel_keeps_everything_inside_it() {
    let mut app = app();
    add_model(&mut app, Model { id: "q21".into(), ..Family::QwenImage21.preset() });
    app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
    let doc = app.session.active().unwrap().doc.id.0;
    // A layer named after a long prompt (as generated layers are), the source and a reference.
    let long = "[qwen_image_2.1] Create a breathtaking, photorealistic nature scene at dawn";
    app.run("layer.new.layer", json!({"name": long})).unwrap();
    app.run("select.all", json!({})).unwrap();
    app.run("edit.fill", json!({"color": "#336699"})).unwrap();
    app.run("select.deselect", json!({})).unwrap();
    let layer = app.session.active().unwrap().active_layer.unwrap().0;
    (app.ui.ai_gen.mode, app.ui.ai_gen.task, app.ui.ai_gen.edit_model) = (Mode::Edit, EditTask::Change, "q21".into());
    app.ui.ai_gen.prompt = "add @dracco rising from the lake".into();
    let _ = doc;
    let reference = |name: &str| json!({"layer": layer, "name": name});
    app.run("ai.setReferences", json!({"references": [reference("dracco_the_dragon_king"), reference("another_long_name"), reference("and_a_third_one")]}))
        .unwrap();
    app.ui.ai_gen.editing_ref = Some(0);
    // The dock is 250 wide at its narrowest (`panels::DOCK_WIDTH`), its content a little less.
    for width in [234.0, 260.0, 300.0, 360.0] {
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, Default::default());
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 900.0))), ..Default::default() };
        let mut used = 0.0f32;
        for _ in 0..3 {
            ctx.run_ui(input(), |ui| {
                let inner = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 900.0));
                let r = ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| panel(&mut app, ui, false)).response.rect;
                used = r.width();
            })
            .textures_delta
            .clear();
        }
        assert!(used <= width + 0.5, "the panel's content is {used} wide in a panel {width} wide");
    }
}

/// Draw the Generative AI section once on `page`, leaving the dialog wherever the frame took it.
fn draw_page(f: &mut Map<String, Value>, page: &str) {
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    f.insert(PAGE.into(), json!(page));
    for _ in 0..2 {
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            let obj = values.get_mut("integrations").and_then(Value::as_object_mut).unwrap();
            prefs_section(ui, obj, f, 480.0);
            f.insert("values".into(), values);
        })
        .textures_delta
        .clear();
    }
}

#[test]
fn models_that_cant_run_say_why_and_lead_to_what_makes_them_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    crate::ai_setup_ui::demo::install(&mut app, dir.path(), crate::ai_setup_ui::demo::Scale::Small);
    crate::ai_setup_ui::demo::pretend_installed(&app, dir.path()).unwrap();
    // Z-Image Turbo's model file is gone since: it is the wizard's, so it can be installed again.
    let models_dir = dir.path().join("models").join("diffusion_models");
    let zimage = std::fs::read_dir(&models_dir).unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().contains("z_image")).unwrap();
    std::fs::remove_file(zimage).unwrap();
    let defaults = Model::defaults();
    let by = |id: &str| defaults.iter().find(|m| m.id == id).cloned().unwrap();
    let gpt = Model::cloud_presets().into_iter().find(|m| m.provider == Provider::Openai).unwrap();
    let krea = by("krea-2-turbo");
    let listed = vec![gpt.clone(), by("qwen-image-2-1"), by("z-image-turbo"), krea.clone()];
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": listed})).unwrap();
    let (_, mut f) = dialog_fields(&mut app);
    // The dialog reads what the setup installed, with ComfyUI not running.
    after_body(&mut app, &mut f);
    assert!(f.get(FILES).is_none());
    let ai = f["values"]["integrations"]["ai"].as_object().unwrap().clone();
    let standing = Standing::of(&ai, &f);
    let model = |i: usize| model_of(&ai["models"][i]).unwrap();
    assert_eq!(standing.missing(&model(0)), Some(Missing::Key(Provider::Openai)));
    assert_eq!(standing.missing(&model(1)), None, "Qwen Image 2.1 is installed");
    let Some(Missing::Files(Some(wizard))) = standing.missing(&model(2)) else { panic!("{:?}", standing.missing(&model(2))) };
    assert_eq!(wizard["id"], "z-image-turbo");
    // Not one of the wizard's models (in this cut-down manifest): it says so, with nothing to press.
    assert_eq!(standing.missing(&model(3)), Some(Missing::Files(None)));
    draw_page(&mut f, "models");
    // A model that can't run keeps its tick for when it can, and the panel doesn't offer it.
    assert_eq!(f["values"]["integrations"]["ai"]["models"][2]["enabled"], json!(by("z-image-turbo").enabled));
    // A typed key is enough for the page.
    f[KEYS]["openai"] = json!("sk-typed");
    assert_eq!(Standing::of(&ai, &f).missing(&model(0)), None);
    f[KEYS]["openai"] = json!("");

    // "Add an API key" goes to the API Keys page with the provider in front.
    act_on_missing(&mut f, &Missing::Key(Provider::Openai));
    draw_page(&mut f, "models");
    assert_eq!((&f[PAGE], &f[KEY_FOCUS]), (&json!("keys"), &json!("openai")));
    draw_page(&mut f, "keys");
    assert_eq!(f[KEY_FOCUS_ONCE], false, "the field took the keyboard once");
    assert_eq!(key_source(Provider::Openai).0, "https://platform.openai.com/api-keys");
    for p in Provider::ALL.into_iter().filter(|p| !p.is_local()) {
        assert!(key_source(p).0.starts_with("https://") && !key_source(p).2.is_empty(), "{p:?}");
    }

    // Install hands over to the wizard, on its Models screen with the model ticked.
    act_on_missing(&mut f, &Missing::Files(Some(wizard)));
    assert_eq!(f[ACTION], "install:z-image-turbo");
    let id = app.ui.dialogs.iter().find(|d| crate::prefs_ui::is_preferences(&d.fields)).unwrap().id;
    app.ui.dialog_mut(id).unwrap().fields = f.clone();
    let mut fields = app.ui.dialog_mut(id).unwrap().fields.clone();
    after_body(&mut app, &mut fields);
    app.ui.dialog_mut(id).unwrap().fields = fields;
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    ctx.run_ui(Default::default(), |ui| crate::ai_setup_ui::window(&mut app, ui.ctx())).textures_delta.clear();
    assert!(app.ui.dialogs.is_empty(), "the preferences were confirmed first");
    assert!(app.ui.ai_setup.open && app.ui.ai_setup.page == crate::ai_setup_ui::Page::Models);
    assert!(app.ui.ai_setup.models.iter().any(|m| m == "z-image-turbo"), "{:?}", app.ui.ai_setup.models);
}

#[test]
fn local_models_before_local_ai_is_set_up_offer_the_setup() {
    let mut app = app();
    add_model(&mut app, z_image("z"));
    let (_, mut f) = dialog_fields(&mut app);
    // Nothing installed, nothing to start, no server answered.
    f.insert(SETUP.into(), json!({"installed": false, "catalog": [{"id": "z-image-turbo", "main": z_image("z").main_file(), "size": "12.3 GB"}]}));
    let ai = f["values"]["integrations"]["ai"].as_object().unwrap().clone();
    let lack = Standing::of(&ai, &f).missing(&model_of(&ai["models"][0]).unwrap());
    assert!(matches!(&lack, Some(Missing::NoLocalAi(Some(w))) if w["id"] == "z-image-turbo"), "{lack:?}");
    act_on_missing(&mut f, &lack.unwrap());
    assert_eq!(f[ACTION], "install:z-image-turbo");
    // A server that answered is enough to know.
    f.insert(FILES.into(), json!({"diffusion_models": [z_image("z").main_file()]}));
    assert_eq!(Standing::of(&ai, &f).missing(&model_of(&ai["models"][0]).unwrap()), None);
}

#[test]
fn the_dialogs_own_look_at_the_server_is_not_reported_twice() {
    use photocraft_engine::jobs::{JobEvent, JobId, JobOutcome};
    let mut app = app();
    let id = open_settings(&mut app, "");
    app.ui.dialog_mut(id).unwrap().fields.insert(JOB.into(), json!({"id": 21, "action": "peek"}));
    let event = |id: u64| JobEvent {
        id: JobId(id),
        command: "ai.comfyModels".into(),
        label: "Connect to ComfyUI".into(),
        document: None,
        outcome: JobOutcome::Failed("Could not reach ComfyUI".into()),
    };
    assert!(on_job_event(&mut app, &event(21)), "the dialog says how its connect ended");
    assert!(!on_job_event(&mut app, &event(22)));
}

#[test]
fn regions_are_drawn_on_the_canvas_and_sent_to_a_model_trained_on_boxes() {
    use crate::canvas::ToolEvent;
    use crate::state::Tool;
    let mut app = app();
    add_model(&mut app, z_image("z"));
    add_model(&mut app, Model { id: "moss".into(), ..Family::MossImage.preset() });
    app.run("file.new", json!({"width": 200, "height": 100})).unwrap();
    menu(&mut app, PANEL, &json!({"show": true})).unwrap().unwrap();
    app.ui.ai_gen.prompt = "a reading nook".into();
    // Z-Image reads a layout as words: the Regions tab stays shut, the choice kept for later.
    app.ui.ai_gen.model = "z".into();
    app.ui.ai_gen.create_task = CreateTask::Regions;
    app.ui.tool = Tool::Brush;
    draw_panel(&mut app);
    assert!(!regions::tab_open(&app) && !regions::editing(&app) && app.ui.tool == Tool::Brush);
    let down = |x, y| ToolEvent::Down { x, y, pressure: 1.0 };
    assert!(!regions::pointer(&mut app, down(5.0, 5.0)), "the brush keeps the pointer");
    // MOSS Image takes them: the tab opens and gives the pointer to the boxes.
    app.ui.ai_gen.model = "moss".into();
    draw_panel(&mut app);
    assert!(regions::tab_open(&app) && regions::editing(&app) && app.ui.tool == Tool::Move);
    let rect = |app: &PhotocraftApp| app.session.active().unwrap().doc.ai_regions.iter().map(|r| r.rect).collect::<Vec<_>>();
    let gesture = |app: &mut PhotocraftApp, from: [f64; 2], to: [f64; 2]| {
        assert!(regions::pointer(app, down(from[0], from[1])));
        assert!(regions::pointer(app, ToolEvent::Move { x: to[0], y: to[1], pressure: 1.0 }));
        assert!(regions::pointer(app, ToolEvent::Up { x: to[0], y: to[1] }));
    };
    // A drag on empty canvas draws a box, selected and open for typing; a click draws none.
    gesture(&mut app, [10.0, 10.0], [60.0, 40.0]);
    assert_eq!(rect(&app), [photocraft_geom::Rect::new(10, 10, 60, 40)]);
    let id = app.session.active().unwrap().doc.ai_regions[0].id;
    assert_eq!((app.ui.ai_gen.region, app.ui.ai_gen.regions.typing), (Some(id), Some(id)));
    gesture(&mut app, [150.0, 80.0], [150.5, 80.0]);
    assert_eq!((rect(&app).len(), app.ui.ai_gen.region), (1, None));
    // A click selects it; dragging it then moves it (kept on the canvas), and its corner resizes it.
    gesture(&mut app, [20.0, 20.0], [20.0, 20.0]);
    assert_eq!(app.ui.ai_gen.region, Some(id));
    gesture(&mut app, [20.0, 20.0], [30.0, 25.0]);
    assert_eq!(rect(&app), [photocraft_geom::Rect::new(20, 15, 70, 45)]);
    gesture(&mut app, [70.0, 45.0], [300.0, 60.0]);
    assert_eq!(rect(&app), [photocraft_geom::Rect::new(20, 15, 200, 60)]);
    // A second box goes in front.
    gesture(&mut app, [100.0, 70.0], [140.0, 95.0]);
    assert_eq!(rect(&app)[0], photocraft_geom::Rect::new(100, 70, 140, 95));
    // Double-clicking a box types its description.
    app.ui.ai_gen.regions.typing = None;
    assert!(regions::double_click(&mut app, 30.0, 30.0));
    assert_eq!(app.ui.ai_gen.regions.typing, Some(id));
    app.run("ai.region.set", json!({"id": id, "caption": "a green armchair"})).unwrap();
    draw_panel(&mut app);
    // The pencil's editor draws, for a text region too.
    app.ui.ai_gen.regions.editor = Some(id);
    app.run("ai.region.set", json!({"id": id, "kind": "text", "text": "HELLO"})).unwrap();
    draw_panel(&mut app);
    app.run("ai.region.set", json!({"id": id, "kind": "object"})).unwrap();
    // Generate sends the scene and the boxes; the layer keeps them, and Load Prompt brings them
    // back with the tab.
    let r = generate(&mut app).unwrap();
    let sent = r["modelPrompt"].as_str().unwrap();
    assert!(sent.contains("abs_centralized") && sent.contains("a green armchair") && sent.contains("a reading nook"), "{sent}");
    let layer = r["layer"].as_u64().unwrap();
    let kept = rect(&app);
    app.run("ai.setRegions", json!({"regions": []})).unwrap();
    app.ui.ai_gen.create_task = CreateTask::Prompt;
    assert!(load_prompt(&mut app, layer));
    assert_eq!((rect(&app), app.ui.ai_gen.create_task), (kept, CreateTask::Regions));
    // Another tool takes the pointer back; the boxes stay until the tab is left.
    app.ui.tool = Tool::Brush;
    draw_panel(&mut app);
    assert!(regions::tab_open(&app) && !regions::editing(&app));
    assert!(!regions::pointer(&mut app, down(30.0, 30.0)));
    // The state reads and loads like the rest of the panel's.
    let state = serde_json::to_value(&app.ui.ai_gen).unwrap();
    assert_eq!(state["createTask"], "regions");
}

#[test]
fn a_drag_on_a_box_that_is_not_selected_draws_a_new_one() {
    use crate::canvas::ToolEvent;
    let mut app = app();
    add_model(&mut app, Model { id: "moss".into(), ..Family::MossImage.preset() });
    app.ui.ai_gen.model = "moss".into();
    app.run("file.new", json!({"width": 200, "height": 100})).unwrap();
    menu(&mut app, PANEL, &json!({"show": true})).unwrap().unwrap();
    app.ui.ai_gen.create_task = CreateTask::Regions;
    draw_panel(&mut app);
    assert!(regions::editing(&app));
    // Two boxes cover the whole canvas: no empty place is left to start a new one from.
    let full = |x0: i32, x1: i32| json!([x0, 0, x1 - x0, 100]);
    app.run("ai.setRegions", json!({"regions": [{"rect": full(0, 100)}, {"rect": full(100, 200)}]})).unwrap();
    let rects = |app: &PhotocraftApp| app.session.active().unwrap().doc.ai_regions.iter().map(|r| r.rect).collect::<Vec<_>>();
    let ids = app.session.active().unwrap().doc.ai_regions.iter().map(|r| r.id).collect::<Vec<_>>();
    let gesture = |app: &mut PhotocraftApp, from: [f64; 2], to: [f64; 2]| {
        assert!(regions::pointer(app, ToolEvent::Down { x: from[0], y: from[1], pressure: 1.0 }));
        assert!(regions::pointer(app, ToolEvent::Move { x: to[0], y: to[1], pressure: 1.0 }));
        assert!(regions::pointer(app, ToolEvent::Up { x: to[0], y: to[1] }));
    };
    // A click selects the box under it.
    app.ui.ai_gen.region = None;
    gesture(&mut app, [150.0, 50.0], [150.0, 50.0]);
    assert_eq!((app.ui.ai_gen.region, rects(&app).len()), (Some(ids[1]), 2));
    // A drag on the other one, not selected, draws a new box over it and leaves it where it is.
    gesture(&mut app, [20.0, 20.0], [60.0, 50.0]);
    let now = rects(&app);
    assert_eq!(now.len(), 3, "{now:?}");
    assert!(now.contains(&photocraft_geom::Rect::new(20, 20, 60, 50)) && now.contains(&photocraft_geom::Rect::new(0, 0, 100, 100)), "{now:?}");
    let new = app.ui.ai_gen.region.unwrap();
    assert!(!ids.contains(&new));
    // The selected box still moves.
    gesture(&mut app, [30.0, 30.0], [40.0, 35.0]);
    assert!(rects(&app).contains(&photocraft_geom::Rect::new(30, 25, 70, 55)), "{:?}", rects(&app));
}

#[test]
fn the_region_editor_follows_the_selection_and_closes_on_a_press_outside_it() {
    let mut app = app();
    add_model(&mut app, Model { id: "moss".into(), ..Family::MossImage.preset() });
    app.run("file.new", json!({"width": 200, "height": 100})).unwrap();
    menu(&mut app, PANEL, &json!({"show": true})).unwrap().unwrap();
    app.ui.ai_gen.model = "moss".into();
    app.ui.ai_gen.create_task = CreateTask::Regions;
    app.run("ai.setRegions", json!({"regions": [{"rect": [0, 0, 50, 50]}, {"rect": [60, 0, 50, 50]}]})).unwrap();
    let ids: Vec<u32> = app.session.active().unwrap().doc.ai_regions.iter().map(|r| r.id).collect();
    let open = |app: &mut PhotocraftApp| {
        app.ui.ai_gen.region = Some(ids[0]);
        app.ui.ai_gen.regions.editor = Some(ids[0]);
    };
    open(&mut app);
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.regions.editor, Some(ids[0]), "open while its region is selected");
    // Selecting another region (in the list or on the canvas) closes it.
    app.ui.ai_gen.region = Some(ids[1]);
    draw_panel(&mut app);
    assert_eq!(app.ui.ai_gen.regions.editor, None);
    // So does a press anywhere outside it.
    open(&mut app);
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    let far = egui::pos2(5000.0, 5000.0);
    let press = egui::RawInput {
        events: vec![
            egui::Event::PointerMoved(far),
            egui::Event::PointerButton { pos: far, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ],
        ..Default::default()
    };
    ctx.run_ui(Default::default(), |ui| panel(&mut app, ui, true)).textures_delta.clear();
    assert_eq!(app.ui.ai_gen.regions.editor, Some(ids[0]), "no press, still open");
    ctx.run_ui(press, |ui| panel(&mut app, ui, true)).textures_delta.clear();
    assert_eq!(app.ui.ai_gen.regions.editor, None);
}
