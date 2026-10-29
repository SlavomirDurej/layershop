//! What ComfyUI says about a render while it runs (the messages of its `/ws`), in words: which
//! step of the workflow it is on, and how far its sampler has got.

use std::collections::HashMap;

use serde_json::Value;

/// A step of a render, by the kind of node ComfyUI is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Reading the text encoder from disk (ComfyUI does this first, to read the prompt).
    LoadText,
    /// Reading the diffusion model from disk.
    Load,
    /// Reading the VAE from disk.
    LoadVae,
    /// Turning the prompt into what the model reads.
    Prompt,
    /// Reading the picture given (an edit or an inpainting).
    Picture,
    /// Sampling: the render itself.
    Render,
    /// Turning the result into an image.
    Decode,
    /// Handing the image over.
    Save,
}

/// The phase of a node of `class` (its `class_type`); `None` for the small nodes in between,
/// which leave the phase as it was.
pub(crate) fn phase_of(class: &str) -> Option<Phase> {
    Some(match class {
        "KSampler" | "KSamplerAdvanced" | "SamplerCustom" | "SamplerCustomAdvanced" => Phase::Render,
        c if c.starts_with("VAEDecode") => Phase::Decode,
        c if c.starts_with("VAEEncode") || c.starts_with("LoadImage") => Phase::Picture,
        c if c.contains("TextEncode") => Phase::Prompt,
        c if c.ends_with("CLIPLoader") => Phase::LoadText,
        "VAELoader" => Phase::LoadVae,
        c if c.ends_with("Loader") || c.ends_with("LoaderSimple") || c.ends_with("LoaderModelOnly") => Phase::Load,
        c if c.starts_with("SaveImage") || c.starts_with("PreviewImage") => Phase::Save,
        _ => return None,
    })
}

/// A render as ComfyUI reports it.
#[derive(Debug, Default)]
pub(crate) struct Live {
    /// The node ids of the workflow and their classes.
    classes: HashMap<String, String>,
    /// ComfyUI has begun this render (until then it is waiting in its queue).
    started: bool,
    phase: Option<Phase>,
    /// The sampler's step and its number of steps.
    step: Option<(u64, u64)>,
}

impl Live {
    pub fn new(workflow: &Value) -> Self {
        let classes = workflow
            .as_object()
            .map(|nodes| nodes.iter().filter_map(|(id, node)| Some((id.clone(), node.get("class_type")?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        Self { classes, ..Self::default() }
    }

    /// Take one of ComfyUI's messages. Anything it does not know is ignored.
    pub fn take(&mut self, text: &str) {
        let Ok(message) = serde_json::from_str::<Value>(text) else { return };
        let data = message.get("data").unwrap_or(&Value::Null);
        match message.get("type").and_then(Value::as_str).unwrap_or("") {
            "execution_start" => self.started = true,
            "executing" => {
                let Some(node) = data.get("node").and_then(Value::as_str) else { return };
                self.started = true;
                let phase = self.classes.get(node).and_then(|class| phase_of(class));
                if let Some(phase) = phase.filter(|p| Some(*p) != self.phase) {
                    self.phase = Some(phase);
                    self.step = None;
                }
            }
            "progress" if self.phase == Some(Phase::Render) => {
                let (value, max) = (data.get("value").and_then(Value::as_u64), data.get("max").and_then(Value::as_u64));
                if let (Some(value), Some(max)) = (value, max)
                    && max > 0
                {
                    self.step = Some((value.min(max), max));
                }
            }
            _ => {}
        }
    }

    /// Where the render is, as a fraction of the whole and in words (`label` names the model);
    /// `None` until ComfyUI has said anything about it.
    pub fn words(&self, label: &str) -> Option<(f32, String)> {
        if !self.started {
            return None;
        }
        Some(match (self.phase?, self.step) {
            // In the order ComfyUI usually takes them; the bar never goes back whatever the order.
            (Phase::LoadText, _) => (0.04, format!("Loading {label}'s text encoder\u{2026}")),
            (Phase::Prompt, _) => (0.08, "Reading the prompt\u{2026}".to_string()),
            (Phase::Load, _) => (0.12, format!("Loading {label}\u{2026}")),
            (Phase::LoadVae, _) => (0.14, format!("Loading {label}'s VAE\u{2026}")),
            (Phase::Picture, _) => (0.15, "Reading the picture\u{2026}".to_string()),
            (Phase::Render, None) => (0.18, format!("Loading {label} into memory\u{2026}")),
            (Phase::Render, Some((done, total))) => (0.2 + 0.7 * done as f32 / total.max(1) as f32, format!("Rendering with {label}: step {done} of {total}")),
            (Phase::Decode, _) => (0.92, "Finishing the image\u{2026}".to_string()),
            (Phase::Save, _) => (0.95, "Finishing the image\u{2026}".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workflow() -> Value {
        json!({
            "1": {"class_type": "UNETLoader"}, "2": {"class_type": "CLIPLoader"}, "3": {"class_type": "TextEncodeQwenImage21"},
            "4": {"class_type": "EmptySD3LatentImage"}, "5": {"class_type": "KSampler"}, "6": {"class_type": "VAEDecode"},
            "7": {"class_type": "PreviewImage"}, "8": {"class_type": "LoadImage"}, "9": {"class_type": "KSamplerSelect"}, "10": {"class_type": "VAELoader"},
        })
    }

    fn said(live: &Live) -> Option<String> {
        live.words("MOSS Image").map(|(_, w)| w)
    }

    /// A render as ComfyUI 0.42 reports it to the client that queued it, message by message, in
    /// the order a Z-Image Turbo render on it took (text encoder first, the model after the prompt).
    #[test]
    fn a_render_reads_as_its_steps_and_the_bar_only_moves_on() {
        let mut live = Live::new(&workflow());
        live.take(r#"{"type":"status","data":{"status":{"exec_info":{"queue_remaining":1}}}}"#);
        assert_eq!(said(&live), None, "queued: nothing said yet");
        let mut last = 0.0f32;
        for (message, want) in [
            (r#"{"type":"execution_start","data":{"prompt_id":"abc"}}"#, None),
            (r#"{"type":"execution_cached","data":{"nodes":[]}}"#, None),
            (r#"{"type":"executing","data":{"node":"2","prompt_id":"abc"}}"#, Some("Loading MOSS Image's text encoder…")),
            (r#"{"type":"executing","data":{"node":"3","prompt_id":"abc"}}"#, Some("Reading the prompt…")),
            (r#"{"type":"executing","data":{"node":"4","prompt_id":"abc"}}"#, Some("Reading the prompt…")),
            (r#"{"type":"executing","data":{"node":"1","prompt_id":"abc"}}"#, Some("Loading MOSS Image…")),
            (r#"{"type":"executing","data":{"node":"10","prompt_id":"abc"}}"#, Some("Loading MOSS Image's VAE…")),
            (r#"{"type":"executing","data":{"node":"9","prompt_id":"abc"}}"#, Some("Loading MOSS Image's VAE…")),
            (r#"{"type":"executing","data":{"node":"5","prompt_id":"abc"}}"#, Some("Loading MOSS Image into memory…")),
            (r#"{"type":"progress","data":{"value":7,"max":20,"node":"5"}}"#, Some("Rendering with MOSS Image: step 7 of 20")),
            (r#"{"type":"progress","data":{"value":20,"max":20,"node":"5"}}"#, Some("Rendering with MOSS Image: step 20 of 20")),
            (r#"{"type":"executing","data":{"node":"6","prompt_id":"abc"}}"#, Some("Finishing the image…")),
            (r#"{"type":"executing","data":{"node":"7","prompt_id":"abc"}}"#, Some("Finishing the image…")),
            (r#"{"type":"executing","data":{"node":null,"prompt_id":"abc"}}"#, Some("Finishing the image…")),
        ] {
            live.take(message);
            assert_eq!(said(&live).as_deref(), want, "after {message}");
            if let Some((fraction, _)) = live.words("x") {
                assert!(fraction >= last && fraction < 1.0, "{fraction} after {last}");
                last = fraction;
            }
        }
        assert!(live.started);
    }

    #[test]
    fn what_it_does_not_know_changes_nothing() {
        let mut live = Live::new(&workflow());
        for junk in [
            "",
            "not json",
            "[]",
            r#"{"type":"executing"}"#,
            r#"{"type":"executing","data":{"node":"404"}}"#,
            r#"{"type":"progress","data":{"value":3,"max":0}}"#,
        ] {
            live.take(junk);
        }
        assert_eq!(said(&live), None);
        // A progress bar of another node (an upscale, say) is not the render's.
        live.take(r#"{"type":"executing","data":{"node":"6"}}"#);
        live.take(r#"{"type":"progress","data":{"value":3,"max":9}}"#);
        assert_eq!(said(&live).as_deref(), Some("Finishing the image…"));
        // A step past the end reads as the end; a workflow that is not an object has no classes.
        let mut live = Live::new(&workflow());
        live.take(r#"{"type":"executing","data":{"node":"5"}}"#);
        live.take(r#"{"type":"progress","data":{"value":99,"max":20}}"#);
        assert_eq!(said(&live).as_deref(), Some("Rendering with MOSS Image: step 20 of 20"));
        assert!(Live::new(&json!([1, 2])).classes.is_empty());
        assert_eq!(phase_of("LoadImage"), Some(Phase::Picture));
        assert_eq!(phase_of("SamplerLCM"), None);
        assert_eq!(phase_of("CheckpointLoaderSimple"), Some(Phase::Load));
        assert_eq!((phase_of("DualCLIPLoader"), phase_of("QuadrupleCLIPLoader")), (Some(Phase::LoadText), Some(Phase::LoadText)));
        assert_eq!((phase_of("UNETLoader"), phase_of("LoraLoaderModelOnly")), (Some(Phase::Load), Some(Phase::Load)));
    }
}
