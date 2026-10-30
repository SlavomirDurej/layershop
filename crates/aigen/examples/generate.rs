//! Try a local model against a running ComfyUI server, without the app:
//!
//! ```sh
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188            # list detected models
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 2 "a red frog" out.png 768 512
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 2 "a red frog" out.png 768 512 alpha   # transparent background
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 2 "a teapot" out.png inpaint image.png mask.png 0.8
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 2 "add a deer" out.png edit image.png [mask.png]
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 find         # how it was started
//! ```
//!
//! The second form renders the detected model with that number at the given selection size and
//! writes the image.

use std::time::Duration;

use photocraft_aigen::{Control, Job, Settings, Source, UreqTransport, comfy_models, detect::detect, generate, random_seed};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let url = args.first().cloned().unwrap_or_else(|| "http://127.0.0.1:8188".into());
    let transport = UreqTransport::new();
    // `<url> find`: how the server running there was started (what "Find Installation" fills in).
    if args.get(1).map(String::as_str) == Some("find") {
        let launch = photocraft_aigen::launch::find_running(&transport, &url).map_err(|e| e.to_string())?;
        println!("python: {}", launch.python);
        println!("folder: {}", launch.folder);
        println!("args:   {}", launch.args);
        return Ok(());
    }
    // `<url> installed [port]`: what Find Installation fills in while ComfyUI is closed; with a
    // port, also start it that way, wait for it, and stop it.
    if args.get(1).map(String::as_str) == Some("installed") {
        use photocraft_aigen::{ComfyProcess, LocalServer};
        let launch = photocraft_aigen::launch::find_installed().ok_or("no ComfyUI Desktop installation found")?;
        println!("python: {}", launch.python);
        println!("folder: {}", launch.folder);
        println!("args:   {}", launch.args);
        let Some(port) = args.get(2) else { return Ok(()) };
        let settings = Settings { comfy_url: format!("http://127.0.0.1:{port}"), comfy_autostart: true, comfy_launch: launch, ..Settings::default() };
        let log = std::env::temp_dir().join("photocraft-comfy-example.log");
        let server = ComfyProcess::new(Some(log));
        let control = Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|d: Duration| std::thread::sleep(d) };
        let begun = std::time::Instant::now();
        server.ensure(&transport, &settings, &control).map_err(|e| e.to_string())?;
        let files = comfy_models(&transport, &settings.comfy_url).map_err(|e| e.to_string())?;
        println!(
            "answering on port {port} after {:.1}s, {} diffusion models",
            begun.elapsed().as_secs_f32(),
            files.get("diffusion_models").map_or(0, Vec::len)
        );
        println!("stopped: {}", server.stop());
        return Ok(());
    }
    // `<url> start <port>`: start a second, hidden ComfyUI on <port> the way the one at <url> was
    // started, wait for it, then stop it. A check of the launcher that leaves <url> alone.
    if args.get(1).map(String::as_str) == Some("start") {
        use photocraft_aigen::{ComfyProcess, LocalServer};
        let port = args.get(2).cloned().unwrap_or_else(|| "8765".into());
        let launch = photocraft_aigen::launch::find_running(&transport, &url).map_err(|e| e.to_string())?;
        let settings = Settings { comfy_url: format!("http://127.0.0.1:{port}"), comfy_autostart: true, comfy_launch: launch, ..Settings::default() };
        let log = std::env::temp_dir().join("photocraft-comfy-example.log");
        let server = ComfyProcess::new(Some(log.clone()));
        let control = Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|d: Duration| std::thread::sleep(d) };
        let begun = std::time::Instant::now();
        server.ensure(&transport, &settings, &control).map_err(|e| e.to_string())?;
        println!("answering on port {port} after {:.1}s (state {:?}, log {})", begun.elapsed().as_secs_f32(), server.state(), log.display());
        let files = comfy_models(&transport, &settings.comfy_url).map_err(|e| e.to_string())?;
        println!("it lists {} diffusion models", files.get("diffusion_models").map_or(0, Vec::len));
        println!("stopped: {}", server.stop());
        std::thread::sleep(Duration::from_secs(2));
        println!("still answering afterwards: {}", photocraft_aigen::launch::is_running(&transport, &settings.comfy_url));
        return Ok(());
    }
    let folders = comfy_models(&transport, &url).map_err(|e| e.to_string())?;
    let models = detect(&folders);
    let Some(pick) = args.get(1) else {
        for (i, m) in models.iter().enumerate() {
            println!("{i:>2}  {:<12} {:<44} steps {:<3} cfg {}", m.family.label(), m.name, m.steps, m.cfg);
        }
        return Ok(());
    };
    let index: usize = pick.parse().map_err(|_| format!("`{pick}` is not a model number"))?;
    let model = models.get(index).ok_or_else(|| format!("no model {index} (there are {})", models.len()))?;
    let prompt = args.get(2).cloned().unwrap_or_else(|| "a red frog on a mossy stone, macro photo".into());
    let out = args.get(3).cloned().unwrap_or_else(|| "generated.png".into());
    let size = |i: usize| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(1024u32);
    // `<url> <n> <prompt> <out.png> inpaint <image.png> <mask.png> [strength]`: repaint the part
    // of the image that is transparent in the mask (both PNGs of one size, edges in multiples of 16).
    // `<url> <n> <prompt> <out.png> edit <image.png> [mask.png]`: change the picture as the prompt
    // says (the Edit tab's Change with an edit model), only where the mask is transparent if one
    // is given.
    let mode = args.get(4).map(String::as_str);
    let job = if mode == Some("inpaint") || mode == Some("edit") {
        let edit = mode == Some("edit");
        let read = |i: usize| {
            let path = args.get(i).ok_or("inpaint needs <image.png> <mask.png>, edit <image.png>")?;
            std::fs::read(path).map_err(|e| format!("{path}: {e}"))
        };
        let image = read(5)?;
        let mask = if edit && args.get(6).is_none() { Vec::new() } else { read(6)? };
        // A PNG's size is in its header: bytes 16..24.
        let edge = |at: usize| image.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes).ok_or("the image is not a PNG");
        let (width, height) = (edge(16)?, edge(20)?);
        let strength = args.get(7).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        Job {
            prompt,
            width,
            height,
            seed: random_seed(),
            alpha: false,
            source: Some(Source { image, mask, width, height, strength, edit, references: Vec::new(), shown: Vec::new() }),
        }
    } else {
        // A further `alpha` asks for a transparent background (the panel's Alpha Channel box).
        Job { prompt, width: size(4), height: size(5), seed: random_seed(), alpha: args.get(6).map(String::as_str) == Some("alpha"), source: None }
    };
    let settings = Settings { comfy_url: url, ..Settings::default() };
    let control = Control { cancelled: &|| false, progress: &|f, m| eprintln!("{:>3.0}%  {m}", f * 100.0), sleep: &|d: Duration| std::thread::sleep(d) };
    let image = generate(&transport, &settings, model, "", &job, &control).map_err(|e| e.to_string())?;
    std::fs::write(&out, &image.bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{} ({} bytes, rendered {}x{}, seed {})", out, image.bytes.len(), image.width, image.height, image.seed);
    Ok(())
}
