//! Document-side placement of a completed upscale, independent of the image provider.

use std::sync::Arc;

use photocraft_algo::transform::{Interp, warp_surface_with};
use photocraft_color::BlendMode;
use photocraft_doc::{Layer, LayerContent, LayerId, SmartObject, SmartSource};
use photocraft_geom::{Affine, Rect, Size};
use photocraft_raster::Surface;

use crate::{EngineError, Result, Session, jobs::JobCtx};

const EMBEDDED_NAME: &str = "Upscaled.png";

impl Session {
    /// Places a finished PNG above one raster layer in the active document, in one history step.
    /// The caller of an asynchronous provider must apply this to its original document snapshot.
    /// The embedded PNG is retained verbatim while the display cache uses the original footprint.
    pub fn place_upscaled_raster(&mut self, source: LayerId, png: Vec<u8>, expected: Size, factor: u32, ctx: &JobCtx) -> Result<LayerId> {
        ctx.check()?;
        let (layer, _, bounds) = self.upscale_source(source)?;
        let doc = &self.active().ok_or(EngineError::NoDocument)?.doc;
        check_dimensions(bounds, expected, factor)?;

        let (width, height) = crate::memory_gate::png_dimensions(&png).ok_or_else(|| invalid("The upscaled image must be a valid PNG"))?;
        if Size::new(width, height) != expected {
            return Err(invalid(format!("Upscaled PNG is {width} × {height}; expected {} × {}", expected.width, expected.height)));
        }

        let format = doc.pixel_format();
        crate::memory_gate::ensure("decode and place the upscaled image", crate::memory_gate::place_need(&png, format.bytes_per_pixel()))?;
        let image = crate::smart_cmds::source_image(EMBEDDED_NAME, &png, format)?;
        if image.bounds.size() != expected {
            return Err(invalid("Decoded upscale dimensions differ from the expected size"));
        }
        ctx.check()?;
        let transform = Affine {
            m: [
                f64::from(bounds.width()) / f64::from(image.bounds.width()),
                0.0,
                0.0,
                f64::from(bounds.height()) / f64::from(image.bounds.height()),
                f64::from(bounds.x0),
                f64::from(bounds.y0),
            ],
        };
        let placement = crate::smart_cmds::affine_homography(&transform);
        crate::transform_cmds::check_budget(image.bounds, &placement, "upscaled layer", format)?;
        let cache = ctx
            .stage(0.8, 0.98, "Placing the upscaled image", |interrupt| {
                warp_surface_with(&image.surface, image.bounds, &placement, Interp::Bicubic, interrupt)
            })?
            .ok_or(EngineError::Cancelled)?;
        let smart = SmartObject::new(SmartSource::Embedded { file_name: EMBEDDED_NAME.into(), bytes: Arc::new(png) }, transform, Some(cache));
        let mut result = Layer::new(format!("Upscaled - {} ({factor}x)", layer.name), LayerContent::Smart(smart));
        result.mask = layer.mask.clone();
        result.visible = layer.visible;
        result.opacity = layer.opacity;
        result.fill_opacity = layer.fill_opacity;

        ctx.check()?;
        self.edit("Place Upscaled Layer", |doc, active| {
            ctx.check()?;
            let path = doc.path_of(source).ok_or_else(|| invalid("The upscale source layer no longer exists"))?;
            if doc.insertion_path(source).as_ref() != Some(&path) {
                return Err(invalid("The upscale source no longer has a valid insertion position"));
            }
            let id = doc.insert_above(Some(source), result);
            let sibling_path = doc.path_of(id);
            let adjacent = path.split_last().zip(sibling_path.as_deref().and_then(|p| p.split_last())).is_some_and(
                |((&source_index, parent), (&result_index, result_parent))| parent == result_parent && source_index.checked_add(1) == Some(result_index),
            );
            if !adjacent {
                return Err(invalid("The upscaled layer could not be placed immediately above its source"));
            }
            *active = Some(id);
            Ok(id)
        })
    }

    pub(crate) fn upscale_source(&self, source: LayerId) -> Result<(&Layer, &Surface, Rect)> {
        let state = self.active().ok_or(EngineError::NoDocument)?;
        if self.color_restrict.is_some() || self.filter_mask_layer.is_some() {
            return Err(invalid("Finish editing the channel or smart filter mask before placing an upscale"));
        }
        if state.selected_layers().len() > 1 {
            return Err(invalid("Select a single raster layer to upscale"));
        }
        let doc = &state.doc;
        let layer = doc.layer(source).ok_or_else(|| invalid("The upscale source layer no longer exists"))?;
        let LayerContent::Raster(pixels) = &layer.content else {
            return Err(invalid("The upscale source must be a raster layer"));
        };
        let path = doc.path_of(source).ok_or_else(|| invalid("The upscale source layer no longer exists"))?;
        if layer.video.is_some() || layer.frame.is_some() || doc.insertion_path(source).as_ref() != Some(&path) {
            return Err(invalid("Video layers and frame contents cannot be upscaled"));
        }
        let in_clipping_group = (1..=path.len())
            .filter_map(|depth| doc.layer_at(&path[..depth]))
            .any(|item| item.clipped || !crate::clip_group::clipped_above(doc, item.id).is_empty());
        if in_clipping_group {
            return Err(invalid("Layers in clipping groups cannot be upscaled"));
        }
        if layer.blend != BlendMode::Normal
            || !layer.effects.items.is_empty()
            || layer.effects.psd_raw.is_some()
            || layer.blend_if != Default::default()
            || layer.advanced != Default::default()
            || layer.excluded_channels != 0
            || layer.vector_mask.is_some()
        {
            return Err(invalid("The source layer has unsupported blending, effects or a vector mask"));
        }
        let bounds = pixels.content_bounds();
        if bounds.is_empty() {
            return Err(invalid("The upscale source layer has no pixels"));
        }
        Ok((layer, pixels, bounds))
    }
}

fn invalid(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}

fn check_dimensions(bounds: Rect, expected: Size, factor: u32) -> Result<()> {
    if !matches!(factor, 2 | 4) {
        return Err(invalid("Upscale factor must be 2 or 4"));
    }
    if bounds.is_empty() {
        return Err(invalid("The upscale source layer has no pixels"));
    }
    let width = bounds.width().checked_mul(factor).ok_or_else(|| invalid("Upscaled width overflows"))?;
    let height = bounds.height().checked_mul(factor).ok_or_else(|| invalid("Upscaled height overflows"))?;
    if Size::new(width, height) != expected {
        return Err(invalid(format!("Expected {width} × {height} for the source layer at {factor}x")));
    }
    let limits = photocraft_codecs::Limits::default();
    if width > limits.max_width || height > limits.max_height || u64::from(width) * u64::from(height) > photocraft_compose::MAX_RENDER_PIXELS {
        return Err(invalid("Upscaled image dimensions exceed the render limit"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
