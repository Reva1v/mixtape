//! Native YouTube Music identifiers, kept as distinct types so they cannot be mixed up.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        id_type!(@base $(#[$doc])* $name);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d).map(Self)
            }
        }
    };
    (@base $(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, ts_rs::TS)]
        #[serde(transparent)]
        #[ts(export, type = "string")]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

id_type!(@base
    /// YouTube `videoId` (11 characters) or a local id (`local…`, `sc…`, `bc…`, …).
    ///
    /// Track ids end up in file names (`library/<id>.<ext>`, the stream cache) and on the
    /// yt-dlp command line, so anything coming from outside — IPC, JSON, URLs — goes through
    /// [`TrackId::parse`] or deserialisation, which accept only `[A-Za-z0-9_-]{1,64}`.
    /// `new` / `From<&str>` are for ids the app builds itself.
    TrackId
);

impl TrackId {
    pub const MAX_LEN: usize = 64;

    /// `true` for `[A-Za-z0-9_-]{1,64}`: safe as a file name and inside a command line.
    pub fn is_valid(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= Self::MAX_LEN
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    /// A track id from untrusted input, `None` when it is not [`valid`](Self::is_valid).
    pub fn parse(s: &str) -> Option<Self> {
        Self::is_valid(s).then(|| Self(s.to_owned()))
    }

    /// Whether this id is safe to use in a path or a command line.
    pub fn is_safe(&self) -> bool {
        Self::is_valid(&self.0)
    }
}

impl<'de> Deserialize<'de> for TrackId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(serde::de::Error::custom(format!(
                "invalid track id {:?}: expected 1-64 of [A-Za-z0-9_-]",
                s.chars().take(80).collect::<String>()
            )))
        }
    }
}
id_type!(
    /// `MPRE…` / `OLAK…` browse id.
    AlbumId
);
id_type!(
    /// `UC…` channel id.
    ArtistId
);
id_type!(
    /// `VL…` / `PL…` for YouTube Music playlists, `local:<uuid>` for local ones.
    PlaylistId
);

impl PlaylistId {
    pub const LOCAL_PREFIX: &'static str = "local:";

    pub fn is_local(&self) -> bool {
        self.0.starts_with(Self::LOCAL_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_the_app_uses_are_valid() {
        for id in [
            "dQw4w9WgXcQ",
            "-_abcDEF123",
            "local0123456789abcdef",
            "sc0123456789abcdef",
            "bc0123456789abcdef",
            "syn00001",
            "t1",
        ] {
            assert!(TrackId::is_valid(id), "{id}");
            let parsed: TrackId = serde_json::from_str(&format!("\"{id}\"")).unwrap();
            assert_eq!(parsed.as_str(), id);
        }
    }

    #[test]
    fn unsafe_track_ids_are_rejected() {
        let long = "a".repeat(65);
        for id in ["", "../x", "a/b", "a\\b", "C:x", "a b", "id.webm", "\u{430}", "x\0", long.as_str()] {
            assert!(TrackId::parse(id).is_none(), "{id:?}");
            assert!(
                serde_json::from_value::<TrackId>(serde_json::Value::String(id.into())).is_err(),
                "{id:?}"
            );
        }
    }

    #[test]
    fn other_ids_stay_unrestricted() {
        let p: PlaylistId = serde_json::from_str("\"local:1234-abcd\"").unwrap();
        assert!(p.is_local());
    }
}
