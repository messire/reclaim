//! Headless front end over the Reclaim engine (ARCHITECTURE §3).
//!
//! ```text
//! reclaim-cli status
//! reclaim-cli list [--eligible]
//! reclaim-cli plan
//! reclaim-cli trim [--dry-run] [--yes] [--no-follow-up] [--json]
//! ```

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use reclaim_core::analysis::analyse;
use reclaim_core::model::{MetricId, SystemSnapshot};
use reclaim_core::outcome::{ProcessOutcome, RunMode, RunRecord};
use reclaim_core::plan::{Plan, Selection};
use reclaim_core::report::{Tone, report};
use reclaim_engine::{Coordinator, Platform, StdClock, Step, Store};

const USAGE: &str = "usage: reclaim-cli <status | list [--eligible] | plan | trim [--dry-run] [--yes] [--no-follow-up] [--json]>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        run(reclaim_platform_windows::WindowsPlatform::new(), &args)
    }
    #[cfg(not(windows))]
    {
        let _ = &args;
        eprintln!(
            "reclaim-cli {}: only Windows is supported for now",
            reclaim_core::VERSION
        );
        ExitCode::from(2)
    }
}

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

/// Runs a command over any platform (the fake one in tests).
pub fn run<P: Platform>(platform: P, args: &[String]) -> ExitCode {
    let store = Store::new(Store::default_dir());
    let (settings, _) = store.load_settings();
    let mut c = Coordinator::new(platform, Box::new(StdClock::default()), settings.policy());
    let out = &mut std::io::stdout().lock();
    let res = match args.first().map(String::as_str) {
        Some("status") => status(&mut c, out),
        Some("list") => list(&mut c, out, flag(args, "--eligible")),
        Some("plan") => c
            .plan(&Selection::AllEligible)
            .map_err(|e| e.to_string())
            .and_then(|p| print_plan(&p, out)),
        Some("trim") => trim(&mut c, out, args, settings.keep_history.then_some(&store)),
        Some("--version") => {
            writeln!(out, "reclaim-cli {}", reclaim_core::VERSION).map_err(|e| e.to_string())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<(), String>;

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn print_system(s: &SystemSnapshot, out: &mut dyn Write) -> Res {
    let row = |m: MetricId, v: String| format!("{:<24} {v}", m.label());
    let lines = [
        row(MetricId::PhysTotal, s.phys_total.to_string()),
        row(
            MetricId::PhysAvailable,
            format!("{} ({} % in use)", s.phys_available, s.load_percent()),
        ),
        row(
            MetricId::CommitCharge,
            format!("{} of {}", s.commit_charge, s.commit_limit),
        ),
        row(MetricId::Pressure, format!("{:?}", s.pressure)),
    ];
    for l in lines {
        writeln!(out, "{l}").map_err(io)?;
    }
    Ok(())
}

fn status<P: Platform>(c: &mut Coordinator<P>, out: &mut dyn Write) -> Res {
    let s = c.platform().sample().map_err(|e| e.to_string())?;
    print_system(&s, out)
}

fn list<P: Platform>(c: &mut Coordinator<P>, out: &mut dyn Write, eligible_only: bool) -> Res {
    let mut rows = c.table().map_err(|e| e.to_string())?;
    rows.sort_by_key(|(p, _)| std::cmp::Reverse(p.memory.map(|m| m.working_set)));
    writeln!(
        out,
        "{:>7}  {:<28} {:>10} {:>10}  status",
        "PID", "process", "WS", "private"
    )
    .map_err(io)?;
    for (p, e) in rows {
        if eligible_only && !e.is_eligible() {
            continue;
        }
        let (ws, pb) = p.memory.map_or(("-".into(), "-".into()), |m| {
            (m.working_set.to_string(), m.private_bytes.to_string())
        });
        writeln!(
            out,
            "{:>7}  {:<28} {:>10} {:>10}  {}",
            p.key.pid,
            p.image.as_str(),
            ws,
            pb,
            e.key()
        )
        .map_err(io)?;
    }
    Ok(())
}

fn print_plan(p: &Plan, out: &mut dyn Write) -> Res {
    writeln!(
        out,
        "Plan {}: {} processes, {} working set in total (an upper bound, not a promise).",
        p.id,
        p.targets.len(),
        p.targeted_working_set()
    )
    .map_err(io)?;
    for t in &p.targets {
        writeln!(
            out,
            "  {:>7}  {:<28} {:>10}",
            t.key.pid,
            t.image.as_str(),
            t.working_set.to_string()
        )
        .map_err(io)?;
    }
    for (reason, n) in p.skip_counts() {
        writeln!(out, "  skipped: {n} × {reason}").map_err(io)?;
    }
    Ok(())
}

fn trim<P: Platform>(
    c: &mut Coordinator<P>,
    out: &mut dyn Write,
    args: &[String],
    store: Option<&Store>,
) -> Res {
    let mode = if flag(args, "--dry-run") {
        RunMode::DryRun
    } else {
        RunMode::Trim
    };
    let json = flag(args, "--json");
    let plan = c.plan(&Selection::AllEligible).map_err(|e| e.to_string())?;
    if !json {
        print_plan(&plan, out)?;
    }
    if plan.targets.is_empty() {
        return Err("nothing to trim".into());
    }
    if mode == RunMode::Trim && !flag(args, "--yes") {
        write!(out, "Trim these working sets? [y/N] ").map_err(io)?;
        out.flush().map_err(io)?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).map_err(io)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            return Err("cancelled".into());
        }
    }
    c.start(plan.id, mode).map_err(|e| e.to_string())?;
    loop {
        match c.step() {
            Step::Progress {
                done,
                total,
                result,
            } if !json => {
                let what = match &result.outcome {
                    ProcessOutcome::Trimmed { before, after } => {
                        format!("{} → {}", before.working_set, after.working_set)
                    }
                    o => format!("{o:?}"),
                };
                writeln!(out, "  [{done}/{total}] {} {what}", result.image).map_err(io)?;
            }
            Step::Progress { .. } => {}
            Step::Completed(_) => break,
            Step::Idle => return Err("run did not start".into()),
        }
    }
    let record: RunRecord = if flag(args, "--no-follow-up") {
        c.finalize(true)
    } else {
        if !json {
            writeln!(out, "Following up for 60 seconds (Ctrl+C to stop)…").map_err(io)?;
        }
        loop {
            let (_, done) = c.tick();
            if let Some(r) = done {
                break r;
            }
            let wait = c.next_follow_up().map_or(0, |d| d.since(c.now()).0);
            std::thread::sleep(Duration::from_millis(wait.max(50)));
        }
    };
    if let Some(s) = store {
        let _ = s.append_history(&record);
    }
    if json {
        return writeln!(out, "{}", record.to_json()).map_err(io);
    }
    let analysis = analyse(&record);
    let pressure = record
        .follow_ups
        .last()
        .map_or(record.system_after.pressure, |f| f.system.pressure);
    writeln!(out).map_err(io)?;
    for s in report(&analysis, pressure) {
        let mark = match s.tone {
            Tone::Info => " ",
            Tone::Warning => "!",
            Tone::Cost => "$",
            Tone::Note => "i",
        };
        writeln!(out, "{mark} {}", s.render_en()).map_err(io)?;
    }
    Ok(())
}
