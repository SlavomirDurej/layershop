//! Prompt enhancement: a cloud text model rewrites a prompt for the image models.
//!
//! - `ai.enhancePrompt` sends the prompt to the service chosen in Preferences › Generative AI ›
//!   Prompt Enhancement (`integrations.ai.enhance`: OpenRouter, OpenAI, xAI, Gemini or
//!   Anthropic, with a model of its own and a timeout) and returns the rewritten prompt. It is a
//!   background job: cancellable, and its progress says how long the answer has been awaited.
//!   It changes no document; the AI Gen panel puts the answer in its prompt box.
//! - `ai.enhanceModels` lists the models a service offers for it (the settings' picker).
//! - With `local` chosen, a text model on this computer writes it on the local engine
//!   (ComfyUI, started if need be): `ai.enhanceLocalModels` lists the three offered and which
//!   are installed, `ai.enhanceInstall` downloads one with the Local AI setup's downloader.
//!
//! The keys are [`crate::ai_cmds::AiRuntime`]'s: OpenAI, xAI and Gemini share their image
//! models' keys; OpenRouter's and Anthropic's are kept beside them (`ai.setKey`).

use photocraft_aigen::enhance::{self, Ask, MAX_PROMPT, MAX_TIMEOUT, MAX_WORDS, MIN_TIMEOUT, MIN_WORDS, Purpose, local};
use photocraft_aigen::{Control, EnhanceSettings, TextProvider};
use serde_json::{Value, json};

use crate::ai_cmds::{ai_error, bad, settings, transport};
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The label of the job (the status bar's progress, Cancel).
pub const LABEL: &str = "Enhance Prompt";

fn provider_param(cmd: &str, p: &Value, chosen: TextProvider) -> Result<TextProvider> {
    match p.get("provider") {
        None => Ok(chosen),
        Some(v) => v.as_str().and_then(TextProvider::from_key).ok_or_else(|| bad(cmd, format!("`provider` must be {}", TextProvider::keys()))),
    }
}

fn enhance_prompt(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.enhancePrompt";
    let original = p.get("prompt").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "missing \"prompt\""))?.to_string();
    if original.chars().count() > MAX_PROMPT {
        return Err(bad(cmd, format!("the prompt is longer than {MAX_PROMPT} characters")));
    }
    let purpose = match p.get("mode") {
        None => Purpose::Create,
        Some(v) => v.as_str().and_then(Purpose::from_mode).ok_or_else(|| bad(cmd, "`mode` must be txt2img|imgEdit|inpaint|img2img"))?,
    };
    let ai = settings(s);
    let mut chosen = ai.enhance.clone();
    chosen.provider = provider_param(cmd, p, chosen.provider)?;
    if let Some(v) = p.get("textModel") {
        let model = v.as_str().map(str::trim).filter(|m| !m.is_empty() && m.len() <= 200).ok_or_else(|| bad(cmd, "`textModel` must be a model id"))?;
        if chosen.provider.is_local() && local::find(model).is_none() {
            return Err(bad(cmd, format!("`textModel` must be a local model: {}", local_ids())));
        }
        chosen.models.insert(chosen.provider.key().to_string(), model.to_string());
    }
    if let Some(v) = p.get("timeout") {
        let range = u64::from(MIN_TIMEOUT)..=u64::from(MAX_TIMEOUT);
        let seconds = v.as_u64().filter(|t| range.contains(t)).ok_or_else(|| bad(cmd, format!("`timeout` must be {MIN_TIMEOUT} to {MAX_TIMEOUT} seconds")))?;
        chosen.timeout_seconds = u32::try_from(seconds).unwrap_or(MAX_TIMEOUT);
    }
    if let Some(v) = p.get("words") {
        let range = u64::from(MIN_WORDS)..=u64::from(MAX_WORDS);
        let words = v.as_u64().filter(|w| range.contains(w)).ok_or_else(|| bad(cmd, format!("`words` must be {MIN_WORDS} to {MAX_WORDS}")))?;
        chosen.words = u32::try_from(words).unwrap_or(MAX_WORDS);
    }
    // The image model the prompt is for: its name tells the text model what it writes for.
    let image_model = match p.get("model") {
        None => String::new(),
        Some(v) => {
            let id = v.as_str().ok_or_else(|| bad(cmd, "`model` must be a model id"))?;
            ai.models.iter().find(|m| m.id == id).map(|m| if m.name.trim().is_empty() { m.family.label().to_string() } else { m.name.clone() }).unwrap_or_default()
        }
    };
    // The style preset the image is rendered in: the prompt is written for it.
    let style = match p.get("style") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if id.trim().is_empty() => None,
        Some(Value::String(id)) => {
            Some(photocraft_aigen::style::find(id).ok_or_else(|| bad(cmd, format!("no style `{}`: ai.styles lists them", id.chars().take(64).collect::<String>())))?)
        }
        Some(_) => return Err(bad(cmd, "`style` must be a style id (ai.styles lists them)")),
    };
    let transport = transport(s)?;
    let ask = Ask { prompt: original.clone(), purpose, image_model, style };
    if chosen.provider.is_local() {
        return enhance_locally(s, transport, chosen, ask, original);
    }
    let key = s.ai.text_key(chosen.provider);
    if key.is_empty() {
        return Err(EngineError::Other(format!("{} needs an API key: add one in Preferences › Generative AI › API Keys", chosen.provider.label())));
    }
    crate::jobs::run(
        s,
        LABEL,
        false,
        move |ctx| {
            let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m), sleep: &|d| std::thread::sleep(d) };
            enhance::enhance(&transport, &chosen, &key, &ask, &control).map_err(ai_error)
        },
        move |_, done| Ok(json!({"prompt": done.prompt, "original": original, "provider": done.provider.key(), "textModel": done.model, "style": style.map(|s| s.id)})),
    )
}

/// `local::MODELS`' ids, for errors.
fn local_ids() -> String {
    local::MODELS.map(|m| m.id).join("|")
}

/// Enhance with a text model on this computer: the local engine is started if it isn't running
/// (as for a local image model), then writes the prompt.
fn enhance_locally(s: &mut Session, transport: std::sync::Arc<dyn photocraft_aigen::Transport>, chosen: EnhanceSettings, ask: Ask, original: String) -> Result<Value> {
    let ai = settings(s);
    let server = s.ai.server.clone();
    let style = ask.style;
    crate::jobs::run(
        s,
        LABEL,
        false,
        move |ctx| {
            let control = Control { cancelled: &|| ctx.cancelled(), progress: &|f, m| ctx.progress(f, m), sleep: &|d| std::thread::sleep(d) };
            if let Some(server) = &server {
                ctx.progress(0.01, "Starting the local engine\u{2026}");
                server.ensure(transport.as_ref(), &ai, &control).map_err(ai_error)?;
            }
            local::enhance(transport.as_ref(), &ai.comfy_url, &chosen, &ask, photocraft_aigen::random_seed(), &control).map_err(ai_error)
        },
        move |_, done| Ok(json!({"prompt": done.prompt, "original": original, "provider": done.provider.key(), "textModel": done.model, "style": style.map(|s| s.id)})),
    )
}

/// The folders the engine reads models from: the setup's own, then the user's it was given.
#[cfg(not(target_arch = "wasm32"))]
fn model_dirs(s: &Session) -> Vec<std::path::PathBuf> {
    let record = s.ai_setup.record();
    let own = record.choices.models_dir.trim().to_string();
    let own = if own.is_empty() { s.ai_setup.env.as_ref().map(|e| e.models_dir.clone()).unwrap_or_default() } else { own };
    let mut dirs = Vec::new();
    if !own.trim().is_empty() {
        dirs.push(std::path::PathBuf::from(own.trim()));
    }
    dirs.extend(record.choices.extra_models());
    dirs
}

/// Is `model`'s file in one of `dirs` (at its full size), or among the files the engine said it
/// has (`server`, a ComfyUI of the user's own)?
#[cfg(not(target_arch = "wasm32"))]
fn installed(model: &local::LocalModel, dirs: &[std::path::PathBuf], server: Option<&std::collections::BTreeMap<String, Vec<String>>>) -> bool {
    let on_disk = dirs.iter().any(|d| std::fs::metadata(d.join(local::FOLDER).join(model.file)).is_ok_and(|m| m.is_file() && m.len() == model.size));
    on_disk || server.and_then(|f| f.get(local::FOLDER)).is_some_and(|names| names.iter().any(|n| n == model.file))
}

fn enhance_local_models(s: &mut Session, _: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let (dirs, server) = (model_dirs(s), s.ai_setup.record().choices.server_files);
    let models: Vec<Value> = local::MODELS
        .iter()
        .map(|m| {
            #[cfg(not(target_arch = "wasm32"))]
            let have = installed(m, &dirs, server.as_ref());
            #[cfg(target_arch = "wasm32")]
            let have = false;
            json!({"id": m.id, "name": m.name, "role": m.role, "file": m.file, "folder": local::FOLDER, "size": m.size, "installed": have})
        })
        .collect();
    Ok(json!({"models": models, "default": local::DEFAULT, "chosen": settings(s).enhance.model(TextProvider::Local)}))
}

#[cfg(not(target_arch = "wasm32"))]
fn enhance_install(s: &mut Session, p: &Value) -> Result<Value> {
    use photocraft_aisetup::download::{self, Spec};
    let cmd = "ai.enhanceInstall";
    let id = p.get("model").and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("missing `model`: {}", local_ids())))?;
    let model = *local::find(id).ok_or_else(|| bad(cmd, format!("`model` must be {}", local_ids())))?;
    let env = s.ai_setup.env.clone().ok_or_else(|| EngineError::Other(crate::ai_setup_cmds::NEEDS_DESKTOP.into()))?;
    let manifest = match env.manifest.clone() {
        Some(m) => m,
        None => crate::ai_setup_cmds::bundled()?,
    };
    let file = manifest.files.get(&model.manifest_key()).cloned().ok_or_else(|| EngineError::Other(format!("{} has no download pinned in this build", model.name)))?;
    let dirs = model_dirs(s);
    let Some(own) = dirs.first().cloned() else {
        return Err(EngineError::Other("Set up Local AI first (File \u{203a} Set Up Local AI): it gives the engine its models folder".into()));
    };
    if installed(&model, &dirs, s.ai_setup.record().choices.server_files.as_ref()) {
        return Ok(json!({"model": model.id, "installed": true, "downloaded": false}));
    }
    let dest = own.join(local::FOLDER).join(model.file);
    let token = s.ai.hf_token();
    crate::jobs::run(
        s,
        "Install Text Model",
        false,
        move |ctx| {
            let size = file.size.max(1);
            let human = photocraft_aisetup::human_bytes;
            let dl = download::Ctx { fetch: env.fetch.as_ref(), cancelled: &|| ctx.cancelled(), sleep: &|d| std::thread::sleep(d), free_space: &download::free_space };
            let spec = Spec { url: &file.url, size: file.size, sha256: &file.sha256, token: (!token.is_empty() && file.gated).then_some(token.as_str()) };
            ctx.progress(0.0, &format!("Downloading {}\u{2026}", model.name));
            let progress = |n: u64| ctx.progress(n as f32 / size as f32, &format!("Downloading {}\u{2026} {} of {}", model.name, human(n), human(file.size)));
            download::download(&dl, &spec, &dest, &progress).map_err(|e| match e {
                photocraft_aisetup::Error::Cancelled => EngineError::Cancelled,
                e => EngineError::Other(e.to_string()),
            })?;
            Ok(dest.display().to_string())
        },
        move |_, path| Ok(json!({"model": model.id, "installed": true, "downloaded": true, "path": path})),
    )
}

#[cfg(target_arch = "wasm32")]
fn enhance_install(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("Installing models needs the desktop app".into()))
}

fn enhance_models(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.enhanceModels";
    let provider = provider_param(cmd, p, settings(s).enhance.provider)?;
    let transport = transport(s)?;
    let key = s.ai.text_key(provider);
    crate::jobs::run(
        s,
        "List Text Models",
        false,
        move |ctx| {
            ctx.progress(0.1, &format!("Asking {} for its models…", provider.short_label()));
            enhance::list_models(transport.as_ref(), provider, &key).map_err(ai_error)
        },
        move |_, models| Ok(json!({"provider": provider.key(), "models": models, "default": provider.default_model()})),
    )
}

fn can_enhance(s: &Session) -> std::result::Result<(), String> {
    if s.ai.transport.is_none() {
        return Err("prompt enhancement needs the desktop app".into());
    }
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.enhancePrompt",
            label: "Enhance Prompt",
            menu: &[],
            shortcut: None,
            params: r##"{"prompt":str,"mode":"txt2img|imgEdit|inpaint|img2img"="txt2img" (what the prompt is for: a new image, an instruction to an edit model, what to paint in a selection, what a picture should become),"model":id? (the image model it is for; its name is passed on),"provider":"openrouter|openai|xai|gemini|anthropic|local"?=integrations.ai.enhance.provider (local: a text model on this computer, run by the local engine, which is started if need be; see ai.enhanceLocalModels),"textModel":str?=the preferences' model for that provider (for local: gemma-4-e2b|qwen3-4b|gemma-4-e4b),"timeout":5..600?=integrations.ai.enhance.timeoutSeconds,"words":10..400?=integrations.ai.enhance.words (the rewritten prompt is at most about that many words),"style":id?=none (the style preset the image will be rendered in, see ai.styles: the prompt is written for that look and asks for no other)} → {prompt (the rewritten prompt),original,provider,textModel,style}: a text model (a cloud service's, or a local one) rewrites the prompt with more detail (subject, setting, light, style), keeping what it says, its names, quoted text and @mentions. A background job (cancellable; changes no document); billed to the provider's account. Keys: ai.setKey (openai, xai and gemini share the image models' keys)"##,
            enabled: can_enhance,
            run: enhance_prompt,
            journal: false,
        },
        CommandSpec {
            id: "ai.enhanceModels",
            label: "List Text Models",
            menu: &[],
            shortcut: None,
            params: r##"{"provider":"openrouter|openai|xai|gemini|anthropic"?=integrations.ai.enhance.provider} → {provider,models:[id,…] (the ones that answer in text, newest first where the provider dates them),default (the model used when the preferences name none)}: asks the provider with its saved key (OpenRouter's list needs none)"##,
            enabled: can_enhance,
            run: enhance_models,
            journal: false,
        },
        CommandSpec {
            id: "ai.enhanceLocalModels",
            label: "Local Text Models",
            menu: &[],
            shortcut: None,
            params: r##"{} → {models:[{id,name,role,file,folder,size (bytes),installed}],default,chosen}: the text models that enhance prompts on this computer (run by the local engine), and which of them its models folders have"##,
            enabled: |_| Ok(()),
            run: enhance_local_models,
            journal: false,
        },
        CommandSpec {
            id: "ai.enhanceInstall",
            label: "Install Text Model",
            menu: &[],
            shortcut: None,
            params: r##"{"model":"gemma-4-e2b|qwen3-4b|gemma-4-e4b"} → {model,installed:true,downloaded (false: it was there already),path?}: downloads a local text model for prompt enhancement into the engine's models folder (text_encoders) with the Local AI setup's downloader: the file is pinned with its checksum, and a stopped download resumes. A background job (cancellable)"##,
            enabled: can_enhance,
            run: enhance_install,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
