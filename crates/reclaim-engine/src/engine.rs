//! The engine thread: command queue, sampling cadence (ARCHITECTURE §4.1), run driving
//! and persistence. One long-lived worker; no thread pool, no async runtime.
//!
//! The thread blocks on its command channel with a timeout equal to the next deadline
//! (sample, follow-up). With the window hidden and no run in progress there is no
//! deadline and it blocks until a command arrives, so the idle process does not wake up.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use reclaim_core::analysis::{RunAnalysis, analyse};
use reclaim_core::model::{Capabilities, SystemSnapshot};
use reclaim_core::outcome::{FollowUpPoint, RunMode, RunRecord};
use reclaim_core::plan::{Plan, Selection};
use reclaim_core::ports::Clock;
use reclaim_core::report::{Statement, report};
use reclaim_core::units::Millis;

use crate::coordinator::{Coordinator, Phase, Platform, Row, Step};
use crate::store::{Settings, SettingsWarning, Store};

/// What the UI is showing; controls the sampling cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Window hidden (tray only): no sampling at all.
    Hidden,
    /// Overview visible and foreground: system sample every second.
    Overview,
    /// Processes screen visible: system every second, process table every 3 s.
    Processes,
    /// Window visible but not foreground: system sample every 5 s.
    Background,
}

impl View {
    fn system_interval(self) -> Option<u64> {
        match self {
            View::Hidden => None,
            View::Overview | View::Processes => Some(1_000),
            View::Background => Some(5_000),
        }
    }

    fn table_interval(self) -> Option<u64> {
        match self {
            View::Processes => Some(3_000),
            _ => None,
        }
    }
}

/// Commands from a front end.
#[derive(Debug, Clone)]
pub enum Command {
    /// Change the sampling cadence.
    Subscribe(View),
    /// Sample system and process table now.
    RefreshNow,
    /// Build a plan for confirmation.
    PlanRun(Selection),
    /// Execute a plan.
    StartRun {
        /// Plan id from `PlanReady`.
        plan_id: u64,
        /// Trim or dry run.
        mode: RunMode,
    },
    /// Cancel the plan, the run or its follow-up.
    CancelRun,
    /// Replace settings (saved to disk).
    UpdateSettings(Settings),
    /// Load or unload the optional detailed counters ("Memory details" panel).
    SetDetails(bool),
    /// Delete history and logs.
    ClearHistory,
    /// Stop the engine.
    Shutdown,
}

/// A finished run with its analysis and report.
#[derive(Debug, Clone)]
pub struct Finished {
    /// The record (also stored in history).
    pub record: RunRecord,
    /// Analysis.
    pub analysis: RunAnalysis,
    /// Report statements.
    pub report: Vec<Statement>,
}

impl Finished {
    fn of(record: RunRecord) -> Self {
        let analysis = analyse(&record);
        let pressure = record
            .follow_ups
            .last()
            .map(|f| f.system.pressure)
            .unwrap_or(record.system_after.pressure);
        let report = report(&analysis, pressure);
        Self {
            record,
            analysis,
            report,
        }
    }
}

/// Events to a front end.
#[derive(Debug, Clone)]
pub enum Event {
    /// Platform capabilities (sent at start and after `SetDetails`).
    Capabilities(Capabilities),
    /// Settings in effect (sent at start and after changes).
    Settings(Settings),
    /// A system sample.
    SystemSample(SystemSnapshot),
    /// The process table with eligibility.
    ProcessTable(Vec<Row>),
    /// A plan waiting for confirmation.
    PlanReady(Plan),
    /// `StartRun` was refused (stale plan, failed first sample); no run is active.
    RunRejected(String),
    /// One target processed.
    RunProgress {
        /// Done so far.
        done: usize,
        /// Total targets.
        total: usize,
    },
    /// Trimming finished; follow-up running. Carries the analysis so far.
    RunCompleted(Box<Finished>),
    /// A follow-up sample was taken.
    FollowUpSample(FollowUpPoint),
    /// The run is final (stored in history if enabled).
    RunFinalized(Box<Finished>),
    /// History as stored (sent at start and after changes).
    History(Vec<RunRecord>),
    /// Something the user should know; never fatal.
    Warning(String),
    /// The engine stopped.
    Stopped,
}

/// Wall and monotonic time from `std`.
#[derive(Debug, Clone, Copy)]
pub struct StdClock {
    origin: std::time::Instant,
}

impl Default for StdClock {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Clock for StdClock {
    fn monotonic(&self) -> Millis {
        Millis(self.origin.elapsed().as_millis() as u64)
    }

    fn wall(&self) -> Millis {
        Millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        )
    }
}

/// Handle to a running engine.
#[derive(Debug)]
pub struct EngineHandle {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Sends a command; ignored if the engine has stopped.
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// A sender for other threads (for example the UI).
    pub fn sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    /// Stops the engine and waits for it (cancels a run and stores the partial record).
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the engine thread.
pub fn spawn<P, F>(
    platform: P,
    clock: Box<dyn Clock + Send>,
    store: Option<Store>,
    mut sink: F,
) -> EngineHandle
where
    P: Platform + 'static,
    F: FnMut(Event) + Send + 'static,
{
    let (tx, rx) = channel();
    let thread = std::thread::Builder::new()
        .name("reclaim-engine".into())
        .spawn(move || {
            let mut e = Engine::new(platform, clock, store, &mut sink);
            e.run(&rx);
            sink(Event::Stopped);
        })
        .ok();
    EngineHandle { tx, thread }
}

struct Engine<'a, P: Platform> {
    c: Coordinator<P>,
    store: Option<Store>,
    settings: Settings,
    sink: &'a mut dyn FnMut(Event),
    view: View,
    next_system: Option<Millis>,
    next_table: Option<Millis>,
}

impl<'a, P: Platform> Engine<'a, P> {
    fn new(
        platform: P,
        clock: Box<dyn Clock + Send>,
        store: Option<Store>,
        sink: &'a mut dyn FnMut(Event),
    ) -> Self {
        let (settings, warning) = store.as_ref().map(Store::load_settings).unwrap_or_default();
        let c = Coordinator::new(platform, clock, settings.policy());
        let mut e = Self {
            c,
            store,
            settings,
            sink,
            view: View::Hidden,
            next_system: None,
            next_table: None,
        };
        match warning {
            Some(SettingsWarning::Corrupt(m)) => {
                e.warn(format!(
                    "Settings file was unreadable ({m}); defaults are used."
                ));
            }
            Some(SettingsWarning::Newer(v)) => e.warn(format!(
                "Settings were written by a newer Reclaim (schema {v}); defaults are used."
            )),
            None => {}
        }
        let caps = e.c.platform().capabilities();
        (e.sink)(Event::Capabilities(caps));
        (e.sink)(Event::Settings(e.settings.clone()));
        let history = e.store.as_ref().map(Store::history).unwrap_or_default();
        (e.sink)(Event::History(history));
        e
    }

    fn warn(&mut self, msg: String) {
        if self.settings.log_enabled
            && let Some(s) = &self.store
        {
            let _ = s.log(&format!("{} {msg}", self.c.now().0));
        }
        (self.sink)(Event::Warning(msg));
    }

    fn deadline(&self) -> Option<Millis> {
        if self.c.phase() == Phase::Running {
            return Some(self.c.now());
        }
        [self.next_system, self.next_table, self.c.next_follow_up()]
            .into_iter()
            .flatten()
            .min()
    }

    fn run(&mut self, rx: &Receiver<Command>) {
        loop {
            let cmd = match self.deadline() {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(d) => {
                    let wait = d.since(self.c.now()).0;
                    rx.recv_timeout(Duration::from_millis(wait))
                }
            };
            match cmd {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
                Ok(cmd) => {
                    if catch_unwind(AssertUnwindSafe(|| self.handle(cmd))).is_err() {
                        self.warn(
                            "Internal error while handling a command; the engine continues.".into(),
                        );
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if catch_unwind(AssertUnwindSafe(|| self.due())).is_err() {
                self.warn("Internal error during sampling; the engine continues.".into());
            }
        }
    }

    fn sample_system(&mut self) {
        match self.c.platform().sample() {
            Ok(s) => (self.sink)(Event::SystemSample(s)),
            Err(e) => self.warn(e.to_string()),
        }
    }

    fn sample_table(&mut self) {
        match self.c.table() {
            Ok(rows) => (self.sink)(Event::ProcessTable(rows)),
            Err(e) => self.warn(e.to_string()),
        }
    }

    fn schedule(&mut self) {
        let now = self.c.now();
        self.next_system = self.view.system_interval().map(|i| now.plus(i));
        self.next_table = self.view.table_interval().map(|i| now.plus(i));
    }

    fn due(&mut self) {
        let now = self.c.now();
        if self.next_system.is_some_and(|t| t <= now) {
            self.sample_system();
            self.next_system = self.view.system_interval().map(|i| now.plus(i));
        }
        if self.next_table.is_some_and(|t| t <= now) {
            self.sample_table();
            self.next_table = self.view.table_interval().map(|i| now.plus(i));
        }
        match self.c.step() {
            Step::Idle => {}
            Step::Progress { done, total, .. } => (self.sink)(Event::RunProgress { done, total }),
            Step::Completed(r) => (self.sink)(Event::RunCompleted(Box::new(Finished::of(*r)))),
        }
        let (points, finished) = self.c.tick();
        for p in points {
            (self.sink)(Event::FollowUpSample(p));
        }
        if let Some(r) = finished {
            self.finished(r);
        }
    }

    fn finished(&mut self, r: RunRecord) {
        if self.settings.keep_history
            && let Some(s) = &self.store
            && let Err(e) = s.append_history(&r)
        {
            let msg = format!("Could not save run history: {e}");
            self.warn(msg);
        }
        (self.sink)(Event::RunFinalized(Box::new(Finished::of(r))));
        if let Some(s) = &self.store {
            (self.sink)(Event::History(s.history()));
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Subscribe(v) => {
                let was_hidden = self.view == View::Hidden;
                self.view = v;
                self.schedule();
                if was_hidden && v != View::Hidden {
                    self.sample_system();
                }
                if v == View::Processes {
                    self.sample_table();
                }
            }
            Command::RefreshNow => {
                self.sample_system();
                self.sample_table();
            }
            Command::PlanRun(sel) => match self.c.plan(&sel) {
                Ok(p) => (self.sink)(Event::PlanReady(p)),
                Err(e) => self.warn(e.to_string()),
            },
            Command::StartRun { plan_id, mode } => match self.c.start(plan_id, mode) {
                Ok(Some(previous)) => self.finished(previous),
                Ok(None) => {}
                Err(e) => (self.sink)(Event::RunRejected(e.to_string())),
            },
            Command::CancelRun => {
                if let Some(r) = self.c.cancel() {
                    self.finished(r);
                }
            }
            Command::UpdateSettings(s) => {
                self.c.policy = s.policy();
                self.settings = s;
                if let Some(st) = &self.store
                    && let Err(e) = st.save_settings(&self.settings)
                {
                    let msg = format!("Could not save settings: {e}");
                    self.warn(msg);
                }
                (self.sink)(Event::Settings(self.settings.clone()));
            }
            Command::SetDetails(on) => {
                if self.c.phase() != Phase::Running && self.c.phase() != Phase::FollowUp {
                    self.c.platform().set_details(on);
                }
                let caps = self.c.platform().capabilities();
                (self.sink)(Event::Capabilities(caps));
                if on {
                    match self.c.platform().sample_detailed() {
                        Ok(s) => (self.sink)(Event::SystemSample(s)),
                        Err(e) => self.warn(e.to_string()),
                    }
                }
            }
            Command::ClearHistory => {
                if let Some(s) = &self.store {
                    if let Err(e) = s.clear_history_and_logs() {
                        let msg = format!("Could not clear history: {e}");
                        self.warn(msg);
                    }
                    (self.sink)(Event::History(Vec::new()));
                }
            }
            Command::Shutdown => {}
        }
    }

    /// Quit during a run: cancel, finish the current call, store the partial record.
    fn shutdown(&mut self) {
        if self.c.phase() == Phase::Running {
            self.c.cancel();
            while let Step::Progress { .. } = self.c.step() {}
        }
        if self.c.phase() == Phase::FollowUp {
            let r = self.c.finalize(true);
            self.finished(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_platform_fake::{FakePlatform, process};
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<Event>>>;

    fn start(f: &FakePlatform, store: Option<Store>) -> (EngineHandle, Log) {
        let log: Log = Arc::default();
        let l = log.clone();
        let h = spawn(f.clone(), Box::new(f.clone()), store, move |e| {
            l.lock().unwrap().push(e)
        });
        (h, log)
    }

    fn wait_for(log: &Log, pred: impl Fn(&Event) -> bool) -> Event {
        for _ in 0..500 {
            if let Some(e) = log.lock().unwrap().iter().find(|e| pred(e)) {
                return e.clone();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("event not seen: {:?}", log.lock().unwrap());
    }

    #[test]
    fn plan_run_follow_up_and_history() {
        let dir = std::env::temp_dir().join(format!("reclaim-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300));
        let (h, log) = start(&f, Some(Store::new(&dir)));
        wait_for(&log, |e| matches!(e, Event::Capabilities(_)));
        h.send(Command::PlanRun(Selection::AllEligible));
        let Event::PlanReady(plan) = wait_for(&log, |e| matches!(e, Event::PlanReady(_))) else {
            unreachable!()
        };
        h.send(Command::StartRun {
            plan_id: plan.id,
            mode: RunMode::Trim,
        });
        wait_for(&log, |e| matches!(e, Event::RunCompleted(_)));
        // The fake clock only moves when told: jump past both follow-up points.
        f.advance(61_000);
        h.send(Command::RefreshNow);
        let Event::RunFinalized(fin) = wait_for(&log, |e| matches!(e, Event::RunFinalized(_)))
        else {
            unreachable!()
        };
        assert_eq!(fin.analysis.totals.trimmed, 1);
        assert_eq!(fin.record.follow_ups.len(), 2);
        assert_eq!(fin.report.last().unwrap().key, "report.responsiveness");
        wait_for(&log, |e| matches!(e, Event::History(h) if h.len() == 1));
        h.send(Command::ClearHistory);
        wait_for(&log, |e| matches!(e, Event::History(h) if h.is_empty()));
        h.shutdown();
        wait_for(&log, |e| matches!(e, Event::Stopped));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn views_settings_and_errors() {
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300));
        let (h, log) = start(&f, None);
        h.send(Command::Subscribe(View::Processes));
        wait_for(&log, |e| matches!(e, Event::ProcessTable(_)));
        wait_for(&log, |e| matches!(e, Event::SystemSample(_)));
        h.send(Command::UpdateSettings(Settings {
            min_working_set_mib: 1000,
            ..Settings::default()
        }));
        wait_for(
            &log,
            |e| matches!(e, Event::Settings(s) if s.min_working_set_mib == 1000),
        );
        h.send(Command::StartRun {
            plan_id: 99,
            mode: RunMode::Trim,
        });
        wait_for(
            &log,
            |e| matches!(e, Event::RunRejected(w) if w.contains("no longer current")),
        );
        h.send(Command::SetDetails(true));
        h.send(Command::Subscribe(View::Hidden));
        f.fail_enumerate(true);
        h.send(Command::RefreshNow);
        wait_for(
            &log,
            |e| matches!(e, Event::Warning(w) if w.contains("enumerate")),
        );
        h.send(Command::CancelRun);
        drop(h);
        wait_for(&log, |e| matches!(e, Event::Stopped));
    }

    #[test]
    fn shutdown_during_follow_up_stores_partial_record() {
        let f = FakePlatform::new();
        f.add(process(10, "a.exe", 300));
        let (h, log) = start(&f, None);
        h.send(Command::PlanRun(Selection::AllEligible));
        let Event::PlanReady(plan) = wait_for(&log, |e| matches!(e, Event::PlanReady(_))) else {
            unreachable!()
        };
        h.send(Command::StartRun {
            plan_id: plan.id,
            mode: RunMode::DryRun,
        });
        wait_for(&log, |e| matches!(e, Event::RunCompleted(_)));
        h.shutdown();
        let Event::RunFinalized(fin) = wait_for(&log, |e| matches!(e, Event::RunFinalized(_)))
        else {
            unreachable!()
        };
        assert_eq!(
            fin.record.completion,
            reclaim_core::outcome::Completion::FollowUpTruncated
        );
    }

    #[test]
    fn std_clock_moves_forward() {
        let c = StdClock::default();
        let a = c.monotonic();
        assert!(c.monotonic() >= a);
        assert!(c.wall().0 > 1_600_000_000_000);
        assert_eq!(View::Background.system_interval(), Some(5_000));
        assert_eq!(View::Hidden.system_interval(), None);
    }
}
