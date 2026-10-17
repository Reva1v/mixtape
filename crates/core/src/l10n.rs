//! The interface language. The UI picks it in settings and translates its own strings;
//! the core reads it for the few messages it writes itself (errors, notifications,
//! taskbar tips) and for the language YouTube answers in.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Lang {
    #[default]
    En,
    Uk,
    Ru,
}

impl Lang {
    /// ISO 639-1 code, also YouTube's `hl`.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Uk => "uk",
            Self::Ru => "ru",
        }
    }

    /// The language for a system locale such as `uk-UA` or `ru`; English otherwise.
    pub fn from_locale(tag: &str) -> Self {
        match tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
            "uk" => Self::Uk,
            "ru" | "be" => Self::Ru,
            _ => Self::En,
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Uk,
        2 => Lang::Ru,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

/// The current language's variant of a fixed message.
pub fn tr(en: &'static str, uk: &'static str, ru: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Uk => uk,
        Lang::Ru => ru,
    }
}

/// [`tr`] for messages with arguments: `trf!("Can't play: {e}", "Не вдалося відтворити: {e}", "Не удалось воспроизвести: {e}")`.
#[macro_export]
macro_rules! trf {
    ($en:literal, $uk:literal, $ru:literal $(, $arg:expr)* $(,)?) => {
        match $crate::l10n::lang() {
            $crate::l10n::Lang::En => format!($en $(, $arg)*),
            $crate::l10n::Lang::Uk => format!($uk $(, $arg)*),
            $crate::l10n::Lang::Ru => format!($ru $(, $arg)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_current_language() {
        assert_eq!(Lang::from_locale("uk-UA"), Lang::Uk);
        assert_eq!(Lang::from_locale("ru_RU"), Lang::Ru);
        assert_eq!(Lang::from_locale("de-DE"), Lang::En);
        let n = 3;
        for (l, want) in [(Lang::Uk, "3 треки"), (Lang::Ru, "3 трека"), (Lang::En, "3 tracks")] {
            set_lang(l);
            assert_eq!(trf!("{n} tracks", "{n} треки", "{n} трека"), want);
            assert_eq!(tr("a", "b", "c"), ["a", "b", "c"][l as usize]);
        }
    }
}
