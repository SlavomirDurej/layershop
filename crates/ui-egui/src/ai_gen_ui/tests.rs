use super::*;
use photocraft_aigen::{Request, Response, Transport};
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
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| {
            let mut values = f.get("values").cloned().unwrap();
            let obj = values.get_mut("integrations").and_then(Value::as_object_mut).unwrap();
            prefs_section(ui, obj, f);
            f.insert("values".into(), values);
        })
        .textures_delta
        .clear();
    }
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
