//! What a generative-AI layer was made from ([`photocraft_doc::Generation`]) in PSD/PSB files and
//! layered TIFFs: one item of the layer's `shmd` (metadata setting) block, under PhotoCraft's own
//! key `pcGn`.
//!
//! `shmd` is the list of keyed, length-prefixed items Photoshop keeps per-layer metadata in
//! (`cmls` for layer comps, `mlst`, `cust`); a reader steps over the keys it does not know, so the
//! file opens anywhere a PSD does. The item's data is the generation as JSON (the same fields
//! `.pcraft` saves), padded with a space to an even length. Layers it names (the picture that was
//! redrawn, the references) are written as PSD layer ids (`lyid`), which are what a file keeps;
//! another open document is not written, as it means nothing once the session ends.

use std::collections::HashMap;

use photocraft_doc::{Document, Generation, GenerationReference, GenerationSource, Layer, LayerId};
use photocraft_psd::metadata::{MetadataItem, parse_shmd, write_shmd};

/// The `shmd` item key.
pub const KEY: [u8; 4] = *b"pcGn";
/// The most item data that is read: a prompt is a few kilobytes at the very most.
const MAX_LEN: usize = 1 << 20;

/// `g` with the layers it names passed through `layer` (`None`: the layer is gone, and so is the
/// reference to it) and other documents (and their layers) dropped. A reference keeps its name.
fn remap(mut g: Generation, layer: impl Fn(u64) -> Option<u64>) -> Generation {
    let map = |s: GenerationSource| match s {
        GenerationSource::Layer(id) => layer(id).map(GenerationSource::Layer),
        GenerationSource::Document(_) | GenerationSource::DocumentLayer { .. } => None,
    };
    g.source = g.source.and_then(map);
    g.references = std::mem::take(&mut g.references).into_iter().filter_map(|r| Some(GenerationReference { source: map(r.source)?, ..r })).collect();
    g
}

/// The item data for `l`'s generation, naming layers by the PSD ids in `ids`.
fn encode(l: &Layer, ids: &HashMap<LayerId, u32>) -> Option<Vec<u8>> {
    let g = remap(*l.generation.clone()?, |id| ids.get(&LayerId(id)).map(|p| u64::from(*p)));
    let mut data = serde_json::to_vec(&g).ok()?;
    if data.len() % 2 == 1 {
        data.push(b' ');
    }
    Some(data)
}

/// The generation in item data, layers still named by PSD id. `None` for anything that is not
/// one: the layer then simply has none.
fn decode(data: &[u8]) -> Option<Generation> {
    if data.len() > MAX_LEN {
        return None;
    }
    // Another writer may have padded with zero bytes, as `shmd` items usually are.
    let end = data.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let mut g: Generation = serde_json::from_slice(data.get(..end)?).ok()?;
    let unit = |v: f64, max: f64, or: f64| if v.is_finite() { v.clamp(0.0, max) } else { or };
    g.strength = unit(g.strength, 1.0, 1.0);
    g.context = unit(g.context, 4.0, 0.0);
    Some(g)
}

/// Sets the `pcGn` item of a layer's `shmd` block among its raw blocks from the layer's
/// generation (removing it when the layer has none), keeping the other metadata items.
pub(crate) fn put(raw: &mut Vec<([u8; 4], Vec<u8>)>, l: &Layer, ids: &HashMap<LayerId, u32>) {
    let pos = raw.iter().position(|(k, _)| k == b"shmd");
    let mut items = match pos.and_then(|i| raw.get(i)).map(|(_, d)| parse_shmd(d)) {
        Some(Ok(items)) => items,
        // A block that can't be read is left as it came: nothing is known about what it holds.
        Some(Err(_)) => return,
        None => Vec::new(),
    };
    let had = items.len();
    items.retain(|it| it.key != KEY);
    let data = encode(l, ids);
    if data.is_none() && items.len() == had {
        return;
    }
    if let Some(data) = data {
        // Copied with the layer: a duplicate was generated the same way.
        items.push(MetadataItem { copy: true, ..MetadataItem::new(KEY, data) });
    }
    match (pos, items.is_empty()) {
        (Some(i), true) => {
            raw.remove(i);
        }
        (Some(i), false) => {
            if let Some(block) = raw.get_mut(i) {
                block.1 = write_shmd(&items);
            }
        }
        (None, false) => raw.push((*b"shmd", write_shmd(&items))),
        (None, true) => {}
    }
}

/// The generation a layer's preserved `shmd` block holds, layers still named by PSD id.
fn stored(l: &Layer) -> Option<Generation> {
    let (_, shmd) = l.psd_blocks.iter().find(|(k, _)| k == b"shmd")?;
    let item = parse_shmd(shmd).ok()?.into_iter().find(|it| it.key == KEY)?;
    decode(&item.data)
}

/// Gives every layer of an imported document the generation its `shmd` block holds, once the
/// layers' PSD ids are known. The item stays in `Layer::psd_blocks`; export rewrites it from the
/// field.
pub(crate) fn import(doc: &mut Document) {
    let walk = doc.walk();
    let by_psd_id: HashMap<u64, u64> = walk.iter().filter_map(|(_, _, l)| l.psd_id.map(|p| (u64::from(p), l.id.0))).collect();
    let found: Vec<(LayerId, Generation)> = walk.iter().filter_map(|(_, _, l)| Some((l.id, stored(l)?))).collect();
    for (id, g) in found {
        if let Some(l) = doc.layer_mut(id) {
            l.generation = Some(Box::new(remap(g, |psd| by_psd_id.get(&psd).copied())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::{GenerationMode, GenerationQuality, PixelFormat};

    fn layer(g: Option<Generation>) -> Layer {
        let mut l = Layer::raster("L", PixelFormat::RGBA8);
        l.generation = g.map(Box::new);
        l
    }

    fn generation() -> Generation {
        Generation {
            mode: GenerationMode::ImgEdit,
            model: "m".into(),
            model_name: "M".into(),
            prompt: "a pond, \"quoted\" \u{1F986}".into(),
            strength: 0.6,
            context: 0.5,
            alpha: true,
            seed: 0xFFFF_FFFF_FFFF,
            source: Some(GenerationSource::Layer(40)),
            references: vec![
                GenerationReference { source: GenerationSource::Layer(41), name: "Luna".into() },
                GenerationSource::Document(9).into(),
                GenerationSource::DocumentLayer { document: 9, layer: 41 }.into(),
                GenerationSource::Layer(99).into(),
            ],
            quality: GenerationQuality::Detail,
        }
    }

    fn item(raw: &[([u8; 4], Vec<u8>)]) -> Option<MetadataItem> {
        let (_, shmd) = raw.iter().find(|(k, _)| k == b"shmd")?;
        parse_shmd(shmd).unwrap().into_iter().find(|it| it.key == KEY)
    }

    #[test]
    fn the_item_is_written_beside_the_others_and_names_layers_by_psd_id() {
        let ids: HashMap<LayerId, u32> = [(LayerId(40), 7), (LayerId(41), 8)].into();
        let mut raw = vec![(*b"shmd", write_shmd(&[MetadataItem::new(*b"cmls", vec![1, 2])]))];
        put(&mut raw, &layer(Some(generation())), &ids);
        assert_eq!(parse_shmd(&raw[0].1).unwrap().len(), 2, "the layer comp item stays");
        let it = item(&raw).unwrap();
        assert!(it.copy && it.data.len().is_multiple_of(2) && &it.signature == b"8BIM");
        let g = decode(&it.data).unwrap();
        // The source and the first reference by PSD id, with its name; the other document, its
        // layer and the layer that is not in the file are gone.
        assert_eq!(
            (g.source, g.references.clone()),
            (Some(GenerationSource::Layer(7)), vec![GenerationReference { source: GenerationSource::Layer(8), name: "Luna".into() }])
        );
        assert_eq!(Generation { source: None, references: vec![], ..g }, Generation { source: None, references: vec![], ..generation() });
        // Writing again replaces it; a layer with no generation loses it and keeps the rest.
        put(&mut raw, &layer(Some(Generation { prompt: "x".into(), ..Default::default() })), &ids);
        assert_eq!((parse_shmd(&raw[0].1).unwrap().len(), decode(&item(&raw).unwrap().data).unwrap().prompt.as_str()), (2, "x"));
        put(&mut raw, &layer(None), &ids);
        assert_eq!(parse_shmd(&raw[0].1).unwrap(), vec![MetadataItem::new(*b"cmls", vec![1, 2])]);
        // Alone in its block, the block goes with it; without one, none is made.
        let mut raw = vec![];
        put(&mut raw, &layer(None), &ids);
        assert!(raw.is_empty());
        put(&mut raw, &layer(Some(generation())), &ids);
        assert_eq!(raw.len(), 1);
        put(&mut raw, &layer(None), &ids);
        assert!(raw.is_empty());
    }

    #[test]
    fn a_block_that_cannot_be_read_is_left_alone() {
        let mut raw = vec![(*b"shmd", vec![0, 0, 0, 9])];
        put(&mut raw, &layer(Some(generation())), &HashMap::new());
        assert_eq!(raw, vec![(*b"shmd", vec![0, 0, 0, 9])]);
    }

    #[test]
    fn hostile_item_data_gives_no_generation() {
        for data in [&b""[..], b"\0\0", b"{", b"[1,2]", b"\xff\xfe", br#"{"mode":"nope"}"#, br#"{"seed":-1}"#, br#"{"references":7}"#] {
            assert_eq!(decode(data), None, "{data:?}");
        }
        assert_eq!(decode(&vec![b' '; MAX_LEN + 1]), None);
        // Zero padding is read past; numbers out of range are brought back into it.
        let g = decode(b"{\"prompt\":\"p\",\"strength\":7.5,\"context\":-3}\0").unwrap();
        assert_eq!((g.prompt.as_str(), g.strength, g.context), ("p", 1.0, 0.0));
        // A layer whose block holds such an item imports without one.
        let mut l = layer(None);
        l.psd_blocks = vec![(*b"shmd", std::sync::Arc::new(write_shmd(&[MetadataItem::new(KEY, b"{".to_vec())])))];
        assert_eq!(stored(&l), None);
        l.psd_blocks = vec![(*b"shmd", std::sync::Arc::new(vec![1, 2, 3]))];
        assert_eq!(stored(&l), None);
    }
}
