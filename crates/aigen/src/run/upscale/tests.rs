use super::*;
use crate::{Method, Response};
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL: &str = "upscale/RealESRGAN_x4plus.safetensors";
const OTHER: &str = "another-users-job";

fn png_header(w: u32, h: u32) -> Vec<u8> {
    [b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice(), &w.to_be_bytes(), &h.to_be_bytes()].concat()
}

fn response(status: u16, body: Value) -> Response {
    Response { status, body: body.to_string().into_bytes() }
}

fn output(filename: &str) -> Value {
    json!({"images":[{"filename":filename,"subfolder":"tmp/sub","type":"temp"}]})
}

pub(super) fn schema(class: &str, model: &str) -> Value {
    let inputs = match class {
        "UpscaleModelLoader" => json!({"model_name":["COMBO", {"options":[model]}]}),
        "LoadImage" => json!({"image":[]}),
        "ImageUpscaleWithModel" => json!({"image":[],"upscale_model":[]}),
        "PreviewImage" => json!({"images":[]}),
        "ImageScale" => json!({"image":[],"upscale_method":[],"width":[],"height":[],"crop":[]}),
        _ => return json!({}),
    };
    json!({(class):{"input":{"required":inputs}}})
}

fn settings() -> Settings {
    Settings { comfy_url: "https://comfy.example".into(), timeout_seconds: 4, ..Settings::default() }
}

#[test]
fn native_and_resized_workflows_and_checked_sizes() {
    for (model, factor, native, resize) in [
        ("RealESRGAN_x2plus.pth", 2, (22, 14), false),
        ("RealESRGAN_x4plus.pth", 4, (44, 28), false),
        ("RealESRGAN_x4plus.pth", 2, (44, 28), true),
        (MODEL, 4, (44, 28), false),
        (MODEL, 2, (44, 28), true),
    ] {
        let plan = UpscalePlan::new(model, factor, (11, 7)).unwrap();
        assert_eq!(plan.native(), native);
        let graph = workflow(plan, model, "sub/image.png");
        assert_eq!(graph["1"]["inputs"]["image"], "sub/image.png");
        assert_eq!(graph["2"]["inputs"]["model_name"], model);
        assert_eq!(graph["3"]["inputs"]["upscale_model"], json!(["2", 0]));
        assert_eq!(graph[OUTPUT]["inputs"]["images"], json!([if resize { "4" } else { "3" }, 0]));
        assert_eq!(graph.get(NATIVE_PREVIEW).is_some(), resize);
        if resize {
            assert_eq!(graph["4"]["inputs"]["width"], 22);
            assert_eq!(graph["4"]["inputs"]["height"], 14);
        }
    }
    for (model, factor, input) in [
        ("RealESRGAN_x2plus.pth", 4, (4, 4)),
        ("some4x.pth", 4, (4, 4)),
        ("some4x.safetensors", 4, (4, 4)),
        ("RealESRGAN_x2plus.safetensors", 2, (4, 4)),
        (MODEL, 3, (4, 4)),
        (MODEL, 2, (u32::MAX, 1)),
        (MODEL, 2, (4096, 4096)),
    ] {
        assert!(UpscalePlan::new(model, factor, input).is_err());
    }
}

struct Fake {
    requests: Mutex<Vec<Request>>,
    history_checks: AtomicUsize,
    missing: bool,
    missing_model: bool,
    bad_native: bool,
    legacy_cancel: bool,
    queue_lost: bool,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            history_checks: AtomicUsize::new(0),
            missing: false,
            missing_model: false,
            bad_native: false,
            legacy_cancel: false,
            queue_lost: false,
        }
    }
}

impl Transport for Fake {
    fn send(&self, req: &Request) -> Result<Response> {
        self.requests.lock().unwrap().push(req.clone());
        let u = req.url.as_str();
        if let Some((_, class)) = u.split_once("/object_info/") {
            return Ok(response(200, if self.missing && class == "PreviewImage" { json!({}) } else { schema(class, MODEL) }));
        }
        if u.ends_with("/models/upscale_models") {
            return Ok(response(200, json!([if self.missing_model { "other.pth" } else { MODEL }])));
        }
        if u.ends_with("/upload/image") {
            return Ok(response(200, json!({"name":"switched.png","subfolder":"sub"})));
        }
        if u.ends_with("/prompt") {
            return if self.queue_lost { Err(Error::Network("lost the reply".into())) } else { Ok(response(200, json!({"prompt_id":"my-job"}))) };
        }
        if u.ends_with("/history/my-job") {
            self.history_checks.fetch_add(1, Ordering::Relaxed);
            return Ok(response(
                200,
                json!({
                    (OTHER):{"status":{"completed":true},"outputs":{(OUTPUT):output("other.png")}},
                    "my-job":{"status":{"completed":true},"outputs":{"0":output("wrong.png"),(OUTPUT):output("right.png"),(NATIVE_PREVIEW):output("native.png")}}
                }),
            ));
        }
        if u.contains("filename=native.png") {
            assert!(req.headers.iter().any(|(name, value)| name == "Range" && value == "bytes=0-23"));
            return Ok(Response { status: 206, body: if self.bad_native { png_header(22, 14) } else { png_header(44, 28) } });
        }
        if u.contains("filename=right.png") {
            assert!(u.contains("subfolder=tmp%2Fsub&type=temp"));
            return Ok(Response { status: 200, body: png_header(22, 14) });
        }
        if u.ends_with("/api/jobs/my-job/cancel") {
            return Ok(response(if self.legacy_cancel { 404 } else { 200 }, json!({"cancelled":true})));
        }
        assert!(u.ends_with("/queue"), "{u}");
        assert_eq!(req.method, Method::Post);
        assert_eq!(serde_json::from_slice::<Value>(&req.body).unwrap(), json!({"delete":["my-job"]}));
        Ok(response(200, json!({})))
    }
}

fn render(fake: &Fake, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Vec<u8>> {
    let plan = UpscalePlan::new(MODEL, 2, (11, 7)).unwrap();
    let control = Control { cancelled, progress: &|_, _| {}, sleep: &|_| {} };
    run_upscale(fake, &settings(), plan, MODEL, b"rgb input", &control)
}

#[test]
fn checks_inventory_upload_and_specific_outputs() {
    let fake = Fake::default();
    assert_eq!(render(&fake, &|| false).unwrap(), png_header(22, 14));
    assert!(render(&fake, &|| false).is_ok());
    let requests = fake.requests.lock().unwrap();
    let uploads: Vec<_> = requests.iter().filter(|r| r.url.ends_with("/upload/image")).collect();
    // The multipart boundary is random too. Compare specifically the transmitted filenames.
    let filenames: Vec<_> =
        uploads.iter().map(|r| String::from_utf8_lossy(&r.body).split("filename=\"").nth(1).unwrap().split('"').next().unwrap().to_string()).collect();
    assert_ne!(filenames[0], filenames[1]);
    let queued = requests.iter().find(|r| r.url.ends_with("/prompt")).unwrap();
    let graph: Value = serde_json::from_slice(&queued.body).unwrap();
    assert_eq!(graph["prompt"]["1"]["inputs"]["image"], "sub/switched.png");
    assert_eq!(graph["prompt"]["2"]["inputs"]["model_name"], MODEL);
    assert!(requests.iter().all(|r| !r.url.contains("wrong.png") && !r.url.contains("other.png")));
}

#[test]
fn rejects_missing_capabilities_models_and_incorrect_native_size() {
    for (fake, expected) in [
        (Fake { missing: true, ..Fake::default() }, "PreviewImage"),
        (Fake { missing_model: true, ..Fake::default() }, "missing from"),
        (Fake { bad_native: true, ..Fake::default() }, "unsupported native scale"),
    ] {
        assert!(render(&fake, &|| false).unwrap_err().to_string().contains(expected));
    }
}

#[test]
fn cancellation_is_scoped_to_one_prompt_on_both_server_versions() {
    for legacy_cancel in [false, true] {
        let fake = Fake { legacy_cancel, ..Fake::default() };
        assert_eq!(render(&fake, &|| fake.history_checks.load(Ordering::Relaxed) > 0), Err(Error::Cancelled));
        let requests = fake.requests.lock().unwrap();
        assert!(requests.iter().any(|r| r.url.ends_with("/api/jobs/my-job/cancel")));
        assert_eq!(requests.iter().any(|r| r.url.ends_with("/queue")), legacy_cancel);
        assert!(requests.iter().all(|r| !r.url.ends_with("/interrupt") && !r.url.contains(OTHER) && !r.url.contains("/view")));
    }
}

#[test]
fn a_lost_prompt_reply_does_not_retry_or_cancel_an_unknown_job() {
    let fake = Fake { queue_lost: true, ..Fake::default() };
    assert!(render(&fake, &|| false).unwrap_err().to_string().contains("may already have accepted"));
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.url.ends_with("/prompt")).count(), 1);
    assert!(requests.iter().all(|r| !r.url.contains("cancel") && !r.url.ends_with("/queue") && !r.url.contains("/history")));
}

#[test]
fn discovery_reports_only_profiles_available_and_supported_by_server_nodes() {
    const X2: &str = "RealESRGAN_x2plus.pth";

    struct Inventory {
        models: Vec<&'static str>,
        resize: bool,
        offline: bool,
    }

    impl Transport for Inventory {
        fn send(&self, req: &Request) -> Result<Response> {
            if self.offline {
                return Err(Error::Network("connection refused".into()));
            }
            if req.url.ends_with("/models/upscale_models") {
                return Ok(response(200, json!(self.models)));
            }
            let (_, class) = req.url.split_once("/object_info/").unwrap();
            if class == "UpscaleModelLoader" {
                let mut loader = schema(class, MODEL);
                loader[class]["input"]["required"]["model_name"][1]["options"] = json!([X2, MODEL, "unverified.safetensors"]);
                return Ok(response(200, loader));
            }
            if class == "ImageScale" && !self.resize {
                return Ok(response(404, json!({})));
            }
            Ok(response(200, schema(class, MODEL)))
        }
    }

    let control = Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|_| {} };
    for (models, resize, expected) in [
        (vec![X2], true, vec![(X2, vec![2])]),
        (vec![MODEL], true, vec![(MODEL, vec![2, 4])]),
        (vec![MODEL], false, vec![(MODEL, vec![4])]),
        (vec![MODEL, X2, "unverified.safetensors"], true, vec![(X2, vec![2]), (MODEL, vec![2, 4])]),
        (vec![], true, vec![]),
    ] {
        let fake = Inventory { models, resize, offline: false };
        let expected: Vec<_> = expected.into_iter().map(|(name, scales)| (name.to_owned(), scales)).collect();
        assert_eq!(upscale_models(&fake, &settings().comfy_url, &control).unwrap(), expected);
    }
    let offline = Inventory { models: vec![X2], resize: true, offline: true };
    assert!(matches!(upscale_models(&offline, &settings().comfy_url, &control), Err(Error::Network(_))));
}
