use super::*;
use photocraft_aigen::{Family, Request, Response};
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use std::sync::Mutex;

/// A ComfyUI that answers every workflow with one `w`×`h` image of `rgba`, and records what it
/// was asked.
struct FakeComfy {
    image: Vec<u8>,
    seen: Mutex<Vec<(String, Vec<u8>)>>,
}

impl FakeComfy {
    fn new(w: u32, h: u32, rgba: [u8; 4]) -> Arc<Self> {
        let img = Image::from_u8(w, h, ChannelLayout::Rgba, rgba.repeat((w * h) as usize)).unwrap();
        Arc::new(Self { image: photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default()).unwrap(), seen: Mutex::new(Vec::new()) })
    }

    fn queued(&self) -> Value {
        let seen = self.seen.lock().unwrap();
        let (_, body) = seen.iter().find(|(u, _)| u.ends_with("/prompt")).expect("nothing was queued");
        serde_json::from_slice(body).unwrap()
    }
}

impl Transport for FakeComfy {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        self.seen.lock().unwrap().push((r.url.clone(), r.body.clone()));
        let body = if r.url.ends_with("/prompt") {
            br#"{"prompt_id":"p1"}"#.to_vec()
        } else if r.url.contains("/history/") {
            br#"{"p1":{"outputs":{"9":{"images":[{"filename":"out.png","subfolder":"","type":"temp"}]}},"status":{"status_str":"success","completed":true}}}"#
                .to_vec()
        } else if r.url.contains("/view?") {
            self.image.clone()
        } else if r.url.contains("/models/") {
            br#"["a.safetensors","b.safetensors"]"#.to_vec()
        } else {
            return Ok(Response { status: 404, body: Vec::new() });
        };
        Ok(Response { status: 200, body })
    }
}

/// A 64×48 document, one local model, and a fake server behind it.
fn session(fake: &Arc<FakeComfy>) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let model = serde_json::to_value(Model { id: "z".into(), ..Family::ZImage.preset() }).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
    s.ai.transport = Some(fake.clone());
    s
}

fn px(s: &mut Session, x: i32, y: i32) -> Vec<f32> {
    serde_json::from_value(s.execute("document.pixel", json!({"x": x, "y": y})).unwrap()).unwrap()
}

fn layer_count(s: &Session) -> usize {
    s.active().unwrap().doc.layers.len()
}

#[test]
fn generates_onto_a_new_layer_over_the_selection() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let before = layer_count(&s);
    s.execute("select.rect", json!({"x": 8, "y": 4, "width": 16, "height": 16})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "  a green   field\nat dawn ", "seed": 5})).unwrap();
    assert_eq!(r["bounds"], json!([8, 4, 16, 16]));
    assert_eq!(r["model"], "z");
    assert_eq!(r["seed"], 5);
    // A square selection is rendered square, near one megapixel, in multiples of 16.
    assert_eq!(r["renderSize"], json!([1008, 1008]));
    assert_eq!(layer_count(&s), before + 1);
    let d = s.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    assert_eq!(layer.name, "a green field at dawn");
    assert!(layer.mask.is_some(), "the selection becomes the layer mask");
    assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(8, 4, 24, 20));
    // Green inside the selection, the white background outside it.
    let inside = px(&mut s, 12, 10);
    assert!(inside[0] < 0.05 && inside[1] > 0.95, "{inside:?}");
    let outside = px(&mut s, 40, 40);
    assert!(outside[0] > 0.95 && outside[1] > 0.95 && outside[2] > 0.95, "{outside:?}");
    // The workflow carried the prompt, the seed and the render size.
    let queued = fake.queued().to_string();
    assert!(queued.contains("a green   field") && queued.contains("\"seed\":5") && queued.contains("\"width\":1008"), "{queued}");
    // One undo step removes the layer.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), before);
}

#[test]
fn without_a_selection_the_canvas_is_filled_and_the_image_covers_it() {
    // A square image into a 64×48 canvas: scaled to cover, centred and cropped, no mask.
    let fake = FakeComfy::new(10, 10, [0, 0, 255, 255]);
    let mut s = session(&fake);
    let r = s.execute("ai.generate", json!({"prompt": "sea"})).unwrap();
    assert_eq!(r["bounds"], json!([0, 0, 64, 48]));
    let d = s.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    assert!(layer.mask.is_none());
    assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(0, 0, 64, 48));
    for (x, y) in [(0, 0), (63, 47), (32, 24)] {
        let p = px(&mut s, x, y);
        assert!(p[2] > 0.95 && p[0] < 0.05, "({x},{y}) {p:?}");
    }
    // An explicit rect wins over the canvas, and is clipped to it.
    let r = s.execute("ai.generate", json!({"prompt": "sea", "rect": [50, 40, 100, 100]})).unwrap();
    assert_eq!(r["bounds"], json!([50, 40, 14, 8]));
}

#[test]
fn works_at_every_bit_depth_and_in_grayscale() {
    for (mode, depth) in [("rgb", 8), ("rgb", 16), ("rgb", 32), ("gray", 8)] {
        let fake = FakeComfy::new(16, 16, [255, 255, 255, 255]);
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 32, "height": 32, "mode": mode, "depth": depth, "background": "transparent"})).unwrap();
        let model = serde_json::to_value(Model { id: "z".into(), ..Family::ZImage.preset() }).unwrap();
        s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
        s.ai.transport = Some(fake.clone());
        s.execute("ai.generate", json!({"prompt": "snow"})).unwrap_or_else(|e| panic!("{mode}/{depth}: {e}"));
        let d = s.active().unwrap();
        let surf = d.doc.layer(d.active_layer.unwrap()).unwrap().surface().unwrap();
        assert_eq!(surf.format().sample, d.doc.pixel_format().sample, "{mode}/{depth}");
        assert_eq!(surf.content_bounds(), Rect::new(0, 0, 32, 32), "{mode}/{depth}");
        assert!(surf.sample_channel(16, 16, 0) > 0.95, "{mode}/{depth}");
    }
}

#[test]
fn bad_params_and_missing_setup_are_errors_not_panics() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = session(&fake);
    for p in [
        json!({}),
        json!({"prompt": ""}),
        json!({"prompt": "   "}),
        json!({"prompt": 7}),
        json!({"prompt": "x", "model": "nope"}),
        json!({"prompt": "x", "seed": -1}),
        json!({"prompt": "x", "seed": "abc"}),
        json!({"prompt": "x", "rect": "all"}),
        json!({"prompt": "x", "rect": [1, 2, 3]}),
        json!({"prompt": "x", "rect": [0, 0, 0, 10]}),
        json!({"prompt": "x", "rect": [0, 0, -5, 10]}),
        json!({"prompt": "x", "rect": [1e300, 1e300, 1e300, 1e300]}),
        json!({"prompt": "x", "rect": [null, {}, [], "a"]}),
        json!({"prompt": "x", "rect": [9000, 9000, 10, 10]}),
        json!({"prompt": "x".repeat(MAX_PROMPT + 1)}),
        Value::Null,
        json!([1, 2]),
    ] {
        assert!(s.execute("ai.generate", p.clone()).is_err(), "{p}");
    }
    assert_eq!(layer_count(&s), 1, "a failed generation leaves the document alone");
    // Huge but finite coordinates are clamped, not overflowed.
    assert!(s.execute("ai.generate", json!({"prompt": "x", "rect": [-1e18, -1e18, 2e18, 2e18]})).is_ok());

    // No models, no transport, no document: each says so.
    let mut none = Session::new();
    let e = none.execute("ai.generate", json!({"prompt": "x"})).unwrap_err().to_string();
    assert!(e.contains("no document"), "{e}");
    none.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
    let e = none.execute("ai.generate", json!({"prompt": "x"})).unwrap_err().to_string();
    assert!(e.contains("desktop app"), "{e}");
    none.ai.transport = Some(fake.clone());
    let e = none.execute("ai.generate", json!({"prompt": "x"})).unwrap_err().to_string();
    assert!(e.contains("no AI model is set up"), "{e}");
}

#[test]
fn a_server_that_returns_garbage_is_an_error_and_the_document_is_untouched() {
    struct Garbage;
    impl Transport for Garbage {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            let body: &[u8] = if r.url.ends_with("/prompt") {
                br#"{"prompt_id":"p1"}"#
            } else if r.url.contains("/history/") {
                br#"{"p1":{"outputs":{"9":{"images":[{"filename":"out.png"}]}}}}"#
            } else {
                b"\x89PNG\r\n\x1a\n this is not a png"
            };
            Ok(Response { status: 200, body: body.to_vec() })
        }
    }
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = session(&fake);
    s.ai.transport = Some(Arc::new(Garbage));
    let e = s.execute("ai.generate", json!({"prompt": "x"})).unwrap_err().to_string();
    assert!(e.contains("not an image"), "{e}");
    assert_eq!(layer_count(&s), 1);
}

#[test]
fn cloud_models_need_a_key_and_keys_stay_out_of_the_preferences() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = session(&fake);
    let cloud = serde_json::to_value(Model { id: "gpt".into(), ..Model::cloud(Provider::Openai, "GPT Image 2", "gpt-image-2", false) }).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [cloud]})).unwrap();
    // The environment may hold a real key on a developer's machine; the test is about the stored one.
    if s.ai.key(Provider::Openai).is_empty() {
        let e = s.execute("ai.generate", json!({"prompt": "x"})).unwrap_err().to_string();
        assert!(e.contains("API key"), "{e}");
        assert_eq!(s.execute("ai.models", json!({})).unwrap()["models"][0]["ready"], false);
    }
    assert_eq!(s.execute("ai.setKey", json!({"provider": "openai", "key": " sk-secret-123 "})).unwrap(), json!({"provider": "openai", "set": true}));
    assert_eq!(s.execute("ai.keys", json!({})).unwrap()["openai"], true);
    assert_eq!(s.execute("ai.models", json!({})).unwrap()["models"][0]["ready"], true);
    // Nowhere an agent can read: preferences, their JSON, the journal, the runtime's debug print.
    assert!(!s.prefs_to_json().contains("sk-secret"));
    assert!(!s.execute("prefs.get", json!({})).unwrap().to_string().contains("sk-secret"));
    assert!(!format!("{:?}", s.journal).contains("sk-secret"));
    assert!(!format!("{:?}", s.ai).contains("sk-secret"));
    // Saved and restored through the app's own file.
    let saved = s.ai.keys_json();
    assert!(saved.contains("sk-secret-123"));
    let mut other = AiRuntime::default();
    other.load_keys_json(&saved);
    assert_eq!(other.key(Provider::Openai), "sk-secret-123");
    for junk in ["", "[]", "{\"openai\": 7}", "not json", "{\"comfy\": \"x\"}"] {
        other.load_keys_json(junk);
        assert!(!other.keys_json().contains("sk-"), "{junk}");
    }
    // Removing it, and hostile params.
    s.execute("ai.setKey", json!({"provider": "openai", "key": ""})).unwrap();
    assert!(!s.ai.keys_json().contains("sk-secret"));
    for p in [
        json!({}),
        json!({"provider": "comfy", "key": "x"}),
        json!({"provider": "openai"}),
        json!({"provider": "openai", "key": "a\nb"}),
        json!({"provider": "openai", "key": "k".repeat(5000)}),
    ] {
        assert!(s.execute("ai.setKey", p.clone()).is_err(), "{p}");
    }
}

#[test]
fn model_files_are_listed_from_the_server() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = session(&fake);
    let r = s.execute("ai.comfyModels", json!({})).unwrap();
    assert_eq!(r["folders"]["vae"], json!(["a.safetensors", "b.safetensors"]));
    assert!(s.execute("ai.comfyModels", json!({"url": "not a url"})).is_err());
    let mut none = Session::new();
    assert!(none.execute("ai.comfyModels", json!({})).is_err());
    let listed = s.execute("ai.models", json!({})).unwrap();
    assert_eq!(listed["models"][0]["id"], "z");
    assert_eq!(listed["models"][0]["local"], true);
    assert_eq!(listed["available"], true);
}

#[test]
fn settings_survive_a_preferences_round_trip() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let s = session(&fake);
    let saved = s.prefs_to_json();
    let mut again = Session::new();
    again.load_prefs_json(&saved).unwrap();
    assert_eq!(again.prefs().integrations.ai, s.prefs().integrations.ai);
    assert_eq!(again.prefs().integrations.ai.models[0].unet, "z_image_turbo_bf16.safetensors");
    // Preferences written before this section existed still load.
    again.load_prefs_json(r#"{"integrations":{"allowAgentControl":false}}"#).unwrap();
    assert!(again.prefs().integrations.ai.models.is_empty());
    assert_eq!(layer_name(&"word ".repeat(40)).chars().count(), 41);
}
