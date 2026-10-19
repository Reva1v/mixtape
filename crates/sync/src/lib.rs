//! Downloads into the library: chunked file fetching, streaming into a sparse cache file,
//! source synchronisation, link parsing and the free-space guard. The job queue itself lives
//! in `mixtape_library::jobs`.

pub mod artists;
pub mod catalog;
pub mod disk;
pub mod downloader;
pub mod external;
pub mod fetch;
pub mod lastfm;
pub mod likes;
pub mod lyrics;
pub mod sources;
pub mod spotify;
pub mod stream;
pub mod url_import;

pub use reqwest;

#[cfg(test)]
mod test_http;
