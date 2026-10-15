//! The Local AI setup through the engine, without the app: the same `ai.setup.*` commands the
//! wizard runs, against the real download hosts. It installs into a folder of your choosing and
//! leaves the app's own settings alone.
//!
//! ```sh
//! cargo run --release -p photocraft-engine --example ai_setup -- D:\ai-setup-test                 # what would be installed
//! cargo run --release -p photocraft-engine --example ai_setup -- D:\ai-setup-test install          # the engine alone (2 GB)
//! cargo run --release -p photocraft-engine --example ai_setup -- D:\ai-setup-test install qwen-image-2-1 z-image-turbo
//! cargo run --release -p photocraft-engine --example ai_setup -- D:\ai-setup-test remove           # delete the engine and the models again
//! ```
//!
//! The engine goes to `<folder>\engine`, the models to `<folder>\models`, the setup's record to
//! `<folder>\ai-setup.json`, the engine's output to `<folder>\comfyui.log` and the test image to
//! `<folder>\test.png`. Stopped with Ctrl+C, or killed, it continues from what arrived when it
//! is run again. `--variant nvidia_cu126|amd|intel` installs another build than the default
//! (`nvidia`); `--no-test` skips starting the engine and the test image. A Hugging Face token
//! for gated models is read from the `HF_TOKEN` environment variable.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use photocraft_aigen::{ComfyProcess, UreqTransport};
use photocraft_aisetup::fetch::UreqFetch;
use photocraft_aisetup::human_bytes;
use photocraft_engine::Session;
use photocraft_engine::ai_setup_cmds::SetupEnv;
use serde_json::{Value, json};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn number(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut flag = |name: &str, takes_value: bool| {
        let at = args.iter().position(|a| a == name)?;
        args.remove(at);
        Some(if takes_value && at < args.len() { args.remove(at) } else { String::new() })
    };
    let variant = flag("--variant", true).filter(|v| !v.is_empty()).unwrap_or_else(|| "nvidia".into());
    let test_image = flag("--no-test", false).is_none();
    let folder = args.first().cloned().ok_or("usage: ai_setup <folder> [install [model-id …] | remove] [--variant <build>] [--no-test]")?;
    let folder = Path::new(&folder);
    if !folder.is_absolute() {
        return Err(format!("`{}` must be a full path", folder.display()));
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;

    let mut s = Session::new();
    s.ai.transport = Some(Arc::new(UreqTransport::new()));
    s.ai.server = Some(Arc::new(ComfyProcess::new(Some(folder.join("comfyui.log")))));
    s.ai_setup.env = Some(SetupEnv {
        fetch: Arc::new(UreqFetch::default()),
        state_file: folder.join("ai-setup.json"),
        engine_dir: folder.join("engine").to_string_lossy().into_owned(),
        models_dir: folder.join("models").to_string_lossy().into_owned(),
        manifest: None,
    });
    let call = |s: &mut Session, id: &str, params: Value| s.execute(id, params).map_err(|e| e.to_string());

    let action = args.get(1).map(String::as_str).unwrap_or("plan");
    let models: Vec<&String> = args.iter().skip(2).collect();
    match action {
        "plan" => {
            let manifest = call(&mut s, "ai.setup.manifest", json!({}))?;
            println!("ComfyUI {}", text(&manifest, "/comfy/version"));
            for m in manifest["models"].as_array().into_iter().flatten() {
                let gated = if m["gated"] == true { "  (gated: needs HF_TOKEN)" } else { "" };
                println!("  {:<24} {:>9}  {}{gated}", text(m, "/id"), human_bytes(number(m, "/bytes")), text(m, "/licence/name"));
            }
            let plan = call(&mut s, "ai.setup.plan", json!({"route": "managed", "models": [], "variant": variant}))?;
            println!(
                "\nThe engine alone ({variant}): {} to download, {} on disk",
                human_bytes(number(&plan, "/downloadBytes")),
                human_bytes(number(&plan, "/engineBytes"))
            );
            for problem in plan["problems"].as_array().into_iter().flatten() {
                println!("  problem: {}", problem.as_str().unwrap_or(""));
            }
            println!("\n`install [model-id …]` installs into {}", folder.display());
            Ok(())
        }
        "remove" => {
            let removed = call(&mut s, "ai.setup.remove", json!({"engine": true, "models": true}))?;
            println!("removed; freed {}", text(&removed, "/freedText"));
            Ok(())
        }
        "install" => {
            let plan = call(&mut s, "ai.setup.plan", json!({"route": "managed", "models": models, "variant": variant}))?;
            println!(
                "to download: {}   on disk: engine {}, models {}",
                human_bytes(number(&plan, "/downloadBytes")),
                human_bytes(number(&plan, "/engineBytes")),
                human_bytes(number(&plan, "/modelsBytes"))
            );
            if let Some(problem) = plan["problems"].as_array().and_then(|p| p.first()).and_then(Value::as_str) {
                return Err(problem.to_string());
            }
            call(&mut s, "ai.setup.start", json!({"route": "managed", "models": models, "variant": variant, "testImage": test_image}))?;
            let began = Instant::now();
            let mut last = String::new();
            let status = loop {
                std::thread::sleep(Duration::from_secs(2));
                let status = call(&mut s, "ai.setup.status", json!({}))?;
                let p = &status["progress"];
                let active: Vec<String> = p["steps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|st| st["state"] == "active")
                    .map(|st| format!("{} {}", text(st, "/label"), text(st, "/note")).trim().to_string())
                    .collect();
                let unpack = p["steps"].as_array().into_iter().flatten().find(|st| st["key"] == "unpack" && st["state"] == "active");
                let unpacking =
                    unpack.map(|st| format!(" | unpacked {} / {}", human_bytes(number(st, "/done")), human_bytes(number(st, "/total")))).unwrap_or_default();
                let line = format!(
                    "{:>5.0} s  {} / {}  {}/s{unpacking}  [{}]",
                    began.elapsed().as_secs_f64(),
                    human_bytes(number(p, "/bytesDone")),
                    human_bytes(number(p, "/bytesTotal")),
                    human_bytes(p["speed"].as_f64().unwrap_or(0.0) as u64),
                    active.join("; ")
                );
                if line != last {
                    println!("{line}");
                    last = line;
                }
                if status["running"] == false {
                    break status;
                }
            };
            let result = &status["result"];
            for step in status["progress"]["steps"].as_array().into_iter().flatten() {
                println!("  {:<8} {} {}", text(step, "/state"), text(step, "/label"), text(step, "/note"));
            }
            if result["ok"] != true {
                return Err(format!("the setup stopped ({}): {}", text(&status, "/state"), text(result, "/error")));
            }
            println!("\ndone in {:.0} s", began.elapsed().as_secs_f64());
            println!("engine at {}", text(result, "/url"));
            println!("installed: {}", result["installed"]);
            println!("skipped: {}", result["skipped"]);
            let ai = &s.prefs().integrations.ai;
            println!("launch: {} | {} | {}", ai.comfy_launch.python, ai.comfy_launch.folder, ai.comfy_launch.args);
            match s.ai_setup.last().and_then(|f| f.test.as_ref()) {
                Some(test) => {
                    let out = folder.join("test.png");
                    std::fs::write(&out, &test.bytes).map_err(|e| e.to_string())?;
                    println!("test image: {} ({} x {}, {} in {:.1} s)", out.display(), test.width, test.height, test.model_name, test.seconds);
                }
                None => println!("no test image: {}", text(result, "/testError")),
            }
            // The engine this run started is stopped with it.
            if let Some(server) = &s.ai.server {
                server.stop();
            }
            Ok(())
        }
        other => Err(format!("`{other}` is not plan, install or remove")),
    }
}
