//! Cleanup discovery (read-only) and execution of a reviewed plan.
//!
//! Workflow (STORAGE-ARCHITECTURE §7): discover → classify → review → estimate →
//! confirm → revalidate → execute → report → measure → refresh. This module does the
//! discovery, the plan, the execution with revalidation (through the [`Remover`]
//! port), the result report and the free-space measurement. Review and confirmation
//! are the UI's and CLI's job; nothing here deletes without a plan.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use reclaim_storage_core::cleanup::{
    Action, Candidate, Category, CategoryResult, CategoryScan, CleanupReport, FoundRoot,
    ItemOutcome, Location, SkipReason, VolumeDelta,
};
use reclaim_storage_core::entry::{EntryMeta, ErrorKind, Kind};
use reclaim_storage_core::guard::{Guard, KnownFolders};
use reclaim_storage_core::path::{PathStyle, glob_match};
use reclaim_storage_core::ports::{Fs, Remover, Target, Volumes};

use crate::scan::Exclusions;

/// Discovery settings.
#[derive(Debug, Clone, Default)]
pub struct DiscoverOptions {
    /// Current time, Unix seconds (age guards).
    pub now: i64,
    /// User exclusions (paths or name globs) never offered for cleanup.
    pub exclude: Vec<String>,
}

fn split_sub(sub: &str) -> Vec<&str> {
    sub.split('\\').filter(|c| !c.is_empty()).collect()
}

/// Resolves a location to existing directories (wildcard components matched against
/// directory names; links are never entered).
pub fn resolve(loc: &Location, kf: &KnownFolders, fs: &dyn Fs) -> Vec<String> {
    let Some(base) = kf.get(loc.base) else {
        return Vec::new();
    };
    let ci = fs.style() == PathStyle::Windows;
    let mut cur = vec![base.trim_end_matches(['\\', '/']).to_owned()];
    if fs.style() == PathStyle::Windows && cur[0].len() == 2 {
        cur[0].push('\\'); // "C:" → "C:\"
    }
    for comp in split_sub(loc.sub) {
        let mut next = Vec::new();
        for dir in &cur {
            if comp.contains('*') || comp.contains('?') {
                let mut list = Vec::new();
                if fs.read_dir(dir, &mut list).is_ok() {
                    list.sort_by(|a, b| a.name.cmp(&b.name));
                    for e in list {
                        if e.kind == Kind::Dir && glob_match(comp, &e.name, ci) {
                            next.push(fs.join(dir, &e.name));
                        }
                    }
                }
            } else {
                next.push(fs.join(dir, comp));
            }
        }
        cur = next;
    }
    cur.retain(|p| fs.stat(p).is_ok_and(|m| m.kind == Kind::Dir));
    cur
}

fn qualifies(loc: &Location, e: &EntryMeta, now: i64, ci: bool) -> Result<bool, ()> {
    if !loc.include.is_empty() && !loc.include.iter().any(|g| glob_match(g, &e.name, ci)) {
        return Ok(false);
    }
    if e.logical < loc.min_size {
        return Ok(false);
    }
    if loc.min_age_hours > 0 && e.mtime > now - i64::from(loc.min_age_hours) * 3600 {
        return Err(()); // too new
    }
    Ok(true)
}

/// Finds the candidates of one category. Read-only; respects cancellation.
pub fn discover(
    cat: &Category,
    kf: &KnownFolders,
    fs: &dyn Fs,
    remover: &dyn Remover,
    guard: &Guard,
    opts: &DiscoverOptions,
    cancel: &AtomicBool,
) -> CategoryScan {
    let mut scan = CategoryScan {
        id: cat.id.to_owned(),
        ..CategoryScan::default()
    };
    if cat.action == Action::EmptyRecycleBin {
        scan.bin = remover.recycle_bin();
        return scan;
    }
    let style = fs.style();
    let ci = style == PathStyle::Windows;
    let excl = Exclusions::new(&opts.exclude, style);
    let mut seen_roots = HashSet::new();
    for loc in cat.locations {
        for root in resolve(loc, kf, fs) {
            if cancel.load(Ordering::Relaxed) {
                return scan;
            }
            let key = if ci {
                root.to_lowercase()
            } else {
                root.clone()
            };
            if !seen_roots.insert(key) {
                continue;
            }
            if guard.check_root(&root).is_err() && cat.action != Action::WindowsCleanup {
                continue; // a rule pointing into a protected tree is never used
            }
            let idx = scan.roots.len();
            scan.roots.push(FoundRoot {
                path: root.clone(),
                error: None,
            });
            let mut stack = vec![root.clone()];
            let mut buf = Vec::new();
            while let Some(dir) = stack.pop() {
                if cancel.load(Ordering::Relaxed) {
                    return scan;
                }
                buf.clear();
                if let Err(k) = fs.read_dir(&dir, &mut buf) {
                    scan.unreadable += 1;
                    if dir == root {
                        scan.roots[idx].error = Some(k);
                    }
                    continue;
                }
                for e in buf.drain(..) {
                    let path = fs.join(&dir, &e.name);
                    if !excl.is_empty() && excl.matches(&e.name, || path.clone(), style) {
                        continue;
                    }
                    match e.kind {
                        Kind::Link => scan.skipped_links += 1,
                        Kind::Dir if loc.recurse => stack.push(path),
                        Kind::File => match qualifies(loc, &e, opts.now, ci) {
                            Ok(true) => scan.candidates.push(Candidate {
                                path,
                                logical: e.logical,
                                allocated: e.allocated.unwrap_or(e.logical),
                                file_id: e.file_id,
                                mtime: e.mtime,
                                hardlinked: e.nlink.is_some_and(|n| n > 1),
                                root: idx,
                            }),
                            Ok(false) => {}
                            Err(()) => scan.too_new += 1,
                        },
                        _ => {}
                    }
                }
            }
        }
    }
    scan
}

/// What the user selected for one category.
#[derive(Debug, Clone)]
pub struct PlanEntry {
    /// The category.
    pub category: &'static Category,
    /// Discovery result.
    pub scan: CategoryScan,
    /// Indices of the selected candidates (`None`: all of them).
    pub selected: Option<Vec<usize>>,
}

impl PlanEntry {
    fn chosen(&self) -> Vec<&Candidate> {
        match &self.selected {
            None => self.scan.candidates.iter().collect(),
            Some(ix) => {
                let set: BTreeSet<usize> = ix.iter().copied().collect();
                set.into_iter()
                    .filter_map(|i| self.scan.candidates.get(i))
                    .collect()
            }
        }
    }
}

/// A reviewed set of categories to clean.
#[derive(Debug, Clone, Default)]
pub struct CleanupPlan {
    /// Entries in execution order.
    pub entries: Vec<PlanEntry>,
}

impl CleanupPlan {
    /// Files the plan touches (Recycle Bin counted by items), after removing paths that
    /// appear in more than one category.
    pub fn files(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| match e.category.action {
                Action::EmptyRecycleBin => e.scan.files(),
                Action::WindowsCleanup => 0,
                _ => e.chosen().len() as u64,
            })
            .sum()
    }

    /// Estimated bytes freed on disk: deleted files' allocated sizes (hard-linked files
    /// excluded) and the Recycle Bin. Recycled files free nothing until the bin is
    /// emptied, so they are not part of this estimate.
    pub fn estimate(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| match e.category.action {
                Action::Delete => e.chosen().iter().map(|c| c.reclaimable()).sum(),
                Action::EmptyRecycleBin => e.scan.estimate(),
                Action::Recycle | Action::WindowsCleanup => 0,
            })
            .sum()
    }

    /// Bytes that would be moved to the Recycle Bin.
    pub fn to_recycle(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.category.action == Action::Recycle)
            .map(|e| e.chosen().iter().map(|c| c.allocated).sum::<u64>())
            .sum()
    }

    /// Whether anything is deleted permanently (the confirmation must say so).
    pub fn deletes_permanently(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(e.category.action, Action::Delete | Action::EmptyRecycleBin)
                && (e.scan.files() > 0 || e.scan.estimate() > 0)
        })
    }
}

/// Progress of an execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CleanupProgress {
    /// Targets processed.
    pub done: u64,
    /// Targets in the plan.
    pub total: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn parent_of(path: &str) -> Option<&str> {
    let i = path.rfind(['\\', '/'])?;
    Some(&path[..i])
}

/// Executes a plan. Every target is revalidated by the remover right before it acts.
/// Cancellation stops before the next file; what is not attempted is reported as
/// skipped (cancelled).
pub fn execute(
    plan: &CleanupPlan,
    remover: &dyn Remover,
    volumes: &dyn Volumes,
    guard: &Guard,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(CleanupProgress),
) -> CleanupReport {
    let mut report = CleanupReport {
        started_ms: now_ms(),
        estimated: plan.estimate(),
        ..CleanupReport::default()
    };
    // Free space before, per volume touched.
    let mut vols: Vec<(String, u64)> = Vec::new();
    let mut probe = |p: &str| {
        if let Some(v) = volumes.volume_of(p)
            && !vols.iter().any(|(r, _)| *r == v.root)
        {
            vols.push((v.root, v.free));
        }
    };
    for e in &plan.entries {
        for r in &e.scan.roots {
            probe(&r.path);
        }
        if e.category.action == Action::EmptyRecycleBin {
            for v in volumes.list() {
                probe(&v.root);
            }
        }
    }

    let total = plan.files();
    let mut done = 0u64;
    let mut seen = HashSet::new();
    for e in &plan.entries {
        let mut res = CategoryResult {
            id: e.category.id.to_owned(),
            ..CategoryResult::default()
        };
        match e.category.action {
            Action::Delete | Action::Recycle => {
                let mut dirs: BTreeSet<(usize, String, usize)> = BTreeSet::new();
                for c in e.chosen() {
                    if !seen.insert(c.path.clone()) {
                        continue; // listed by an earlier category too
                    }
                    let outcome = if cancel.load(Ordering::Relaxed) {
                        report.cancelled = true;
                        ItemOutcome::Skipped(SkipReason::Cancelled)
                    } else {
                        let root = e.scan.roots.get(c.root).map(|r| r.path.clone());
                        let t = Target {
                            path: c.path.clone(),
                            root,
                            file_id: c.file_id,
                            allocated: c.allocated,
                            is_dir: false,
                        };
                        if e.category.action == Action::Delete {
                            remover.delete(&t, guard)
                        } else {
                            remover.recycle(&t, guard)
                        }
                    };
                    if matches!(outcome, ItemOutcome::Deleted(_))
                        && let Some(p) = parent_of(&c.path)
                    {
                        let depth = p.matches(['\\', '/']).count();
                        dirs.insert((usize::MAX - depth, p.to_owned(), c.root));
                    }
                    res.add(&c.path, &outcome);
                    done += 1;
                    on_progress(CleanupProgress { done, total });
                }
                // Remove directories emptied by the cleanup, deepest first, never the
                // location root itself.
                let mut pending: Vec<(String, usize)> =
                    dirs.into_iter().map(|(_, p, r)| (p, r)).collect();
                let mut tried = HashSet::new();
                while let Some((dir, r)) = pending.first().cloned() {
                    pending.remove(0);
                    if !tried.insert(dir.clone()) {
                        continue;
                    }
                    let Some(root) = e.scan.roots.get(r).map(|x| x.path.as_str()) else {
                        continue;
                    };
                    if remover.remove_empty_dir(&dir, root, guard) {
                        res.dirs_removed += 1;
                        if let Some(p) = parent_of(&dir) {
                            pending.push((p.to_owned(), r));
                        }
                    }
                }
            }
            Action::EmptyRecycleBin => {
                if cancel.load(Ordering::Relaxed) {
                    report.cancelled = true;
                } else {
                    let before = remover.recycle_bin().unwrap_or((0, 0));
                    match remover.empty_recycle_bin() {
                        Ok(()) => report.bin_emptied = Some(before),
                        Err(k) => res.add("Recycle Bin", &ItemOutcome::Failed(k)),
                    }
                }
            }
            Action::WindowsCleanup => match remover.open_system_cleanup() {
                Ok(()) => report.windows_cleanup_opened = true,
                Err(k) => res.add("Disk Cleanup", &ItemOutcome::Failed(k)),
            },
        }
        report.categories.push(res);
    }

    for (root, before) in vols {
        if let Some(v) = volumes.volume_of(&root) {
            report.volumes.push(VolumeDelta {
                volume: root,
                free_before: before,
                free_after: v.free,
            });
        }
    }
    report.finished_ms = now_ms();
    report
}

/// Maps an I/O error raised while deleting.
pub fn delete_error(e: &std::io::Error) -> ItemOutcome {
    match ErrorKind::from_io(e) {
        ErrorKind::NotFound => ItemOutcome::Skipped(SkipReason::Missing),
        ErrorKind::InUse => ItemOutcome::Skipped(SkipReason::InUse),
        k => ItemOutcome::Failed(k),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFs;
    use reclaim_storage_core::cleanup::{Regenerates, Safety, category};
    use reclaim_storage_core::guard::Folder;

    fn kf() -> KnownFolders {
        KnownFolders::new([
            (Folder::Profile, "/home/me".to_owned()),
            (Folder::LocalAppData, "/home/me/.cache".to_owned()),
            (Folder::Temp, "/tmp/me".to_owned()),
            (Folder::Downloads, "/home/me/Downloads".to_owned()),
            (Folder::Documents, "/home/me/Documents".to_owned()),
        ])
    }

    fn fs() -> FakeFs {
        let mut f = FakeFs::new();
        f.add_file("/tmp/me", "old.tmp", 5000);
        f.add_file("/tmp/me", "new.tmp", 7000);
        f.add_file("/tmp/me/sub/deeper", "x.log", 100);
        f.link("/tmp/me/ln");
        f.add_file(
            "/home/me/.cache/Google/Chrome/User Data/Default/Cache",
            "data_1",
            9000,
        );
        f.add_file(
            "/home/me/.cache/Google/Chrome/User Data/Profile 2/Cache",
            "data_2",
            1000,
        );
        f.add_file(
            "/home/me/.cache/Google/Chrome/User Data/Default",
            "Cookies",
            10,
        );
        f.add_file("/home/me/Downloads", "setup.exe", 50_000);
        f.add_file("/home/me/Downloads", "notes.txt", 50_000);
        f.add_file("/home/me/Documents", "thesis.docx", 1_000);
        let now = 10_000_000;
        for p in [
            "/tmp/me/old.tmp",
            "/tmp/me/sub/deeper/x.log",
            "/home/me/Downloads/setup.exe",
            "/home/me/Downloads/notes.txt",
        ] {
            f.set_mtime(p, now - 400 * 24 * 3600);
        }
        f.set_mtime("/tmp/me/new.tmp", now - 60);
        f
    }

    fn opts() -> DiscoverOptions {
        DiscoverOptions {
            now: 10_000_000,
            exclude: Vec::new(),
        }
    }

    fn guard() -> Guard {
        Guard::posix(&kf())
    }

    #[test]
    fn discovery_is_rule_bound_and_read_only() {
        let f = fs();
        let no = AtomicBool::new(false);
        let temp = discover(
            category("user-temp").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &opts(),
            &no,
        );
        let names: Vec<_> = temp.candidates.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names, ["/tmp/me/old.tmp", "/tmp/me/sub/deeper/x.log"]);
        assert_eq!(temp.too_new, 1);
        assert_eq!(temp.skipped_links, 1);
        assert_eq!(temp.estimate(), 8192 + 4096);

        let br = discover(
            category("browser-cache").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &opts(),
            &no,
        );
        let mut names: Vec<_> = br.candidates.iter().map(|c| c.path.clone()).collect();
        names.sort();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| n.contains("/Cache/")));
        assert_eq!(br.roots.len(), 2);

        let inst = discover(
            category("old-installers").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &opts(),
            &no,
        );
        assert_eq!(inst.candidates.len(), 1);
        assert!(inst.candidates[0].path.ends_with("setup.exe"));
        // Nothing was removed by discovery.
        assert!(f.exists("/tmp/me/old.tmp"));

        f.set_bin(3, 999);
        let bin = discover(
            category("recycle-bin").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &opts(),
            &no,
        );
        assert_eq!(bin.estimate(), 999);

        let yes = AtomicBool::new(true);
        let c = discover(
            category("user-temp").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &opts(),
            &yes,
        );
        assert!(c.candidates.is_empty());
    }

    #[test]
    fn exclusions_are_honoured() {
        let f = fs();
        let o = DiscoverOptions {
            exclude: vec!["/tmp/me/sub".into(), "*.tmp".into()],
            ..opts()
        };
        let no = AtomicBool::new(false);
        let s = discover(
            category("user-temp").unwrap(),
            &kf(),
            &f,
            &f,
            &guard(),
            &o,
            &no,
        );
        assert!(s.candidates.is_empty(), "{:?}", s.candidates);
    }

    fn plan_for(f: &FakeFs, ids: &[&str]) -> CleanupPlan {
        let no = AtomicBool::new(false);
        CleanupPlan {
            entries: ids
                .iter()
                .map(|id| {
                    let c = category(id).unwrap();
                    PlanEntry {
                        category: c,
                        scan: discover(c, &kf(), f, f, &guard(), &opts(), &no),
                        selected: None,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn execution_deletes_revalidates_and_reports() {
        let f = fs();
        let plan = plan_for(&f, &["user-temp", "browser-cache"]);
        assert_eq!(plan.files(), 4);
        assert!(plan.deletes_permanently());
        let est = plan.estimate();
        // A file is replaced between review and execution, another is locked.
        let mut f = f;
        f.remove("/tmp/me/old.tmp");
        f.add_file("/tmp/me", "old.tmp", 5000); // same name, new identity
        f.lock_file("/home/me/.cache/Google/Chrome/User Data/Profile 2/Cache/data_2");
        let free_before = f.list()[0].free;
        let no = AtomicBool::new(false);
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        assert_eq!(r.estimated, est);
        let temp = &r.categories[0];
        assert_eq!(temp.deleted, 1); // x.log
        assert_eq!(temp.skipped, 1); // old.tmp changed
        assert!(temp.notes[0].contains("Changed"));
        assert_eq!(temp.dirs_removed, 2); // deeper and sub, not the root
        assert!(f.exists("/tmp/me"));
        assert!(!f.exists("/tmp/me/sub"));
        assert!(f.exists("/tmp/me/old.tmp"));
        let br = &r.categories[1];
        assert_eq!((br.deleted, br.skipped), (1, 1));
        assert!(f.exists("/home/me/.cache/Google/Chrome/User Data/Default/Cookies"));
        assert_eq!(r.volumes.len(), 1);
        assert_eq!(r.volumes[0].free_before, free_before);
        assert_eq!(r.measured(), (4096 + 12288) as i128);
        assert!(!r.cancelled);
    }

    #[test]
    fn recycle_bin_and_review_categories() {
        let f = fs();
        f.set_bin(2, 1000);
        let mut plan = plan_for(&f, &["old-installers", "recycle-bin"]);
        // The recycle category goes first: the plan order is execution order, so the
        // bin is emptied after the installer is recycled. Reorder like the UI does.
        plan.entries.reverse();
        assert_eq!(plan.estimate(), 1000);
        assert_eq!(plan.to_recycle(), 53248);
        let no = AtomicBool::new(false);
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        assert_eq!(r.bin_emptied, Some((2, 1000)));
        assert_eq!(r.categories[1].recycled, 1);
        assert!(!f.exists("/home/me/Downloads/setup.exe"));
        assert!(f.exists("/home/me/Downloads/notes.txt"));
        assert_eq!(f.recycle_bin(), Some((1, 53248)));
        assert_eq!(r.recycled_bytes(), 53248);
    }

    #[test]
    fn partial_selection_cancel_and_duplicates() {
        let f = fs();
        let mut plan = plan_for(&f, &["user-temp", "user-temp"]);
        plan.entries[0].selected = Some(vec![1, 1]);
        assert_eq!(plan.entries[0].chosen().len(), 1);
        let no = AtomicBool::new(false);
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        // The second entry lists the same files: x.log is processed once.
        assert_eq!(r.categories[0].deleted, 1);
        assert_eq!(r.categories[1].deleted, 1); // old.tmp
        assert_eq!(r.categories[1].skipped, 0);

        let f = fs();
        let plan = plan_for(&f, &["user-temp"]);
        let yes = AtomicBool::new(true);
        let r = execute(&plan, &f, &f, &guard(), &yes, &|_| {});
        assert!(r.cancelled);
        assert_eq!(r.categories[0].skipped, 2);
        assert!(f.exists("/tmp/me/old.tmp"));
    }

    #[test]
    fn guard_blocks_misdirected_targets() {
        // A forged plan pointing outside its root is refused at execution.
        let f = fs();
        let bad = CategoryScan {
            id: "user-temp".into(),
            roots: vec![FoundRoot {
                path: "/tmp/me".into(),
                error: None,
            }],
            candidates: vec![Candidate {
                path: "/home/me/Documents/thesis.docx".into(),
                logical: 1000,
                allocated: 4096,
                file_id: None,
                mtime: 0,
                hardlinked: false,
                root: 0,
            }],
            ..CategoryScan::default()
        };
        let plan = CleanupPlan {
            entries: vec![PlanEntry {
                category: category("user-temp").unwrap(),
                scan: bad,
                selected: None,
            }],
        };
        let no = AtomicBool::new(false);
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        assert_eq!(r.categories[0].skipped, 1);
        assert!(f.exists("/home/me/Documents/thesis.docx"));
        // A link replacing a parent directory is refused too.
        let mut f = fs();
        let plan = plan_for(&f, &["user-temp"]);
        f.remove("/tmp/me/sub");
        f.link("/tmp/me/sub");
        f.add_file("/tmp/me/sub/deeper", "x.log", 100); // "through" the link
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        assert!(
            r.categories[0].notes.iter().any(|n| n.contains("Link")),
            "{:?}",
            r.categories[0].notes
        );
        assert!(f.exists("/tmp/me/sub/deeper/x.log"));
    }

    #[test]
    fn windows_cleanup_is_delegated() {
        let f = fs();
        let plan = plan_for(&f, &["windows-system"]);
        assert_eq!(plan.estimate(), 0);
        let no = AtomicBool::new(false);
        let r = execute(&plan, &f, &f, &guard(), &no, &|_| {});
        assert!(!r.windows_cleanup_opened);
        assert_eq!(r.categories[0].failed, 1);
        let _ = (Safety::Safe, Regenerates::Yes);
        assert_eq!(
            delete_error(&std::io::Error::from(std::io::ErrorKind::NotFound)),
            ItemOutcome::Skipped(SkipReason::Missing)
        );
    }
}
