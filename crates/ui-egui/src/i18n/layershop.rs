//! LayerShop's own interface strings: the few that the craft-i18n catalogs don't have (its
//! Generative AI preferences, the AI Gen panel's name, the update card's Download). They live in
//! `layershop.tsv` beside this file (locale, English, translation per line; `#` starts a comment)
//! and [`super::tr`] looks here before craft-i18n.

use std::collections::HashMap;
use std::sync::OnceLock;

const SOURCE: &str = include_str!("layershop.tsv");

/// The table, read once: (locale, English) to its translation.
fn rows() -> &'static HashMap<(&'static str, &'static str), &'static str> {
    static ROWS: OnceLock<HashMap<(&'static str, &'static str), &'static str>> = OnceLock::new();
    ROWS.get_or_init(|| parse(SOURCE))
}

/// Lines of three tab-separated fields, none empty; anything else (comments, blank or short
/// lines) is skipped rather than refused.
fn parse(source: &'static str) -> HashMap<(&'static str, &'static str), &'static str> {
    source
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (locale, english, translated) = (fields.next()?, fields.next()?, fields.next()?);
            (fields.next().is_none() && !locale.is_empty() && !english.is_empty() && !translated.is_empty()).then_some(((locale, english), translated))
        })
        .collect()
}

/// `english` in the language `code` (`de`, `pt-br`…), when the table has it. English itself is
/// the source and never looked up.
pub(super) fn get(code: &str, english: &str) -> Option<&'static str> {
    if code.eq_ignore_ascii_case("en") {
        return None;
    }
    let code = code.to_ascii_lowercase();
    rows().get(&(code.as_str(), english)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_layershops_rows_in_every_language_it_had() {
        assert_eq!(get("de", "Generative AI…"), Some("Generative KI…"));
        assert_eq!(get("pt-br", "AI Gen"), Some("AI Gen"));
        assert!(get("sr-latn", "Download").is_some(), "Serbian is keyed by its current code");
        let locales: std::collections::HashSet<_> = rows().keys().map(|(l, _)| *l).collect();
        assert_eq!(locales.len(), 20, "{locales:?}");
        for locale in &locales {
            assert!(super::super::LANGUAGES.iter().any(|l| l.code == *locale), "{locale} is a language the app knows");
        }
    }

    #[test]
    fn english_and_unknown_strings_are_not_looked_up() {
        assert_eq!(get("en", "Download"), None);
        assert_eq!(get("de", "Not a LayerShop string"), None);
        assert_eq!(get("xx", "Download"), None);
        assert_eq!(get("DE", "Download"), get("de", "Download"), "codes match in any case");
    }

    /// Through [`super::super::tr`], with the craft-i18n catalogs embedded (`CRAFT_I18N_DIR`):
    /// this table first, and the product named LayerShop in what the catalogs say.
    #[test]
    fn tr_reads_this_table_first_and_names_the_product_layershop() {
        if !super::super::needs_catalogs() {
            return;
        }
        let de = super::super::Lang::from_code("de").unwrap();
        assert_eq!(super::super::tr(de, "Generative AI…"), "Generative KI…");
        let about = super::super::tr(de, "About PhotoCraft");
        assert!(about.contains(crate::brand::NAME) && !about.contains("PhotoCraft"), "{about}");
    }

    #[test]
    fn comments_and_malformed_lines_are_skipped() {
        let rows = parse("# a comment\n\nde\tOnly two\nde\t\tempty\nde\tA\tB\tC\nde\tGood\tGut\r\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.get(&("de", "Good")), Some(&"Gut"));
    }
}
