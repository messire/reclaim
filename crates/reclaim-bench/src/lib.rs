//! Benchmark harness, probe workloads and research spikes for Reclaim
//! (`docs/architecture/BENCHMARKS.md`, PLAN M5).
//!
//! Nothing in this crate is shipped. It may use APIs that the product must not
//! use (for example to measure or to clean up probes), see ADR-0003.
//!
//! The library part is platform independent and tested over the fake platform: statistics,
//! the arm driver, the soak loop and trend check, and the summary writer. The Windows
//! binaries (`reclaim-scenario`, `reclaim-soak`) only add process spawning and OS counters.

pub mod probe;
pub mod run;
pub mod spawn;
pub mod stats;
pub mod summary;
