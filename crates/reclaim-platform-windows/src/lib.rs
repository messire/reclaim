//! Windows implementation of Reclaim's platform ports.
//!
//! All `unsafe` FFI code of the product lives here, each block with a `// SAFETY:`
//! comment. Policy decisions never live here: the adapter reports facts and errors, and
//! `reclaim-core` decides. On other platforms the crate is empty.

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{WinHandle, WindowsPlatform, own_private_bytes};
