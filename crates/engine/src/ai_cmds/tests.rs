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
        let model = serde_json::to_value(Model { id: "z".into(), megapixels: 0.0, ..Family::ZImage.preset() }).unwrap();
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
    for mode in ["img2img", "imgEdit"] {
        assert!(fails(&mut s, json!({"prompt": "x", "mode": mode})).contains("not available yet"));
    }
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
    fade_inwards(&mut soft, w, h, 8);
    let at = |x: usize, y: usize| soft[y * w + x];
    assert!(soft.iter().zip(&hard).all(|(s, m)| s <= m && (0.0..=1.0).contains(s)), "the fade only takes away");
    assert!(at(10, 15) < 0.2 && at(29, 15) < 0.2, "{} {}", at(10, 15), at(29, 15));
    assert!(at(20, 15) > 0.95, "{}", at(20, 15));
    assert!(at(12, 15) < at(14, 15) && at(14, 15) < at(18, 15));
    // The picture's own edge counts as an edge: there is nothing beyond it to join.
    assert!(at(20, 0) < 0.2 && at(20, 29) < 0.2);
    // No band, an empty picture or a buffer of the wrong size: left alone, not a panic.
    for (len, w, h, band) in [(w * h, w, h, 0), (0, 0, 0, 8), (7, w, h, 8), (w * h, w, h, 100_000)] {
        let mut m = vec![1.0; len];
        fade_inwards(&mut m, w, h, band);
        assert!(m.iter().all(|v| (0.0..=1.0).contains(v)));
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
