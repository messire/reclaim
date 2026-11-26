//! Storage view model against the in-memory file system: scan, tree, map, lists,
//! search, recycling, cleanup and duplicates, with the background threads running for
//! real and their events applied as the adapter does.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reclaim_app::storage_vm::{
    ListSort, ListTab, MapMode, SLOT_DIR, SearchForm, Storage, StorageEvent, StorageForm,
    StoragePlatform,
};
use reclaim_storage_core::guard::{Folder, Guard, KnownFolders};
use reclaim_storage_engine::fake::FakeFs;
use reclaim_storage_engine::store::StorageStore;

fn kf() -> KnownFolders {
    KnownFolders::new([
        (Folder::Profile, "/home/me".to_owned()),
        (Folder::LocalAppData, "/home/me/.cache".to_owned()),
        (Folder::Temp, "/tmp/me".to_owned()),
        (Folder::Downloads, "/home/me/Downloads".to_owned()),
        (Folder::Documents, "/home/me/Documents".to_owned()),
    ])
}

const NOW_MS: u64 = 1_800_000_000_000;

fn setup(f: FakeFs, store: Option<StorageStore>) -> (Storage, Receiver<StorageEvent>, Arc<FakeFs>) {
    let f = Arc::new(f);
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let folders = kf();
    let guard = Arc::new(Guard::posix(&folders));
    let p = StoragePlatform {
        fs: f.clone(),
        volumes: f.clone(),
        remover: f.clone(),
        folders,
        guard,
        fast_scan: None,
        elevated: false,
        store,
        clock: Arc::new(|| NOW_MS),
    };
    let notify = Arc::new(move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    (Storage::new(p, notify, 0), rx, f)
}

/// Applies events until `done` holds (or fails after a timeout).
fn pump(s: &mut Storage, rx: &Receiver<StorageEvent>, done: impl Fn(&Storage) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done(s) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!left.is_zero(), "timed out waiting for background work");
        if let Ok(e) = rx.recv_timeout(left) {
            s.apply(e);
        }
    }
}

fn sample() -> FakeFs {
    let mut f = FakeFs::sample();
    f.add_file("/r/b", "movie.mkv", 50_000);
    f.add_file("/r/b", "copy.mkv", 50_000);
    f.add_content("/r", "one.bin", vec![7; 3000]);
    f.add_content("/r/b", "two.bin", vec![7; 3000]);
    f.mkdir("/r/locked");
    f.add_file("/r/locked", "x", 9);
    f.deny("/r/locked");
    f
}

fn scanned() -> (Storage, Receiver<StorageEvent>, Arc<FakeFs>) {
    let (mut s, rx, f) = setup(sample(), None);
    s.activate();
    s.scan(vec!["/r".into()]);
    assert!(s.scanning());
    pump(&mut s, &rx, |s| !s.scanning());
    (s, rx, f)
}

#[test]
fn nothing_runs_until_asked() {
    let (mut s, rx, _) = setup(sample(), None);
    assert!(!s.scanning());
    assert!(rx.try_recv().is_err());
    let st = s.status();
    assert!(st.line.contains("Nothing is scanned until you ask"));
    assert!(!st.has_index);
    let d = s.activate();
    assert!(d.drives);
    assert_eq!(s.drives().len(), 1);
    assert!(s.drives()[0].detail.contains("free of"));
    // Empty input is rejected with a message, not a scan.
    s.scan(vec!["  ".into()]);
    assert!(!s.scanning());
    assert!(s.status().line.contains("Choose a drive"));
}

#[test]
fn scan_builds_tree_map_and_lists() {
    let (mut s, _rx, _) = scanned();
    let st = s.status();
    assert!(st.has_index && !st.scanning);
    assert!(st.partial, "an unreadable directory makes the scan partial");
    assert!(!st.cancelled, "gaps alone are not a cancelled scan");
    assert!(st.line.contains("unreadable"));

    // The single root is expanded; children sorted by size.
    let rows = s.tree_rows().to_vec();
    assert_eq!(rows[0].name, "/r");
    assert!(rows[0].expanded);
    assert_eq!(rows[1].name, "b");
    assert!(rows[1].is_dir && rows[1].slot == SLOT_DIR);
    assert!(
        rows.iter()
            .any(|r| r.name == "locked" && r.flags.contains("unreadable"))
    );

    // Expand b: movie files appear, biggest first.
    let b = rows[1].id;
    s.toggle_expanded(b);
    let rows = s.tree_rows().to_vec();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    let i = names.iter().position(|n| *n == "b").unwrap();
    assert!(names[i + 1].ends_with(".mkv"));

    // Treemap tiles cover the area, files are coloured by extension.
    s.set_map_size(400.0, 300.0);
    let tiles = s.tiles();
    assert!(!tiles.is_empty());
    let mkv = tiles
        .iter()
        .filter(|t| t.leaf && t.slot != SLOT_DIR)
        .count();
    assert!(mkv >= 2);
    let area: f32 = tiles.iter().filter(|t| t.leaf).map(|t| t.w * t.h).sum();
    assert!(area > 400.0 * 300.0 * 0.8);

    // Click on the map selects; tree and details follow.
    let big = tiles
        .iter()
        .max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)))
        .unwrap();
    s.select_at(big.x + big.w / 2.0, big.y + big.h / 2.0);
    let sel = s.selected().unwrap();
    s.tree_rows();
    assert!(s.selected_row().is_some());
    let v = s.selection();
    assert!(v.any && v.path.starts_with("/r/"));
    assert!(v.detail.contains("of parent"));
    assert!(v.can_recycle);
    assert!(s.describe(sel).contains(&v.path));
    assert!(s.selection_rect().is_some());

    // Lists: largest files and extension colours agree with the map.
    let files = s.list_rows();
    assert!(files[0].name.ends_with(".mkv"));
    s.set_tab(ListTab::Extensions);
    let ext = s.list_rows();
    assert_eq!(ext[0].name, ".mkv");
    assert_eq!(ext[0].slot, 1);
    s.set_tab(ListTab::Errors);
    let err = s.list_rows();
    assert_eq!(err.len(), 1);
    assert!(err[0].detail.ends_with("locked"));
    s.set_tab(ListTab::Files);
    s.sort_lists(ListSort::Path, true);
    let by_path = s.list_rows();
    let mut sorted: Vec<String> = by_path.iter().map(|r| r.detail.clone()).collect();
    sorted.sort();
    assert_eq!(
        by_path.iter().map(|r| r.detail.clone()).collect::<Vec<_>>(),
        sorted
    );
}

#[test]
fn zoom_back_forward_and_icicle() {
    let (mut s, _rx, _) = scanned();
    s.set_map_size(300.0, 200.0);
    s.tiles();
    let b = s.tree_rows().iter().find(|r| r.name == "b").unwrap().id;
    s.zoom_in(b);
    let v = s.selection();
    assert_eq!(v.zoom_path, "/r/b");
    assert!(v.can_back && v.can_up && !v.can_forward);
    s.back();
    assert!(s.selection().can_forward);
    s.forward();
    assert_eq!(s.selection().zoom_path, "/r/b");
    s.zoom_up();
    assert_eq!(s.selection().zoom_path, "/r");
    s.set_map_mode(MapMode::Icicle);
    let tiles = s.tiles();
    assert!(tiles.iter().all(|t| t.leaf));
    assert!(tiles.iter().any(|t| t.y > 0.0));
}

#[test]
fn map_double_click_zooms_one_level_at_a_time() {
    let (mut s, _rx, _) = scanned();
    s.set_map_size(300.0, 200.0);
    s.tiles();
    let id = |s: &mut reclaim_app::storage_vm::Storage, name: &str| {
        s.list_rows();
        let t = s.tree().read().unwrap();
        (0..t.len() as u32)
            .find(|&i| t.get(i).is_some_and(|n| n.live()) && t.name(i) == name)
            .unwrap()
    };
    let movie = id(&mut s, "movie.mkv");
    s.zoom_toward(movie);
    assert_eq!(s.selection().zoom_path, "/r");
    s.zoom_toward(movie);
    assert_eq!(s.selection().zoom_path, "/r/b");
    // Already in the file's folder: nothing deeper to zoom into.
    s.zoom_toward(movie);
    assert_eq!(s.selection().zoom_path, "/r/b");
    // Outside the zoomed folder: no change.
    let one = id(&mut s, "one.bin");
    s.zoom_toward(one);
    assert_eq!(s.selection().zoom_path, "/r/b");
    s.back();
    assert_eq!(s.selection().zoom_path, "/r");
}

#[test]
fn selection_rectangle_follows_the_selection() {
    let (mut s, _rx, _) = scanned();
    s.set_map_size(300.0, 200.0);
    s.tiles();
    let b = s.tree_rows().iter().find(|r| r.name == "b").unwrap().id;
    s.select(b);
    let whole = s.selection_rect().expect("b has a cell");
    s.zoom_in(b);
    s.tiles();
    let zoomed = s.selection_rect().expect("b is the zoomed folder");
    assert_ne!(whole, zoomed);
    let one = s
        .tree_rows()
        .iter()
        .find(|r| r.name == "one.bin")
        .map(|r| r.id);
    if let Some(one) = one {
        s.select(one);
        assert_eq!(s.selection_rect(), None, "outside the zoomed folder");
    }
}

#[test]
fn index_is_freed_on_request_and_scans_again() {
    let (mut s, rx, _) = scanned();
    assert_eq!(s.release_after_min(), 10);
    assert!(s.has_index() && s.can_release_index());
    assert!(s.release_index());
    assert!(!s.has_index());
    assert!(s.tree_rows().is_empty());
    assert!(s.tiles().is_empty());
    assert_eq!(
        s.tree().read().unwrap().len(),
        1,
        "only the top node is left"
    );
    let st = s.status();
    assert!(!st.has_index && st.line.contains("freed"));
    // Nothing to free twice.
    assert!(!s.release_index());
    s.scan(vec!["/r".into()]);
    // Not while a scan runs.
    assert!(!s.can_release_index());
    assert!(!s.release_index());
    pump(&mut s, &rx, |s| !s.scanning());
    assert!(s.has_index() && !s.tree_rows().is_empty());
}

#[test]
fn search_runs_without_rescan() {
    let (mut s, rx, _) = scanned();
    let form = SearchForm {
        name: "*.mkv".into(),
        min_size: "40 KB".into(),
        ..SearchForm::default()
    };
    s.search(&form);
    assert_eq!(s.tab(), ListTab::Search);
    pump(&mut s, &rx, |s| s.search_message().contains("matches"));
    assert!(s.search_message().starts_with("2 matches"));
    assert_eq!(s.list_rows().len(), 2);

    s.search(&SearchForm {
        name: "/^one\\.(bin|txt)$/".into(),
        ..SearchForm::default()
    });
    pump(&mut s, &rx, |s| s.search_message().contains("matches"));
    assert_eq!(s.list_rows().len(), 1);

    s.search(&SearchForm {
        min_size: "lots".into(),
        ..SearchForm::default()
    });
    assert!(s.search_message().contains("not a size"));
    s.search(&SearchForm::default());
    assert!(s.search_message().contains("at least one criterion"));
    s.clear_search();
    assert!(s.list_rows().is_empty());
}

#[test]
fn recycle_selection_after_confirmation() {
    let (mut s, rx, f) = scanned();
    let id = s
        .list_rows()
        .iter()
        .find(|r| r.name == "movie.mkv")
        .and_then(|r| r.id)
        .unwrap();
    s.select(id);
    s.request_recycle_selected();
    let c = s.confirm_view().unwrap().clone();
    assert!(c.title.contains("Recycle Bin"));
    assert!(c.body.contains("restore"));
    assert!(!c.danger);
    // Dismiss changes nothing.
    s.dismiss();
    assert!(f.exists("/r/b/movie.mkv"));
    s.request_recycle_selected();
    s.confirm();
    pump(&mut s, &rx, |s| s.status().line.contains("Recycle Bin"));
    assert!(!f.exists("/r/b/movie.mkv"));
    assert!(s.selected().is_none());
    assert!(!s.list_rows().iter().any(|r| r.name == "movie.mkv"));
}

#[test]
fn roots_are_not_recyclable() {
    let (mut s, _rx, _) = scanned();
    let root = s.tree_rows()[0].id;
    s.select(root);
    assert!(!s.selection().can_recycle);
}

#[test]
fn refresh_reflects_changes() {
    let (mut s, rx, f) = scanned();
    let before = s.tree_rows()[0].size.clone();
    // The file system changes behind the index (FakeFs is shared through an Arc, so
    // mutate through interior state: remove via the remover API).
    use reclaim_storage_core::ports::{Remover, Target};
    let g = Guard::posix(&kf());
    let o = f.recycle(
        &Target {
            path: "/r/b/copy.mkv".into(),
            root: None,
            file_id: None,
            mtime: None,
            allocated: 0,
            is_dir: false,
        },
        &g,
    );
    assert!(matches!(
        o,
        reclaim_storage_core::cleanup::ItemOutcome::Recycled(_)
    ));
    s.refresh_paths(&["/r/b/copy.mkv".into()]);
    assert!(s.scanning());
    pump(&mut s, &rx, |s| !s.scanning());
    assert_ne!(s.tree_rows()[0].size, before);
    assert!(!s.list_rows().iter().any(|r| r.name == "copy.mkv"));
}

#[test]
fn cancel_keeps_partial_results() {
    let (mut s, rx, _) = setup(FakeFs::wide(40, 20).slow(Duration::from_millis(5)), None);
    s.scan(vec!["/w".into()]);
    s.set_paused(true);
    s.set_paused(false);
    s.cancel_scan();
    pump(&mut s, &rx, |s| !s.scanning());
    let st = s.status();
    assert!(st.partial);
    assert!(st.cancelled);
    assert!(st.line.contains("cancelled") || st.line.contains("gaps"));
}

#[test]
fn duplicates_keep_one_copy() {
    let (mut s, rx, f) = scanned();
    let mut form = s.settings_form();
    form.duplicate_min_kib = "1".into();
    s.save_settings(&form).unwrap();
    s.find_duplicates();
    pump(&mut s, &rx, |s| !s.dupes_view().busy);
    let v = s.dupes_view();
    assert!(v.status.contains("groups"), "{}", v.status);
    // Content-identical .bin files form a confirmed group; equal-sized .mkv files with
    // different content (FakeFs fills by name) may not.
    let header = v.rows.iter().find(|r| r.header).unwrap();
    assert!(header.text.contains("copies"));
    let g = header.group;
    let files: Vec<usize> = v
        .rows
        .iter()
        .filter(|r| r.group == g && !r.header)
        .filter_map(|r| r.file)
        .collect();
    assert!(files.len() >= 2);
    for &fi in &files {
        s.toggle_duplicate(g, fi);
    }
    let v = s.dupes_view();
    let marked = v.rows.iter().filter(|r| r.marked).count();
    assert_eq!(marked, files.len() - 1, "the last copy cannot be marked");
    assert!(v.status.contains("always stays"));
    s.request_recycle_duplicates();
    assert!(s.confirm_view().is_some());
    s.confirm();
    pump(&mut s, &rx, |s| s.status().line.contains("Recycle Bin"));
    let remaining = ["/r/one.bin", "/r/b/two.bin"]
        .iter()
        .filter(|p| f.exists(p))
        .count();
    assert_eq!(remaining, 1);
}

#[test]
fn cleanup_examines_selects_confirms_and_reports() {
    let mut f = FakeFs::new();
    f.add_file("/tmp/me", "old.tmp", 5000);
    f.set_mtime("/tmp/me/old.tmp", (NOW_MS / 1000) as i64 - 10 * 86_400);
    f.add_file("/home/me/Downloads", "setup.exe", 80_000_000);
    f.set_mtime(
        "/home/me/Downloads/setup.exe",
        (NOW_MS / 1000) as i64 - 90 * 86_400,
    );
    f.add_file("/home/me/Documents", "thesis.doc", 100);
    f.set_bin(2, 1000);
    let dir = std::env::temp_dir().join(format!("reclaim-app-storage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut s, rx, f) = setup(f, Some(StorageStore::new(&dir)));
    s.activate();
    assert!(
        s.cleanup_view()
            .status
            .contains("Examining changes nothing")
    );
    s.cleanup_scan();
    let v = s.cleanup_view();
    assert!(v.busy);
    // Results arrive through the event channel, so nothing is examined yet: the first
    // category is being examined, the others wait their turn.
    assert_eq!(v.categories[0].activity, 2);
    assert!(v.categories[1..].iter().all(|c| c.activity == 1));
    pump(&mut s, &rx, |s| !s.cleanup_view().busy);
    let v = s.cleanup_view();
    assert!(v.categories.iter().all(|c| c.activity == 0));
    assert!(
        v.categories.iter().all(|c| !c.selected),
        "nothing is selected until the user ticks it"
    );
    assert!(!v.can_clean);
    s.cleanup_toggle("user-temp");
    let v = s.cleanup_view();
    let temp = v.categories.iter().find(|c| c.id == "user-temp").unwrap();
    assert!(temp.selected);
    assert!(v.status.contains("in 1 category"), "{}", v.status);
    assert!(f.exists("/tmp/me/old.tmp"), "examining deletes nothing");

    // Review categories are chosen file by file.
    let review = v
        .categories
        .iter()
        .find(|c| c.safety_level == 2 && c.enabled);
    if let Some(r) = review {
        let id = r.id.clone();
        s.cleanup_toggle(&id);
        let v = s.cleanup_view();
        assert!(v.per_file);
        assert!(!v.categories.iter().find(|c| c.id == id).unwrap().selected);
        if !v.candidates.is_empty() {
            s.cleanup_toggle_file(0);
            assert!(
                s.cleanup_view()
                    .categories
                    .iter()
                    .find(|c| c.id == id)
                    .unwrap()
                    .selected
            );
            s.cleanup_toggle_file(0);
        }
    }
    s.cleanup_focus("user-temp");
    assert!(s.cleanup_view().detail.contains("After cleaning"));

    s.request_cleanup();
    let c = s.confirm_view().unwrap().clone();
    assert!(c.title.contains("Temporary files"), "{}", c.title);
    assert!(
        !c.title.contains("categories"),
        "one category ticked: {}",
        c.title
    );
    assert!(c.danger);
    assert!(c.body.contains("permanently"));
    s.confirm();
    pump(&mut s, &rx, |s| !s.cleanup_view().report.is_empty());
    let v = s.cleanup_view();
    assert!(v.report.contains("Deleted"));
    assert!(!f.exists("/tmp/me/old.tmp"));
    assert!(f.exists("/home/me/Documents/thesis.doc"));
    assert_eq!(v.history.len(), 1, "the run is in the local history");
    // History survives a restart and holds no paths.
    let text = std::fs::read_to_string(dir.join("cleanup-history.jsonl")).unwrap();
    assert!(!text.contains("/tmp/me"));
    s.clear_history();
    assert!(s.cleanup_view().history.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_validate_and_switch_basis() {
    let (mut s, _rx, _) = scanned();
    let mut form: StorageForm = s.settings_form();
    form.scan_threads = "999".into();
    assert!(s.save_settings(&form).is_err());
    form.scan_threads = "2".into();
    form.release_index_after_min = "2000".into();
    assert!(s.save_settings(&form).is_err());
    form.release_index_after_min = "0".into();
    form.allocated_sizes = false;
    form.scan_exclusions = "node_modules\n\n*.tmp".into();
    s.save_settings(&form).unwrap();
    assert_eq!(s.settings().scan_threads, 2);
    assert_eq!(s.release_after_min(), 0);
    assert_eq!(s.settings().scan_exclusions, vec!["node_modules", "*.tmp"]);
    // Logical sizes now: the 1-byte file shows as 1 B.
    let rows = s.tree_rows().to_vec();
    assert!(rows.iter().any(|r| r.name == "a" && r.size == "1 B"));
}

#[test]
fn export_writes_json_and_csv() {
    let (mut s, rx, _) = scanned();
    let dir = std::env::temp_dir().join(format!("reclaim-app-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let json = dir.join("scan.json").to_string_lossy().into_owned();
    s.export(json.clone(), false);
    pump(&mut s, &rx, |s| s.status().line.contains("Exported"));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v["complete"], false);
    let csv = dir.join("scan.csv").to_string_lossy().into_owned();
    s.export(csv.clone(), true);
    pump(&mut s, &rx, |s| s.status().line.contains("scan.csv"));
    assert!(std::fs::read_to_string(&csv).unwrap().contains("path,kind"));
    s.export("/nonexistent-dir/x/y.json".into(), false);
    pump(&mut s, &rx, |s| s.status().line.contains("Export failed"));
    let _ = std::fs::remove_dir_all(&dir);
}
