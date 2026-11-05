//! Prompt enhancement: a cloud text model rewrites the AI Gen panel's prompt so the image model
//! gets a richer, clearer one (subject, setting, light, style), keeping what the user wrote.
//!
//! One of five services does it, chosen in Preferences › Generative AI › Prompt Enhancement
//! ([`EnhanceSettings`]): OpenRouter, OpenAI, xAI (Grok), Google Gemini or Anthropic (Claude).
//! OpenAI, xAI and Gemini use the same API key as their image models.
//!
//! Requests are built and answers read by pure functions, tested without a network. [`enhance`]
//! sends on a helper thread and watches it, so a cancel or the timeout ends the wait at once and
//! the elapsed time is reported while the answer is awaited.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Control, Error, Provider, Request, Response, Result, Transport};

pub mod local;

/// Longest prompt sent, and longest answer kept (characters).
pub const MAX_PROMPT: usize = 20_000;
/// The timeout's range and default, in seconds.
pub const MIN_TIMEOUT: u32 = 5;
pub const MAX_TIMEOUT: u32 = 600;
pub const DEFAULT_TIMEOUT: u32 = 60;
/// The enhanced prompt's length range and default, in words (at most about that many).
pub const MIN_WORDS: u32 = 10;
pub const MAX_WORDS: u32 = 400;
pub const DEFAULT_WORDS: u32 = 120;
/// What a cloud-only function says of [`TextProvider::Local`], which [`local::enhance`] runs.
const LOCAL_IS_NO_SERVICE: &str = "a local model runs on the engine, not a cloud service";
/// Longest model id accepted.
const MAX_MODEL_ID: usize = 200;
/// Most models a listing returns.
const MAX_LISTED: usize = 1000;
/// Most tokens Anthropic may answer with (its API needs a number).
const ANTHROPIC_MAX_TOKENS: u32 = 2048;
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The services that can enhance a prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextProvider {
    /// OpenRouter: one key for many providers' models.
    Openrouter,
    /// OpenAI's Chat Completions (the ChatGPT models).
    #[default]
    Openai,
    /// xAI's Grok.
    Xai,
    /// Google's Gemini.
    Gemini,
    /// Anthropic's Claude.
    Anthropic,
    /// A model on this computer, run by the local engine (ComfyUI): [`local`].
    Local,
}

impl TextProvider {
    /// The cloud services, in the order the settings list them ([`TextProvider::Local`] is apart:
    /// it needs no key).
    pub const ALL: [TextProvider; 5] = [TextProvider::Openrouter, TextProvider::Openai, TextProvider::Xai, TextProvider::Gemini, TextProvider::Anthropic];

    /// Its key in the settings and in the stored API keys. OpenAI, xAI and Gemini share their
    /// image models' keys ([`Provider::key`]).
    pub fn key(self) -> &'static str {
        match self {
            TextProvider::Openrouter => "openrouter",
            TextProvider::Openai => "openai",
            TextProvider::Xai => "xai",
            TextProvider::Gemini => "gemini",
            TextProvider::Anthropic => "anthropic",
            TextProvider::Local => "local",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().chain([TextProvider::Local]).find(|p| p.key() == key)
    }

    /// `openrouter|openai|…|local`, for parameter docs and errors.
    pub fn keys() -> String {
        let mut keys = Self::ALL.map(Self::key).join("|");
        keys.push_str("|local");
        keys
    }

    /// Runs on this computer (no key, no account).
    pub fn is_local(self) -> bool {
        self == TextProvider::Local
    }

    pub fn label(self) -> &'static str {
        match self {
            TextProvider::Openrouter => "OpenRouter",
            TextProvider::Openai => "OpenAI (ChatGPT)",
            TextProvider::Xai => "xAI (Grok)",
            TextProvider::Gemini => "Google Gemini",
            TextProvider::Anthropic => "Anthropic (Claude)",
            TextProvider::Local => "Local model",
        }
    }

    /// The name used in progress lines ("Waiting for Grok…").
    pub fn short_label(self) -> &'static str {
        match self {
            TextProvider::Openrouter => "OpenRouter",
            TextProvider::Openai => "OpenAI",
            TextProvider::Xai => "Grok",
            TextProvider::Gemini => "Gemini",
            TextProvider::Anthropic => "Claude",
            TextProvider::Local => "local model",
        }
    }

    /// The image provider whose API key this one uses, if any.
    pub fn image_provider(self) -> Option<Provider> {
        match self {
            TextProvider::Openai => Some(Provider::Openai),
            TextProvider::Xai => Some(Provider::Xai),
            TextProvider::Gemini => Some(Provider::Gemini),
            TextProvider::Openrouter | TextProvider::Anthropic | TextProvider::Local => None,
        }
    }

    /// The environment variable read when no key is stored.
    pub fn key_env(self) -> &'static str {
        match self {
            TextProvider::Openrouter => "OPENROUTER_API_KEY",
            TextProvider::Openai => "OPENAI_API_KEY",
            TextProvider::Xai => "XAI_API_KEY",
            TextProvider::Gemini => "GEMINI_API_KEY",
            TextProvider::Anthropic => "ANTHROPIC_API_KEY",
            TextProvider::Local => "",
        }
    }

    /// The model used when the settings name none: a fast, inexpensive one that writes well.
    pub fn default_model(self) -> &'static str {
        match self {
            TextProvider::Openrouter => "google/gemini-3.5-flash-lite",
            TextProvider::Openai => "gpt-5.4-mini",
            TextProvider::Xai => "grok-4.20-0309-non-reasoning",
            TextProvider::Gemini => "gemini-flash-lite-latest",
            TextProvider::Anthropic => "claude-haiku-5-5",
            TextProvider::Local => local::DEFAULT,
        }
    }

    /// Where its API keys are made (the page, the address to show of it), and what to know.
    pub fn key_source(self) -> (&'static str, &'static str, &'static str) {
        match self {
            TextProvider::Openrouter => (
                "https://openrouter.ai/settings/keys",
                "openrouter.ai",
                "One key reaches many providers' models. Requests are paid for from the credits on your OpenRouter account.",
            ),
            TextProvider::Anthropic => (
                "https://console.anthropic.com/settings/keys",
                "console.anthropic.com",
                "Create a key in the Claude Console. Requests are billed to your API account, which needs credit: a Claude subscription doesn't cover it.",
            ),
            TextProvider::Openai => (
                "https://platform.openai.com/api-keys",
                "platform.openai.com",
                "Requests are billed to your OpenAI API account, which needs credit: a ChatGPT subscription doesn't cover it.",
            ),
            TextProvider::Xai => ("https://console.x.ai", "console.x.ai", "Requests are paid for from your team's credits in the xAI console."),
            TextProvider::Gemini => ("https://aistudio.google.com/apikey", "aistudio.google.com", "Requests are billed to the key's Google Cloud project."),
            TextProvider::Local => ("", "", ""),
        }
    }
}

/// `integrations.ai.enhance` in the preferences: which service enhances prompts, with which
/// model, and how long to wait for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnhanceSettings {
    /// The one service used (the settings' radio group).
    pub provider: TextProvider,
    /// The model chosen for each service, by [`TextProvider::key`]; one not named uses
    /// [`TextProvider::default_model`].
    pub models: BTreeMap<String, String>,
    /// Give up after this long ([`MIN_TIMEOUT`]..=[`MAX_TIMEOUT`]).
    pub timeout_seconds: u32,
    /// The enhanced prompt is at most about this many words ([`MIN_WORDS`]..=[`MAX_WORDS`]).
    pub words: u32,
    /// What the text model is told to do, in place of [`DEFAULT_INSTRUCTIONS`] (empty: those).
    pub instructions: String,
}

impl Default for EnhanceSettings {
    fn default() -> Self {
        Self { provider: TextProvider::default(), models: BTreeMap::new(), timeout_seconds: DEFAULT_TIMEOUT, words: DEFAULT_WORDS, instructions: String::new() }
    }
}

impl EnhanceSettings {
    /// The model `provider` is asked with.
    pub fn model(&self, provider: TextProvider) -> String {
        self.models.get(provider.key()).map(|m| m.trim()).filter(|m| !m.is_empty()).unwrap_or(provider.default_model()).to_string()
    }

    /// The timeout in seconds, within its range.
    pub fn timeout(&self) -> u64 {
        u64::from(self.timeout_seconds.clamp(MIN_TIMEOUT, MAX_TIMEOUT))
    }

    /// The length in words, within its range.
    pub fn words(&self) -> u32 {
        self.words.clamp(MIN_WORDS, MAX_WORDS)
    }

    /// The instructions shown and edited in the settings: the user's own, or the built-in ones.
    pub fn instructions_text(&self) -> &str {
        let own = self.instructions.trim();
        if own.is_empty() { DEFAULT_INSTRUCTIONS } else { &self.instructions }
    }

    /// Keep `text` as the instructions: the built-in ones (or nothing) are stored as empty, so
    /// they follow the app's when it improves them.
    pub fn set_instructions(&mut self, text: &str) {
        let text = text.trim();
        self.instructions = if text == DEFAULT_INSTRUCTIONS.trim() { String::new() } else { text.to_string() };
    }
}

/// What the prompt is for: the AI Gen panel's task, which changes what a good prompt is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    /// A new image from the prompt alone.
    #[default]
    Create,
    /// An instruction to an edit model that is shown the picture.
    Change,
    /// What to paint inside a selection.
    Fill,
    /// What a picture should become, keeping its composition.
    Restyle,
}

impl Purpose {
    /// From `ai.generate`'s `mode`: `txt2img|imgEdit|inpaint|img2img`.
    pub fn from_mode(mode: &str) -> Option<Self> {
        match mode {
            "txt2img" => Some(Purpose::Create),
            "imgEdit" => Some(Purpose::Change),
            "inpaint" => Some(Purpose::Fill),
            "img2img" => Some(Purpose::Restyle),
            _ => None,
        }
    }

    fn note(self) -> &'static str {
        match self {
            Purpose::Create => "The prompt describes a new image to generate.",
            Purpose::Change => {
                "The prompt is an instruction to an image editing model that is shown the picture. Keep it an instruction: say clearly what to change and what to keep the same. Don't describe the whole picture, and don't add changes the user didn't ask for. An @mention names a reference picture the model is shown: keep it as written, without describing what it looks like."
            }
            Purpose::Fill => {
                "The prompt describes what to paint inside a selected area of an existing picture. Describe only what goes in that area, so it blends with the picture around it; don't describe the rest of the picture."
            }
            Purpose::Restyle => {
                "The prompt describes what an existing picture should become (its style, look or medium); the picture's composition is kept, so describe the look rather than a new scene."
            }
        }
    }
}

/// What the text model is told when the settings have no instructions of their own.
pub const DEFAULT_INSTRUCTIONS: &str = "You improve prompts for an AI image generator. Rewrite the user's prompt so the image model gets a clear, vivid, specific description. Keep the user's intent, subject and every detail they gave; fill in what they left open and what helps a picture: details of the subject, setting, composition and framing, lighting, colour, mood, and style or medium. Keep names, text to be drawn in the image (in quotes) and @mentions (a word right after @, naming a reference picture) exactly as written, and never add an @mention the user didn't write. Don't add text, logos or watermarks the user didn't ask for. Write in the language of the user's prompt, as one paragraph. Answer with the improved prompt only: no title, no quotes around it, no explanation.";

/// One request to enhance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    pub prompt: String,
    pub purpose: Purpose,
    /// The image model the prompt is for (its name), when known: models differ in what they
    /// follow best.
    pub image_model: String,
    /// The style preset the image is rendered in (its sentence is added after the prompt): the
    /// prompt is written for that look, and never asks for another.
    pub style: Option<&'static crate::style::Style>,
}

/// An enhanced prompt, and who wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct Enhanced {
    pub prompt: String,
    pub provider: TextProvider,
    pub model: String,
}

/// The system instructions for `ask`.
pub fn instructions(settings: &EnhanceSettings, ask: &Ask) -> String {
    let own = settings.instructions.trim();
    let mut text = match (own.is_empty(), ask.style) {
        // With a style preset the look is the preset's: the built-in instructions don't ask for one.
        (true, Some(_)) => DEFAULT_INSTRUCTIONS.replacen(PICK_A_LOOK, "", 1),
        (true, None) => DEFAULT_INSTRUCTIONS.to_string(),
        (false, _) => own.to_string(),
    };
    // The length is a setting of its own, so it holds with the user's instructions too.
    text.push_str(&format!(" Keep it to at most about {} words.", settings.words()));
    text.push_str("\n\n");
    text.push_str(ask.purpose.note());
    let model = ask.image_model.trim();
    if !model.is_empty() {
        let model: String = model.chars().filter(|c| !c.is_control()).take(120).collect();
        text.push_str(&format!(" The image model is {model}."));
    }
    if let Some(style) = ask.style {
        text.push_str("\n\n");
        text.push_str(&style_rules(style));
    }
    text
}

/// Words that ask for a look: the ones a style preset's own sentence doesn't use are left out
/// of a prompt written for it.
const LOOK_WORDS: [&str; 20] = [
    "photorealistic",
    "hyperrealistic",
    "photograph",
    "photo",
    "shot on",
    "lens",
    "bokeh",
    "depth of field",
    "cinematic",
    "film still",
    "3D render",
    "CGI",
    "illustration",
    "painting",
    "watercolour",
    "sketch",
    "anime",
    "cartoon",
    "pixel art",
    "vector",
];

/// The part of the built-in instructions that asks for a look: left out when a style preset
/// gives the look.
const PICK_A_LOOK: &str = ", and style or medium";

/// What the text model is told about a style preset: the preset decides the look, so the
/// prompt is about the content only, drops the user's words for another look, and asks for
/// nothing the style can't show (colours in a monochrome one, a lens in a painting).
fn style_rules(style: &crate::style::Style) -> String {
    let look = style.prompt.to_ascii_lowercase();
    let monochrome = monochrome(style);
    let few_colours = !monochrome && ["limited", "restricted palette", "two or three", "few colours"].iter().any(|w| look.contains(w));
    let photograph = style.group == crate::style::StyleGroup::Photography;
    let mut limits = String::from("Describe only what this style can show.");
    if monochrome {
        limits.push_str(" It is monochrome: name no colours at all; describe tone, light and shadow instead.");
    } else if few_colours {
        limits.push_str(" Its palette is limited: name at most a few broad colours, and no gradients or subtle colour effects.");
    }
    if !photograph {
        limits.push_str(" It is not a photograph: no camera, lens, film stock, depth of field, bokeh or other photographic terms.");
    }
    // Spelled out: a fast model follows a list of words better than a rule. A word the
    // preset's own sentence uses (a cinematic still, a photographed clay set) is not on it.
    let leave_out = leave_out(style);
    if !leave_out.is_empty() {
        limits.push_str(&format!(" Never write these, even when the user did: {}.", leave_out.join(", ")));
    }
    format!(
        "A style preset decides how the image looks: after your prompt the app adds this sentence: \"{sentence}\" These rules come before keeping the user's words:\n\
         - Describe only the content: the subject, what is happening, the setting, composition, mood and light. Don't repeat or paraphrase the style sentence, and don't name a medium, art style or rendering technique yourself.\n\
         - If the user's prompt asks for another look or medium (photorealistic, a photo, a 3D render, cinematic, a camera, lens or film, a painting or drawing technique), leave those words out: the {name} preset replaces them.\n\
         - {limits}",
        sentence = style.prompt,
        name = style.name,
    )
}

/// Is `style` monochrome (black and white, pencil, line art)? Its prompt names no colours.
fn monochrome(style: &crate::style::Style) -> bool {
    let look = style.prompt.to_ascii_lowercase();
    look.contains("monochrome") || look.contains("black line art")
}

/// The look words a prompt written for `style` must not have: [`LOOK_WORDS`] less the ones its
/// own sentence uses (a cinematic still, a photographed clay set), and less the camera words for
/// a photograph.
fn leave_out(style: &crate::style::Style) -> Vec<&'static str> {
    let look = style.prompt.to_ascii_lowercase();
    let photograph = style.group == crate::style::StyleGroup::Photography;
    let camera = ["photograph", "photo", "shot on", "lens", "bokeh", "depth of field"];
    LOOK_WORDS.iter().copied().filter(|w| !look.contains(&w.to_ascii_lowercase())).filter(|w| !(photograph && camera.contains(w))).collect()
}

/// A short reminder of the style preset's rules, put after the prompt itself for a small local
/// model: it follows what is next to the prompt better than what the instructions said. `None`
/// without a preset.
pub fn reminder(ask: &Ask) -> Option<String> {
    let style = ask.style?;
    let words = leave_out(style);
    let mut text = format!("(Write it for the {} style: describe the content only", style.name);
    if monochrome(style) {
        text.push_str(", name no colours at all (it is black and white: describe tone, light and shadow)");
    }
    if !words.is_empty() {
        text.push_str(&format!(", and leave out these words even where the prompt has them: {}", words.join(", ")));
    }
    text.push_str(".)");
    Some(text)
}

/// `answer` with the quotes back around the text of `prompt` that was quoted (words to be drawn
/// in the picture), where a model kept the words but dropped the quotes.
pub fn keep_quotes(prompt: &str, answer: &str) -> String {
    let mut out = answer.to_string();
    for (open, close) in [('"', '"'), ('\u{201c}', '\u{201d}')] {
        let mut rest = prompt;
        // A handful at most: a prompt with more quotes than that isn't asking for drawn words.
        for _ in 0..8 {
            let Some(start) = rest.find(open) else { break };
            let after = rest.get(start + open.len_utf8()..).unwrap_or("");
            let Some(len) = after.find(close) else { break };
            let quoted = after.get(..len).unwrap_or("").trim();
            rest = after.get(len + close.len_utf8()..).unwrap_or("");
            if quoted.is_empty() || quoted.chars().count() > 200 {
                continue;
            }
            let already = [format!("\"{quoted}\""), format!("\u{201c}{quoted}\u{201d}"), format!("'{quoted}'")].iter().any(|q| out.contains(q.as_str()));
            if already {
                continue;
            }
            if let Some(at) = out.find(quoted) {
                let (Some(before), Some(tail)) = (out.get(..at), out.get(at + quoted.len()..)) else { continue };
                out = format!("{before}\"{quoted}\"{tail}");
            }
        }
    }
    out
}

/// `text` without the style sentence a model copied into it (the app adds it after the prompt
/// itself, so it would be there twice), however it was cased and with or without "Style:".
pub fn without_style(text: &str, style: &crate::style::Style) -> String {
    let look = style.prompt.trim_start_matches("Style:").trim().trim_end_matches('.');
    if look.len() < 12 {
        return text.to_string();
    }
    let mut out = text.to_string();
    // ASCII lowercasing keeps byte offsets, so a match in the lowered copy is one in `out`.
    while let Some(at) = out.to_ascii_lowercase().find(&look.to_ascii_lowercase()) {
        let mut start = at;
        let lowered = out.to_ascii_lowercase();
        if let Some(head) = lowered.get(..at)
            && head.trim_end().ends_with("style:")
        {
            start = head.trim_end().len() - "style:".len();
        }
        let end = (at + look.len()).min(out.len());
        let (Some(before), Some(after)) = (out.get(..start), out.get(end..)) else { break };
        let after = after.trim_start_matches('.');
        out = format!("{}{}", before.trim_end().trim_end_matches([',', ';', ':']).trim_end(), after);
    }
    let out = out.trim();
    if out.is_empty() || out.ends_with(['.', '!', '?', '"', '\u{201d}']) { out.to_string() } else { format!("{out}.") }
}

/// A model id the request can carry: letters, digits and `.-_/:@`. Gemini's ids are put in the
/// address, so they may not have a `/` (after `models/`) or `:`.
fn model_id(provider: TextProvider, model: &str) -> Result<String> {
    let mut id = model.trim();
    if provider == TextProvider::Gemini {
        id = id.strip_prefix("models/").unwrap_or(id);
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') || (provider != TextProvider::Gemini && matches!(c, '/' | ':' | '@'));
    if id.is_empty() {
        return Err(Error::Config(format!("choose a {} model", provider.short_label())));
    }
    if id.len() > MAX_MODEL_ID || !id.chars().all(allowed) {
        return Err(Error::Config(format!("`{}` is not a {} model id", id.chars().take(60).collect::<String>(), provider.short_label())));
    }
    Ok(id.to_string())
}

fn checked_key(provider: TextProvider, key: &str) -> Result<String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Config(format!("{} needs an API key: add one in Preferences › Generative AI › API Keys", provider.label())));
    }
    if key.len() > 2048 || key.chars().any(|c| c.is_control() || c == ' ') {
        return Err(Error::Config(format!("the {} API key is not valid: paste it again", provider.label())));
    }
    Ok(key.to_string())
}

/// The request that asks `provider`'s `model` to rewrite `prompt` following `system`.
pub fn request(provider: TextProvider, model: &str, key: &str, system: &str, prompt: &str, timeout_seconds: u64) -> Result<Request> {
    if provider.is_local() {
        return Err(Error::Config(LOCAL_IS_NO_SERVICE.into()));
    }
    let model = model_id(provider, model)?;
    let key = checked_key(provider, key)?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(Error::Config("write a prompt first".into()));
    }
    if prompt.chars().count() > MAX_PROMPT {
        return Err(Error::Config(format!("the prompt is longer than {MAX_PROMPT} characters")));
    }
    let chat = |url: &str| {
        let body = json!({"model": model, "messages": [{"role": "system", "content": system}, {"role": "user", "content": prompt}]});
        Request::post_json(url, body.to_string().into_bytes()).header("Authorization", &format!("Bearer {key}"))
    };
    let request = match provider {
        TextProvider::Openai => chat("https://api.openai.com/v1/chat/completions"),
        TextProvider::Xai => chat("https://api.x.ai/v1/chat/completions"),
        TextProvider::Openrouter => chat("https://openrouter.ai/api/v1/chat/completions"),
        TextProvider::Anthropic => {
            let body = json!({"model": model, "max_tokens": ANTHROPIC_MAX_TOKENS, "system": system, "messages": [{"role": "user", "content": prompt}]});
            Request::post_json("https://api.anthropic.com/v1/messages", body.to_string().into_bytes())
                .header("x-api-key", &key)
                .header("anthropic-version", ANTHROPIC_VERSION)
        }
        TextProvider::Gemini => {
            let body = json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"role": "user", "parts": [{"text": prompt}]}],
            });
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
            Request::post_json(url, body.to_string().into_bytes()).header("x-goog-api-key", &key)
        }
        TextProvider::Local => return Err(Error::Config(LOCAL_IS_NO_SERVICE.into())),
    };
    // The watch in `enhance` ends the wait at the timeout; the connection's own is a little
    // longer, so it is not the one that fires.
    Ok(request.timeout(timeout_seconds.saturating_add(2)))
}

/// What an error answer says, and what to do about it.
fn api_error(provider: TextProvider, model: &str, status: u16, v: &Value) -> Error {
    let who = provider.short_label();
    let said = v
        .pointer("/error/message")
        .or_else(|| v.pointer("/message"))
        .or_else(|| v.get("error").filter(|e| e.is_string()))
        .and_then(Value::as_str)
        .map(|m| m.trim().trim_end_matches('.').chars().take(400).collect::<String>())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let hint = match status {
        401 | 403 => " Check the API key in Preferences › Generative AI › Prompt Enhancement.".to_string(),
        404 => format!(" `{model}` may not be one of its models: choose another in Preferences › Generative AI › Prompt Enhancement."),
        402 | 429 => " The account may be out of credit, or asked too often: wait a moment, or check its billing.".to_string(),
        _ => String::new(),
    };
    Error::Server(format!("{who} refused the request: {said}.{hint}"))
}

fn json_of(provider: TextProvider, body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Error::BadResponse(format!("{} answered with something that isn't JSON", provider.short_label())))
}

/// The text of the answer to [`request`], cleaned up ([`clean`]).
pub fn parse(provider: TextProvider, model: &str, status: u16, body: &[u8]) -> Result<String> {
    let who = provider.short_label();
    let v = match json_of(provider, body) {
        Ok(v) => v,
        Err(e) if (200..300).contains(&status) => return Err(e),
        Err(_) => Value::Null,
    };
    if !(200..300).contains(&status) || v.get("error").is_some_and(|e| !e.is_null()) {
        return Err(api_error(provider, model, status, &v));
    }
    let text = match provider {
        TextProvider::Openai | TextProvider::Xai | TextProvider::Openrouter => {
            let choice = v.pointer("/choices/0").ok_or_else(|| Error::BadResponse(format!("{who} answered without a choice")))?;
            let message = choice.get("message");
            let text = match message.and_then(|m| m.get("content")) {
                Some(Value::String(s)) => s.clone(),
                // Some answer in parts.
                Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
                _ => String::new(),
            };
            if text.trim().is_empty() {
                if let Some(refusal) = message.and_then(|m| m.get("refusal")).and_then(Value::as_str).filter(|r| !r.trim().is_empty()) {
                    return Err(Error::Server(format!("{who} declined: {}", refusal.trim())));
                }
                if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
                    return Err(Error::BadResponse(format!("{who} ran out of room before it answered: choose another model")));
                }
            }
            text
        }
        TextProvider::Anthropic => {
            let parts = v.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
            let text: String = parts.iter().filter(|p| p.get("type").and_then(Value::as_str) == Some("text")).filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
            if text.trim().is_empty() && v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
                return Err(Error::Server(format!("{who} declined to rewrite this prompt")));
            }
            text
        }
        TextProvider::Local => return Err(Error::Config(LOCAL_IS_NO_SERVICE.into())),
        TextProvider::Gemini => {
            if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
                return Err(Error::Server(format!("{who} blocked the prompt ({reason})")));
            }
            let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array).cloned().unwrap_or_default();
            // Its thoughts, when it shares them, are not the answer.
            parts
                .iter()
                .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect()
        }
    };
    let text = clean(&text);
    if text.is_empty() {
        return Err(Error::BadResponse(format!("{who} answered with no text")));
    }
    Ok(text)
}

/// The prompt in a model's answer: without a code fence, a "Prompt:" label or quotes around it,
/// and no longer than [`MAX_PROMPT`].
pub fn clean(text: &str) -> String {
    let mut t = text.trim();
    // A fenced block: what is inside it.
    if let Some(inner) = t.strip_prefix("```") {
        let inner = inner.split_once('\n').map_or("", |(_, rest)| rest);
        t = inner.trim_end().strip_suffix("```").unwrap_or(inner).trim();
    }
    for label in ["enhanced prompt:", "improved prompt:", "rewritten prompt:", "prompt:"] {
        if t.get(..label.len()).is_some_and(|head| head.eq_ignore_ascii_case(label)) {
            t = t.get(label.len()..).unwrap_or("").trim_start();
            break;
        }
    }
    for (open, close) in [('"', '"'), ('\u{201c}', '\u{201d}'), ('\'', '\'')] {
        if let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close))
            && !inner.contains(open)
            && !inner.contains(close)
        {
            t = inner.trim();
        }
    }
    t.chars().take(MAX_PROMPT).collect::<String>().trim().to_string()
}

/// The request that lists `provider`'s models. OpenRouter's list is public: its key is sent
/// when there is one.
pub fn models_request(provider: TextProvider, key: &str) -> Result<Request> {
    let request = match provider {
        TextProvider::Openrouter => {
            let r = Request::get("https://openrouter.ai/api/v1/models");
            if key.trim().is_empty() { r } else { r.header("Authorization", &format!("Bearer {}", checked_key(provider, key)?)) }
        }
        TextProvider::Openai => Request::get("https://api.openai.com/v1/models").header("Authorization", &format!("Bearer {}", checked_key(provider, key)?)),
        TextProvider::Xai => Request::get("https://api.x.ai/v1/models").header("Authorization", &format!("Bearer {}", checked_key(provider, key)?)),
        TextProvider::Gemini => {
            Request::get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000").header("x-goog-api-key", &checked_key(provider, key)?)
        }
        TextProvider::Anthropic => Request::get("https://api.anthropic.com/v1/models?limit=1000")
            .header("x-api-key", &checked_key(provider, key)?)
            .header("anthropic-version", ANTHROPIC_VERSION),
        TextProvider::Local => return Err(Error::Config(LOCAL_IS_NO_SERVICE.into())),
    };
    Ok(request.timeout(30))
}

/// Words in a model id that say it doesn't write text answers (images, speech, embeddings…).
const NOT_TEXT: [&str; 22] = [
    "image",
    "imagine",
    "imagen",
    "video",
    "veo",
    "audio",
    "realtime",
    "transcribe",
    "tts",
    "whisper",
    "embedding",
    "moderation",
    "dall-e",
    "search",
    "codex",
    "live",
    "lyria",
    "robotics",
    "computer-use",
    "deep-research",
    "banana",
    "instruct",
];

/// The models in an answer to [`models_request`] that can enhance a prompt: newest first where
/// the answer dates them.
pub fn parse_models(provider: TextProvider, status: u16, body: &[u8]) -> Result<Vec<String>> {
    let v = match json_of(provider, body) {
        Ok(v) => v,
        Err(e) if (200..300).contains(&status) => return Err(e),
        Err(_) => Value::Null,
    };
    if !(200..300).contains(&status) {
        return Err(api_error(provider, "", status, &v));
    }
    let entries = match provider {
        TextProvider::Gemini => v.get("models"),
        _ => v.get("data"),
    }
    .and_then(Value::as_array)
    .ok_or_else(|| Error::BadResponse(format!("{} answered without a list of models", provider.short_label())))?;
    let mut found: Vec<(String, Option<i64>)> = Vec::new();
    for e in entries {
        let id = match provider {
            TextProvider::Gemini => {
                let writes = e.get("supportedGenerationMethods").and_then(Value::as_array).is_some_and(|m| m.iter().any(|x| x.as_str() == Some("generateContent")));
                if !writes {
                    continue;
                }
                e.get("name").and_then(Value::as_str).map(|n| n.strip_prefix("models/").unwrap_or(n))
            }
            _ => e.get("id").and_then(Value::as_str),
        };
        let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else { continue };
        let lower = id.to_ascii_lowercase();
        // OpenRouter says what each model answers with; a batch model answers later, not now.
        let text_out = e.pointer("/architecture/output_modalities").and_then(Value::as_array).is_none_or(|m| m.iter().any(|x| x.as_str() == Some("text")));
        let skip = !text_out
            || lower.ends_with(":batch")
            || (provider != TextProvider::Openrouter && NOT_TEXT.iter().any(|w| lower.contains(w)))
            || (provider == TextProvider::Openai && !(lower.starts_with("gpt-") || lower.starts_with("chatgpt-") || lower.starts_with('o')))
            || model_id(provider, id).is_err();
        if skip || found.iter().any(|(f, _)| f == id) {
            continue;
        }
        found.push((id.to_string(), e.get("created").and_then(Value::as_i64)));
        if found.len() >= MAX_LISTED {
            break;
        }
    }
    if found.iter().all(|(_, created)| created.is_some()) {
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    Ok(found.into_iter().map(|(id, _)| id).collect())
}

/// The models `provider` offers for enhancing, asked with `key`.
pub fn list_models(transport: &dyn Transport, provider: TextProvider, key: &str) -> Result<Vec<String>> {
    let request = models_request(provider, key)?;
    let response = transport.send(&request).map_err(|e| Error::Network(format!("Could not reach {}: {e}", provider.short_label())))?;
    parse_models(provider, response.status, &response.body)
}

/// Rewrite `ask.prompt` with the service in `settings`. Blocks until the answer arrives, the
/// timeout passes or `control` reports a cancel, reporting the time waited meanwhile.
pub fn enhance(transport: &Arc<dyn Transport>, settings: &EnhanceSettings, key: &str, ask: &Ask, control: &Control<'_>) -> Result<Enhanced> {
    let provider = settings.provider;
    if provider.is_local() {
        return Err(Error::Config(LOCAL_IS_NO_SERVICE.into()));
    }
    let model = settings.model(provider);
    let timeout = settings.timeout();
    let system = instructions(settings, ask);
    let request = request(provider, &model, key, &system, &ask.prompt, timeout)?;
    if (control.cancelled)() {
        return Err(Error::Cancelled);
    }
    let who = provider.short_label();
    (control.progress)(0.02, &format!("Connecting to {who}…"));
    let response = send_watched(transport, request, control, &format!("Waiting for {who} ({model})…"), timeout)?;
    (control.progress)(0.97, "Reading the answer…");
    let mut prompt = keep_quotes(&ask.prompt, &parse(provider, &model, response.status, &response.body)?);
    // A model that copied the style sentence: the app adds it once already.
    if let Some(style) = ask.style {
        prompt = without_style(&prompt, style);
        if prompt.is_empty() {
            return Err(Error::BadResponse(format!("{who} answered with the style alone")));
        }
    }
    Ok(Enhanced { prompt, provider, model })
}

/// How far along the bar is after `seconds` with no word: it creeps towards the end and never
/// gets there, so it moves however long the answer takes.
fn creep(seconds: f32) -> f32 {
    0.05 + 0.85 * (1.0 - 1.0 / (1.0 + seconds.max(0.0) / 6.0))
}

/// Send `request` on a helper thread and wait for it here, checking for a cancel and the
/// timeout ten times a second and saying how long it has been. A cancelled request's thread is
/// left to end on its own (its answer is dropped).
#[cfg(not(target_arch = "wasm32"))]
fn send_watched(transport: &Arc<dyn Transport>, request: Request, control: &Control<'_>, waiting: &str, timeout: u64) -> Result<Response> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};
    let (tx, rx) = std::sync::mpsc::channel();
    let sender = Arc::clone(transport);
    let sent = request.clone();
    let spawned = std::thread::Builder::new().name("photocraft-enhance".into()).spawn(move || {
        // The receiver is gone when the wait was cancelled: nobody to tell.
        let _ = tx.send(sender.send(&sent));
    });
    let unreachable = |e: Error| match e {
        Error::Network(m) => Error::Network(format!("Could not reach the service: {m}")),
        e => e,
    };
    if spawned.is_err() {
        // No thread to spare: wait for it here, without the watch.
        return transport.send(&request).map_err(unreachable);
    }
    let start = Instant::now();
    loop {
        if (control.cancelled)() {
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => return answer.map_err(unreachable),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(Error::Network("the request ended without an answer".into())),
        }
        let elapsed = start.elapsed();
        if elapsed.as_secs() >= timeout {
            return Err(Error::Network(format!(
                "no answer within {timeout} s. Try again, or allow longer in Preferences › Generative AI › Prompt Enhancement › Timeout"
            )));
        }
        (control.progress)(creep(elapsed.as_secs_f32()), &format!("{waiting} {} s of {timeout} s", elapsed.as_secs()));
    }
}

/// The browser has no threads to wait on (nor a transport): the request is sent as it is.
#[cfg(target_arch = "wasm32")]
fn send_watched(transport: &Arc<dyn Transport>, request: Request, _: &Control<'_>, _: &str, _: u64) -> Result<Response> {
    transport.send(&request)
}

#[cfg(test)]
mod tests;
