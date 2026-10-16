//! Native YouTube Music identifiers, kept as distinct types so they cannot be mixed up.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ts_rs::TS)]
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

id_type!(
    /// YouTube `videoId`, 11 characters.
    TrackId
);
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
