//! Style presets: a look added to a prompt (`ai.generate`'s `style`, the AI Gen panel's Style
//! picker). Each preset is a sentence about the medium and its marks that goes after the prompt,
//! so the prompt stays about the subject and the preset about how it is rendered. They work with
//! any model, local or cloud: the sentence is all the model is given.
//!
//! The presets are grouped as the picker's tabs list them ([`StyleGroup`]); their thumbnails
//! (one subject throughout, so the treatments compare) are the shell's.

/// The picker's tabs: what kind of look a preset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleGroup {
    Photography,
    Illustration,
    TraditionalArt,
    Craft,
    Graphic,
}

impl StyleGroup {
    pub const ALL: [StyleGroup; 5] = [StyleGroup::Photography, StyleGroup::Illustration, StyleGroup::TraditionalArt, StyleGroup::Craft, StyleGroup::Graphic];

    /// The tab's title.
    pub fn label(self) -> &'static str {
        match self {
            StyleGroup::Photography => "Photography",
            StyleGroup::Illustration => "Illustration",
            StyleGroup::TraditionalArt => "Traditional art",
            StyleGroup::Craft => "3D & craft",
            StyleGroup::Graphic => "Graphic & experimental",
        }
    }

    /// As `ai.styles` names it.
    pub fn key(self) -> &'static str {
        match self {
            StyleGroup::Photography => "photography",
            StyleGroup::Illustration => "illustration",
            StyleGroup::TraditionalArt => "traditionalArt",
            StyleGroup::Craft => "craft",
            StyleGroup::Graphic => "graphic",
        }
    }
}

/// One preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// As `ai.generate`'s `style` names it, and its thumbnail's file.
    pub id: &'static str,
    pub name: &'static str,
    pub group: StyleGroup,
    /// What it looks like, in a few words (the picker's larger preview).
    pub description: &'static str,
    /// What the model is told after the prompt.
    pub prompt: &'static str,
}

const fn style(id: &'static str, name: &'static str, group: StyleGroup, description: &'static str, prompt: &'static str) -> Style {
    Style { id, name, group, description, prompt }
}

use StyleGroup::{Craft, Graphic, Illustration, Photography, TraditionalArt};

/// Every preset, in the picker's order.
pub const STYLES: [Style; 24] = [
    style(
        "natural-photo",
        "Natural Photo",
        Photography,
        "Believable daylight, lifelike skin and materials",
        "Style: a natural, true-to-life photograph in soft daylight, with lifelike skin and material textures, accurate colours and a relaxed, candid composition.",
    ),
    style(
        "cinematic",
        "Cinematic",
        Photography,
        "Film-still framing, directional light and colour grading",
        "Style: a cinematic film still, widescreen framing, dramatic directional lighting, atmospheric depth with haze and shallow depth of field, and a deliberate teal-and-amber colour grade.",
    ),
    style(
        "studio-photo",
        "Studio Photo",
        Photography,
        "Seamless backdrop, controlled light, a polished finish",
        "Style: a professional studio photograph against a seamless backdrop, controlled softbox lighting with crisp highlights, and a polished commercial finish.",
    ),
    style(
        "vintage-film",
        "Vintage Film",
        Photography,
        "Visible grain, faded colours and analogue flaws",
        "Style: a vintage analogue film photograph with visible grain, gently faded warm colours, soft contrast, slight light leaks and the imperfections of an old print.",
    ),
    style(
        "black-and-white",
        "Black & White",
        Photography,
        "Monochrome photography with strong tonal separation",
        "Style: a black and white photograph, monochrome only, with rich blacks, bright highlights, strong tonal separation and fine silver-gelatin texture.",
    ),
    style(
        "flat-vector",
        "Flat Vector",
        Illustration,
        "Crisp geometric shapes, solid colours, little shading",
        "Style: a flat vector illustration built from crisp geometric shapes and solid fills, a limited harmonious palette, clean edges and minimal shading, with no texture or gradients.",
    ),
    style(
        "storybook",
        "Storybook",
        Illustration,
        "Soft painted texture, rounded forms, warm storytelling",
        "Style: a children's storybook illustration with soft painted texture, rounded friendly forms, warm gentle colours and an expressive, storytelling mood.",
    ),
    style(
        "anime",
        "Anime",
        Illustration,
        "Clean linework, cel shading, anime proportions",
        "Style: an anime illustration with clean confident linework, flat cel shading with crisp shadow shapes, vivid colours and classic anime proportions and eyes.",
    ),
    style(
        "comic-book",
        "Comic Book",
        Illustration,
        "Bold ink outlines, strong shadows, halftone dots",
        "Style: a comic book illustration with bold black ink outlines, heavy spot blacks, strong shadows, halftone dot shading and punchy printed colours.",
    ),
    style(
        "digital-painting",
        "Digital Painting",
        Illustration,
        "Detailed painterly rendering with convincing volume",
        "Style: a detailed digital painting with visible painterly brushwork, rich lighting, convincing volume and form, and a concept-art finish.",
    ),
    style(
        "watercolour",
        "Watercolour",
        TraditionalArt,
        "Translucent paint washes on textured paper",
        "Style: a watercolour painting with translucent layered washes, soft pigment blooms and runs, loose edges and generous areas of visible textured white paper.",
    ),
    style(
        "oil-painting",
        "Oil Painting",
        TraditionalArt,
        "Rich opaque paint, textured brushstrokes",
        "Style: an oil painting on canvas with rich opaque paint, thick textured impasto brushstrokes, layered colour and the canvas weave showing through.",
    ),
    style(
        "pencil-sketch",
        "Pencil Sketch",
        TraditionalArt,
        "Graphite shading, hatching and construction lines",
        "Style: a graphite pencil sketch on paper, monochrome grey, with hatching and soft shading, loose construction lines and unfinished edges.",
    ),
    style(
        "line-art",
        "Line Art",
        TraditionalArt,
        "Clean contours on white, little or no shading",
        "Style: clean black line art on a plain white background, confident even contours, no fill and little or no shading.",
    ),
    style(
        "printmaking",
        "Printmaking",
        TraditionalArt,
        "Bold carved marks, few colours, uneven ink",
        "Style: a linocut or woodcut print with bold carved marks, two or three ink colours on off-white paper, and slightly uneven, imperfect ink coverage.",
    ),
    style(
        "3d-cartoon",
        "3D Cartoon",
        Craft,
        "Rounded 3D characters, smooth materials",
        "Style: a 3D animated cartoon render with rounded, appealing characters, exaggerated expressive proportions, smooth materials and soft global illumination.",
    ),
    style(
        "realistic-cgi",
        "Realistic CGI",
        Craft,
        "Precise surfaces, reflections and physical lighting",
        "Style: a high-end 3D CGI render, clearly computer-generated, with flawless smooth digital surfaces, physically based materials, precise reflections and ray-traced global illumination, like a modern game cinematic.",
    ),
    style(
        "clay-model",
        "Clay Model",
        Craft,
        "Matte clay with handmade marks",
        "Style: a handmade clay model in matte plasticine, with fingerprints, tool marks and small irregularities, photographed like a stop-motion set.",
    ),
    style(
        "paper-craft",
        "Paper Craft",
        Craft,
        "Layered cut paper, folds and small shadows",
        "Style: a layered cut-paper craft scene with folded edges, coloured card stock, visible paper fibres and small cast shadows between the layers.",
    ),
    style(
        "textile-craft",
        "Textile Craft",
        Craft,
        "Felt, wool and stitched fabric",
        "Style: a handmade textile craft piece in which everything, figure and setting, is made of needle-felted wool, knitted yarn and stitched fabric, with visible fibres, seams and embroidery stitches.",
    ),
    style(
        "pixel-art",
        "Pixel Art",
        Graphic,
        "Visible pixels, a few colours, stepped edges",
        "Style: retro pixel art with clearly visible square pixels, a restricted palette of a few colours, deliberate stepped edges and no anti-aliasing.",
    ),
    style(
        "retro-poster",
        "Retro Poster",
        Graphic,
        "Simple silhouettes, few inks, vintage print texture",
        "Style: a retro screen-printed poster with simplified bold silhouettes, a limited palette of flat ink colours, slight misregistration and worn vintage paper texture.",
    ),
    style(
        "collage",
        "Collage",
        Graphic,
        "Cut photographs and torn paper, visibly layered",
        "Style: a mixed-media paper collage assembled from cut-out magazine photographs at different scales, torn paper edges, strips of tape and overlapping layers glued on a flat backing.",
    ),
    style(
        "abstract",
        "Abstract",
        Graphic,
        "Nonrepresentational shapes, expressive colour",
        "Style: an abstract composition of bold geometric and organic shapes, flat planes of expressive colour and rich texture, the subject reduced to suggested forms.",
    ),
];

/// The preset `id` names, if any.
pub fn find(id: &str) -> Option<&'static Style> {
    let id = id.trim();
    STYLES.iter().find(|s| s.id.eq_ignore_ascii_case(id))
}

/// `prompt` with `style`'s sentence after it, in a paragraph of its own.
pub fn styled(prompt: &str, style: Option<&Style>) -> String {
    match style {
        Some(style) if prompt.trim().is_empty() => style.prompt.to_string(),
        Some(style) => format!("{}\n\n{}", prompt.trim_end(), style.prompt),
        None => prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_found_case_blind() {
        for (i, s) in STYLES.iter().enumerate() {
            assert!(STYLES.iter().skip(i + 1).all(|o| o.id != s.id), "{} twice", s.id);
            assert_eq!(find(&s.id.to_uppercase()).map(|f| f.id), Some(s.id));
            assert!(s.prompt.starts_with("Style: "));
        }
        assert!(find("").is_none());
        assert!(find("nope").is_none());
    }

    #[test]
    fn every_group_has_presets() {
        for g in StyleGroup::ALL {
            assert!(STYLES.iter().filter(|s| s.group == g).count() >= 4, "{g:?}");
        }
    }

    #[test]
    fn a_style_follows_the_prompt() {
        let w = find("watercolour");
        assert_eq!(styled("a fox", None), "a fox");
        assert!(styled("a fox  ", w).starts_with("a fox\n\nStyle: a watercolour"));
        assert!(styled("", w).starts_with("Style: "));
    }
}
