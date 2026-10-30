//! The AI Gen panel's regions ([`photocraft_doc::Document::ai_regions`]) in PSD/PSB files: one
//! image resource of PhotoCraft's own, beside the references' ([`crate::ai_refs_map`]), in the
//! range the PSD specification leaves to plug-ins (4000–4999), which other readers step over.
//!
//! The data: the signature `pcRG`, a version (u16, 1), then the regions as JSON (UTF-8), front to
//! back, as `.pcraft` keeps them. Regions name no layers, so nothing is remapped.

use photocraft_doc::{AiRegion, Document};

/// The image resource id.
pub const AI_REGIONS: u16 = 4001;
/// The resource's name, for a reader that lists resources.
pub const NAME: &str = "PhotoCraft AI regions";
const SIGNATURE: &[u8; 4] = b"pcRG";
const VERSION: u16 = 1;
/// More than the panel offers: a file that holds more is not read.
pub const MAX_COUNT: usize = 256;
/// A resource larger than this is not read (each region is a few hundred bytes).
const MAX_BYTES: usize = 4 << 20;

/// Is `data` (of a resource with id `id`) the regions resource, rather than a plug-in's?
pub fn is_ours(id: u16, data: &[u8]) -> bool {
    id == AI_REGIONS && data.starts_with(SIGNATURE)
}

/// Append the regions resource for `doc` to `resources`, when it has regions.
pub(crate) fn push(doc: &Document, resources: &mut Vec<photocraft_psd::resources::ImageResource>) {
    if let Some(data) = encode(doc) {
        let mut r = photocraft_psd::resources::ImageResource::new(AI_REGIONS, data);
        r.name = crate::psd_export::legacy_name(NAME);
        resources.push(r);
    }
}

/// The resource data for `doc`'s regions; `None` when it has none.
pub fn encode(doc: &Document) -> Option<Vec<u8>> {
    if doc.ai_regions.is_empty() {
        return None;
    }
    let json = serde_json::to_vec(&doc.ai_regions).ok()?;
    let mut out = Vec::with_capacity(6 + json.len());
    out.extend_from_slice(SIGNATURE);
    out.extend(VERSION.to_be_bytes());
    out.extend(json);
    Some(out)
}

/// The regions in resource `data`; `None` for data that is not one or can't be read.
pub fn decode(data: &[u8]) -> Option<Vec<AiRegion>> {
    if data.len() > MAX_BYTES {
        return None;
    }
    let rest = data.strip_prefix(SIGNATURE)?;
    let (version, json) = rest.split_at_checked(2)?;
    if version != VERSION.to_be_bytes() {
        return None;
    }
    let regions: Vec<AiRegion> = serde_json::from_slice(json).ok()?;
    (regions.len() <= MAX_COUNT).then_some(regions)
}

/// Gives an imported document the regions its resource holds, and takes the raw resource out of
/// the ones kept for export (export writes it afresh from the field). A resource that can't be
/// read stays as it came.
pub(crate) fn import(doc: &mut Document) {
    let Some(at) = doc.metadata.psd_resources.iter().position(|(id, _, data)| is_ours(*id, data)) else { return };
    let Some(found) = doc.metadata.psd_resources.get(at).and_then(|(_, _, data)| decode(data)) else { return };
    doc.ai_regions = found;
    doc.metadata.psd_resources.remove(at);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use photocraft_doc::{AiRegionKind, ColorMode, Rect, SampleType, Size};

    fn doc() -> Document {
        let mut d = Document::new("D", Size::new(4, 4), ColorMode::Rgb, SampleType::U8);
        d.ai_regions = vec![
            AiRegion { id: 2, kind: AiRegionKind::Text, caption: "cream serif \u{1F986}".into(), text: "READ \"MORE\"".into(), rect: Rect::new(1, 2, 3, 4), ..AiRegion::default() },
            AiRegion { id: 1, caption: "armchair".into(), rect: Rect::new(-5, -5, 900, 700), fill: false, ..AiRegion::default() },
        ];
        d
    }

    #[test]
    fn regions_round_trip_and_are_imported_in_place_of_the_raw_resource() {
        let mut d = doc();
        let data = encode(&d).unwrap();
        assert!(is_ours(AI_REGIONS, &data) && !is_ours(crate::ai_refs_map::AI_REFERENCES, &data) && !is_ours(AI_REGIONS, b"8BIMxxxx"));
        assert_eq!(decode(&data).unwrap(), d.ai_regions);
        let expected = d.ai_regions.clone();
        d.ai_regions.clear();
        d.metadata.psd_resources.push((4002, String::new(), Arc::new(b"plug-in".to_vec())));
        d.metadata.psd_resources.push((AI_REGIONS, NAME.into(), Arc::new(data)));
        import(&mut d);
        assert_eq!(d.ai_regions, expected);
        assert_eq!(d.metadata.psd_resources.len(), 1, "the plug-in's resource stays");
        assert_eq!(encode(&Document::new("E", Size::new(1, 1), ColorMode::Rgb, SampleType::U8)), None);
    }

    #[test]
    fn hostile_data_is_no_regions_and_never_a_panic() {
        let good = encode(&doc()).unwrap();
        for cut in 0..good.len() {
            assert!(decode(good.get(..cut).unwrap()).is_none(), "{cut}");
        }
        assert_eq!(decode(b"pcRG\0\x02[]"), None, "another version");
        assert_eq!(decode(b"pcRG\0\x01{\"not\":\"a list\"}"), None);
        let many = format!("pcRG\0\x01[{}]", vec!["{}"; MAX_COUNT + 1].join(","));
        assert_eq!(decode(many.as_bytes()), None);
        assert_eq!(decode(b"pcRG\0\x01[{\"rect\":{\"x0\":1e99}}]"), None, "a number out of range");
        // Missing fields take their defaults.
        assert_eq!(decode(b"pcRG\0\x01[{\"caption\":\"x\"}]").unwrap()[0].caption, "x");
        let mut d = doc();
        d.ai_regions.clear();
        d.metadata.psd_resources.push((AI_REGIONS, NAME.into(), Arc::new(b"pcRG\0\x01[".to_vec())));
        import(&mut d);
        assert!(d.ai_regions.is_empty() && d.metadata.psd_resources.len() == 1);
    }
}
