use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{AiRegion, AiRegionKind, Document, Generation, GenerationMode, GenerationQuality, GenerationReference, GenerationSource, Layer, Rect, Size};
use photocraft_format::LoadOptions;

fn generated(source: Option<GenerationSource>) -> Generation {
    Generation {
        mode: GenerationMode::Img2img,
        model: "flux".into(),
        model_name: "FLUX.2 Klein".into(),
        prompt: "a deer\neating \u{201c}grass\u{201d}".into(),
        strength: 0.35,
        context: 0.5,
        alpha: true,
        seed: u64::MAX >> 16,
        source,
        references: Vec::new(),
        quality: GenerationQuality::Detail,
        regions: Vec::new(),
    }
}

fn region(id: u32, kind: AiRegionKind) -> AiRegion {
    AiRegion {
        id,
        kind,
        caption: "Ginger cat \"asleep\"\non the cushion".into(),
        text: if kind == AiRegionKind::Text { "READ MORE".into() } else { String::new() },
        rect: Rect::new(-10, 4, 300, 220),
        color: [1, 2, 3],
        fill: id.is_multiple_of(2),
        fill_opacity: 0.25,
    }
}

#[test]
fn the_documents_regions_and_a_layers_layout_survive_a_pcraft_round_trip() {
    let mut doc = Document::new("Laid out", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    doc.ai_regions = vec![region(3, AiRegionKind::Character), region(1, AiRegionKind::Object), region(2, AiRegionKind::Text)];
    let mut made = Layer::raster("From a layout", doc.pixel_format());
    made.generation = Some(Box::new(Generation { mode: GenerationMode::Txt2img, regions: doc.ai_regions[..2].to_vec(), ..generated(None) }));
    doc.layers.push(made);
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    assert_eq!(photocraft_format::load_from_bytes(&bytes).unwrap(), doc);
    // A file saved before regions existed loads with none (and a layer's generation without them).
    let old: Generation = serde_json::from_str(r#"{"mode":"txt2img","prompt":"x"}"#).unwrap();
    assert!(old.regions.is_empty());
    assert!(serde_json::to_value(&old).unwrap().get("regions").is_none(), "an empty layout is not written");
}

#[test]
fn a_layers_generation_survives_a_pcraft_round_trip() {
    let mut doc = Document::new("Generated", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let source = Layer::raster("Source", doc.pixel_format());
    let source_id = source.id.0;
    let mut plain = Layer::raster("From a prompt", doc.pixel_format());
    plain.generation = Some(Box::new(Generation { mode: GenerationMode::Inpaint, ..generated(None) }));
    let mut child = Layer::raster("Redrawn", doc.pixel_format());
    child.generation = Some(Box::new(generated(Some(GenerationSource::Layer(source_id)))));
    // An edit's named reference keeps its name.
    let mut edited = Layer::raster("Edited", doc.pixel_format());
    let reference = GenerationReference { source: GenerationSource::Layer(source_id), name: "Luna".into() };
    edited.generation =
        Some(Box::new(Generation { mode: GenerationMode::ImgEdit, references: vec![reference], ..generated(Some(GenerationSource::Layer(source_id))) }));
    doc.layers.extend([source, plain, Layer::group("Group", vec![child]), edited]);
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    assert_eq!(photocraft_format::load_from_bytes(&bytes).unwrap(), doc);
    // A document without generated layers loads with none.
    let plain_doc = Document::new("Plain", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let plain_bytes = photocraft_format::save_to_bytes(&plain_doc, &Default::default()).unwrap();
    assert_eq!(photocraft_format::load_from_bytes(&plain_bytes).unwrap().layers.iter().filter(|l| l.generation.is_some()).count(), 0);
}

#[test]
fn a_generations_source_follows_its_layer_through_fresh_ids() {
    let mut doc = Document::new("Generated", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let source = Layer::raster("Source", doc.pixel_format());
    let mut redrawn = Layer::raster("Redrawn", doc.pixel_format());
    redrawn.generation = Some(Box::new(generated(Some(GenerationSource::Layer(source.id.0)))));
    let mut gone = Layer::raster("Its source is gone", doc.pixel_format());
    gone.generation = Some(Box::new(generated(Some(GenerationSource::Layer(u64::MAX)))));
    let mut other = Layer::raster("From another document", doc.pixel_format());
    other.generation = Some(Box::new(generated(Some(GenerationSource::Document(7)))));
    // References follow their layers too, keeping their names; another document's layer is
    // forgotten like the document.
    let mut edited = Layer::raster("Edited", doc.pixel_format());
    edited.generation = Some(Box::new(Generation {
        references: vec![
            GenerationReference { source: GenerationSource::DocumentLayer { document: 7, layer: source.id.0 }, name: "Max".into() },
            GenerationReference { source: GenerationSource::Layer(source.id.0), name: "Luna".into() },
        ],
        ..generated(None)
    }));
    doc.layers.extend([source, Layer::group("Group", vec![redrawn]), gone, other, edited]);
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    let loaded = photocraft_format::load_from_bytes_with(&bytes, &LoadOptions { preserve_ids: false, ..Default::default() }).unwrap();
    let source_id = loaded.layers[0].id.0;
    assert_ne!(source_id, doc.layers[0].id.0, "the ids are fresh");
    let of = |l: &Layer| *l.generation.clone().unwrap();
    assert_eq!(of(&loaded.layers[1].children().unwrap()[0]), generated(Some(GenerationSource::Layer(source_id))));
    // A layer that is not in the file, and a document id (which only lasted for its session), are
    // forgotten; the rest of the generation stays.
    assert_eq!(of(&loaded.layers[2]), generated(None));
    assert_eq!(of(&loaded.layers[3]), generated(None));
    assert_eq!(of(&loaded.layers[4]).references, [GenerationReference { source: GenerationSource::Layer(source_id), name: "Luna".into() }]);
}

#[test]
fn an_edits_references_follow_their_layers_too() {
    let mut doc = Document::new("Edited", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let shirt = Layer::raster("Shirt", doc.pixel_format());
    let mut edited = Layer::raster("Edited", doc.pixel_format());
    let references = vec![GenerationSource::Document(7).into(), GenerationSource::Layer(shirt.id.0).into(), GenerationSource::Layer(u64::MAX).into()];
    edited.generation = Some(Box::new(Generation { mode: GenerationMode::ImgEdit, references, ..generated(None) }));
    doc.layers.extend([shirt, edited]);
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    // A document id only lasted for its session and a layer that is not in the file is gone:
    // both are forgotten, the layer that is there is kept.
    let same = photocraft_format::load_from_bytes(&bytes).unwrap();
    assert_eq!(same.layers[1].generation.clone().unwrap().references, vec![GenerationSource::Layer(doc.layers[0].id.0).into()]);
    // With fresh ids that layer is followed to its new id.
    let loaded = photocraft_format::load_from_bytes_with(&bytes, &LoadOptions { preserve_ids: false, ..Default::default() }).unwrap();
    let g = loaded.layers[1].generation.clone().unwrap();
    assert_eq!((g.mode, g.quality), (GenerationMode::ImgEdit, GenerationQuality::Detail));
    assert_eq!(g.references, vec![GenerationSource::Layer(loaded.layers[0].id.0).into()]);
}

#[test]
fn the_documents_ai_references_survive_a_pcraft_round_trip() {
    use photocraft_doc::{AiPicture, AiReference};
    let mut doc = Document::new("Refs", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let luna = Layer::raster("Luna", doc.pixel_format());
    let luna_id = luna.id.0;
    doc.layers.push(luna);
    let png = std::sync::Arc::new(b"\x89PNG a copy".to_vec());
    doc.ai_references = vec![
        AiReference { name: "Luna".into(), picture: AiPicture::Layer(luna_id) },
        AiReference { name: "all".into(), picture: AiPicture::Document },
        AiReference { name: "Max".into(), picture: AiPicture::Image { png, label: "Layer: Max (Other)".into() } },
        AiReference { name: "gone".into(), picture: AiPicture::Layer(u64::MAX) },
    ];
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    let same = photocraft_format::load_from_bytes(&bytes).unwrap();
    // A layer that is not in the file goes; the rest come back as they were.
    assert_eq!(same.ai_references, doc.ai_references[..3]);
    // With fresh ids the layer is followed to its new id.
    let fresh = photocraft_format::load_from_bytes_with(&bytes, &LoadOptions { preserve_ids: false, ..Default::default() }).unwrap();
    assert_eq!(fresh.ai_references[0].picture, AiPicture::Layer(fresh.layers[0].id.0));
    assert_eq!(fresh.ai_references[1..], doc.ai_references[1..3]);
}
