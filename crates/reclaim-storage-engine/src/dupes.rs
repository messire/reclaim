//! Duplicate finder: size groups → 64 KiB prefix hash → full BLAKE3 hash.
//!
//! Runs only on request (never as part of an ordinary scan). Readers are bounded
//! (`readers`; one for rotational disks, where parallel reads would make the head seek
//! between files) and read with a 1 MiB buffer. Each file is read at most once in full
//! after its prefix; files of 64 KiB or less are confirmed from the prefix pass alone.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use reclaim_storage_core::dupes::{Confidence, DupFile, DupGroup, size_groups};
use reclaim_storage_core::entry::{Attrs, ErrorKind, Kind, ScanError};
use reclaim_storage_core::ports::Fs;
use reclaim_storage_core::tree::{NodeId, Tree};

/// Bytes hashed in the prefix pass.
pub const PREFIX: usize = 64 * 1024;
const BUF: usize = 1 << 20;

/// Settings.
#[derive(Debug, Clone, Copy)]
pub struct DupOptions {
    /// Ignore files smaller than this.
    pub min_size: u64,
    /// Parallel readers (at least 1).
    pub readers: usize,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_size: 1 << 20,
            readers: 2,
        }
    }
}

/// Which pass is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DupStage {
    /// Grouping by size.
    #[default]
    Grouping,
    /// Hashing prefixes.
    Prefix,
    /// Hashing whole files.
    Full,
    /// Finished.
    Done,
}

/// Progress counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DupProgress {
    /// Current pass.
    pub stage: DupStage,
    /// Files hashed in this pass.
    pub files_done: u64,
    /// Files to hash in this pass.
    pub files_total: u64,
    /// Bytes read so far (both passes).
    pub bytes_read: u64,
}

/// Result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DupResult {
    /// Groups, largest savings first.
    pub groups: Vec<DupGroup>,
    /// Files that could not be read.
    pub errors: Vec<ScanError>,
    /// Stopped by the user: unfinished groups are reported as probable.
    pub cancelled: bool,
    /// Bytes read.
    pub bytes_read: u64,
    /// Files compared (after size grouping).
    pub candidates: u64,
}

impl DupResult {
    /// Total bytes that keeping one copy per group would free.
    pub fn savings(&self) -> u64 {
        self.groups.iter().map(DupGroup::savings).sum()
    }
}

/// Files of a scanned subtree as duplicate candidates. Hard-link names counted once
/// in the index (`HARDLINK_DUP`) are left out, so a file and its links are never
/// reported as duplicates of each other.
pub fn files_from_tree(t: &Tree, from: NodeId, min_size: u64) -> Vec<DupFile> {
    let mut out = Vec::new();
    t.walk_paths(from, |id, path| {
        let n = t.node(id);
        if n.kind == Kind::File
            && !n.attrs.has(Attrs::HARDLINK_DUP)
            && !n.attrs.has(Attrs::OFFLINE)
            && n.logical >= min_size.max(1)
        {
            out.push(DupFile {
                path: path.to_owned(),
                size: n.logical,
                allocated: n.allocated,
                identity: None,
                mtime: n.mtime(),
            });
        }
        true
    });
    out
}

fn hash_file(
    fs: &dyn Fs,
    path: &str,
    limit: Option<usize>,
    cancel: &AtomicBool,
    read: &AtomicU64,
) -> Result<Option<blake3::Hash>, ErrorKind> {
    let mut f = fs.open_read(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; limit.map_or(BUF, |l| l.min(BUF))];
    let mut left = limit.unwrap_or(usize::MAX);
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let want = buf.len().min(left);
        let n = f
            .read(&mut buf[..want])
            .map_err(|e| ErrorKind::from_io(&e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        read.fetch_add(n as u64, Ordering::Relaxed);
        left -= n;
    }
    Ok(Some(h.finalize()))
}

/// Hashes `items` on `readers` threads; returns `(index, hash)` for those hashed.
#[allow(clippy::too_many_arguments)] // Shared counters of one find() call.
fn hash_all(
    fs: &dyn Fs,
    items: &[&DupFile],
    limit: Option<usize>,
    readers: usize,
    cancel: &AtomicBool,
    read: &AtomicU64,
    errors: &Mutex<Vec<ScanError>>,
    tick: &(dyn Fn(u64) + Sync),
) -> Vec<(usize, blake3::Hash)> {
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let out = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..readers.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                    if i >= items.len() || cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    match hash_file(fs, &items[i].path, limit, cancel, read) {
                        Ok(Some(h)) => out.lock().unwrap_or_else(|p| p.into_inner()).push((i, h)),
                        Ok(None) => return,
                        Err(kind) => {
                            errors
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(ScanError {
                                    path: items[i].path.clone(),
                                    kind,
                                })
                        }
                    }
                    tick(done.fetch_add(1, Ordering::Relaxed) + 1);
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap_or_else(|p| p.into_inner());
    v.sort_by_key(|x| x.0);
    v
}

fn regroup<'a>(
    items: &[&'a DupFile],
    hashes: Vec<(usize, blake3::Hash)>,
) -> Vec<(blake3::Hash, Vec<&'a DupFile>)> {
    let mut by: HashMap<blake3::Hash, Vec<&DupFile>> = HashMap::new();
    for (i, h) in hashes {
        by.entry(h).or_default().push(items[i]);
    }
    let mut v: Vec<_> = by.into_iter().filter(|(_, g)| g.len() > 1).collect();
    v.sort_by(|a, b| a.1[0].path.cmp(&b.1[0].path));
    v
}

/// Finds duplicates among `files`.
pub fn find(
    fs: &dyn Fs,
    files: Vec<DupFile>,
    opts: &DupOptions,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(DupProgress) + Sync),
) -> DupResult {
    on_progress(DupProgress::default());
    let groups = size_groups(files, opts.min_size);
    let read = AtomicU64::new(0);
    let errors = Mutex::new(Vec::new());
    let mut res = DupResult::default();
    let prefix_items: Vec<&DupFile> = groups.iter().flatten().collect();
    res.candidates = prefix_items.len() as u64;
    let total = prefix_items.len() as u64;
    let tick = |stage, total| {
        let read = &read;
        move |done| {
            on_progress(DupProgress {
                stage,
                files_done: done,
                files_total: total,
                bytes_read: read.load(Ordering::Relaxed),
            });
        }
    };

    // Pass 1: prefixes, per size group.
    let prefix = hash_all(
        fs,
        &prefix_items,
        Some(PREFIX),
        opts.readers,
        cancel,
        &read,
        &errors,
        &tick(DupStage::Prefix, total),
    );
    let mut by_size: HashMap<u64, Vec<(usize, blake3::Hash)>> = HashMap::new();
    for (i, h) in prefix {
        by_size
            .entry(prefix_items[i].size)
            .or_default()
            .push((i, h));
    }
    let mut full_todo: Vec<Vec<&DupFile>> = Vec::new();
    let mut sizes: Vec<u64> = by_size.keys().copied().collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    for size in sizes {
        let hs = by_size.remove(&size).unwrap_or_default();
        for (h, g) in regroup(&prefix_items, hs) {
            if size as usize <= PREFIX {
                res.groups.push(DupGroup {
                    size,
                    hash: h.to_hex().to_string(),
                    confidence: Confidence::Confirmed,
                    files: g.into_iter().cloned().collect(),
                });
            } else {
                full_todo.push(g);
            }
        }
    }

    // Pass 2: whole files of the prefix groups.
    let full_items: Vec<&DupFile> = full_todo.iter().flatten().copied().collect();
    let full = if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        hash_all(
            fs,
            &full_items,
            None,
            opts.readers,
            cancel,
            &read,
            &errors,
            &tick(DupStage::Full, full_items.len() as u64),
        )
    };
    res.cancelled = cancel.load(Ordering::Relaxed);
    if res.cancelled {
        // Prefix matches that were not verified: probable only.
        for g in full_todo {
            res.groups.push(DupGroup {
                size: g[0].size,
                hash: String::new(),
                confidence: Confidence::Probable,
                files: g.into_iter().cloned().collect(),
            });
        }
    } else {
        let mut by_size: HashMap<u64, Vec<(usize, blake3::Hash)>> = HashMap::new();
        for (i, h) in full {
            by_size.entry(full_items[i].size).or_default().push((i, h));
        }
        for (size, hs) in by_size {
            for (h, g) in regroup(&full_items, hs) {
                res.groups.push(DupGroup {
                    size,
                    hash: h.to_hex().to_string(),
                    confidence: Confidence::Confirmed,
                    files: g.into_iter().cloned().collect(),
                });
            }
        }
    }
    res.groups.sort_by(|a, b| {
        b.savings()
            .cmp(&a.savings())
            .then_with(|| a.files[0].path.cmp(&b.files[0].path))
    });
    res.errors = errors.into_inner().unwrap_or_else(|p| p.into_inner());
    res.bytes_read = read.load(Ordering::Relaxed);
    on_progress(DupProgress {
        stage: DupStage::Done,
        files_done: total,
        files_total: total,
        bytes_read: res.bytes_read,
    });
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFs;
    use crate::scan::{ScanControl, ScanOptions, ScanTarget, scan_blocking};
    use reclaim_storage_core::path::PathStyle;
    use std::sync::{Arc, RwLock};

    fn content(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add((i % 251) as u8))
            .collect()
    }

    fn fs() -> FakeFs {
        let mut f = FakeFs::new();
        let big = content(1, 200_000);
        let mut big_diff_tail = big.clone();
        *big_diff_tail.last_mut().unwrap() ^= 0xff;
        f.add_content("/d", "big1", big.clone());
        f.add_content("/d/sub", "big2", big.clone());
        f.add_content("/d", "big3-tail-differs", big_diff_tail);
        f.add_content("/d", "small1", content(7, 3000));
        f.add_content("/d", "small2", content(7, 3000));
        f.add_content("/d", "small-other", content(8, 3000));
        f.add_content("/d", "unique", content(9, 5000));
        f.add_content("/d", "empty1", Vec::new());
        f.add_content("/d", "empty2", Vec::new());
        f.hardlink("/d/unique", "/d/unique-link");
        f
    }

    fn files(f: &FakeFs) -> Vec<DupFile> {
        let t = Arc::new(RwLock::new(Tree::new(PathStyle::Posix)));
        scan_blocking(
            f,
            &t,
            ScanTarget::Roots(vec!["/d".into()]),
            &ScanOptions::default(),
            &ScanControl::default(),
        );
        let t = t.read().unwrap();
        files_from_tree(&t, reclaim_storage_core::TOP, 1)
    }

    #[test]
    fn finds_confirmed_groups_and_ignores_hardlinks() {
        let f = fs();
        let input = files(&f);
        // The hard link pair was collapsed by the index.
        assert_eq!(
            input.iter().filter(|x| x.path.contains("unique")).count(),
            1
        );
        for readers in [1, 3] {
            let r = find(
                &f,
                input.clone(),
                &DupOptions {
                    min_size: 1,
                    readers,
                },
                &AtomicBool::new(false),
                &|_| {},
            );
            assert!(!r.cancelled);
            assert_eq!(r.groups.len(), 2, "{:?}", r.groups);
            let big = &r.groups[0];
            assert_eq!(big.confidence, Confidence::Confirmed);
            assert_eq!(big.files.len(), 2);
            assert!(big.files.iter().all(|x| !x.path.contains("tail")));
            assert_eq!(big.hash.len(), 64);
            let small = &r.groups[1];
            assert_eq!(small.files.len(), 2);
            assert_eq!(r.savings(), big.savings() + small.savings());
            assert_eq!(r.candidates, 6); // 3 big + 3 small
            // big files are read twice (prefix and full), everything else once.
            assert_eq!(r.bytes_read, 3 * PREFIX as u64 + 3 * 200_000 + 3 * 3000);
        }
    }

    #[test]
    fn min_size_read_errors_and_cancel() {
        let mut f = fs();
        let input = files(&f);
        let r = find(
            &f,
            input.clone(),
            &DupOptions {
                min_size: 100_000,
                readers: 1,
            },
            &AtomicBool::new(false),
            &|_| {},
        );
        assert_eq!(r.groups.len(), 1);
        f.remove("/d/sub/big2");
        let r = find(
            &f,
            input.clone(),
            &DupOptions {
                min_size: 1,
                readers: 2,
            },
            &AtomicBool::new(false),
            &|_| {},
        );
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.groups.len(), 1); // only the small pair is left
        let r = find(
            &f,
            input,
            &DupOptions::default(),
            &AtomicBool::new(true),
            &|_| {},
        );
        assert!(r.cancelled);
        assert!(r.groups.is_empty());
    }

    #[test]
    fn cancel_during_full_pass_reports_probable() {
        let f = fs();
        let input = files(&f);
        let cancel = AtomicBool::new(false);
        let r = find(
            &f,
            input,
            &DupOptions {
                min_size: 100_000,
                readers: 1,
            },
            &cancel,
            &|p| {
                if p.stage == DupStage::Full
                    || (p.stage == DupStage::Prefix
                        && p.files_done == p.files_total
                        && p.files_total > 0)
                {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        );
        assert!(r.cancelled);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.groups[0].confidence, Confidence::Probable);
        assert_eq!(r.groups[0].files.len(), 3); // tail difference not seen yet
    }
}
