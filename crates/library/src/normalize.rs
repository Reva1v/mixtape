//! Text normalisation for search, and wrong-keyboard-layout conversion.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lowercases, folds `ё` to `е`, strips diacritics from Latin letters (`é` → `e`) and
/// collapses whitespace. Cyrillic letters keep their marks, so `й` stays `й`.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_latin = false;
    let mut pending_space = false;
    for c in s.nfkd() {
        if is_combining_mark(c) {
            if !prev_latin {
                out.push(c);
            }
            continue;
        }
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        prev_latin = c.is_ascii_alphabetic();
        for l in c.to_lowercase() {
            out.push(if l == 'ё' { 'е' } else { l });
        }
    }
    // Re-compose what we kept (й = и + breve).
    out.nfc().collect::<String>().replace('ё', "е")
}

const LATIN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const CYRILLIC: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

/// Re-types `s` as if it had been typed with the other keyboard layout
/// (`ьфкл` → `mark`, `ghbdtn` → `привет`). Returns `None` when nothing changes.
pub fn swap_layout(s: &str) -> Option<String> {
    let mut changed = false;
    let out: String = s
        .chars()
        .map(|c| {
            let lc = c.to_lowercase().next().unwrap_or(c);
            if let Some(i) = LATIN.chars().position(|x| x == lc) {
                changed = true;
                CYRILLIC.chars().nth(i).unwrap_or(c)
            } else if let Some(i) = CYRILLIC.chars().position(|x| x == lc) {
                changed = true;
                LATIN.chars().nth(i).unwrap_or(c)
            } else {
                lc
            }
        })
        .collect();
    changed.then_some(out)
}

/// Builds the text a track is found by: title, artists and album, normalised.
pub fn track_search_text(title: &str, artists: &[&str], album: Option<&str>) -> String {
    let mut s = String::from(title);
    for a in artists {
        s.push(' ');
        s.push_str(a);
    }
    if let Some(al) = album {
        s.push(' ');
        s.push_str(al);
    }
    normalize(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_yo_and_latin_accents() {
        assert_eq!(normalize("Beyoncé  —  ЁЛКА"), "beyonce — елка");
        assert_eq!(normalize("Мой  Йогурт"), "мой йогурт");
        assert_eq!(normalize("  Sigur Rós "), "sigur ros");
    }

    #[test]
    fn swaps_layout_both_ways() {
        assert_eq!(swap_layout("ьфкл").as_deref(), Some("mark"));
        assert_eq!(swap_layout("ghbdtn").as_deref(), Some("привет"));
        assert_eq!(swap_layout("123"), None);
    }

    #[test]
    fn builds_track_search_text() {
        assert_eq!(
            track_search_text("Take On Me", &["a-ha"], Some("Hunting High and Low")),
            "take on me a-ha hunting high and low"
        );
    }
}
