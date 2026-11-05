use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::*;
use crate::Response;
use crate::enhance::Purpose;

/// A local engine: queues every workflow as `p1`, has no history for it `pending` times, then
/// the history `done` gives. Keeps the requests.
struct Engine {
    queued: Value,
    done: Value,
    pending: Mutex<usize>,
    seen: Mutex<Vec<Request>>,
}

impl Engine {
    fn new(done: Value) -> Self {
        Self { queued: json!({"prompt_id": "p1", "number": 1}), done, pending: Mutex::new(2), seen: Mutex::new(Vec::new()) }
    }

    fn wrote(text: &str) -> Self {
        Self::new(json!({"p1": {"status": {"status_str": "success", "completed": true}, "outputs": {"4": {"text": [text]}}}}))
    }

    fn urls(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|r| r.url.clone()).collect()
    }
}

impl Transport for Engine {
    fn send(&self, r: &Request) -> Result<Response> {
        self.seen.lock().unwrap().push(r.clone());
        let body = if r.url.ends_with("/prompt") {
            self.queued.clone()
        } else if r.url.contains("/history/") {
            let mut pending = self.pending.lock().unwrap();
            if *pending > 0 {
                *pending -= 1;
                json!({})
            } else {
                self.done.clone()
            }
        } else {
            json!({})
        };
        Ok(Response { status: 200, body: body.to_string().into_bytes() })
    }
}

fn ask(prompt: &str) -> Ask {
    Ask { prompt: prompt.into(), purpose: Purpose::Create, image_model: String::new(), style: None }
}

fn local(model: &str) -> EnhanceSettings {
    let mut s = EnhanceSettings { provider: TextProvider::Local, ..EnhanceSettings::default() };
    if !model.is_empty() {
        s.models.insert("local".into(), model.into());
    }
    s
}

fn run(engine: &Engine, s: &EnhanceSettings, ask: &Ask) -> Result<Enhanced> {
    enhance(engine, "http://127.0.0.1:8190/", s, ask, 7, &Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|_| {} })
}

#[test]
fn the_models_are_the_three_offered_with_the_smallest_first() {
    assert_eq!(MODELS.map(|m| m.id), ["gemma-4-e2b", "qwen3-4b", "gemma-4-e4b"]);
    assert_eq!(DEFAULT, MODELS[0].id);
    assert_eq!(TextProvider::Local.default_model(), DEFAULT);
    assert_eq!(find(" qwen3-4b ").map(|m| m.file), Some("qwen_3_4b_fp8_mixed.safetensors"));
    assert_eq!(find("nope"), None);
    assert_eq!(MODELS[2].manifest_key(), "text_encoders/gemma4_e4b_it_fp8_scaled.safetensors");
    assert!(MODELS.windows(2).all(|w| w[0].size <= w[1].size), "smallest first");
    // Local is a choice of its own: no key, not one of the cloud services.
    assert_eq!(TextProvider::from_key("local"), Some(TextProvider::Local));
    assert!(!TextProvider::ALL.contains(&TextProvider::Local));
    assert!(TextProvider::keys().ends_with("|local"));
}

#[test]
fn the_answer_is_capped_by_the_length() {
    assert_eq!(max_tokens(120), 256);
    assert_eq!(max_tokens(0), 64);
    assert_eq!(max_tokens(u32::MAX), 1024);
    assert!(max_tokens(400) >= 400 + 64);
}

#[test]
fn the_workflow_loads_the_model_writes_without_thinking_and_returns_text() {
    let s = EnhanceSettings { words: 60, ..local("gemma-4-e4b") };
    let style = crate::style::find("anime");
    let w = workflow(&MODELS[2], &s, &Ask { purpose: Purpose::Change, style, ..ask("make the cat \"Luna\" red, like @Mia") }, 9);
    let nodes = w.as_object().unwrap();
    let class = |c: &str| nodes.values().find(|n| n["class_type"] == c).unwrap_or_else(|| panic!("no {c} in {w}"));
    assert_eq!(nodes.len(), 4, "{w}");
    assert_eq!(class("CLIPLoader")["inputs"]["clip_name"], "gemma4_e4b_it_fp8_scaled.safetensors");
    let generate = &class("TextGenerate")["inputs"];
    assert_eq!(generate["thinking"], false);
    assert_eq!(generate["max_length"], max_tokens(60));
    assert_eq!(generate["sampling_mode.seed"], 9);
    // The style's rules follow the prompt too: a small model heeds them there.
    let prompt = generate["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("make the cat \"Luna\" red, like @Mia\n\n(Write it for the Anime style") && prompt.contains("photorealistic"), "{prompt}");
    let plain = workflow(&MODELS[0], &s, &ask("a cat"), 1);
    assert!(plain.as_object().unwrap().values().any(|n| n["inputs"]["prompt"] == "a cat"), "no style: the prompt alone");
    // The instructions are the cloud services': the task, the length and the style are in them.
    let system = class("PrimitiveStringMultiline")["inputs"]["value"].as_str().unwrap().to_string();
    assert!(system.contains("@mentions") && system.contains("Keep it an instruction") && system.contains("at most about 60 words"), "{system}");
    assert!(system.contains("the Anime preset replaces them"), "{system}");
    assert!(class("PreviewAny")["inputs"]["source"].is_array(), "the text goes to Preview as Text");
    assert!(!nodes.values().any(|n| ["UNETLoader", "KSampler", "VAEDecode"].contains(&n["class_type"].as_str().unwrap())), "no image model runs");
}

#[test]
fn enhances_on_the_engine_and_reads_the_text_back() {
    let engine = Engine::wrote("\"A ginger cat asleep on a sunlit windowsill.\"");
    let done = run(&engine, &local(""), &ask("cat window")).unwrap();
    assert_eq!(done, Enhanced { prompt: "A ginger cat asleep on a sunlit windowsill.".into(), provider: TextProvider::Local, model: "gemma-4-e2b".into() });
    let urls = engine.urls();
    assert_eq!(urls.first().map(String::as_str), Some("http://127.0.0.1:8190/prompt"));
    assert!(urls.iter().filter(|u| u.ends_with("/history/p1")).count() >= 3, "{urls:?}");
    let queued: Value = serde_json::from_slice(&engine.seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(queued["client_id"], "photocraft-enhance");
    // A copied style sentence is taken out, as from a cloud service.
    let anime = crate::style::find("anime").unwrap();
    let engine = Engine::wrote(&format!("An old fisherman on a pier. {}", anime.prompt));
    assert_eq!(run(&engine, &local("qwen3-4b"), &Ask { style: Some(anime), ..ask("fisherman") }).unwrap().prompt, "An old fisherman on a pier.");
}

#[test]
fn a_missing_model_a_failure_and_bad_settings_are_errors() {
    // Load CLIP lists only the files the engine has.
    let engine = Engine { queued: json!({"error": {"message": "Prompt outputs failed validation"}, "node_errors": {"1": {"class_type": "CLIPLoader", "errors": [{"message": "Value not in list", "details": "clip_name: 'qwen_3_4b_fp8_mixed.safetensors' not in [...]"}]}}}), ..Engine::wrote("x") };
    let e = run(&engine, &local("qwen3-4b"), &ask("cat")).unwrap_err().to_string();
    assert!(e.contains("Qwen3 4B is not installed") && e.contains("Local Models"), "{e}");
    let failed = Engine::new(json!({"p1": {"status": {"status_str": "error", "messages": [["execution_error", {"node_type": "TextGenerate", "exception_message": "out of memory"}]]}}}));
    assert!(run(&failed, &local(""), &ask("cat")).unwrap_err().to_string().contains("TextGenerate: out of memory"));
    let empty = Engine::wrote("   ");
    assert!(matches!(run(&empty, &local(""), &ask("cat")), Err(Error::BadResponse(_))));
    let nothing = Engine::new(json!({"p1": {"status": {"status_str": "success", "completed": true}, "outputs": {}}}));
    assert!(run(&nothing, &local(""), &ask("cat")).unwrap_err().to_string().contains("no text"));
    for (s, a) in [(local("gpt-5"), ask("cat")), (local(""), ask("   ")), (local(""), ask(&"x".repeat(MAX_PROMPT + 1)))] {
        let engine = Engine::wrote("x");
        assert!(matches!(run(&engine, &s, &a), Err(Error::Config(_))));
        assert!(engine.urls().is_empty(), "nothing is queued");
    }
    let engine = Engine::wrote("x");
    let bad_url = enhance(&engine, "localhost", &local(""), &ask("cat"), 1, &Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|_| {} });
    assert!(matches!(bad_url, Err(Error::Config(_))));
}

#[test]
fn a_cancel_stops_the_engine_and_the_timeout_gives_up() {
    let engine = Engine { pending: Mutex::new(usize::MAX), ..Engine::wrote("x") };
    let stop = AtomicBool::new(false);
    let sleep = |_| stop.store(true, Ordering::Relaxed);
    let control = Control { cancelled: &|| stop.load(Ordering::Relaxed), progress: &|_, _| {}, sleep: &sleep };
    assert!(matches!(enhance(&engine, "http://127.0.0.1:8190", &local(""), &ask("cat"), 1, &control), Err(Error::Cancelled)));
    let urls = engine.urls();
    assert!(urls.iter().any(|u| u.ends_with("/interrupt")) && urls.iter().any(|u| u.ends_with("/queue")), "{urls:?}");
    // Never done: given up at the timeout, and the engine is told to stop.
    let engine = Engine { pending: Mutex::new(usize::MAX), ..Engine::wrote("x") };
    let s = EnhanceSettings { timeout_seconds: 5, ..local("") };
    let e = run(&engine, &s, &ask("cat")).unwrap_err().to_string();
    assert!(e.contains("did not finish within 5 seconds"), "{e}");
    assert!(engine.urls().iter().any(|u| u.ends_with("/interrupt")));
}

#[test]
fn text_history_is_read_safely() {
    assert_eq!(comfy::parse_text_history(b"{}", "p1").unwrap(), None);
    assert_eq!(comfy::parse_text_history(br#"{"p1": {"status": {"completed": false}, "outputs": {}}}"#, "p1").unwrap(), None);
    assert_eq!(comfy::parse_text_history(br#"{"p1": {"outputs": {"9": {"text": [5, "hi"]}}}}"#, "p1").unwrap().as_deref(), Some("hi"));
    for hostile in [&br#"{"p1": 5}"#[..], br#"{"p1": {"outputs": {"9": {"text": "x"}}}}"#, br#"{"p1": {"outputs": []}}"#] {
        assert_eq!(comfy::parse_text_history(hostile, "p1").unwrap(), None, "{}", String::from_utf8_lossy(hostile));
    }
    assert!(comfy::parse_text_history(b"\xff", "p1").is_err());
}

#[test]
fn dropped_quotes_go_back_and_the_reminder_follows_the_style() {
    use crate::enhance::{keep_quotes, reminder};
    assert_eq!(keep_quotes(r#"a sign saying "OPEN""#, "A sign that reads OPEN in red."), r#"A sign that reads "OPEN" in red."#);
    assert_eq!(keep_quotes(r#"a sign saying "OPEN""#, r#"A sign reading "OPEN"."#), r#"A sign reading "OPEN"."#);
    assert_eq!(keep_quotes("a sign saying \u{201c}Café\u{201d}", "It says Café."), r#"It says "Café"."#);
    assert_eq!(keep_quotes("no quotes", "Nothing to do."), "Nothing to do.");
    assert_eq!(keep_quotes(r#""unclosed"#, "unclosed"), "unclosed");
    assert_eq!(keep_quotes(r#""gone""#, "Something else."), "Something else.", "words the answer dropped aren't added");
    assert_eq!(reminder(&ask("x")), None);
    let mono = reminder(&Ask { style: crate::style::find("black-and-white"), ..ask("x") }).unwrap();
    assert!(mono.contains("Black & White") && mono.contains("name no colours"), "{mono}");
    let cinematic = reminder(&Ask { style: crate::style::find("cinematic"), ..ask("x") }).unwrap();
    assert!(!cinematic.contains(" cinematic,") && !cinematic.contains(", cinematic."), "the preset's own words are allowed: {cinematic}");
}
