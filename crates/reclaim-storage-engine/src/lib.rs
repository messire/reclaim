//! Storage engine of Reclaim: the parallel scanner, cleanup discovery and execution,
//! duplicate hashing, export and local settings/history. Plain `std` threads that exist
//! only while an operation runs; no OS FFI (adapters implement the ports of
//! `reclaim-storage-core`).

pub mod cleanup;
pub mod dupes;
pub mod export;
pub mod fake;
pub mod scan;
pub mod stdfs;
pub mod store;

pub use scan::{
    ScanControl, ScanHandle, ScanOptions, ScanProgress, ScanState, ScanSummary, ScanTarget,
};
pub use stdfs::StdFs;
pub use store::{StorageSettings, StorageStore};
