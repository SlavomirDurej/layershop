use std::collections::BTreeMap;
use std::sync::Arc;

use photocraft_aigen::{Family, FillMethod, Model, Provider, Request, Response, Transport};
use photocraft_engine::Session;
use serde_json::json;

use super::*;

/// A ComfyUI that renders a green square for any graph.
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
        } else {
            return Ok(Response { status: 404, body: Vec::new() });
        };
        Ok(Response { status: 200, body })
    }
}

fn local(id: &str, family: Family) -> Model {
    Model { id: id.into(), name: id.into(), ..family.preset() }
}

/// An app with the fake server, `models` set up and their files on it, and a 64 x 48 white
/// document.
fn app_with(models: &[Model]) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.ai.transport = Some(Arc::new(Fake));
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": models})).unwrap();
    app.ui.ai_setup.local_worked = true;
    app.ui.ai_gen.server_asked = 0.0;
    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in models.iter().filter(|m| m.provider.is_local()) {
        let folder = if m.checkpoint.trim().is_empty() { "diffusion_models" } else { "checkpoints" };
        files.entry(folder.into()).or_default().push(m.main_file().to_string());
    }
    app.ui.ai_gen.server_files = Some(files);
    app.run("file.new", json!({"width": 64, "height": 48, "background": "white"})).unwrap();
    app
}

fn layers(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().doc.layers.len()
}

/// Open Edit › Fill, choose AI Gen with `prompt`, press OK.
fn fill_with_ai(app: &mut PhotocraftApp, prompt: &str) -> (u64, Result<Value, String>) {
    let id = crate::fill_ui::open(app);
    let d = app.ui.dialog_mut(id).unwrap();
    d.fields.insert("contents".into(), json!(crate::fill_ui::AI_GEN));
    d.fields.insert(crate::fill_ui::PROMPT.into(), json!(prompt));
    (id, crate::dialogs::confirm(app, id))
}

#[test]
fn the_fill_model_is_the_one_chosen_while_it_can_run_else_qwen_image_2_1() {
    let models = [local("z", Family::ZImage), local("fibo", Family::Fibo2), local("qwen", Family::QwenImage21)];
    let mut app = app_with(&models);
    // Nothing chosen: the recommended edit model. Z-Image only creates, so it is never offered.
    assert_eq!(model(&app).map(|m| m.id), Some("qwen".into()));
    assert_eq!(offered(&app).iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["fibo", "qwen"]);
    app.run("prefs.set", json!({"path": "integrations.ai.fill.model", "value": "fibo"})).unwrap();
    assert_eq!(model(&app).map(|m| m.id), Some("fibo".into()));
    // Its files gone from the server: back to the one that can run.
    app.ui.ai_gen.server_files.as_mut().unwrap().insert("diffusion_models".into(), vec![models[2].main_file().to_string()]);
    assert_eq!(model(&app).map(|m| m.id), Some("qwen".into()));
    // Switched off in Manage Models: not offered either.
    let mut off = models.to_vec();
    off[2].enabled = false;
    app.run("prefs.set", json!({"path": "integrations.ai.models", "value": off})).unwrap();
    assert!(model(&app).is_none());
}

#[test]
fn ai_gen_fills_the_selection_on_a_new_layer_masked_by_it() {
    let mut app = app_with(&[local("qwen", Family::QwenImage21)]);
    app.run("select.rect", json!({"x": 16, "y": 12, "width": 16, "height": 16})).unwrap();
    let before = layers(&app);
    let (id, r) = fill_with_ai(&mut app, "  a red kite  ");
    let r = r.unwrap();
    assert!(app.ui.dialog_mut(id).is_none(), "OK closed the dialog");
    assert_eq!((r["mode"].as_str(), r["model"].as_str()), (Some("imgEdit"), Some("qwen")));
    assert_eq!(layers(&app), before + 1);
    let d = app.session.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    assert!(layer.mask.is_some(), "the selection becomes the layer's mask");
    let g = layer.generation.clone().unwrap();
    assert_eq!(g.prompt, "a red kite");
    // The whole picture is what changes, not the empty layer that may be active; no references.
    assert_eq!(g.source, Some(photocraft_doc::GenerationSource::Document(d.doc.id.0)));
    assert!(g.references.is_empty());
    // The dialog opens on AI Gen with the prompt again, to try once more after an undo.
    let saved = crate::fill_ui::fields(&app);
    assert_eq!((saved["contents"].as_str(), saved["prompt"].as_str()), (Some("aiGen"), Some("  a red kite  ")));
}

#[test]
fn an_empty_prompt_asks_for_the_selection_to_be_filled_from_around_it() {
    let mut app = app_with(&[local("qwen", Family::QwenImage21)]);
    let qwen = model(&app).unwrap();
    assert_eq!(params(&app, &qwen, " ")["prompt"].as_str(), Some(photocraft_aigen::fill::DEFAULT_EMPTY_PROMPT));
    app.run("prefs.set", json!({"path": "integrations.ai.fill.emptyPrompt", "value": "Clean background"})).unwrap();
    app.run("prefs.set", json!({"path": "integrations.ai.fill.quality", "value": "draft"})).unwrap();
    app.run("prefs.set", json!({"path": "integrations.ai.fill.context", "value": 1.5})).unwrap();
    let p = params(&app, &qwen, "");
    assert_eq!((p["prompt"].as_str(), p["quality"].as_str(), p["context"].as_f64()), (Some("Clean background"), Some("draft"), Some(1.5)));
}

#[test]
fn a_model_that_does_both_fills_the_way_the_page_says() {
    let both = Model { inpaint: Some(true), ..local("flux2", Family::Flux2) };
    let mut app = app_with(std::slice::from_ref(&both));
    assert_eq!(params(&app, &both, "x")["mode"].as_str(), Some("imgEdit"));
    app.run("prefs.set", json!({"path": "integrations.ai.fill.method", "value": FillMethod::Inpaint.key()})).unwrap();
    app.run("prefs.set", json!({"path": "integrations.ai.fill.strength", "value": 0.8})).unwrap();
    let p = params(&app, &both, "x");
    assert_eq!((p["mode"].as_str(), p["strength"].as_f64(), p.get("source")), (Some("inpaint"), Some(0.8), None));
}

#[test]
fn without_a_model_or_something_to_fill_the_dialog_stays_open_and_says_why() {
    // Only a model that creates: nothing can fill.
    let mut app = app_with(&[local("z", Family::ZImage)]);
    let before = layers(&app);
    let (id, r) = fill_with_ai(&mut app, "a kite");
    assert_eq!(r.unwrap_err(), no_model());
    assert!(app.ui.dialog_mut(id).is_some(), "the dialog stays open");
    assert_eq!(layers(&app), before);
    // An inpainting model with no selection to paint into.
    let mut app = app_with(&[local("fill", Family::Flux1Fill)]);
    let (id, r) = fill_with_ai(&mut app, "a kite");
    assert!(r.is_err());
    assert!(app.ui.dialog_mut(id).is_some());
    assert_eq!(layers(&app), before);
    // A cloud model without its key.
    let gpt = Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) };
    let app = app_with(std::slice::from_ref(&gpt));
    assert!(model(&app).is_none(), "a cloud model without a key can't run");
}

#[test]
fn hostile_dialog_fields_neither_crash_nor_fill() {
    let mut app = app_with(&[local("qwen", Family::QwenImage21)]);
    let id = crate::fill_ui::open(&mut app);
    let d = app.ui.dialog_mut(id).unwrap();
    d.fields.insert("contents".into(), json!(crate::fill_ui::AI_GEN));
    d.fields.insert(crate::fill_ui::PROMPT.into(), json!({"not": "text"}));
    // A prompt that isn't text is an empty one: the fill still runs, with the empty-prompt text.
    let r = crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(r["modelPrompt"].as_str().map(|p| p.contains("surroundings")), Some(true), "{r}");
    // A remembered prompt that isn't text, or is absurdly long, is not reopened.
    let long = "x".repeat(20_000);
    for bad in [json!(7), json!(long)] {
        app.session.prefs.edit(|p| p.dialogs.insert(crate::fill_ui::COMMAND.into(), json!({"contents": "aiGen", "prompt": bad})));
        assert_eq!(crate::fill_ui::fields(&app)["prompt"].as_str(), Some(""));
    }
}

#[test]
fn change_opens_preferences_on_generative_fill() {
    let mut app = app_with(&[local("qwen", Family::QwenImage21)]);
    let id = crate::fill_ui::open(&mut app);
    app.ui.dialog_mut(id).unwrap().fields.insert("__fillAiSettings".into(), json!(true));
    crate::fill_ui::open_requested(&mut app, id);
    let prefs = app.ui.dialogs.iter().find(|d| crate::prefs_ui::is_preferences(&d.fields)).expect("Preferences opened over Fill");
    assert_eq!(prefs.fields.get(super::super::PAGE).and_then(Value::as_str), Some(PAGE));
    assert!(app.ui.dialog_mut(id).is_some(), "Fill stays open under it");
    // Asked once: drawing again opens no second one.
    crate::fill_ui::open_requested(&mut app, id);
    assert_eq!(app.ui.dialogs.iter().filter(|d| crate::prefs_ui::is_preferences(&d.fields)).count(), 1);
}

#[test]
fn the_page_lists_the_models_that_can_fill_and_keeps_the_one_clicked() {
    let gpt = Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) };
    let missing = Model { unet: "not_there.safetensors".into(), ..local("fibo", Family::Fibo2) };
    let models = [local("z", Family::ZImage), gpt, missing, local("qwen", Family::QwenImage21), Model { enabled: false, ..local("ming", Family::MingImage) }];
    let mut ai = serde_json::to_value(photocraft_aigen::Settings { models: models.to_vec(), ..photocraft_aigen::Settings::default_without_models() })
        .unwrap()
        .as_object()
        .cloned()
        .unwrap();
    let mut f = Map::new();
    // The server has Qwen's file (not FIBO's), and an OpenAI key was typed in this dialog.
    f.insert(super::super::FILES.into(), json!({"diffusion_models": [models[3].main_file()]}));
    f.insert(super::super::KEYS.into(), json!({"openai": "sk-test"}));
    let ids = |ai: &Map<String, Value>, f: &Map<String, Value>| ready(ai, f).into_iter().map(|l| (l.index, l.model.id)).collect::<Vec<_>>();
    assert_eq!(ids(&ai, &f), [(1, "gpt".to_string()), (3, "qwen".to_string())]);
    // Cloud models hidden in Manage Models: only Qwen.
    ai.insert("showCloud".into(), json!(false));
    assert_eq!(ids(&ai, &f), [(3, "qwen".to_string())]);
    ai.insert("showCloud".into(), json!(true));
    // Drawing the page keeps the settings it shows, and their defaults where there were none.
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, Default::default());
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| page(ui, &mut ai, &mut f)).textures_delta.clear();
    }
    let s: FillSettings = serde_json::from_value(ai["fill"].clone()).unwrap();
    assert_eq!(s, FillSettings::default());
    // A corrupt fill setting or model entry is drawn over, not a crash.
    ai.insert("fill".into(), json!("garbage"));
    ai.get_mut("models").unwrap().as_array_mut().unwrap().push(json!(42));
    ctx.run_ui(Default::default(), |ui| page(ui, &mut ai, &mut f)).textures_delta.clear();
    assert_eq!(serde_json::from_value::<FillSettings>(ai["fill"].clone()).unwrap(), FillSettings::default());
}
