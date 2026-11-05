//! Try prompt enhancement against the real services, without the app:
//!
//! ```sh
//! cargo run -p photocraft-aigen --example enhance -- openai "a cat on a sofa"
//! cargo run -p photocraft-aigen --example enhance -- xai,gemini "make the jacket red" imgEdit
//! cargo run -p photocraft-aigen --example enhance -- anthropic "a cat" txt2img claude-opus-5-5
//! cargo run -p photocraft-aigen --example enhance -- gemini list      # the models it offers
//! cargo run -p photocraft-aigen --example enhance -- local "a cat" txt2img qwen3-4b
//! ```
//!
//! `local` runs on a ComfyUI that is already running at `PHOTOCRAFT_COMFY`
//! (default `http://127.0.0.1:8190`, the Local AI setup's engine); `PHOTOCRAFT_WORDS` sets the
//! length. `PHOTOCRAFT_STYLE=anime` writes the prompt for that style preset. Keys come from the environment: `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`,
//! `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`. Each run prints how long the answer took.

use std::sync::Arc;
use std::time::Instant;

use photocraft_aigen::enhance::{Ask, EnhanceSettings, Purpose, TextProvider, enhance, list_models};
use photocraft_aigen::{Control, Transport, UreqTransport};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let providers = args.first().ok_or(format!("usage: enhance <{}>[,…] <prompt|list> [mode] [model]", TextProvider::keys()))?;
    let prompt = args.get(1).ok_or("give a prompt, or `list`")?;
    let purpose = match args.get(2) {
        Some(mode) => Purpose::from_mode(mode).ok_or("mode must be txt2img|imgEdit|inpaint|img2img")?,
        None => Purpose::Create,
    };
    let transport: Arc<dyn Transport> = Arc::new(UreqTransport::new());
    for key in providers.split(',') {
        let provider = TextProvider::from_key(key.trim()).ok_or(format!("no provider `{key}`: {}", TextProvider::keys()))?;
        let api_key = std::env::var(provider.key_env()).unwrap_or_default();
        let started = Instant::now();
        if prompt == "list" {
            match list_models(transport.as_ref(), provider, &api_key) {
                Ok(models) => println!("{} ({:.1} s): {}", provider.label(), started.elapsed().as_secs_f32(), models.join(", ")),
                Err(e) => println!("{}: error: {e}", provider.label()),
            }
            continue;
        }
        let mut settings = EnhanceSettings { provider, ..EnhanceSettings::default() };
        if let Some(words) = std::env::var("PHOTOCRAFT_WORDS").ok().and_then(|w| w.parse().ok()) {
            settings.words = words;
        }
        if provider.is_local() {
            settings.timeout_seconds = 300;
        }
        if let Some(model) = args.get(3) {
            settings.models.insert(provider.key().into(), model.clone());
        }
        let last = std::sync::Mutex::new(String::new());
        let control = Control {
            cancelled: &|| false,
            progress: &|_, m| *last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = m.to_string(),
            sleep: &|d| std::thread::sleep(d),
        };
        let style = std::env::var("PHOTOCRAFT_STYLE").ok().and_then(|id| photocraft_aigen::style::find(&id));
        let ask = Ask { prompt: prompt.clone(), purpose, image_model: String::new(), style };
        let result = if provider.is_local() {
            let base = std::env::var("PHOTOCRAFT_COMFY").unwrap_or_else(|_| "http://127.0.0.1:8190".into());
            photocraft_aigen::enhance::local::enhance(transport.as_ref(), &base, &settings, &ask, photocraft_aigen::random_seed(), &control)
        } else {
            enhance(&transport, &settings, &api_key, &ask, &control)
        };
        match result {
            Ok(done) => println!("{} · {} ({:.1} s)\n  {}\n", provider.label(), done.model, started.elapsed().as_secs_f32(), done.prompt),
            Err(e) => println!("{} · {} ({:.1} s): error: {e}\n", provider.label(), settings.model(provider), started.elapsed().as_secs_f32()),
        }
    }
    Ok(())
}
