//! Downloads into the library: chunked file fetching, streaming into a sparse cache file,
//! source synchronisation, link parsing and the free-space guard. The job queue itself lives
//! in `mixtape_library::jobs`.

pub mod artists;
pub mod disk;
pub mod downloader;
pub mod fetch;
pub mod likes;
pub mod lyrics;
pub mod sources;
pub mod stream;
pub mod url_import;

pub use reqwest;

#[cfg(test)]
mod test_http;
