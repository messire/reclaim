//! View-model behaviour against the real engine over the fake platform (PLAN M4.1).

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use reclaim_app::vm::{AppState, Column, Reaction, Screen, SettingsForm, Stage, TrayState};
use reclaim_core::model::{AccessHint, Pressure, SystemSnapshot};
use reclaim_core::outcome::RunMode;
use reclaim_core::units::Bytes;
use reclaim_engine::{Command, EngineHandle, Event, Settings, View, spawn};
use reclaim_platform_fake::{FakePlatform, process};

struct Rig {
    f: FakePlatform,
    h: EngineHandle,
    rx: Receiver<Event>,
    s: AppState,
}

impl Rig {
    fn new(confirm: bool) -> Rig {
        let f = FakePlatform::new();
        f.add(process(10, "big.exe", 400));
        f.add(process(11, "mid.exe", 200));
        f.add(process(12, "tiny.exe", 4));
        f.add(process(13, "admin.exe", 300).access(AccessHint::RequiresElevation));
        let (tx, rx) = channel();
        let h = spawn(f.clone(), Box::new(f.clone()), None, move |e| {
            let _ = tx.send(e);
        });
        let mut r = Rig {
            f,
            h,
            rx,
            s: AppState::new(0),
        };
        r.until(|e| matches!(e, Event::Settings(_)));
        if !confirm {
            let react = r.s.save_settings(&SettingsForm {
                confirm_runs: false,
                ..r.s.settings_form()
            });
            r.send(react.unwrap());
            r.until(|e| matches!(e, Event::Settings(s) if !s.confirm_runs));
        }
        r
    }

    fn send(&self, r: Reaction) {
        for c in r.commands {
            self.h.send(c);
        }
    }

    /// Applies events until one matches, sending the commands they produce.
    fn until(&mut self, pred: impl Fn(&Event) -> bool) {
        loop {
            let e = self
                .rx
                .recv_timeout(Duration::from_secs(5))
                .expect("event in time");
            let hit = pred(&e);
            let r = self.s.apply(e);
            self.send(r);
            if hit {
                return;
            }
        }
    }
}

#[test]
fn processes_screen_sorts_filters_and_selects() {
    let mut r = Rig::new(true);
    let v = r.s.set_visible(true);
    assert!(v.dirty.any());
    r.send(v);
    let show = r.s.show(Screen::Processes);
    assert!(
        show.commands
            .iter()
            .any(|c| matches!(c, Command::Subscribe(View::Processes)))
    );
    r.send(show);
    r.until(|e| matches!(e, Event::ProcessTable(_)));

    let rows = r.s.processes();
    assert_eq!(rows[0].name, "big.exe", "largest working set first");
    assert!(rows.iter().any(|p| p.status == "Below size threshold"));
    assert!(
        rows.iter()
            .any(|p| p.status == "Needs administrator rights")
    );

    r.s.sort_by(Column::Name, true);
    let names: Vec<_> = r.s.processes().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["admin.exe", "big.exe", "mid.exe", "tiny.exe"]);

    r.s.set_eligible_only(true);
    assert_eq!(r.s.processes().len(), 2);

    let tiny = r.s.key_of("tiny.exe").unwrap();
    r.s.toggle_selected(tiny);
    assert_eq!(
        r.s.selected_count(),
        0,
        "ineligible rows cannot be selected"
    );
    let mid = r.s.key_of("mid.exe").unwrap();
    r.s.toggle_selected(mid);
    assert_eq!(r.s.selected_count(), 1);

    // Trim only the selection.
    let req = r.s.request_run(RunMode::Trim, true);
    r.send(req);
    r.until(|e| matches!(e, Event::PlanReady(_)));
    let plan = r.s.plan().expect("dialog shown");
    assert!(plan.title.contains("1 processes"), "{}", plan.title);
    assert!(plan.caveat.contains("does not free committed memory"));
    let go = r.s.confirm();
    assert_eq!(
        r.s.screen,
        Screen::Processes,
        "a run does not switch screens"
    );
    assert!(r.s.overview().status.starts_with("Trimming 0 of 1"));
    assert!(r.s.overview().can_cancel);
    r.send(go);
    r.until(|e| matches!(e, Event::RunCompleted(_)));
    assert!(
        r.s.overview().status.starts_with("1 trimmed · "),
        "{}",
        r.s.overview().status
    );
    assert!(
        !r.s.overview().can_cancel,
        "the follow-up runs in the background"
    );
    assert!(r.s.result().can_cancel);
    assert!(r.s.result().status.contains("follow-up"));
    let res = r.s.result();
    assert_eq!(res.processes.len(), 1);
    assert!(res.processes[0].1.starts_with("Trimmed:"));

    // Hiding stops sampling; cancelling the follow-up finalizes as truncated.
    let hide = r.s.set_visible(false);
    assert!(
        hide.commands
            .iter()
            .any(|c| matches!(c, Command::Subscribe(View::Hidden)))
    );
    r.send(hide);
    let c = r.s.cancel_run();
    r.send(c);
    r.until(|e| matches!(e, Event::RunFinalized(_)));
    assert!(matches!(r.s.stage(), Stage::Final(_)));
    assert_eq!(
        r.s.result().status,
        "Follow-up cut short by a newer run or by closing Reclaim."
    );
    assert!(!r.s.result().can_cancel);
    assert!(r.s.overview().status.starts_with("1 trimmed · "));
    r.s.apply(Event::History(Vec::new()));
    assert_eq!(
        r.s.overview().status,
        "No run yet.",
        "clearing history clears the summary"
    );
    r.h.shutdown();
}

#[test]
fn unconfirmed_dry_run_completes_and_reaches_history() {
    let mut r = Rig::new(false);
    let v = r.s.set_visible(true);
    r.send(v);
    let req = r.s.request_run(RunMode::DryRun, false);
    assert!(!r.s.overview().can_run, "busy while planning");
    assert!(r.s.request_run(RunMode::Trim, false).commands.is_empty());
    r.send(req);
    r.until(|e| matches!(e, Event::RunCompleted(_)));
    assert_eq!(r.s.tray_state(), TrayState::Working, "follow-up pending");
    assert!(r.s.plan().is_none(), "no dialog when confirmation is off");
    assert_eq!(r.s.screen, Screen::Overview);
    r.f.advance(61_000);
    r.h.send(Command::RefreshNow);
    r.until(|e| matches!(e, Event::RunFinalized(_)));
    assert_ne!(r.s.tray_state(), TrayState::Working);
    assert_eq!(
        r.s.overview().status,
        "No run yet.",
        "dry runs are not summarised"
    );
    let res = r.s.result();
    assert_eq!(res.status, "Follow-up complete.");
    assert!(res.lines[0].1.starts_with("Dry run"));
    assert!(
        res.lines
            .iter()
            .any(|(_, l)| l.contains("does not release committed memory"))
    );
    r.h.shutdown();
}

#[test]
fn dismissing_and_empty_plans() {
    let mut r = Rig::new(true);
    let form = SettingsForm {
        min_working_set_mib: "100000".into(),
        ..r.s.settings_form()
    };
    assert!(r.s.save_settings(&form).is_err(), "threshold validated");
    let form = SettingsForm {
        min_working_set_mib: "60000".into(),
        exclusions: "a.exe\n\n A.EXE \nb.exe".into(),
        ..r.s.settings_form()
    };
    let saved = r.s.save_settings(&form).unwrap();
    r.send(saved);
    r.until(|e| matches!(e, Event::Settings(s) if s.min_working_set_mib == 60000));
    assert_eq!(r.s.settings().exclusions, ["a.exe", "b.exe"]);
    assert_eq!(r.s.settings_form().exclusions, "a.exe\nb.exe");

    let req = r.s.request_run(RunMode::Trim, false);
    r.send(req);
    r.until(|e| matches!(e, Event::PlanReady(_)));
    let p = r.s.plan().unwrap();
    assert!(!p.can_start);
    assert_eq!(p.title, "No process is eligible for trimming right now.");
    assert!(p.skipped.iter().any(|l| l.contains("Below size threshold")));
    let d = r.s.dismiss_plan();
    assert!(matches!(d.commands[..], [Command::CancelRun]));
    r.send(d);
    assert!(r.s.plan().is_none());
    assert!(r.s.overview().can_run);
    assert!(r.s.confirm().commands.is_empty(), "nothing to confirm");
    r.h.shutdown();
}

#[test]
fn overview_and_tray_follow_samples() {
    let mut s = AppState::new(180);
    assert_eq!(s.overview().used, "not available");
    assert_eq!(s.tray_tooltip(), "Reclaim");
    assert_eq!(s.tray_state(), TrayState::Unavailable, "no sample yet");
    let snap = SystemSnapshot {
        phys_total: Bytes::from_mib(8192),
        phys_available: Bytes::from_mib(2048),
        commit_charge: Bytes::from_mib(6000),
        commit_limit: Bytes::from_mib(12000),
        pressure: Pressure::Normal,
        ..SystemSnapshot::default()
    };
    let r = s.apply(Event::SystemSample(snap));
    assert!(r.dirty.overview && r.dirty.tray);
    let o = s.overview();
    assert_eq!(o.used, "6.0 GiB");
    assert_eq!(o.load, "75 % in use");
    assert_eq!(o.committed, "5.9 GiB of 11.7 GiB");
    assert_eq!(o.pressure, "normal");
    assert_eq!(o.standby, "not available");
    assert_eq!(s.tray_tooltip(), "Reclaim: 2.0 GiB available");
    assert_eq!((o.used_permille, o.commit_permille), (750, 500));
    assert!(!o.pressure_high);
    assert_eq!(s.tray_state(), TrayState::Normal);
    let high = SystemSnapshot {
        pressure: Pressure::High,
        ..snap
    };
    s.apply(Event::SystemSample(high));
    assert!(s.overview().pressure_high);
    assert_eq!(s.tray_state(), TrayState::High);
    s.apply(Event::SystemSample(snap));
    assert!(
        !s.apply(Event::SystemSample(snap)).dirty.any(),
        "unchanged sample redraws nothing"
    );
    assert_eq!(s.overview().status, "No run yet.");
    assert!(s.history().is_empty());

    // A refused start returns the UI to a state where the user can try again.
    s.request_run(RunMode::Trim, false);
    assert!(!s.overview().can_run);
    let r = s.apply(Event::RunRejected("stale plan".into()));
    assert!(r.dirty.warning);
    assert!(s.overview().can_run);
    assert!(matches!(s.stage(), Stage::Idle));
    s.dismiss_warning();
    assert!(!s.eligible_only());

    let w = s.apply(Event::Warning("x".into()));
    assert!(w.dirty.warning);
    assert_eq!(s.warning(), Some("x"));
    s.dismiss_warning();
    assert_eq!(s.warning(), None);

    let d = s.set_details(true);
    assert!(matches!(d.commands[0], Command::SetDetails(true)));
    assert!(s.overview().details);
    assert!(matches!(
        s.clear_history().commands[..],
        [Command::ClearHistory]
    ));
    assert_eq!(
        Screen::from_index(Screen::Settings.index()),
        Screen::Settings
    );
    assert_eq!(
        Screen::from_index(3),
        Screen::About,
        "About is the last screen"
    );
    assert_eq!(Screen::from_index(99), Screen::Overview);
    let _ = Settings::default();
}
