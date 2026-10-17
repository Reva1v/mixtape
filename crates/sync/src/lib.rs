//! Downloads into the library: chunked file fetching, source synchronisation, link
//! parsing and the free-space guard. The job queue itself lives in `mixtape_library::jobs`.

pub mod disk;
pub mod downloader;
pub mod fetch;
pub mod sources;
pub mod url_import;
