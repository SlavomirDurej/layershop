use craft_psd::PsdFile;
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, LayerMask};
use photocraft_io::{ExportOptions, export, import, linked, smart_map};
use photocraft_raster::Surface;

use super::*;

fn fixture(bounds: Rect, grouped: bool) -> (Session, LayerId) {
    let mut doc = Document::new("Upscale fixture", Size::new(3000, 2000), ColorMode::Rgb, SampleType::U8);
    let mut pixels = Surface::new(doc.pixel_format());
    pixels.fill_rect(bounds, &[0.1, 0.2, 0.3, 1.0]);
    let source = Layer::new("Original", LayerContent::Raster(pixels));
    let id = source.id;
    let above = Layer::raster("Existing sibling", doc.pixel_format());
    doc.layers = if grouped { vec![Layer::group("Group", vec![source, above])] } else { vec![source, above] };
    let mut session = Session::new();
    session.add_document(doc, None);
    session.select_layer(id).unwrap();
    (session, id)
}

fn png(size: Size) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size.width as usize * size.height as usize * 4);
    for y in 0..size.height {
        for x in 0..size.width {
            let band = match x * 5 / size.width {
                0 => 0,
                1 => 64,
                2 => 128,
                3 => 192,
                _ => 255,
            };
            let ramp = (size.height / 4).max(1);
            let soft_edge = y.saturating_sub(ramp).min(ramp);
            let alpha = 255 - (255 - band) * soft_edge / ramp;
            bytes.extend([255, 0, 0, alpha as u8]);
        }
    }
    let image = Image::from_u8(size.width, size.height, ChannelLayout::Rgba, bytes).unwrap();
    photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap()
}

fn smart(layer: &Layer) -> &SmartObject {
    let LayerContent::Smart(smart) = &layer.content else { panic!("expected a smart object") };
    smart
}

fn embedded(smart: &SmartObject) -> (&str, &Arc<Vec<u8>>) {
    let SmartSource::Embedded { file_name, bytes } = &smart.source else { panic!("expected embedded image bytes") };
    (file_name, bytes)
}

fn place(session: &mut Session, source: LayerId, bytes: Vec<u8>, size: Size, factor: u32) -> LayerId {
    session.place_upscaled_raster(source, bytes, size, factor, &JobCtx::new()).unwrap()
}

#[test]
fn placements_preserve_source_footprint_and_full_resolution() {
    for (bounds, factor, grouped) in [
        (Rect::from_xywh(400, 300, 1024, 1024), 2, false),
        (Rect::from_xywh(400, 300, 1024, 1024), 4, false),
        (Rect::from_xywh(2250, 1520, 800, 600), 4, false),
        (Rect::from_xywh(-11, -9, 37, 29), 4, true),
    ] {
        let (mut session, source) = fixture(bounds, grouped);
        let original = session.active().unwrap().doc.clone();
        let size = Size::new(bounds.width() * factor, bounds.height() * factor);
        let bytes = png(size);
        let id = place(&mut session, source, bytes.clone(), size, factor);
        let state = session.active().unwrap();
        assert_eq!(state.selected_layers(), vec![id]);
        assert_eq!(state.doc.size, original.size);
        assert_eq!(state.doc.layer(source), original.layer(source));
        assert_eq!(state.doc.path_of(source), Some(if grouped { vec![0, 0] } else { vec![0] }));
        assert_eq!(state.doc.path_of(id), Some(if grouped { vec![0, 1] } else { vec![1] }));
        let smart = smart(state.doc.layer(id).unwrap());
        let k = 1.0 / f64::from(factor);
        assert_eq!(smart.transform.m, [k, 0.0, 0.0, k, f64::from(bounds.x0), f64::from(bounds.y0)]);
        assert_eq!(smart.cache.as_ref().unwrap().content_bounds(), bounds);
        let (_, actual) = embedded(smart);
        assert_eq!(actual.as_slice(), bytes.as_slice());
        assert_eq!(crate::memory_gate::png_dimensions(actual), Some((size.width, size.height)));
    }
}

#[test]
fn undo_and_redo_restore_only_the_new_layer_and_selection() {
    let (mut session, source) = fixture(Rect::from_xywh(-3, 4, 35, 21), true);
    let before = session.active().unwrap().doc.clone();
    let size = Size::new(70, 42);
    let id = place(&mut session, source, png(size), size, 2);
    let after = session.active().unwrap().doc.clone();
    assert!(session.undo());
    let state = session.active().unwrap();
    assert_eq!(state.doc.layers, before.layers);
    assert_eq!(state.selected_layers(), vec![source]);
    assert!(!session.undo(), "placement must create exactly one history step");
    assert!(session.redo());
    let state = session.active().unwrap();
    assert_eq!(state.doc.layers, after.layers);
    assert_eq!(state.selected_layers(), vec![id]);
    let bytes = embedded(smart(state.doc.layer(id).unwrap())).1;
    assert!(Arc::ptr_eq(bytes, embedded(smart(after.layer(id).unwrap())).1));
    assert!(!session.redo());
}

#[test]
fn invalid_or_cancelled_placement_is_atomic() {
    let (mut session, source) = fixture(Rect::from_xywh(4, 3, 11, 9), false);
    let before = session.active().unwrap().doc.clone();
    let size = Size::new(22, 18);
    let ctx = JobCtx::new();
    let unknown = Layer::raster("missing", before.pixel_format()).id;
    let bytes = png(size);
    let broken = bytes[..33].to_vec(); // Valid dimensions in IHDR, missing image data.
    for (id, bytes, expected, factor, message) in [
        (unknown, bytes.clone(), size, 2, "no longer exists"),
        (source, b"not an image".to_vec(), size, 2, "valid PNG"),
        (source, png(Size::new(20, 18)), size, 2, "expected 22"),
        (source, bytes.clone(), Size::new(20, 18), 2, "Expected 22"),
        (source, bytes.clone(), size, 3, "must be 2 or 4"),
    ] {
        let error = session.place_upscaled_raster(id, bytes, expected, factor, &ctx).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
    assert!(session.place_upscaled_raster(source, broken, size, 2, &ctx).is_err());
    ctx.cancel();
    assert!(matches!(session.place_upscaled_raster(source, bytes, size, 2, &ctx), Err(EngineError::Cancelled)));
    let state = session.active().unwrap();
    assert!(Arc::ptr_eq(&state.doc, &before));
    assert_eq!(state.selected_layers(), vec![source]);
    assert!(!session.undo());
}

#[test]
fn extreme_dimensions_fail_before_decoding() {
    assert!(check_dimensions(Rect::new(i32::MIN, 0, i32::MAX, 1), Size::new(1, 2), 2).unwrap_err().to_string().contains("overflows"));
    assert!(check_dimensions(Rect::from_xywh(0, 0, 20_000, 20_000), Size::new(80_000, 80_000), 4).unwrap_err().to_string().contains("render limit"));
    assert!(check_dimensions(Rect::from_xywh(0, 0, 150_000, 1), Size::new(300_000, 2), 2).unwrap_err().to_string().contains("render limit"));
}

#[test]
fn clipping_and_multi_selection_are_rejected() {
    let (mut session, source) = fixture(Rect::from_xywh(0, 0, 9, 7), false);
    let size = Size::new(18, 14);
    let sibling = session.active().unwrap().doc.layers[1].id;
    session.active_mut().unwrap().selected_layers = vec![source, sibling];
    assert!(session.place_upscaled_raster(source, Vec::new(), size, 2, &JobCtx::new()).unwrap_err().to_string().contains("single raster"));
    session.select_layer(source).unwrap();
    session
        .edit("Fixture clipping", |doc, _| {
            doc.layers[1].clipped = true;
            Ok(())
        })
        .unwrap();
    assert!(session.place_upscaled_raster(source, Vec::new(), size, 2, &JobCtx::new()).unwrap_err().to_string().contains("clipping"));
    assert!(session.place_upscaled_raster(sibling, Vec::new(), size, 2, &JobCtx::new()).unwrap_err().to_string().contains("clipping"));
}

#[test]
fn non_raster_empty_and_unsupported_blending_sources_are_rejected() {
    let (mut session, source) = fixture(Rect::from_xywh(0, 0, 9, 7), true);
    let group = session.active().unwrap().doc.layers[0].id;
    let empty = session.active().unwrap().doc.layers[0].children().unwrap()[1].id;
    let size = Size::new(18, 14);
    let ctx = JobCtx::new();
    for (id, message) in [(group, "must be a raster layer"), (empty, "has no pixels")] {
        let error = session.place_upscaled_raster(id, Vec::new(), size, 2, &ctx).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
    let result = place(&mut session, source, png(size), size, 2);
    let error = session.place_upscaled_raster(result, Vec::new(), size, 2, &ctx).unwrap_err();
    assert!(error.to_string().contains("must be a raster layer"));

    session
        .edit("Fixture blending", |doc, _| {
            doc.layer_mut(source).unwrap().blend = BlendMode::Multiply;
            Ok(())
        })
        .unwrap();
    let before = session.active().unwrap().doc.clone();
    let error = session.place_upscaled_raster(source, Vec::new(), size, 2, &ctx).unwrap_err();
    assert!(error.to_string().contains("unsupported blending"));
    assert!(Arc::ptr_eq(&session.active().unwrap().doc, &before));
}

#[test]
fn cache_keeps_soft_alpha_and_composites_against_light_and_dark() {
    let bounds = Rect::from_xywh(0, 0, 35, 24);
    let (mut session, source) = fixture(bounds, false);
    let size = Size::new(140, 96);
    let id = place(&mut session, source, png(size), size, 4);
    let doc = &session.active().unwrap().doc;
    let cache = smart(doc.layer(id).unwrap()).cache.as_ref().unwrap();
    for (x, alpha) in [(3, 0.0), (10, 64.0 / 255.0), (17, 128.0 / 255.0), (24, 192.0 / 255.0), (31, 1.0)] {
        assert!((cache.sample_channel(x, 16, 3) - alpha).abs() < 0.01);
    }
    let soft_alpha = cache.sample_channel(3, 9, 3);
    assert!((0.25..0.75).contains(&soft_alpha));
    for background in [0.0, 1.0] {
        let mut sample = Document::new("Composite", bounds.size(), ColorMode::Rgb, SampleType::U8);
        let mut backdrop = Surface::new(sample.pixel_format());
        backdrop.fill_rect(bounds, &[background, background, background, 1.0]);
        sample.layers.push(Layer::new("Backdrop", LayerContent::Raster(backdrop)));
        sample.layers.push(doc.layer(id).unwrap().clone());
        let actual = photocraft_compose::flatten_to_surface(&sample, sample.pixel_format(), None);
        assert!((actual.sample_channel(3, 16, 1) - background).abs() < 0.01);
        assert!((actual.sample_channel(3, 9, 1) - background * (1.0 - soft_alpha)).abs() < 0.02);
        assert!((actual.sample_channel(17, 16, 1) - background * (1.0 - 128.0 / 255.0)).abs() < 0.02);
        assert!(actual.sample_channel(17, 16, 0) > actual.sample_channel(17, 16, 1));
    }
}

#[test]
fn native_and_psd_keep_full_embedded_pixels_transform_and_mask() {
    let bounds = Rect::from_xywh(-7, 9, 35, 21);
    let (mut session, source) = fixture(bounds, false);
    let mut mask = LayerMask::reveal_all();
    mask.surface.fill_rect(Rect::from_xywh(-5, 11, 9, 8), &[0.25]);
    session
        .edit("Fixture mask", |doc, _| {
            doc.layer_mut(source).unwrap().mask = Some(mask.clone());
            Ok(())
        })
        .unwrap();
    let size = Size::new(140, 84);
    let bytes = png(size);
    let id = place(&mut session, source, bytes.clone(), size, 4);
    let doc = &session.active().unwrap().doc;
    let transform = smart(doc.layer(id).unwrap()).transform;
    assert_eq!(doc.layer(id).unwrap().mask.as_ref(), Some(&mask));
    let mut edited = doc.layer(id).unwrap().clone();
    edited.mask.as_mut().unwrap().surface.fill_rect(Rect::from_xywh(-4, 12, 1, 1), &[0.0]);
    assert_eq!(edited.mask.as_ref().unwrap().value(-4, 12), 0.0);
    assert_eq!(doc.layer(source).unwrap().mask.as_ref().unwrap().value(-4, 12), mask.value(-4, 12));
    let native = photocraft_format::save_to_bytes(doc, &Default::default()).unwrap();
    let psd = export(doc, "psd", &ExportOptions::default()).unwrap();
    assert!(psd.warnings.is_empty(), "{:?}", psd.warnings);
    // Optional fixture for validating this exact export with an independent reader.
    if let Some(path) = std::env::var_os("PHOTOCRAFT_UPSCALE_PSD_FIXTURE") {
        std::fs::write(path, &psd.bytes).unwrap();
    }
    let parsed = PsdFile::from_bytes(&psd.bytes).unwrap();
    let layer = parsed.layers().iter().find(|l| l.name() == "Upscaled - Original (4x)").unwrap();
    assert!(layer.block(b"PlLd").is_some());
    let placed = smart_map::parse_sold(&layer.block(b"SoLd").unwrap().data).unwrap();
    assert_eq!(placed.transform, transform);
    assert_eq!(smart_map::stored_size(&placed.descriptor), Some((140.0, 84.0)));
    let lnk2 = parsed.global_blocks.iter().find(|b| &b.key == b"lnk2").unwrap();
    let files = linked::parse_linked_files(&lnk2.data);
    let payload = files.iter().find(|file| file.uuid == placed.idnt).unwrap();
    assert_eq!(payload.bytes.as_slice(), bytes.as_slice());

    let dir = tempfile::tempdir().unwrap();
    let native_path = dir.path().join("roundtrip.pcraft");
    let psd_path = dir.path().join("roundtrip.psd");
    std::fs::write(&native_path, native).unwrap();
    std::fs::write(&psd_path, psd.bytes).unwrap();
    session.close(0).unwrap();
    assert!(session.active().is_none());
    let native = photocraft_format::load_path(&native_path).unwrap();
    let psd = import("roundtrip.psd", &std::fs::read(psd_path).unwrap()).unwrap().document;
    for reopened in [native, psd] {
        assert_reopened(&reopened, bounds, &mask, size, &bytes, transform);
    }
}

fn assert_reopened(reopened: &Document, bounds: Rect, mask: &LayerMask, size: Size, bytes: &[u8], transform: Affine) {
    assert_eq!(reopened.size, Size::new(3000, 2000));
    let [original, upscaled, sibling] = reopened.layers.as_slice() else { panic!("expected three layers") };
    assert_eq!(original.name, "Original");
    assert!(matches!(original.content, LayerContent::Raster(_)));
    let pixels = original.surface().unwrap();
    assert_eq!(pixels.content_bounds(), bounds);
    assert!((pixels.sample_channel(-6, 10, 0) - 0.1).abs() < 0.01);
    assert_eq!(pixels.sample_channel(-6, 10, 3), 1.0);
    assert_eq!(upscaled.name, "Upscaled - Original (4x)");
    assert_eq!(sibling.name, "Existing sibling");
    for layer in [original, upscaled] {
        let restored = layer.mask.as_ref().unwrap();
        assert_eq!(restored.surface.content_bounds(), mask.surface.content_bounds());
        assert_eq!(restored.value(-4, 12), mask.value(-4, 12));
        assert_eq!(restored.value(40, 12), 1.0);
    }
    let smart = smart(upscaled);
    assert_eq!(smart.transform, transform);
    let cache = smart.cache.as_ref().unwrap();
    assert_eq!(cache.content_bounds(), bounds);
    assert!((cache.sample_channel(10, 24, 3) - 128.0 / 255.0).abs() < 0.01);
    // PSD imports its embedded source as an ID into the document's lnk2 data.
    let (name, actual) = crate::smart_cmds::source_bytes(&reopened.metadata, &smart.source).unwrap();
    assert_eq!(actual.as_slice(), bytes);
    let decoded = crate::smart_cmds::decode_source(&name, &actual).unwrap();
    assert_eq!(decoded.size, size);
    let pixels = decoded.layers[0].surface().unwrap();
    assert_eq!(pixels.sample_channel(5, 60, 3), 0.0);
    assert!((pixels.sample_channel(70, 60, 3) - 128.0 / 255.0).abs() < 0.01);
    assert_eq!(pixels.sample_channel(135, 60, 3), 1.0);
    let mut full = smart.clone();
    full.transform = Affine::IDENTITY;
    let rendered = crate::smart_cmds::render(reopened, &full).unwrap().unwrap();
    assert_eq!(rendered.content_bounds().size(), size);
}
