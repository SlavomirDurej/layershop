use std::sync::{Arc, Mutex};

use photocraft_aigen::{Request, Response, Transport};
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_geom::Rect;
use serde_json::{Value, json};

use photocraft_algo::segment::RgbImage;

use super::{NOTHING_TITLE, closer_crop, cut_object, fitted, found_box, grown, mask_in_crop, nothing_to_remove, padded, sent_points};
use crate::Session;

/// What the fake engine's masks hold: for a matting model and for SAM, at the size it was sent.
type Answer = fn(u32, u32) -> Vec<u8>;

/// A ComfyUI that answers each workflow with the mask its model would make.
struct FakeEngine {
    matte: Answer,
    sam: Answer,
    /// The size of the last picture uploaded, and whether the last workflow queued was SAM's.
    last: Mutex<((u32, u32), bool)>,
    /// The sizes uploaded, and the workflows queued.
    sizes: Mutex<Vec<(u32, u32)>>,
    queued: Mutex<Vec<Value>>,
}

impl FakeEngine {
    fn new(matte: Answer, sam: Answer) -> Arc<Self> {
        Arc::new(Self { matte, sam, last: Mutex::new(((0, 0), false)), sizes: Mutex::new(Vec::new()), queued: Mutex::new(Vec::new()) })
    }
}

fn gray_png(w: u32, h: u32, values: &[u8]) -> Vec<u8> {
    let rgb: Vec<u8> = values.iter().flat_map(|v| [*v, *v, *v]).collect();
    let image = Image::from_u8(w, h, ChannelLayout::Rgb, rgb).unwrap();
    photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap()
}

impl Transport for FakeEngine {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        let body = if r.url.ends_with("/upload/image") {
            let at = r.body.windows(4).position(|w| w == b"\x89PNG").unwrap();
            let size = photocraft_codecs::decode(&r.body[at..]).unwrap().dimensions();
            self.last.lock().unwrap().0 = size;
            self.sizes.lock().unwrap().push(size);
            br#"{"name":"in.png","subfolder":""}"#.to_vec()
        } else if r.url.ends_with("/prompt") {
            let sent: Value = serde_json::from_slice(&r.body).unwrap();
            let prompt = sent["prompt"].clone();
            self.last.lock().unwrap().1 = prompt.as_object().unwrap().values().any(|n| n["class_type"] == "SAM3_Detect");
            self.queued.lock().unwrap().push(prompt);
            br#"{"prompt_id":"j"}"#.to_vec()
        } else if r.url.contains("/history/") {
            br#"{"j":{"status":{"status_str":"success","completed":true},"outputs":{"5":{"images":[{"filename":"m.png","subfolder":"","type":"temp"}]}}}}"#.to_vec()
        } else {
            let ((w, h), sam) = *self.last.lock().unwrap();
            gray_png(w, h, &(if sam { self.sam } else { self.matte })(w, h))
        };
        Ok(Response { status: 200, body })
    }
}

fn session(fake: &Arc<FakeEngine>) -> Session {
    let mut s = Session::new();
    s.ai.transport = Some(fake.clone());
    s.execute("file.new", json!({"width": 80, "height": 40, "depth": 8})).unwrap();
    s.edit("paint", |doc, _| {
        let bg = doc.layers[0].surface_mut().unwrap();
        let n = bg.channels();
        let px: Vec<f32> = (0..80 * 40).flat_map(|i| [0.2, 0.5, (i % 7) as f32 / 7.0, 1.0].into_iter().take(n)).collect();
        bg.write_region(Rect::new(0, 0, 80, 40), &px);
        Ok(())
    })
    .unwrap();
    s
}

/// White in a box given as shares of the picture, black around it.
fn boxed(w: u32, h: u32, b: [f32; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            out.push(if u >= b[0] && u < b[2] && v >= b[1] && v < b[3] { 255 } else { 0 });
        }
    }
    out
}

fn white(w: u32, h: u32) -> Vec<u8> {
    vec![255; (w * h) as usize]
}

fn black(w: u32, h: u32) -> Vec<u8> {
    vec![0; (w * h) as usize]
}

fn left_half(w: u32, h: u32) -> Vec<u8> {
    boxed(w, h, [0.0, 0.0, 0.5, 1.0])
}

fn right_part(w: u32, h: u32) -> Vec<u8> {
    boxed(w, h, [0.6, 0.25, 0.9, 0.75])
}

/// The active layer's mask at a pixel, 0..=1.
fn mask_at(s: &Session, x: i32, y: i32) -> f32 {
    let d = s.active().unwrap();
    let layer = d.doc.layer(d.active_layer.unwrap()).unwrap();
    layer.mask.as_ref().unwrap().value(x, y)
}

#[test]
fn the_main_subject_becomes_a_layer_mask_and_no_pixel_changes() {
    let fake = FakeEngine::new(left_half, black);
    let mut s = session(&fake);
    let before = s.active().unwrap().doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 80, 40));
    let r = s.execute("ai.removeBackground", json!({})).unwrap();
    assert_eq!((r["keep"].as_str(), r["output"].as_str(), r["model"].as_str(), r["matted"].as_bool()), (Some("subject"), Some("mask"), Some("birefnet"), Some(true)));
    assert!(mask_at(&s, 10, 20) > 0.99 && mask_at(&s, 70, 20) < 0.01);
    let d = s.active().unwrap();
    assert_eq!(d.doc.layers.len(), 1, "no new layer");
    assert_eq!(d.doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 80, 40)), before, "the pixels stay");
    let queued = fake.queued.lock().unwrap();
    assert!(queued.iter().all(|w| w["2"]["class_type"] == "LoadBackgroundRemovalModel"), "no SAM for the main subject");
    drop(queued);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active().unwrap().doc.layers[0].mask.is_none(), "one undo takes it away");
}

#[test]
fn a_description_finds_the_object_and_the_matting_model_cuts_it_from_a_crop() {
    let fake = FakeEngine::new(white, right_part);
    let mut s = session(&fake);
    let r = s.execute("ai.removeBackground", json!({"keep": "describe", "prompt": "keep the couple, remove everything else", "model": "lucida"})).unwrap();
    assert_eq!((r["phrase"].as_str(), r["model"].as_str(), r["matted"].as_bool()), (Some("couple"), Some("lucida"), Some(true)));
    let queued = fake.queued.lock().unwrap();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0]["3"]["inputs"]["text"], "couple", "SAM is asked for the object");
    assert_eq!(queued[1]["2"]["inputs"]["bg_removal_name"], "lucida.safetensors");
    let sizes = fake.sizes.lock().unwrap();
    assert_eq!(sizes[0], (80, 40), "SAM sees the whole layer");
    assert!(sizes[1].0 < 80, "the matting model sees it cropped: {:?}", sizes[1]);
    assert!(mask_at(&s, 60, 20) > 0.9, "the object is kept");
    assert!(mask_at(&s, 5, 20) < 0.01, "what is far from it is hidden, though the matte was white there");
}

#[test]
fn where_the_models_disagree_sams_object_is_kept() {
    let fake = FakeEngine::new(black, right_part);
    let mut s = session(&fake);
    let r = s.execute("ai.removeBackground", json!({"keep": "describe", "prompt": "dog"})).unwrap();
    assert_eq!(r["matted"], json!(false));
    assert!(mask_at(&s, 60, 20) > 0.9);
}

#[test]
fn nothing_found_says_so_and_changes_nothing() {
    let fake = FakeEngine::new(white, black);
    let mut s = session(&fake);
    let e = s.execute("ai.removeBackground", json!({"keep": "describe", "prompt": "unicorn"})).unwrap_err().to_string();
    assert!(e.contains("found no") && e.contains("unicorn"), "{e}");
    assert!(s.active().unwrap().doc.layers[0].mask.is_none());
    assert_eq!(fake.queued.lock().unwrap().len(), 1, "no matting without an object");
}

#[test]
fn a_selection_holds_the_result_near_it() {
    let fake = FakeEngine::new(white, black);
    let mut s = session(&fake);
    s.execute("select.rect", json!({"x": 40, "y": 10, "width": 20, "height": 20})).unwrap();
    let r = s.execute("ai.removeBackground", json!({"keep": "selection"})).unwrap();
    assert_eq!(r["keep"], "selection");
    assert!(mask_at(&s, 50, 20) > 0.9 && mask_at(&s, 2, 2) < 0.01);
    assert!(s.active().unwrap().doc.selection.is_none(), "the mask replaces the selection");
}

#[test]
fn clicks_reach_sam_in_the_pixels_it_was_sent() {
    let fake = FakeEngine::new(white, right_part);
    let mut s = session(&fake);
    let points = json!([{"x": 60, "y": 20}, {"x": 10, "y": 5, "keep": false}]);
    s.execute("ai.removeBackground", json!({"keep": "points", "points": points})).unwrap();
    let queued = fake.queued.lock().unwrap();
    let read = |key: &str| serde_json::from_str::<Value>(queued[0]["4"]["inputs"][key].as_str().unwrap()).unwrap();
    assert_eq!(read("positive_coords"), json!([{"x": 60, "y": 20}]));
    assert_eq!(read("negative_coords"), json!([{"x": 10, "y": 5}]));
}

#[test]
fn a_new_layer_keeps_the_original_and_cleaned_colours_need_one() {
    let fake = FakeEngine::new(left_half, black);
    let mut s = session(&fake);
    let source = s.active().unwrap().doc.layers[0].id;
    let r = s.execute("ai.removeBackground", json!({"output": "mask", "decontaminate": true})).unwrap();
    assert_eq!(r["output"], "layer", "cleaned colours are new pixels");
    let d = s.active().unwrap();
    assert_eq!(d.doc.layers.len(), 2);
    assert!(!d.doc.layer(source).unwrap().visible, "the original is hidden, not changed");
    assert_ne!(d.active_layer, Some(source));
    assert!(mask_at(&s, 10, 20) > 0.99);
}

#[test]
fn a_selection_can_be_the_output() {
    let fake = FakeEngine::new(left_half, black);
    let mut s = session(&fake);
    s.execute("ai.removeBackground", json!({"output": "selection"})).unwrap();
    let d = s.active().unwrap();
    assert!(d.doc.layers[0].mask.is_none());
    let selection = d.doc.selection.as_ref().unwrap();
    assert!(selection.read_region(Rect::new(10, 20, 11, 21))[0] > 0.99 && selection.read_region(Rect::new(70, 20, 71, 21))[0] < 0.01);
}

#[test]
fn bad_params_are_refused_without_a_crash() {
    let fake = FakeEngine::new(white, right_part);
    let mut s = session(&fake);
    for p in [
        json!({"keep": 3}),
        json!({"keep": "everything"}),
        json!({"keep": "selection"}),
        json!({"keep": "describe"}),
        json!({"keep": "describe", "prompt": "the"}),
        json!({"keep": "points"}),
        json!({"keep": "points", "points": []}),
        json!({"keep": "points", "points": "x"}),
        json!({"keep": "points", "points": [{"x": 1}]}),
        json!({"keep": "points", "points": [{"x": 1e300, "y": -5}]}),
        json!({"keep": "points", "points": [{"x": 900, "y": 5}]}),
        json!({"keep": "points", "points": [{"x": 5, "y": 5, "keep": false}]}),
        json!({"keep": "points", "points": [{"x": 5, "y": 5, "keep": "yes"}]}),
        json!({"keep": "points", "points": vec![json!({"x": 1, "y": 1}); 33]}),
        json!({"model": "sam-3-1"}),
        json!({"model": 7}),
        json!({"output": "elsewhere"}),
        json!({"decontaminate": "yes"}),
        json!({"output": "selection", "decontaminate": true}),
        json!({"layer": 999_999}),
    ] {
        assert!(s.execute("ai.removeBackground", p.clone()).is_err(), "{p}");
    }
    assert!(s.execute("ai.removeBackgroundInstall", json!({"model": "nope"})).is_err());
    assert!(s.execute("ai.removeBackgroundInstall", json!({})).is_err());
    assert!(s.active().unwrap().doc.layers[0].mask.is_none());
}

#[test]
fn an_engine_answer_that_is_no_mask_is_an_error() {
    struct Garbage;
    impl Transport for Garbage {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            let body: &[u8] = if r.url.ends_with("/upload/image") {
                br#"{"name":"in.png"}"#
            } else if r.url.ends_with("/prompt") {
                br#"{"prompt_id":"j"}"#
            } else if r.url.contains("/history/") {
                br#"{"j":{"outputs":{"5":{"images":[{"filename":"m.png"}]}}}}"#
            } else {
                b"\x89PNG\r\n\x1a\n not a png"
            };
            Ok(Response { status: 200, body: body.to_vec() })
        }
    }
    let fake = FakeEngine::new(white, black);
    let mut s = session(&fake);
    s.ai.transport = Some(Arc::new(Garbage));
    assert!(s.execute("ai.removeBackground", json!({})).is_err());
    assert!(s.active().unwrap().doc.layers[0].mask.is_none());
}

#[test]
fn the_models_are_listed_with_their_licences() {
    let fake = FakeEngine::new(white, black);
    let mut s = session(&fake);
    let r = s.execute("ai.removeBackgroundModels", json!({})).unwrap();
    let models = r["models"].as_array().unwrap();
    assert_eq!(models.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(), ["birefnet", "lucida", "rmbg-3", "sam-3-1"]);
    assert_eq!((&models[2]["licence"], &models[2]["kind"]), (&json!("CC BY-NC 4.0"), &json!("matte")));
    assert_eq!(models[3]["licence"], "SAM License");
    assert_eq!(models[3]["kind"], "target");
}

#[test]
fn every_model_is_pinned_in_the_setup_manifest() {
    let manifest = crate::ai_setup_cmds::bundled().unwrap();
    for model in photocraft_aigen::bg_removal::models() {
        let file = manifest.files.get(&model.manifest_key()).unwrap_or_else(|| panic!("{} is not in the manifest", model.id));
        assert_eq!(file.size, model.size, "{}", model.id);
        assert!(!file.gated, "{}", model.id);
        for shared in model.shared {
            assert_eq!(manifest.files.get(&shared.manifest_key()).map(|f| f.size), Some(shared.size), "{}", shared.file);
        }
    }
}

#[test]
fn rmbg_is_sent_the_size_it_renders_and_cuts_like_the_others() {
    let fake = FakeEngine::new(left_half, black);
    let mut s = session(&fake);
    let r = s.execute("ai.removeBackground", json!({"model": "rmbg-3"})).unwrap();
    assert_eq!(r["model"], "rmbg-3");
    assert!(mask_at(&s, 10, 20) > 0.9 && mask_at(&s, 70, 20) < 0.1);
    assert_eq!(fake.sizes.lock().unwrap()[0], (1408, 704));
    let queued = fake.queued.lock().unwrap();
    assert_eq!((&queued[0]["5"]["inputs"]["width"], &queued[0]["5"]["inputs"]["height"]), (&json!(1408), &json!(704)));
}

#[test]
fn rmbg_is_ready_only_with_the_files_it_shares() {
    use photocraft_aigen::bg_removal::find_matte;
    let rmbg = find_matte("rmbg-3").unwrap();
    let dir = std::env::temp_dir().join(format!("photocraft-rmbg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dirs = [dir.clone()];
    let shared: u64 = rmbg.shared.iter().map(|f| f.size).sum();
    assert_eq!(super::left_to_fetch(rmbg, &dirs, None), rmbg.size + shared);
    // Listed by the engine, under the older folder names too.
    let mut listed = std::collections::BTreeMap::new();
    listed.insert("unet".to_string(), vec![rmbg.file.to_string()]);
    listed.insert("clip".to_string(), vec![rmbg.shared[0].file.to_string()]);
    assert_eq!(super::left_to_fetch(rmbg, &dirs, Some(&listed)), rmbg.shared[1].size, "only the VAE is left");
    assert!(!super::on_disk(rmbg, &dirs, Some(&listed)));
    listed.insert("vae".to_string(), vec![rmbg.shared[1].file.to_string()]);
    assert!(super::on_disk(rmbg, &dirs, Some(&listed)));
    // A shared file is known by its name: another publisher's copy can differ in size.
    std::fs::create_dir_all(dir.join("vae")).unwrap();
    std::fs::write(dir.join("vae").join(rmbg.shared[1].file), b"weights").unwrap();
    assert!(super::has_file("vae", rmbg.shared[1].file, None, &dirs, None));
    assert!(!super::has_file("vae", rmbg.shared[1].file, Some(rmbg.shared[1].size), &dirs, None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sizes_boxes_and_clicks_are_kept_in_bounds() {
    assert_eq!(fitted(4000, 2000, 2048), (2048, 1024));
    assert_eq!(fitted(100, 50, 2048), (100, 50), "never scaled up");
    assert_eq!(fitted(100_000, 1, 2048), (2048, 1));
    let within = Rect::new(0, 0, 100, 100);
    assert_eq!(padded(Rect::new(0, 40, 10, 50), within), Rect::new(0, 24, 26, 66));
    assert_eq!(found_box(&[0.0; 16], (4, 4)), None);
    assert_eq!(found_box(&[1.0; 16], (4, 4)), Some((0, 0, 4, 4)));
    assert_eq!(found_box(&[1.0; 4], (0, 4)), None);
    assert_eq!(sent_points(&[[99.9, 0.0]], within, (10, 10)), [[9, 0]]);
}

#[test]
fn a_mask_lands_where_it_was_on_the_crop() {
    // A 4 × 2 mask of a 40 × 20 area, its right half on.
    let mask = [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0];
    let area = Rect::new(10, 10, 50, 30);
    let got = mask_in_crop(&mask, (4, 2), area, Rect::new(30, 10, 50, 30), (2, 2));
    assert_eq!(got, [1.0; 4]);
    assert_eq!(mask_in_crop(&mask, (4, 2), area, Rect::new(10, 10, 30, 30), (2, 2)), [0.0; 4]);
}

#[test]
fn an_outline_grows_to_the_edge_of_the_picture_without_fading_there() {
    let (w, h) = (20, 10);
    let mut mask = vec![0.0; w * h];
    mask[5 * w] = 1.0; // On the left edge.
    let out = grown(&mask, w, h, 3);
    assert!(out[5 * w] > 0.99, "{}", out[5 * w]);
    assert!(out[5 * w + 2] > 0.5);
    assert_eq!(out[5 * w + 12], 0.0);
    assert!(grown(&[], 0, 0, 3).is_empty());
}

#[test]
fn the_matte_is_used_only_where_it_covers_the_object() {
    let size = (8, 8);
    let object: Vec<f32> = (0..64).map(|i| f32::from(i % 8 < 4)).collect();
    let picture = RgbImage { w: 8, h: 8, px: object.iter().map(|o| [*o, *o, *o]).collect() };
    let (kept, matted) = cut_object(&vec![1.0; 64], &object, &picture, size);
    assert!(matted);
    assert!(kept[0] > 0.9);
    let (kept, matted) = cut_object(&vec![0.0; 64], &object, &picture, size);
    assert!(!matted);
    assert!(kept[0] > 0.9 && kept[7] < 0.1, "SAM's object");
}

/// A matte that took what stands next to the object for the subject too (trophies on a mantel).
#[test]
fn what_the_matte_wrongly_kept_beside_the_object_leaves_no_blurred_band() {
    let (w, h) = (200usize, 100usize);
    let object: Vec<f32> = (0..w * h).map(|i| f32::from(i % w < 100)).collect();
    let picture = RgbImage { w, h, px: object.iter().map(|o| [*o, 0.3, 1.0 - *o]).collect() };
    let (kept, matted) = cut_object(&vec![1.0; w * h], &object, &picture, (w as u32, h as u32));
    assert!(matted);
    let row = 50 * w;
    assert!(kept[row + 98] > 0.9, "the object: {}", kept[row + 98]);
    assert!(kept[row + 103] < 0.05, "just past its edge: {}", kept[row + 103]);
}

#[test]
fn hair_that_runs_along_the_outline_keeps_the_mattes_edge() {
    let (w, h) = (200usize, 100usize);
    let object: Vec<f32> = (0..w * h).map(|i| f32::from(i % w < 100)).collect();
    let picture = RgbImage { w, h, px: vec![[0.5; 3]; w * h] };
    // A strand one pixel wide, just outside SAM's outline.
    let mut matte = object.clone();
    for y in 0..h {
        matte[y * w + 102] = 0.8;
    }
    let (kept, _) = cut_object(&matte, &object, &picture, (w as u32, h as u32));
    assert!(kept[50 * w + 102] > 0.7, "{}", kept[50 * w + 102]);
}

#[test]
fn a_subject_filling_part_of_the_layer_gets_a_closer_look() {
    let crop = Rect::new(0, 0, 200, 100);
    let mut matte = vec![0.0; 200 * 100];
    for y in 20..80 {
        for x in 60..120 {
            matte[y * 200 + x] = 1.0;
        }
    }
    let closer = closer_crop(&matte, (200, 100), crop).unwrap();
    assert!(closer.x0 > 0 && closer.x1 < 200 && closer.x0 <= 60 && closer.x1 >= 120, "{closer:?}");
    assert_eq!(closer_crop(&vec![1.0; 200 * 100], (200, 100), crop), None, "it fills the layer");
    assert_eq!(closer_crop(&vec![0.0; 200 * 100], (200, 100), crop), None, "nothing found");
}

/// A session whose active layer has pixels only in a small box.
fn small_layer(fake: &Arc<FakeEngine>) -> Session {
    let mut s = session(fake);
    s.execute("layer.new.layer", json!({})).unwrap();
    s.edit("paint", |doc, active| {
        let id = active.unwrap();
        let surface = doc.layer_mut(id).unwrap().surface_mut().unwrap();
        let n = surface.channels();
        let px: Vec<f32> = (0..100).flat_map(|_| [0.8, 0.2, 0.1, 1.0].into_iter().take(n)).collect();
        surface.write_region(Rect::new(10, 10, 20, 20), &px);
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn clicks_only_where_the_layer_is_empty_say_there_is_nothing_to_remove() {
    let fake = FakeEngine::new(white, right_part);
    let mut s = small_layer(&fake);
    assert!(nothing_to_remove(&s, &[[60.0, 30.0]]));
    assert!(!nothing_to_remove(&s, &[[60.0, 30.0], [15.0, 15.0]]), "one click is on it");
    let e = s.execute("ai.removeBackground", json!({"keep": "points", "points": [{"x": 60, "y": 30}]})).unwrap_err().to_string();
    assert!(e.contains(NOTHING_TITLE), "{e}");
    assert!(!e.contains("invalid parameters"), "{e}");
    assert!(fake.queued.lock().unwrap().is_empty(), "the engine is not asked");
}

#[test]
fn an_empty_layer_has_nothing_to_remove() {
    let fake = FakeEngine::new(white, black);
    let mut s = session(&fake);
    s.execute("layer.new.layer", json!({})).unwrap();
    assert!(nothing_to_remove(&s, &[]));
    let e = s.execute("ai.removeBackground", json!({})).unwrap_err().to_string();
    assert!(e.contains(NOTHING_TITLE), "{e}");
}
