//! Windows adapter of Reclaim's storage ports.
//!
//! - `WinFs`: handle-based directory listing (`FILE_ID_BOTH_DIR_INFO`) with allocated
//!   sizes, file ids and reparse-tag-aware link detection; long paths throughout.
//! - `WinVolumes`: drives, labels, free space and seek penalty.
//! - `WinRemover`: deletion and recycling that revalidate the very object they act
//!   on (open handle: identity, type, reparse point, final path, guard).
//! - `known_folders`, `shell` (Explorer, properties, elevation, dialogs),
//!   `fast` (NTFS scan from `FSCTL_QUERY_FILE_LAYOUT`) and `watch` (change
//!   notifications for refreshes).
//!
//! All `unsafe` FFI code lives in the private `win` module, each block with a
//! `// SAFETY:` comment. On other platforms the crate is empty (its pure parsers are
//! compiled for their unit tests only).

#[cfg(any(windows, test))]
mod common;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{WinFs, WinRemover, WinVolumes, fast, known_folders, shell, watch};
