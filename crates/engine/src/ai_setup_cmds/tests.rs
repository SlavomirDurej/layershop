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

    const COMMANDS: [&str; 9] = [
        "ai.setup.probe",
        "ai.setup.manifest",
        "ai.setup.plan",
        "ai.setup.status",
        "ai.setup.start",
        "ai.setup.cancel",
        "ai.setup.repair",
        "ai.setup.remove",
        "ai.setup.setToken",
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
    }

    impl Fetch for FakeNet {
        fn head(&self, url: &str, _: Option<&str>) -> photocraft_aisetup::Result<Head> {
            Ok(Head { status: 200, size: self.files.get(url).map(|f| f.len() as u64), sha256: None })
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
        assert_eq!(m["comfy"]["version"], "0.39.0");
        assert_eq!(m["models"].as_array().unwrap().len(), 8);
        assert_eq!(
            (m["presets"]["starter"]["bytes"].as_u64(), m["presets"]["recommended"]["models"].as_array().map(Vec::len)),
            (Some(18_520_930_264), Some(3))
        );
        let qwen = &m["models"][0];
        assert_eq!(
            (qwen["id"].as_str(), qwen["installed"].as_bool(), qwen["fit"].as_str(), qwen["licence"]["commercial"].as_bool()),
            (Some("qwen-image-2-1"), Some(false), Some("tested"), Some(false))
        );
        // A smaller card than any it has run on: not tested, and it says so instead of guessing.
        assert_eq!(s.execute("ai.setup.manifest", json!({"vramGb": 8})).unwrap()["models"][0]["fit"], "untested");
        assert_eq!(s.execute("ai.setup.manifest", json!({})).unwrap()["models"][0]["fit"], "untested");
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
        assert_eq!(rows, ["os", "gpu", "disk", "comfy", "net"]);
        assert_eq!(seen["defaults"]["modelsDir"], rig.models_dir().to_string_lossy().as_ref());
        assert!(seen["variant"].is_string() && seen["canInstall"].is_boolean());
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
        // Nothing was written by asking.
        assert!(!rig.dir.path().join("engine").exists() && !rig.models_dir().exists());
        assert_eq!(rig.net.served.load(Ordering::SeqCst), 0);
        // A folder too deep for Windows is a problem named before Install is pressed.
        let deep = format!("{}{}", rig.dir.path().join("engine").to_string_lossy(), "\\deeper".repeat(30));
        let plan = rig.run("ai.setup.plan", json!({"models": [], "engineDir": deep})).unwrap();
        assert_eq!(plan["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("too long")), cfg!(windows));
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
        assert_eq!(rig.run("ai.setup.status", json!({})).unwrap()["token"], std::env::var("HF_TOKEN").is_ok_and(|t| !t.trim().is_empty()));
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
            "ai.setup.setToken"
        ]
    );
    for spec in specs() {
        assert!(crate::commands::find(spec.id).is_some(), "{}", spec.id);
        assert!(spec.params.contains('→') && spec.menu.is_empty() && !spec.journal, "{}", spec.id);
    }
}
