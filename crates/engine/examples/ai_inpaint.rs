//! Inpaint through the engine against a running ComfyUI server, without the app: the same
//! `ai.generate {"mode": "inpaint"}` the AI Gen panel runs, so what the model is sent (the crop,
//! the faded mask, the workflow) is what the app sends.
//!
//! ```sh
//! cargo run -p photocraft-engine --example ai_inpaint -- http://127.0.0.1:8188                 # list detected models
//! cargo run -p photocraft-engine --example ai_inpaint -- http://127.0.0.1:8188 2 photo.png 292,291,584,489 "a deer eating grass" out.png [context] [strength]
//! ```
//!
//! The rectangle is the selection (`x,y,width,height`); `out.png` is the flattened result.

use std::sync::Arc;

use photocraft_aigen::{UreqTransport, comfy_models, detect::detect};
use photocraft_engine::Session;
use serde_json::json;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let url = args.first().cloned().unwrap_or_else(|| "http://127.0.0.1:8188".into());
    let transport = Arc::new(UreqTransport::new());
    let folders = comfy_models(transport.as_ref(), &url).map_err(|e| e.to_string())?;
    let models = detect(&folders);
    let Some(pick) = args.get(1) else {
        for (i, m) in models.iter().enumerate() {
            println!("{i:>2}  {:<14} {:<44} inpaints: {}", m.family.label(), m.name, m.can_inpaint());
        }
        return Ok(());
    };
    let index: usize = pick.parse().map_err(|_| format!("`{pick}` is not a model number"))?;
    let mut model = models.get(index).cloned().ok_or_else(|| format!("no model {index} (there are {})", models.len()))?;
    model.id = "model".into();
    // Whatever its architecture does unasked: this is for trying it.
    model.inpaint = Some(true);
    let arg = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| format!("missing {what}"));
    let (image, rect, prompt, out) = (arg(2, "the image")?, arg(3, "the selection x,y,width,height")?, arg(4, "the prompt")?, arg(5, "the output file")?);
    let rect: Vec<i64> = rect.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    let [x, y, width, height] = rect[..] else { return Err("the selection must be x,y,width,height".into()) };
    let number = |i: usize, default: f64| args.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default);

    let mut s = Session::new();
    s.ai.transport = Some(transport);
    let run = |s: &mut Session, id: &str, params| s.execute(id, params).map_err(|e| format!("{id}: {e}"));
    run(&mut s, "prefs.set", json!({"path": "integrations.ai.comfyUrl", "value": url}))?;
    run(&mut s, "prefs.set", json!({"path": "integrations.ai.models", "value": [serde_json::to_value(&model).map_err(|e| e.to_string())?]}))?;
    run(&mut s, "file.openAs", json!({"path": image}))?;
    run(&mut s, "select.rect", json!({"x": x, "y": y, "width": width, "height": height}))?;
    let mut params = json!({"prompt": prompt, "mode": "inpaint", "strength": number(7, 1.0)});
    if let Some(context) = args.get(6).and_then(|v| v.parse::<f64>().ok()) {
        params["context"] = json!(context);
    }
    let result = run(&mut s, "ai.generate", params)?;
    println!("{result}");
    let layer = s.active().and_then(|d| d.doc.layer(d.active_layer?)).map(|l| l.name.clone()).unwrap_or_default();
    println!("layer: {layer}");
    run(&mut s, "file.saveACopy", json!({"path": out, "layers": false}))?;
    println!("wrote {out}");
    Ok(())
}
