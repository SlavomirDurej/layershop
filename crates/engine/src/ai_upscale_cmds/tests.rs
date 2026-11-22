use super::*;
use photocraft_aigen::{Request, Response};
use photocraft_doc::{Layer, LayerContent, LayerMask, SmartSource};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

const MODEL: &str = "RealESRGAN_x2plus.pth";

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    uploaded: Mutex<Vec<u8>>,
    workflow: Mutex<Value>,
    hold: bool,
    entered: AtomicBool,
    release: (Mutex<bool>, Condvar),
    wrong_size: bool,
    corrupt: bool,
}

fn response(value: Value) -> photocraft_aigen::Result<Response> {
    Ok(Response { status: 200, body: value.to_string().into_bytes() })
}

impl Transport for Fake {
    fn send(&self, req: &Request) -> photocraft_aigen::Result<Response> {
        let url = req.url.as_str();
        self.calls.lock().unwrap().push(url.to_string());
        if let Some((_, class)) = url.split_once("/object_info/") {
            let inputs = match class {
                "UpscaleModelLoader" => json!({"model_name":[[MODEL],{}]}),
                "LoadImage" => json!({"image":[]}),
                "ImageUpscaleWithModel" => json!({"image":[],"upscale_model":[]}),
                "PreviewImage" => json!({"images":[]}),
                _ => panic!("unexpected class {class}"),
            };
            return response(json!({(class):{"input":{"required":inputs}}}));
        }
        if url.ends_with("/models/upscale_models") {
            return response(json!([MODEL]));
        }
        if url.ends_with("/upload/image") {
            let from = req.body.windows(8).position(|v| v == b"\x89PNG\r\n\x1a\n").unwrap();
            let end = req.body[from..].windows(8).position(|v| v == b"IEND\xaeB`\x82").unwrap() + from + 8;
            *self.uploaded.lock().unwrap() = req.body[from..end].to_vec();
            return response(json!({"name":"real-input.png","subfolder":"real-sub"}));
        }
        if url.ends_with("/prompt") {
            *self.workflow.lock().unwrap() = serde_json::from_slice(&req.body).unwrap();
            return response(json!({"prompt_id":"ours"}));
        }
        if url.ends_with("/history/ours") {
            self.entered.store(true, Ordering::Release);
            if self.hold {
                let (lock, wait) = &self.release;
                let guard = lock.lock().unwrap();
                drop(wait.wait_timeout_while(guard, Duration::from_secs(5), |released| !*released).unwrap());
            }
            return response(json!({"ours":{"status":{"completed":true},"outputs":{
                "0":{"images":[{"filename":"unrelated.png"}]},
                "5":{"images":[{"filename":"legit.png","type":"temp","subfolder":""}]}
            }}}));
        }
        if url.contains("/view?filename=legit.png") {
            let size = if self.wrong_size { (6, 6) } else { (8, 6) };
            let mut body = encode(size, ChannelLayout::Rgb, [17, 200, 90].repeat((size.0 * size.1) as usize)).unwrap();
            if self.corrupt {
                body.truncate(24);
            }
            return Ok(Response { status: 200, body });
        }
        assert!(url.ends_with("/api/jobs/ours/cancel"), "{url}");
        response(json!({"cancelled":true}))
    }
}

fn fixture(fake: &Arc<Fake>) -> (Session, LayerId) {
    let mut session = Session::new();
    session.ai.transport = Some(fake.clone());
    let mut doc = Document::new("Upscale", Size::new(12, 10), ColorMode::Rgb, SampleType::U8);
    let mut source = Surface::new(doc.pixel_format());
    source.fill_rect(Rect::new(-2, 3, 2, 6), &[1.0, 0.0, 0.0, 1.0]);
    source.fill_rect(Rect::new(-2, 4, -1, 5), &[0.0, 0.0, 0.0, 0.0]);
    source.fill_rect(Rect::new(-1, 4, 0, 5), &[1.0, 0.0, 0.0, 0.5]);
    let mut layer = Layer::new("Source", LayerContent::Raster(source));
    layer.opacity = 0.7;
    layer.mask = Some(LayerMask::hide_all());
    let id = layer.id;
    let mut neighbor = Surface::new(doc.pixel_format());
    neighbor.fill_rect(doc.bounds(), &[0.0, 0.0, 1.0, 1.0]);
    doc.layers = vec![Layer::new("Neighbor", LayerContent::Raster(neighbor)), layer];
    session.add_document(doc, None);
    session.select_layer(id).unwrap();
    (session, id)
}

fn start(s: &mut Session) -> jobs::JobId {
    let jobs::Started::Job(id) = s.start(ID, json!({"scale":2,"model":MODEL})).unwrap() else { panic!("expected an upscale job") };
    id
}

#[test]
fn full_chain_uploads_only_unmasked_layer_and_applies_once() {
    let fake = Arc::new(Fake::default());
    let (mut s, source) = fixture(&fake);
    let original = s.active().unwrap().doc.clone();
    let job = start(&mut s);
    let reply = s.wait_job(job).unwrap();
    assert_eq!(reply["job"], job.0);
    assert_eq!(reply["sourceLayer"], source.0);
    assert_eq!(reply["sourceSize"], json!([4, 3]));
    assert_eq!(reply["outputSize"], json!([8, 6]));
    let png = photocraft_codecs::decode(&fake.uploaded.lock().unwrap()).unwrap();
    assert_eq!(png.dimensions(), (4, 3));
    assert_eq!(png.layout(), ChannelLayout::Rgb);
    assert_eq!(png.into_data(), [255, 0, 0].repeat(12));
    assert_eq!(fake.workflow.lock().unwrap()["prompt"]["1"]["inputs"]["image"], "real-sub/real-input.png");
    assert_eq!(fake.workflow.lock().unwrap()["prompt"]["2"]["inputs"]["model_name"], MODEL);
    s.poll_jobs();
    s.poll_jobs();
    let doc = &s.active().unwrap().doc;
    assert_eq!(&doc.layers[..2], &original.layers);
    assert_eq!(doc.layers.len(), 3);
    let layer = &doc.layers[2];
    assert_eq!(reply["layer"], layer.id.0);
    assert_eq!(layer.mask, original.layers[1].mask);
    assert_eq!(layer.opacity, 0.7);
    let LayerContent::Smart(object) = &layer.content else { panic!("expected a smart object") };
    assert_eq!(object.transform.m, [0.5, 0.0, 0.0, 0.5, -2.0, 3.0]);
    let SmartSource::Embedded { bytes, .. } = &object.source else { panic!("expected embedded PNG") };
    let image = photocraft_codecs::decode(bytes).unwrap();
    assert_eq!(image.layout(), ChannelLayout::Rgba);
    let rgba = image.into_data();
    assert_eq!(&rgba[0..4], &[17, 200, 90, 255]);
    assert!(rgba[2 * 8 * 4 + 3] < 100);
    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc.layers, original.layers);
    assert!(s.redo());
    assert_eq!(s.active().unwrap().doc.layers.len(), 3);
}

#[test]
fn errors_or_invalid_parameters_never_edit_the_document() {
    for fake in [Fake { wrong_size: true, ..Fake::default() }, Fake { corrupt: true, ..Fake::default() }] {
        let (mut s, source) = fixture(&Arc::new(fake));
        let before = s.active().unwrap().doc.clone();
        for params in [
            json!({"scale":3,"model":MODEL}),
            json!({"scale":2,"model":"unknown.pth"}),
            json!({"scale":2,"model":MODEL,"layer":u64::MAX}),
            json!({"scale":2,"model":MODEL,"layer":source.0,"url":"http://another-server"}),
        ] {
            assert!(s.execute(ID, params).is_err());
        }
        let job = start(&mut s);
        assert!(s.wait_job(job).is_err());
        assert!(Arc::ptr_eq(&s.active().unwrap().doc, &before));
        assert!(!s.undo());
    }
}

fn reach_history(fake: &Fake) {
    let until = std::time::Instant::now() + Duration::from_secs(3);
    while !fake.entered.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn cancellation_and_stale_snapshot_cannot_add_a_layer() {
    for cancel in [true, false] {
        let fake = Arc::new(Fake { hold: true, ..Fake::default() });
        let (mut s, _) = fixture(&fake);
        let job = start(&mut s);
        reach_history(&fake);
        if cancel {
            assert!(s.cancel_job(job));
        } else {
            // Direct edits can bypass command admission; the job still must reject stale snapshots.
            s.edit("new revision", |doc, _| {
                doc.layers[0].name = "Changed".into();
                Ok(())
            })
            .unwrap();
        }
        *fake.release.0.lock().unwrap() = true;
        fake.release.1.notify_all();
        let error = s.wait_job(job).unwrap_err();
        if cancel {
            assert!(matches!(error, EngineError::Cancelled));
            s.join_cancelled_jobs();
            assert!(fake.calls.lock().unwrap().iter().any(|url| url.ends_with("/api/jobs/ours/cancel")));
            assert!(!s.undo());
        } else {
            assert!(error.to_string().contains("changed while it ran"), "{error}");
        }
        assert_eq!(s.active().unwrap().doc.layers.len(), 2);
        assert!(fake.calls.lock().unwrap().iter().all(|url| !url.ends_with("/interrupt")));
    }
}

#[test]
fn alpha_interpolation_and_rgba_reconstruction_are_bounded() {
    assert_eq!((0..4).map(|x| sample_alpha(&[0, 255], (2, 1), (4, 2), x, 0)).collect::<Vec<_>>(), [0, 64, 191, 255]);
    let size = (4, 2);
    let rgb = encode(size, ChannelLayout::Rgb, [4, 8, 12].repeat(8)).unwrap();
    let png = restore_rgba(rgb, &[0, 255], (2, 1), size, &JobCtx::new()).unwrap();
    let pixels = photocraft_codecs::decode(&png).unwrap().into_data();
    assert_eq!(&pixels[..16], &[4, 8, 12, 0, 4, 8, 12, 64, 4, 8, 12, 191, 4, 8, 12, 255]);
    assert!(count((u32::MAX, u32::MAX)).is_err());
}
