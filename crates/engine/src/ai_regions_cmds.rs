//! The boxes a text-to-image is laid out in (the AI Gen panel's Create › Regions;
//! [`photocraft_doc::Document::ai_regions`]), saved with the document in PSD and `.pcraft` files.
//! Only models trained on boxes take them ([`photocraft_aigen::Model::layout`]).
//!
//! - `ai.regions` lists them, front to back.
//! - `ai.setRegions` replaces them all: adding, removing, reordering (depth) and clearing.
//! - `ai.region.set` changes one: a move, a resize, its caption being typed (`"coalesce"` merges
//!   a run of them into one History step).
//! - `ai.layoutPrompt` is what `ai.generate` would send a model for them: the panel's copy
//!   button.
//! - `ai.setLayerPrompt` changes the prompt an AI layer keeps (what Load Prompt brings back).
//!
//! `ai.generate` with `"regions": true` sends the document's ([`layout`]). `ai.setRegions` and
//! `ai.region.set` with `"layer"` change that AI layer's regions to match in the same History
//! step: the panel keeps the selected layer's layout up to date as it is edited.

use photocraft_aigen::{LayoutFormat, Model, Region, RegionKind, layout_prompt};
use photocraft_doc::{AiRegion, AiRegionKind, LayerId};
use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::ai_cmds::MAX_PROMPT;
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The most regions a document keeps: what one prompt carries.
pub const MAX_REGIONS: usize = photocraft_aigen::layout::MAX_REGIONS;
/// The longest a caption or a text region's words may be, in characters.
pub const MAX_TEXT: usize = photocraft_aigen::layout::MAX_REGION_TEXT;
/// How far outside the canvas, and how large, a box may be (in pixels).
const MAX_COORD: i64 = 1 << 24;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".to_string())
}

/// A region as the commands describe it.
pub fn region_json(r: &AiRegion) -> Value {
    let [cr, cg, cb] = r.color;
    json!({
        "id": r.id,
        "kind": r.kind.key(),
        "caption": r.caption,
        "text": r.text,
        "rect": [r.rect.x0, r.rect.y0, r.rect.width(), r.rect.height()],
        "color": format!("#{cr:02x}{cg:02x}{cb:02x}"),
        "fill": r.fill,
        "fillOpacity": r.fill_opacity,
    })
}

fn listing(s: &Session) -> Value {
    let list: Vec<Value> = s.active().map(|d| d.doc.ai_regions.iter().map(region_json).collect()).unwrap_or_default();
    json!({"regions": list})
}

fn regions(s: &mut Session, _: &Value) -> Result<Value> {
    s.active().ok_or(EngineError::NoDocument)?;
    Ok(listing(s))
}

fn color(cmd: &str, v: &Value) -> Result<[u8; 3]> {
    let msg = || bad(cmd, "`color` must be \"#rrggbb\"");
    let hex = v.as_str().and_then(|s| s.strip_prefix('#')).filter(|h| h.len() == 6 && h.is_ascii()).ok_or_else(msg)?;
    let byte = |i: usize| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()).ok_or_else(msg);
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

fn text(cmd: &str, key: &str, v: &Value, max: usize) -> Result<String> {
    let t = v.as_str().ok_or_else(|| bad(cmd, format!("`{key}` must be a string")))?;
    if t.chars().count() > max {
        return Err(bad(cmd, format!("`{key}` is longer than {max} characters")));
    }
    Ok(t.to_string())
}

fn rect(cmd: &str, v: &Value) -> Result<Rect> {
    let msg = || bad(cmd, "`rect` must be [x, y, width, height] in pixels, with a width and height of 1 or more");
    let nums: Vec<i64> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| x.as_f64().filter(|x| x.is_finite() && x.abs() <= MAX_COORD as f64).map(|x| x.round() as i64))
        .collect::<Option<_>>()
        .ok_or_else(msg)?;
    let [x, y, w, h] = nums[..] else { return Err(msg()) };
    if w < 1 || h < 1 {
        return Err(msg());
    }
    let i = |v: i64| i32::try_from(v.clamp(-MAX_COORD, 2 * MAX_COORD)).map_err(|_| msg());
    Ok(Rect::new(i(x)?, i(y)?, i(x + w)?, i(y + h)?))
}

/// `r` with the fields `p` gives changed; a field `p` lacks stays.
fn patched(cmd: &str, mut r: AiRegion, p: &Value) -> Result<AiRegion> {
    let o = p.as_object().ok_or_else(|| bad(cmd, "each region is an object"))?;
    for (key, v) in o {
        match key.as_str() {
            "id" | "coalesce" | "layer" => {}
            "kind" => r.kind = v.as_str().and_then(AiRegionKind::from_key).ok_or_else(|| bad(cmd, "`kind` must be object|character|text"))?,
            "caption" => r.caption = text(cmd, key, v, MAX_TEXT)?,
            "text" => r.text = text(cmd, key, v, MAX_TEXT)?,
            "rect" => r.rect = rect(cmd, v)?,
            "color" => r.color = color(cmd, v)?,
            "fill" => r.fill = v.as_bool().ok_or_else(|| bad(cmd, "`fill` must be true or false"))?,
            "fillOpacity" => {
                r.fill_opacity = v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`fillOpacity` must be a number from 0 to 1"))? as f32
            }
            other => return Err(bad(cmd, format!("a region has no `{other}`"))),
        }
    }
    Ok(r)
}

/// `p["layer"]`: an AI layer (one with a generation record) whose regions follow the edit.
fn follower(s: &Session, cmd: &str, p: &Value) -> Result<Option<LayerId>> {
    let Some(v) = p.get("layer").filter(|v| !v.is_null()) else { return Ok(None) };
    let id = LayerId(v.as_u64().ok_or_else(|| bad(cmd, "`layer` must be a layer id"))?);
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let l = d.doc.layer(id).ok_or_else(|| bad(cmd, format!("the document has no layer {}", id.0)))?;
    if l.generation.is_none() {
        return Err(bad(cmd, format!("layer {} was not made with AI Gen", id.0)));
    }
    Ok(Some(id))
}

/// Does `layer`'s generation record keep other regions than `regions`?
fn layer_differs(s: &Session, layer: Option<LayerId>, regions: &[AiRegion]) -> bool {
    layer.is_some_and(|id| s.active().and_then(|d| d.doc.layer(id)).and_then(|l| l.generation.as_ref()).is_some_and(|g| g.regions != regions))
}

/// The document's regions become `regions`, and `layer`'s too.
fn store(s: &mut Session, label: &str, regions: Vec<AiRegion>, layer: Option<LayerId>) -> Result<()> {
    s.edit(label, |doc, _| {
        if let Some(g) = layer.and_then(|id| doc.layer_mut(id)).and_then(|l| l.generation.as_mut()) {
            g.regions = regions.clone();
        }
        doc.ai_regions = regions;
        Ok(())
    })
}

/// The colours new regions are given in turn, told apart in lightness as well as hue.
pub const PALETTE: [[u8; 3]; 8] = [
    [0xf2, 0xb3, 0x3d],
    [0x4f, 0xd1, 0xc5],
    [0xa9, 0x9b, 0xff],
    [0xf2, 0x8b, 0xb5],
    [0x9f, 0xd3, 0x56],
    [0x6c, 0xb6, 0xff],
    [0xff, 0x8a, 0x65],
    [0xe0, 0xe0, 0xe0],
];

fn set_regions(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.setRegions";
    let list = p.get("regions").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`regions` must be a list"))?;
    if list.len() > MAX_REGIONS {
        return Err(bad(cmd, format!("a document keeps {MAX_REGIONS} regions at most")));
    }
    let layer = follower(s, cmd, p)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let old = &d.doc.ai_regions;
    // New ids follow the highest there is, so an id is never reused within the document's life
    // of this list.
    let mut next = old.iter().map(|r| r.id).max().unwrap_or(0);
    let mut kept: Vec<AiRegion> = Vec::with_capacity(list.len());
    for entry in list {
        let id = match entry.get("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().and_then(|i| u32::try_from(i).ok()).ok_or_else(|| bad(cmd, "`id` must be a region's id"))?),
        };
        // A region that is there is changed by what the entry gives; a new one starts from the
        // defaults and the next colour.
        let base = match id.and_then(|id| old.iter().find(|r| r.id == id)) {
            Some(r) => r.clone(),
            None => {
                let id = match id {
                    Some(id) => id,
                    None => {
                        next = next.checked_add(1).ok_or_else(|| bad(cmd, "no region ids are left"))?;
                        next
                    }
                };
                let color = PALETTE.get(id as usize % PALETTE.len()).copied().unwrap_or(PALETTE[0]);
                if entry.get("rect").is_none() {
                    return Err(bad(cmd, "a new region needs a `rect`"));
                }
                AiRegion { id, color, ..AiRegion::default() }
            }
        };
        next = next.max(base.id);
        if kept.iter().any(|r| r.id == base.id) {
            return Err(bad(cmd, format!("region {} is listed twice", base.id)));
        }
        kept.push(patched(cmd, base, entry)?);
    }
    if s.active().is_some_and(|d| d.doc.ai_regions != kept) || layer_differs(s, layer, &kept) {
        store(s, "AI Regions", kept, layer)?;
    }
    Ok(listing(s))
}

fn set_region(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.region.set";
    let id = p.get("id").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).ok_or_else(|| bad(cmd, "`id` must be a region's id"))?;
    let layer = follower(s, cmd, p)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let r = d.doc.ai_regions.iter().find(|r| r.id == id).cloned().ok_or_else(|| bad(cmd, format!("the document has no region {id}")))?;
    let changed = patched(cmd, r.clone(), p)?;
    let all: Vec<AiRegion> = d.doc.ai_regions.iter().map(|r| if r.id == id { changed.clone() } else { r.clone() }).collect();
    if changed != r || layer_differs(s, layer, &all) {
        store(s, "AI Region", all, layer)?;
    }
    Ok(json!({"region": region_json(&changed)}))
}

fn set_layer_prompt(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.setLayerPrompt";
    let layer = follower(s, cmd, p)?.ok_or_else(|| bad(cmd, "`layer` must be a layer id"))?;
    let prompt = p.get("prompt").ok_or_else(|| bad(cmd, "`prompt` must be a string")).and_then(|v| text(cmd, "prompt", v, MAX_PROMPT))?;
    let same = s.active().and_then(|d| d.doc.layer(layer)).and_then(|l| l.generation.as_ref()).is_some_and(|g| g.prompt == prompt);
    if !same {
        s.edit("AI Layer Prompt", |doc, _| {
            if let Some(g) = doc.layer_mut(layer).and_then(|l| l.generation.as_mut()) {
                g.prompt = prompt.clone();
            }
            Ok(())
        })?;
    }
    Ok(json!({"layer": layer.0, "prompt": prompt}))
}

/// The regions in `p["regions"]` (`true`: the document's), each as the document keeps it.
pub(crate) fn regions_param(s: &Session, cmd: &str, p: &Value) -> Result<Vec<AiRegion>> {
    match p.get("regions") {
        None | Some(Value::Bool(false)) | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Bool(true)) => Ok(s.active().map(|d| d.doc.ai_regions.clone()).unwrap_or_default()),
        Some(Value::Array(list)) => {
            if list.len() > MAX_REGIONS {
                return Err(bad(cmd, format!("{MAX_REGIONS} regions at most")));
            }
            list.iter().enumerate().map(|(i, e)| patched(cmd, AiRegion { id: u32::try_from(i).unwrap_or(u32::MAX), ..AiRegion::default() }, e)).collect()
        }
        Some(_) => Err(bad(cmd, "`regions` must be true (the document's) or a list")),
    }
}

/// `regions` (document pixels, front to back) as the layout of a generation into `area`: each
/// box as a fraction of the area, so it is cut where it leaves it, and one wholly outside it is
/// left out by the writer.
pub fn layout(regions: &[AiRegion], area: Rect) -> Vec<Region> {
    let (w, h) = (f64::from(area.width().max(1)), f64::from(area.height().max(1)));
    regions
        .iter()
        .map(|r| Region {
            kind: match r.kind {
                AiRegionKind::Object => RegionKind::Object,
                AiRegionKind::Character => RegionKind::Character,
                AiRegionKind::Text => RegionKind::Text,
            },
            caption: r.caption.clone(),
            text: r.text.clone(),
            bounds: [
                f64::from(r.rect.x0.saturating_sub(area.x0)) / w,
                f64::from(r.rect.y0.saturating_sub(area.y0)) / h,
                f64::from(r.rect.x1.saturating_sub(area.x0)) / w,
                f64::from(r.rect.y1.saturating_sub(area.y0)) / h,
            ],
        })
        .collect()
}

/// The layout format `model` reads, or an error saying which models take regions.
pub(crate) fn format_of(model: &Model, settings: &photocraft_aigen::Settings) -> Result<LayoutFormat> {
    model.layout().ok_or_else(|| {
        let able: Vec<&str> = settings.listed().into_iter().filter(|m| m.layout().is_some()).map(|m| m.name.as_str()).collect();
        let which = if able.is_empty() { "MOSS Image, Ideogram 4, FLUX.3 Klein, FLUX 3 Image or FIBO 2".to_string() } else { able.join(", ") };
        EngineError::Other(format!("{} can't place regions: it reads a layout as words. Models trained on boxes: {which}", model.name))
    })
}

fn layout_prompt_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.layoutPrompt";
    let scene = p.get("prompt").map(|v| text(cmd, "prompt", v, MAX_PROMPT)).transpose()?.unwrap_or_default();
    let settings = crate::ai_cmds::settings(s);
    let id = p.get("model").and_then(Value::as_str).unwrap_or("");
    let model = settings.model(id).ok_or_else(|| bad(cmd, if id.is_empty() { "no AI model is set up".to_string() } else { format!("no AI model `{id}`") }))?;
    let format = format_of(model, &settings)?;
    let regions = if p.get("regions").is_some() { regions_param(s, cmd, p)? } else { regions_param(s, cmd, &json!({"regions": true}))? };
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let area = crate::ai_cmds::target_area(p, d.doc.selection.as_ref(), d.doc.bounds())?;
    let text = layout_prompt(format, &scene, &layout(&regions, area), area.width(), area.height());
    Ok(json!({"text": text, "format": format.key(), "area": [area.x0, area.y0, area.width(), area.height()]}))
}

const REGION_PARAMS: &str = r##"{"id":int?,"kind":"object|character|text"?,"caption":str?,"text":str? (text regions: the words to render),"rect":[x,y,w,h]? (document pixels; a new region needs it),"color":"#rrggbb"?,"fill":bool?,"fillOpacity":0..1?}"##;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.regions",
            label: "AI Regions",
            menu: &[],
            shortcut: None,
            params: r##"{} → {regions:[{id,kind,caption,text,rect:[x,y,w,h],color,fill,fillOpacity},…]}: the boxes the active document lays a text-to-image out in (the AI Gen panel's Create › Regions), front to back"##,
            enabled: has_doc,
            run: regions,
            journal: false,
        },
        CommandSpec {
            id: "ai.setRegions",
            label: "Set AI Regions",
            menu: &[],
            shortcut: None,
            params: Box::leak(
                format!(
                    "{{\"regions\":[{REGION_PARAMS},…] ({MAX_REGIONS} at most, front to back)}} → as ai.regions: replace the active document's regions (saved with it in PSD and .pcraft files). An entry with the id of a region there changes it by the fields it gives; one without an id is new (it needs a rect, and gets the next colour). Order is depth: the first is in front. \"layer\":id? also gives that AI layer these regions (what Load Prompt brings back). One History step"
                )
                .into_boxed_str(),
            ),
            enabled: has_doc,
            run: set_regions,
            journal: true,
        },
        CommandSpec {
            id: "ai.region.set",
            label: "Set AI Region",
            menu: &[],
            shortcut: None,
            params: Box::leak(format!("{REGION_PARAMS} with \"id\" required, \"layer\":id? → {{region}}: change one region (and give that AI layer the document's regions); \"coalesce\":key merges a run into one History step").into_boxed_str()),
            enabled: has_doc,
            run: set_region,
            journal: true,
        },
        CommandSpec {
            id: "ai.layoutPrompt",
            label: "AI Layout Prompt",
            menu: &[],
            shortcut: None,
            params: r##"{"prompt":str (the scene),"model":id?=first enabled model,"regions":true|[…]?=the document's,"rect":[x,y,w,h]?=the selection's bounds, else the canvas} → {text,format:"moss|ideogram|flux3|fibo",area}: the prompt ai.generate with "regions": true would send that model (its layout format: boxes made relative to the area, back to front). Refused for a model that can't place regions"##,
            enabled: has_doc,
            run: layout_prompt_cmd,
            journal: false,
        },
        CommandSpec {
            id: "ai.setLayerPrompt",
            label: "Set AI Layer Prompt",
            menu: &[],
            shortcut: None,
            params: r##"{"layer":id,"prompt":str,"coalesce":key?} → {layer,prompt}: change the prompt an AI layer keeps (its generation record: what Load Prompt brings back). Refused for a layer not made with AI Gen"##,
            enabled: has_doc,
            run: set_layer_prompt,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 200, "height": 100})).unwrap();
        s
    }

    fn ids(s: &Session) -> Vec<u32> {
        s.active().unwrap().doc.ai_regions.iter().map(|r| r.id).collect()
    }

    #[test]
    fn regions_are_set_reordered_patched_and_undone() {
        let mut s = session();
        let v = s
            .execute(
                "ai.setRegions",
                json!({"regions": [
                    {"kind": "character", "caption": "a cat", "rect": [10, 10, 50, 40]},
                    {"caption": "an armchair", "rect": [0, 0, 120, 90], "fill": false},
                    {"kind": "text", "text": "READ MORE", "caption": "cream serif", "rect": [5, 5, 80, 20], "color": "#00ff80"}
                ]}),
            )
            .unwrap();
        assert_eq!(ids(&s), [1, 2, 3]);
        assert_eq!(
            v["regions"][0],
            json!({"id": 1, "kind": "character", "caption": "a cat", "text": "", "rect": [10, 10, 50, 40], "color": "#4fd1c5", "fill": true, "fillOpacity": 0.12f32})
        );
        assert_eq!((&v["regions"][1]["fill"], &v["regions"][2]["color"]), (&json!(false), &json!("#00ff80")));
        // Reordered (depth) and one dropped: ids stay, and a new one never takes a used id.
        s.execute("ai.setRegions", json!({"regions": [{"id": 3}, {"id": 1}, {"rect": [1, 1, 2, 2]}]})).unwrap();
        assert_eq!(ids(&s), [3, 1, 4]);
        assert_eq!(s.active().unwrap().doc.ai_regions[0].text, "READ MORE");
        // One changed, typing merged into one step.
        for caption in ["a g", "a gin", "a ginger cat"] {
            s.execute("ai.region.set", json!({"id": 1, "caption": caption, "coalesce": "caption-1"})).unwrap();
        }
        assert_eq!(s.active().unwrap().doc.ai_regions[1].caption, "a ginger cat");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active().unwrap().doc.ai_regions[1].caption, "a cat");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(ids(&s), [1, 2, 3]);
        assert_eq!(s.execute("ai.regions", json!({})).unwrap()["regions"].as_array().unwrap().len(), 3);
        s.execute("ai.setRegions", json!({"regions": []})).unwrap();
        assert!(ids(&s).is_empty());
    }

    #[test]
    fn bad_params_are_errors_never_a_panic() {
        let mut s = session();
        s.execute("ai.setRegions", json!({"regions": [{"rect": [0, 0, 10, 10]}]})).unwrap();
        let too_many: Vec<Value> = (0..=MAX_REGIONS).map(|_| json!({"rect": [0, 0, 5, 5]})).collect();
        for p in [
            json!({}),
            json!({"regions": 5}),
            json!({"regions": [5]}),
            json!({"regions": [{}]}),
            json!({"regions": [{"rect": [0, 0, 0, 5]}]}),
            json!({"regions": [{"rect": [0, 0, 5]}]}),
            json!({"regions": [{"rect": [0, 0, 1e300, 5]}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "kind": "dog"}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "color": "red"}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "color": "#ééé"}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "fillOpacity": 2}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "caption": "x".repeat(MAX_TEXT + 1)}]}),
            json!({"regions": [{"rect": [0, 0, 5, 5], "size": 3}]}),
            json!({"regions": [{"id": 1}, {"id": 1}]}),
            json!({"regions": [{"id": -1}]}),
            json!({"regions": too_many}),
        ] {
            assert!(s.execute("ai.setRegions", p.clone()).is_err(), "{p}");
        }
        assert_eq!(ids(&s), [1], "a refused list changes nothing");
        let plain = s.active().unwrap().active_layer.unwrap().0;
        for p in [
            json!({}),
            json!({"id": 9}),
            json!({"id": 1, "rect": "big"}),
            json!({"id": 1, "fill": 1}),
            json!({"id": 1, "layer": plain}),
            json!({"id": 1, "layer": "x"}),
        ] {
            assert!(s.execute("ai.region.set", p.clone()).is_err(), "{p}");
        }
        assert!(s.execute("ai.setRegions", json!({"regions": [], "layer": 99_999})).is_err());
        for p in [json!({}), json!({"layer": plain, "prompt": "x"}), json!({"layer": 99_999, "prompt": "x"}), json!({"layer": plain, "prompt": 5})] {
            assert!(s.execute("ai.setLayerPrompt", p.clone()).is_err(), "{p}");
        }
    }

    #[test]
    fn an_ai_layer_keeps_the_regions_and_prompt_it_is_edited_with() {
        let mut s = session();
        s.execute("ai.setRegions", json!({"regions": [{"caption": "a cat", "rect": [10, 10, 50, 40]}]})).unwrap();
        let layer = s.active().unwrap().active_layer.unwrap();
        let regions = s.active().unwrap().doc.ai_regions.clone();
        s.edit("AI", |doc, _| {
            doc.layer_mut(layer).unwrap().generation = Some(Box::new(photocraft_doc::Generation { prompt: "a room".into(), regions, ..Default::default() }));
            Ok(())
        })
        .unwrap();
        let kept = |s: &Session| s.active().unwrap().doc.layer(layer).unwrap().generation.as_ref().map(|g| (g.prompt.clone(), g.regions.clone())).unwrap();
        // A change with the layer named reaches it; one without leaves it alone.
        s.execute("ai.region.set", json!({"id": 1, "caption": "a ginger cat", "layer": layer.0})).unwrap();
        assert_eq!(kept(&s).1[0].caption, "a ginger cat");
        s.execute("ai.setRegions", json!({"regions": [{"rect": [0, 0, 5, 5]}, {"id": 1}], "layer": layer.0})).unwrap();
        assert_eq!(kept(&s).1, s.active().unwrap().doc.ai_regions);
        s.execute("ai.setRegions", json!({"regions": []})).unwrap();
        assert_eq!(kept(&s).1.len(), 2);
        // The prompt, typed: one History step, undone with the layer's regions untouched.
        for prompt in ["a r", "a reading", "a reading nook"] {
            s.execute("ai.setLayerPrompt", json!({"layer": layer.0, "prompt": prompt, "coalesce": "scene"})).unwrap();
        }
        assert_eq!(kept(&s).0, "a reading nook");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!((kept(&s).0.as_str(), kept(&s).1.len()), ("a room", 2));
    }

    #[test]
    fn the_layout_is_relative_to_the_area_and_cut_by_it() {
        let regions =
            [AiRegion { rect: Rect::new(50, 25, 150, 75), ..AiRegion::default() }, AiRegion { rect: Rect::new(-50, -50, 0, 0), ..AiRegion::default() }];
        let l = layout(&regions, Rect::new(50, 0, 250, 100));
        assert_eq!(l[0].bounds, [0.0, 0.25, 0.5, 0.75]);
        // Wholly outside: the writer leaves it out.
        let text = layout_prompt(LayoutFormat::Ideogram, "s", &l, 200, 100);
        assert_eq!(text.matches("\"obj\"").count(), 1, "{text}");
        assert!(layout(&regions, Rect::EMPTY).iter().all(|r| r.bounds.iter().all(|v| v.is_finite())));
    }
}
