use super::*;
use photocraft_aigen::{Family, Request, Response};
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_doc::{Generation, GenerationMode, GenerationQuality, GenerationReference, GenerationSource};
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

    /// The PNG uploaded as `name`, decoded.
    fn uploaded(&self, name: &str) -> Image {
        let seen = self.seen.lock().unwrap();
        let body = seen.iter().filter(|(u, _)| u.ends_with("/upload/image")).map(|(_, b)| b).find(|b| String::from_utf8_lossy(b).contains(name));
        let body = body.unwrap_or_else(|| panic!("{name} was not uploaded"));
        let start = body.windows(8).position(|w| w == b"\x89PNG\r\n\x1a\n").expect("no PNG in the upload");
        // The file ends where its part does: at the PNG's IEND chunk (4 bytes of type, 4 of CRC).
        let end = body.windows(4).rposition(|w| w == b"IEND").expect("a truncated PNG") + 8;
        photocraft_codecs::decode(&body[start..end]).unwrap()
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
        } else if r.url.ends_with("/upload/image") {
            // Like ComfyUI: the file is kept under the name it was sent with.
            let text = String::from_utf8_lossy(&r.body);
            let name = text.split("filename=\"").nth(1).and_then(|t| t.split('"').next()).unwrap_or("");
            json!({"name": name, "subfolder": "", "type": "input"}).to_string().into_bytes()
        } else {
            return Ok(Response { status: 404, body: Vec::new() });
        };
        Ok(Response { status: 200, body })
    }
}

/// A 64×48 document, one local model (ticked for inpainting, which Z-Image is not unasked), and a
/// fake server behind it.
fn session(fake: &Arc<FakeComfy>) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let model = serde_json::to_value(Model { id: "z".into(), inpaint: Some(true), ..Family::ZImage.preset() }).unwrap();
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
    assert_eq!(layer.name, "[z_image_turbo] a green field at dawn");
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
    none.execute("prefs.set", json!({"path": "integrations.ai.models", "value": []})).unwrap();
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

/// Ideogram 4's refusal as it measured: flat grey 109 with a band of light type over about 1.5%.
fn refusal_png(w: u32, h: u32) -> Vec<u8> {
    let mut rgba = [109u8, 109, 109, 255].repeat((w * h) as usize);
    for y in (h * 9 / 20)..(h * 11 / 20) {
        for x in (w / 7)..(w * 6 / 7 / 2 + w / 7) {
            if x % 3 != 0 {
                rgba[((y * w + x) * 4) as usize..][..3].copy_from_slice(&[230, 230, 230]);
            }
        }
    }
    photocraft_codecs::encode(&Image::from_u8(w, h, ChannelLayout::Rgba, rgba).unwrap(), Format::Png, &EncodeOptions::default()).unwrap()
}

#[test]
fn the_refusal_picture_is_told_from_real_images() {
    assert!(is_refusal_picture(&refusal_png(160, 90)));
    // A colourful, busy picture is not.
    let busy: Vec<u8> = (0..160 * 90u32).flat_map(|i| [(i * 7 % 251) as u8, (i * 13 % 241) as u8, (i * 29 % 239) as u8, 255]).collect();
    let png = |rgba: Vec<u8>| {
        photocraft_codecs::encode(&Image::from_u8(160, 90, ChannelLayout::Rgba, rgba).unwrap(), Format::Png, &EncodeOptions::default()).unwrap()
    };
    assert!(!is_refusal_picture(&png(busy)));
    // Plain grey with nothing written on it is a grey image, not a refusal.
    assert!(!is_refusal_picture(&png([109, 109, 109, 255].repeat(160 * 90))));
    // Flat, but not that grey, or not grey at all.
    assert!(!is_refusal_picture(&png([200, 200, 200, 255].repeat(160 * 90))));
    assert!(!is_refusal_picture(&png([109, 40, 200, 255].repeat(160 * 90))));
    // Not an image, or an empty one.
    assert!(!is_refusal_picture(b""));
    assert!(!is_refusal_picture(b"\x89PNG\r\n\x1a\n this is not a png"));
}

/// An Ideogram 4 that draws its refusal for the first `refusals` images it is asked for, then a
/// green one.
struct Refuses {
    inner: Arc<FakeComfy>,
    refusal: Vec<u8>,
    refusals: usize,
    drawn: Mutex<usize>,
}

impl Transport for Refuses {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        let mut response = self.inner.send(r)?;
        if r.url.contains("/view?") {
            let mut drawn = self.drawn.lock().unwrap();
            if *drawn < self.refusals {
                response.body = self.refusal.clone();
            }
            *drawn += 1;
        }
        Ok(response)
    }
}

fn ideogram_session(refusals: usize) -> (Session, Arc<Refuses>) {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let transport = Arc::new(Refuses { inner: fake, refusal: refusal_png(64, 36), refusals, drawn: Mutex::new(0) });
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let model = serde_json::to_value(Model { id: "ideo".into(), ..Family::Ideogram4.preset() }).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
    s.ai.transport = Some(transport.clone());
    (s, transport)
}

fn queued_seeds(t: &Refuses) -> Vec<u64> {
    let seen = t.inner.seen.lock().unwrap();
    seen.iter()
        .filter(|(u, _)| u.ends_with("/prompt"))
        .map(|(_, b)| {
            let v: Value = serde_json::from_slice(b).unwrap();
            // Ideogram 4 seeds a RandomNoise node, the KSampler graphs a `seed`.
            v["prompt"]
                .as_object()
                .unwrap()
                .values()
                .find_map(|n| n["inputs"]["noise_seed"].as_u64().or_else(|| n["inputs"]["seed"].as_u64()))
                .expect("no seed in the workflow")
        })
        .collect()
}

#[test]
fn a_refused_seed_is_replaced_by_a_new_one_and_the_refusal_never_reaches_a_layer() {
    let (mut s, t) = ideogram_session(2);
    let r = s.execute("ai.generate", json!({"prompt": "a coffee shop"})).unwrap();
    let seeds = queued_seeds(&t);
    assert_eq!(seeds.len(), 3, "two refusals, then the image");
    assert!(seeds[0] != seeds[1] && seeds[1] != seeds[2], "{seeds:?}");
    // The layer remembers the seed that drew its image, so "render this one again" reproduces it.
    assert_eq!(r["seed"], seeds[2]);
    assert_eq!(layer_count(&s), 2);
    let p = px(&mut s, 32, 24);
    assert!(p[1] > 0.95 && p[0] < 0.05, "the green image, not the grey refusal: {p:?}");
}

#[test]
fn a_model_that_keeps_refusing_is_an_error_after_a_few_seeds_and_no_layer_is_added() {
    let (mut s, t) = ideogram_session(usize::MAX);
    let e = s.execute("ai.generate", json!({"prompt": "a coffee shop"})).unwrap_err().to_string();
    assert!(e.contains("Image blocked by safety filter") && e.contains("on 4 seeds in a row") && e.contains("not a PhotoCraft or ComfyUI filter"), "{e}");
    assert_eq!(queued_seeds(&t).len(), 4, "the first render and three more");
    assert_eq!(layer_count(&s), 1);
}

#[test]
fn a_seed_the_user_chose_is_not_replaced() {
    let (mut s, t) = ideogram_session(usize::MAX);
    let e = s.execute("ai.generate", json!({"prompt": "a coffee shop", "seed": 5})).unwrap_err().to_string();
    assert!(e.contains("at seed 5"), "{e}");
    assert_eq!(queued_seeds(&t), vec![5]);
    assert_eq!(layer_count(&s), 1);
}

#[test]
fn other_models_are_not_asked_to_render_a_grey_picture_again() {
    // Z-Image has no such refusal: a flat grey picture with light marks is simply its answer.
    let fake = FakeComfy::new(32, 32, [0, 0, 0, 255]);
    let mut s = session(&fake);
    let t = Arc::new(Refuses { inner: fake, refusal: refusal_png(64, 36), refusals: usize::MAX, drawn: Mutex::new(0) });
    s.ai.transport = Some(t.clone());
    s.execute("ai.generate", json!({"prompt": "a grey wall with chalk marks"})).unwrap();
    assert_eq!(queued_seeds(&t).len(), 1);
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

/// Connect finds a ComfyUI Desktop on port 8000 when the address says 8188, and says which
/// address answered; with nothing on either port it fails, naming both.
#[test]
fn connect_finds_the_other_local_port() {
    struct OnlyOn(&'static str);
    impl Transport for OnlyOn {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            if !r.url.starts_with(self.0) {
                return Err(photocraft_aigen::Error::Network("connection refused".into()));
            }
            let body: &[u8] = if r.url.ends_with("/system_stats") { br#"{"system":{"argv":["main.py","--port","8000"]}}"# } else { br#"["a.safetensors"]"# };
            Ok(Response { status: 200, body: body.to_vec() })
        }
    }
    let mut s = Session::new();
    s.ai.transport = Some(Arc::new(OnlyOn("http://127.0.0.1:8000/")));
    let r = s.execute("ai.comfyModels", json!({"url": "http://127.0.0.1:8188", "start": false, "findPort": true})).unwrap();
    assert_eq!(r["url"], "http://127.0.0.1:8000");
    assert_eq!(r["folders"]["vae"], json!(["a.safetensors"]));
    // Already right: kept.
    let r = s.execute("ai.comfyModels", json!({"url": "http://127.0.0.1:8000/", "start": false, "findPort": true})).unwrap();
    assert_eq!(r["url"], "http://127.0.0.1:8000");
    // Another computer is never swapped for this one's other port.
    assert!(s.execute("ai.comfyModels", json!({"url": "http://10.0.0.5:8188", "start": false, "findPort": true})).is_err());
    s.ai.transport = Some(Arc::new(OnlyOn("http://nowhere/")));
    let e = s.execute("ai.comfyModels", json!({"url": "http://127.0.0.1:8188", "start": false, "findPort": true})).unwrap_err().to_string();
    assert!(e.contains("127.0.0.1:8188") && e.contains("127.0.0.1:8000"), "{e}");
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
    // They get the models a new installation lists.
    again.load_prefs_json(r#"{"integrations":{"allowAgentControl":false}}"#).unwrap();
    assert_eq!(again.prefs().integrations.ai.models, Model::defaults());
    // A list the user emptied stays empty.
    again.load_prefs_json(r#"{"integrations":{"ai":{"models":[]}}}"#).unwrap();
    assert!(again.prefs().integrations.ai.models.is_empty());
    assert_eq!(layer_name(&Family::ZImage.preset(), &"word ".repeat(40)).chars().count(), "[z_image_turbo] ".len() + 41);
}

#[test]
fn inpainting_repaints_the_selected_part_of_the_layer_on_a_new_masked_layer() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let before = layer_count(&s);
    s.execute("select.rect", json!({"x": 24, "y": 16, "width": 16, "height": 16})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "a green pond", "mode": "inpaint", "strength": 0.75, "seed": 9})).unwrap();
    // The model is shown a quarter of the selection's edge more on every side: 4 pixels.
    assert_eq!(r["bounds"], json!([20, 12, 24, 24]));
    assert_eq!((r["mode"].as_str(), r["seed"].as_u64()), (Some("inpaint"), Some(9)));
    assert_eq!(r["renderSize"], json!([1008, 1008]));
    assert_eq!(layer_count(&s), before + 1);
    {
        let d = s.active().unwrap();
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(20, 12, 44, 36));
        assert_eq!(layer.name, "[z_image_turbo] a green pond");
        // The selection is the mask: the new layer shows inside it only.
        assert_eq!(layer.mask.as_ref().unwrap().surface.content_bounds(), Rect::new(24, 16, 40, 32));
        assert_eq!(d.history.entries().last().map(String::as_str), Some(INPAINT_LABEL));
    }
    let inside = px(&mut s, 30, 20);
    assert!(inside[0] < 0.05 && inside[1] > 0.95, "{inside:?}");
    let context = px(&mut s, 21, 13);
    assert!(context[0] > 0.95 && context[2] > 0.95, "outside the selection the layer below shows: {context:?}");
    // The model got the layer (white here) and the mask, at the size it renders.
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert_eq!((image.width(), image.height(), image.layout()), (1008, 1008, ChannelLayout::Rgb));
    assert!(image.data().iter().all(|b| *b == 255));
    let mask = fake.uploaded("photocraft_inpaint_mask.png");
    assert_eq!((mask.width(), mask.height(), mask.layout()), (1008, 1008, ChannelLayout::Rgba));
    let alpha = |x: usize, y: usize| mask.data()[(y * 1008 + x) * 4 + 3];
    // Transparent where the model paints: the selection (168..840 of 1008), opaque around it.
    assert_eq!((alpha(504, 504), alpha(300, 300), alpha(700, 700)), (0, 0, 0));
    assert_eq!((alpha(10, 10), alpha(100, 504), alpha(900, 504), alpha(504, 950)), (255, 255, 255, 255));
    // It fades in from the selection's edge, so what is painted joins what is kept: just inside
    // the edge the picture is mostly kept, a little further in half of it, then none.
    assert!(alpha(172, 504) > 200 && alpha(504, 836) > 200, "{} {}", alpha(172, 504), alpha(504, 836));
    assert!((40..215).contains(&alpha(200, 504)), "{}", alpha(200, 504));
    assert!(alpha(260, 504) < 10, "{}", alpha(260, 504));
    let queued = fake.queued().to_string();
    for want in [
        "SetLatentNoiseMask",
        "DifferentialDiffusion",
        "VAEEncode",
        "photocraft_inpaint_image.png",
        "photocraft_inpaint_mask.png",
        "\"denoise\":0.75",
        "a green pond",
    ] {
        assert!(queued.contains(want), "{want} missing in {queued}");
    }
    assert!(!queued.contains("EmptySD3LatentImage"));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), before);
    // `context` 0 shows the model the selection alone; a rect names the area outright.
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "inpaint", "context": 0})).unwrap();
    assert_eq!(r["bounds"], json!([24, 16, 16, 16]));
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "inpaint", "rect": [0, 0, 64, 48]})).unwrap();
    assert_eq!(r["bounds"], json!([0, 0, 64, 48]));
}

#[test]
fn inpainting_reads_the_layers_pixels_at_every_depth() {
    for (mode, depth) in [("rgb", 8), ("rgb", 16), ("rgb", 32), ("gray", 8), ("cmyk", 8)] {
        let fake = FakeComfy::new(16, 16, [255, 0, 0, 255]);
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 40, "height": 40, "mode": mode, "depth": depth, "background": "white"})).unwrap();
        let model = serde_json::to_value(Model { id: "z".into(), megapixels: 0.0, inpaint: Some(true), ..Family::ZImage.preset() }).unwrap();
        s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
        s.ai.transport = Some(fake.clone());
        s.execute("select.rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})).unwrap();
        s.execute("ai.generate", json!({"prompt": "x", "mode": "inpaint"})).unwrap_or_else(|e| panic!("{mode}/{depth}: {e}"));
        let image = fake.uploaded("photocraft_inpaint_image.png");
        // White paper in every colour model arrives as white.
        assert!(image.data().iter().all(|b| *b >= 250), "{mode}/{depth}: {:?}", &image.data()[..6]);
        let d = s.active().unwrap();
        assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().surface().unwrap().format().sample, d.doc.pixel_format().sample, "{mode}/{depth}");
    }
}

#[test]
fn inpainting_says_what_it_needs() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = session(&fake);
    let fails = |s: &mut Session, p: Value| s.execute("ai.generate", p).unwrap_err().to_string();
    // No selection.
    let e = fails(&mut s, json!({"prompt": "x", "mode": "inpaint"}));
    assert!(e.contains("needs a selection"), "{e}");
    assert_eq!(inpaint_blocked(&s).unwrap_err(), e);
    s.execute("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    assert_eq!(inpaint_blocked(&s), Ok(Rect::new(8, 8, 24, 24)));
    // Params that are not what they say.
    for p in [
        json!({"prompt": "x", "mode": "outpaint"}),
        json!({"prompt": "x", "mode": 7}),
        json!({"prompt": "x", "mode": "inpaint", "strength": 2}),
        json!({"prompt": "x", "mode": "inpaint", "strength": "high"}),
        json!({"prompt": "x", "mode": "inpaint", "strength": -0.1}),
        json!({"prompt": "x", "mode": "inpaint", "context": 99}),
        json!({"prompt": "x", "mode": "inpaint", "context": null}),
        json!({"prompt": "x", "mode": "inpaint", "rect": [40, 40, 10, 8]}),
    ] {
        assert!(s.execute("ai.generate", p.clone()).is_err(), "{p}");
    }
    // Z-Image generates; it does not edit.
    assert!(fails(&mut s, json!({"prompt": "x", "mode": "imgEdit"})).contains("can't edit a picture"));
    // A model switched off for inpainting, and one whose architecture can't.
    let models = [Model { id: "off".into(), inpaint: Some(false), ..Family::ZImage.preset() }, Model { id: "o1".into(), ..Family::HidreamO1.preset() }];
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": serde_json::to_value(&models).unwrap()})).unwrap();
    for id in ["off", "o1"] {
        let e = fails(&mut s, json!({"prompt": "x", "mode": "inpaint", "model": id}));
        assert!(e.contains("not set up for inpainting"), "{e}");
    }
    let listed = s.execute("ai.models", json!({})).unwrap();
    assert_eq!((listed["models"][0]["inpaint"].as_bool(), listed["models"][1]["inpaint"].as_bool()), (Some(false), Some(false)));
    // An empty layer has nothing to repaint; neither has a layer that is not pixels.
    let mut blank = session(&fake);
    blank.execute("layer.new.layer", json!({})).unwrap();
    blank.execute("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let e = fails(&mut blank, json!({"prompt": "x", "mode": "inpaint"}));
    assert!(e.contains("is empty inside the selection"), "{e}");
    blank.execute("layer.newAdjustmentLayer.invert", json!({})).unwrap();
    let e = fails(&mut blank, json!({"prompt": "x", "mode": "inpaint"}));
    assert!(e.contains("pixel layer") && e.contains("Adjustment"), "{e}");
    assert_eq!(layer_count(&s), 1, "a refused inpainting leaves the document alone");
    // Text to image is as it was: no selection needed, nothing uploaded.
    let plain = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut t = session(&plain);
    assert_eq!(t.execute("ai.generate", json!({"prompt": "x", "mode": "txt2img"})).unwrap()["mode"], "txt2img");
    assert!(!plain.seen.lock().unwrap().iter().any(|(u, _)| u.ends_with("/upload/image")));
}

#[test]
fn the_inpaint_area_grows_the_selection_within_the_canvas() {
    let canvas = Rect::new(0, 0, 1000, 800);
    assert_eq!(inpaint_area(Rect::new(400, 300, 600, 400), canvas, 0.5), Rect::new(300, 200, 700, 500));
    assert_eq!(inpaint_area(Rect::new(0, 0, 100, 100), canvas, 0.5), Rect::new(0, 0, 150, 150));
    assert_eq!(inpaint_area(Rect::new(400, 300, 600, 400), canvas, 0.0), Rect::new(400, 300, 600, 400));
    for hostile in [f64::NAN, f64::INFINITY, -3.0, 1e300] {
        let r = inpaint_area(Rect::new(400, 300, 600, 400), canvas, hostile);
        assert!(!r.is_empty() && r.intersect(&canvas) == r, "{hostile}: {r:?}");
    }
    assert_eq!(inpaint_area(Rect::new(i32::MAX - 5, 0, i32::MAX, 5), canvas, 4.0), Rect::EMPTY.intersect(&canvas));
}

#[test]
fn the_mask_fades_in_from_the_edge_of_what_it_covers_and_never_grows() {
    // A 40 x 30 picture, covered from x = 10 to 30 over its whole height.
    let (w, h) = (40usize, 30usize);
    let hard: Vec<f32> = (0..w * h).map(|i| if (10..30).contains(&(i % w)) { 1.0 } else { 0.0 }).collect();
    let mut soft = hard.clone();
    fade_inwards(&mut soft, w, h, 8, [false; 4]);
    let at = |x: usize, y: usize| soft[y * w + x];
    assert!(soft.iter().zip(&hard).all(|(s, m)| s <= m && (0.0..=1.0).contains(s)), "the fade only takes away");
    assert!(at(10, 15) < 0.2 && at(29, 15) < 0.2, "{} {}", at(10, 15), at(29, 15));
    assert!(at(20, 15) > 0.95, "{}", at(20, 15));
    assert!(at(12, 15) < at(14, 15) && at(14, 15) < at(18, 15));
    // The picture's own edge counts as an edge: there is nothing beyond it to join.
    assert!(at(20, 0) < 0.2 && at(20, 29) < 0.2);
    // Unless the mask goes on past it (the canvas's edge): there it stays whole.
    let mut open = hard.clone();
    fade_inwards(&mut open, w, h, 8, [false, true, false, true]);
    let at = |x: usize, y: usize| open[y * w + x];
    assert!(at(20, 0) > 0.95 && at(20, 29) > 0.95 && at(10, 15) < 0.2, "{} {} {}", at(20, 0), at(20, 29), at(10, 15));
    let mut all = vec![1.0; w * h];
    fade_inwards(&mut all, w, h, 8, [true; 4]);
    assert!(all.iter().all(|v| *v >= 1.0));
    // No band, an empty picture or a buffer of the wrong size: left alone, not a panic.
    for (len, w, h, band) in [(w * h, w, h, 0), (0, 0, 0, 8), (7, w, h, 8), (w * h, w, h, 100_000)] {
        for open in [[false; 4], [true; 4]] {
            let mut m = vec![1.0; len];
            fade_inwards(&mut m, w, h, band, open);
            assert!(m.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
    assert_eq!(box_blur_rows(&[], 0, 3), Vec::<f32>::new());
}

#[test]
fn layers_are_named_after_the_model_and_the_prompt() {
    let named = |name: &str| short_model_name(&Model { name: name.into(), id: "the-id".into(), ..Model::default() });
    for (name, want) in [
        ("flux 2 klein 9b fp8", "flux_2_klein"),
        ("flux 2 klein base 9b fp8", "flux_2_klein_base"),
        ("FLUX.2 Klein 9B", "flux.2_klein"),
        ("z image turbo bf16", "z_image_turbo"),
        ("qwen image 2512 fp8 e4m3fn", "qwen_image_2512"),
        ("qwen image 2.1 int8 convrot", "qwen_image_2.1"),
        ("ming image 0.1 design int8 convrot", "ming_image_0.1_design"),
        ("flux2 dev fp8mixed", "flux2_dev"),
        ("ltx 2.3 22b dev Q4_K_M", "ltx_2.3_dev_k_m"),
        ("GPT Image 2.5 Sunburst", "gpt_image_2.5_sunburst"),
        ("zImageUltimateNSFW v20", "zimageultimatensfw_v20"),
        ("Mój  modèle…", "mój_modèle"),
        // Nothing left of the name: the id, then a word.
        ("fp8 9b", "the_id"),
        ("", "the_id"),
    ] {
        assert_eq!(named(name), want, "{name}");
    }
    assert_eq!(short_model_name(&Model::default()), "ai");
    // Whole words, to a bounded length; one endless word is cut.
    let long = named("a very long model name that goes on and on and on");
    assert!(long.chars().count() <= 28 && !long.ends_with('_'), "{long}");
    assert_eq!(named(&"x".repeat(500)).chars().count(), 28);
    let m = Model { name: "flux 2 klein 9b fp8".into(), ..Model::default() };
    assert_eq!(layer_name(&m, "  Add here a\n beautiful deer "), "[flux_2_klein] Add here a beautiful deer");
}

#[test]
fn a_dedicated_inpainting_model_gets_the_selection_as_its_mask_and_does_nothing_else() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let models = [
        Model { id: "fill".into(), ..Family::Flux1Fill.preset() },
        Model { id: "qwen".into(), ..Family::QwenInpaint.preset() },
        Model { id: "z".into(), ..Family::ZImage.preset() },
    ];
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": serde_json::to_value(&models).unwrap()})).unwrap();
    let listed = s.execute("ai.models", json!({})).unwrap();
    let flags: Vec<(bool, bool)> =
        listed["models"].as_array().unwrap().iter().map(|m| (m["generate"].as_bool().unwrap(), m["inpaint"].as_bool().unwrap())).collect();
    assert_eq!(flags, [(false, true), (false, true), (true, false)]);
    // From a prompt alone it is refused in words; so is the first enabled model when it is one.
    for p in [json!({"prompt": "x", "model": "fill"}), json!({"prompt": "x", "model": "qwen", "mode": "txt2img"}), json!({"prompt": "x"})] {
        let e = s.execute("ai.generate", p.clone()).unwrap_err().to_string();
        assert!(e.contains("is an inpainting model"), "{p}: {e}");
    }
    s.execute("select.rect", json!({"x": 24, "y": 16, "width": 16, "height": 16})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "a pond", "mode": "inpaint", "model": "fill"})).unwrap();
    assert_eq!((r["mode"].as_str(), r["model"].as_str()), (Some("inpaint"), Some("fill")));
    let queued = fake.queued().to_string();
    assert!(queued.contains("InpaintModelConditioning") && queued.contains("flux1-fill-dev.safetensors"), "{queued}");
    // The model knows where the mask is: it gets the selection itself, whole up to its edge
    // (168..840 of 1008), not one that fades in (32 pixels inside, a faded mask still keeps half the picture).
    let mask = fake.uploaded("photocraft_inpaint_mask.png");
    let alpha = |x: usize, y: usize| mask.data()[(y * 1008 + x) * 4 + 3];
    assert_eq!((alpha(200, 504), alpha(220, 504), alpha(504, 808), alpha(504, 504)), (0, 0, 0, 0));
    assert_eq!((alpha(160, 504), alpha(504, 850)), (255, 255));
    let d = s.active().unwrap();
    assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().name, "[flux.1_fill_dev] a pond");
}

#[test]
fn image_to_image_redraws_a_layer_in_place() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let before = layer_count(&s);
    let background = s.active().unwrap().active_layer.unwrap().0;
    // No source named: the active layer, over its own pixels (here the whole canvas).
    let r = s.execute("ai.generate", json!({"prompt": "as a watercolour", "mode": "img2img", "seed": 3})).unwrap();
    assert_eq!((r["mode"].as_str(), r["bounds"].clone(), r["seed"].as_u64()), (Some("img2img"), json!([0, 0, 64, 48]), Some(3)));
    assert_eq!(layer_count(&s), before + 1);
    {
        let d = s.active().unwrap();
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert_eq!(layer.name, "[z_image_turbo] as a watercolour");
        assert!(layer.mask.is_none());
        // Stretched back onto the area it came from, whatever shape the model answered in.
        assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(0, 0, 64, 48));
        assert_eq!(d.history.entries().last().map(String::as_str), Some(IMG2IMG_LABEL));
    }
    // The model got the picture and no mask, and is told to keep to it (strength 0.6 unasked).
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert_eq!((image.width(), image.height(), image.layout()), (1152, 864, ChannelLayout::Rgb));
    assert!(image.data().iter().all(|b| *b == 255));
    assert_eq!(fake.seen.lock().unwrap().iter().filter(|(u, _)| u.ends_with("/upload/image")).count(), 1);
    let queued = fake.queued().to_string();
    assert!(queued.contains("\"denoise\":0.6") && queued.contains("VAEEncode") && !queued.contains("SetLatentNoiseMask"), "{queued}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), before);
    // With a selection: that part of the layer, and the selection masks the result.
    s.execute("select.rect", json!({"x": 8, "y": 4, "width": 16, "height": 16})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "source": {"layer": background}, "strength": 0.3})).unwrap();
    assert_eq!(r["bounds"], json!([8, 4, 16, 16]));
    let d = s.active().unwrap();
    assert!(d.doc.layer(d.active_layer.unwrap()).unwrap().mask.is_some());
    assert!(fake.seen.lock().unwrap().iter().any(|(u, b)| u.ends_with("/prompt") && String::from_utf8_lossy(b).contains("\"denoise\":0.3")));
}

#[test]
fn image_to_image_takes_another_open_document_fitted_to_the_area() {
    let fake = FakeComfy::new(32, 32, [0, 0, 255, 255]);
    let mut s = session(&fake);
    let first = s.active().unwrap().doc.id.0;
    // A second, taller document becomes the active one; the first (64 x 48, white) is the source.
    s.execute("file.new", json!({"width": 30, "height": 60, "background": "transparent"})).unwrap();
    assert_ne!(s.active().unwrap().doc.id.0, first);
    let plan = img2img_plan(&s, Some(SourceRef::Document(first)), None).unwrap();
    // Scaled to cover the 30 x 60 canvas and cropped to its shape: the middle 24 x 48 of the source.
    assert_eq!((plan.area, plan.source_rect), (Rect::new(0, 0, 30, 60), Rect::new(20, 0, 44, 48)));
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "source": {"document": first}})).unwrap();
    assert_eq!(r["bounds"], json!([0, 0, 30, 60]));
    let d = s.active().unwrap();
    assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().surface().unwrap().content_bounds(), Rect::new(0, 0, 30, 60));
    // The source document's image (white), in the area's shape; the source itself is untouched.
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert!(image.height() == 2 * image.width() && image.data().iter().all(|b| *b == 255), "{}x{}", image.width(), image.height());
    assert_eq!(s.documents().iter().find(|o| o.doc.id.0 == first).unwrap().doc.layers.len(), 1);
    // The active document has only an empty layer: no layer to start from, the other document is
    // what is offered, and it is the default.
    let listed = s.execute("ai.sources", json!({})).unwrap();
    assert_eq!(listed["sources"].as_array().unwrap().len(), 2, "{listed}");
    assert_eq!(listed["sources"][1]["source"], json!({"document": first}));
    assert!(listed["sources"][1]["name"].as_str().unwrap().starts_with("Document: "));
    // (The generated layer is now the first source, and the default.)
    assert_eq!(listed["sources"][0]["default"], true);
}

#[test]
fn image_to_image_says_what_it_needs() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = Session::new();
    s.ai.transport = Some(fake.clone());
    let models = [Model { id: "z".into(), ..Family::ZImage.preset() }, Model { id: "fill".into(), ..Family::Flux1Fill.preset() }];
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": serde_json::to_value(&models).unwrap()})).unwrap();
    assert!(img2img_sources(&s).is_empty() && default_img2img_source(&s).is_none());
    assert_eq!(s.execute("ai.sources", json!({})).unwrap(), json!({"sources": []}));
    s.execute("file.new", json!({"width": 40, "height": 40, "background": "transparent"})).unwrap();
    let fails = |s: &mut Session, p: Value| s.execute("ai.generate", p).unwrap_err().to_string();
    // Nothing with pixels, and no other document.
    let e = fails(&mut s, json!({"prompt": "x", "mode": "img2img"}));
    assert!(e.contains("needs a picture to start from"), "{e}");
    let empty = s.active().unwrap().active_layer.unwrap().0;
    let e = fails(&mut s, json!({"prompt": "x", "mode": "img2img", "source": {"layer": empty}}));
    assert!(e.contains("is empty"), "{e}");
    // Sources that are not there, or not sources.
    for (source, want) in [
        (json!({"layer": 999_999}), "is not in this document"),
        (json!({"document": 999_999}), "is not open"),
        (json!({"layer": "top"}), "`source` must be"),
        (json!({"file": "a.png"}), "`source` must be"),
        (json!("layer"), "`source` must be"),
        (json!(null), "`source` must be"),
        (json!({"layer": -1}), "`source` must be"),
    ] {
        let e = fails(&mut s, json!({"prompt": "x", "mode": "img2img", "source": source}));
        assert!(e.contains(want), "{source}: {e}");
    }
    // A layer with pixels, but the selection is off them.
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.rect", json!({"x": 20, "y": 20, "width": 10, "height": 10})).unwrap();
    let e = fails(&mut s, json!({"prompt": "x", "mode": "img2img"}));
    assert!(e.contains("is empty inside the selection"), "{e}");
    // Without the selection: the layer's own pixels are the area.
    s.execute("select.deselect", json!({})).unwrap();
    assert_eq!(img2img_plan(&s, None, None).unwrap().area, Rect::new(0, 0, 10, 10));
    assert_eq!(s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img"})).unwrap()["bounds"], json!([0, 0, 10, 10]));
    // A rect names the area; one off the canvas or off the pixels is refused.
    assert!(s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "source": {"layer": empty}, "rect": [2, 2, 6, 6]})).is_ok());
    for rect in [json!([500, 500, 10, 10]), json!([30, 30, 5, 5]), json!([0, 0, 0, 0]), json!("all")] {
        assert!(s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "source": {"layer": empty}, "rect": rect})).is_err(), "{rect}");
    }
    // A model made for inpainting can't, and strength is a fraction.
    assert!(fails(&mut s, json!({"prompt": "x", "mode": "img2img", "model": "fill"})).contains("inpainting model"));
    assert!(s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "strength": 7})).is_err());
}

#[test]
fn the_source_is_cropped_to_the_shape_of_the_area() {
    let src = Rect::new(0, 0, 400, 200);
    assert_eq!(cover_crop(src, 100, 100), Rect::new(100, 0, 300, 200));
    assert_eq!(cover_crop(src, 400, 100), Rect::new(0, 50, 400, 150));
    assert_eq!(cover_crop(src, 2, 1), src);
    assert_eq!(cover_crop(Rect::new(10, 20, 110, 220), 3, 1), Rect::new(10, 103, 110, 136));
    for (w, h) in [(0, 0), (1, u32::MAX), (u32::MAX, 1), (u32::MAX, u32::MAX)] {
        let c = cover_crop(src, w, h);
        assert!(!c.is_empty() && c.intersect(&src) == c, "{w}x{h}: {c:?}");
    }
    assert!(cover_crop(Rect::EMPTY, 4, 3).is_empty());
}

#[test]
fn the_inpaint_mask_is_painted_and_becomes_the_selection_and_the_layers_mask() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let before = layer_count(&s);
    assert!(!mask_painting(&s) && !inpaint_marked(&s));
    // Nothing selected: the mask starts empty, tinted where it will be painted.
    assert_eq!(s.execute("ai.mask", json!({"paint": true})).unwrap(), json!({"painting": true, "empty": true}));
    assert!(mask_painting(&s) && !inpaint_marked(&s));
    assert!(inpaint_blocked(&s).unwrap_err().contains("needs a selection"));
    {
        let q = s.active().unwrap().doc.quick_mask.as_ref().unwrap();
        assert_eq!((q.indicates, q.surface.content_bounds()), (ColorIndicates::SelectedAreas, Rect::EMPTY));
    }
    // A stroke of white paints it in; black paints it out again.
    let stroke_of = |s: &mut Session, points: Value, color: &str, size: u32| {
        s.execute("paint.stroke", json!({"points": points, "size": size, "hardness": 1.0, "color": color, "target": "quickMask"})).unwrap();
    };
    let stroke = |s: &mut Session, points: Value, color: &str| stroke_of(s, points, color, 12);
    stroke(&mut s, json!([[20, 20], [40, 20]]), "#ffffff");
    let painted = inpaint_blocked(&s).unwrap();
    assert!(painted.x0 <= 16 && painted.x1 >= 44 && painted.y0 <= 16 && painted.y1 >= 24 && painted.height() < 20, "{painted:?}");
    assert!(inpaint_marked(&s));
    // (With a wider brush: the same one would leave the soft rim of the stroke behind.)
    stroke_of(&mut s, json!([[20, 20], [40, 20]]), "#000000", 24);
    assert!(!inpaint_marked(&s), "painted out again");
    stroke(&mut s, json!([[20, 20], [40, 20]]), "#ffffff");
    // Generating leaves the mode: the mask is the selection, and the new layer's mask.
    let r = s.execute("ai.generate", json!({"prompt": "a boat", "mode": "inpaint", "context": 0})).unwrap();
    assert_eq!(r["mode"], "inpaint");
    assert_eq!(layer_count(&s), before + 1);
    {
        let d = s.active().unwrap();
        assert!(d.doc.quick_mask.is_none() && !mask_painting(&s));
        let selected = d.doc.selection.as_ref().unwrap().content_bounds();
        assert_eq!(selected, painted);
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert_eq!(layer.mask.as_ref().unwrap().surface.content_bounds(), painted);
        assert_eq!(r["bounds"], json!([painted.x0, painted.y0, painted.width(), painted.height()]));
    }
    // Entering again starts from that selection; leaving gives it back; clearing drops it.
    assert_eq!(s.execute("ai.mask", json!({"paint": true})).unwrap(), json!({"painting": true, "empty": false}));
    assert_eq!(inpaint_blocked(&s).ok(), Some(painted));
    assert_eq!(s.execute("ai.mask", json!({"paint": false})).unwrap(), json!({"painting": false, "empty": false}));
    assert_eq!(s.active().unwrap().doc.selection.as_ref().unwrap().content_bounds(), painted);
    assert_eq!(s.execute("ai.mask", json!({"clear": true})).unwrap(), json!({"painting": false, "empty": true}));
    assert!(s.active().unwrap().doc.selection.is_none());
    // Cleared while painting: the mode stays, the mask is empty; an empty mask selects nothing.
    s.execute("ai.mask", json!({"paint": true})).unwrap();
    stroke(&mut s, json!([[10, 10]]), "#ffffff");
    assert_eq!(s.execute("ai.mask", json!({"clear": true})).unwrap(), json!({"painting": true, "empty": true}));
    let e = s.execute("ai.generate", json!({"prompt": "x", "mode": "inpaint"})).unwrap_err().to_string();
    assert!(e.contains("needs a selection") && mask_painting(&s), "{e}");
    s.execute("ai.mask", json!({"paint": false})).unwrap();
    assert!(s.active().unwrap().doc.selection.is_none() && s.active().unwrap().doc.quick_mask.is_none());
    // Quick Mask entered the usual way (its tint over the masked part) is not this mode until asked.
    s.execute("select.editInQuickMaskMode", json!({"on": true})).unwrap();
    assert!(!mask_painting(&s));
    assert_eq!(s.execute("ai.mask", json!({"paint": true})).unwrap()["painting"], true);
    s.execute("ai.mask", json!({"paint": false})).unwrap();
    // Params that are not what they say, and no document.
    for p in [json!({}), json!({"paint": "yes"}), json!({"clear": 1}), json!({"clear": false}), Value::Null, json!([true])] {
        assert!(s.execute("ai.mask", p.clone()).is_err(), "{p}");
    }
    assert!(Session::new().execute("ai.mask", json!({"paint": true})).is_err());
}

#[test]
fn a_generated_layer_keeps_what_it_was_made_from() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = session(&fake);
    let background = s.active().unwrap().active_layer.unwrap().0;
    fn made(s: &Session) -> Option<Generation> {
        let d = s.active().unwrap();
        d.doc.layer(d.active_layer.unwrap()).unwrap().generation.clone()
    }
    assert_eq!(made(&s), None, "a layer that was not generated has none");
    let name = Family::ZImage.preset().name;
    // Text to image: the prompt as it was sent, the model, the seed.
    s.execute("ai.generate", json!({"prompt": "  a green field ", "seed": 5})).unwrap();
    let txt = Generation {
        mode: GenerationMode::Txt2img,
        model: "z".into(),
        model_name: name.clone(),
        prompt: "a green field".into(),
        strength: 1.0,
        context: DEFAULT_CONTEXT,
        alpha: false,
        seed: 5,
        source: None,
        references: Vec::new(),
        quality: GenerationQuality::Standard,
    };
    assert_eq!(made(&s), Some(txt.clone()));
    // The layer tree reports it, named as `ai.generate` names its params.
    let d = s.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    let info = crate::inspect::layer(layer);
    assert_eq!(
        info["generation"],
        json!({"mode": "txt2img", "model": "z", "modelName": name, "prompt": "a green field", "strength": 1.0, "context": DEFAULT_CONTEXT, "alpha": false, "seed": 5, "source": null, "references": [], "quality": "standard"})
    );
    assert!(crate::inspect::layer(d.doc.layer(LayerId(background)).unwrap()).get("generation").is_none());
    // A copy of the layer is as much that generation as the layer is.
    assert_eq!(layer.duplicate().generation, Some(txt.clone()));
    // Image to image: the picture it was redrawn from and how closely.
    s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "source": {"layer": background}, "strength": 0.3, "seed": 6})).unwrap();
    let img = Generation {
        mode: GenerationMode::Img2img,
        prompt: "x".into(),
        strength: 0.3,
        seed: 6,
        source: Some(GenerationSource::Layer(background)),
        ..txt.clone()
    };
    assert_eq!(made(&s), Some(img));
    // Unnamed, the source is the one that was used: the active layer.
    let active = s.active().unwrap().active_layer.unwrap().0;
    s.execute("ai.generate", json!({"prompt": "x", "mode": "img2img", "seed": 6})).unwrap();
    assert_eq!(made(&s).unwrap().source, Some(GenerationSource::Layer(active)));
    // Inpainting: strength and context.
    s.execute("select.rect", json!({"x": 24, "y": 16, "width": 16, "height": 16})).unwrap();
    s.execute("ai.generate", json!({"prompt": "a pond", "mode": "inpaint", "strength": 0.75, "context": 0.5, "seed": 9})).unwrap();
    let inpaint = Generation { mode: GenerationMode::Inpaint, prompt: "a pond".into(), strength: 0.75, context: 0.5, seed: 9, ..txt };
    assert_eq!(made(&s), Some(inpaint));
    // A seed the engine drew is the one kept.
    let r = s.execute("ai.generate", json!({"prompt": "y"})).unwrap();
    assert_eq!(Some(made(&s).unwrap().seed), r["seed"].as_u64());
    // Undo takes the layer, and its generation with it.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(made(&s).unwrap().prompt, "a pond");
}

/// [`session`] with Ming Image as its one model.
fn ming_session(fake: &Arc<FakeComfy>) -> Session {
    let mut s = session(fake);
    let model = serde_json::to_value(Model { id: "ming".into(), ..Family::MingImage.preset() }).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
    s
}

/// Ming Image is the local model that renders transparency: it is asked for it in its own words,
/// at its reference size, and the alpha it answers with reaches the layer. Given a picture it is
/// not asked (it answers opaque), and works at half that size.
#[test]
fn ming_image_generates_transparency_and_edits_at_half_its_size() {
    // A server that answers half transparent.
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 128]);
    let mut s = ming_session(&fake);
    let listed = s.execute("ai.models", json!({})).unwrap();
    let m = &listed["models"][0];
    assert_eq!(
        (&m["alpha"], &m["generate"], &m["edit"], &m["inpaint"], &m["references"]),
        (&json!(true), &json!(true), &json!(true), &json!(false), &json!(3))
    );
    let r = s.execute("ai.generate", json!({"prompt": "a logo", "alpha": true, "seed": 3})).unwrap();
    // The 64 x 48 canvas at two megapixels.
    assert_eq!((r["mode"].as_str(), &r["renderSize"]), (Some("txt2img"), &json!([1632, 1232])));
    let queued = fake.queued().to_string();
    assert!(
        queued.contains("RGBA, 4-channel, transparent background, a logo") && queued.contains("BasicGuider") && !queued.contains("SplitImageWithAlpha"),
        "{queued}"
    );
    // Green at half strength over the white background: the layer kept the alpha.
    let p = px(&mut s, 30, 20);
    assert!((0.2..0.8).contains(&p[0]) && p[1] > 0.95, "{p:?}");
    // Without the Alpha Channel box the prompt is the user's, and the workflow drops the alpha.
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = ming_session(&fake);
    s.execute("ai.generate", json!({"prompt": "a logo", "seed": 3})).unwrap();
    let queued = fake.queued().to_string();
    assert!(queued.contains("\"text\":\"a logo\"") && queued.contains("SplitImageWithAlpha"), "{queued}");

    // An edit: the picture at two megapixels, the edit template's encoder and sampler, no phrase.
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = ming_session(&fake);
    let r = s.execute("ai.generate", json!({"prompt": "make it green", "mode": "imgEdit", "alpha": true, "seed": 3})).unwrap();
    assert_eq!((r["mode"].as_str(), &r["bounds"], &r["renderSize"]), (Some("imgEdit"), &json!([0, 0, 64, 48]), &json!([1632, 1232])));
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert_eq!((image.width(), image.height()), (1632, 1232));
    let queued = fake.queued().to_string();
    for want in ["TextEncodeMingImageEdit", "SamplerLCM", "SplitImageWithAlpha", "make it green"] {
        assert!(queued.contains(want), "missing {want}: {queued}");
    }
    assert!(!queued.contains("RGBA, 4-channel") && !queued.contains("KSamplerSelect"), "{queued}");
    let p = px(&mut s, 30, 20);
    assert!(p[0] < 0.05 && p[1] > 0.95, "{p:?}");
}

/// [`session`] with the models an edit is tried with: Qwen Image 2.1 (the default), Qwen Image
/// Edit, and Z-Image, which can't edit.
fn edit_session(fake: &Arc<FakeComfy>) -> Session {
    let mut s = session(fake);
    let models = [
        Model { id: "q21".into(), ..Family::QwenImage21.preset() },
        Model { id: "edit".into(), ..Family::QwenEdit.preset() },
        Model { id: "z".into(), ..Family::ZImage.preset() },
    ];
    s.execute("prefs.set", json!({"path": "integrations.ai.models", "value": serde_json::to_value(&models).unwrap()})).unwrap();
    s
}

#[test]
fn an_edit_with_a_selection_is_cropped_edited_and_composited() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    let before = layer_count(&s);
    let background = s.active().unwrap().active_layer.unwrap().0;
    s.execute("select.rect", json!({"x": 16, "y": 12, "width": 16, "height": 16})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "make it green", "mode": "imgEdit", "seed": 3})).unwrap();
    // The model is sent the selection with a quarter of its edge around it, not the whole layer.
    assert_eq!((r["mode"].as_str(), r["model"].as_str(), &r["bounds"]), (Some("imgEdit"), Some("q21"), &json!([12, 8, 24, 24])));
    // Qwen Image 2.1 renders in multiples of 32, in the area's shape.
    assert_eq!(r["renderSize"], json!([992, 992]));
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert_eq!((image.width(), image.height()), (992, 992));
    let uploads = fake.seen.lock().unwrap().iter().filter(|(u, _)| u.ends_with("/upload/image")).count();
    assert_eq!(uploads, 2, "the picture and the selection as a mask");
    let queued = fake.queued().to_string();
    assert!(queued.contains("TextEncodeQwenImage21") && queued.contains("make it green") && queued.contains("SetLatentNoiseMask"), "{queued}");
    // The model paints only in the selection (transparent), fading in from its edge, so what it
    // paints is composed inside it and not cut by the layer mask.
    let mask = fake.uploaded("photocraft_inpaint_mask.png");
    assert_eq!((mask.width(), mask.height()), (992, 992));
    let alpha = |x: usize, y: usize| mask.data()[(y * 992 + x) * 4 + 3];
    // The selection is x, y = 4..20 of the 24 px area: 165..827 rendered.
    assert_eq!((alpha(496, 496), alpha(100, 496), alpha(900, 900)), (0, 255, 255));
    assert!((1..255).contains(&alpha(175, 496)), "{}", alpha(175, 496));
    // A new layer over that area, masked by the selection: green inside it, untouched outside.
    assert_eq!(layer_count(&s), before + 1);
    {
        let d = s.active().unwrap();
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(12, 8, 36, 32));
        assert_eq!(layer.mask.as_ref().unwrap().surface.content_bounds(), Rect::new(16, 12, 32, 28));
        let g = layer.generation.clone().unwrap();
        assert_eq!((g.mode, g.source, g.quality, g.seed), (GenerationMode::ImgEdit, Some(GenerationSource::Layer(background)), GenerationQuality::Standard, 3));
        assert_eq!(d.doc.layer(LayerId(background)).unwrap().surface().unwrap().content_bounds(), Rect::new(0, 0, 64, 48), "the source is untouched");
    }
    let inside = px(&mut s, 20, 20);
    assert!(inside[0] < 0.05 && inside[1] > 0.95, "{inside:?}");
    // In the context, outside the selection: the answer is there, and hidden.
    for (x, y) in [(13, 9), (40, 40)] {
        let outside = px(&mut s, x, y);
        assert!(outside.iter().take(3).all(|c| *c > 0.95), "({x},{y}) {outside:?}");
    }
    // One undo step removes the layer.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), before);
    assert_eq!(edit_plan(&s, None, DEFAULT_CONTEXT, None).unwrap().area, Rect::new(12, 8, 36, 32));
    // No context: the selection's bounds. A rect names the area outright.
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "source": {"layer": background}, "context": 0})).unwrap();
    assert_eq!(r["bounds"], json!([16, 12, 16, 16]));
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "source": {"layer": background}, "rect": [0, 0, 32, 16]})).unwrap();
    assert_eq!((&r["bounds"], &r["renderSize"]), (&json!([0, 0, 32, 16]), &json!([1408, 704])));
}

#[test]
fn an_edit_of_all_the_picture_is_not_masked() {
    // Everything selected: the whole picture is the model's to change, up to the canvas's edge.
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    s.execute("select.all", json!({})).unwrap();
    s.execute("ai.generate", json!({"prompt": "make it green", "mode": "imgEdit", "seed": 3})).unwrap();
    let uploads = fake.seen.lock().unwrap().iter().filter(|(u, _)| u.ends_with("/upload/image")).count();
    assert_eq!(uploads, 1, "the picture, and no mask");
    assert!(!fake.queued().to_string().contains("SetLatentNoiseMask"));
    // A selection at the canvas's edge is whole there: it fades in from its inner edges only.
    let canvas = Rect::new(0, 0, 64, 48);
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 32, "height": 48})).unwrap();
    let selection = s.active().unwrap().doc.selection.clone().unwrap();
    let mask = photocraft_codecs::decode(&edit_mask(&selection, canvas, canvas, (640, 480)).unwrap()).unwrap();
    let alpha = |x: usize, y: usize| mask.data()[(y * 640 + x) * 4 + 3];
    assert_eq!((alpha(0, 240), alpha(100, 0), alpha(100, 479), alpha(500, 240)), (0, 0, 0, 255));
    assert!(alpha(318, 240) > 0, "faded at its inner edge");
}

#[test]
fn a_painted_mask_scopes_an_edit_like_the_selection_it_becomes() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    let before = layer_count(&s);
    let background = s.active().unwrap().active_layer.unwrap().0;
    // The area is painted instead of selected (`ai.mask`). An empty mask scopes nothing.
    s.execute("ai.mask", json!({"paint": true})).unwrap();
    assert_eq!(edit_plan(&s, None, 0.0, None).unwrap().area, Rect::new(0, 0, 64, 48));
    s.execute("paint.stroke", json!({"points": [[20, 20], [40, 20]], "size": 12, "hardness": 1.0, "color": "#ffffff", "target": "quickMask"})).unwrap();
    // While it is painted, the plan already counts it as the selection it will become.
    let painted = inpaint_blocked(&s).unwrap();
    assert_eq!(edit_plan(&s, None, 0.0, None).unwrap().area, painted);
    // Generating leaves the mode: the mask is the selection, the edit's area and the layer's mask.
    let r = s.execute("ai.generate", json!({"prompt": "make it green", "mode": "imgEdit", "context": 0})).unwrap();
    assert_eq!((r["mode"].as_str(), &r["bounds"]), (Some("imgEdit"), &json!([painted.x0, painted.y0, painted.width(), painted.height()])));
    assert_eq!(layer_count(&s), before + 1);
    {
        let d = s.active().unwrap();
        assert!(d.doc.quick_mask.is_none() && !mask_painting(&s));
        assert_eq!(d.doc.selection.as_ref().unwrap().content_bounds(), painted);
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert_eq!(layer.mask.as_ref().unwrap().surface.content_bounds(), painted);
    }
    // An edit that can't run keeps the mask being painted.
    s.execute("ai.mask", json!({"paint": true})).unwrap();
    for p in [json!({"source": {"layer": 999_999}}), json!({"rect": [900, 900, 8, 8]}), json!({"rect": "all"})] {
        let mut params = json!({"prompt": "x", "mode": "imgEdit"});
        params.as_object_mut().unwrap().extend(p.as_object().unwrap().clone());
        assert!(s.execute("ai.generate", params).is_err() && mask_painting(&s), "{p}");
    }
    // A mask emptied again: no selection, and the whole picture is edited.
    s.execute("ai.mask", json!({"clear": true})).unwrap();
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "source": {"layer": background}})).unwrap();
    assert_eq!(r["bounds"], json!([0, 0, 64, 48]));
    let d = s.active().unwrap();
    assert!(d.doc.selection.is_none() && d.doc.quick_mask.is_none());
}

#[test]
fn an_edit_without_a_selection_takes_the_whole_picture() {
    let fake = FakeComfy::new(32, 32, [0, 0, 255, 255]);
    let mut s = edit_session(&fake);
    let doc = s.active().unwrap().doc.id.0;
    // The layer's own pixels (here the canvas), no mask; the answer is stretched back onto them.
    let r = s.execute("ai.generate", json!({"prompt": "at night", "mode": "imgEdit"})).unwrap();
    assert_eq!(r["bounds"], json!([0, 0, 64, 48]));
    {
        let d = s.active().unwrap();
        let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
        assert!(layer.mask.is_none());
        assert_eq!(layer.surface().unwrap().content_bounds(), Rect::new(0, 0, 64, 48));
    }
    let image = fake.uploaded("photocraft_inpaint_image.png");
    assert!(image.width().is_multiple_of(32) && image.height().is_multiple_of(32) && image.width() > image.height(), "{}x{}", image.width(), image.height());
    for (x, y) in [(0, 0), (63, 47)] {
        let p = px(&mut s, x, y);
        assert!(p[2] > 0.95 && p[0] < 0.05, "({x},{y}) {p:?}");
    }
    // The document as it shows is a source too, taken where it is: first on the list for an edit,
    // while the active layer stays the default.
    let listed = s.execute("ai.sources", json!({"mode": "imgEdit"})).unwrap();
    assert_eq!((&listed["sources"][0]["source"], listed["sources"][0]["name"].as_str()), (&json!({"document": doc}), Some("This document (all layers)")));
    assert_eq!(listed["sources"].as_array().unwrap().iter().filter(|x| x["default"] == true).count(), 1);
    assert_eq!(s.execute("ai.sources", json!({})).unwrap()["sources"].as_array().unwrap().len(), listed["sources"].as_array().unwrap().len() - 1);
    assert!(s.execute("ai.sources", json!({"mode": "txt2img"})).is_err());
    s.execute("select.rect", json!({"x": 40, "y": 20, "width": 8, "height": 8})).unwrap();
    let plan = edit_plan(&s, Some(SourceRef::Document(doc)), 0.5, None).unwrap();
    assert_eq!((plan.area, plan.source_rect), (Rect::new(36, 16, 52, 32), Rect::new(36, 16, 52, 32)));
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "source": {"document": doc}, "context": 0.5})).unwrap();
    assert_eq!(r["bounds"], json!([36, 16, 16, 16]));
    {
        let d = s.active().unwrap();
        assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().generation.clone().unwrap().source, Some(GenerationSource::Document(doc)));
    }
    // Another document is fitted to the selection as it is: there is no "around it" to show.
    s.execute("file.new", json!({"width": 30, "height": 60})).unwrap();
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 10, "height": 10})).unwrap();
    let plan = edit_plan(&s, Some(SourceRef::Document(doc)), 0.5, None).unwrap();
    assert_eq!((plan.area, plan.source_rect), (Rect::new(10, 10, 20, 20), Rect::new(8, 0, 56, 48)));
}

#[test]
fn an_edit_is_given_its_references_and_its_quality() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    let background = s.active().unwrap().active_layer.unwrap().0;
    // A small red layer to refer to, and a second, large document.
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 20, "height": 10})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    let red = s.active().unwrap().active_layer.unwrap().0;
    s.execute("file.new", json!({"width": 3000, "height": 1500})).unwrap();
    let big = s.active().unwrap().doc.id.0;
    assert!(s.set_active(0));
    let params = json!({
        "prompt": "put the red patch of image 2 on it", "mode": "imgEdit", "source": {"layer": background},
        "references": [{"layer": red}, {"document": big}], "quality": "draft", "seed": 1,
    });
    s.execute("ai.generate", params).unwrap();
    // Each reference is uploaded after the picture, under its own name: a small one brought up
    // to a quarter of a megapixel, a large one down to one, both in their own shape.
    let one = fake.uploaded("photocraft_edit_reference_1.png");
    assert_eq!((one.width(), one.height()), (724, 362));
    let two = fake.uploaded("photocraft_edit_reference_2.png");
    assert_eq!((two.width(), two.height()), (1448, 724));
    let queued = fake.queued();
    let text = queued.to_string();
    assert!(text.contains("images.image_3") && !text.contains("images.image_4"), "{text}");
    // A draft runs a quarter of Qwen Image 2.1's 25 steps.
    let sampler = queued["prompt"].as_object().unwrap().values().find(|n| n["class_type"] == "KSampler").unwrap().clone();
    assert_eq!(sampler["inputs"]["steps"], 6);
    let d = s.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    let g = layer.generation.clone().unwrap();
    assert_eq!(g.references, vec![GenerationSource::Layer(red).into(), GenerationSource::Document(big).into()]);
    assert_eq!(g.quality, GenerationQuality::Draft);
    assert_eq!(crate::inspect::layer(layer)["generation"]["references"], json!([{"layer": red}, {"document": big}]));
    assert_eq!(crate::inspect::layer(layer)["generation"]["quality"], "draft");
}

#[test]
fn references_named_in_the_prompt_are_the_ones_sent() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    let background = s.active().unwrap().active_layer.unwrap().0;
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 20, "height": 10})).unwrap();
    s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    let red = s.active().unwrap().active_layer.unwrap().0;
    // Another document, and a layer of it that has pixels (a tall blue one).
    s.execute("file.new", json!({"width": 100, "height": 50})).unwrap();
    let other = s.active().unwrap().doc.id.0;
    s.execute("layer.new.layer", json!({"name": "Luna portrait.png"})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 40})).unwrap();
    s.execute("edit.fill", json!({"color": "#0000ff"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    let luna = s.active().unwrap().active_layer.unwrap().0;
    assert!(s.set_active(0));
    // It is offered to refer to, after its document, and named after its layer.
    let offered = reference_sources(&s);
    let at = offered.iter().position(|(r, _)| *r == SourceRef::Document(other)).unwrap();
    let theirs = SourceRef::DocumentLayer { document: other, layer: luna };
    assert_eq!(offered.get(at + 1).map(|(r, _)| *r), Some(theirs));
    assert!(offered[at + 1].1.ends_with("(Untitled-2)") || offered[at + 1].1.contains("Luna portrait.png"), "{:?}", offered[at + 1]);
    assert!(!edit_sources(&s).iter().any(|(r, _)| *r == theirs), "it is a reference, not a picture to change");
    assert_eq!(reference_name(&s, theirs), "Luna_portrait");
    assert_eq!(reference_name(&s, SourceRef::Layer(999)), "");
    let listed = s.execute("ai.sources", json!({"mode": "reference"})).unwrap();
    assert!(listed["sources"].as_array().unwrap().iter().any(|e| e["source"] == json!({"document": other, "layer": luna})));
    // Mentions decide: only Luna goes, as image 2, and the model is told so; the prompt the layer
    // keeps is the one typed, with every reference and its name.
    let references = json!([{"layer": red, "name": "Patch"}, {"document": other, "layer": luna, "name": "Luna"}, {"layer": background}]);
    let r = s
        .execute("ai.generate", json!({"prompt": "@luna waves", "mode": "imgEdit", "source": {"layer": background}, "references": references, "seed": 1}))
        .unwrap();
    assert_eq!(r["modelPrompt"], "Luna (image 2) waves");
    let one = fake.uploaded("photocraft_edit_reference_1.png");
    assert!(one.height() > one.width(), "the tall layer of the other document, cropped to its pixels");
    let text = fake.queued().to_string();
    assert!(text.contains("Luna (image 2) waves") && text.contains("images.image_2") && !text.contains("images.image_3"), "{text}");
    let d = s.active().unwrap();
    let g = d.doc.layer(d.active_layer.unwrap()).unwrap().generation.clone().unwrap();
    assert_eq!(g.prompt, "@luna waves");
    assert_eq!(g.references.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Patch", "Luna", ""]);
    assert_eq!(g.references[1].source, GenerationSource::DocumentLayer { document: other, layer: luna });
    // No mention: all three go, the prompt as typed.
    let r = s.execute("ai.generate", json!({"prompt": "use image 3", "mode": "imgEdit", "references": references, "seed": 1})).unwrap();
    assert_eq!(r["modelPrompt"], "use image 3");
    let last = fake.seen.lock().unwrap().iter().rev().find(|(u, _)| u.ends_with("/prompt")).map(|(_, b)| String::from_utf8_lossy(b).into_owned()).unwrap();
    assert!(last.contains("images.image_4"), "{last}");
    // Names are cleaned, and two alike are refused.
    let twice = json!([{"layer": red, "name": "Luna"}, {"layer": background, "name": "LUNA"}]);
    let e = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "references": twice})).unwrap_err().to_string();
    assert!(e.contains("two references are named"), "{e}");
    let odd = json!([{"layer": red, "name": "  my hat!! "}]);
    let r = s.execute("ai.generate", json!({"prompt": "give him @my_hat", "mode": "imgEdit", "references": odd, "seed": 1})).unwrap();
    assert_eq!(r["modelPrompt"], "give him my hat (image 2)");
    // A layer of another document that is gone, or isn't one, is an error that says so.
    for (bad, want) in [(json!({"document": other, "layer": 999_999}), "is not in"), (json!({"document": 999_999, "layer": luna}), "is not open")] {
        let e = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "references": [bad]})).unwrap_err().to_string();
        assert!(e.contains(want), "{e}");
    }
    // It can be the picture to change too: fitted to the area like its document.
    let plan = edit_plan(&s, Some(theirs), 0.0, None).unwrap();
    assert_eq!(plan.area, s.active().unwrap().doc.bounds());
    // Of the active document, it is that document's layer.
    let mine = SourceRef::DocumentLayer { document: s.active().unwrap().doc.id.0, layer: red };
    assert_eq!(mine.settled(&s), SourceRef::Layer(red));
    // And the layer keeps it so, which is what a saved file can keep.
    let r = s
        .execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "references": [{"document": s.active().unwrap().doc.id.0, "layer": red}], "seed": 1}))
        .unwrap();
    let d = s.active().unwrap();
    let g = d.doc.layer(photocraft_doc::LayerId(r["layer"].as_u64().unwrap())).unwrap().generation.clone().unwrap();
    assert_eq!(g.references, [GenerationSource::Layer(red).into()]);
    assert_eq!(serde_json::to_value(theirs).unwrap(), json!({"document": other, "layer": luna}));
    assert_eq!(serde_json::from_value::<SourceRef>(json!({"layer": 4})).unwrap(), SourceRef::Layer(4));
    assert!(serde_json::from_value::<SourceRef>(json!({"name": "x"})).is_err());
}

#[test]
fn an_edit_without_references_sends_the_ones_the_document_keeps() {
    let fake = FakeComfy::new(32, 32, [0, 255, 0, 255]);
    let mut s = edit_session(&fake);
    let background = s.active().unwrap().active_layer.unwrap().0;
    let here = s.active().unwrap().doc.id.0;
    // A picture from another document, copied in; that document then closes.
    s.execute("file.new", json!({"width": 30, "height": 60})).unwrap();
    let there = s.active().unwrap().doc.id.0;
    assert!(s.set_active(0));
    s.execute("ai.setReferences", json!({"references": [{"document": there, "name": "Max"}, {"layer": background, "name": "Sky"}]})).unwrap();
    let last = s.documents().len() - 1;
    assert!(s.close(last).is_some());
    assert_eq!(s.active().unwrap().doc.id.0, here);
    let r = s.execute("ai.generate", json!({"prompt": "@Max under @Sky", "mode": "imgEdit", "seed": 1})).unwrap();
    assert_eq!(r["modelPrompt"], "Max (image 2) under Sky (image 3)");
    // The copy goes as it was kept (tall, like its document), the layer is read now.
    let one = fake.uploaded("photocraft_edit_reference_1.png");
    assert!(one.height() > one.width(), "{}x{}", one.width(), one.height());
    fake.uploaded("photocraft_edit_reference_2.png");
    // The layer names the layer it could refer to by name; the copy it can't name.
    let d = s.active().unwrap();
    let g = d.doc.layer(d.active_layer.unwrap()).unwrap().generation.clone().unwrap();
    assert_eq!(g.references, [GenerationReference { source: GenerationSource::Layer(background), name: "Sky".into() }]);
    // References asked for replace the kept ones for that edit.
    let r = s.execute("ai.generate", json!({"prompt": "x", "mode": "imgEdit", "references": [], "seed": 1})).unwrap();
    assert_eq!(r["modelPrompt"], "x");
}

#[test]
fn an_edit_says_what_it_needs() {
    let fake = FakeComfy::new(8, 8, [1, 2, 3, 255]);
    let mut s = edit_session(&fake);
    let background = s.active().unwrap().active_layer.unwrap().0;
    let fails = |s: &mut Session, p: Value| s.execute("ai.generate", p).unwrap_err().to_string();
    let edit = |extra: Value| {
        let mut p = json!({"prompt": "x", "mode": "imgEdit"});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    // A model that generates but does not edit, and an edit model asked to do anything else.
    assert!(fails(&mut s, edit(json!({"model": "z"}))).contains("can't edit a picture"));
    for mode in ["txt2img", "img2img"] {
        let e = fails(&mut s, json!({"prompt": "x", "mode": mode, "model": "edit"}));
        assert!(e.contains("is an edit model"), "{mode}: {e}");
    }
    assert!(fails(&mut s, json!({"prompt": "x", "mode": "inpaint", "model": "edit"})).contains("not set up for inpainting"));
    let listed = s.execute("ai.models", json!({})).unwrap();
    let flags: Vec<(bool, bool, u64)> =
        listed["models"].as_array().unwrap().iter().map(|m| (m["edit"] == true, m["generate"] == true, m["references"].as_u64().unwrap())).collect();
    assert_eq!(flags, [(true, true, 3), (true, false, 2), (false, true, 3)]);
    // References that are not there, are not references, or are too many for the model.
    let four = json!([{"layer": background}, {"layer": background}, {"layer": background}, {"layer": background}]);
    for (references, want) in [
        (json!([{"layer": 999_999}]), "is not in this document"),
        (json!([{"document": 999_999}]), "is not open"),
        (json!({"layer": background}), "`references` must be"),
        (json!([{"file": "a.png"}]), "`references` must be"),
        (json!([7]), "`references` must be"),
        (json!(null), "`references` must be"),
        (four, "3 pictures at most"),
    ] {
        let e = fails(&mut s, edit(json!({"references": references})));
        assert!(e.contains(want), "{references}: {e}");
    }
    let three = json!([{"layer": background}, {"layer": background}, {"layer": background}]);
    assert!(fails(&mut s, edit(json!({"references": three, "model": "edit"}))).contains("2 reference pictures at most"));
    assert!(s.execute("ai.generate", edit(json!({"references": three, "source": {"layer": background}}))).is_ok());
    // Nothing was sent for any of the refusals above: one generation, one workflow.
    assert_eq!(fake.seen.lock().unwrap().iter().filter(|(u, _)| u.ends_with("/prompt")).count(), 1);
    // An empty layer is neither a source nor a reference.
    s.execute("layer.new.layer", json!({})).unwrap();
    let empty = s.active().unwrap().active_layer.unwrap().0;
    assert!(fails(&mut s, edit(json!({"source": {"layer": empty}}))).contains("is empty"));
    assert!(fails(&mut s, edit(json!({"source": {"layer": background}, "references": [{"layer": empty}]}))).contains("is empty"));
    for (key, value) in
        [("quality", json!("best")), ("quality", json!(3)), ("context", json!(99)), ("source", json!("layer")), ("rect", json!([500, 500, 4, 4]))]
    {
        let mut p = edit(json!({"source": {"layer": background}}));
        p[key] = value.clone();
        assert!(s.execute("ai.generate", p).is_err(), "{key}: {value}");
    }
    // With nothing to edit at all.
    let mut bare = Session::new();
    bare.ai.transport = Some(fake.clone());
    let model = serde_json::to_value(Model { id: "q21".into(), ..Family::QwenImage21.preset() }).unwrap();
    bare.execute("prefs.set", json!({"path": "integrations.ai.models", "value": [model]})).unwrap();
    bare.execute("file.new", json!({"width": 40, "height": 40, "background": "transparent"})).unwrap();
    assert!(edit_sources(&bare).is_empty());
    assert!(fails(&mut bare, edit(json!({}))).contains("Editing needs a picture"));
}

/// A ComfyUI this app started, which has stopped on its own.
struct Crashed;

impl LocalServer for Crashed {
    fn ensure(&self, _: &dyn Transport, _: &Settings, _: &Control<'_>) -> photocraft_aigen::Result<()> {
        Ok(())
    }
    fn stop(&self) -> bool {
        false
    }
    fn state(&self) -> ServerState {
        ServerState::NotStarted
    }
    fn find(&self, _: &dyn Transport, _: &str) -> photocraft_aigen::Result<photocraft_aigen::Launch> {
        Err(photocraft_aigen::Error::Config("no".into()))
    }
    fn stopped(&self) -> Option<String> {
        Some("exit code 0xC0000005".into())
    }
}

#[test]
fn a_render_that_loses_the_server_it_started_says_how_that_ended() {
    let lost = || photocraft_aigen::Error::Network("Lost the connection to ComfyUI at http://127.0.0.1:8190 while it was generating".into());
    let crashed: Arc<dyn LocalServer> = Arc::new(Crashed);
    let e = lost_server(Some(&crashed), lost()).to_string();
    assert!(e.contains("ComfyUI stopped while it was generating (exit code 0xC0000005)") && e.contains("ran out of memory"), "{e}");
    // A server started elsewhere, or a failure that isn't a lost connection, is reported as it is.
    assert!(lost_server(None, lost()).to_string().contains("Lost the connection"));
    let refused = lost_server(Some(&crashed), photocraft_aigen::Error::Server("bad workflow".into())).to_string();
    assert!(refused.contains("bad workflow") && !refused.contains("stopped"), "{refused}");
    assert!(matches!(lost_server(Some(&crashed), photocraft_aigen::Error::Cancelled), EngineError::Cancelled));
}
