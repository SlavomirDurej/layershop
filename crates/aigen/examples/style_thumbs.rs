//! Render the style presets' thumbnails (`assets/ai-styles`), one subject throughout so the
//! treatments compare, with each preset's sentence added as `ai.generate` adds it:
//!
//! ```sh
//! GEMINI_API_KEY=… cargo run -p photocraft-aigen --example style_thumbs -- out/            # every preset
//! GEMINI_API_KEY=… cargo run -p photocraft-aigen --example style_thumbs -- out/ anime,collage
//! ```
//!
//! Each is written as `<out>/<id>.png` (a square 1K image from Nano Banana Pro); one that is
//! there already is skipped, so a run that failed half way is run again. Four render at a time.
//! The shipped thumbnails are these, scaled to 512 px and saved as JPEG.

use std::path::PathBuf;
use std::sync::Mutex;

use photocraft_aigen::style::{STYLES, Style, styled};
use photocraft_aigen::{Control, Job, Model, Provider, Settings, UreqTransport, generate, random_seed};

/// The subject every thumbnail shows.
const SUBJECT: &str = "A young woman in a mustard-yellow jacket holding a red ceramic mug, beside a leafy green potted plant, in front of a simple teal café wall. \
Waist-up, three-quarter view, centred, one clear focal point, square composition. No text, no lettering, no watermark, no border, a single image.";

/// How many render at once.
const AT_ONCE: usize = 4;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().ok_or("usage: style_thumbs <out dir> [id,id,…]")?);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let only: Vec<&str> = args.get(1).map(|a| a.split(',').map(str::trim).collect()).unwrap_or_default();
    let key = std::env::var("GEMINI_API_KEY").map_err(|_| "set GEMINI_API_KEY")?;
    let model = Model::cloud(Provider::Gemini, "Nano Banana Pro", "gemini-3-pro-image-preview", false);
    let settings = Settings::default();
    let todo: Vec<&Style> = STYLES.iter().filter(|s| only.is_empty() || only.contains(&s.id)).filter(|s| !out.join(format!("{}.png", s.id)).exists()).collect();
    println!("{} to render", todo.len());
    let queue = Mutex::new(todo.into_iter());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..AT_ONCE {
            scope.spawn(|| {
                let transport = UreqTransport::new();
                loop {
                    let Some(style) = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).next() else { break };
                    let job = Job { prompt: styled(SUBJECT, Some(style)), width: 1024, height: 1024, seed: random_seed(), alpha: false, source: None, loras: Vec::new() };
                    let control = Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|d| std::thread::sleep(d) };
                    let started = std::time::Instant::now();
                    let path = out.join(format!("{}.png", style.id));
                    match generate(&transport, &settings, &model, &key, &job, &control).and_then(|image| {
                        std::fs::write(&path, &image.bytes).map_err(|e| photocraft_aigen::Error::Config(format!("{}: {e}", path.display())))
                    }) {
                        Ok(()) => println!("{} ({:.0} s)", style.id, started.elapsed().as_secs_f32()),
                        Err(e) => {
                            println!("{}: error: {e}", style.id);
                            failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(style.id);
                        }
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    if failed.is_empty() { Ok(()) } else { Err(format!("failed: {}", failed.join(","))) }
}
