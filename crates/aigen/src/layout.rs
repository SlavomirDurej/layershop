//! Layout prompts: a scene and boxes placed on it ("regions"), written in the format of a model
//! trained to follow boxes. Each format is the text the model is given as its prompt, so the
//! back ends send it as they send any prompt (MOSS Image and FLUX.3 Klein through ComfyUI's text
//! encoder, FIBO 2 through its own prompt node, FLUX 3 Image through Black Forest Labs' API).
//!
//! Which models read which format: [`crate::Family::layout`] and [`crate::Model::layout`]. The
//! formats follow each vendor's documentation (October 2026):
//!
//! - **MOSS Image**: its authors' layout JSON, as ComfyUI's `MOSSImageLayoutPrompt` writes it:
//!   items labelled `object`, `character` or `text`, boxes `[top, left, bottom, right]` in pixels
//!   from the canvas's centre (`abs_centralized`, `yxyx`).
//! - **Ideogram 4**: its structured caption: `high_level_description` and
//!   `compositional_deconstruction` with `elements` of type `obj` or `text`, boxes
//!   `[top, left, bottom, right]` on a 0–1000 grid from the top left.
//! - **FLUX 3** (Image and Klein): the caption citing each box as `<obj_N>`, a space, then a JSON
//!   array of `{"id", "bbox", "desc"}` rows, boxes as Ideogram's. Text is quoted in `desc`.
//! - **FIBO 2**: Bria's VGL, `objects` and `text_render` with `bounding_box` `[x, y, width,
//!   height]`, normalised. Bria doesn't document the range or the origin: 0–1 from the top left
//!   is assumed here until a render confirms it.
//!
//! No format has a depth field. Every one is written back to front (the last region first), so
//! what is in front comes last, as the vendors advise for overlaps.

use serde_json::{Value, json};

/// The most regions one prompt carries. Models drop the later ones of long prompts (Ideogram's
/// documentation), and every box costs tokens.
pub const MAX_REGIONS: usize = 32;
/// The longest a region's caption or words may be, in characters; longer text is cut.
pub const MAX_REGION_TEXT: usize = 2000;
/// The smallest edge a box may have, as a fraction of the area: smaller ones are dropped (models
/// drop them anyway: FLUX 3 left out new elements in boxes of about 40 × 25 px).
const MIN_EDGE: f64 = 0.001;

/// The layout format a model reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutFormat {
    Moss,
    Ideogram,
    Flux3,
    Fibo,
}

impl LayoutFormat {
    pub const ALL: [LayoutFormat; 4] = [LayoutFormat::Moss, LayoutFormat::Ideogram, LayoutFormat::Flux3, LayoutFormat::Fibo];

    /// As `ai.models` and `ai.layoutPrompt` name it.
    pub fn key(self) -> &'static str {
        match self {
            LayoutFormat::Moss => "moss",
            LayoutFormat::Ideogram => "ideogram",
            LayoutFormat::Flux3 => "flux3",
            LayoutFormat::Fibo => "fibo",
        }
    }
}

/// What a region holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegionKind {
    #[default]
    Object,
    /// A person or creature. Formats without the label send it as an object.
    Character,
    /// Words to render, exactly as given ([`Region::text`]).
    Text,
}

/// One box of a layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub kind: RegionKind,
    /// What goes in the box (for text, how the words look).
    pub caption: String,
    /// Text regions: the words to render.
    pub text: String,
    /// `[left, top, right, bottom]` as fractions of the area generated, 0..=1 from its top left.
    pub bounds: [f64; 4],
}

/// A region ready to write: its text cut to size and its box inside the area, or `None` when it
/// has no area there.
fn settled(r: &Region) -> Option<(RegionKind, String, String, [f64; 4])> {
    let [l, t, rt, b] = r.bounds;
    if ![l, t, rt, b].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (l, rt) = (l.min(rt).clamp(0.0, 1.0), l.max(rt).clamp(0.0, 1.0));
    let (t, b) = (t.min(b).clamp(0.0, 1.0), t.max(b).clamp(0.0, 1.0));
    if rt - l < MIN_EDGE || b - t < MIN_EDGE {
        return None;
    }
    let cut = |s: &str| s.trim().chars().take(MAX_REGION_TEXT).collect::<String>();
    Some((r.kind, cut(&r.caption), cut(&r.text), [l, t, rt, b]))
}

/// A JSON object with its keys in the order given (the vendors' documentation order), on one line.
fn object(fields: &[(&str, Value)]) -> String {
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{}", Value::from(*k), v)).collect();
    format!("{{{}}}", body.join(","))
}

/// A 0–1000 grid position.
fn grid(f: f64) -> i64 {
    (f * 1000.0).round() as i64
}

/// `[top, left, bottom, right]` on the 0–1000 grid.
fn grid_box([l, t, r, b]: [f64; 4]) -> Value {
    json!([grid(t), grid(l), grid(b), grid(r)])
}

/// The prompt that tells a model reading `format` the `scene` and the `regions` (front to back,
/// [`MAX_REGIONS`] at most) for a `width` × `height` area. Without regions it is the scene as
/// written.
pub fn layout_prompt(format: LayoutFormat, scene: &str, regions: &[Region], width: u32, height: u32) -> String {
    let scene = scene.trim();
    // Back to front: what is in front is written last.
    let regions: Vec<_> = regions.iter().take(MAX_REGIONS).filter_map(settled).collect::<Vec<_>>().into_iter().rev().collect();
    if regions.is_empty() {
        return scene.to_string();
    }
    match format {
        LayoutFormat::Moss => {
            let (w, h) = (i64::from(width.max(1)), i64::from(height.max(1)));
            // Pixels from the centre: the same arithmetic as a typed prompt's box
            // ([`crate::comfy::moss_layout`]), so an edge at the canvas's edge lands on it.
            let px = |f: f64, size: i64| (f * size as f64).round() as i64 - size / 2;
            let items: Vec<String> = regions
                .iter()
                .map(|(kind, caption, text, [l, t, r, b])| {
                    let label = match kind {
                        RegionKind::Object => "object",
                        RegionKind::Character => "character",
                        RegionKind::Text => "text",
                    };
                    let mut fields = vec![("label", json!(label)), ("caption", json!(caption))];
                    if *kind == RegionKind::Text {
                        fields.push(("text", json!(text)));
                    }
                    fields.push(("bbox", json!([px(*t, h), px(*l, w), px(*b, h), px(*r, w)])));
                    object(&fields)
                })
                .collect();
            format!(
                "{{\"summary\":{},\"items\":[{}],\"bbox_coord_space\":\"abs_centralized\",\"bbox_format\":\"yxyx\"}}",
                object(&[("caption", json!(scene))]),
                items.join(",")
            )
        }
        LayoutFormat::Ideogram => {
            let elements: Vec<String> = regions
                .iter()
                .map(|(kind, caption, text, bounds)| {
                    let mut fields = vec![("type", json!(if *kind == RegionKind::Text { "text" } else { "obj" })), ("bbox", grid_box(*bounds))];
                    if *kind == RegionKind::Text {
                        fields.push(("text", json!(text)));
                    }
                    fields.push(("desc", json!(caption)));
                    object(&fields)
                })
                .collect();
            format!(
                "{{\"high_level_description\":{},\"compositional_deconstruction\":{{\"background\":\"\",\"elements\":[{}]}}}}",
                Value::from(scene),
                elements.join(",")
            )
        }
        LayoutFormat::Flux3 => {
            let rows: Vec<String> = regions
                .iter()
                .enumerate()
                .map(|(i, (kind, caption, text, bounds))| {
                    let desc = match (kind, caption.is_empty()) {
                        (RegionKind::Text, true) => format!("text reading \"{text}\""),
                        (RegionKind::Text, false) => format!("{caption}, text reading \"{text}\""),
                        _ => caption.clone(),
                    };
                    object(&[("id", json!(format!("obj_{}", i + 1))), ("bbox", grid_box(*bounds)), ("desc", json!(desc))])
                })
                .collect();
            let cited: Vec<String> = (1..=regions.len()).map(|i| format!("<obj_{i}>")).collect();
            let lead = scene.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
            let lead = if lead.is_empty() { "A picture".to_string() } else { lead.to_string() };
            format!("{lead}, with {}. [{}]", cited.join(", "), rows.join(","))
        }
        LayoutFormat::Fibo => {
            let bbox = |[l, t, r, b]: [f64; 4]| {
                let q = |v: f64| (v * 1000.0).round() / 1000.0;
                json!([q(l), q(t), q(r - l), q(b - t)])
            };
            let objects: Vec<String> = regions
                .iter()
                .filter(|(kind, ..)| *kind != RegionKind::Text)
                .map(|(_, caption, _, bounds)| object(&[("description", json!(caption)), ("bounding_box", bbox(*bounds))]))
                .collect();
            let texts: Vec<String> = regions
                .iter()
                .filter(|(kind, ..)| *kind == RegionKind::Text)
                .map(|(_, caption, text, bounds)| object(&[("text", json!(text)), ("appearance_details", json!(caption)), ("bounding_box", bbox(*bounds))]))
                .collect();
            format!(
                "{{\"short_description\":{},\"objects\":[{}],\"text_render\":[{}]}}",
                Value::from(scene),
                objects.join(","),
                texts.join(",")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The research report's example scene, boxes given there as `[top, left, bottom, right]` on
    /// 0–1000, listed front to back.
    fn scene() -> Vec<Region> {
        let r = |kind, caption: &str, text: &str, [t, l, b, rt]: [f64; 4]| Region {
            kind,
            caption: caption.into(),
            text: text.into(),
            bounds: [l / 1000.0, t / 1000.0, rt / 1000.0, b / 1000.0],
        };
        vec![
            r(RegionKind::Text, "Clean white sans-serif lettering", "Hello world", [60.0, 80.0, 170.0, 480.0]),
            r(RegionKind::Character, "Ginger cat curled up asleep on the cushion", "", [470.0, 320.0, 650.0, 610.0]),
            r(RegionKind::Object, "Green velvet armchair, worn armrests, a knitted throw", "", [380.0, 180.0, 990.0, 740.0]),
            r(RegionKind::Object, "Tall window, rain streaking the glass, grey dusk outside", "", [40.0, 550.0, 560.0, 960.0]),
        ]
    }

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn moss_boxes_are_pixels_from_the_centre_back_to_front() {
        let v = parse(&layout_prompt(LayoutFormat::Moss, "A cosy reading corner", &scene(), 1024, 1024));
        assert_eq!(v["summary"]["caption"], "A cosy reading corner");
        assert_eq!((&v["bbox_coord_space"], &v["bbox_format"]), (&json!("abs_centralized"), &json!("yxyx")));
        let items = v["items"].as_array().unwrap();
        // The report's numbers (v × 1.024 − 512), the window (at the back) first.
        assert_eq!(items[0]["bbox"], json!([-471, 51, 61, 471]));
        assert_eq!(items[1]["bbox"], json!([-123, -328, 502, 246]));
        assert_eq!(items[2]["bbox"], json!([-31, -184, 154, 113]));
        assert_eq!(items[3]["bbox"], json!([-451, -430, -338, -20]));
        assert_eq!(items.iter().map(|i| i["label"].as_str().unwrap()).collect::<Vec<_>>(), ["object", "object", "character", "text"]);
        assert_eq!((&items[3]["text"], items[0].get("text")), (&json!("Hello world"), None));
        // A box filling the area runs from edge to edge, as a typed prompt's would.
        let full = Region { bounds: [0.0, 0.0, 1.0, 1.0], ..Region::default() };
        assert_eq!(parse(&layout_prompt(LayoutFormat::Moss, "x", &[full], 17, 3))["items"][0]["bbox"], json!([-1, -8, 2, 9]));
    }

    #[test]
    fn ideogram_and_flux_3_use_the_0_to_1000_grid() {
        let v = parse(&layout_prompt(LayoutFormat::Ideogram, "A cosy reading corner", &scene(), 1536, 1024));
        assert_eq!(v["high_level_description"], "A cosy reading corner");
        let c = &v["compositional_deconstruction"];
        assert_eq!(c["background"], "");
        let el = c["elements"].as_array().unwrap();
        assert_eq!(el[1], json!({"type": "obj", "bbox": [380, 180, 990, 740], "desc": "Green velvet armchair, worn armrests, a knitted throw"}));
        assert_eq!(el[2]["type"], "obj", "a character is an object to Ideogram");
        assert_eq!(el[3], json!({"type": "text", "bbox": [60, 80, 170, 480], "text": "Hello world", "desc": "Clean white sans-serif lettering"}));
        // The documented key order.
        assert!(layout_prompt(LayoutFormat::Ideogram, "s", &scene(), 8, 8).contains(r#"{"type":"text","bbox":[60,80,170,480],"text":"Hello world","desc""#));

        let flux = layout_prompt(LayoutFormat::Flux3, "A cosy reading corner.", &scene(), 1536, 1024);
        let (caption, rows) = flux.split_once(". [").unwrap();
        assert_eq!(caption, "A cosy reading corner, with <obj_1>, <obj_2>, <obj_3>, <obj_4>");
        let rows = parse(&format!("[{rows}"));
        assert_eq!(rows[0], json!({"id": "obj_1", "bbox": [40, 550, 560, 960], "desc": "Tall window, rain streaking the glass, grey dusk outside"}));
        assert_eq!(rows[3]["desc"], "Clean white sans-serif lettering, text reading \"Hello world\"");
        assert!(flux.contains(r#"{"id":"obj_1","bbox":"#));
    }

    #[test]
    fn fibo_splits_objects_from_text_with_normalised_boxes() {
        let v = parse(&layout_prompt(LayoutFormat::Fibo, "A cosy reading corner", &scene(), 1024, 1024));
        assert_eq!(v["short_description"], "A cosy reading corner");
        assert_eq!(v["objects"].as_array().unwrap().len(), 3);
        assert_eq!(v["objects"][0]["bounding_box"], json!([0.55, 0.04, 0.41, 0.52]));
        assert_eq!(v["text_render"][0], json!({"text": "Hello world", "appearance_details": "Clean white sans-serif lettering", "bounding_box": [0.08, 0.06, 0.4, 0.11]}));
    }

    #[test]
    fn hostile_regions_are_dropped_or_clamped_never_a_panic() {
        let bad = [
            Region { bounds: [f64::NAN, 0.0, 1.0, 1.0], ..Region::default() },
            Region { bounds: [0.5, 0.5, 0.5, 0.9], ..Region::default() },
            Region { bounds: [f64::INFINITY, 0.0, 1.0, 1.0], ..Region::default() },
            Region { caption: "x".repeat(10_000), bounds: [1.5, -2.0, -0.5, 3.0], ..Region::default() },
        ];
        for f in LayoutFormat::ALL {
            assert_eq!(layout_prompt(f, " just a scene ", &[], 64, 64), "just a scene");
            assert_eq!(layout_prompt(f, "s", &bad[..3], 64, 64), "s", "{f:?}");
            let one = layout_prompt(f, "", &bad, 0, 0);
            assert!(one.len() < 2600 && one.contains("xxx"), "{f:?}: {one}");
        }
        // The swapped, out-of-range box became the whole area.
        assert_eq!(parse(&layout_prompt(LayoutFormat::Ideogram, "s", &bad[3..], 1, 1))["compositional_deconstruction"]["elements"][0]["bbox"], json!([0, 0, 1000, 1000]));
        // At most MAX_REGIONS are written, the first ones (in front) kept.
        let many: Vec<Region> = (0..100).map(|i| Region { caption: format!("r{i}"), bounds: [0.0, 0.0, 0.5, 0.5], ..Region::default() }).collect();
        let v = parse(&layout_prompt(LayoutFormat::Moss, "s", &many, 64, 64));
        let items = v["items"].as_array().unwrap();
        assert_eq!((items.len(), &items[MAX_REGIONS - 1]["caption"]), (MAX_REGIONS, &json!("r0")));
        // Quotes and newlines in the text stay valid JSON.
        let quoted = Region { kind: RegionKind::Text, caption: "a \"bold\"\nsign".into(), text: "SAY \"HI\"".into(), bounds: [0.1, 0.1, 0.9, 0.9] };
        for f in [LayoutFormat::Moss, LayoutFormat::Ideogram, LayoutFormat::Fibo] {
            parse(&layout_prompt(f, "a \"q\"", std::slice::from_ref(&quoted), 64, 64));
        }
    }
}
