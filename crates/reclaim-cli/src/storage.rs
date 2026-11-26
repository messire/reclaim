//! `reclaim-cli storage …`: headless disk analysis, cleanup and duplicates.
//!
//! ```text
//! reclaim-cli storage drives
//! reclaim-cli storage scan <path>... [--fast] [--threads N] [--top N] [--json FILE] [--csv FILE]
//! reclaim-cli storage dupes <path>... [--min-kib N]
//! reclaim-cli storage cleanup [--category ID]... [--run --yes]
//! ```
//!
//! `scan` and `dupes` work on every platform (standard library file system); `drives`
//! and `cleanup` need Windows. Cleanup only examines unless `--run --yes` is given.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use reclaim_core::units::Bytes;
use reclaim_storage_core::ports::Fs;
use reclaim_storage_core::query::{TopKind, extension_stats, largest};
use reclaim_storage_core::tree::{SizeBasis, TOP, Tree};
use reclaim_storage_engine::dupes::{DupOptions, files_from_tree, find};
use reclaim_storage_engine::export::{ExportOptions, csv, json};
use reclaim_storage_engine::scan::{
    ScanControl, ScanOptions, ScanSummary, ScanTarget, scan_blocking,
};

const USAGE: &str = "usage: reclaim-cli storage <drives | scan <path>... [--fast] [--threads N] [--top N] [--json FILE] [--csv FILE] | dupes <path>... [--min-kib N] | cleanup [--category ID]... [--run --yes]>";

type Res = Result<(), String>;

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(String::as_str)
}

#[cfg(any(windows, test))]
fn values<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].as_str())
        .collect()
}

/// Positional arguments (not flags, not flag values).
fn positional(args: &[String]) -> Vec<String> {
    let takes_value = [
        "--threads",
        "--top",
        "--json",
        "--csv",
        "--min-kib",
        "--category",
    ];
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if takes_value.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with("--") {
            out.push(a.clone());
        }
    }
    out
}

fn fs() -> Arc<dyn Fs> {
    #[cfg(windows)]
    {
        Arc::new(reclaim_storage_windows::WinFs)
    }
    #[cfg(not(windows))]
    {
        Arc::new(reclaim_storage_engine::stdfs::StdFs)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Entry point of `storage`.
pub fn run(args: &[String]) -> ExitCode {
    let out = &mut std::io::stdout().lock();
    let rest = args.get(1..).unwrap_or_default();
    let res = match args.first().map(String::as_str) {
        Some("scan") => scan(rest, out),
        Some("dupes") => dupes(rest, out),
        Some("drives") => drives(out),
        Some("cleanup") => cleanup(rest, out),
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

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn index(args: &[String]) -> Result<(Arc<RwLock<Tree>>, ScanSummary, bool), String> {
    let roots = positional(args);
    if roots.is_empty() {
        return Err("give at least one path to scan".into());
    }
    let fs = fs();
    let tree = Arc::new(RwLock::new(Tree::new(fs.style())));
    let control = ScanControl::default();
    if args.iter().any(|a| a == "--fast") {
        #[cfg(windows)]
        {
            let started = Instant::now();
            match reclaim_storage_windows::fast::fast_scan(&roots[0], &tree, &control, &|_| {}) {
                Ok(s) => {
                    let summary = ScanSummary {
                        state: reclaim_storage_engine::scan::ScanState::Complete,
                        progress: reclaim_storage_engine::scan::ScanProgress {
                            files: s.files,
                            dirs: s.dirs,
                            bytes: s.allocated,
                            errors: 0,
                            elapsed_ms: started.elapsed().as_millis() as u64,
                            paused: false,
                        },
                        incomplete_dirs: 0,
                        hardlink_dups: 0,
                        partial: s.orphans > 0,
                    };
                    return Ok((tree, summary, true));
                }
                Err(e) => eprintln!("fast scan not used: {e}; scanning folder by folder"),
            }
        }
        #[cfg(not(windows))]
        eprintln!("fast scan is Windows-only; scanning folder by folder");
    }
    let threads = value(args, "--threads")
        .and_then(|t| t.parse().ok())
        .unwrap_or(4);
    let opts = ScanOptions {
        threads,
        ..ScanOptions::default()
    };
    let s = scan_blocking(&*fs, &tree, ScanTarget::Roots(roots), &opts, &control);
    Ok((tree, s, false))
}

fn scan(args: &[String], out: &mut dyn Write) -> Res {
    let (tree, s, fast) = index(args)?;
    let t = tree.read().unwrap_or_else(|p| p.into_inner());
    let top = t.node(TOP);
    let c = t.counts(TOP);
    writeln!(
        out,
        "{:?}{}{}: {} files, {} folders, {} logical, {} on disk, {} unreadable, {} ms",
        s.state,
        if s.partial { " (partial)" } else { "" },
        if fast { ", fast NTFS scan" } else { "" },
        c.files,
        c.dirs,
        Bytes(top.logical),
        Bytes(top.allocated),
        c.errors,
        s.progress.elapsed_ms
    )
    .map_err(io)?;
    writeln!(out, "index memory: {}", Bytes(t.memory_bytes() as u64)).map_err(io)?;
    let n = value(args, "--top")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    if n > 0 {
        writeln!(out, "\nlargest files:").map_err(io)?;
        for id in largest(&t, TOP, TopKind::Files, SizeBasis::Allocated, n) {
            writeln!(
                out,
                "  {:>12}  {}",
                Bytes(t.node(id).allocated).to_string(),
                t.path(id)
            )
            .map_err(io)?;
        }
        writeln!(out, "\nlargest folders:").map_err(io)?;
        for id in largest(&t, TOP, TopKind::Dirs, SizeBasis::Allocated, n) {
            writeln!(
                out,
                "  {:>12}  {}",
                Bytes(t.node(id).allocated).to_string(),
                t.path(id)
            )
            .map_err(io)?;
        }
        writeln!(out, "\nextensions:").map_err(io)?;
        for e in extension_stats(&t, TOP, SizeBasis::Allocated)
            .iter()
            .take(n)
        {
            writeln!(
                out,
                "  {:>12}  {:>8} files  .{}",
                Bytes(e.allocated).to_string(),
                e.files,
                e.name
            )
            .map_err(io)?;
        }
    }
    let o = ExportOptions::default();
    if let Some(p) = value(args, "--json") {
        let mut w = std::io::BufWriter::new(std::fs::File::create(p).map_err(io)?);
        json(&t, Some(&s), reclaim_core::VERSION, now_ms(), &o, &mut w).map_err(io)?;
        w.flush().map_err(io)?;
        writeln!(out, "\nwrote {p}").map_err(io)?;
    }
    if let Some(p) = value(args, "--csv") {
        let mut w = std::io::BufWriter::new(std::fs::File::create(p).map_err(io)?);
        csv(&t, Some(&s), reclaim_core::VERSION, now_ms(), &o, &mut w).map_err(io)?;
        w.flush().map_err(io)?;
        writeln!(out, "wrote {p}").map_err(io)?;
    }
    Ok(())
}

fn dupes(args: &[String], out: &mut dyn Write) -> Res {
    let (tree, _, _) = index(args)?;
    let min = value(args, "--min-kib")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1024)
        .saturating_mul(1024)
        .max(1);
    let files = files_from_tree(&tree.read().unwrap_or_else(|p| p.into_inner()), TOP, min);
    let started = Instant::now();
    let r = find(
        &*fs(),
        files,
        &DupOptions {
            min_size: min,
            readers: 2,
        },
        &AtomicBool::new(false),
        &|_| {},
    );
    writeln!(
        out,
        "{} groups, {} reclaimable by keeping one copy each; {} candidates, {} read in {} ms, {} unreadable",
        r.groups.len(),
        Bytes(r.savings()),
        r.candidates,
        Bytes(r.bytes_read),
        started.elapsed().as_millis(),
        r.errors.len()
    )
    .map_err(io)?;
    for g in r.groups.iter().take(20) {
        writeln!(
            out,
            "\n{} x {} ({:?})",
            g.files.len(),
            Bytes(g.size),
            g.confidence
        )
        .map_err(io)?;
        for f in &g.files {
            writeln!(out, "  {}", f.path).map_err(io)?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn drives(_out: &mut dyn Write) -> Res {
    Err("drives are listed on Windows only".into())
}

#[cfg(not(windows))]
fn cleanup(_args: &[String], _out: &mut dyn Write) -> Res {
    Err("cleanup runs on Windows only".into())
}

#[cfg(windows)]
fn drives(out: &mut dyn Write) -> Res {
    use reclaim_storage_core::ports::Volumes;
    for v in reclaim_storage_windows::WinVolumes.list() {
        writeln!(
            out,
            "{:<6} {:<10} {:<8} {:?}  {} free of {}  seek penalty: {:?}",
            v.root,
            v.fs,
            v.label,
            v.kind,
            Bytes(v.free),
            Bytes(v.total),
            v.seek_penalty
        )
        .map_err(io)?;
    }
    Ok(())
}

#[cfg(windows)]
fn cleanup(args: &[String], out: &mut dyn Write) -> Res {
    use reclaim_storage_core::cleanup::{Action, Safety, WINDOWS_CATEGORIES};
    use reclaim_storage_core::guard::Guard;
    use reclaim_storage_engine::cleanup::{
        CleanupPlan, DiscoverOptions, PlanEntry, discover, execute,
    };
    use reclaim_storage_windows::{WinRemover, WinVolumes, known_folders};

    let wanted = values(args, "--category");
    let kf = known_folders();
    let guard = Guard::windows(&kf);
    let fs = reclaim_storage_windows::WinFs;
    let no = AtomicBool::new(false);
    let opts = DiscoverOptions {
        now: (now_ms() / 1000) as i64,
        exclude: Vec::new(),
    };
    let mut plan = CleanupPlan::default();
    for cat in WINDOWS_CATEGORIES {
        if !wanted.is_empty() && !wanted.contains(&cat.id) {
            continue;
        }
        let s = discover(cat, &kf, &fs, &WinRemover, &guard, &opts, &no);
        writeln!(
            out,
            "{:<18} {:<8} {:>12}  {:>8} files{}",
            cat.id,
            cat.safety.id(),
            Bytes(s.estimate()).to_string(),
            s.files(),
            if s.incomplete() {
                "  (partly unreadable)"
            } else {
                ""
            }
        )
        .map_err(io)?;
        // Review categories are never cleaned from the command line without a file
        // list; Windows system files are left to Disk Cleanup.
        let selectable = cat.safety != Safety::Review
            && cat.action != Action::WindowsCleanup
            && (!wanted.is_empty() || cat.default_selected);
        if selectable {
            plan.entries.push(PlanEntry {
                category: cat,
                scan: std::sync::Arc::new(s),
                selected: None,
            });
        }
    }
    writeln!(
        out,
        "\nwould free about {} ({} files) from: {}",
        Bytes(plan.estimate()),
        plan.files(),
        plan.entries
            .iter()
            .map(|e| e.category.id)
            .collect::<Vec<_>>()
            .join(", ")
    )
    .map_err(io)?;
    let run = args.iter().any(|a| a == "--run");
    if !run {
        writeln!(out, "examined only; add --run --yes to clean").map_err(io)?;
        return Ok(());
    }
    if !args.iter().any(|a| a == "--yes") {
        return Err("--run deletes cache files permanently; add --yes to confirm".into());
    }
    let r = execute(&plan, &WinRemover, &WinVolumes, &guard, &no, &|_| {});
    for c in &r.categories {
        writeln!(
            out,
            "{:<18} deleted {} ({}), recycled {}, skipped {}, failed {}",
            c.id,
            c.deleted,
            Bytes(c.deleted_bytes),
            c.recycled,
            c.skipped,
            c.failed
        )
        .map_err(io)?;
    }
    writeln!(
        out,
        "freed {} (estimate {}), measured free-space change {} bytes",
        Bytes(r.deleted_bytes()),
        Bytes(r.estimated),
        r.measured()
    )
    .map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_skips_flag_values() {
        let a: Vec<String> = ["C:\\", "--top", "5", "--fast", "D:\\x", "--json", "o.json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(positional(&a), vec!["C:\\", "D:\\x"]);
        assert_eq!(value(&a, "--top"), Some("5"));
        assert_eq!(values(&a, "--json"), vec!["o.json"]);
    }
}
