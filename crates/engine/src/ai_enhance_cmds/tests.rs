use std::sync::{Arc, Mutex};

use photocraft_aigen::{Request, Response, Transport};
use serde_json::{Value, json};

use crate::Session;

/// Answers chat requests with one rewritten prompt, and model listings with two models,
/// keeping what it was asked.
#[derive(Default)]
struct FakeText {
    seen: Mutex<Vec<Request>>,
}

impl Transport for FakeText {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        self.seen.lock().unwrap().push(r.clone());
        let body = if r.url.ends_with("/models") || r.url.contains("/models?") {
            json!({"data": [{"id": "claude-opus-5-5"}, {"id": "claude-haiku-5-5"}]})
        } else if r.url.contains("anthropic.com") {
            json!({"content": [{"type": "text", "text": "A ginger cat asleep on a sunlit windowsill, soft morning light"}]})
        } else {
            json!({"choices": [{"message": {"content": "\"A ginger cat, golden hour\""}}]})
        };
        Ok(Response { status: 200, body: body.to_string().into_bytes() })
    }
}

fn session(fake: &Arc<FakeText>) -> Session {
    let mut s = Session::new();
    s.ai.transport = Some(fake.clone());
    s
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[test]
fn enhances_with_the_chosen_service_and_model() {
    let fake = Arc::new(FakeText::default());
    let mut s = session(&fake);
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": " ak-1 "})).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.enhance", "value": {"provider": "anthropic", "models": {"anthropic": "claude-opus-5-5"}, "timeoutSeconds": 30}}))
        .unwrap();
    // No document is needed: the prompt is the panel's.
    let r = s.execute("ai.enhancePrompt", json!({"prompt": "  cat window ", "mode": "imgEdit"})).unwrap();
    assert_eq!(
        r,
        json!({"prompt": "A ginger cat asleep on a sunlit windowsill, soft morning light", "original": "cat window", "provider": "anthropic", "textModel": "claude-opus-5-5", "style": null})
    );
    let seen = fake.seen.lock().unwrap();
    let sent = seen.last().unwrap();
    assert_eq!(sent.url, "https://api.anthropic.com/v1/messages");
    assert!(sent.headers.iter().any(|(k, v)| k == "x-api-key" && v == "ak-1"));
    assert_eq!(body(sent)["messages"][0]["content"], "cat window");
    assert!(body(sent)["system"].as_str().unwrap().contains("Keep it an instruction"), "the task is passed on");
    assert_eq!(sent.timeout_seconds, 32);
}

#[test]
fn params_override_the_preferences() {
    let fake = Arc::new(FakeText::default());
    let mut s = session(&fake);
    s.execute("ai.setKey", json!({"provider": "openrouter", "key": "or-1"})).unwrap();
    let model = serde_json::to_value(photocraft_aigen::Model { id: "z".into(), ..photocraft_aigen::Family::ZImage.preset() }).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
    let r = s.execute("ai.enhancePrompt", json!({"prompt": "cat", "provider": "openrouter", "textModel": "x/y:free", "model": "z", "timeout": 5})).unwrap();
    assert_eq!((r["prompt"].as_str(), r["provider"].as_str(), r["textModel"].as_str()), (Some("A ginger cat, golden hour"), Some("openrouter"), Some("x/y:free")));
    let seen = fake.seen.lock().unwrap();
    let sent = seen.last().unwrap();
    assert_eq!(sent.url, "https://openrouter.ai/api/v1/chat/completions");
    assert_eq!(body(sent)["model"], "x/y:free");
    assert!(body(sent)["messages"][0]["content"].as_str().unwrap().contains("The image model is Z-Image"), "{}", body(sent));
    drop(seen);
    // A style preset: the prompt is written for it, and the answer says which it was.
    let r = s.execute("ai.enhancePrompt", json!({"prompt": "a girl in a café", "provider": "openrouter", "style": "Anime"})).unwrap();
    assert_eq!(r["style"], "anime");
    let seen = fake.seen.lock().unwrap();
    let system = body(seen.last().unwrap())["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(system.contains("the Anime preset replaces them") && system.contains("It is not a photograph"), "{system}");
}

#[test]
fn bad_params_and_missing_keys_are_errors() {
    let fake = Arc::new(FakeText::default());
    let mut s = session(&fake);
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": "ak"})).unwrap();
    for p in [
        json!({}),
        json!({"prompt": "   "}),
        json!({"prompt": 5}),
        json!({"prompt": "x".repeat(photocraft_aigen::enhance::MAX_PROMPT + 1)}),
        json!({"prompt": "cat", "mode": "paint"}),
        json!({"prompt": "cat", "mode": 1}),
        json!({"prompt": "cat", "provider": "comfy"}),
        json!({"prompt": "cat", "provider": "anthropic", "textModel": ""}),
        json!({"prompt": "cat", "provider": "anthropic", "textModel": 3}),
        json!({"prompt": "cat", "provider": "anthropic", "timeout": 1}),
        json!({"prompt": "cat", "provider": "anthropic", "timeout": -5}),
        json!({"prompt": "cat", "provider": "anthropic", "timeout": "60"}),
        json!({"prompt": "cat", "provider": "anthropic", "model": 7}),
        json!({"prompt": "cat", "provider": "anthropic", "textModel": "a b"}),
        json!({"prompt": "cat", "provider": "anthropic", "style": "nope"}),
        json!({"prompt": "cat", "provider": "anthropic", "style": 3}),
    ] {
        assert!(s.execute("ai.enhancePrompt", p.clone()).is_err(), "{p}");
    }
    // A provider without a key says where to add one, and nothing is sent.
    if std::env::var("OPENROUTER_API_KEY").is_err() {
        let e = s.execute("ai.enhancePrompt", json!({"prompt": "cat", "provider": "openrouter"})).unwrap_err().to_string();
        assert!(e.contains("OpenRouter needs an API key") && e.contains("API Keys"), "{e}");
    }
    assert!(fake.seen.lock().unwrap().is_empty(), "nothing that was refused was sent");
    // Without a network (the browser, a plain headless session) it says so.
    let mut offline = Session::new();
    assert!(offline.execute("ai.enhancePrompt", json!({"prompt": "cat"})).is_err());
    assert!(offline.execute("ai.enhanceModels", json!({})).is_err());
}

#[test]
fn keys_for_enhancement_only_are_kept_with_the_others() {
    let mut s = Session::new();
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": "ak"})).unwrap();
    s.execute("ai.setKey", json!({"provider": "openrouter", "key": "or"})).unwrap();
    s.execute("ai.setKey", json!({"provider": "xai", "key": "xk"})).unwrap();
    assert!(s.execute("ai.setKey", json!({"provider": "comfy", "key": "x"})).is_err());
    assert!(s.execute("ai.setKey", json!({"provider": "anthropic", "key": "a\nb"})).is_err());
    let keys = s.execute("ai.keys", json!({})).unwrap();
    assert_eq!((keys["anthropic"].as_bool(), keys["openrouter"].as_bool(), keys["xai"].as_bool()), (Some(true), Some(true), Some(true)));
    // xAI's image key is the one Grok enhances with.
    assert_eq!(s.ai.text_key(photocraft_aigen::TextProvider::Xai), "xk");
    // Saved and restored with the others.
    let saved = s.ai.keys_json();
    let mut again = Session::new();
    again.ai.load_keys_json(&saved);
    assert_eq!(again.ai.text_key(photocraft_aigen::TextProvider::Anthropic), "ak");
    assert_eq!(again.ai.text_key(photocraft_aigen::TextProvider::Openrouter), "or");
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": ""})).unwrap();
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        assert_eq!(s.ai.text_key(photocraft_aigen::TextProvider::Anthropic), "");
    }
    // Hostile saved text loads nothing odd.
    let mut odd = Session::new();
    odd.ai.load_keys_json(r#"{"anthropic": 5, "openrouter": "a\u0000b"}"#);
    if std::env::var("OPENROUTER_API_KEY").is_err() {
        assert_eq!(odd.ai.text_key(photocraft_aigen::TextProvider::Openrouter), "");
    }
}

#[test]
fn lists_a_services_models() {
    let fake = Arc::new(FakeText::default());
    let mut s = session(&fake);
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": "ak"})).unwrap();
    let r = s.execute("ai.enhanceModels", json!({"provider": "anthropic"})).unwrap();
    assert_eq!(r, json!({"provider": "anthropic", "models": ["claude-opus-5-5", "claude-haiku-5-5"], "default": "claude-haiku-5-5"}));
    assert!(s.execute("ai.enhanceModels", json!({"provider": "nope"})).is_err());
}

#[test]
fn runs_as_a_background_job() {
    let fake = Arc::new(FakeText::default());
    let mut s = session(&fake);
    s.execute("ai.setKey", json!({"provider": "anthropic", "key": "ak"})).unwrap();
    let crate::jobs::Started::Job(id) = s.start("ai.enhancePrompt", json!({"prompt": "cat", "provider": "anthropic"})).unwrap() else {
        panic!("expected a job");
    };
    assert!(s.jobs().iter().any(|j| j.id == id && j.label == super::LABEL && j.document.is_none()));
    let r = s.wait_job(id).unwrap();
    assert_eq!(r["provider"], "anthropic");
}

/// A local engine (ComfyUI): queues the workflow, then has its text ready at once. Keeps what
/// it was sent.
#[derive(Default)]
struct FakeEngine {
    seen: Mutex<Vec<Request>>,
}

impl Transport for FakeEngine {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        self.seen.lock().unwrap().push(r.clone());
        let body = if r.url.ends_with("/prompt") {
            json!({"prompt_id": "p1"})
        } else if r.url.contains("/history/") {
            json!({"p1": {"status": {"status_str": "success", "completed": true}, "outputs": {"4": {"text": ["A ginger cat on a sofa holding a sign that reads OPEN."]}}}})
        } else {
            json!({})
        };
        Ok(Response { status: 200, body: body.to_string().into_bytes() })
    }
}

#[test]
fn a_local_model_enhances_on_the_engine_without_a_key() {
    let fake = Arc::new(FakeEngine::default());
    let mut s = Session::new();
    s.ai.transport = Some(fake.clone());
    s.execute("prefs.set", json!({"path": "integrations.ai.comfyUrl", "value": "http://127.0.0.1:8190"})).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.enhance", "value": {"provider": "local", "models": {"local": "qwen3-4b"}}})).unwrap();
    let r = s.execute("ai.enhancePrompt", json!({"prompt": "cat on sofa, sign \"OPEN\"", "style": "anime"})).unwrap();
    assert_eq!((r["provider"].as_str(), r["textModel"].as_str(), r["style"].as_str()), (Some("local"), Some("qwen3-4b"), Some("anime")));
    // The quotes the model dropped are put back.
    assert_eq!(r["prompt"], "A ginger cat on a sofa holding a sign that reads \"OPEN\".");
    let seen = fake.seen.lock().unwrap();
    let queued = body(&seen[0]);
    assert_eq!(seen[0].url, "http://127.0.0.1:8190/prompt");
    let nodes = queued["prompt"].as_object().unwrap();
    assert!(nodes.values().any(|n| n["class_type"] == "CLIPLoader" && n["inputs"]["clip_name"] == "qwen_3_4b_fp8_mixed.safetensors"));
    assert!(nodes.values().any(|n| n["class_type"] == "TextGenerate" && n["inputs"]["thinking"] == false));
    assert!(nodes.values().any(|n| n["class_type"] == "PreviewAny"));
    drop(seen);
    // Only the models offered; the provider param picks local too.
    assert!(s.execute("ai.enhancePrompt", json!({"prompt": "cat", "textModel": "gpt-5"})).is_err());
    let r = s.execute("ai.enhancePrompt", json!({"prompt": "cat", "provider": "local", "textModel": "gemma-4-e4b"})).unwrap();
    assert_eq!(r["textModel"], "gemma-4-e4b");
}

#[test]
fn local_models_are_listed_and_installing_needs_the_desktop_app() {
    let mut s = Session::new();
    let r = s.execute("ai.enhanceLocalModels", json!({})).unwrap();
    let ids: Vec<&str> = r["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["gemma-4-e2b", "qwen3-4b", "gemma-4-e4b"]);
    assert_eq!((r["default"].as_str(), r["chosen"].as_str()), (Some("gemma-4-e2b"), Some("gemma-4-e2b")));
    assert!(r["models"].as_array().unwrap().iter().all(|m| m["installed"] == false && m["folder"] == "text_encoders" && m["size"].as_u64().unwrap() > 1_000_000_000));
    for p in [json!({}), json!({"model": 3}), json!({"model": "llama"}), json!({"model": "gemma-4-e2b"})] {
        assert!(s.execute("ai.enhanceInstall", p.clone()).is_err(), "{p}");
    }
}
