//! Prompt enhancement on this computer: a small text model run by the local engine (ComfyUI)
//! rewrites the prompt, with no key, no account and nothing sent away.
//!
//! The engine runs a three-node workflow ([`crate::comfy::text_workflow`]): Load CLIP with the
//! model's file, Generate Text (thinking off, the answer capped by the length setting) and
//! Preview as Text, whose output is read back from the history. No image model runs. The
//! instructions are the cloud services' ([`super::instructions`]): the task, the image model,
//! the length and the style preset are added to them in the same way.
//!
//! The models offered are text encoders the engine can also write with: checked against the
//! pinned engine (ComfyUI 0.43.0), whose loader recognises Gemma 4 E2B and E4B and Qwen3 4B and
//! whose Generate Text runs them without thinking. Their files are pinned, with checksums, in
//! the Local AI setup's manifest (`text_encoders/<file>`).

use std::time::Duration;

use serde_json::json;

use super::{Ask, EnhanceSettings, Enhanced, MAX_PROMPT, TextProvider, clean, instructions, keep_quotes, reminder, without_style};
use crate::{Control, Error, Request, Result, Transport, comfy};

/// The engine's folder the models' files are in.
pub const FOLDER: &str = "text_encoders";
/// The model used when the settings name none.
pub const DEFAULT: &str = "gemma-4-e2b";
/// How often the engine is asked whether the text is ready.
const POLL: Duration = Duration::from_millis(250);
/// Failed polls in a row before the engine is taken for lost.
const MAX_POLL_FAILURES: u32 = 20;
/// The id the engine's queue knows these requests by.
const CLIENT: &str = "photocraft-enhance";

/// A text model the engine can enhance with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalModel {
    /// Its id in the settings (`integrations.ai.enhance.models.local`).
    pub id: &'static str,
    pub name: &'static str,
    /// How fast it writes, beside the others (measured on the pinned engine, 2026-11-05).
    pub speed: &'static str,
    /// What it is for, as the settings say it.
    pub role: &'static str,
    /// Its file in the engine's `text_encoders` folder.
    pub file: &'static str,
    /// The file's size in bytes.
    pub size: u64,
}

/// The models offered, the default first.
pub const MODELS: [LocalModel; 3] = [
    LocalModel {
        id: "gemma-4-e2b",
        name: "Gemma 4 E2B IT",
        speed: "Fastest",
        role: "The smallest: for everyday use.",
        file: "gemma4_e2b_it_int8_convrot.safetensors",
        size: 5_199_997_904,
    },
    LocalModel {
        id: "qwen3-4b",
        name: "Qwen3 4B",
        speed: "Slowest",
        role: "Another voice, to compare its writing.",
        file: "qwen_3_4b_fp8_mixed.safetensors",
        size: 5_631_994_051,
    },
    LocalModel {
        id: "gemma-4-e4b",
        name: "Gemma 4 E4B IT",
        speed: "Slower, more detail",
        role: "Larger: needs more memory.",
        file: "gemma4_e4b_it_fp8_scaled.safetensors",
        size: 9_057_782_194,
    },
];

/// The model with this id.
pub fn find(id: &str) -> Option<&'static LocalModel> {
    MODELS.iter().find(|m| m.id == id.trim())
}

impl LocalModel {
    /// Its file's key in the Local AI setup's manifest: `text_encoders/<file>`.
    pub fn manifest_key(&self) -> String {
        format!("{FOLDER}/{}", self.file)
    }
}

/// The most tokens the answer may take for a prompt of at most about `words` words: room for
/// the words (a word is a token and a bit) and a little more, so the cap ends a run-on answer
/// without cutting a fitting one short.
pub fn max_tokens(words: u32) -> u32 {
    words.saturating_mul(8).saturating_div(5).saturating_add(64).clamp(64, 1024)
}

/// The workflow that rewrites `ask.prompt` with `model`, as `settings` say. A style preset's
/// rules are also put after the prompt ([`reminder`]): a small model follows them better there.
pub fn workflow(model: &LocalModel, settings: &EnhanceSettings, ask: &Ask, seed: u64) -> serde_json::Value {
    let prompt = match reminder(ask) {
        Some(rules) => format!("{}\n\n{rules}", ask.prompt.trim()),
        None => ask.prompt.trim().to_string(),
    };
    comfy::text_workflow(model.file, &instructions(settings, ask), &prompt, max_tokens(settings.words()), seed)
}

/// Rewrite `ask.prompt` with the local model `settings` name, on the engine at `base` (which
/// must answer: the caller starts it). Blocks until the text is ready, the timeout passes or
/// `control` reports a cancel; a cancel stops the engine's work and takes it off the queue.
pub fn enhance(transport: &dyn Transport, base: &str, settings: &EnhanceSettings, ask: &Ask, seed: u64, control: &Control<'_>) -> Result<Enhanced> {
    let id = settings.model(TextProvider::Local);
    let model = find(&id).ok_or_else(|| {
        Error::Config(format!("`{}` is not a local model to enhance with: choose one in Preferences › Generative AI › Prompt Enhancement", id.chars().take(60).collect::<String>()))
    })?;
    let prompt = ask.prompt.trim();
    if prompt.is_empty() {
        return Err(Error::Config("write a prompt first".into()));
    }
    if prompt.chars().count() > MAX_PROMPT {
        return Err(Error::Config(format!("the prompt is longer than {MAX_PROMPT} characters")));
    }
    let base = comfy::base_url(base)?;
    let workflow = workflow(model, settings, ask, seed);
    if (control.cancelled)() {
        return Err(Error::Cancelled);
    }
    (control.progress)(0.03, &format!("Loading {}…", model.name));
    let unreachable = |e: Error| Error::Network(format!("Could not reach the local engine at {base}. Is it running? ({e})"));
    let queued = transport.send(&Request::post_json(format!("{base}/prompt"), comfy::queue_body(&workflow, CLIENT)).timeout(30)).map_err(unreachable)?;
    let job = comfy::parse_queued(queued.status, &queued.body).map_err(|e| missing(model, e))?;
    let timeout = settings.timeout();
    let polls = (timeout.saturating_mul(1000) / POLL.as_millis().max(1) as u64).max(1);
    let mut failures = 0u32;
    for n in 0..polls {
        if (control.cancelled)() {
            stop(transport, &base, &job);
            return Err(Error::Cancelled);
        }
        let seconds = n as f32 * POLL.as_secs_f32();
        (control.progress)(super::creep(seconds), &format!("Writing with {}… {} s of {timeout} s", model.name, seconds as u64));
        (control.sleep)(POLL);
        let text = match transport.send(&Request::get(format!("{base}/history/{job}")).timeout(20)) {
            Ok(r) if r.ok() => comfy::parse_text_history(&r.body, &job).map_err(|e| missing(model, e))?,
            Ok(_) | Err(_) => {
                failures += 1;
                if failures > MAX_POLL_FAILURES {
                    return Err(Error::Network(format!("Lost the connection to the local engine at {base} while it was writing")));
                }
                continue;
            }
        };
        failures = 0;
        let Some(text) = text else { continue };
        (control.progress)(0.97, "Reading the answer…");
        let mut prompt = keep_quotes(&ask.prompt, &clean(&text));
        if let Some(style) = ask.style {
            prompt = without_style(&prompt, style);
        }
        if prompt.is_empty() {
            return Err(Error::BadResponse(format!("{} wrote nothing: try again, or choose another model", model.name)));
        }
        return Ok(Enhanced { prompt, provider: TextProvider::Local, model: model.id.to_string() });
    }
    stop(transport, &base, &job);
    Err(Error::Server(format!("{} did not finish within {timeout} seconds: raise the timeout in Preferences › Generative AI › Prompt Enhancement › Settings", model.name)))
}

/// Best effort: stop the engine's work on `job` and take it off the queue.
fn stop(transport: &dyn Transport, base: &str, job: &str) {
    let _ = transport.send(&Request::post_json(format!("{base}/interrupt"), b"{}".to_vec()).timeout(5));
    let _ = transport.send(&Request::post_json(format!("{base}/queue"), json!({"delete": [job]}).to_string().into_bytes()).timeout(5));
}

/// `e`, the engine's refusal, saying what to do when it is because the model's file is missing
/// (Load CLIP lists only the files it has).
fn missing(model: &LocalModel, e: Error) -> Error {
    match e {
        Error::Server(msg) if msg.contains("clip_name") || msg.contains(model.file) => Error::Server(format!(
            "{} is not installed: the engine has no {}. Install it in Preferences › Generative AI › Prompt Enhancement › Local Models ({msg})",
            model.name, model.file
        )),
        e => e,
    }
}

#[cfg(test)]
mod tests;
