//! Integration tests of the Windows adapter on a real NTFS volume (CI: windows-latest,
//! elevated). Every test works in its own directory below the temp folder and removes
//! it at the end.

#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reclaim_storage_core::cleanup::{ItemOutcome, SkipReason};
use reclaim_storage_core::entry::{Attrs, Kind};
use reclaim_storage_core::guard::{Folder, Guard};
use reclaim_storage_core::path::PathStyle;
use reclaim_storage_core::ports::{Fs, Remover, Target, Volumes};
use reclaim_storage_core::tree::{TOP, Tree};
use reclaim_storage_engine::ScanControl;
use reclaim_storage_engine::scan::{ScanOptions, ScanState, ScanTarget, scan_blocking};
use reclaim_storage_windows::watch::Watcher;
use reclaim_storage_windows::{WinFs, WinRemover, WinVolumes, fast, known_folders, shell};

const FILE_SHARE_READ: u32 = 1;

/// A unique directory below the temp folder (long form, no `\\?\` prefix).
struct TempDir {
    path: String,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let base = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let base = base.to_string_lossy();
        let base = base.strip_prefix("\\\\?\\").unwrap_or(&base).to_owned();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = format!(
            "{}\\reclaim-sw-{tag}-{}-{nanos}",
            base.trim_end_matches('\\'),
            std::process::id()
        );
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn p(&self, rel: &str) -> String {
        format!("{}\\{rel}", self.path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // The long-path form also removes the > MAX_PATH test tree; junctions are
        // removed, not followed.
        let _ = std::fs::remove_dir_all(format!("\\\\?\\{}", self.path));
    }
}

fn write(path: &str, len: usize) {
    if let Some((dir, _)) = path.rsplit_once('\\') {
        std::fs::create_dir_all(dir).expect("parent dir");
    }
    std::fs::write(path, vec![b'x'; len]).expect("write file");
}

fn junction(link: &str, target: &str) {
    let st = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J", link, target])
        .output()
        .expect("run mklink");
    assert!(st.status.success(), "mklink /J failed: {st:?}");
}

fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(format!("\\\\?\\{path}")).is_ok()
}

fn guard() -> Guard {
    Guard::windows(&known_folders())
}

/// A cleanup target as discovery would produce it (identity from `stat`).
fn target(path: &str, root: Option<&str>) -> Target {
    let m = WinFs.stat(path).expect("stat target");
    Target {
        path: path.to_owned(),
        root: root.map(str::to_owned),
        file_id: m.file_id,
        allocated: m.allocated.unwrap_or(0),
        is_dir: m.kind == Kind::Dir,
    }
}

#[test]
fn winfs_scan_counts_links_once_and_handles_long_paths() {
    let d = TempDir::new("scan");
    write(&d.p("a.bin"), 10_000);
    write(&d.p("sub\\b.bin"), 5_000);
    write(&d.p("sub\\deep\\c.txt"), 100);
    std::fs::hard_link(d.p("sub\\b.bin"), d.p("hl.bin")).expect("hard link");
    junction(&d.p("j"), &d.p("sub"));
    // A path longer than 300 characters, created through the long-path prefix.
    let mut long_dir = d.p("long");
    while long_dir.len() < 300 {
        long_dir.push('\\');
        long_dir.push_str(&"x".repeat(50));
    }
    std::fs::create_dir_all(format!("\\\\?\\{long_dir}")).expect("long dir");
    let long_file = format!("{long_dir}\\long.txt");
    std::fs::write(format!("\\\\?\\{long_file}"), vec![b'y'; 123]).expect("long file");

    // Listing facts.
    let mut entries = Vec::new();
    WinFs.read_dir(&d.path, &mut entries).expect("read_dir");
    let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.bin", "hl.bin", "j", "long", "sub"]);
    assert!(
        entries
            .iter()
            .all(|e| e.file_id.is_some() && e.volume.is_some())
    );
    let a = entries.iter().find(|e| e.name == "a.bin").expect("a.bin");
    assert_eq!((a.kind, a.logical), (Kind::File, 10_000));
    assert!(a.allocated.expect("allocated") >= 10_000);
    let j = entries.iter().find(|e| e.name == "j").expect("j");
    assert_eq!((j.kind, j.logical), (Kind::Link, 0));
    assert!(j.attrs.has(Attrs::REPARSE));
    // stat: link count, same identity convention as the listing, volume roots.
    let hl = WinFs.stat(&d.p("hl.bin")).expect("stat hl");
    assert_eq!(hl.nlink, Some(2));
    let b = WinFs.stat(&d.p("sub\\b.bin")).expect("stat b");
    assert_eq!(hl.file_id, b.file_id);
    let listed_hl = entries.iter().find(|e| e.name == "hl.bin").expect("hl");
    assert_eq!(listed_hl.file_id, hl.file_id);
    assert_eq!(WinFs.stat(&d.p("j")).expect("stat j").kind, Kind::Link);
    let root = WinFs.stat("C:\\").expect("stat C:\\");
    assert_eq!((root.kind, root.name.as_str()), (Kind::Dir, "C:\\"));
    let mut read = String::new();
    std::io::Read::read_to_string(
        &mut WinFs.open_read(&long_file).expect("open long"),
        &mut read,
    )
    .expect("read long");
    assert_eq!(read.len(), 123);

    // Scan.
    let tree = Arc::new(RwLock::new(Tree::new(PathStyle::Windows)));
    let s = scan_blocking(
        &WinFs,
        &tree,
        ScanTarget::Roots(vec![d.path.clone()]),
        &ScanOptions::default(),
        &ScanControl::default(),
    );
    assert_eq!(s.state, ScanState::Complete);
    assert!(!s.partial, "errors: {:?}", tree.read().unwrap().errors());
    let t = tree.read().unwrap();
    assert_eq!(t.hardlink_duplicates(), 1);
    // The hard link and the junction's target are counted once.
    assert_eq!(t.node(TOP).logical, 10_000 + 5_000 + 100 + 123);
    let jn = t.find(&d.p("j")).expect("junction node");
    assert_eq!(t.node(jn).kind, Kind::Link);
    assert_eq!(t.child_count(jn), 0);
    let ln = t.find(&long_file).expect("long file found");
    assert_eq!(t.node(ln).logical, 123);
    let an = t.find(&d.p("a.bin")).expect("a.bin node");
    assert!(t.node(an).allocated >= 10_000);
}

#[test]
fn remover_revalidates_before_deleting() {
    let d = TempDir::new("remove");
    let g = guard();
    let root = d.path.as_str();

    // Plain delete.
    let f = d.p("del.tmp");
    write(&f, 3_000);
    let t = target(&f, Some(root));
    assert!(matches!(WinRemover.delete(&t, &g), ItemOutcome::Deleted(_)));
    assert!(!exists(&f));

    // Read-only files are deleted too (cleanup of caches).
    let ro = d.p("ro.tmp");
    write(&ro, 10);
    let mut perm = std::fs::metadata(&ro).expect("meta").permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm).expect("set readonly");
    let t = target(&ro, Some(root));
    assert!(matches!(WinRemover.delete(&t, &g), ItemOutcome::Deleted(_)));
    assert!(!exists(&ro));

    // Replaced after discovery: another identity.
    let r = d.p("replaced.tmp");
    write(&r, 10);
    let t = target(&r, Some(root));
    std::fs::remove_file(&r).expect("remove");
    write(&r, 10);
    assert_eq!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Changed)
    );
    assert!(exists(&r));

    // Gone.
    let gone = d.p("gone.tmp");
    write(&gone, 10);
    let t = target(&gone, Some(root));
    std::fs::remove_file(&gone).expect("remove");
    assert_eq!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Missing)
    );

    // Parent reached through a junction.
    write(&d.p("real\\x.tmp"), 10);
    junction(&d.p("jl"), &d.p("real"));
    let t = target(&d.p("jl\\x.tmp"), Some(root));
    assert_eq!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Link)
    );
    assert!(exists(&d.p("real\\x.tmp")));
    // The junction itself is a reparse point.
    let t = Target {
        path: d.p("jl"),
        root: Some(root.to_owned()),
        file_id: None,
        allocated: 0,
        is_dir: true,
    };
    assert_eq!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Link)
    );

    // A directory where a file is expected.
    std::fs::create_dir(d.p("adir")).expect("mkdir");
    let mut t = target(&d.p("adir"), Some(root));
    t.is_dir = false;
    assert_eq!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Changed)
    );

    // Held open without FILE_SHARE_DELETE.
    let busy = d.p("busy.tmp");
    write(&busy, 10);
    let t = target(&busy, Some(root));
    {
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&busy)
            .expect("open busy");
        assert_eq!(
            WinRemover.delete(&t, &g),
            ItemOutcome::Skipped(SkipReason::InUse)
        );
    }
    assert!(exists(&busy));

    // Outside its root.
    std::fs::create_dir(d.p("rootA")).expect("mkdir");
    let out = d.p("outside.tmp");
    write(&out, 10);
    let t = target(&out, Some(&d.p("rootA")));
    assert!(matches!(
        WinRemover.delete(&t, &g),
        ItemOutcome::Skipped(SkipReason::Protected(_))
    ));
    assert!(exists(&out));

    // Empty directories.
    std::fs::create_dir(d.p("empty")).expect("mkdir");
    assert!(WinRemover.remove_empty_dir(&d.p("empty"), root, &g));
    assert!(!exists(&d.p("empty")));
    write(&d.p("full\\f.tmp"), 1);
    assert!(!WinRemover.remove_empty_dir(&d.p("full"), root, &g));
    assert!(exists(&d.p("full\\f.tmp")));
    // Never the root itself.
    assert!(!WinRemover.remove_empty_dir(&d.p("rootA"), &d.p("rootA"), &g));
    assert!(exists(&d.p("rootA")));
}

#[test]
fn recycle_moves_to_the_bin() {
    let d = TempDir::new("recycle");
    let g = guard();
    let f = d.p("to-bin.tmp");
    write(&f, 2_000);
    let t = target(&f, None);
    let o = WinRemover.recycle(&t, &g);
    assert!(matches!(o, ItemOutcome::Recycled(_)), "{o:?}");
    assert!(!exists(&f));
    // A folder selection.
    write(&d.p("folder\\inner.tmp"), 10);
    let t = target(&d.p("folder"), None);
    assert!(t.is_dir);
    let o = WinRemover.recycle(&t, &g);
    assert!(matches!(o, ItemOutcome::Recycled(_)), "{o:?}");
    assert!(!exists(&d.p("folder")));
    assert!(WinRemover.recycle_bin().is_some());
}

#[test]
fn known_folders_and_volumes() {
    let k = known_folders();
    for f in [
        Folder::LocalAppData,
        Folder::Temp,
        Folder::Windows,
        Folder::Profile,
        Folder::Downloads,
        Folder::SystemDrive,
    ] {
        let p = k.get(f).unwrap_or_else(|| panic!("{f:?} missing"));
        assert!(p.len() >= 3 && !p.contains('~'), "{f:?}: {p}");
    }
    assert!(!k.get(Folder::Temp).unwrap().ends_with('\\'));
    let vols = WinVolumes.list();
    let c = vols
        .iter()
        .find(|v| v.root.eq_ignore_ascii_case("C:\\"))
        .expect("C:\\ listed");
    assert!(c.total > c.free && c.free > 0, "{c:?}");
    assert!(c.fs.eq_ignore_ascii_case("NTFS"), "{c:?}");
    let d = TempDir::new("vol");
    let v = WinVolumes.volume_of(&d.path).expect("volume of temp");
    assert!(v.root.eq_ignore_ascii_case("C:\\"), "{v:?}");
    let _ = shell::is_elevated();
}

#[test]
fn fast_scan_matches_the_volume() {
    if !shell::is_elevated() {
        eprintln!("note: not elevated; the fast scan test is skipped");
        return;
    }
    let d = TempDir::new("fast");
    let known = d.p("known.bin");
    write(&known, 7_777);
    write(&d.p("pair\\one.bin"), 4_321);
    std::fs::hard_link(d.p("pair\\one.bin"), d.p("pair\\two.bin")).expect("hard link");
    let root = WinVolumes.volume_of(&d.path).expect("volume of temp").root;
    let tree = RwLock::new(Tree::new(PathStyle::Windows));
    let progress = std::sync::atomic::AtomicU64::new(0);
    let s = fast::fast_scan(&root, &tree, &ScanControl::default(), &|n| {
        progress.store(n, std::sync::atomic::Ordering::Relaxed);
    })
    .unwrap_or_else(|e| panic!("fast scan failed: {e}"));
    assert!(!s.cancelled);
    assert!(s.files > 1000, "{s:?}");
    assert!(progress.load(std::sync::atomic::Ordering::Relaxed) > 1000);
    let t = tree.read().unwrap();
    let n = t.find(&known).expect("known file found");
    assert_eq!((t.node(n).kind, t.node(n).logical), (Kind::File, 7_777));
    let one = t.find(&d.p("pair\\one.bin")).expect("one");
    let two = t.find(&d.p("pair\\two.bin")).expect("two");
    let dup = |id| t.node(id).attrs.has(Attrs::HARDLINK_DUP);
    assert!(dup(one) ^ dup(two), "exactly one name is the duplicate");
    let pair = t.find(&d.p("pair")).expect("pair dir");
    assert_eq!(t.node(pair).logical, 4_321);
    assert_eq!(s.logical, t.node(TOP).logical);
    // A second run needs an empty index.
    assert!(fast::fast_scan(&root, &tree, &ScanControl::default(), &|_| {}).is_err());
}

#[test]
fn fast_scan_refuses_folders() {
    let d = TempDir::new("fastdir");
    let tree = RwLock::new(Tree::new(PathStyle::Windows));
    let r = fast::fast_scan(&d.path, &tree, &ScanControl::default(), &|_| {});
    assert_eq!(r, Err(fast::FastError::NotWholeVolume));
}

#[test]
fn watcher_reports_changed_directories() {
    let d = TempDir::new("watch");
    std::fs::create_dir(d.p("sub")).expect("mkdir");
    let w = Watcher::start(&d.path).expect("start watcher");
    // Let the thread issue its first wait (changes before it are not reported).
    std::thread::sleep(Duration::from_millis(300));
    write(&d.p("sub\\new.txt"), 42);
    let want = d.p("sub").to_lowercase();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let (changed, _overflowed) = w.take_changed();
        seen.extend(changed);
        if seen.iter().any(|p| p.to_lowercase() == want) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    w.stop();
    assert!(
        seen.iter().any(|p| p.to_lowercase() == want),
        "changes seen: {seen:?}"
    );
    // Starting on a missing folder fails cleanly.
    assert!(Watcher::start(&d.p("missing")).is_err());
}
