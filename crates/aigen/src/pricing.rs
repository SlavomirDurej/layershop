//! What an OpenAI image costs: an estimate before generating, and the billed amount after.
//!
//! OpenAI bills GPT Image models per token. The answer to a request reports the tokens it used,
//! so the cost *after* a generation is exact ([`billed`]). *Before* it, only the figures OpenAI
//! publishes are known ([`estimate`]): token counts by quality for the three sizes of the GPT
//! Image 1 family, and per-image prices at those sizes for GPT Image 2. OpenAI publishes no
//! formula for other sizes and no figures for GPT Image 2.5 (same token price as GPT Image 2,
//! token counts from an interactive calculator only), so those estimates are marked approximate.
//!
//! Rates and figures are from OpenAI's image generation guide and model pages as of 2026-10.
//! They change; the billed amount, computed from the same rates, is only as current as this table.

use serde_json::Value;

use crate::cloud::openai_size;
use crate::model::Model;

/// What Meta bills for one image from Muse Image: a flat price, whatever its size (Meta Model
/// API pricing, 2026-10).
pub const MUSE_IMAGE_USD: f64 = 0.01;
/// What one Black Forest Labs credit costs: the answer to a request says how many it used.
pub const BFL_CREDIT_USD: f64 = 0.01;
/// xAI reports what a request cost in ticks: ten thousand million to the dollar.
pub const XAI_TICKS_PER_USD: f64 = 1e10;

/// A reported amount as dollars, when it is one: finite, not negative, and not absurd.
pub fn reported(amount: f64, unit_usd: f64) -> Option<f64> {
    let usd = amount * unit_usd;
    (usd.is_finite() && (0.0..=10_000.0).contains(&usd)).then_some(usd)
}

/// US dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub text_input: f64,
    pub image_input: f64,
    pub image_output: f64,
}

/// The token rates of an OpenAI image model, by model id. `None` for a model without published
/// token rates here (DALL·E, or one newer than this table).
pub fn rates(api_model: &str) -> Option<Rates> {
    let id = api_model.trim();
    Some(if id.starts_with("gpt-image-2") {
        Rates { text_input: 5.0, image_input: 8.0, image_output: 30.0 }
    } else if id.starts_with("gpt-image-1.5") {
        Rates { text_input: 5.0, image_input: 8.0, image_output: 32.0 }
    } else if id.starts_with("gpt-image-1-mini") {
        Rates { text_input: 2.0, image_input: 2.5, image_output: 8.0 }
    } else if id.starts_with("gpt-image-1") {
        Rates { text_input: 5.0, image_input: 10.0, image_output: 40.0 }
    } else {
        return None;
    })
}

/// What one image is expected to cost, in US dollars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub low: f64,
    pub high: f64,
    /// Derived from figures for other sizes or another model, not a published price for this one.
    pub approximate: bool,
}

/// The sizes OpenAI publishes figures for, in table column order.
const SIZES: [(u32, u32); 3] = [(1024, 1024), (1024, 1536), (1536, 1024)];
/// Output tokens of the GPT Image 1 family: low, medium, high by [`SIZES`].
const TOKENS_V1: [[f64; 3]; 3] = [[272.0, 408.0, 400.0], [1056.0, 1584.0, 1568.0], [4160.0, 6240.0, 6208.0]];
/// Per-image output prices of GPT Image 2: low, medium, high by [`SIZES`].
const PRICE_V2: [[f64; 3]; 3] = [[0.006, 0.005, 0.005], [0.053, 0.041, 0.041], [0.211, 0.165, 0.165]];

/// The table rows a quality setting can mean: one row, or all of them for `auto` (the model
/// picks). `None` for the levels without published figures (`xhigh`, `max`).
fn quality_rows(quality: &str) -> Option<std::ops::RangeInclusive<usize>> {
    match quality.trim() {
        "low" => Some(0..=0),
        "medium" => Some(1..=1),
        "high" => Some(2..=2),
        "" | "auto" => Some(0..=2),
        _ => None,
    }
}

/// The expected cost of one image from `model` for a `width` × `height` selection, or `None`
/// when nothing is published to base it on. Covers the output image only: the prompt's text
/// tokens add a fraction of a cent.
pub fn estimate(model: &Model, width: u32, height: u32) -> Option<Estimate> {
    let id = model.api_model.trim();
    let rates = rates(id)?;
    let rows = quality_rows(&model.quality)?;
    let (w, h) = openai_size(model, width, height);
    let column = SIZES.iter().position(|s| *s == (w, h));
    let mut range: Option<(f64, f64)> = None;
    let mut add = |v: f64| range = Some(range.map_or((v, v), |(lo, hi): (f64, f64)| (lo.min(v), hi.max(v))));
    let approximate = if id.starts_with("gpt-image-1") {
        // Always one of the three sizes: tokens × rate is the published price.
        let column = column?;
        for row in rows {
            add(TOKENS_V1.get(row)?.get(column)? * rates.image_output / 1e6);
        }
        false
    } else {
        // GPT Image 2 and 2.5. At a published size, that price; otherwise the spread of the
        // published ones, grown with the pixel count beyond the largest of them.
        let pixels = f64::from(w) * f64::from(h);
        let scale = (pixels / (1536.0 * 1024.0)).max(1.0);
        for row in rows {
            let prices = PRICE_V2.get(row)?;
            match column {
                Some(c) => add(*prices.get(c)?),
                None => prices.iter().for_each(|p| add(p * scale)),
            }
        }
        column.is_none() || !id.starts_with("gpt-image-2-") && id != "gpt-image-2"
    };
    let (low, high) = range?;
    Some(Estimate { low, high, approximate })
}

fn tokens(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0).map_or(0.0, |n| n.min(1e12))
}

/// The cost of a finished request, in US dollars, from the `usage` object of OpenAI's answer.
/// `None` without usage, or for a model without rates here.
pub fn billed(api_model: &str, usage: &Value) -> Option<f64> {
    let rates = rates(api_model)?;
    let usage = usage.as_object()?;
    let output = tokens(usage.get("output_tokens"));
    let details = usage.get("input_tokens_details");
    let image_in = tokens(details.and_then(|d| d.get("image_tokens")));
    // Without the breakdown, input is the prompt: text.
    let text_in = match details.and_then(|d| d.get("text_tokens")) {
        Some(t) => tokens(Some(t)),
        None => (tokens(usage.get("input_tokens")) - image_in).max(0.0),
    };
    if output + image_in + text_in <= 0.0 {
        return None;
    }
    Some((text_in * rates.text_input + image_in * rates.image_input + output * rates.image_output) / 1e6)
}

/// A dollar amount for display: three decimals under a dollar (a low-quality image costs half a
/// cent), two above.
pub fn format_usd(amount: f64) -> String {
    if !amount.is_finite() || amount < 0.0 {
        return "$?".into();
    }
    if amount < 1.0 { format!("${amount:.3}") } else { format!("${amount:.2}") }
}

impl Estimate {
    /// `$0.041`, `$0.005–$0.165`, with `≈` in front when approximate.
    pub fn label(&self) -> String {
        let (lo, hi) = (format_usd(self.low), format_usd(self.high));
        let text = if lo == hi { lo } else { format!("{lo}–{hi}") };
        if self.approximate { format!("≈ {text}") } else { text }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;
    use serde_json::json;

    fn model(id: &str, quality: &str) -> Model {
        Model { quality: quality.into(), ..Model::cloud(Provider::Openai, id, id, false) }
    }

    #[test]
    fn gpt_image_1_family_prices_match_the_published_table() {
        // GPT Image 1.5, 1024×1024: $0.009 / $0.034 / $0.133; portrait high: $0.20.
        for (quality, want) in [("low", 0.009), ("medium", 0.034), ("high", 0.133)] {
            let e = estimate(&model("gpt-image-1.5", quality), 800, 800).unwrap();
            assert!((e.low - want).abs() < 0.001 && e.low == e.high && !e.approximate, "{quality}: {e:?}");
        }
        let e = estimate(&model("gpt-image-1.5", "high"), 600, 900).unwrap();
        assert!((e.low - 0.2).abs() < 0.001, "{e:?}");
        // GPT Image 1, landscape medium: $0.063.
        let e = estimate(&model("gpt-image-1", "medium"), 900, 600).unwrap();
        assert!((e.low - 0.063).abs() < 0.001, "{e:?}");
        // Auto: the model picks, so the whole spread.
        let e = estimate(&model("gpt-image-1.5", "auto"), 800, 800).unwrap();
        assert!(e.low < 0.01 && e.high > 0.13, "{e:?}");
        assert_eq!(e.label(), "$0.009–$0.133");
    }

    #[test]
    fn gpt_image_2_uses_its_published_prices_and_marks_the_rest_approximate() {
        // A square selection at 1 MP renders at 1008×1008: not a published size.
        let e = estimate(&model("gpt-image-2", "medium"), 500, 500).unwrap();
        assert!(e.approximate && (e.low - 0.041).abs() < 1e-9 && (e.high - 0.053).abs() < 1e-9, "{e:?}");
        assert_eq!(e.label(), "≈ $0.041–$0.053");
        // Larger than the published sizes: grows with the pixels.
        let mut big = model("gpt-image-2", "high");
        big.megapixels = 4.0;
        let e = estimate(&big, 2000, 2000).unwrap();
        assert!(e.high > 0.5 && e.approximate, "{e:?}");
        // GPT Image 2.5 has no published figures: GPT Image 2's, always approximate.
        for id in ["gpt-image-2.5-flare", "gpt-image-2.5-sunburst"] {
            let e = estimate(&model(id, "low"), 500, 500).unwrap();
            assert!(e.approximate && e.low >= 0.005 && e.high <= 0.006, "{id}: {e:?}");
        }
    }

    #[test]
    fn nothing_is_invented_where_nothing_is_published() {
        assert_eq!(estimate(&model("gpt-image-2.5-flare", "xhigh"), 500, 500), None);
        assert_eq!(estimate(&model("gpt-image-2.5-flare", "max"), 500, 500), None);
        assert_eq!(estimate(&model("dall-e-3", "high"), 500, 500), None);
        assert_eq!(estimate(&model("some-future-model", "low"), 500, 500), None);
        assert_eq!(rates("gemini-3-pro-image-preview"), None);
        for (w, h) in [(0, 0), (u32::MAX, 1), (1, u32::MAX)] {
            let e = estimate(&model("gpt-image-2", "auto"), w, h).unwrap();
            assert!(e.low.is_finite() && e.high.is_finite() && e.low > 0.0 && e.low <= e.high, "{w}x{h}: {e:?}");
        }
    }

    #[test]
    fn billed_cost_comes_from_the_reported_tokens() {
        let usage = json!({"input_tokens": 50, "output_tokens": 1767, "total_tokens": 1817, "input_tokens_details": {"text_tokens": 50, "image_tokens": 0}});
        let cost = billed("gpt-image-2.5-flare", &usage).unwrap();
        assert!((cost - (50.0 * 5.0 + 1767.0 * 30.0) / 1e6).abs() < 1e-12, "{cost}");
        assert_eq!(format_usd(cost), "$0.053");
        // No breakdown: the input counts as text.
        let cost = billed("gpt-image-1", &json!({"input_tokens": 100, "output_tokens": 272})).unwrap();
        assert!((cost - (100.0 * 5.0 + 272.0 * 40.0) / 1e6).abs() < 1e-12);
        // Image input (edits) is billed at its own rate.
        let cost = billed("gpt-image-2", &json!({"input_tokens": 300, "output_tokens": 0, "input_tokens_details": {"text_tokens": 100, "image_tokens": 200}}))
            .unwrap();
        assert!((cost - (100.0 * 5.0 + 200.0 * 8.0) / 1e6).abs() < 1e-12);
        for junk in [json!(null), json!([]), json!({}), json!({"output_tokens": "many"}), json!({"output_tokens": -5}), json!({"output_tokens": 0})] {
            assert_eq!(billed("gpt-image-2", &junk), None, "{junk}");
        }
        assert!(billed("gpt-image-2", &json!({"output_tokens": 1e300})).unwrap().is_finite());
        assert_eq!(billed("dall-e-3", &usage), None);
    }

    #[test]
    fn dollars_are_formatted_for_small_amounts() {
        assert_eq!(format_usd(0.0054), "$0.005");
        assert_eq!(format_usd(0.211), "$0.211");
        assert_eq!(format_usd(1.5), "$1.50");
        assert_eq!(format_usd(f64::NAN), "$?");
        assert_eq!(format_usd(-1.0), "$?");
    }
}
