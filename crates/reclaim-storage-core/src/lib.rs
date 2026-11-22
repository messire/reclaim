//! Platform-independent core of Reclaim's storage analyzer and cleaner.
//!
//! - [`tree`]: the compact scan index with incremental aggregation and hard-link
//!   accounting; [`query`]: extension statistics, largest items, filters;
//!   [`treemap`]: treemap and icicle geometry.
//! - [`cleanup`]: cleanup categories, candidates and results; [`guard`]: the
//!   protected-path policy; [`dupes`]: duplicate grouping and the removal rule.
//! - [`ports`]: the traits a platform adapter implements.
//!
//! No OS calls, threads or I/O here (see `docs/storage/ARCHITECTURE.md`).

pub mod cleanup;
pub mod dupes;
pub mod entry;
pub mod guard;
pub mod path;
pub mod ports;
pub mod query;
pub mod tree;
pub mod treemap;

pub use entry::{Attrs, EntryMeta, ErrorKind, Kind, ScanError};
pub use path::{NormPath, PathStyle};
pub use tree::{NodeId, SizeBasis, TOP, Tree};
