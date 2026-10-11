use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Generation, GenerationMode, GenerationSource, Layer, Size};
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
    }
}

#[test]
fn a_layers_generation_survives_a_pcraft_round_trip() {
    let mut doc = Document::new("Generated", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let source = Layer::raster("Source", doc.pixel_format());
    let source_id = source.id.0;
    let mut plain = Layer::raster("From a prompt", doc.pixel_format());
    plain.generation = Some(Generation { mode: GenerationMode::Inpaint, ..generated(None) });
    let mut child = Layer::raster("Redrawn", doc.pixel_format());
    child.generation = Some(generated(Some(GenerationSource::Layer(source_id))));
    doc.layers.extend([source, plain, Layer::group("Group", vec![child])]);
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
    redrawn.generation = Some(generated(Some(GenerationSource::Layer(source.id.0))));
    let mut gone = Layer::raster("Its source is gone", doc.pixel_format());
    gone.generation = Some(generated(Some(GenerationSource::Layer(u64::MAX))));
    let mut other = Layer::raster("From another document", doc.pixel_format());
    other.generation = Some(generated(Some(GenerationSource::Document(7))));
    doc.layers.extend([source, Layer::group("Group", vec![redrawn]), gone, other]);
    let bytes = photocraft_format::save_to_bytes(&doc, &Default::default()).unwrap();
    let loaded = photocraft_format::load_from_bytes_with(&bytes, &LoadOptions { preserve_ids: false, ..Default::default() }).unwrap();
    let source_id = loaded.layers[0].id.0;
    assert_ne!(source_id, doc.layers[0].id.0, "the ids are fresh");
    let of = |l: &Layer| l.generation.clone().unwrap();
    assert_eq!(of(&loaded.layers[1].children().unwrap()[0]), generated(Some(GenerationSource::Layer(source_id))));
    // A layer that is not in the file, and a document id (which only lasted for its session), are
    // forgotten; the rest of the generation stays.
    assert_eq!(of(&loaded.layers[2]), generated(None));
    assert_eq!(of(&loaded.layers[3]), generated(None));
}
