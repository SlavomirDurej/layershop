//! Try a local model against a running ComfyUI server, without the app:
//!
//! ```sh
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188            # list detected models
//! cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188 2 "a red frog" out.png 768 512
//! ```
//!
//! The second form renders the detected model with that number at the given selection size and
//! writes the image.

use std::time::Duration;

use photocraft_aigen::{Control, Job, Settings, UreqTransport, comfy_models, detect::detect, generate, random_seed};

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
    let job = Job { prompt, width: size(4), height: size(5), seed: random_seed(), alpha: false };
    let settings = Settings { comfy_url: url, ..Settings::default() };
    let control = Control { cancelled: &|| false, progress: &|f, m| eprintln!("{:>3.0}%  {m}", f * 100.0), sleep: &|d: Duration| std::thread::sleep(d) };
    let image = generate(&transport, &settings, model, "", &job, &control).map_err(|e| e.to_string())?;
    std::fs::write(&out, &image.bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{} ({} bytes, rendered {}x{}, seed {})", out, image.bytes.len(), image.width, image.height, image.seed);
    Ok(())
}
