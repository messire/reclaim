//! Scriptable, deterministic fake implementation of Reclaim's platform ports.
//!
//! Used by tests of `reclaim-core` consumers and `reclaim-engine` to simulate access
//! denied, process exit races, PID reuse, slow calls and metric sequences
//! (ARCHITECTURE §10). All clones of a [`FakePlatform`] share one state, so a test can
//! keep a handle for scripting while the engine owns another.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use reclaim_core::model::{
    AccessHint, Capabilities, ImageLocation, ImageName, MemorySnapshot, ProcessInfo, ProcessKey,
    SystemSnapshot,
};
use reclaim_core::ports::{
    Clock, OpenError, PlatformError, ProcessHandle, ProcessSource, ReclaimAction, ReclaimError,
    Reclaimer, SessionInfo, SystemMetrics,
};
use reclaim_core::units::{Bytes, Millis};

/// How a process reacts when Reclaim acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Behaviour {
    /// Open and trim succeed.
    #[default]
    Normal,
    /// Open fails with access denied.
    DenyOpen,
    /// Open succeeds, the trim call is denied while the process runs.
    DenyTrim,
    /// The process exits right before it is opened.
    ExitBeforeOpen,
    /// The process exits after it is opened, before the trim call.
    ExitBeforeTrim,
    /// The PID was reused: open finds a different start time.
    PidReused,
}

/// One scripted process.
#[derive(Debug, Clone)]
pub struct FakeProcess {
    /// What enumeration reports.
    pub info: ProcessInfo,
    /// Reaction to open/trim.
    pub behaviour: Behaviour,
    /// Working set right after a successful trim.
    pub ws_after_trim: Bytes,
    /// Simulated duration of the trim call.
    pub trim_cost: Millis,
    /// False once the process has exited.
    pub alive: bool,
    /// Number of successful trim calls.
    pub trims: u32,
}

#[derive(Debug)]
struct State {
    processes: Vec<FakeProcess>,
    system: VecDeque<SystemSnapshot>,
    last_system: SystemSnapshot,
    now: Millis,
    wall_origin: Millis,
    self_pid: u32,
    foreground: Option<ProcessKey>,
    capabilities: Capabilities,
    fail_enumerate: bool,
    log: Vec<String>,
}

/// The fake platform. Cheap to clone; clones share state.
#[derive(Debug, Clone)]
pub struct FakePlatform {
    state: Arc<Mutex<State>>,
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a process with sensible defaults: accessible, not critical, private bytes equal
/// to the working set.
pub fn process(pid: u32, image: &str, ws_mib: u64) -> FakeProcess {
    FakeProcess {
        info: ProcessInfo {
            key: ProcessKey::new(pid, 1_000 + u64::from(pid)),
            image: ImageName::new(image),
            location: ImageLocation::Other,
            session_id: 1,
            access: AccessHint::Accessible,
            critical: Some(false),
            memory: Some(MemorySnapshot {
                working_set: Bytes::from_mib(ws_mib),
                private_working_set: Some(Bytes::from_mib(ws_mib)),
                private_bytes: Bytes::from_mib(ws_mib),
                page_faults: 0,
                io_bytes: Some(Bytes::ZERO),
            }),
        },
        behaviour: Behaviour::Normal,
        ws_after_trim: Bytes::ZERO,
        trim_cost: Millis(1),
        alive: true,
        trims: 0,
    }
}

impl FakeProcess {
    /// Sets the behaviour.
    pub fn behaviour(mut self, b: Behaviour) -> Self {
        self.behaviour = b;
        self
    }

    /// Sets the location class.
    pub fn location(mut self, l: ImageLocation) -> Self {
        self.info.location = l;
        self
    }

    /// Sets the access hint.
    pub fn access(mut self, a: AccessHint) -> Self {
        self.info.access = a;
        self
    }

    /// Sets the simulated trim duration.
    pub fn trim_cost(mut self, ms: u64) -> Self {
        self.trim_cost = Millis(ms);
        self
    }

    /// Sets the working set left after a trim.
    pub fn ws_after_trim_mib(mut self, mib: u64) -> Self {
        self.ws_after_trim = Bytes::from_mib(mib);
        self
    }
}

impl FakePlatform {
    /// An empty platform: no processes, a 16 GiB system, Reclaim as PID 1.
    pub fn new() -> Self {
        let sys = SystemSnapshot {
            phys_total: Bytes::from_mib(16_384),
            phys_available: Bytes::from_mib(8_192),
            commit_charge: Bytes::from_mib(10_000),
            commit_limit: Bytes::from_mib(24_000),
            ..Default::default()
        };
        Self {
            state: Arc::new(Mutex::new(State {
                processes: Vec::new(),
                system: VecDeque::new(),
                last_system: sys,
                now: Millis(0),
                wall_origin: Millis(1_700_000_000_000),
                self_pid: 1,
                foreground: None,
                capabilities: Capabilities::all(),
                fail_enumerate: false,
                log: Vec::new(),
            })),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds a process.
    pub fn add(&self, p: FakeProcess) -> ProcessKey {
        let key = p.info.key;
        self.st().processes.push(p);
        key
    }

    /// Queues system samples; the last one repeats once the queue is empty.
    pub fn push_system(&self, s: SystemSnapshot) {
        self.st().system.push_back(s);
    }

    /// Sets the foreground process.
    pub fn set_foreground(&self, key: Option<ProcessKey>) {
        self.st().foreground = key;
    }

    /// Sets Reclaim's own PID.
    pub fn set_self_pid(&self, pid: u32) {
        self.st().self_pid = pid;
    }

    /// Replaces the capabilities.
    pub fn set_capabilities(&self, c: Capabilities) {
        self.st().capabilities = c;
    }

    /// Makes `enumerate` fail.
    pub fn fail_enumerate(&self, fail: bool) {
        self.st().fail_enumerate = fail;
    }

    /// Advances the clock.
    pub fn advance(&self, ms: u64) {
        let mut s = self.st();
        s.now = s.now.plus(ms);
    }

    /// Simulates a process touching memory again: sets its working set and adds faults.
    pub fn touch(&self, key: ProcessKey, ws_mib: u64, faults: u64) {
        let mut s = self.st();
        if let Some(p) = s.processes.iter_mut().find(|p| p.info.key == key)
            && let Some(m) = p.info.memory.as_mut()
        {
            m.working_set = Bytes::from_mib(ws_mib);
            m.private_working_set = Some(Bytes::from_mib(ws_mib));
            m.page_faults += faults;
        }
    }

    /// Ends a process.
    pub fn exit(&self, key: ProcessKey) {
        if let Some(p) = self.st().processes.iter_mut().find(|p| p.info.key == key) {
            p.alive = false;
        }
    }

    /// Snapshot of a scripted process.
    pub fn get(&self, key: ProcessKey) -> Option<FakeProcess> {
        self.st()
            .processes
            .iter()
            .find(|p| p.info.key == key)
            .cloned()
    }

    /// Calls made so far, for assertions.
    pub fn log(&self) -> Vec<String> {
        self.st().log.clone()
    }
}

impl SystemMetrics for FakePlatform {
    fn sample(&mut self) -> Result<SystemSnapshot, PlatformError> {
        let mut s = self.st();
        if let Some(next) = s.system.pop_front() {
            s.last_system = next;
        }
        s.log.push("sample".into());
        Ok(s.last_system)
    }
}

/// Handle returned by [`FakePlatform::open`].
#[derive(Debug)]
pub struct FakeHandle {
    key: ProcessKey,
    state: Arc<Mutex<State>>,
}

impl ProcessHandle for FakeHandle {
    fn key(&self) -> ProcessKey {
        self.key
    }

    fn memory(&self) -> Result<MemorySnapshot, PlatformError> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let p = s.processes.iter().find(|p| p.info.key == self.key);
        match p {
            // An exited process still answers with an empty working set (spike A13).
            Some(p) if !p.alive => Ok(MemorySnapshot {
                working_set: Bytes::ZERO,
                private_working_set: Some(Bytes::ZERO),
                ..p.info.memory.unwrap_or_default()
            }),
            Some(p) => p.info.memory.ok_or(PlatformError::new("memory", Some(5))),
            None => Err(PlatformError::new("memory", Some(87))),
        }
    }

    fn has_exited(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.processes
            .iter()
            .find(|p| p.info.key == self.key)
            .is_none_or(|p| !p.alive)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ProcessSource for FakePlatform {
    fn enumerate(&mut self) -> Result<Vec<ProcessInfo>, PlatformError> {
        let mut s = self.st();
        s.log.push("enumerate".into());
        if s.fail_enumerate {
            return Err(PlatformError::new("enumerate", Some(31)));
        }
        Ok(s.processes
            .iter()
            .filter(|p| p.alive)
            .map(|p| p.info.clone())
            .collect())
    }

    fn open(&mut self, key: ProcessKey) -> Result<Box<dyn ProcessHandle>, OpenError> {
        let mut s = self.st();
        s.log.push(format!("open {}", key.pid));
        let Some(p) = s.processes.iter_mut().find(|p| p.info.key.pid == key.pid) else {
            return Err(OpenError::Exited);
        };
        match p.behaviour {
            Behaviour::ExitBeforeOpen => {
                p.alive = false;
                return Err(OpenError::Exited);
            }
            Behaviour::DenyOpen => return Err(OpenError::AccessDenied),
            Behaviour::PidReused => return Err(OpenError::IdentityChanged),
            _ => {}
        }
        if !p.alive {
            return Err(OpenError::Exited);
        }
        if p.info.key != key {
            return Err(OpenError::IdentityChanged);
        }
        Ok(Box::new(FakeHandle {
            key,
            state: Arc::clone(&self.state),
        }))
    }
}

impl Reclaimer for FakePlatform {
    fn capabilities(&self) -> Capabilities {
        self.st().capabilities.clone()
    }

    fn reclaim(
        &mut self,
        h: &dyn ProcessHandle,
        action: ReclaimAction,
    ) -> Result<(), ReclaimError> {
        let ReclaimAction::TrimWorkingSet = action;
        let mut s = self.st();
        if !s.capabilities.trim_working_set.is_available() {
            return Err(ReclaimError::Unsupported);
        }
        let key = h.key();
        s.log.push(format!("trim {}", key.pid));
        let Some(idx) = s.processes.iter().position(|p| p.info.key == key) else {
            return Err(ReclaimError::Exited);
        };
        let cost = s.processes[idx].trim_cost;
        s.now = s.now.plus(cost.0);
        let p = &mut s.processes[idx];
        if p.behaviour == Behaviour::ExitBeforeTrim {
            p.alive = false;
        }
        if !p.alive {
            return Err(ReclaimError::Exited);
        }
        if p.behaviour == Behaviour::DenyTrim {
            return Err(ReclaimError::AccessDenied);
        }
        p.trims += 1;
        let after = p.ws_after_trim;
        if let Some(m) = p.info.memory.as_mut() {
            m.working_set = after;
            m.private_working_set = Some(after);
        }
        Ok(())
    }
}

impl SessionInfo for FakePlatform {
    fn self_pid(&self) -> u32 {
        self.st().self_pid
    }

    fn foreground_process(&self) -> Option<ProcessKey> {
        self.st().foreground
    }
}

impl Clock for FakePlatform {
    fn monotonic(&self) -> Millis {
        self.st().now
    }

    fn wall(&self) -> Millis {
        let s = self.st();
        s.wall_origin.plus(s.now.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_open_and_trim() {
        let mut f = FakePlatform::new();
        let ok = f.add(process(10, "a.exe", 300).ws_after_trim_mib(5).trim_cost(40));
        let denied = f.add(process(11, "b.exe", 300).behaviour(Behaviour::DenyOpen));
        let deny_trim = f.add(process(12, "c.exe", 300).behaviour(Behaviour::DenyTrim));
        let exits = f.add(process(13, "d.exe", 300).behaviour(Behaviour::ExitBeforeTrim));
        let gone = f.add(process(14, "e.exe", 300).behaviour(Behaviour::ExitBeforeOpen));
        let reused = f.add(process(15, "f.exe", 300).behaviour(Behaviour::PidReused));

        let h = f.open(ok).unwrap();
        assert_eq!(h.key(), ok);
        assert!(h.as_any().downcast_ref::<FakeHandle>().is_some());
        f.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet)
            .unwrap();
        assert_eq!(h.memory().unwrap().working_set, Bytes::from_mib(5));
        assert_eq!(h.memory().unwrap().private_bytes, Bytes::from_mib(300));
        assert_eq!(f.monotonic(), Millis(40));
        assert_eq!(f.get(ok).unwrap().trims, 1);

        assert_eq!(f.open(denied).unwrap_err(), OpenError::AccessDenied);
        let h = f.open(deny_trim).unwrap();
        assert_eq!(
            f.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet),
            Err(ReclaimError::AccessDenied)
        );
        let h = f.open(exits).unwrap();
        assert!(!h.has_exited());
        assert_eq!(
            f.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet),
            Err(ReclaimError::Exited)
        );
        assert!(h.has_exited());
        assert_eq!(h.memory().unwrap().working_set, Bytes::ZERO);
        assert_eq!(f.open(gone).unwrap_err(), OpenError::Exited);
        assert_eq!(f.open(reused).unwrap_err(), OpenError::IdentityChanged);
        assert_eq!(
            f.open(ProcessKey::new(10, 1)).unwrap_err(),
            OpenError::IdentityChanged
        );
        assert_eq!(
            f.open(ProcessKey::new(99, 1)).unwrap_err(),
            OpenError::Exited
        );
        assert_eq!(
            f.enumerate().unwrap().len(),
            4,
            "exited processes disappear"
        );
        assert!(f.log().contains(&"trim 10".to_string()));
    }

    #[test]
    fn metrics_session_and_clock() {
        let mut f = FakePlatform::new();
        let first = f.sample().unwrap();
        let next = SystemSnapshot {
            phys_available: Bytes::from_mib(1),
            ..first
        };
        f.push_system(next);
        assert_eq!(f.sample().unwrap(), next);
        assert_eq!(f.sample().unwrap(), next, "last sample repeats");
        assert_eq!(f.sample_detailed().unwrap(), next);

        let k = f.add(process(20, "x.exe", 50));
        f.set_foreground(Some(k));
        f.set_self_pid(7);
        assert_eq!(f.foreground_process(), Some(k));
        assert_eq!(f.self_pid(), 7);
        f.touch(k, 80, 1000);
        let p = f.get(k).unwrap();
        assert_eq!(p.info.memory.unwrap().working_set, Bytes::from_mib(80));
        assert_eq!(p.info.memory.unwrap().page_faults, 1000);
        f.advance(500);
        assert_eq!(f.monotonic(), Millis(500));
        assert_eq!(f.wall(), Millis(1_700_000_000_500));
        f.exit(k);
        assert!(f.enumerate().unwrap().is_empty());
        f.fail_enumerate(true);
        assert!(f.enumerate().is_err());
    }

    #[test]
    fn unsupported_capability() {
        let mut f = FakePlatform::default();
        let k = f.add(process(30, "x.exe", 50));
        f.set_capabilities(Capabilities::none("test"));
        assert!(!f.capabilities().trim_working_set.is_available());
        let h = f.open(k).unwrap();
        assert_eq!(
            f.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet),
            Err(ReclaimError::Unsupported)
        );
        let _ = f
            .clone()
            .get(k)
            .unwrap()
            .location(ImageLocation::System)
            .access(AccessHint::OtherUser);
    }
}
