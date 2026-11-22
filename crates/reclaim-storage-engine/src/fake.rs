//! In-memory filesystem for tests of the scanner, cleanup and duplicate finder, with
//! injectable failures (access denied, vanishing directories, slow listings, links,
//! other volumes, locked files). Also used by the app's view-model tests.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::Mutex;
use std::time::Duration;

use reclaim_storage_core::cleanup::{ItemOutcome, SkipReason};
use reclaim_storage_core::entry::{Attrs, EntryMeta, ErrorKind, Kind};
use reclaim_storage_core::guard::Guard;
use reclaim_storage_core::path::PathStyle;
use reclaim_storage_core::ports::{DriveKind, Fs, Remover, Target, VolumeInfo, Volumes};

#[derive(Debug, Clone)]
struct Obj {
    meta: EntryMeta,
    content: Vec<u8>,
}

/// In-memory POSIX-style filesystem.
#[derive(Debug, Default)]
pub struct FakeFs {
    objs: Mutex<BTreeMap<String, Obj>>,
    denied: Vec<String>,
    vanished: Vec<String>,
    locked: Mutex<Vec<String>>,
    delay: Duration,
    next_id: Mutex<u64>,
    bin: Mutex<(u64, u64)>,
    free: Mutex<u64>,
}

fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => "",
    }
}

fn name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

impl FakeFs {
    /// Empty filesystem with a root directory `/`.
    pub fn new() -> Self {
        let f = Self::default();
        *f.free.lock().unwrap_or_else(|p| p.into_inner()) = 1 << 30;
        f.put("/", EntryMeta::dir("/"), Vec::new());
        f
    }

    fn put(&self, path: &str, mut meta: EntryMeta, content: Vec<u8>) {
        let mut id = self.next_id.lock().unwrap_or_else(|p| p.into_inner());
        *id += 1;
        if meta.file_id.is_none() {
            meta.file_id = Some(*id);
        }
        if meta.volume.is_none() {
            meta.volume = Some(1);
        }
        meta.name = name(path).to_owned();
        self.objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.to_owned(), Obj { meta, content });
    }

    /// Creates directories along `path`.
    pub fn mkdir(&mut self, path: &str) {
        let mut cur = String::new();
        for c in path.split('/').filter(|c| !c.is_empty()) {
            cur.push('/');
            cur.push_str(c);
            let exists = self
                .objs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&cur);
            if !exists {
                self.put(&cur, EntryMeta::dir(c), Vec::new());
            }
        }
    }

    /// Adds a file of `size` bytes (content: `size` copies of the first byte of `name`).
    pub fn add_file(&mut self, dir: &str, file: &str, size: u64) {
        let fill = file.as_bytes().first().copied().unwrap_or(b'x');
        self.add_content(dir, file, vec![fill; size as usize]);
    }

    /// Adds a file with given content.
    pub fn add_content(&mut self, dir: &str, file: &str, content: Vec<u8>) {
        self.mkdir(dir);
        let path = if dir == "/" {
            format!("/{file}")
        } else {
            format!("{dir}/{file}")
        };
        self.put(&path, EntryMeta::file(file, content.len() as u64), content);
    }

    /// Sets a file's modification time.
    pub fn set_mtime(&mut self, path: &str, mtime: i64) {
        if let Some(o) = self
            .objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(path)
        {
            o.meta.mtime = mtime;
        }
    }

    /// Adds a second name for an existing file (a hard link).
    pub fn hardlink(&mut self, existing: &str, new_path: &str) {
        let o = self
            .objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(existing)
            .cloned();
        if let Some(mut o) = o {
            o.meta.nlink = Some(2);
            self.objs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(existing.to_owned())
                .and_modify(|e| e.meta.nlink = Some(2));
            o.meta.name = name(new_path).to_owned();
            self.objs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(new_path.to_owned(), o);
        }
    }

    /// Adds a symbolic link (never followed).
    pub fn link(&mut self, path: &str) {
        let mut m = EntryMeta::file(name(path), 0);
        m.kind = Kind::Link;
        m.attrs.set(Attrs::REPARSE);
        self.put(path, m, Vec::new());
    }

    /// Removes a path (and what is below it).
    pub fn remove(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|k, _| k != path && !k.starts_with(&prefix));
    }

    /// Listing `path` fails with access denied.
    pub fn deny(&mut self, path: &str) {
        self.denied.push(path.to_owned());
    }

    /// Listing `path` fails with not found (removed between parent listing and read).
    pub fn vanish(&mut self, path: &str) {
        self.vanished.push(path.to_owned());
    }

    /// Deleting `path` fails with a sharing violation.
    pub fn lock_file(&self, path: &str) {
        self.locked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(path.to_owned());
    }

    /// Marks a directory as another volume.
    pub fn set_volume(&mut self, path: &str, vol: u64) {
        let prefix = format!("{path}/");
        for (k, o) in self
            .objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter_mut()
        {
            if k == path || k.starts_with(&prefix) {
                o.meta.volume = Some(vol);
            }
        }
    }

    /// Every listing sleeps this long.
    pub fn slow(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }

    /// Sets the Recycle Bin contents (items, bytes).
    pub fn set_bin(&self, items: u64, bytes: u64) {
        *self.bin.lock().unwrap_or_else(|p| p.into_inner()) = (items, bytes);
    }

    /// Whether a path exists.
    pub fn exists(&self, path: &str) -> bool {
        self.objs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(path)
    }

    /// `/r/a` (1 B), `/r/z` (20 B), `/r/b/c` (300 B), `/r/b/e/f` (4000 B).
    pub fn sample() -> Self {
        let mut f = Self::new();
        f.add_file("/r", "a", 1);
        f.add_file("/r", "z", 20);
        f.add_file("/r/b", "c", 300);
        f.add_file("/r/b/e", "f", 4000);
        f
    }

    /// `/w/d{i}/f{j}`: `dirs` directories with `files` one-byte files each.
    pub fn wide(dirs: usize, files: usize) -> Self {
        let mut f = Self::new();
        for i in 0..dirs {
            let d = format!("/w/d{i}");
            for j in 0..files {
                f.add_file(&d, &format!("f{j}"), 1);
            }
        }
        f
    }

    /// A chain of `depth` nested directories below `/d`, one 1-byte file in each level.
    pub fn deep(depth: usize) -> Self {
        let mut f = Self::new();
        let mut p = "/d".to_owned();
        f.add_file(&p, "f", 1);
        for _ in 0..depth {
            p.push_str("/l");
            f.add_file(&p, "f", 1);
        }
        f
    }
}

impl Fs for FakeFs {
    fn style(&self) -> PathStyle {
        PathStyle::Posix
    }

    fn stat(&self, path: &str) -> Result<EntryMeta, ErrorKind> {
        let objs = self.objs.lock().unwrap_or_else(|p| p.into_inner());
        let mut m = objs
            .get(path)
            .map(|o| o.meta.clone())
            .ok_or(ErrorKind::NotFound)?;
        m.name = path.to_owned();
        Ok(m)
    }

    fn read_dir(&self, path: &str, out: &mut Vec<EntryMeta>) -> Result<(), ErrorKind> {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        if self.denied.iter().any(|d| d == path) {
            return Err(ErrorKind::AccessDenied);
        }
        if self.vanished.iter().any(|d| d == path) {
            return Err(ErrorKind::NotFound);
        }
        let objs = self.objs.lock().unwrap_or_else(|p| p.into_inner());
        match objs.get(path) {
            Some(o) if o.meta.kind == Kind::Dir => {}
            Some(_) => return Err(ErrorKind::Os(20)),
            None => return Err(ErrorKind::NotFound),
        }
        for (k, o) in objs.iter() {
            if k != path && parent(k) == path {
                out.push(o.meta.clone());
            }
        }
        Ok(())
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, ErrorKind> {
        let objs = self.objs.lock().unwrap_or_else(|p| p.into_inner());
        match objs.get(path) {
            Some(o) if o.meta.kind == Kind::File => Ok(Box::new(Cursor::new(o.content.clone()))),
            Some(_) => Err(ErrorKind::Os(21)),
            None => Err(ErrorKind::NotFound),
        }
    }
}

impl Volumes for FakeFs {
    fn list(&self) -> Vec<VolumeInfo> {
        let free = *self.free.lock().unwrap_or_else(|p| p.into_inner());
        vec![VolumeInfo {
            root: "/".into(),
            label: "fake".into(),
            fs: "memfs".into(),
            kind: DriveKind::Fixed,
            total: 2 << 30,
            free,
            seek_penalty: Some(false),
        }]
    }

    fn volume_of(&self, _path: &str) -> Option<VolumeInfo> {
        self.list().into_iter().next()
    }
}

impl FakeFs {
    fn check(&self, t: &Target, guard: &Guard) -> Result<EntryMeta, ItemOutcome> {
        let m = self
            .stat(&t.path)
            .map_err(|_| ItemOutcome::Skipped(SkipReason::Missing))?;
        if m.kind == Kind::Link {
            return Err(ItemOutcome::Skipped(SkipReason::Link));
        }
        // Links on the path: any ancestor that is a link means the path is redirected.
        let mut p = parent(&t.path);
        while p != "/" && !p.is_empty() {
            if self.stat(p).is_ok_and(|a| a.kind == Kind::Link) {
                return Err(ItemOutcome::Skipped(SkipReason::Link));
            }
            p = parent(p);
        }
        if t.file_id.is_some() && t.file_id != m.file_id {
            return Err(ItemOutcome::Skipped(SkipReason::Changed));
        }
        if t.mtime.is_some_and(|mt| mt != m.mtime) {
            return Err(ItemOutcome::Skipped(SkipReason::Changed));
        }
        t.check(guard, &t.path)
            .map_err(|d| ItemOutcome::Skipped(SkipReason::from_denied(&d)))?;
        if self
            .locked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&t.path)
        {
            return Err(ItemOutcome::Skipped(SkipReason::InUse));
        }
        Ok(m)
    }

    fn take(&self, path: &str) -> u64 {
        let mut objs = self.objs.lock().unwrap_or_else(|p| p.into_inner());
        let prefix = format!("{path}/");
        let mut bytes = 0;
        objs.retain(|k, o| {
            let gone = k == path || k.starts_with(&prefix);
            if gone {
                bytes += o.meta.allocated.unwrap_or(0);
            }
            !gone
        });
        *self.free.lock().unwrap_or_else(|p| p.into_inner()) += bytes;
        bytes
    }
}

impl Remover for FakeFs {
    fn delete(&self, t: &Target, guard: &Guard) -> ItemOutcome {
        match self.check(t, guard) {
            Err(o) => o,
            Ok(m) if m.kind == Kind::Dir => ItemOutcome::Skipped(SkipReason::Changed),
            Ok(_) => ItemOutcome::Deleted(self.take(&t.path)),
        }
    }

    fn remove_empty_dir(&self, path: &str, root: &str, guard: &Guard) -> bool {
        if guard.check_cleanup(path, root).is_err() {
            return false;
        }
        let mut v = Vec::new();
        if self.read_dir(path, &mut v).is_err() || !v.is_empty() {
            return false;
        }
        self.take(path);
        true
    }

    fn recycle(&self, t: &Target, guard: &Guard) -> ItemOutcome {
        match self.check(t, guard) {
            Err(o) => o,
            Ok(_) => {
                let b = self.take(&t.path);
                // Recycling moves data, it does not free it.
                *self.free.lock().unwrap_or_else(|p| p.into_inner()) -= b;
                let mut bin = self.bin.lock().unwrap_or_else(|p| p.into_inner());
                bin.0 += 1;
                bin.1 += b;
                ItemOutcome::Recycled(b)
            }
        }
    }

    fn recycle_bin(&self) -> Option<(u64, u64)> {
        Some(*self.bin.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn empty_recycle_bin(&self) -> Result<(), ErrorKind> {
        let mut bin = self.bin.lock().unwrap_or_else(|p| p.into_inner());
        *self.free.lock().unwrap_or_else(|p| p.into_inner()) += bin.1;
        *bin = (0, 0);
        Ok(())
    }

    fn open_system_cleanup(&self) -> Result<(), ErrorKind> {
        Err(ErrorKind::Os(-1))
    }
}
