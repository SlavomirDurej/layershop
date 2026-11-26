//! Named references of an edit (`ai.generate`'s `references`): a picture the prompt calls by name,
//! `@Luna`, as one would a character or a prop.
//!
//! The model numbers its pictures (the one it edits is image 1, the references follow), so a
//! mention is written out for it as `Luna (image 2)`. When the prompt mentions any reference only
//! the mentioned ones are sent, so a reference kept for later does not wander into the picture;
//! a prompt that mentions none is sent every reference, which it can call image 2, image 3, …

use std::ops::Range;

/// The longest name a reference can have, in characters.
pub const MAX_NAME: usize = 24;

/// Can `c` be part of a name? Letters, digits, `_` and `-`: a mention ends at anything else, so
/// `@Luna's` and `@Luna,` name Luna.
pub fn name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// `raw` as a reference name: a run of spaces becomes one `_`, other characters a name can't
/// hold are dropped, and it is cut to [`MAX_NAME`].
pub fn clean_name(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join("_").chars().filter(|c| name_char(*c)).take(MAX_NAME).collect()
}

/// A name for a reference to a picture called `label` (a layer's or a document's name): its
/// file extension dropped, then [`clean_name`]. Empty when nothing usable is left.
pub fn suggest_name(label: &str) -> String {
    let label = label.trim();
    let stem = match label.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && (1..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => label,
    };
    clean_name(stem).trim_matches(|c| c == '_' || c == '-').to_string()
}

/// The first name in `names` that another before it already has (names are told apart without
/// regard to case); empty names are not compared.
pub fn duplicate<'a>(names: &[&'a str]) -> Option<&'a str> {
    names.iter().enumerate().find(|(i, n)| !n.is_empty() && names.iter().take(*i).any(|m| same(m, n))).map(|(_, n)| *n)
}

fn same(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// The bytes of `text` that `name` begins it with, compared without regard to case; `None` when
/// it doesn't begin with it.
fn prefix_len(text: &str, name: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for n in name.chars() {
        let (_, c) = chars.next()?;
        if !c.to_lowercase().eq(n.to_lowercase()) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// A mention of a reference in a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// The bytes of `@Name` in the prompt.
    pub range: Range<usize>,
    /// The reference it names: an index into the names it was looked for among.
    pub reference: usize,
}

/// The mentions in `prompt` of the references called `names`, in the order they come. A mention
/// is `@` (not inside a word: `me@luna.org` is none) and a name, matched without regard to case,
/// that a letter or digit does not continue; the longest name that fits wins. An `@` naming no
/// reference is not a mention.
pub fn mentions(prompt: &str, names: &[&str]) -> Vec<Mention> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    for (at, c) in prompt.char_indices() {
        let starts = c == '@' && !prev.is_some_and(name_char);
        prev = Some(c);
        if !starts {
            continue;
        }
        let Some(rest) = prompt.get(at + 1..) else { continue };
        let found = names
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.is_empty())
            .filter_map(|(i, n)| {
                let len = prefix_len(rest, n)?;
                let next = rest.get(len..).and_then(|r| r.chars().next());
                (!next.is_some_and(char::is_alphanumeric)).then_some((i, len))
            })
            // The longest; of two the same, the first.
            .fold(None, |best: Option<(usize, usize)>, (i, len)| if best.is_some_and(|(_, b)| b >= len) { best } else { Some((i, len)) });
        if let Some((reference, len)) = found {
            out.push(Mention { range: at..at + 1 + len, reference });
        }
    }
    out
}

/// What an edit sends: the references (indices into its names) in the order the model numbers
/// them, and the prompt the model is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub sent: Vec<usize>,
    pub prompt: String,
}

/// Which of the references named `names` (one name each, empty for none) an edit with `prompt`
/// sends, and what the model is told: see the module docs. A mention `@Luna` of the second
/// reference sent reads `Luna (image 3)`, with any `_` of the name as a space.
pub fn resolve(prompt: &str, names: &[&str]) -> Resolved {
    let found = mentions(prompt, names);
    if found.is_empty() {
        return Resolved { sent: (0..names.len()).collect(), prompt: prompt.to_string() };
    }
    let sent: Vec<usize> = (0..names.len()).filter(|i| found.iter().any(|m| m.reference == *i)).collect();
    let mut out = String::with_capacity(prompt.len() + 16 * found.len());
    let mut from = 0;
    for m in &found {
        out.push_str(prompt.get(from..m.range.start).unwrap_or(""));
        let name = names.get(m.reference).copied().unwrap_or("").replace('_', " ");
        let number = sent.iter().position(|i| *i == m.reference).map_or(0, |p| p + 2);
        out.push_str(&format!("{name} (image {number})"));
        from = m.range.end;
    }
    out.push_str(prompt.get(from..).unwrap_or(""));
    Resolved { sent, prompt: out }
}

/// `prompt` with its mentions of a reference called `old` naming `new` instead (a reference was
/// renamed). Unchanged when either name is empty.
pub fn rename(prompt: &str, old: &str, new: &str) -> String {
    if old.is_empty() || new.is_empty() {
        return prompt.to_string();
    }
    let mut out = String::with_capacity(prompt.len());
    let mut from = 0;
    for m in mentions(prompt, &[old]) {
        out.push_str(prompt.get(from..m.range.start).unwrap_or(""));
        out.push('@');
        out.push_str(new);
        from = m.range.end;
    }
    out.push_str(prompt.get(from..).unwrap_or(""));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_letters_digits_and_dashes() {
        assert_eq!(clean_name("  Luna  "), "Luna");
        assert_eq!(clean_name("red car!"), "red_car");
        assert_eq!(clean_name("Zoë-2"), "Zoë-2");
        assert_eq!(clean_name(&"x".repeat(100)).chars().count(), MAX_NAME);
        assert_eq!(clean_name("@#$"), "");
        assert_eq!(suggest_name("luna_portrait.png"), "luna_portrait");
        assert_eq!(suggest_name("Layer 1"), "Layer_1");
        assert_eq!(suggest_name("v1.2 final"), "v12_final");
        assert_eq!(suggest_name(".png"), "png");
        assert_eq!(suggest_name("  "), "");
        assert_eq!(duplicate(&["Luna", "Max", "luna"]), Some("luna"));
        assert_eq!(duplicate(&["", "", "Luna"]), None);
    }

    #[test]
    fn a_mention_is_an_at_and_a_name() {
        let names = ["Luna", "Max", "Luna_2"];
        let at = |p: &str| mentions(p, &names).into_iter().map(|m| (p[m.range].to_string(), m.reference)).collect::<Vec<_>>();
        assert_eq!(at("@Luna and @max, @Luna's cat"), [("@Luna".into(), 0), ("@max".into(), 1), ("@Luna".into(), 0)]);
        // The longest name that fits; a word that continues the name is another word.
        assert_eq!(at("@Luna_2 waves"), [("@Luna_2".into(), 2)]);
        assert_eq!(at("@Lunatic @Maxi"), []);
        assert_eq!(at("@Luna-like"), [("@Luna".into(), 0)]);
        // Not inside a word, not an @ alone, not a name that isn't there.
        assert_eq!(at("me@Luna.org @ @Bob @"), []);
        assert_eq!(at("(@Luna)"), [("@Luna".into(), 0)]);
        assert_eq!(mentions("@Luna", &["", "Luna"]).len(), 1);
        assert!(mentions("@", &[]).is_empty());
        // Wide characters before and inside.
        let p = "🦆 @Zoë!";
        assert_eq!(mentions(p, &["zoË"]).into_iter().map(|m| &p[m.range]).collect::<Vec<_>>(), ["@Zoë"]);
    }

    #[test]
    fn mentioned_references_are_sent_and_numbered() {
        let names = ["Luna", "Max", ""];
        // None mentioned: all of them, and the prompt as typed.
        assert_eq!(resolve("put the hat from image 2 on him", &names), Resolved { sent: vec![0, 1, 2], prompt: "put the hat from image 2 on him".into() });
        // Mentioned: those alone, numbered in the panel's order after the picture.
        assert_eq!(resolve("@Max hugs @luna", &names), Resolved { sent: vec![0, 1], prompt: "Max (image 3) hugs Luna (image 2)".into() });
        assert_eq!(resolve("@Max waves, then @Max sits", &names), Resolved { sent: vec![1], prompt: "Max (image 2) waves, then Max (image 2) sits".into() });
        assert_eq!(resolve("@red_car", &["red_car"]).prompt, "red car (image 2)");
        assert_eq!(resolve("", &[]), Resolved { sent: vec![], prompt: String::new() });
    }

    #[test]
    fn renaming_a_reference_follows_it_in_the_prompt() {
        assert_eq!(rename("@Luna waves at @luna's cat, @Lunatic", "Luna", "Nova"), "@Nova waves at @Nova's cat, @Lunatic");
        assert_eq!(rename("@Luna", "", "Nova"), "@Luna");
        assert_eq!(rename("@Luna", "Luna", ""), "@Luna");
    }
}
