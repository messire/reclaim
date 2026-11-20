//! Platform ports implemented by the OS adapters (ARCHITECTURE §6).

use std::fmt;

use crate::model::{Capabilities, MemorySnapshot, ProcessInfo, ProcessKey, SystemSnapshot};
use crate::outcome::FailureKind;
use crate::units::Millis;

/// A platform call failed in a way that is not about a specific process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    /// What was being done.
    pub operation: &'static str,
    /// OS error code, if any.
    pub code: Option<i64>,
}

impl PlatformError {
    /// Creates an error.
    pub const fn new(operation: &'static str, code: Option<i64>) -> Self {
        Self { operation, code }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(c) => write!(f, "{} failed (error {c})", self.operation),
            None => write!(f, "{} failed", self.operation),
        }
    }
}

impl std::error::Error for PlatformError {}

/// Opening a process for action failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Access denied (elevated, other user, protected).
    AccessDenied,
    /// The process no longer exists.
    Exited,
    /// The PID belongs to a different process now (start time mismatch).
    IdentityChanged,
    /// Other OS error.
    Os(i64),
}

impl From<OpenError> for FailureKind {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::AccessDenied => FailureKind::AccessDenied,
            OpenError::Exited => FailureKind::Exited,
            OpenError::IdentityChanged => FailureKind::IdentityChanged,
            OpenError::Os(c) => FailureKind::Os(c),
        }
    }
}

/// A reclaim action failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReclaimError {
    /// Access denied while the process was still running.
    AccessDenied,
    /// The process had exited (checked before interpreting access denied, spike A13).
    Exited,
    /// The action is not supported.
    Unsupported,
    /// Other OS error.
    Os(i64),
}

impl From<ReclaimError> for FailureKind {
    fn from(e: ReclaimError) -> Self {
        match e {
            ReclaimError::AccessDenied => FailureKind::AccessDenied,
            ReclaimError::Exited => FailureKind::Exited,
            ReclaimError::Unsupported => FailureKind::Unsupported,
            ReclaimError::Os(c) => FailureKind::Os(c),
        }
    }
}

/// What to do to a process. An intent, not an API call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReclaimAction {
    /// Remove as many pages as possible from the working set (Windows).
    TrimWorkingSet,
}

/// System-wide metrics.
pub trait SystemMetrics {
    /// Takes a cheap sample (no optional breakdown).
    fn sample(&mut self) -> Result<SystemSnapshot, PlatformError>;
    /// Takes a sample including optional, more expensive counters where available.
    fn sample_detailed(&mut self) -> Result<SystemSnapshot, PlatformError> {
        self.sample()
    }
    /// Loads or unloads the optional detailed counters; returns whether they are loaded.
    fn set_details(&mut self, _on: bool) -> bool {
        false
    }
}

/// An open, validated process. Holding it keeps the identity valid.
pub trait ProcessHandle: fmt::Debug {
    /// Identity validated at open time.
    fn key(&self) -> ProcessKey;
    /// Current memory counters.
    fn memory(&self) -> Result<MemorySnapshot, PlatformError>;
    /// True if the process has exited.
    fn has_exited(&self) -> bool;
    /// Lets an adapter recover its own concrete handle type.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Enumerates and opens processes.
pub trait ProcessSource {
    /// Lists processes with their memory counters.
    fn enumerate(&mut self) -> Result<Vec<ProcessInfo>, PlatformError>;
    /// Opens a process for action and validates its identity.
    fn open(&mut self, key: ProcessKey) -> Result<Box<dyn ProcessHandle>, OpenError>;
}

/// Performs reclaim actions.
pub trait Reclaimer {
    /// What this adapter can do.
    fn capabilities(&self) -> Capabilities;
    /// Acts on an open process.
    fn reclaim(&mut self, h: &dyn ProcessHandle, action: ReclaimAction)
    -> Result<(), ReclaimError>;
}

/// Session facts.
pub trait SessionInfo {
    /// Reclaim's own PID.
    fn self_pid(&self) -> u32;
    /// Owner of the foreground window.
    fn foreground_process(&self) -> Option<ProcessKey>;
}

/// Time source. Monotonic for intervals, wall clock for records.
pub trait Clock {
    /// Monotonic milliseconds since an arbitrary origin.
    fn monotonic(&self) -> Millis;
    /// Wall-clock Unix milliseconds.
    fn wall(&self) -> Millis;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_failures() {
        assert_eq!(
            FailureKind::from(OpenError::AccessDenied),
            FailureKind::AccessDenied
        );
        assert_eq!(FailureKind::from(OpenError::Exited), FailureKind::Exited);
        assert_eq!(
            FailureKind::from(OpenError::IdentityChanged),
            FailureKind::IdentityChanged
        );
        assert_eq!(FailureKind::from(OpenError::Os(3)), FailureKind::Os(3));
        assert_eq!(
            FailureKind::from(ReclaimError::AccessDenied),
            FailureKind::AccessDenied
        );
        assert_eq!(FailureKind::from(ReclaimError::Exited), FailureKind::Exited);
        assert_eq!(
            FailureKind::from(ReclaimError::Unsupported),
            FailureKind::Unsupported
        );
        assert_eq!(FailureKind::from(ReclaimError::Os(9)), FailureKind::Os(9));
        assert_eq!(
            PlatformError::new("sample", Some(5)).to_string(),
            "sample failed (error 5)"
        );
        assert_eq!(
            PlatformError::new("sample", None).to_string(),
            "sample failed"
        );
    }
}
