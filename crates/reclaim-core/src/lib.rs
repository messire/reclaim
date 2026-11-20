//! Platform-independent core of Reclaim.
//!
//! Holds the domain model, policy (exclusions and eligibility), run outcomes,
//! analysis and report wording. It never calls the OS, spawns threads or does
//! I/O; see `docs/architecture/ARCHITECTURE.md` §3.
//!
//! M0 skeleton: modules arrive in milestone M1.

/// Crate version, exposed for the About screen and run records.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
