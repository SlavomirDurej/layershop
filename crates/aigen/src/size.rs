//! From a selection's size to the size a model is asked to render.

/// Smallest and largest edge a local model is asked for.
const MIN_EDGE: f64 = 256.0;
const MAX_EDGE: f64 = 4096.0;

fn snap(v: f64, multiple: u32) -> u32 {
    let m = f64::from(multiple.max(1));
    let v = if v.is_finite() { v } else { MIN_EDGE };
    ((v.clamp(MIN_EDGE, MAX_EDGE) / m).round() * m) as u32
}

/// The render size for a `width` × `height` selection: the same aspect ratio at about
/// `megapixels` (0 or less: the selection's own size), each edge a multiple of `multiple` and
/// within 256..=4096. Diffusion models work in latent blocks, so they need the multiple, and
/// they are trained near one megapixel, so a small or huge selection renders badly at its own size.
pub fn render_size(width: u32, height: u32, megapixels: f64, multiple: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    // Keep extreme strips renderable: beyond 4:1 the short edge would fall under the minimum.
    let aspect = (w / h).clamp(0.25, 4.0);
    if megapixels.is_finite() && megapixels > 0.0 {
        let area = megapixels.min(16.0) * 1_000_000.0;
        let rh = (area / aspect).sqrt();
        (snap(rh * aspect, multiple), snap(rh, multiple))
    } else {
        (snap(w, multiple), snap(h, multiple))
    }
}

/// The sizes HiDream O1 was trained at, about four megapixels each (ComfyUI's Empty HiDream-O1
/// Latent Image node lists them). Away from them its quality drops: at one and two megapixels
/// a photographic scene came out smeared and waxy (2026-10-12).
pub const HIDREAM_O1_SIZES: [(u32, u32); 11] =
    [(2048, 2048), (2304, 1728), (1728, 2304), (2560, 1440), (1440, 2560), (2496, 1664), (1664, 2496), (3104, 1312), (1312, 3104), (2304, 1792), (1792, 2304)];

/// The entry of `ratios` (width, height) closest to `width`:`height`, compared on a log scale so
/// 2:1 and 1:2 are equally far from square.
pub fn nearest_ratio(width: u32, height: u32, ratios: &[(u32, u32)]) -> Option<(u32, u32)> {
    let want = (f64::from(width.max(1)) / f64::from(height.max(1))).ln();
    ratios.iter().copied().filter(|(_, h)| *h > 0).min_by(|a, b| {
        let d = |r: &(u32, u32)| ((f64::from(r.0) / f64::from(r.1)).ln() - want).abs();
        d(a).total_cmp(&d(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_aspect_ratio_near_the_requested_megapixels() {
        assert_eq!(render_size(500, 500, 1.0, 16), (1008, 1008));
        let (w, h) = render_size(1920, 1080, 1.0, 16);
        assert_eq!((w % 16, h % 16), (0, 0));
        assert!((f64::from(w) / f64::from(h) - 16.0 / 9.0).abs() < 0.03, "{w}x{h}");
        assert!((f64::from(w * h) / 1e6 - 1.0).abs() < 0.05, "{w}x{h}");
    }

    #[test]
    fn zero_megapixels_uses_the_selection_size() {
        assert_eq!(render_size(1030, 770, 0.0, 16), (1024, 768));
        assert_eq!(render_size(10, 10, 0.0, 16), (256, 256));
        assert_eq!(render_size(100_000, 90_000, 0.0, 16), (4096, 4096));
    }

    #[test]
    fn hostile_sizes_stay_in_range() {
        for (w, h, mp) in [(0, 0, 1.0), (1, u32::MAX, 1.0), (u32::MAX, 1, 4.0), (7, 9, f64::NAN), (7, 9, f64::INFINITY), (7, 9, -3.0), (7, 9, 1e30)] {
            let (rw, rh) = render_size(w, h, mp, 16);
            assert!((256..=4096).contains(&rw) && (256..=4096).contains(&rh), "{w}x{h}@{mp} -> {rw}x{rh}");
        }
        // A zero multiple means "any size", not a division by zero.
        assert_eq!(render_size(640, 640, 0.0, 0), (640, 640));
    }

    #[test]
    fn picks_the_closest_supported_ratio() {
        let r = [(1, 1), (16, 9), (9, 16), (3, 2)];
        assert_eq!(nearest_ratio(1920, 1080, &r), Some((16, 9)));
        assert_eq!(nearest_ratio(500, 520, &r), Some((1, 1)));
        assert_eq!(nearest_ratio(100, 400, &r), Some((9, 16)));
        assert_eq!(nearest_ratio(0, 0, &r), Some((1, 1)));
        assert_eq!(nearest_ratio(4, 3, &[]), None);
    }
}
