//! The AI Gen panel's references ([`photocraft_doc::Document::ai_references`]) in PSD/PSB files:
//! one image resource of PhotoCraft's own, in the range the PSD specification leaves to plug-ins
//! (4000–4999), which other readers step over.
//!
//! The data: the signature `pcAR`, a version (u16, 1) and a count (u16), then per reference its
//! name, a kind byte and what that kind needs: a layer (0) by its PSD layer id (`lyid`, u32), the
//! document as it shows (1) with nothing more, or a copied picture (2) as its label and its PNG.
//! Strings and the PNG are a u32 length and the bytes. Numbers are big-endian, as in the rest of
//! the file. A reference whose layer is not in the file is not written.

use std::collections::HashMap;
use std::sync::Arc;

use photocraft_doc::{AiPicture, AiReference, Document, LayerId};

/// The image resource id.
pub const AI_REFERENCES: u16 = 4000;
/// The resource's name, for a reader that lists resources.
pub const NAME: &str = "PhotoCraft AI references";
const SIGNATURE: &[u8; 4] = b"pcAR";
const VERSION: u16 = 1;
/// More than the panel offers: a file that says it has more is not read.
const MAX_COUNT: usize = 64;

/// Is `data` (of a resource with id `id`) the references resource, rather than a plug-in's?
pub fn is_ours(id: u16, data: &[u8]) -> bool {
    id == AI_REFERENCES && data.starts_with(SIGNATURE)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    out.extend(u32::try_from(bytes.len()).ok()?.to_be_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

/// Append the references resource for `doc` to `resources`, when it has references to write.
pub(crate) fn push(doc: &Document, layer_ids: &HashMap<LayerId, u32>, resources: &mut Vec<photocraft_psd::resources::ImageResource>) {
    if let Some(data) = encode(doc, layer_ids) {
        let mut r = photocraft_psd::resources::ImageResource::new(AI_REFERENCES, data);
        r.name = crate::psd_export::legacy_name(NAME);
        resources.push(r);
    }
}

/// The resource data for `doc`'s references, naming layers by the PSD ids in `layer_ids`; `None`
/// when there are none to write.
pub fn encode(doc: &Document, layer_ids: &HashMap<LayerId, u32>) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut count: u16 = 0;
    for r in &doc.ai_references {
        let mut entry = Vec::new();
        put_bytes(&mut entry, r.name.as_bytes())?;
        match &r.picture {
            AiPicture::Layer(id) => {
                let Some(psd) = layer_ids.get(&LayerId(*id)) else { continue };
                entry.push(0);
                entry.extend(psd.to_be_bytes());
            }
            AiPicture::Document => entry.push(1),
            AiPicture::Image { png, label } => {
                entry.push(2);
                put_bytes(&mut entry, label.as_bytes())?;
                put_bytes(&mut entry, png)?;
            }
        }
        body.extend(entry);
        count = count.checked_add(1)?;
    }
    if count == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(SIGNATURE);
    out.extend(VERSION.to_be_bytes());
    out.extend(count.to_be_bytes());
    out.extend(body);
    Some(out)
}

/// A reader over resource data that never reads past its end.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let bytes = self.data.get(self.at..end)?;
        self.at = end;
        Some(bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([*b.first()?, *b.get(1)?]))
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_be_bytes([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?]))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.u32()?).ok()?;
        self.take(n)
    }

    fn text(&mut self) -> Option<String> {
        Some(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
}

/// The references in resource `data`, layers still named by PSD id (`AiPicture::Layer`). `None`
/// for data that is not one, or is cut short.
pub fn decode(data: &[u8]) -> Option<Vec<AiReference>> {
    let mut r = Reader { data, at: 0 };
    if r.take(4)? != SIGNATURE || r.u16()? != VERSION {
        return None;
    }
    let count = usize::from(r.u16()?);
    if count > MAX_COUNT {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.text()?;
        let picture = match r.u8()? {
            0 => AiPicture::Layer(u64::from(r.u32()?)),
            1 => AiPicture::Document,
            2 => {
                let label = r.text()?;
                AiPicture::Image { png: Arc::new(r.bytes()?.to_vec()), label }
            }
            _ => return None,
        };
        out.push(AiReference { name, picture });
    }
    Some(out)
}

/// Gives an imported document the references its resource holds, once the layers' PSD ids are
/// known, and takes the raw resource out of the ones kept for export (export writes it afresh
/// from the field). A reference to a layer that is not there is dropped; a resource that can't
/// be read stays as it came.
pub(crate) fn import(doc: &mut Document) {
    let Some(at) = doc.metadata.psd_resources.iter().position(|(id, _, data)| is_ours(*id, data)) else { return };
    let Some(found) = doc.metadata.psd_resources.get(at).and_then(|(_, _, data)| decode(data)) else { return };
    let by_psd_id: HashMap<u64, u64> = doc.walk().iter().filter_map(|(_, _, l)| l.psd_id.map(|p| (u64::from(p), l.id.0))).collect();
    doc.ai_references = found
        .into_iter()
        .filter_map(|r| match r.picture {
            AiPicture::Layer(psd) => by_psd_id.get(&psd).map(|id| AiReference { picture: AiPicture::Layer(*id), ..r }),
            _ => Some(r),
        })
        .collect();
    doc.metadata.psd_resources.remove(at);
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::{ColorMode, Layer, PixelFormat, SampleType, Size};

    fn doc() -> Document {
        let mut d = Document::new("D", Size::new(4, 4), ColorMode::Rgb, SampleType::U8);
        d.layers.push(Layer::raster("Luna", PixelFormat::RGBA8));
        let luna = d.layers[0].id.0;
        d.ai_references = vec![
            AiReference { name: "Luna".into(), picture: AiPicture::Layer(luna) },
            AiReference { name: String::new(), picture: AiPicture::Document },
            AiReference {
                name: "Max \u{1F986}".into(),
                picture: AiPicture::Image { png: Arc::new(b"\x89PNG fake".to_vec()), label: "Layer: Max (Other)".into() },
            },
            AiReference { name: "gone".into(), picture: AiPicture::Layer(999_999) },
        ];
        d
    }

    #[test]
    fn references_round_trip_with_layers_by_psd_id() {
        let mut d = doc();
        let luna = d.layers[0].id;
        let data = encode(&d, &HashMap::from([(luna, 7)])).unwrap();
        assert!(is_ours(AI_REFERENCES, &data) && !is_ours(4001, &data) && !is_ours(AI_REFERENCES, b"8BIMxxxx"));
        // The layer that is not in the file is not written.
        let back = decode(&data).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0], AiReference { name: "Luna".into(), picture: AiPicture::Layer(7) });
        assert_eq!(back[1..], d.ai_references[1..3]);
        // Imported: the PSD id becomes the layer's id again, and the raw resource goes.
        d.layers[0].psd_id = Some(7);
        let expected = d.ai_references[..3].to_vec();
        d.ai_references.clear();
        d.metadata.psd_resources.push((4001, String::new(), Arc::new(b"plug-in".to_vec())));
        d.metadata.psd_resources.push((AI_REFERENCES, NAME.into(), Arc::new(data)));
        import(&mut d);
        assert_eq!(d.ai_references, expected);
        assert_eq!(d.metadata.psd_resources.len(), 1, "the plug-in's resource stays");
        // None to write: no resource.
        assert_eq!(encode(&Document::new("E", Size::new(1, 1), ColorMode::Rgb, SampleType::U8), &HashMap::new()), None);
    }

    #[test]
    fn hostile_data_is_no_references_and_never_a_panic() {
        let good = encode(&doc(), &HashMap::new()).unwrap();
        for cut in 0..good.len() {
            assert!(decode(good.get(..cut).unwrap()).is_none_or(|r| r.len() <= 3), "{cut}");
        }
        let mut huge = b"pcAR\0\x01\xff\xff".to_vec();
        huge.extend([0u8; 16]);
        assert_eq!(decode(&huge), None);
        let mut lying = b"pcAR\0\x01\0\x01".to_vec();
        lying.extend(u32::MAX.to_be_bytes());
        assert_eq!(decode(&lying), None);
        assert_eq!(decode(b"pcAR\0\x02\0\0"), None, "another version");
        assert_eq!(decode(b"pcAR\0\x01\0\x01\0\0\0\0\x09"), None, "an unknown kind");
        // A resource that can't be read stays as it came, and the document has no references.
        let mut d = doc();
        d.ai_references.clear();
        d.metadata.psd_resources.push((AI_REFERENCES, NAME.into(), Arc::new(b"pcAR\0\x01\0\x05".to_vec())));
        import(&mut d);
        assert!(d.ai_references.is_empty() && d.metadata.psd_resources.len() == 1);
    }
}
