//! Windows implementation of Reclaim's platform ports.
//!
//! All `unsafe` FFI code of the product lives here, each block with a
//! `// SAFETY:` comment. Policy decisions never live here.
//!
//! M0 skeleton: the adapter arrives in milestone M2, informed by the API spike
//! (`crates/reclaim-bench/src/bin/api_spike`).
