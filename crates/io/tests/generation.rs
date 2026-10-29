//! What a generative-AI layer was made from survives PSD, PSB and layered TIFF: the `pcGn` item
//! of the layer's `shmd` block (`photocraft_io::generation_map`).

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Generation, GenerationMode, GenerationQuality, GenerationReference, GenerationSource, Layer, Size};
use photocraft_io::{ExportOptions, export, generation_map, import};
use photocraft_psd::PsdFile;
use photocraft_psd::metadata::{MetadataItem, parse_shmd, write_shmd};

fn document(depth: SampleType) -> Document {
    let mut doc = Document::new("Generated", Size::new(4, 4), ColorMode::Rgb, depth);
    let fmt = doc.pixel_format();
    let photo = Layer::raster("Photo", fmt);
    let other = Layer::raster("Other", fmt);
    let (photo_id, other_id) = (photo.id.0, other.id.0);
    let mut txt = Layer::raster("[z] a green field", fmt);
    txt.generation = Some(Box::new(Generation {
        mode: GenerationMode::Txt2img,
        model: "z-image-turbo".into(),
        model_name: "Z Image Turbo".into(),
        prompt: "a green field, \"wide\"\nsecond line \u{1F33E}".into(),
        strength: 1.0,
        context: 0.25,
        alpha: true,
        seed: 0xFFFF_FFFF_FFFF,
        ..Default::default()
    }));
    let mut edit = Layer::raster("[q] make it night", fmt);
    edit.generation = Some(Box::new(Generation {
        mode: GenerationMode::ImgEdit,
        model: "qwen-edit".into(),
        model_name: "Qwen Edit".into(),
        prompt: "make it night".into(),
        strength: 0.6,
        context: 0.5,
        alpha: false,
        seed: 7,
        source: Some(GenerationSource::Layer(photo_id)),
        // Another open document means nothing in a file; a layer that is gone is not named.
        references: vec![
            GenerationReference { source: GenerationSource::Layer(other_id), name: "Luna".into() },
            GenerationSource::Document(3).into(),
            GenerationSource::Layer(u64::MAX).into(),
        ],
        quality: GenerationQuality::Draft,
    }));
    // In a group: the walk reaches it.
    doc.layers = vec![photo, other, txt, Layer::group("Group", vec![edit])];
    doc
}

fn layer<'a>(doc: &'a Document, name: &str) -> &'a Layer {
    doc.walk().into_iter().map(|(_, _, l)| l).find(|l| l.name == name).unwrap()
}

/// What `doc`'s generations should read as once it has been through a file.
fn check(back: &Document, original: &Document) {
    assert_eq!(layer(back, "[z] a green field").generation, layer(original, "[z] a green field").generation);
    let edit = *layer(back, "[q] make it night").generation.clone().unwrap();
    // The source and the reference name the layers the file gave new ids.
    assert_eq!(edit.source, Some(GenerationSource::Layer(layer(back, "Photo").id.0)));
    // A reference keeps its name in the file.
    assert_eq!(edit.references, vec![GenerationReference { source: GenerationSource::Layer(layer(back, "Other").id.0), name: "Luna".into() }]);
    let was = *layer(original, "[q] make it night").generation.clone().unwrap();
    assert_eq!(Generation { source: None, references: vec![], ..edit }, Generation { source: None, references: vec![], ..was });
    assert_eq!((&layer(back, "Photo").generation, &layer(back, "Group").generation), (&None, &None));
}

#[test]
fn generations_survive_psd_psb_and_layered_tiff() {
    for depth in SampleType::ALL {
        let doc = document(depth);
        for name in ["x.psd", "x.psb", "x.tif"] {
            let opts = ExportOptions { tiff_layers: true, ..Default::default() };
            let bytes = export(&doc, name, &opts).unwrap().bytes;
            let back = import(name, &bytes).unwrap().document;
            check(&back, &doc);
            // And through a second save, from the imported document.
            let again = import(name, &export(&back, name, &opts).unwrap().bytes).unwrap().document;
            check(&again, &doc);
        }
    }
}

#[test]
fn the_item_sits_in_a_well_formed_shmd_block_other_readers_step_over() {
    let doc = document(SampleType::U8);
    let bytes = export(&doc, "x.psd", &ExportOptions::default()).unwrap().bytes;
    let file = PsdFile::from_bytes(&bytes).unwrap();
    assert_eq!(file.to_bytes().unwrap(), bytes, "byte-stable through the psd crate");
    let mut with = 0;
    for rec in file.layers() {
        let Some(block) = rec.block(b"shmd") else { continue };
        // Every item is a signed, keyed, even-length record, as the PSD spec lays `shmd` out.
        let items = parse_shmd(&block.data).unwrap();
        for it in &items {
            assert_eq!((&it.signature, it.data.len() % 2), (b"8BIM", 0));
        }
        with += items.iter().filter(|it| it.key == generation_map::KEY).count();
    }
    assert_eq!(with, 2, "only the generated layers carry the item");
}

#[test]
fn clearing_a_generation_removes_the_item_and_a_broken_one_is_ignored() {
    let doc = document(SampleType::U8);
    let opts = ExportOptions::default();
    let mut back = import("x.psd", &export(&doc, "x.psd", &opts).unwrap().bytes).unwrap().document;
    // The imported block still holds the item; the field is what is written.
    let id = layer(&back, "[z] a green field").id;
    back.layer_mut(id).unwrap().generation = None;
    let again = import("x.psd", &export(&back, "x.psd", &opts).unwrap().bytes).unwrap().document;
    assert_eq!(layer(&again, "[z] a green field").generation, None);
    assert!(layer(&again, "[q] make it night").generation.is_some());
    // Items that are not a generation (truncated JSON, another type, nothing at all): the file
    // opens, and the layer has none.
    for data in [&b"{\"prompt\":"[..], b"[]", b"", b"\xff\xff"] {
        let mut broken = document(SampleType::U8);
        let l = broken.layers.get_mut(0).unwrap();
        l.psd_blocks = vec![(*b"shmd", std::sync::Arc::new(write_shmd(&[MetadataItem::new(generation_map::KEY, data.to_vec())])))];
        let back = import("x.psd", &export(&broken, "x.psd", &opts).unwrap().bytes).unwrap().document;
        assert_eq!(layer(&back, "Photo").generation, None, "{data:?}");
    }
}

#[test]
fn the_documents_ai_references_survive_psd_and_psb() {
    use photocraft_doc::{AiPicture, AiReference};
    for depth in SampleType::ALL {
        let mut doc = document(depth);
        let other = layer(&doc, "Other").id.0;
        let png = std::sync::Arc::new(b"\x89PNG\r\n\x1a\n a copied picture".to_vec());
        doc.ai_references = vec![
            AiReference { name: "Luna".into(), picture: AiPicture::Layer(other) },
            AiReference { name: String::new(), picture: AiPicture::Document },
            AiReference { name: "Max".into(), picture: AiPicture::Image { png: png.clone(), label: "Layer: Max (Other.psd)".into() } },
        ];
        for name in ["x.psd", "x.psb"] {
            let opts = ExportOptions::default();
            let back = import(name, &export(&doc, name, &opts).unwrap().bytes).unwrap().document;
            // The layer is named by the id the file gave it; the copy comes back byte for byte.
            let expected = vec![
                AiReference { name: "Luna".into(), picture: AiPicture::Layer(layer(&back, "Other").id.0) },
                doc.ai_references[1].clone(),
                doc.ai_references[2].clone(),
            ];
            assert_eq!(back.ai_references, expected, "{name} {depth:?}");
            assert!(!back.metadata.psd_resources.iter().any(|(id, _, _)| *id == photocraft_io::ai_refs_map::AI_REFERENCES), "the field, not a raw copy");
            // A second save writes them once, from the field.
            let again = import(name, &export(&back, name, &opts).unwrap().bytes).unwrap().document;
            assert_eq!(again.ai_references.len(), 3);
            assert_eq!(again.ai_references[2], expected[2]);
            // None kept: no resource.
            let mut none = back.clone();
            none.ai_references.clear();
            let bytes = export(&none, name, &opts).unwrap().bytes;
            assert!(import(name, &bytes).unwrap().document.ai_references.is_empty());
            let file = PsdFile::from_bytes(&bytes).unwrap();
            assert!(!file.resources.iter().any(|r| r.id == photocraft_io::ai_refs_map::AI_REFERENCES));
        }
    }
}
