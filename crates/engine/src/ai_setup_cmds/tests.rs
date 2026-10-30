use super::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use photocraft_aigen::{Method, Request, Response, Transport};
    use photocraft_aisetup::fetch::{Begin, Fetch, Head, Sink};
    use photocraft_aisetup::plan::{Choices, Route};
    use photocraft_aisetup::state::{self, Phase, SetupState};
    use photocraft_aisetup::{Manifest, runtime};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const COMMANDS: [&str; 14] = [
        "ai.setup.probe",
        "ai.setup.manifest",
        "ai.setup.plan",
        "ai.setup.status",
        "ai.setup.start",
        "ai.setup.cancel",
        "ai.setup.repair",
        "ai.setup.remove",
        "ai.setup.setToken",
        "ai.setup.hfStatus",
        "ai.setup.hfAccess",
        "ai.setup.hfSignIn",
        "ai.setup.hfSignOut",
        "ai.setup.hfUseFound",
    ];

    fn sha(bytes: &[u8]) -> String {
        photocraft_aisetup::download::sha256_hex(bytes)
    }

    fn contents(key: &str) -> Vec<u8> {
        let seed = key.bytes().fold(3u32, |a, b| a.wrapping_mul(33).wrapping_add(u32::from(b)));
        (0..20_000u32).map(|i| (i.wrapping_mul(seed) >> 5) as u8).collect()
    }

    /// The real manifest with two models, and files small enough to serve from memory.
    fn small_manifest() -> Manifest {
        let mut m = Manifest::bundled().unwrap().clone();
        m.models.retain(|x| ["qwen-image-2-1", "z-image-turbo"].contains(&x.id.as_str()));
        let used: Vec<String> = m.models.iter().flat_map(|x| x.files.clone()).collect();
        m.files.retain(|key, _| used.contains(key));
        for (key, file) in &mut m.files {
            let bytes = contents(key);
            (file.size, file.sha256) = (bytes.len() as u64, sha(&bytes));
        }
        m.presets.retain(|name, _| name == "starter");
        m
    }

    #[derive(Default)]
    struct FakeNet {
        files: HashMap<String, Vec<u8>>,
        served: AtomicU64,
        /// Hold every download until this is cleared.
        hold: AtomicBool,
        /// A gated file (by address) and the token whose owner accepted its licence: other tokens
        /// get 403, no token (or a `revoked…` one) 401, as Hugging Face answers. `moved`: the file
        /// is not there any more, 404 to anyone signed in.
        gated: std::sync::Mutex<HashMap<String, String>>,
    }

    impl Fetch for FakeNet {
        fn head(&self, url: &str, token: Option<&str>) -> photocraft_aisetup::Result<Head> {
            if let Some(accepted) = self.gated.lock().unwrap().get(url) {
                let status = match token {
                    None => 401,
                    Some(t) if t.starts_with("revoked") => 401,
                    Some(_) if accepted == "moved" => 404,
                    Some(t) if t == accepted => 200,
                    Some(_) => 403,
                };
                return Ok(Head { status, error_code: (status != 200).then(|| "GatedRepo".to_string()), ..Head::default() });
            }
            Ok(Head { status: 200, size: self.files.get(url).map(|f| f.len() as u64), sha256: None, ..Head::default() })
        }

        fn fetch(&self, request: &photocraft_aisetup::fetch::Request<'_>, sink: &mut dyn Sink, cancelled: &dyn Fn() -> bool) -> photocraft_aisetup::Result<()> {
            while self.hold.load(Ordering::SeqCst) {
                if cancelled() {
                    return Err(photocraft_aisetup::Error::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let data = self.files.get(request.url).ok_or_else(|| photocraft_aisetup::Error::Server("404".into()))?;
            let from = (request.offset as usize).min(data.len());
            sink.begin(&Begin { offset: from as u64, total: Some(data.len() as u64), validator: String::new() })?;
            self.served.fetch_add((data.len() - from) as u64, Ordering::SeqCst);
            sink.write(&data[from..])
        }
    }

    struct FakeComfy;

    impl Transport for FakeComfy {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            let ok = |body: &str| Ok(Response { status: 200, body: body.as_bytes().to_vec() });
            if r.url.ends_with("/system_stats") {
                ok(r#"{"system":{"argv":["main.py"]}}"#)
            } else if r.url.ends_with("/prompt") && r.method == Method::Post {
                ok(r#"{"prompt_id":"p1"}"#)
            } else if r.url.contains("/history/") {
                ok(r#"{"p1":{"outputs":{"9":{"images":[{"filename":"t.png","subfolder":"","type":"output"}]}},"status":{"completed":true}}}"#)
            } else if r.url.contains("/view?") {
                ok("PNG")
            } else {
                Ok(Response { status: 404, body: Vec::new() })
            }
        }
    }

    struct Rig {
        dir: tempfile::TempDir,
        net: Arc<FakeNet>,
        manifest: Arc<Manifest>,
        s: Session,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let manifest = Arc::new(small_manifest());
            let files = manifest.files.iter().map(|(key, f)| (f.url.clone(), contents(key))).collect();
            let net = Arc::new(FakeNet { files, ..FakeNet::default() });
            let mut s = Session::new();
            s.ai.transport = Some(Arc::new(FakeComfy));
            s.ai_setup.env = Some(SetupEnv {
                fetch: net.clone(),
                state_file: dir.path().join("settings").join("ai-setup.json"),
                engine_dir: dir.path().join("engine").to_string_lossy().into_owned(),
                models_dir: dir.path().join("models").to_string_lossy().into_owned(),
                manifest: Some(manifest.clone()),
                hf_client_id: String::new(),
            });
            Self { dir, net, manifest, s }
        }

        fn models_dir(&self) -> PathBuf {
            self.dir.path().join("models")
        }

        fn run(&mut self, id: &str, params: Value) -> Result<Value> {
            self.s.execute(id, params)
        }

        /// Wait for the running setup to end and be applied.
        fn finish(&mut self) -> Value {
            let began = Instant::now();
            loop {
                let status = self.run("ai.setup.status", json!({})).unwrap();
                if status["running"] == false {
                    return status;
                }
                assert!(began.elapsed() < Duration::from_secs(30), "the setup did not end");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn existing(&self, models: &[&str]) -> Value {
            json!({"route": "existing", "models": models, "modelsDir": self.models_dir().to_string_lossy(), "url": "http://127.0.0.1:8000", "serverFiles": {}})
        }
    }

    #[test]
    fn without_the_desktop_app_nothing_installs_and_everything_answers() {
        let mut s = Session::new();
        for id in ["ai.setup.probe", "ai.setup.plan", "ai.setup.start", "ai.setup.repair", "ai.setup.remove"] {
            let e = s.execute(id, json!({"models": ["qwen-image-2-1"], "engine": true})).unwrap_err().to_string();
            assert!(e.contains("needs the desktop app"), "{id}: {e}");
            assert!(!s.is_enabled(id), "{id}");
        }
        // What can be installed is no secret: the bundled manifest, with its totals.
        let m = s.execute("ai.setup.manifest", json!({"vramGb": 24})).unwrap();
        assert_eq!(m["comfy"]["version"], "0.43.0");
        assert_eq!(m["models"].as_array().unwrap().len(), 10);
        assert_eq!(
            (m["presets"]["starter"]["bytes"].as_u64(), m["presets"]["recommended"]["models"].as_array().map(Vec::len)),
            (Some(18_520_930_264), Some(3))
        );
        let qwen = &m["models"][0];
        assert_eq!(
            (qwen["id"].as_str(), qwen["installed"].as_bool(), qwen["fit"].as_str(), qwen["licence"]["commercial"].as_bool()),
            (Some("qwen-image-2-1"), Some(false), Some("tested"), Some(false))
        );
        // Each says which ComfyUI it needs: FIBO 2 the one the setup installs. It is offered, not
        // ticked: no preset but Everything has it.
        assert_eq!(qwen["minComfy"], "0.37.0");
        let fibo = m["models"].as_array().unwrap().iter().find(|x| x["id"] == "fibo-2-dev").unwrap();
        assert_eq!((fibo["minComfy"].as_str(), fibo["licence"]["commercial"].as_bool(), fibo["gated"].as_bool()), (Some("0.43.0"), Some(false), Some(false)));
        for preset in ["light", "starter", "recommended"] {
            assert!(!m["presets"][preset]["models"].as_array().unwrap().iter().any(|id| id == "fibo-2-dev"), "{preset}");
        }
        assert_eq!(m["suggested"], "recommended");
        // A smaller card than any it has run on: not tested, and it says so instead of guessing.
        // It starts on the light preset, its model listed first.
        let small = s.execute("ai.setup.manifest", json!({"vramGb": 8})).unwrap();
        assert_eq!((small["suggested"].as_str(), small["models"][0]["id"].as_str()), (Some("light"), Some("moss-image")));
        assert_eq!(small["presets"]["light"]["models"], json!(["moss-image"]));
        assert_eq!(small["models"][0]["fit"], "untested");
        assert_eq!(small["models"][1]["id"], "qwen-image-2-1");
        let unknown = s.execute("ai.setup.manifest", json!({})).unwrap();
        assert_eq!((unknown["suggested"].as_str(), unknown["models"][0]["fit"].as_str()), (Some("recommended"), Some("untested")));
        let status = s.execute("ai.setup.status", json!({})).unwrap();
        assert_eq!((status["available"].as_bool(), status["state"].as_str(), status["running"].as_bool()), (Some(false), Some("none"), Some(false)));
        assert_eq!(s.execute("ai.setup.cancel", json!({})).unwrap()["stopping"], false);
        assert!(!poll(&mut s) && s.ai_setup.seq() == 0 && s.ai_setup.last().is_none());
    }

    /// Rule 9: every command answers odd parameters with an error, never a panic, and none of
    /// them starts anything.
    #[test]
    fn odd_parameters_are_errors() {
        let mut rig = Rig::new();
        let odd = [
            json!(null),
            json!([]),
            json!("x"),
            json!({"models": "all"}),
            json!({"models": [1, 2]}),
            json!({"models": ["../../etc"]}),
            json!({"models": ["no-such-model"]}),
            json!({"route": "sideways"}),
            json!({"route": 7}),
            json!({"engineDir": 7}),
            json!({"modelsDir": "relative/path", "models": []}),
            json!({"modelsDir": "x\u{0}y"}),
            json!({"variant": "voodoo"}),
            json!({"recheck": "yes"}),
            json!({"testImage": 1}),
            json!({"serverFiles": []}),
            json!({"serverFiles": {"vae": "x"}}),
            json!({"vramGb": -1}),
            json!({"vramGb": "lots"}),
            json!({"token": 7}),
            json!({"token": "a\nb"}),
            json!({"engine": "yes"}),
            json!({"models": {"a": 1}, "engine": true}),
            json!({"launch": "python"}),
            json!({"url": "ftp://x", "route": "existing", "models": []}),
            json!({"cancel": "yes"}),
            json!({"product": 7}),
        ];
        // The probe takes no parameters and really looks at this machine: once is enough.
        for id in COMMANDS.into_iter().filter(|id| *id != "ai.setup.probe") {
            for params in &odd {
                // Each either refuses or does something harmless; it never panics.
                let _ = rig.run(id, params.clone());
            }
        }
        let seen = rig.run("ai.setup.probe", json!({"odd": [1, 2]})).unwrap();
        let rows: Vec<&str> = seen["checks"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        // Memory is listed where Windows says how much there is.
        let rows: Vec<&str> = rows.into_iter().filter(|r| *r != "memory").collect();
        assert_eq!(rows, ["os", "gpu", "disk", "comfy", "net"]);
        assert_eq!(seen["defaults"]["modelsDir"], rig.models_dir().to_string_lossy().as_ref());
        assert!(seen["variant"].is_string() && seen["canInstall"].is_boolean() && seen["gpuUsable"].is_boolean());
        assert!(!rig.s.ai_setup.running(), "nothing was started by odd parameters");
        assert_eq!(rig.net.served.load(Ordering::SeqCst), 0);
        assert!(!rig.dir.path().join("engine").exists());
        // The ones that must be refused are.
        for (id, params) in [
            ("ai.setup.plan", json!({"models": ["no-such-model"]})),
            ("ai.setup.plan", json!({"route": "sideways"})),
            ("ai.setup.plan", json!({"variant": "voodoo"})),
            ("ai.setup.plan", json!({"modelsDir": "relative/path", "models": []})),
            ("ai.setup.start", json!({"models": [1]})),
            ("ai.setup.start", json!({"url": "ftp://x", "route": "existing", "models": []})),
            ("ai.setup.remove", json!({})),
            ("ai.setup.remove", json!({"models": ["nope"]})),
            ("ai.setup.setToken", json!({})),
            ("ai.setup.setToken", json!({"token": "a\nb"})),
            ("ai.setup.manifest", json!({"vramGb": "lots"})),
            ("ai.setup.hfAccess", json!({"models": "all"})),
            ("ai.setup.hfAccess", json!({"models": ["no-such-model"]})),
            ("ai.setup.hfSignIn", json!({"cancel": "yes"})),
        ] {
            assert!(matches!(rig.run(id, params.clone()), Err(EngineError::BadParams { .. })), "{id} {params}");
        }
        // Nothing has been set up: there is nothing to repair or remove.
        assert!(rig.run("ai.setup.repair", json!({})).unwrap_err().to_string().contains("no setup to repair"));
        assert!(rig.run("ai.setup.remove", json!({"engine": true})).unwrap_err().to_string().contains("nothing this app installed"));
    }

    #[test]
    fn a_plan_says_what_would_be_downloaded_and_what_stands_in_the_way() {
        let mut rig = Rig::new();
        let plan = rig.run("ai.setup.plan", json!({"models": ["qwen-image-2-1"]})).unwrap();
        // The app's default folders, the build for NVIDIA, the real runtime's size.
        assert_eq!(plan["engineDir"], rig.dir.path().join("engine").to_string_lossy().as_ref());
        assert_eq!((plan["variant"].as_str(), plan["route"].as_str()), (Some("nvidia"), Some("managed")));
        let steps: Vec<&str> = plan["steps"].as_array().unwrap().iter().map(|s| s["label"].as_str().unwrap()).collect();
        assert_eq!(steps.len(), 8, "{steps:?}");
        assert_eq!(plan["modelsBytes"], 60_000);
        assert_eq!(plan["downloadBytes"].as_u64().unwrap(), 60_000 + rig.manifest.comfy.assets["nvidia"].size);
        assert!(plan["modelsFree"].as_u64().unwrap() > 0 && plan["sameDrive"] == true);
        assert!(plan["problems"].as_array().unwrap().is_empty(), "{}", plan["problems"]);
        assert!(plan["folderProblems"].as_array().unwrap().is_empty() && plan["modelProblems"].as_array().unwrap().is_empty());
        // Nothing was written by asking.
        assert!(!rig.dir.path().join("engine").exists() && !rig.models_dir().exists());
        assert_eq!(rig.net.served.load(Ordering::SeqCst), 0);
        // A folder too deep for Windows is a problem named before Install is pressed.
        let deep = format!("{}{}", rig.dir.path().join("engine").to_string_lossy(), "\\deeper".repeat(30));
        let plan = rig.run("ai.setup.plan", json!({"models": [], "engineDir": deep})).unwrap();
        assert_eq!(plan["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("too long")), cfg!(windows));
        assert_eq!(plan["folderProblems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("too long")), cfg!(windows));
    }

    #[test]
    fn room_for_the_models_is_not_a_folder_problem() {
        use crate::ai_setup_cmds::native::{RoomProblems, room_problems};
        const GB: u64 = 1_000_000_000;
        // 40 GB free on one drive: the engine fits, the models picked so far don't. Only fewer
        // models are needed, so the Location page can go on to the Models page.
        let room = room_problems(true, (Some(40 * GB), 5 * GB), (Some(40 * GB), 36 * GB));
        assert!(room.folders.is_empty() && room.models.len() == 1, "{room:?}");
        assert!(room.models[0].contains("Choose fewer models"), "{room:?}");
        // With fewer models it all fits; with none, so does the engine alone.
        assert_eq!(room_problems(true, (Some(40 * GB), 5 * GB), (Some(40 * GB), 20 * GB)), RoomProblems::default());
        assert_eq!(room_problems(true, (Some(40 * GB), 5 * GB), (Some(40 * GB), 0)), RoomProblems::default());
        // The engine alone does not fit: another folder is the only fix, on either layout.
        let room = room_problems(true, (Some(GB), 5 * GB), (Some(GB), 0));
        assert!(room.folders.len() == 1 && room.models.is_empty() && room.folders[0].contains("engine"), "{room:?}");
        let room = room_problems(false, (Some(GB), 5 * GB), (Some(100 * GB), 36 * GB));
        assert!(room.folders.len() == 1 && room.models.is_empty(), "{room:?}");
        // Separate drives: the models' drive is short.
        let room = room_problems(false, (Some(100 * GB), 5 * GB), (Some(10 * GB), 36 * GB));
        assert!(room.folders.is_empty() && room.models.len() == 1 && room.models[0].contains("models folder"), "{room:?}");
        // Free space that can't be told is not a problem.
        assert_eq!(room_problems(false, (None, 5 * GB), (None, 36 * GB)), RoomProblems::default());
    }

    #[test]
    fn a_setup_runs_beside_the_session_and_leaves_its_settings_when_it_ends() {
        let mut rig = Rig::new();
        rig.net.hold.store(true, Ordering::SeqCst);
        let before = rig.s.prefs().integrations.ai.clone();
        let params = rig.existing(&["qwen-image-2-1"]);
        let started = rig.run("ai.setup.start", params.clone()).unwrap();
        assert_eq!((started["state"].as_str(), started["running"].as_bool()), (Some("running"), Some(true)));
        assert_eq!(started["progress"]["steps"].as_array().map(Vec::len), Some(5));
        // While it runs: the session is free, the settings are untouched, a second one is refused.
        assert!(rig.s.execute("ai.models", json!({})).is_ok());
        assert_eq!(rig.s.prefs().integrations.ai, before);
        assert!(rig.run("ai.setup.start", params).unwrap_err().to_string().contains("already running"));
        assert!(rig.run("ai.setup.remove", json!({"models": true})).unwrap_err().to_string().contains("running"));
        assert_eq!(rig.s.ai_setup.record().phase, Phase::Installing);
        // The user changes a setting meanwhile: it must survive the setup's own changes.
        rig.s.edit_prefs(|p| p.integrations.ai.timeout_seconds = 1234);
        rig.net.hold.store(false, Ordering::SeqCst);
        let ended = rig.finish();
        assert_eq!((ended["state"].as_str(), ended["seq"].as_u64()), (Some("done"), Some(1)));
        let result = &ended["result"];
        assert_eq!(
            (result["ok"].as_bool(), result["installed"][0].as_str(), result["testImage"]["model"].as_str()),
            (Some(true), Some("qwen-image-2-1"), Some("qwen-image-2-1"))
        );
        assert_eq!(ended["installedNow"][0], "qwen-image-2-1");
        // The settings: their server's address, the model ticked, the user's own change kept.
        let ai = &rig.s.prefs().integrations.ai;
        assert_eq!((ai.comfy_url.as_str(), ai.timeout_seconds), ("http://127.0.0.1:8000", 1234));
        assert!(ai.model("qwen-image-2-1").is_some_and(|m| m.enabled));
        let last = rig.s.ai_setup.last().unwrap();
        assert!(last.ok && last.ready.contains(&"qwen-image-2-1".to_string()) && last.test.as_ref().is_some_and(|t| t.bytes == b"PNG"));
        for key in &rig.manifest.model("qwen-image-2-1").unwrap().files {
            let (folder, name) = photocraft_aisetup::manifest::split_key(key).unwrap();
            assert_eq!(std::fs::read(rig.models_dir().join(folder).join(name)).unwrap(), contents(key));
        }
        // Asked again, the result is the same one: it is applied once.
        assert_eq!(rig.run("ai.setup.status", json!({})).unwrap()["seq"], 1);
        // The manifest now says the model is there; adding another downloads only its files.
        let m = rig.run("ai.setup.manifest", json!({})).unwrap();
        assert_eq!(m["models"].as_array().unwrap().iter().map(|x| x["installed"].as_bool().unwrap()).collect::<Vec<_>>(), [true, false]);
        let served = rig.net.served.load(Ordering::SeqCst);
        let params = rig.existing(&["qwen-image-2-1", "z-image-turbo"]);
        rig.run("ai.setup.start", params).unwrap();
        assert_eq!(rig.finish()["result"]["installed"].as_array().map(Vec::len), Some(2));
        assert_eq!(rig.net.served.load(Ordering::SeqCst) - served, 60_000);
    }

    #[test]
    fn stopping_a_setup_keeps_it_resumable() {
        let mut rig = Rig::new();
        rig.net.hold.store(true, Ordering::SeqCst);
        let params = rig.existing(&["qwen-image-2-1"]);
        rig.run("ai.setup.start", params.clone()).unwrap();
        assert_eq!(rig.run("ai.setup.cancel", json!({})).unwrap()["stopping"], true);
        let ended = rig.finish();
        assert_eq!(
            (ended["state"].as_str(), ended["result"]["cancelled"].as_bool(), ended["result"]["ok"].as_bool()),
            (Some("paused"), Some(true), Some(false))
        );
        // Nothing was applied to the settings by a setup that did not finish.
        assert_eq!(rig.s.prefs().integrations.ai.comfy_url, photocraft_aigen::Settings::default().comfy_url);
        // Continued without repeating the choices: the record remembers them.
        rig.net.hold.store(false, Ordering::SeqCst);
        rig.run("ai.setup.start", json!({"url": "http://127.0.0.1:8000"})).unwrap();
        let ended = rig.finish();
        assert_eq!((ended["state"].as_str(), ended["result"]["ok"].as_bool()), (Some("done"), Some(true)));
        assert_eq!(ended["record"]["models"][0], "qwen-image-2-1");
        // A record left as "installing" by an app that was closed reads as interrupted.
        let path = rig.s.ai_setup.env.as_ref().unwrap().state_file.clone();
        state::save(&path, &SetupState { phase: Phase::Installing, ..state::load(&path) }).unwrap();
        assert_eq!(rig.run("ai.setup.status", json!({})).unwrap()["state"], "interrupted");
    }

    /// Continue Setup sends no choices: the engine runs on the processor when the setup being
    /// continued did, and as asked when it is asked.
    #[test]
    fn continuing_runs_the_engine_where_the_last_setup_did() {
        let mut rig = Rig::new();
        fake_engine(&rig);
        let path = rig.s.ai_setup.env.as_ref().unwrap().state_file.clone();
        let record = state::load(&path);
        assert_eq!(rig.run("ai.setup.plan", json!({})).unwrap()["cpu"], false);
        state::save(&path, &SetupState { choices: Choices { cpu: true, ..record.choices.clone() }, ..record }).unwrap();
        assert_eq!(rig.run("ai.setup.plan", json!({})).unwrap()["cpu"], true);
        assert_eq!(rig.run("ai.setup.plan", json!({"cpu": false})).unwrap()["cpu"], false);
        // Someone's own ComfyUI is never started on the processor by the setup.
        let existing = rig.existing(&["qwen-image-2-1"]);
        assert_eq!(rig.run("ai.setup.plan", existing).unwrap()["cpu"], false);
    }

    /// An engine as the setup leaves one, without running a setup.
    fn fake_engine(rig: &Rig) -> PathBuf {
        let root = rig.dir.path().join("engine").join("0.39.0");
        std::fs::create_dir_all(root.join("python_embeded")).unwrap();
        std::fs::write(root.join("python_embeded").join("python.exe"), vec![0u8; 5000]).unwrap();
        runtime::write_marker(&root, &runtime::Marker { version: "0.39.0".into(), variant: "nvidia".into(), sha256: "ab".into() }).unwrap();
        let record = SetupState {
            phase: Phase::Done,
            choices: Choices {
                route: Route::Managed,
                models: vec!["qwen-image-2-1".into(), "z-image-turbo".into()],
                engine_dir: rig.dir.path().join("engine").to_string_lossy().into_owned(),
                models_dir: rig.models_dir().to_string_lossy().into_owned(),
                variant: "nvidia".into(),
                ..Choices::default()
            },
            port: 8190,
            ..SetupState::default()
        };
        state::save(&rig.s.ai_setup.env.as_ref().unwrap().state_file, &record).unwrap();
        for key in rig.manifest.files.keys() {
            let (folder, name) = photocraft_aisetup::manifest::split_key(key).unwrap();
            std::fs::create_dir_all(rig.models_dir().join(folder)).unwrap();
            std::fs::write(rig.models_dir().join(folder).join(name), contents(key)).unwrap();
        }
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn removing_deletes_only_what_the_setup_installed_and_says_what_it_freed() {
        let mut rig = Rig::new();
        let root = fake_engine(&rig);
        let mine = rig.models_dir().join("vae").join("my_own.safetensors");
        write(&mine, b"mine");
        let notes = rig.dir.path().join("engine").join("notes.txt");
        write(&notes, b"mine too");
        rig.s.edit_prefs(|p| {
            let ai = &mut p.integrations.ai;
            (ai.comfy_autostart, ai.comfy_url) = (true, "http://127.0.0.1:8190".into());
            ai.comfy_launch.python = root.join("python_embeded").join("python.exe").to_string_lossy().into_owned();
        });
        let status = rig.run("ai.setup.status", json!({})).unwrap();
        assert_eq!((status["engines"][0]["version"].as_str(), status["installedNow"].as_array().map(Vec::len)), (Some("0.39.0"), Some(2)));
        // What its models folder has, for telling what is installed while the engine is off.
        assert!(status["modelFiles"]["vae"].as_array().unwrap().iter().any(|f| f == "my_own.safetensors"), "{}", status["modelFiles"]);
        // One model: its own files go, the engine and the other model stay.
        let removed = rig.run("ai.setup.remove", json!({"models": ["z-image-turbo"]})).unwrap();
        assert_eq!(
            (removed["freed"].as_u64(), removed["installed"][0].as_str(), removed["engine"].as_bool()),
            (Some(60_000), Some("qwen-image-2-1"), Some(false))
        );
        assert!(root.join("python_embeded").join("python.exe").is_file());
        let ai = &rig.s.prefs().integrations.ai;
        assert!(!ai.model("z-image-turbo").unwrap().enabled && ai.model("qwen-image-2-1").unwrap().enabled && ai.comfy_autostart);
        // Everything: the engine and the rest of the models. What the user put there stays.
        let removed = rig.run("ai.setup.remove", json!({"engine": true, "models": true})).unwrap();
        assert!(removed["freed"].as_u64().unwrap() >= 65_000 && removed["freedText"].as_str().unwrap().contains("KB"), "{removed}");
        assert!(!root.exists() && mine.is_file() && notes.is_file());
        let ai = &rig.s.prefs().integrations.ai;
        assert!(!ai.comfy_autostart && ai.comfy_launch.python.is_empty() && !ai.model("qwen-image-2-1").unwrap().enabled);
        let status = rig.run("ai.setup.status", json!({})).unwrap();
        assert_eq!(
            (status["state"].as_str(), status["engines"].as_array().map(Vec::len), status["installedNow"].as_array().map(Vec::len)),
            (Some("none"), Some(0), Some(0))
        );
        assert_eq!(rig.s.ai_setup.seq(), 2, "the shell is told to save the settings");
    }

    #[test]
    fn the_hugging_face_token_is_kept_with_the_keys_and_out_of_the_preferences() {
        let mut rig = Rig::new();
        // A token in the environment is never used unasked.
        assert_eq!(rig.run("ai.setup.status", json!({})).unwrap()["token"], false);
        assert_eq!(rig.run("ai.setup.setToken", json!({"token": " hf_secret123 "})).unwrap()["set"], true);
        assert_eq!(rig.s.ai.hf_token(), "hf_secret123");
        assert_eq!(rig.run("ai.setup.status", json!({})).unwrap()["token"], true);
        assert_eq!(rig.run("ai.setup.plan", json!({"models": []})).unwrap()["token"], true);
        // It travels with the API keys, and nowhere an agent can read.
        let keys = rig.s.ai.keys_json();
        assert!(keys.contains("hf_secret123"));
        for readable in [rig.s.prefs_to_json(), rig.run("ai.setup.status", json!({})).unwrap().to_string(), rig.run("ai.keys", json!({})).unwrap().to_string()]
        {
            assert!(!readable.contains("hf_secret123"));
        }
        assert!(!format!("{:?}", rig.s.ai).contains("hf_secret123"));
        let mut other = Session::new();
        other.ai.load_keys_json(&keys);
        assert_eq!(other.ai.hf_token(), "hf_secret123");
        assert_eq!(rig.run("ai.setup.setToken", json!({"token": ""})).unwrap()["set"], false);
        assert!(!rig.s.ai.keys_json().contains("hf_secret123"));
    }

    /// The bundled manifest's FLUX.3 Klein (gated) and Qwen Image 2.1, their files small.
    fn gated_manifest() -> Manifest {
        let mut m = Manifest::bundled().unwrap().clone();
        m.models.retain(|x| ["qwen-image-2-1", "flux-3-klein"].contains(&x.id.as_str()));
        let used: Vec<String> = m.models.iter().flat_map(|x| x.files.clone()).collect();
        m.files.retain(|key, _| used.contains(key));
        for (key, file) in &mut m.files {
            let bytes = contents(key);
            (file.size, file.sha256) = (bytes.len() as u64, sha(&bytes));
        }
        m.presets.retain(|name, _| name == "starter");
        m
    }

    /// Hugging Face's sign-in endpoints beside the ComfyUI of [`FakeComfy`].
    struct HubAndComfy {
        token_answer: std::sync::Mutex<String>,
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl Transport for HubAndComfy {
        fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
            self.seen.lock().unwrap().push(r.url.clone());
            let answer = |body: &str| Ok(Response { status: if body.is_empty() { 400 } else { 200 }, body: body.as_bytes().to_vec() });
            match r.url.as_str() {
                photocraft_aisetup::hf::TOKEN_URL => answer(&self.token_answer.lock().unwrap()),
                photocraft_aisetup::hf::USERINFO_URL => answer(r#"{"preferred_username":"pixel-painter"}"#),
                _ => FakeComfy.send(r),
            }
        }
    }

    /// Sign in as the browser does: Sign In, then Hugging Face sending the browser back to the
    /// app's callback with a code; returns once the sign-in has ended.
    fn sign_in_through_the_browser(rig: &mut Rig) {
        let started = rig.run("ai.setup.hfSignIn", json!({"product": "LayerShop"})).unwrap();
        let url = started["url"].as_str().unwrap().to_string();
        assert!(rig.s.ai_setup.signing_in());
        assert_eq!(rig.run("ai.setup.hfSignIn", json!({})).unwrap()["started"], false, "one at a time");
        let port: u16 = url.split("127.0.0.1%3A").nth(1).and_then(|r| r.split("%2F").next()).unwrap().parse().unwrap();
        let state = url.split("state=").nth(1).unwrap().split('&').next().unwrap().to_string();
        let mut browser = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::io::Write::write_all(&mut browser, format!("GET /callback?code=c0de&state={state} HTTP/1.1\r\n\r\n").as_bytes()).unwrap();
        let seq = rig.s.ai_setup.hf_seq();
        let began = Instant::now();
        while rig.s.ai_setup.hf_seq() == seq {
            poll(&mut rig.s);
            assert!(began.elapsed() < Duration::from_secs(10), "the sign-in did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// No command answer, preference or debug line shows any of `secrets`.
    fn assert_no_token_shown(rig: &mut Rig, models: &Value, secrets: &[&str]) {
        for readable in [
            rig.s.prefs_to_json(),
            rig.run("ai.setup.status", json!({})).unwrap().to_string(),
            rig.run("ai.setup.hfStatus", json!({})).unwrap().to_string(),
            rig.run("ai.setup.hfAccess", models.clone()).unwrap().to_string(),
            format!("{:?}", rig.s.ai),
        ] {
            for secret in secrets {
                assert!(!readable.contains(secret), "{secret} in {readable}");
            }
        }
    }

    /// A setup whose FLUX.3 Klein is gated (open to `hf_oauth_first`), with Hugging Face answering
    /// a sign-in with that token; not yet able to sign in (no app id).
    fn gated_rig() -> (Rig, Arc<FakeNet>, Arc<HubAndComfy>) {
        let mut rig = Rig::new();
        let manifest = Arc::new(gated_manifest());
        let klein_url = manifest.files["diffusion_models/flux3_image_klein_base_int8_convrot.safetensors"].url.clone();
        let files = manifest.files.iter().map(|(key, f)| (f.url.clone(), contents(key))).collect();
        let net = Arc::new(FakeNet { files, ..FakeNet::default() });
        net.gated.lock().unwrap().insert(klein_url, "hf_oauth_first".into());
        let hub = Arc::new(HubAndComfy {
            token_answer: std::sync::Mutex::new(r#"{"access_token":"hf_oauth_first","expires_in":28800,"refresh_token":"hf_refresh_1"}"#.into()),
            seen: std::sync::Mutex::new(Vec::new()),
        });
        rig.s.ai.transport = Some(hub.clone());
        if let Some(env) = rig.s.ai_setup.env.as_mut() {
            (env.manifest, env.fetch) = (Some(manifest), net.clone());
        }
        (rig, net, hub)
    }

    #[test]
    fn signing_in_with_hugging_face_unlocks_the_gated_model_and_signing_out_forgets_it() {
        use photocraft_aisetup::hf;
        let (mut rig, net, hub) = gated_rig();
        // Without an app to sign in as, Sign in is unavailable and says what to do instead.
        let e = rig.run("ai.setup.hfSignIn", json!({"product": "LayerShop"})).unwrap_err().to_string();
        assert!(e.contains("access token"), "{e}");
        assert_eq!(rig.run("ai.setup.hfStatus", json!({})).unwrap()["canSignIn"], false);
        if let Some(env) = rig.s.ai_setup.env.as_mut() {
            env.hf_client_id = "test-client-id".into();
        }
        let models = json!({"models": ["qwen-image-2-1", "flux-3-klein"]});
        // Not signed in: the plan says Klein needs it, and access says sign in.
        let plan = rig.run("ai.setup.plan", models.clone()).unwrap();
        assert_eq!(plan["needsSignIn"][0]["id"], "flux-3-klein");
        assert!(plan["needsSignIn"][0]["page"].as_str().unwrap().starts_with("https://huggingface.co/"));
        let access = rig.run("ai.setup.hfAccess", models.clone()).unwrap();
        assert_eq!((access["models"][0]["access"].as_str(), access["signedIn"].as_bool()), (Some("signIn"), Some(false)));
        let status = rig.run("ai.setup.hfStatus", json!({})).unwrap();
        assert_eq!((status["signedIn"].as_bool(), status["signingIn"].as_bool()), (Some(false), Some(false)));
        assert_eq!(status["links"]["help"], "https://layershop.aardvarkstudios.net/hugging-face/");
        assert_eq!(rig.run("ai.setup.hfStatus", json!({})).unwrap()["canSignIn"], true);
        sign_in_through_the_browser(&mut rig);
        assert!(rig.s.ai.take_keys_dirty(), "the app is told to save the keys");
        let status = rig.run("ai.setup.hfStatus", json!({})).unwrap();
        assert_eq!((status["signedIn"].as_bool(), status["user"].as_str(), status["via"].as_str()), (Some(true), Some("pixel-painter"), Some("oauth")));
        // Signed in, the licence not accepted with this account: 403.
        net.gated.lock().unwrap().values_mut().for_each(|t| *t = "someone-elses".into());
        assert_eq!(rig.run("ai.setup.hfAccess", models.clone()).unwrap()["models"][0]["access"], "licence");
        // Accepted: ready, and the plan includes it.
        net.gated.lock().unwrap().values_mut().for_each(|t| *t = "hf_oauth_first".into());
        assert_eq!(rig.run("ai.setup.hfAccess", models.clone()).unwrap()["models"][0]["access"], "ready");
        assert_eq!(rig.run("ai.setup.plan", models.clone()).unwrap()["models"][1]["skipped"], Value::Null);
        // Hours later, about to expire: the check renews it first, and the new one opens the file.
        let mut c = rig.s.ai.hf();
        c.expires_at = hf::unix_now() + 60;
        rig.s.ai.set_hf(&c);
        *hub.token_answer.lock().unwrap() = r#"{"access_token":"hf_oauth_fresh","expires_in":28800,"refresh_token":"hf_refresh_2"}"#.into();
        net.gated.lock().unwrap().values_mut().for_each(|t| *t = "hf_oauth_fresh".into());
        let _ = rig.s.ai.take_keys_dirty();
        assert_eq!(rig.run("ai.setup.hfAccess", models.clone()).unwrap()["models"][0]["access"], "ready");
        let renewed = rig.s.ai.hf();
        assert_eq!((renewed.access.as_str(), renewed.refresh.as_str(), renewed.user.as_str()), ("hf_oauth_fresh", "hf_refresh_2", "pixel-painter"));
        assert!(rig.s.ai.take_keys_dirty());
        // No command, preference or debug line carries a token; only Hugging Face was sent one.
        assert_no_token_shown(&mut rig, &models, &["hf_oauth_fresh", "hf_refresh_2", "hf_oauth_first", "hf_refresh_1"]);
        assert!(hub.seen.lock().unwrap().iter().all(|u| u.starts_with("https://huggingface.co/")));
        // The keys file keeps all of it, and loads back.
        let mut other = Session::new();
        other.ai.load_keys_json(&rig.s.ai.keys_json());
        assert_eq!(other.ai.hf(), renewed);
        // Signing out forgets it here.
        let out = rig.run("ai.setup.hfSignOut", json!({})).unwrap();
        assert_eq!(out["connectedApps"], "https://huggingface.co/settings/connected-applications");
        assert!(!rig.s.ai.hf().signed_in() && !rig.s.ai.keys_json().contains("hf_oauth_fresh") && rig.s.ai.take_keys_dirty());
        assert_eq!(rig.run("ai.setup.hfAccess", models).unwrap()["models"][0]["access"], "signIn");
    }

    #[test]
    fn a_sign_in_that_runs_out_or_is_refused_is_forgotten_and_says_why() {
        use photocraft_aisetup::hf::Credentials;
        let (mut rig, net, hub) = gated_rig();
        if let Some(env) = rig.s.ai_setup.env.as_mut() {
            env.hf_client_id = "test-client-id".into();
        }
        sign_in_through_the_browser(&mut rig);
        let renewed = rig.s.ai.hf();
        assert!(renewed.signed_in());
        // A sign-in Hugging Face won't renew any more is signed out, and says why.
        rig.s.ai.set_hf(&Credentials { expires_at: 1, ..renewed.clone() });
        hub.token_answer.lock().unwrap().clear();
        let access = rig.run("ai.setup.hfAccess", json!({"models": ["flux-3-klein"]})).unwrap();
        assert_eq!(access["models"][0]["access"], "signIn");
        assert!(access["note"].as_str().unwrap().contains("Sign in again") && !rig.s.ai.hf().signed_in(), "{access}");
        // Run out with no refresh token to renew it: the status says so before anything is asked,
        // and the check forgets it without sending it.
        rig.s.ai.set_hf(&Credentials { expires_at: 1, refresh: String::new(), ..renewed.clone() });
        let status = rig.run("ai.setup.hfStatus", json!({})).unwrap();
        assert_eq!((status["signedIn"].as_bool(), status["expired"].as_bool()), (Some(false), Some(true)));
        let asked = hub.seen.lock().unwrap().len();
        let access = rig.run("ai.setup.hfAccess", json!({"models": ["flux-3-klein"]})).unwrap();
        assert!(access["note"].as_str().unwrap().contains("expired") && !rig.s.ai.hf().signed_in(), "{access}");
        assert_eq!(hub.seen.lock().unwrap().len(), asked, "an expired token is not sent");
        // A sign-in Hugging Face refuses (the app disconnected in Connected apps, say) is not shown
        // as signed in any more: forgotten, and the note says why.
        rig.s.ai.set_hf(&Credentials { access: "revoked-token".into(), expires_at: 0, ..renewed.clone() });
        let access = rig.run("ai.setup.hfAccess", json!({"models": ["flux-3-klein"]})).unwrap();
        assert_eq!((access["models"][0]["access"].as_str(), access["signedIn"].as_bool()), (Some("signIn"), Some(false)));
        assert!(access["note"].as_str().unwrap().contains("no longer accepts") && !rig.s.ai.hf().signed_in(), "{access}");
        assert_eq!(rig.run("ai.setup.hfStatus", json!({})).unwrap()["signedIn"], false);
        // A file its maker moved: that model is unavailable and says why; the sign-in stays.
        net.gated.lock().unwrap().values_mut().for_each(|t| *t = "moved".into());
        rig.s.ai.set_hf(&Credentials { expires_at: 0, ..renewed });
        let access = rig.run("ai.setup.hfAccess", json!({"models": ["flux-3-klein"]})).unwrap();
        assert_eq!((access["models"][0]["access"].as_str(), access["signedIn"].as_bool()), (Some("unavailable"), Some(true)), "{access}");
        assert!(access["models"][0]["detail"].as_str().unwrap().contains("newer version"), "{access}");
        assert!(rig.s.ai.hf().signed_in());
    }

    #[test]
    fn a_token_from_an_older_keys_file_counts_as_pasted() {
        let mut s = Session::new();
        s.ai.load_keys_json(r#"{"openai":"sk-x","huggingface":"hf_old","huggingfaceVia":"bogus","huggingfaceExpires":"soon","x":"\u0007"}"#);
        let c = s.ai.hf();
        assert_eq!((c.access.as_str(), c.via, c.expires_at, c.refresh.as_str()), ("hf_old", photocraft_aisetup::hf::Via::Token, 0, ""));
        assert!(!s.ai.take_keys_dirty(), "loading is not a change to save");
    }

    #[test]
    fn the_wizard_is_offered_once() {
        let rig = Rig::new();
        assert!(!rig.s.ai_setup.record().offered);
        rig.s.ai_setup.mark_offered();
        rig.s.ai_setup.mark_offered();
        assert!(rig.s.ai_setup.record().offered);
        // Without the app there is no record to mark.
        Session::new().ai_setup.mark_offered();
    }
}

#[test]
fn every_setup_command_is_registered_with_its_parameters_documented() {
    let ids: Vec<&str> = specs().iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        [
            "ai.setup.probe",
            "ai.setup.manifest",
            "ai.setup.plan",
            "ai.setup.status",
            "ai.setup.start",
            "ai.setup.cancel",
            "ai.setup.repair",
            "ai.setup.remove",
            "ai.setup.setToken",
            "ai.setup.hfStatus",
            "ai.setup.hfAccess",
            "ai.setup.hfSignIn",
            "ai.setup.hfSignOut",
            "ai.setup.hfUseFound"
        ]
    );
    for spec in specs() {
        assert!(crate::commands::find(spec.id).is_some(), "{}", spec.id);
        assert!(spec.params.contains('→') && spec.menu.is_empty() && !spec.journal, "{}", spec.id);
    }
}
