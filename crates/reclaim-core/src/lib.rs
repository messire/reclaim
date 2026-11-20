//! Platform-independent core of Reclaim.
//!
//! Holds the domain model, policy (exclusions and eligibility), run plans and outcomes,
//! analysis and report wording, and the platform ports. It never calls the OS, spawns
//! threads or does I/O; see `docs/architecture/ARCHITECTURE.md` §3.

pub mod analysis;
pub mod model;
pub mod outcome;
pub mod plan;
pub mod policy;
pub mod ports;
pub mod report;
pub mod units;

pub use model::{
    AccessHint, Availability, Capabilities, ImageLocation, ImageName, MemorySnapshot, MetricId,
    Pressure, ProcessInfo, ProcessKey, SystemSnapshot,
};
pub use units::{ByteDelta, Bytes, Millis};

/// Crate version, exposed for the About screen and run records.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
