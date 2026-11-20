//! Starting and stopping probe processes (`reclaim-probe`) from a harness.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::probe::parse_latency_line;

/// Pass times observed from a `latency` probe: (when received, pass µs).
pub type Passes = Arc<Mutex<Vec<(Instant, u64)>>>;

/// A running probe. Dropping it closes its stdin (the probe exits) and reaps it.
#[derive(Debug)]
pub struct Probe {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Latency samples, for `latency` probes.
    pub passes: Option<Passes>,
}

/// Path of `reclaim-probe` next to the running harness.
pub fn probe_exe() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_default();
    let name = if cfg!(windows) {
        "reclaim-probe.exe"
    } else {
        "reclaim-probe"
    };
    me.with_file_name(name)
}

impl Probe {
    /// Starts a probe and waits for its `ready` line.
    pub fn start(args: &[&str]) -> Result<Probe, String> {
        let mut child = Command::new(probe_exe())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", probe_exe().display()))?;
        let stdin = child.stdin.take();
        let mut out = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut line = String::new();
        out.read_line(&mut line).map_err(|e| e.to_string())?;
        if line.trim() != "ready" {
            let _ = child.kill();
            return Err(format!("probe {args:?} did not start: {line:?}"));
        }
        let passes = (args.first() == Some(&"latency")).then(|| {
            let p: Passes = Arc::default();
            let q = p.clone();
            std::thread::spawn(move || {
                for l in out.lines().map_while(Result::ok) {
                    if let Some((_, us)) = parse_latency_line(&l) {
                        q.lock().map(|mut v| v.push((Instant::now(), us))).ok();
                    }
                }
            });
            p
        });
        Ok(Probe {
            child,
            stdin,
            passes,
        })
    }

    /// The probe's PID.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Pass times (µs) received in `[from, to)`.
    pub fn passes_between(&self, from: Instant, to: Instant) -> Vec<f64> {
        self.passes
            .as_ref()
            .and_then(|p| p.lock().ok())
            .map(|v| {
                v.iter()
                    .filter(|(t, _)| *t >= from && *t < to)
                    .map(|(_, us)| *us as f64)
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        drop(self.stdin.take());
        // Probes exit on stdin EOF; kill covers a stuck one (bench-only API use, ADR-0003).
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }
}
