use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

/// Answers every request with `status` and `body`, after `delay`, keeping the requests.
struct Fake {
    status: u16,
    body: String,
    delay: Duration,
    seen: Mutex<Vec<Request>>,
}

impl Fake {
    fn new(status: u16, body: Value) -> Arc<Self> {
        Self::slow(status, body, Duration::ZERO)
    }

    fn slow(status: u16, body: Value, delay: Duration) -> Arc<Self> {
        Arc::new(Self { status, body: body.to_string(), delay, seen: Mutex::new(Vec::new()) })
    }
}

impl Transport for Fake {
    fn send(&self, request: &Request) -> Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        std::thread::sleep(self.delay);
        Ok(Response { status: self.status, body: self.body.clone().into_bytes() })
    }
}

fn control<'a>(cancelled: &'a (dyn Fn() -> bool + Sync), progress: &'a (dyn Fn(f32, &str) + Sync)) -> Control<'a> {
    Control { cancelled, progress, sleep: &|_| {} }
}

fn ask(prompt: &str) -> Ask {
    Ask { prompt: prompt.into(), purpose: Purpose::Create, image_model: String::new(), style: None }
}

fn settings(provider: TextProvider) -> EnhanceSettings {
    EnhanceSettings { provider, ..EnhanceSettings::default() }
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

#[test]
fn settings_default_and_load_from_json() {
    let s = EnhanceSettings::default();
    assert_eq!(s.provider, TextProvider::Openai);
    assert_eq!(s.timeout(), u64::from(DEFAULT_TIMEOUT));
    assert_eq!(s.model(TextProvider::Anthropic), "claude-haiku-5-5");
    let s: EnhanceSettings = serde_json::from_value(json!({"provider": "anthropic", "models": {"anthropic": " claude-opus-5-5 ", "xai": ""}, "timeoutSeconds": 99999})).unwrap();
    assert_eq!(s.provider, TextProvider::Anthropic);
    assert_eq!(s.model(TextProvider::Anthropic), "claude-opus-5-5");
    assert_eq!(s.model(TextProvider::Xai), TextProvider::Xai.default_model(), "an empty choice is the default");
    assert_eq!(s.timeout(), u64::from(MAX_TIMEOUT));
    assert_eq!(EnhanceSettings { timeout_seconds: 0, ..s }.timeout(), u64::from(MIN_TIMEOUT));
    // Every provider round-trips through its key.
    for p in TextProvider::ALL {
        assert_eq!(TextProvider::from_key(p.key()), Some(p));
        assert_eq!(serde_json::to_value(p).unwrap(), json!(p.key()));
    }
    assert_eq!(TextProvider::keys(), "openrouter|openai|xai|gemini|anthropic|local");
    // The shared keys are the image providers' own.
    assert_eq!(TextProvider::Xai.image_provider().map(Provider::key), Some("xai"));
    assert_eq!(TextProvider::Anthropic.image_provider(), None);
}

#[test]
fn instructions_follow_the_task_and_the_settings() {
    let s = EnhanceSettings::default();
    let create = instructions(&s, &ask("a cat"));
    assert!(create.starts_with(DEFAULT_INSTRUCTIONS) && create.contains("a new image"));
    let change = instructions(&s, &Ask { purpose: Purpose::Change, image_model: "Qwen Image 2.1\n".into(), ..ask("make it red") });
    assert!(change.contains("Keep it an instruction") && change.ends_with("The image model is Qwen Image 2.1."), "{change}");
    let own = EnhanceSettings { instructions: "  Write haiku.  ".into(), ..s.clone() };
    let text = instructions(&own, &Ask { purpose: Purpose::Fill, ..ask("x") });
    assert!(text.starts_with("Write haiku. Keep it to at most about 120 words.\n\n") && text.contains("selected area"), "{text}");
    // The length is its own setting, kept in its range, and holds with the user's instructions.
    assert!(instructions(&EnhanceSettings { words: 60, ..own.clone() }, &ask("x")).contains("at most about 60 words"));
    assert!(instructions(&EnhanceSettings { words: 0, ..s.clone() }, &ask("x")).contains(&format!("at most about {MIN_WORDS} words")));
    assert!(instructions(&EnhanceSettings { words: 99_999, ..s.clone() }, &ask("x")).contains(&format!("at most about {MAX_WORDS} words")));
    // The settings show the built-in instructions when there are none of the user's, and keep
    // them as none, so they follow the app's.
    assert_eq!(s.instructions_text(), DEFAULT_INSTRUCTIONS);
    assert_eq!(own.instructions_text().trim(), "Write haiku.");
    let mut kept = own.clone();
    kept.set_instructions(&format!("  {DEFAULT_INSTRUCTIONS}\n"));
    assert_eq!(kept.instructions, "");
    kept.set_instructions(" Mine. ");
    assert_eq!(kept.instructions, "Mine.");
    // Older settings without a length read as the default.
    let old: EnhanceSettings = serde_json::from_value(json!({"provider": "xai"})).unwrap();
    assert_eq!(old.words(), DEFAULT_WORDS);
    // A style: the prompt is written for it, and asks for no other look.
    let anime = instructions(&s, &Ask { style: crate::style::find("anime"), ..ask("a girl in a café") });
    assert!(DEFAULT_INSTRUCTIONS.contains(PICK_A_LOOK), "the built-in instructions changed: keep PICK_A_LOOK in step");
    assert!(!anime.contains(PICK_A_LOOK), "with a preset the instructions ask for no look of their own");
    assert!(anime.contains("the Anime preset replaces them") && anime.contains("an anime illustration with clean") && anime.contains("It is not a photograph"), "{anime}");
    assert!(!anime.contains("monochrome"));
    // The words for another look, but not the preset's own.
    let never = |id: &str| {
        let text = instructions(&s, &Ask { style: crate::style::find(id), ..ask("x") });
        text.split("Never write these, even when the user did: ").nth(1).unwrap_or("").to_string()
    };
    assert!(never("anime").contains("photorealistic, hyperrealistic, photograph") && !never("anime").contains("anime"), "{}", never("anime"));
    assert!(never("cinematic").contains("illustration") && !never("cinematic").contains("cinematic") && !never("cinematic").contains("lens"));
    assert!(!never("realistic-cgi").contains("CGI") && !never("clay-model").contains("photograph"));
    // What each preset can't show.
    let mono = instructions(&s, &Ask { style: crate::style::find("black-and-white"), ..ask("x") });
    assert!(mono.contains("name no colours at all") && !mono.contains("It is not a photograph"), "{mono}");
    assert!(instructions(&s, &Ask { style: crate::style::find("line-art"), ..ask("x") }).contains("name no colours at all"));
    assert!(instructions(&s, &Ask { style: crate::style::find("pixel-art"), ..ask("x") }).contains("palette is limited"));
    // The user's own instructions still get the preset's rules.
    let own = EnhanceSettings { instructions: "Write haiku.".into(), ..EnhanceSettings::default() };
    assert!(instructions(&own, &Ask { style: crate::style::find("anime"), ..ask("x") }).contains("the Anime preset replaces them"));
    for style in &crate::style::STYLES {
        assert!(instructions(&s, &Ask { style: Some(style), ..ask("x") }).contains(style.prompt), "{}", style.id);
    }
    assert!(!create.contains("A style preset decides"));
    assert_eq!(Purpose::from_mode("img2img"), Some(Purpose::Restyle));
    assert_eq!(Purpose::from_mode("nope"), None);
}

#[test]
fn each_service_gets_its_own_request() {
    let openai = request(TextProvider::Openai, "gpt-5.4-mini", "sk-1", "SYS", " a cat ", 60).unwrap();
    assert_eq!(openai.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(header(&openai, "authorization"), Some("Bearer sk-1"));
    assert_eq!(body(&openai), json!({"model": "gpt-5.4-mini", "messages": [{"role": "system", "content": "SYS"}, {"role": "user", "content": "a cat"}]}));
    assert_eq!(openai.timeout_seconds, 62);
    assert_eq!(request(TextProvider::Xai, "grok-4.7", "k", "S", "p", 5).unwrap().url, "https://api.x.ai/v1/chat/completions");
    let router = request(TextProvider::Openrouter, "anthropic/claude-haiku-5.5:free", "k", "S", "p", 5).unwrap();
    assert_eq!(router.url, "https://openrouter.ai/api/v1/chat/completions");
    assert_eq!(body(&router)["model"], "anthropic/claude-haiku-5.5:free");

    let claude = request(TextProvider::Anthropic, "claude-haiku-5-5", "ak", "SYS", "p", 30).unwrap();
    assert_eq!(claude.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(header(&claude, "x-api-key"), Some("ak"));
    assert_eq!(header(&claude, "anthropic-version"), Some(ANTHROPIC_VERSION));
    assert_eq!(header(&claude, "authorization"), None);
    let b = body(&claude);
    assert_eq!((b["system"].as_str(), b["messages"][0]["content"].as_str()), (Some("SYS"), Some("p")));
    assert!(b["max_tokens"].as_u64().is_some());

    let gemini = request(TextProvider::Gemini, "models/gemini-flash-latest", "gk", "SYS", "p", 30).unwrap();
    assert_eq!(gemini.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-flash-latest:generateContent");
    assert_eq!(header(&gemini, "x-goog-api-key"), Some("gk"));
    assert!(!gemini.url.contains("gk"), "the key never goes in the address");
    assert_eq!(body(&gemini)["systemInstruction"]["parts"][0]["text"], "SYS");
}

#[test]
fn bad_requests_are_errors_not_requests() {
    let config = |r: Result<Request>| matches!(r, Err(Error::Config(_)));
    assert!(config(request(TextProvider::Openai, "gpt", "", "S", "p", 5)), "no key");
    assert!(config(request(TextProvider::Openai, "gpt", "a\r\nInjected: 1", "S", "p", 5)), "a key that would add a header");
    assert!(config(request(TextProvider::Openai, "gpt", "k", "S", "   ", 5)), "no prompt");
    assert!(config(request(TextProvider::Openai, "gpt", "k", "S", &"x".repeat(MAX_PROMPT + 1), 5)), "too long");
    assert!(config(request(TextProvider::Openai, "", "k", "S", "p", 5)), "no model");
    assert!(config(request(TextProvider::Openai, "gpt 5", "k", "S", "p", 5)), "a space in the model");
    assert!(config(request(TextProvider::Openai, &"m".repeat(500), "k", "S", "p", 5)));
    // Gemini's id is part of the address: nothing that leaves its path.
    for hostile in ["../../x", "gemini:streamGenerateContent?alt=sse", "a/b", "gemini?key=1", "gemini#x"] {
        assert!(config(request(TextProvider::Gemini, hostile, "k", "S", "p", 5)), "{hostile}");
    }
}

#[test]
fn answers_are_read_and_cleaned() {
    let chat = json!({"choices": [{"message": {"role": "assistant", "content": "Prompt: \"A tabby cat on a velvet sofa\""}, "finish_reason": "stop"}]});
    assert_eq!(parse(TextProvider::Openai, "m", 200, chat.to_string().as_bytes()).unwrap(), "A tabby cat on a velvet sofa");
    let parts = json!({"choices": [{"message": {"content": [{"type": "text", "text": "A "}, {"type": "text", "text": "cat"}]}}]});
    assert_eq!(parse(TextProvider::Openrouter, "m", 200, parts.to_string().as_bytes()).unwrap(), "A cat");
    let claude = json!({"content": [{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": "```\nA cat, golden hour\n```"}], "stop_reason": "end_turn"});
    assert_eq!(parse(TextProvider::Anthropic, "m", 200, claude.to_string().as_bytes()).unwrap(), "A cat, golden hour");
    let gemini = json!({"candidates": [{"content": {"parts": [{"text": "thinking…", "thought": true}, {"text": "\u{201c}A cat\u{201d}"}]}}]});
    assert_eq!(parse(TextProvider::Gemini, "m", 200, gemini.to_string().as_bytes()).unwrap(), "A cat");
    // Quotes that belong to the prompt stay.
    assert_eq!(clean("A sign that says \"OPEN\""), "A sign that says \"OPEN\"");
    assert_eq!(clean("\"OPEN\" and \"CLOSED\""), "\"OPEN\" and \"CLOSED\"");
    assert_eq!(clean("```text\nx"), "x");
    assert_eq!(clean(&"é".repeat(MAX_PROMPT + 10)).chars().count(), MAX_PROMPT);
    assert_eq!(clean("prompt:"), "");
}

#[test]
fn failures_say_what_happened() {
    let msg = |p, status, v: Value| parse(p, "gpt-x", status, v.to_string().as_bytes()).unwrap_err().to_string();
    let e = msg(TextProvider::Openai, 401, json!({"error": {"message": "Incorrect API key provided."}}));
    assert!(e.contains("Incorrect API key provided.") && e.contains("Check the API key") && !e.contains(".."), "{e}");
    let e = msg(TextProvider::Anthropic, 404, json!({"type": "error", "error": {"type": "not_found_error", "message": "model: gpt-x"}}));
    assert!(e.contains("`gpt-x` may not be one of its models"), "{e}");
    assert!(msg(TextProvider::Openrouter, 402, json!({"error": {"message": "Insufficient credits"}})).contains("out of credit"));
    // An error status without JSON still says its status.
    let e = parse(TextProvider::Xai, "m", 502, b"<html>Bad gateway</html>").unwrap_err().to_string();
    assert!(e.contains("HTTP 502"), "{e}");
    // A 200 with an error in it (OpenRouter does that) is an error.
    assert!(matches!(parse(TextProvider::Openrouter, "m", 200, json!({"error": {"message": "upstream"}}).to_string().as_bytes()), Err(Error::Server(_))));
    assert!(matches!(parse(TextProvider::Openai, "m", 200, b"not json"), Err(Error::BadResponse(_))));
    assert!(matches!(parse(TextProvider::Openai, "m", 200, b"{}"), Err(Error::BadResponse(_))));
    assert!(matches!(parse(TextProvider::Openai, "m", 200, br#"{"choices":[{"message":{"content":null,"refusal":"No."}}]}"#), Err(Error::Server(_))));
    let long = json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}]});
    assert!(parse(TextProvider::Openai, "m", 200, long.to_string().as_bytes()).unwrap_err().to_string().contains("ran out of room"));
    assert!(matches!(parse(TextProvider::Anthropic, "m", 200, br#"{"content":[],"stop_reason":"refusal"}"#), Err(Error::Server(_))));
    assert!(parse(TextProvider::Gemini, "m", 200, br#"{"promptFeedback":{"blockReason":"SAFETY"}}"#).unwrap_err().to_string().contains("SAFETY"));
    assert!(matches!(parse(TextProvider::Gemini, "m", 200, br#"{"candidates":[]}"#), Err(Error::BadResponse(_))));
}

#[test]
fn model_lists_keep_the_ones_that_write() {
    let openai = json!({"data": [
        {"id": "gpt-5.4-mini", "created": 3}, {"id": "gpt-image-2", "created": 9}, {"id": "gpt-6-sol", "created": 5},
        {"id": "whisper-1", "created": 1}, {"id": "o4-mini", "created": 2}, {"id": "gpt-realtime", "created": 7}, {"id": "text-embedding-3", "created": 8},
    ]});
    assert_eq!(parse_models(TextProvider::Openai, 200, openai.to_string().as_bytes()).unwrap(), ["gpt-6-sol", "gpt-5.4-mini", "o4-mini"]);
    let gemini = json!({"models": [
        {"name": "models/gemini-flash-latest", "supportedGenerationMethods": ["generateContent", "countTokens"]},
        {"name": "models/gemini-3.1-flash-image", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]},
    ]});
    assert_eq!(parse_models(TextProvider::Gemini, 200, gemini.to_string().as_bytes()).unwrap(), ["gemini-flash-latest"]);
    let router = json!({"data": [
        {"id": "a/text", "created": 1, "architecture": {"output_modalities": ["text"]}},
        {"id": "a/text:batch", "created": 2, "architecture": {"output_modalities": ["text"]}},
        {"id": "a/picture", "created": 3, "architecture": {"output_modalities": ["image"]}},
        {"id": "b/both", "created": 4, "architecture": {"output_modalities": ["image", "text"]}},
    ]});
    assert_eq!(parse_models(TextProvider::Openrouter, 200, router.to_string().as_bytes()).unwrap(), ["b/both", "a/text"]);
    // Anthropic lists newest first, undated: the order stays.
    let claude = json!({"data": [{"id": "claude-opus-5-5"}, {"id": "claude-haiku-5-5"}, {"id": "claude-opus-5-5"}]});
    assert_eq!(parse_models(TextProvider::Anthropic, 200, claude.to_string().as_bytes()).unwrap(), ["claude-opus-5-5", "claude-haiku-5-5"]);
    assert!(parse_models(TextProvider::Xai, 401, br#"{"error":"bad key"}"#).unwrap_err().to_string().contains("bad key"));
    assert!(parse_models(TextProvider::Xai, 200, br#"{"data":"no"}"#).is_err());
    assert!(models_request(TextProvider::Openai, "").is_err(), "a listing needs the key");
    assert!(models_request(TextProvider::Openrouter, "").is_ok(), "OpenRouter's is public");
}

#[test]
fn enhance_sends_and_reports_progress() {
    let fake = Fake::new(200, json!({"content": [{"type": "text", "text": "A ginger cat asleep on a sunlit windowsill"}]}));
    let transport: Arc<dyn Transport> = fake.clone();
    let said = Mutex::new(Vec::new());
    let progress = |f: f32, m: &str| said.lock().unwrap().push((f, m.to_string()));
    let s = EnhanceSettings { provider: TextProvider::Anthropic, models: [("anthropic".into(), "claude-opus-5-5".into())].into(), ..EnhanceSettings::default() };
    let done = enhance(&transport, &s, "ak", &ask("cat window"), &control(&|| false, &progress)).unwrap();
    assert_eq!(done, Enhanced { prompt: "A ginger cat asleep on a sunlit windowsill".into(), provider: TextProvider::Anthropic, model: "claude-opus-5-5".into() });
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(body(&seen[0])["model"], "claude-opus-5-5");
    let said = said.lock().unwrap();
    assert!(said.first().is_some_and(|(_, m)| m.contains("Connecting to Claude")), "{said:?}");
    assert!(said.last().is_some_and(|(f, _)| *f > 0.9));
}

#[test]
fn a_slow_answer_reports_the_wait_and_a_cancel_ends_it_at_once() {
    let slow = Fake::slow(200, json!({}), Duration::from_secs(30));
    let transport: Arc<dyn Transport> = slow;
    let polls = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let lines = Mutex::new(Vec::new());
    let progress = |_: f32, m: &str| lines.lock().unwrap().push(m.to_string());
    let cancelled = || {
        // Cancel after about a second of waiting.
        if polls.fetch_add(1, Ordering::Relaxed) > 12 {
            cancel.store(true, Ordering::Relaxed);
        }
        cancel.load(Ordering::Relaxed)
    };
    let started = std::time::Instant::now();
    let e = enhance(&transport, &settings(TextProvider::Openai), "k", &ask("cat"), &control(&cancelled, &progress)).unwrap_err();
    assert_eq!(e, Error::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5), "the cancel doesn't wait for the answer");
    assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("Waiting for OpenAI (gpt-5.4-mini)… ") && l.ends_with(" of 60 s")), "{:?}", lines.lock().unwrap());
}

#[test]
fn the_timeout_ends_the_wait() {
    let slow = Fake::slow(200, json!({}), Duration::from_secs(30));
    let transport: Arc<dyn Transport> = slow;
    let s = EnhanceSettings { timeout_seconds: 0, ..settings(TextProvider::Xai) };
    let started = std::time::Instant::now();
    let e = enhance(&transport, &s, "k", &ask("cat"), &control(&|| false, &|_, _| {})).unwrap_err();
    assert!(matches!(&e, Error::Network(m) if m.contains("no answer within 5 s")), "{e}");
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[test]
fn nothing_is_sent_when_cancelled_first_or_misconfigured() {
    let fake = Fake::new(200, json!({}));
    let transport: Arc<dyn Transport> = fake.clone();
    assert_eq!(enhance(&transport, &settings(TextProvider::Gemini), "k", &ask("cat"), &control(&|| true, &|_, _| {})), Err(Error::Cancelled));
    assert!(matches!(enhance(&transport, &settings(TextProvider::Gemini), "", &ask("cat"), &control(&|| false, &|_, _| {})), Err(Error::Config(_))));
    assert!(fake.seen.lock().unwrap().is_empty());
}

#[test]
fn a_copied_style_sentence_is_taken_out_of_the_answer() {
    let anime = crate::style::find("anime").unwrap();
    let look = anime.prompt.trim_start_matches("Style: ");
    // Pasted at the end (what the models did), with and without "Style:", in any case.
    assert_eq!(without_style(&format!("An old fisherman on a pier, dawn light. {look}"), anime), "An old fisherman on a pier, dawn light.");
    assert_eq!(without_style(&format!("An old fisherman, {}", look.trim_end_matches('.')), anime), "An old fisherman.");
    assert_eq!(without_style(&format!("A fisherman.\n\n{}", anime.prompt), anime), "A fisherman.");
    assert_eq!(without_style(&format!("{} A fisherman mends a net.", look.to_uppercase()), anime), "A fisherman mends a net.");
    // Nothing of it: unchanged; only it: nothing left.
    assert_eq!(without_style("A fisherman in anime style.", anime), "A fisherman in anime style.");
    assert_eq!(without_style(anime.prompt, anime), "");
    assert_eq!(without_style("Ünïcödé fisherman", anime), "Ünïcödé fisherman.");
    // A model that answers with the style alone is an error, not an empty prompt.
    let fake = Fake::new(200, json!({"choices": [{"message": {"content": anime.prompt}}]}));
    let transport: Arc<dyn Transport> = fake;
    let e = enhance(&transport, &settings(TextProvider::Openai), "k", &Ask { style: Some(anime), ..ask("cat") }, &control(&|| false, &|_, _| {})).unwrap_err();
    assert!(matches!(e, Error::BadResponse(_)), "{e}");
}

#[test]
fn creep_moves_and_never_finishes() {
    assert!(creep(0.0) < creep(5.0) && creep(5.0) < creep(500.0) && creep(1e9) < 0.95);
    assert!((creep(f32::NAN) - creep(0.0)).abs() < 1e-6, "no time read as no time");
}
