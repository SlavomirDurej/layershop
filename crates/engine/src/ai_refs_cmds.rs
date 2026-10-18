//! The references a document keeps for its edits (the AI Gen panel's chips, `@Luna` in a prompt;
//! [`photocraft_doc::Document::ai_references`]), saved with it in PSD and `.pcraft` files.
//!
//! - `ai.references` lists them.
//! - `ai.setReferences` replaces them. A layer of the document, or the document itself, is read
//!   when an edit runs; another open document, or a layer of one, is copied into the document
//!   when it is set (that document need not be open next time). An entry can keep one that is
//!   there by its index, renamed or not. It is one History step (`"coalesce"` merges a run of
//!   them, such as a name being typed).
//!
//! `ai.generate` with `"mode": "imgEdit"` and no `references` sends these (see
//! [`crate::ai_refs`] for how a prompt picks them by name).

use photocraft_doc::{AiPicture, AiReference, LayerId};
use serde_json::{Value, json};

use crate::ai_cmds::{SourceRef, reference_copy};
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".to_string())
}

/// What a kept reference's picture is, to show: its layer, the document, or the copy's label.
pub fn describe(s: &Session, picture: &AiPicture) -> String {
    match picture {
        AiPicture::Layer(id) => {
            let name = s.active().and_then(|d| d.doc.layer(LayerId(*id))).map_or_else(|| format!("layer {id}"), |l| l.name.clone());
            format!("Layer: {name}")
        }
        AiPicture::Document => "This document (all layers)".to_string(),
        AiPicture::Image { label, .. } => format!("Copy of {label}"),
    }
}

fn listing(s: &Session) -> Value {
    let list: Vec<Value> = s
        .active()
        .map(|d| {
            d.doc
                .ai_references
                .iter()
                .map(|r| {
                    let mut v = match &r.picture {
                        AiPicture::Layer(id) => json!({"layer": id}),
                        AiPicture::Document => json!({"document": d.doc.id.0}),
                        AiPicture::Image { png, label } => json!({"copy": {"label": label, "bytes": png.len()}}),
                    };
                    v["name"] = json!(r.name);
                    v["label"] = json!(describe(s, &r.picture));
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    json!({"references": list})
}

fn references(s: &mut Session, _: &Value) -> Result<Value> {
    s.active().ok_or(EngineError::NoDocument)?;
    Ok(listing(s))
}

/// The picture an entry of `ai.setReferences` names, as the document keeps it.
fn picture(s: &Session, cmd: &str, entry: &Value) -> Result<AiPicture> {
    let d = s.active().ok_or(EngineError::NoDocument)?;
    if let Some(keep) = entry.get("keep") {
        let i = keep.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(cmd, "`keep` must be the index of a reference the document has"))?;
        return d.doc.ai_references.get(i).map(|r| r.picture.clone()).ok_or_else(|| bad(cmd, format!("the document has no reference {i} to keep")));
    }
    let source: SourceRef = serde_json::from_value(entry.clone())
        .map_err(|_| bad(cmd, "each reference is {\"layer\": id}, {\"document\": id}, both, or {\"keep\": index}, with a \"name\" or none"))?;
    let here = d.doc.id.0;
    match source.settled(s) {
        // This document's own pictures are read when an edit runs.
        SourceRef::Layer(id) => {
            let layer = d.doc.layer(LayerId(id)).ok_or_else(|| EngineError::Other(format!("the layer {id} is not in this document")))?;
            if layer.surface().is_none() {
                return Err(EngineError::Other(format!("\u{201c}{}\u{201d} has no pixels to refer to: choose a layer that has, or rasterize it", layer.name)));
            }
            Ok(AiPicture::Layer(id))
        }
        SourceRef::Document(id) if id == here => Ok(AiPicture::Document),
        // Another document's are copied in, so they are there when it is closed.
        other => {
            let (png, label) = reference_copy(s, other).map_err(EngineError::Other)?;
            Ok(AiPicture::Image { png, label })
        }
    }
}

fn set_references(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.setReferences";
    let list = p.get("references").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`references` must be a list"))?;
    if list.len() > photocraft_aigen::MAX_REFERENCES {
        return Err(bad(cmd, format!("a document keeps {} references at most", photocraft_aigen::MAX_REFERENCES)));
    }
    let mut kept = Vec::with_capacity(list.len());
    for entry in list {
        if !entry.is_object() {
            return Err(bad(cmd, "each reference is an object"));
        }
        let name = match entry.get("name") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(n)) => crate::ai_refs::clean_name(n),
            Some(_) => return Err(bad(cmd, "`name` must be a string")),
        };
        kept.push(AiReference { name, picture: picture(s, cmd, entry)? });
    }
    if let Some(name) = crate::ai_refs::duplicate(&kept.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()) {
        return Err(bad(cmd, format!("two references are named \u{201c}{name}\u{201d}: give each its own name")));
    }
    if s.active().is_some_and(|d| d.doc.ai_references != kept) {
        s.edit("AI References", |doc, _| {
            doc.ai_references = kept;
            Ok(())
        })?;
    }
    Ok(listing(s))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.references",
            label: "AI References",
            menu: &[],
            shortcut: None,
            params: r##"{} → {references:[{"layer":id}|{"document":id}|{"copy":{label,bytes}} with name,label]}: the pictures the active document keeps for its edits to refer to (the AI Gen panel's Refs), in order; a copy is a picture from another document kept in this one"##,
            enabled: has_doc,
            run: references,
            journal: false,
        },
        CommandSpec {
            id: "ai.setReferences",
            label: "Set AI References",
            menu: &[],
            shortcut: None,
            params: r##"{"references":[{"layer":id}|{"document":id}|{"document":id,"layer":id}|{"keep":index}, each with "name":str? (letters, digits, _ and -; no two alike),…] (3 at most)} → as ai.references: replace the active document's references (saved with it in PSD and .pcraft files). A layer of this document, or the document itself, is read when an edit runs; another open document, or a layer of one, is copied in now; keep takes one the document has, by its index. ai.generate imgEdit without references sends these: those the prompt @mentions by name, else all. One History step"##,
            enabled: has_doc,
            run: set_references,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn session() -> (Session, u64, u64) {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 40, "height": 30})).unwrap();
        let background = s.active().unwrap().active_layer.unwrap().0;
        s.execute("layer.new.layer", json!({"name": "Luna"})).unwrap();
        s.execute("select.rect", json!({"x": 2, "y": 2, "width": 10, "height": 20})).unwrap();
        s.execute("edit.fill", json!({"color": "#3366cc"})).unwrap();
        s.execute("select.deselect", json!({})).unwrap();
        let luna = s.active().unwrap().active_layer.unwrap().0;
        (s, background, luna)
    }

    #[test]
    fn a_document_keeps_its_references_and_copies_other_documents_pictures() {
        let (mut s, background, luna) = session();
        let here = s.active().unwrap().doc.id.0;
        // Another document with a layer of its own.
        s.execute("file.new", json!({"width": 20, "height": 20})).unwrap();
        let there = s.active().unwrap().doc.id.0;
        s.execute("layer.new.layer", json!({"name": "Max"})).unwrap();
        s.execute("select.all", json!({})).unwrap();
        s.execute("edit.fill", json!({"color": "#cc3333"})).unwrap();
        let max = s.active().unwrap().active_layer.unwrap().0;
        assert!(s.set_active(0));
        let undo_before = s.active().unwrap().history.can_undo();
        let r = s
            .execute(
                "ai.setReferences",
                json!({"references": [
                    {"layer": luna, "name": "Luna"},
                    {"document": here, "layer": background, "name": " the  sky "},
                    {"document": there, "layer": max, "name": "Max"},
                ]}),
            )
            .unwrap();
        let list = r["references"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!((list[0]["layer"].as_u64(), list[0]["label"].as_str()), (Some(luna), Some("Layer: Luna")));
        assert_eq!((list[1]["layer"].as_u64(), list[1]["name"].as_str()), (Some(background), Some("the_sky")));
        assert!(list[2]["copy"]["bytes"].as_u64().unwrap() > 0, "{}", list[2]);
        assert!(list[2]["label"].as_str().unwrap().starts_with("Copy of Layer: Max"));
        // A History step: undo takes them away, and setting the same again changes nothing.
        assert!(s.active().unwrap().history.can_undo() || undo_before);
        let refs = s.active().unwrap().doc.ai_references.clone();
        assert!(matches!(&refs[2].picture, AiPicture::Image { png, .. } if png.starts_with(b"\x89PNG")));
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.active().unwrap().doc.ai_references.is_empty());
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.active().unwrap().doc.ai_references, refs);
        // The copy outlives the document it came from; keep renames without copying again.
        assert!(s.close(1).is_some());
        let r = s.execute("ai.setReferences", json!({"references": [{"keep": 2, "name": "Maximus"}, {"document": here}]})).unwrap();
        assert_eq!(r["references"][0]["name"], "Maximus");
        assert_eq!(s.active().unwrap().doc.ai_references[0].picture, refs[2].picture);
        assert_eq!(s.active().unwrap().doc.ai_references[1].picture, AiPicture::Document);
        assert_eq!(s.execute("ai.references", json!({})).unwrap()["references"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn bad_references_are_errors_that_change_nothing() {
        let (mut s, _, luna) = session();
        s.execute("layer.new.group", json!({})).ok();
        let fails = |s: &mut Session, p: Value| s.execute("ai.setReferences", p).unwrap_err().to_string();
        for (p, want) in [
            (json!({}), "must be a list"),
            (json!({"references": 3}), "must be a list"),
            (json!({"references": [1]}), "is an object"),
            (json!({"references": [{"name": "x"}]}), "each reference is"),
            (json!({"references": [{"layer": luna, "name": 5}]}), "must be a string"),
            (json!({"references": [{"keep": 0}]}), "no reference 0"),
            (json!({"references": [{"keep": -1}]}), "`keep` must be"),
            (json!({"references": [{"layer": 999_999}]}), "is not in this document"),
            (json!({"references": [{"document": 999_999}]}), "is not open"),
            (json!({"references": [{"layer": luna, "name": "A"}, {"layer": luna, "name": "a"}]}), "two references are named"),
            (json!({"references": [{"layer": luna}, {"layer": luna}, {"layer": luna}, {"layer": luna}]}), "3 references at most"),
        ] {
            let e = fails(&mut s, p.clone());
            assert!(e.contains(want), "{p}: {e}");
        }
        assert!(s.active().unwrap().doc.ai_references.is_empty());
        // No document: refused, not a panic.
        let mut empty = Session::new();
        assert!(empty.execute("ai.setReferences", json!({"references": []})).is_err());
        assert!(empty.execute("ai.references", json!({})).is_err());
    }
}
