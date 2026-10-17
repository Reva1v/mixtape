//! Downloads into the library: the persistent job queue, the chunked file fetcher,
//! source synchronisation, link import and the free-space guard.

pub mod disk;
pub mod fetch;
pub mod sources;
pub mod url_import;
