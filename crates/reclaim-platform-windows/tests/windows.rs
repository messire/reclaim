//! Integration tests of the Windows adapter (PLAN M2.7). They act only on the probe child
//! process they start, never on other processes.
#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use reclaim_core::model::{Pressure, ProcessKey};
use reclaim_core::ports::{
    OpenError, ProcessSource, ReclaimAction, ReclaimError, Reclaimer, SessionInfo, SystemMetrics,
};
use reclaim_core::units::Bytes;
use reclaim_platform_windows::WindowsPlatform;

struct Probe {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Probe {
    fn start(mib: u32) -> Probe {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reclaim-test-probe"))
            .arg(mib.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start probe");
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ready");
        Probe {
            child,
            stdin,
            stdout,
        }
    }

    fn touch(&mut self) {
        writeln!(self.stdin.as_mut().unwrap(), "touch").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "done");
    }

    fn key(&self, p: &mut WindowsPlatform) -> ProcessKey {
        let pid = self.child.id();
        p.enumerate()
            .unwrap()
            .into_iter()
            .find(|i| i.key.pid == pid)
            .expect("probe enumerated")
            .key
    }

    fn stop(mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn system_sample_is_consistent() {
    let mut p = WindowsPlatform::new();
    let s = p.sample().unwrap();
    assert!(s.phys_total > Bytes::ZERO);
    assert!(s.phys_available <= s.phys_total);
    assert!(s.commit_charge <= s.commit_limit);
    assert!(matches!(
        s.pressure,
        Pressure::High | Pressure::Normal | Pressure::Low
    ));
    assert!(p.capabilities().trim_working_set.is_available());
    assert!(p.capabilities().pressure_signal.is_available());
}

#[test]
fn details_load_and_unload_pdh() {
    let mut p = WindowsPlatform::new();
    assert!(!p.capabilities().standby_breakdown.is_available());
    assert!(p.set_details(true));
    let s = p.sample_detailed().unwrap();
    assert!(s.standby.is_some());
    assert!(s.modified.is_some());
    assert!(p.capabilities().paging_io_rate.is_available());
    assert!(!p.set_details(false));
    assert!(p.sample_detailed().unwrap().standby.is_none());
}

#[test]
fn enumeration_finds_self_with_identity() {
    let mut p = WindowsPlatform::new();
    let list = p.enumerate().unwrap();
    assert!(list.len() > 10);
    let me = list
        .iter()
        .find(|i| i.key.pid == p.self_pid())
        .expect("self listed");
    assert!(me.key.start_time > 0);
    assert!(me.memory.is_some());
    assert_eq!(me.critical, Some(false));
    let system = list.iter().find(|i| i.key.pid == 4).expect("System listed");
    assert!(system.image.matches("System"));
    assert!(reclaim_platform_windows::own_private_bytes().is_some());
}

#[test]
fn trim_probe_reduces_working_set_not_commit() {
    let mut p = WindowsPlatform::new();
    let mut probe = Probe::start(128);
    let key = probe.key(&mut p);
    let h = p.open(key).unwrap();
    let before = h.memory().unwrap();
    assert!(before.working_set >= Bytes::from_mib(120), "{before:?}");
    p.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet)
        .unwrap();
    let after = h.memory().unwrap();
    assert!(after.working_set < Bytes::from_mib(16), "{after:?}");
    assert_eq!(
        after.private_bytes, before.private_bytes,
        "trim never changes commit"
    );

    probe.touch();
    let rebound = h.memory().unwrap();
    assert!(rebound.working_set >= Bytes::from_mib(120));
    assert!(rebound.page_faults > after.page_faults);
    assert!(!h.has_exited());
    probe.stop();
    assert!(h.has_exited());
}

#[test]
fn exit_between_open_and_trim_is_reported_as_exited() {
    let mut p = WindowsPlatform::new();
    let probe = Probe::start(16);
    let key = probe.key(&mut p);
    let h = p.open(key).unwrap();
    probe.stop();
    assert!(h.has_exited());
    assert_eq!(
        p.reclaim(h.as_ref(), ReclaimAction::TrimWorkingSet),
        Err(ReclaimError::Exited)
    );
    // The held handle keeps the PID from being reused; once released, opening fails.
    drop(h);
    assert!(matches!(
        p.open(key),
        Err(OpenError::Exited | OpenError::IdentityChanged)
    ));
}

#[test]
fn wrong_start_time_is_rejected() {
    let mut p = WindowsPlatform::new();
    let probe = Probe::start(16);
    let key = probe.key(&mut p);
    let wrong = ProcessKey::new(key.pid, key.start_time + 1);
    assert_eq!(p.open(wrong).unwrap_err(), OpenError::IdentityChanged);
    probe.stop();
}

#[test]
fn protected_system_processes_deny_trim_rights() {
    let mut p = WindowsPlatform::new();
    let list = p.enumerate().unwrap();
    for name in ["System", "csrss.exe"] {
        let info = list.iter().find(|i| i.image.matches(name)).unwrap();
        assert_eq!(
            p.open(info.key).unwrap_err(),
            OpenError::AccessDenied,
            "{name}"
        );
    }
}

#[test]
fn foreground_is_none_or_valid() {
    let p = WindowsPlatform::new();
    if let Some(k) = p.foreground_process() {
        assert!(k.pid > 0);
        assert!(k.start_time > 0);
    }
}
