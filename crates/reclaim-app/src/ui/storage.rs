//! Storage screens' adapter: creates [`Storage`] on the first visit (nothing is scanned
//! or started before), pushes its views into the window and wires the callbacks.
//! Background events arrive through `invoke_from_event_loop`.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use reclaim_app::storage_vm::{
    FastStats, ListSort, ListTab, MapMode, SLOT_DIR, SLOT_REST, SearchForm, Storage, StorageDirty,
    StorageForm, StoragePlatform,
};
use reclaim_storage_core::guard::Guard;
use reclaim_storage_core::tree::Tree;
use reclaim_storage_engine::scan::ScanControl;
use reclaim_storage_engine::store::StorageStore;
use reclaim_storage_windows::{WinFs, WinRemover, WinVolumes, fast, known_folders, shell, watch};

use super::view::{CandRow, CatRow, DriveItem, DupRow, ListRow, MainWindow, Tile, TreeRow};
use super::{App, with};

/// Treemap colours: slot 0 is "other", 1..=14 the largest extensions, then directory
/// frames and merged rest tiles. Muted so the dark theme stays calm.
const PALETTE: [u32; 17] = [
    0x5b6b73, 0x3fe3f2, 0xc9a86a, 0x7d9cf0, 0xe07a9b, 0x8fd18a, 0xb58cf0, 0xf0a35e, 0x4fb8a8,
    0xd8d06a, 0x6fa0c8, 0xc46a6a, 0x9fb36a, 0xa07ac0, 0x7fc6e6, 0x22323b, 0x3a4a52,
];

fn color(slot: u8) -> Color {
    let i = match slot {
        SLOT_DIR => 15,
        SLOT_REST => 16,
        s => usize::from(s).min(14),
    };
    let rgb = PALETTE[i];
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Updates a list in place when only row contents changed (a click that selects a row),
/// so the rows on screen are kept and a click or double-click in progress is not lost.
/// A different row count replaces the model.
fn update<T: Clone + PartialEq + 'static>(
    cur: &ModelRc<T>,
    v: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if let Some(vm) = cur.as_any().downcast_ref::<VecModel<T>>()
        && vm.row_count() == v.len()
    {
        for (i, r) in v.into_iter().enumerate() {
            if vm.row_data(i).as_ref() != Some(&r) {
                vm.set_row_data(i, r);
            }
        }
    } else {
        set(model(v));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The platform services and the storage state.
fn create(utc_offset_min: i32) -> Storage {
    let folders = known_folders();
    let guard = Arc::new(Guard::windows(&folders));
    let fast_scan: reclaim_app::storage_vm::FastScan = Arc::new(
        |root: &str, tree: &RwLock<Tree>, control: &ScanControl, progress: &dyn Fn(u64)| {
            fast::fast_scan(root, tree, control, progress)
                .map(|s| FastStats {
                    files: s.files,
                    dirs: s.dirs,
                    elapsed_ms: s.elapsed_ms,
                    cancelled: s.cancelled,
                })
                .map_err(|e| e.to_string())
        },
    );
    let platform = StoragePlatform {
        fs: Arc::new(WinFs),
        volumes: Arc::new(WinVolumes),
        remover: Arc::new(WinRemover),
        folders,
        guard,
        fast_scan: Some(fast_scan),
        elevated: shell::is_elevated(),
        store: Some(StorageStore::new(reclaim_engine::Store::default_dir())),
        clock: Arc::new(now_ms),
    };
    let notify = Arc::new(|e| {
        let _ = slint::invoke_from_event_loop(move || {
            with(|a| {
                if let Some(s) = &mut a.storage {
                    let d = s.apply(e);
                    a.after_storage(d);
                }
            });
        });
    });
    Storage::new(platform, notify, utc_offset_min)
}

/// Native window handle for dialogs (0 if unavailable).
fn hwnd(w: &MainWindow) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = w.window().window_handle();
    match handle.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

impl App {
    /// Creates the storage state on first use and refreshes the drive list.
    pub(super) fn enter_storage(&mut self) {
        if self.storage.is_none() {
            self.storage = Some(create(self.state.utc_offset()));
        }
        if let Some(s) = &mut self.storage {
            let mut d = s.activate();
            d.merge(StorageDirty::all());
            self.push_storage(d);
        }
    }

    /// Pushes a storage change; starts or stops change watching after a scan.
    pub(super) fn after_storage(&mut self, d: StorageDirty) {
        if d.status
            && let Some(s) = &self.storage
            && !s.scanning()
            && self.watch_on
            && self.watcher.is_none()
        {
            self.start_watch();
        }
        self.push_storage(d);
    }

    fn start_watch(&mut self) {
        let Some(s) = &self.storage else { return };
        let roots: Vec<String> = {
            let t = s.tree().read().unwrap_or_else(|p| p.into_inner());
            t.roots().into_iter().map(|r| t.path(r)).collect()
        };
        let watchers: Vec<watch::Watcher> = roots
            .iter()
            .filter_map(|r| watch::Watcher::start(r).ok())
            .collect();
        if watchers.is_empty() {
            return;
        }
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_secs(3), || {
            with(|a| {
                let mut changed = Vec::new();
                let mut overflow = false;
                if let Some((ws, _)) = &a.watcher {
                    for w in ws {
                        let (c, o) = w.take_changed();
                        changed.extend(c);
                        overflow |= o;
                    }
                }
                if let Some(s) = &mut a.storage
                    && !s.scanning()
                    && (overflow || !changed.is_empty())
                {
                    let d = if overflow {
                        s.refresh()
                    } else {
                        s.refresh_paths(&changed)
                    };
                    a.push_storage(d);
                }
            });
        });
        self.watcher = Some((watchers, timer));
    }

    pub(super) fn stop_watch(&mut self) {
        if let Some((ws, timer)) = self.watcher.take() {
            timer.stop();
            for w in ws {
                w.stop();
            }
        }
    }

    /// Pushes the changed parts of the storage screens to the window.
    pub(super) fn push_storage(&mut self, d: StorageDirty) {
        let (Some(w), Some(s)) = (&self.window, &mut self.storage) else {
            return;
        };
        if d.drives {
            let drives = s.drives();
            w.set_st_drive_titles(model(
                drives
                    .iter()
                    .map(|v| SharedString::from(format!("{}  ·  {}", v.title, v.detail)))
                    .collect(),
            ));
            w.set_st_drives(model(
                drives
                    .into_iter()
                    .map(|v| DriveItem {
                        root: v.root.into(),
                        title: v.title.into(),
                        detail: v.detail.into(),
                        used: i32::from(v.used_permille),
                    })
                    .collect(),
            ));
        }
        if d.status {
            let st = s.status();
            w.set_st_status(st.line.into());
            w.set_st_scanning(st.scanning);
            w.set_st_paused(st.paused);
            w.set_st_can_pause(st.can_pause);
            w.set_st_has_index(st.has_index);
            w.set_st_partial(st.partial);
            w.set_st_cancelled(st.cancelled);
        }
        if d.tree || d.selection {
            let rows: Vec<TreeRow> = s
                .tree_rows()
                .iter()
                .map(|r| TreeRow {
                    id: r.id as i32,
                    depth: r.depth as i32,
                    name: r.name.as_str().into(),
                    size: r.size.as_str().into(),
                    share: i32::from(r.share),
                    parent_pct: r.parent_pct.as_str().into(),
                    files: r.files.as_str().into(),
                    modified: r.modified.as_str().into(),
                    expandable: r.expandable,
                    expanded: r.expanded,
                    selected: r.selected,
                    flags: r.flags.as_str().into(),
                    more: r.more,
                    color: color(r.slot),
                })
                .collect();
            update(&w.get_st_tree(), rows, |m| w.set_st_tree(m));
            if d.selection {
                w.set_st_tree_scroll(s.selected_row().map_or(-1, |i| i as i32));
                w.set_st_tree_serial(w.get_st_tree_serial().wrapping_add(1));
            }
        }
        if d.map {
            let tiles: Vec<Tile> = s
                .tiles()
                .into_iter()
                .map(|t| Tile {
                    x: t.x,
                    y: t.y,
                    w: t.w,
                    h: t.h,
                    color: color(t.slot),
                    leaf: t.leaf,
                })
                .collect();
            w.set_st_tiles(model(tiles));
            w.set_st_icicle(s.map_mode() == MapMode::Icicle);
        }
        if d.map || d.selection {
            match s.selection_rect() {
                Some((x, y, ww, h)) => {
                    w.set_st_sel_x(x);
                    w.set_st_sel_y(y);
                    w.set_st_sel_w(ww);
                    w.set_st_sel_h(h);
                    w.set_st_map_sel(true);
                }
                None => w.set_st_map_sel(false),
            }
            let v = s.selection();
            w.set_st_sel_path(v.path.into());
            w.set_st_sel_detail(v.detail.into());
            w.set_st_can_recycle(v.can_recycle);
            w.set_st_can_refresh(v.can_refresh);
            w.set_st_zoom_path(v.zoom_path.into());
            w.set_st_can_back(v.can_back);
            w.set_st_can_forward(v.can_forward);
            w.set_st_can_up(v.can_up);
            w.set_st_has_sel(v.any);
        }
        if d.lists {
            let rows: Vec<ListRow> = s
                .list_rows()
                .into_iter()
                .map(|r| ListRow {
                    id: r.id.map_or(-1, |i| i as i32),
                    name: r.name.into(),
                    detail: r.detail.into(),
                    size: r.size.into(),
                    extra: r.extra.into(),
                    color: color(r.slot),
                })
                .collect();
            update(&w.get_st_list(), rows, |m| w.set_st_list(m));
            w.set_st_tab(s.tab().index() as i32);
            let (by, asc) = s.list_sort();
            w.set_st_list_sort(by.index() as i32);
            w.set_st_list_asc(asc);
            w.set_st_search_message(s.search_message().into());
        }
        if d.cleanup {
            let v = s.cleanup_view();
            w.set_cl_categories(model(
                v.categories
                    .into_iter()
                    .map(|c| CatRow {
                        id: c.id.into(),
                        name: c.name.into(),
                        safety: c.safety.into(),
                        level: i32::from(c.safety_level),
                        size: c.size.into(),
                        files: c.files.into(),
                        selected: c.selected,
                        enabled: c.enabled,
                        focused: c.focused,
                    })
                    .collect(),
            ));
            w.set_cl_detail(v.detail.into());
            w.set_cl_per_file(v.per_file);
            w.set_cl_candidates(model(
                v.candidates
                    .into_iter()
                    .map(|c| CandRow {
                        path: c.path.into(),
                        size: c.size.into(),
                        modified: c.modified.into(),
                        selected: c.selected,
                    })
                    .collect(),
            ));
            w.set_cl_status(v.status.into());
            w.set_cl_busy(v.busy);
            w.set_cl_progress((v.progress * 1000.0) as i32);
            w.set_cl_can_clean(v.can_clean);
            w.set_cl_report(v.report.into());
            w.set_cl_history(model(
                v.history.into_iter().map(SharedString::from).collect(),
            ));
        }
        if d.dupes || d.status {
            let v = s.dupes_view();
            if d.dupes {
                w.set_du_rows(model(
                    v.rows
                        .into_iter()
                        .map(|r| DupRow {
                            group: r.group as i32,
                            file: r.file.map_or(-1, |f| f as i32),
                            text: r.text.into(),
                            size: r.size.into(),
                            marked: r.marked,
                            header: r.header,
                        })
                        .collect(),
                ));
            }
            w.set_du_status(v.status.into());
            w.set_du_busy(v.busy);
            w.set_du_progress((v.progress * 1000.0) as i32);
            w.set_du_can_find(v.can_find);
            w.set_du_can_recycle(v.can_recycle);
        }
        if d.confirm {
            match s.confirm_view() {
                Some(c) => {
                    w.set_sc_title(c.title.as_str().into());
                    w.set_sc_body(c.body.as_str().into());
                    w.set_sc_ok(c.ok.as_str().into());
                    w.set_sc_danger(c.danger);
                    w.set_sc_open(true);
                }
                None => w.set_sc_open(false),
            }
        }
        if d.settings {
            let f = s.settings_form();
            w.set_ss_scan_excl(f.scan_exclusions.into());
            w.set_ss_clean_excl(f.cleanup_exclusions.into());
            w.set_ss_threads(f.scan_threads.into());
            w.set_ss_fast(f.fast_ntfs_scan);
            w.set_ss_allocated(f.allocated_sizes);
            w.set_ss_dup_min(f.duplicate_min_kib.into());
            w.set_ss_release(f.release_index_after_min.into());
            w.set_ss_history(f.keep_cleanup_history);
            w.set_ss_fast_available(s.fast_scan_available());
            w.set_ss_message(self.storage_message.as_str().into());
        }
    }
}

/// Runs `f` on the storage state (created on demand) and pushes what changed.
fn st(f: impl FnOnce(&mut Storage) -> StorageDirty) {
    with(|a| {
        if a.storage.is_none() {
            a.enter_storage();
        }
        if let Some(s) = &mut a.storage {
            let d = f(s);
            a.push_storage(d);
        }
    });
}

fn search_form(w: &MainWindow) -> SearchForm {
    SearchForm {
        name: w.get_st_q_name().into(),
        path: w.get_st_q_path().into(),
        extensions: w.get_st_q_ext().into(),
        min_size: w.get_st_q_min().into(),
        max_size: w.get_st_q_max().into(),
        older_than_days: w.get_st_q_older().into(),
        newer_than_days: w.get_st_q_newer().into(),
        kind: w.get_st_q_kind().max(0) as usize,
    }
}

fn storage_form(w: &MainWindow) -> StorageForm {
    StorageForm {
        scan_exclusions: w.get_ss_scan_excl().into(),
        cleanup_exclusions: w.get_ss_clean_excl().into(),
        scan_threads: w.get_ss_threads().into(),
        fast_ntfs_scan: w.get_ss_fast(),
        allocated_sizes: w.get_ss_allocated(),
        duplicate_min_kib: w.get_ss_dup_min().into(),
        keep_cleanup_history: w.get_ss_history(),
        release_index_after_min: w.get_ss_release().into(),
    }
}

fn node(i: i32) -> u32 {
    i.max(0) as u32
}

/// Wires the storage callbacks of a new window.
pub(super) fn wire(w: &MainWindow) {
    let weak = w.as_weak();
    w.on_st_scan_drive(move |i| {
        let Some(w) = weak.upgrade() else { return };
        let root = w
            .get_st_drives()
            .iter()
            .nth(i.max(0) as usize)
            .map(|d| d.root.to_string());
        if let Some(r) = root {
            st(|s| s.scan(vec![r]));
        }
    });
    let weak = w.as_weak();
    w.on_st_scan_folders(move || {
        let Some(w) = weak.upgrade() else { return };
        let roots: Vec<String> = w
            .get_st_folders()
            .split(';')
            .map(|p| p.trim().trim_matches('"').to_owned())
            .filter(|p| !p.is_empty())
            .collect();
        st(|s| s.scan(roots));
    });
    let weak = w.as_weak();
    w.on_st_browse(move || {
        let Some(w) = weak.upgrade() else { return };
        if let Some(p) = shell::pick_folder(hwnd(&w), "Choose a folder to scan") {
            let cur = w.get_st_folders().to_string();
            let next = if cur.trim().is_empty() {
                p
            } else {
                format!("{cur}; {p}")
            };
            w.set_st_folders(next.into());
        }
    });
    w.on_st_pause(|on| st(|s| s.set_paused(on)));
    w.on_st_cancel(|| st(Storage::cancel_scan));
    w.on_st_refresh(|| st(Storage::refresh));
    let weak = w.as_weak();
    w.on_st_export(move |csv| {
        let Some(w) = weak.upgrade() else { return };
        let (desc, ext) = if csv {
            ("CSV files", "csv")
        } else {
            ("JSON files", "json")
        };
        if let Some(path) =
            shell::save_file_dialog(hwnd(&w), &format!("reclaim-scan.{ext}"), desc, ext)
        {
            st(|s| s.export(path, csv));
        }
    });
    w.on_st_set_watch(|on| {
        with(|a| {
            a.watch_on = on;
            if on {
                if a.storage.as_ref().is_some_and(|s| !s.scanning()) {
                    a.start_watch();
                }
            } else {
                a.stop_watch();
            }
        });
    });
    w.on_st_select(|id| st(|s| s.select(node(id))));
    w.on_st_toggle(|id| st(|s| s.toggle_expanded(node(id))));
    w.on_st_more(|id| st(|s| s.show_more(node(id))));
    w.on_st_zoom_node(|id| st(|s| s.zoom_in(node(id))));
    w.on_st_map_resized(|ww, h| st(|s| s.set_map_size(ww, h)));
    w.on_st_map_hover(|x, y| {
        with(|a| {
            a.storage
                .as_ref()
                .and_then(|s| s.node_at(x, y).map(|n| s.describe(n)))
                .unwrap_or_default()
        })
        .unwrap_or_default()
        .into()
    });
    w.on_st_map_click(|x, y| st(|s| s.select_at(x, y)));
    w.on_st_map_zoom(|x, y| {
        st(|s| match s.node_at(x, y) {
            Some(n) => {
                let mut d = s.select(n);
                d.merge(s.zoom_toward(n));
                d
            }
            None => StorageDirty::default(),
        })
    });
    w.on_st_back(|| st(Storage::back));
    w.on_st_forward(|| st(Storage::forward));
    w.on_st_up(|| st(Storage::zoom_up));
    w.on_st_set_icicle(|on| {
        st(|s| {
            s.set_map_mode(if on {
                MapMode::Icicle
            } else {
                MapMode::Treemap
            })
        })
    });
    w.on_st_open_explorer(|| {
        if let Some(p) = with(|a| a.storage.as_ref().and_then(Storage::selected_path)).flatten() {
            let _ = shell::open_in_explorer(&p);
        }
    });
    w.on_st_properties(|| {
        if let Some(p) = with(|a| a.storage.as_ref().and_then(Storage::selected_path)).flatten() {
            let _ = shell::show_properties(&p);
        }
    });
    w.on_st_recycle(|| st(Storage::request_recycle_selected));
    w.on_st_set_tab(|i| st(|s| s.set_tab(ListTab::from_index(i.max(0) as usize))));
    w.on_st_sort(|c, asc| st(|s| s.sort_lists(ListSort::from_index(c.max(0) as usize), asc)));
    let weak = w.as_weak();
    w.on_st_search(move || {
        let Some(w) = weak.upgrade() else { return };
        let f = search_form(&w);
        st(|s| s.search(&f));
    });
    w.on_st_clear_search(|| st(Storage::clear_search));

    w.on_cl_examine(|| st(Storage::cleanup_scan));
    w.on_cl_clean(|| st(Storage::request_cleanup));
    w.on_cl_cancel(|| st(Storage::cleanup_cancel));
    w.on_cl_toggle(|id| st(|s| s.cleanup_toggle(&id)));
    w.on_cl_focus(|id| st(|s| s.cleanup_focus(&id)));
    w.on_cl_toggle_file(|i| st(|s| s.cleanup_toggle_file(i.max(0) as usize)));
    w.on_cl_clear_history(|| st(Storage::clear_history));

    w.on_du_find(|| st(Storage::find_duplicates));
    w.on_du_cancel(|| st(Storage::cancel_duplicates));
    w.on_du_toggle(|g, f| {
        if g >= 0 && f >= 0 {
            st(|s| s.toggle_duplicate(g as usize, f as usize));
        }
    });
    w.on_du_open(|g, f| {
        let path = with(|a| {
            a.storage
                .as_ref()
                .and_then(|s| s.duplicate_path(g.max(0) as usize, f.max(0) as usize))
        })
        .flatten();
        if let Some(p) = path {
            let _ = shell::open_in_explorer(&p);
        }
    });
    w.on_du_recycle(|| st(Storage::request_recycle_duplicates));

    w.on_sc_confirm(|| st(Storage::confirm));
    w.on_sc_dismiss(|| st(Storage::dismiss));

    let weak = w.as_weak();
    w.on_ss_save(move || {
        let Some(w) = weak.upgrade() else { return };
        let form = storage_form(&w);
        with(|a| {
            if a.storage.is_none() {
                a.enter_storage();
            }
            let Some(s) = &mut a.storage else { return };
            let r = s.save_settings(&form);
            a.storage_message = match &r {
                Ok(_) => reclaim_app::strings::tr("settings.saved").to_owned(),
                Err(e) => e.clone(),
            };
            let d = r.unwrap_or(StorageDirty {
                settings: true,
                ..StorageDirty::default()
            });
            a.push_storage(d);
            if let Some(w) = &a.window {
                w.set_ss_message(a.storage_message.as_str().into());
            }
        });
    });
    w.on_ss_revert(|| {
        with(|a| {
            a.storage_message.clear();
            if a.storage.is_none() {
                a.enter_storage();
            }
            a.push_storage(StorageDirty {
                settings: true,
                ..StorageDirty::default()
            });
        });
    });
}
