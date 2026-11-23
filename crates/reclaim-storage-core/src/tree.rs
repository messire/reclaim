//! The scan index: a compact arena of nodes with sizes aggregated bottom-up.
//!
//! Layout (memory, ARCHITECTURE-STORAGE §4):
//! - one `Node` per file or directory (56 bytes), names in one shared `String` arena;
//! - the children of a directory occupy one contiguous id range, because a directory's
//!   listing is inserted in one call ([`Tree::fill_dir`]);
//! - directory counters live in a side table, so files pay nothing for them.
//!
//! Aggregates are maintained incrementally: inserting a directory listing adds its
//! totals to every ancestor once (O(depth) per directory, not per file). A partially
//! scanned tree is therefore always consistent, which is what makes cancellation,
//! pause and partial results cheap and honest. Refreshing a directory subtracts its old
//! totals and inserts a new listing; old nodes are marked dead and skipped.

use std::collections::HashMap;

use crate::entry::{Attrs, EntryMeta, ErrorKind, Kind, ScanError};
use crate::path::PathStyle;

/// Index of a node.
pub type NodeId = u32;
/// "No node".
pub const NONE: NodeId = u32::MAX;
/// The synthetic top node whose children are the scan roots.
pub const TOP: NodeId = 0;
/// At most this many errors are kept with their paths; the rest are only counted.
pub const MAX_ERRORS: usize = 10_000;

/// Which size a view or query uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeBasis {
    /// Bytes on disk (clusters allocated). Default: this is what occupies the disk.
    #[default]
    Allocated,
    /// File length (end of file).
    Logical,
}

/// One file or directory.
#[derive(Debug, Clone, Copy)]
pub struct Node {
    /// Parent (`NONE` for the top node).
    pub parent: NodeId,
    first_child: NodeId,
    child_count: u32,
    name_off: u32,
    name_len: u16,
    /// Kind.
    pub kind: Kind,
    /// Attributes and index flags.
    pub attrs: Attrs,
    /// File: extension id. Directory: index into the counter table.
    aux: u32,
    /// Logical bytes (directory: sum over the subtree, hard links counted once).
    pub logical: u64,
    /// Allocated bytes (directory: sum over the subtree, hard links counted once).
    pub allocated: u64,
    /// Last write time, Unix seconds, stored unsigned in 32 bits (1970–2106; earlier
    /// times read as 0, "unknown"). Kept small: one record per file on the drive.
    mtime_s: u32,
}

impl Node {
    /// Last write time, Unix seconds (0 when unknown or before 1970).
    pub fn mtime(&self) -> i64 {
        i64::from(self.mtime_s)
    }

    /// Size under a basis.
    pub fn size(&self, basis: SizeBasis) -> u64 {
        match basis {
            SizeBasis::Allocated => self.allocated,
            SizeBasis::Logical => self.logical,
        }
    }

    /// Whether the node is still part of the index.
    pub fn live(&self) -> bool {
        !self.attrs.has(Attrs::DEAD)
    }

    /// Whether this is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
}

/// Counters of a directory's subtree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirCounts {
    /// Files below (any depth), links and other objects included.
    pub files: u64,
    /// Directories below (any depth).
    pub dirs: u64,
    /// Directories below that could not be read.
    pub errors: u64,
}

/// Stored form of [`DirCounts`]: 32-bit counters (4 billion entries per subtree).
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    files: u32,
    dirs: u32,
    errors: u32,
}

#[derive(Debug, Clone, Copy)]
struct LinkKey {
    volume: u64,
    id: u64,
    node: NodeId,
}

/// Interned lower-case file extensions; id 0 is "no extension".
#[derive(Debug, Default, Clone)]
pub struct Extensions {
    names: Vec<String>,
    ids: HashMap<String, u32>,
}

impl Extensions {
    fn new() -> Self {
        Self {
            names: vec![String::new()],
            ids: HashMap::new(),
        }
    }

    fn intern(&mut self, ext: &str) -> u32 {
        if ext.is_empty() {
            return 0;
        }
        if let Some(&i) = self.ids.get(ext) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(ext.to_owned());
        self.ids.insert(ext.to_owned(), i);
        i
    }

    /// Extension text of an id (empty for none).
    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", String::as_str)
    }

    /// Id of an extension (lower case, without the dot), if present.
    pub fn id(&self, ext: &str) -> Option<u32> {
        if ext.is_empty() {
            Some(0)
        } else {
            self.ids.get(ext).copied()
        }
    }

    /// Number of distinct extensions, "none" included.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether only "none" exists.
    pub fn is_empty(&self) -> bool {
        self.names.len() <= 1
    }
}

/// Extension of a file name for statistics: the text after the last dot, lower case,
/// at most 16 characters, not for dot files like `.gitignore` (those have none).
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => {
            let e = &name[i + 1..];
            if e.is_empty() || e.chars().count() > 16 || e.contains(' ') {
                String::new()
            } else {
                e.to_lowercase()
            }
        }
    }
}

/// The scan index.
#[derive(Debug, Clone)]
pub struct Tree {
    style: PathStyle,
    nodes: Vec<Node>,
    names: String,
    counts: Vec<Counts>,
    exts: Extensions,
    links: Vec<LinkKey>,
    link_dups: Vec<NodeId>,
    errors: Vec<ScanError>,
    errors_dropped: u64,
    dead: u64,
    allocated_estimated: bool,
}

impl Tree {
    /// An empty index with only the top node.
    pub fn new(style: PathStyle) -> Self {
        let mut t = Self {
            style,
            nodes: Vec::new(),
            names: String::new(),
            counts: Vec::new(),
            exts: Extensions::new(),
            links: Vec::new(),
            link_dups: Vec::new(),
            errors: Vec::new(),
            errors_dropped: 0,
            dead: 0,
            allocated_estimated: false,
        };
        t.counts.push(Counts::default());
        t.nodes.push(Node {
            parent: NONE,
            first_child: NONE,
            child_count: 0,
            name_off: 0,
            name_len: 0,
            kind: Kind::Dir,
            attrs: Attrs::default(),
            aux: 0,
            logical: 0,
            allocated: 0,
            mtime_s: 0,
        });
        t
    }

    /// Path style of the scanned paths.
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// Number of node slots (dead ones included). Valid ids are `0..len()`.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether nothing but the top node exists.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// Live nodes, the top node excluded.
    pub fn live_count(&self) -> u64 {
        self.nodes.len() as u64 - 1 - self.dead
    }

    /// Gives back the spare capacity the arrays grew into during a scan (up to half of
    /// them). Call when a scan or refresh has finished; later inserts grow them again.
    pub fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
        self.names.shrink_to_fit();
        self.counts.shrink_to_fit();
        self.links.shrink_to_fit();
        self.link_dups.shrink_to_fit();
        self.errors.shrink_to_fit();
    }

    /// Heap use split as (node records, names, directory counters, hard-link table) in
    /// bytes, by capacity (benchmarks).
    pub fn memory_parts(&self) -> (usize, usize, usize, usize) {
        (
            self.nodes.capacity() * std::mem::size_of::<Node>(),
            self.names.capacity(),
            self.counts.capacity() * std::mem::size_of::<Counts>(),
            self.links.capacity() * std::mem::size_of::<LinkKey>() + self.link_dups.capacity() * 4,
        )
    }

    /// Approximate heap use of the index in bytes (benchmarks, About).
    pub fn memory_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.names.capacity()
            + self.counts.capacity() * std::mem::size_of::<Counts>()
            + self.links.capacity() * std::mem::size_of::<LinkKey>()
            + self.link_dups.capacity() * 4
    }

    /// The node.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    /// Node by id if it exists and is live.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize).filter(|n| n.live())
    }

    /// The name of a node (roots: the full path as given).
    pub fn name(&self, id: NodeId) -> &str {
        let n = self.node(id);
        &self.names[n.name_off as usize..n.name_off as usize + usize::from(n.name_len)]
    }

    /// Interned extensions.
    pub fn extensions(&self) -> &Extensions {
        &self.exts
    }

    /// Extension id of a file (0 for directories and files without one).
    pub fn ext_id(&self, id: NodeId) -> u32 {
        let n = self.node(id);
        if n.kind == Kind::Dir { 0 } else { n.aux }
    }

    /// Subtree counters of a directory (zero for files).
    pub fn counts(&self, id: NodeId) -> DirCounts {
        let n = self.node(id);
        if n.kind == Kind::Dir {
            let c = self.counts[n.aux as usize];
            DirCounts {
                files: u64::from(c.files),
                dirs: u64::from(c.dirs),
                errors: u64::from(c.errors),
            }
        } else {
            DirCounts::default()
        }
    }

    /// Scan roots (live children of the top node).
    pub fn roots(&self) -> Vec<NodeId> {
        self.children(TOP).collect()
    }

    /// Live children of a node, in insertion order.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let n = self.node(id);
        let (first, count) = if n.first_child == NONE {
            (0, 0)
        } else {
            (n.first_child, n.child_count)
        };
        (first..first + count).filter(|&c| self.nodes[c as usize].live())
    }

    /// Number of live children.
    pub fn child_count(&self, id: NodeId) -> usize {
        self.children(id).count()
    }

    /// Ancestors from the parent up to (excluding) the top node.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.node(id).parent;
        std::iter::from_fn(move || {
            if cur == NONE || cur == TOP {
                None
            } else {
                let c = cur;
                cur = self.node(c).parent;
                Some(c)
            }
        })
    }

    /// Whether `id` is `ancestor` or lies below it.
    pub fn is_within(&self, id: NodeId, ancestor: NodeId) -> bool {
        id == ancestor || ancestor == TOP || self.ancestors(id).any(|a| a == ancestor)
    }

    /// The root this node belongs to.
    pub fn root_of(&self, id: NodeId) -> NodeId {
        if id == TOP {
            return TOP;
        }
        let mut cur = id;
        while self.node(cur).parent != TOP && self.node(cur).parent != NONE {
            cur = self.node(cur).parent;
        }
        cur
    }

    /// Depth below the top node (roots are 1).
    pub fn depth(&self, id: NodeId) -> usize {
        if id == TOP {
            0
        } else {
            1 + self.ancestors(id).count()
        }
    }

    /// Full display path of a node, built from the root path and the names below it.
    pub fn path(&self, id: NodeId) -> String {
        if id == TOP {
            return String::new();
        }
        let mut parts: Vec<&str> = vec![self.name(id)];
        parts.extend(self.ancestors(id).map(|a| self.name(a)));
        let sep = self.style.sep();
        let mut s = String::new();
        for (i, p) in parts.iter().rev().enumerate() {
            if i > 0 && !s.ends_with(sep) && !s.ends_with('/') {
                s.push(sep);
            }
            s.push_str(p);
        }
        s
    }

    /// Finds a live node by display path (roots compared by their path, components by
    /// name; case-insensitive on Windows).
    pub fn find(&self, path: &str) -> Option<NodeId> {
        let fold = |s: &str| -> String {
            let s = s.trim_end_matches(['\\', '/']);
            if self.style == PathStyle::Windows {
                s.replace('/', "\\").to_lowercase()
            } else {
                s.to_owned()
            }
        };
        let want = fold(path);
        let sep = self.style.sep();
        for r in self.roots() {
            let rp = fold(self.name(r));
            if want == rp {
                return Some(r);
            }
            let Some(rest) = want.strip_prefix(&rp) else {
                continue;
            };
            let Some(rest) =
                rest.strip_prefix(sep)
                    .or(if rp.is_empty() { Some(rest) } else { None })
            else {
                continue;
            };
            let mut cur = r;
            let mut ok = true;
            for comp in rest.split(sep).filter(|c| !c.is_empty()) {
                match self.children(cur).find(|&c| fold(self.name(c)) == comp) {
                    Some(c) => cur = c,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return Some(cur);
            }
        }
        None
    }

    /// Kept scan errors.
    pub fn errors(&self) -> &[ScanError] {
        &self.errors
    }

    /// Errors not kept because of [`MAX_ERRORS`].
    pub fn errors_dropped(&self) -> u64 {
        self.errors_dropped
    }

    /// Whether at least one allocated size was estimated from the logical size because
    /// the adapter could not report it.
    pub fn allocated_estimated(&self) -> bool {
        self.allocated_estimated
    }

    /// Records an error (bounded).
    pub fn record_error(&mut self, e: ScanError) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(e);
        } else {
            self.errors_dropped += 1;
        }
    }

    fn push_name(&mut self, name: &str) -> (u32, u16) {
        // File names are at most 255 UTF-16 units (≤ 765 bytes); only a pathological root
        // path could be longer, and it is cut at a character boundary.
        let mut end = name.len().min(usize::from(u16::MAX));
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let off = self.names.len() as u32;
        self.names.push_str(&name[..end]);
        (off, end as u16)
    }

    fn new_node(&mut self, parent: NodeId, e: &EntryMeta) -> Node {
        let (name_off, name_len) = self.push_name(&e.name);
        let aux = if e.kind == Kind::Dir {
            self.counts.push(Counts::default());
            (self.counts.len() - 1) as u32
        } else {
            self.exts.intern(&extension_of(&e.name))
        };
        let mut attrs = e.attrs;
        attrs.clear(Attrs::HARDLINK | Attrs::HARDLINK_DUP | Attrs::DEAD);
        if e.nlink.is_some_and(|n| n > 1) {
            attrs.set(Attrs::HARDLINK);
        }
        let (logical, allocated) = match e.kind {
            Kind::File => {
                let a = match e.allocated {
                    Some(a) => a,
                    None => {
                        self.allocated_estimated = true;
                        e.logical
                    }
                };
                (e.logical, a)
            }
            // Links and other objects occupy (almost) nothing; their targets are not ours.
            Kind::Link | Kind::Other => (0, e.allocated.unwrap_or(0)),
            Kind::Dir => (0, 0),
        };
        Node {
            parent,
            first_child: NONE,
            child_count: 0,
            name_off,
            name_len,
            kind: e.kind,
            attrs,
            aux,
            logical,
            allocated,
            mtime_s: u32::try_from(e.mtime.max(0)).unwrap_or(u32::MAX),
        }
    }

    /// Sets the scan roots (once, before any listing). Each root's name is its full
    /// display path. Returns their ids in order.
    pub fn set_roots(&mut self, roots: &[EntryMeta]) -> Vec<NodeId> {
        assert!(
            self.nodes[0].first_child == NONE,
            "roots can only be set once"
        );
        self.insert_children(TOP, roots)
    }

    /// Inserts the listing of a directory that has no children yet and adds the totals
    /// to every ancestor. Returns the ids of the inserted entries in order.
    pub fn fill_dir(&mut self, dir: NodeId, entries: &[EntryMeta]) -> Vec<NodeId> {
        debug_assert!(self.node(dir).is_dir());
        debug_assert!(self.node(dir).child_count == 0, "directory already filled");
        self.insert_children(dir, entries)
    }

    fn insert_children(&mut self, dir: NodeId, entries: &[EntryMeta]) -> Vec<NodeId> {
        let first = self.nodes.len() as NodeId;
        let mut sum_l = 0u64;
        let mut sum_a = 0u64;
        let mut files = 0u64;
        let mut dirs = 0u64;
        let mut ids = Vec::with_capacity(entries.len());
        for e in entries {
            let n = self.new_node(dir, e);
            sum_l += n.logical;
            sum_a += n.allocated;
            if n.kind == Kind::Dir {
                dirs += 1;
            } else {
                files += 1;
            }
            let id = self.nodes.len() as NodeId;
            if n.kind == Kind::File
                && let Some(fid) = e.file_id
                && e.nlink.is_none_or(|l| l > 1)
            {
                self.links.push(LinkKey {
                    volume: e.volume.unwrap_or(0),
                    id: fid,
                    node: id,
                });
            }
            self.nodes.push(n);
            ids.push(id);
        }
        let d = &mut self.nodes[dir as usize];
        d.first_child = first;
        d.child_count = entries.len() as u32;
        self.add_up(
            dir,
            sum_l as i128,
            sum_a as i128,
            files as i64,
            dirs as i64,
            0,
            true,
        );
        ids
    }

    /// Adds deltas to `from` (when `inclusive`) and every ancestor including the top.
    #[allow(clippy::too_many_arguments)] // One delta per aggregate; a struct adds nothing.
    fn add_up(
        &mut self,
        from: NodeId,
        dl: i128,
        da: i128,
        df: i64,
        dd: i64,
        de: i64,
        inclusive: bool,
    ) {
        let mut cur = if inclusive {
            from
        } else {
            self.nodes[from as usize].parent
        };
        while cur != NONE {
            let n = &mut self.nodes[cur as usize];
            n.logical = (i128::from(n.logical) + dl).max(0) as u64;
            n.allocated = (i128::from(n.allocated) + da).max(0) as u64;
            let aux = n.aux as usize;
            let parent = n.parent;
            let c = &mut self.counts[aux];
            c.files = (i64::from(c.files) + df).clamp(0, i64::from(u32::MAX)) as u32;
            c.dirs = (i64::from(c.dirs) + dd).clamp(0, i64::from(u32::MAX)) as u32;
            c.errors = (i64::from(c.errors) + de).clamp(0, i64::from(u32::MAX)) as u32;
            cur = parent;
        }
    }

    /// Marks a directory as unreadable (its size is unknown) and records the error.
    pub fn mark_error(&mut self, dir: NodeId, kind: ErrorKind) {
        let path = self.path(dir);
        self.record_error(ScanError { path, kind });
        let n = &mut self.nodes[dir as usize];
        if !n.attrs.has(Attrs::ERROR) {
            n.attrs.set(Attrs::ERROR);
            if n.kind == Kind::Dir {
                self.add_up(dir, 0, 0, 0, 0, 1, false);
            }
        }
    }

    /// Sets an index flag on a node (`INCOMPLETE`, `EXCLUDED`, `OTHER_VOLUME`).
    pub fn mark(&mut self, id: NodeId, flag: u16) {
        self.nodes[id as usize].attrs.set(flag);
    }

    /// Whether any live directory is still marked incomplete or unreadable.
    pub fn has_gaps(&self) -> bool {
        self.nodes
            .iter()
            .any(|n| n.live() && n.attrs.0 & (Attrs::INCOMPLETE | Attrs::ERROR) != 0)
    }

    /// Removes a node (and its subtree) from the totals, e.g. after it was deleted.
    pub fn remove(&mut self, id: NodeId) {
        if id == TOP || !self.node(id).live() {
            return;
        }
        let n = *self.node(id);
        let counted = !n.attrs.has(Attrs::HARDLINK_DUP);
        let (dl, da) = if counted || n.is_dir() {
            (n.logical, n.allocated)
        } else {
            (0, 0)
        };
        let c = self.counts(id);
        let (df, dd) = if n.is_dir() {
            (c.files as i64, c.dirs as i64 + 1)
        } else {
            (1, 0)
        };
        let de = c.errors as i64 + i64::from(n.attrs.has(Attrs::ERROR) && n.is_dir());
        self.add_up(id, -(dl as i128), -(da as i128), -df, -dd, -de, false);
        self.kill_subtree(id, true);
    }

    fn kill_subtree(&mut self, id: NodeId, include_self: bool) {
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            let n = self.nodes[cur as usize];
            if n.first_child != NONE {
                stack.extend(n.first_child..n.first_child + n.child_count);
            }
            if (cur != id || include_self) && n.live() {
                self.nodes[cur as usize].attrs.set(Attrs::DEAD);
                self.dead += 1;
            }
        }
    }

    /// Drops a directory's listing so it can be filled again (refresh). Its old totals
    /// are subtracted from it and its ancestors; the old children become dead.
    pub fn clear_dir(&mut self, dir: NodeId) {
        let n = *self.node(dir);
        if !n.is_dir() {
            return;
        }
        let c = self.counts(dir);
        let had_error = n.attrs.has(Attrs::ERROR);
        self.add_up(
            dir,
            -(n.logical as i128),
            -(n.allocated as i128),
            -(c.files as i64),
            -(c.dirs as i64),
            -(c.errors as i64),
            true,
        );
        if had_error {
            self.add_up(dir, 0, 0, 0, 0, -1, false);
        }
        self.kill_subtree(dir, false);
        let d = &mut self.nodes[dir as usize];
        d.first_child = NONE;
        d.child_count = 0;
        d.attrs
            .clear(Attrs::ERROR | Attrs::INCOMPLETE | Attrs::EXCLUDED | Attrs::OTHER_VOLUME);
    }

    /// Counts every hard-linked file once: of the scanned links to one file identity,
    /// the first in scan order keeps its size in the totals, the others are flagged
    /// `HARDLINK_DUP` and subtracted. Idempotent; call after a scan or refresh.
    ///
    /// Only identities seen more than once are kept for later passes. Windows listings
    /// do not report link counts, so every file is a candidate while scanning; keeping
    /// all of them would cost 24 bytes per file for as long as the index lives. The
    /// price: a new link that a refresh finds to a file seen only once before is
    /// counted twice until the next full scan.
    pub fn resolve_hardlinks(&mut self) {
        // Undo the previous pass (dead nodes were already subtracted with their parent).
        let previous = std::mem::take(&mut self.link_dups);
        for id in previous {
            let n = self.nodes[id as usize];
            if n.live() && n.attrs.has(Attrs::HARDLINK_DUP) {
                self.add_up(id, n.logical as i128, n.allocated as i128, 0, 0, 0, false);
                self.nodes[id as usize].attrs.clear(Attrs::HARDLINK_DUP);
            }
        }
        let nodes = &self.nodes;
        self.links.retain(|k| nodes[k.node as usize].live());
        self.links
            .sort_unstable_by_key(|k| (k.volume, k.id, k.node));
        let mut i = 0;
        let mut dups = Vec::new();
        let mut grouped = Vec::new();
        let mut kept = Vec::new();
        while i < self.links.len() {
            let mut j = i + 1;
            while j < self.links.len()
                && self.links[j].volume == self.links[i].volume
                && self.links[j].id == self.links[i].id
            {
                j += 1;
            }
            if j - i > 1 {
                for k in &self.links[i..j] {
                    grouped.push(k.node);
                }
                kept.extend_from_slice(&self.links[i..j]);
                for k in &self.links[i + 1..j] {
                    dups.push(k.node);
                }
            }
            i = j;
        }
        kept.shrink_to_fit();
        self.links = kept;
        for id in grouped {
            self.nodes[id as usize].attrs.set(Attrs::HARDLINK);
        }
        for &id in &dups {
            let n = self.nodes[id as usize];
            self.nodes[id as usize].attrs.set(Attrs::HARDLINK_DUP);
            self.add_up(
                id,
                -(n.logical as i128),
                -(n.allocated as i128),
                0,
                0,
                0,
                false,
            );
        }
        dups.shrink_to_fit();
        self.link_dups = dups;
    }

    /// Links that were counted once (secondary names), after [`Tree::resolve_hardlinks`].
    pub fn hardlink_duplicates(&self) -> usize {
        self.link_dups.len()
    }

    /// All live node ids below `from` (inclusive), depth first.
    pub fn descendants(&self, from: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![from];
        while let Some(cur) = stack.pop() {
            if !self.node(cur).live() {
                continue;
            }
            out.push(cur);
            let n = self.node(cur);
            if n.first_child != NONE {
                for c in (n.first_child..n.first_child + n.child_count).rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    /// Visits every live node below `from` (inclusive) with its full path, depth first,
    /// building paths incrementally (one allocation per directory level, not per node).
    pub fn walk_paths(&self, from: NodeId, mut f: impl FnMut(NodeId, &str) -> bool) {
        let sep = self.style.sep();
        let start = self.path(from);
        // (node, path length to truncate to before appending this node's name)
        let mut stack: Vec<(NodeId, usize)> = Vec::new();
        let mut path = String::new();
        if from == TOP {
            for r in self.roots().into_iter().rev() {
                stack.push((r, 0));
            }
        } else {
            path.push_str(&start);
            if !f(from, &path) {
                return;
            }
            self.push_children(from, path.len(), &mut stack);
        }
        while let Some((id, base)) = stack.pop() {
            path.truncate(base);
            if self.node(id).parent == TOP {
                path.push_str(self.name(id));
            } else {
                if !path.ends_with(sep) && !path.ends_with('/') {
                    path.push(sep);
                }
                path.push_str(self.name(id));
            }
            if !f(id, &path) {
                return;
            }
            if self.node(id).is_dir() {
                self.push_children(id, path.len(), &mut stack);
            }
        }
    }

    fn push_children(&self, id: NodeId, base: usize, stack: &mut Vec<(NodeId, usize)>) {
        let n = self.node(id);
        if n.first_child == NONE {
            return;
        }
        for c in (n.first_child..n.first_child + n.child_count).rev() {
            if self.nodes[c as usize].live() {
                stack.push((c, base));
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> (Tree, NodeId) {
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/data")])[0];
        let ids = t.fill_dir(
            r,
            &[
                EntryMeta::file("a.txt", 100),
                EntryMeta::dir("sub"),
                EntryMeta::file("b.BIN", 5000),
            ],
        );
        t.fill_dir(
            ids[1],
            &[EntryMeta::file("c.txt", 10), EntryMeta::file(".hidden", 1)],
        );
        (t, r)
    }

    #[test]
    fn aggregation_and_counts() {
        let (t, r) = sample();
        assert_eq!(t.node(r).logical, 5111);
        assert_eq!(t.node(r).allocated, 4096 * 5);
        assert_eq!(t.node(TOP).logical, 5111);
        let c = t.counts(r);
        assert_eq!((c.files, c.dirs, c.errors), (4, 1, 0));
        assert_eq!(t.counts(TOP).dirs, 2);
        assert_eq!(t.child_count(r), 3);
        assert_eq!(t.live_count(), 6);
        assert!(t.memory_bytes() > 0);
        assert_eq!(t.counts(1 + 1), DirCounts::default()); // a file
    }

    #[test]
    fn paths_and_find() {
        let (t, r) = sample();
        let sub = t.children(r).nth(1).unwrap();
        let c = t.children(sub).next().unwrap();
        assert_eq!(t.path(c), "/data/sub/c.txt");
        assert_eq!(t.find("/data/sub/c.txt"), Some(c));
        assert_eq!(t.find("/data/"), Some(r));
        assert_eq!(t.find("/data/sub/C.txt"), None);
        assert_eq!(t.find("/other"), None);
        assert_eq!(t.root_of(c), r);
        assert_eq!(t.depth(c), 3);
        assert!(t.is_within(c, r));
        assert!(!t.is_within(r, c));
        let mut seen = Vec::new();
        t.walk_paths(TOP, |_, p| {
            seen.push(p.to_owned());
            true
        });
        assert_eq!(
            seen,
            [
                "/data",
                "/data/a.txt",
                "/data/sub",
                "/data/sub/c.txt",
                "/data/sub/.hidden",
                "/data/b.BIN"
            ]
        );
        let mut sub_seen = Vec::new();
        t.walk_paths(sub, |_, p| {
            sub_seen.push(p.to_owned());
            true
        });
        assert_eq!(
            sub_seen,
            ["/data/sub", "/data/sub/c.txt", "/data/sub/.hidden"]
        );
        assert_eq!(t.descendants(sub).len(), 3);
    }

    #[test]
    fn windows_paths() {
        let mut t = Tree::new(PathStyle::Windows);
        let r = t.set_roots(&[EntryMeta::dir("C:\\")])[0];
        let d = t.fill_dir(r, &[EntryMeta::dir("Users")])[0];
        let f = t.fill_dir(d, &[EntryMeta::file("X.TXT", 3)])[0];
        assert_eq!(t.path(f), "C:\\Users\\X.TXT");
        assert_eq!(t.find("c:\\users\\x.txt"), Some(f));
        assert_eq!(t.find("C:/Users"), Some(d));
        assert_eq!(t.find("C:\\"), Some(r));
        assert_eq!(t.extensions().name(t.ext_id(f)), "txt");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_of("a.TXT"), "txt");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("Makefile"), "");
        assert_eq!(extension_of("a."), "");
        assert_eq!(extension_of("a.b.tar.GZ"), "gz");
        assert_eq!(extension_of("x.this is not ext"), "");
        assert_eq!(extension_of("x.abcdefghijklmnopq"), "");
        let (t, _) = sample();
        assert_eq!(t.extensions().id("txt"), Some(1));
        assert_eq!(t.extensions().id(""), Some(0));
        assert_eq!(t.extensions().id("zip"), None);
        assert_eq!(t.extensions().len(), 3);
        assert!(!t.extensions().is_empty());
    }

    #[test]
    fn remove_and_refresh() {
        let (mut t, r) = sample();
        let sub = t.children(r).nth(1).unwrap();
        t.remove(sub);
        assert_eq!(t.node(r).logical, 5100);
        assert_eq!(t.counts(r).files, 2);
        assert_eq!(t.counts(r).dirs, 0);
        assert_eq!(t.child_count(r), 2);
        assert_eq!(t.live_count(), 3);
        // Removing twice is a no-op.
        t.remove(sub);
        assert_eq!(t.node(r).logical, 5100);

        let a = t.children(r).next().unwrap();
        t.remove(a);
        assert_eq!(t.node(r).logical, 5000);
        assert_eq!(t.counts(TOP).files, 1);

        t.clear_dir(r);
        assert_eq!(t.node(r).logical, 0);
        assert_eq!(t.node(TOP).logical, 0);
        assert_eq!(t.counts(r), DirCounts::default());
        t.fill_dir(r, &[EntryMeta::file("new", 7)]);
        assert_eq!(t.node(TOP).logical, 7);
        assert_eq!(t.counts(TOP).files, 1);
    }

    #[test]
    fn errors_and_flags() {
        let (mut t, r) = sample();
        let sub = t.children(r).nth(1).unwrap();
        t.mark_error(sub, ErrorKind::AccessDenied);
        t.mark_error(sub, ErrorKind::AccessDenied);
        assert_eq!(t.counts(r).errors, 1);
        assert_eq!(t.errors().len(), 2);
        assert!(t.has_gaps());
        t.clear_dir(sub);
        assert_eq!(t.counts(r).errors, 0);
        assert!(!t.has_gaps());
        t.mark(sub, Attrs::INCOMPLETE);
        assert!(t.has_gaps());
        for _ in 0..MAX_ERRORS + 3 {
            t.record_error(ScanError {
                path: "/x".into(),
                kind: ErrorKind::NotFound,
            });
        }
        assert_eq!(t.errors().len(), MAX_ERRORS);
        assert!(t.errors_dropped() >= 3);
    }

    #[test]
    fn hardlinks_counted_once() {
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/v")])[0];
        let link = |name: &str| EntryMeta {
            file_id: Some(42),
            nlink: Some(2),
            volume: Some(1),
            ..EntryMeta::file(name, 8192)
        };
        let d = t.fill_dir(r, &[link("one"), EntryMeta::dir("d")]);
        t.fill_dir(d[1], &[link("two"), EntryMeta::file("solo", 1)]);
        assert_eq!(t.node(r).logical, 8192 * 2 + 1);
        t.resolve_hardlinks();
        assert_eq!(t.node(r).logical, 8192 + 1);
        assert_eq!(t.node(d[1]).logical, 1);
        assert_eq!(t.hardlink_duplicates(), 1);
        let two = t.children(d[1]).next().unwrap();
        assert!(t.node(two).attrs.has(Attrs::HARDLINK_DUP));
        assert!(t.node(d[0]).attrs.has(Attrs::HARDLINK));
        // Idempotent.
        t.resolve_hardlinks();
        assert_eq!(t.node(r).logical, 8192 + 1);
        // Removing the counted link makes the other one count.
        t.remove(d[0]);
        t.resolve_hardlinks();
        assert_eq!(t.node(r).logical, 8192 + 1);
        assert!(!t.node(two).attrs.has(Attrs::HARDLINK_DUP));
        assert_eq!(t.hardlink_duplicates(), 0);
        // Removing a duplicate does not subtract twice.
        let mut t2 = Tree::new(PathStyle::Posix);
        let r2 = t2.set_roots(&[EntryMeta::dir("/v")])[0];
        let ids = t2.fill_dir(r2, &[link("a"), link("b")]);
        t2.resolve_hardlinks();
        assert_eq!(t2.node(r2).logical, 8192);
        t2.remove(ids[1]);
        assert_eq!(t2.node(r2).logical, 8192);
    }

    #[test]
    fn identities_seen_once_are_not_kept() {
        // Windows listings give a file id but no link count: every file is a candidate.
        let mut t = Tree::new(PathStyle::Windows);
        let r = t.set_roots(&[EntryMeta::dir("C:\\")])[0];
        let file = |name: &str, id: u64| EntryMeta {
            file_id: Some(id),
            nlink: None,
            volume: Some(1),
            ..EntryMeta::file(name, 100)
        };
        let ids = t.fill_dir(
            r,
            &[
                file("a", 1),
                file("b", 2),
                file("c", 3),
                file("a2", 1),
                EntryMeta::dir("d"),
            ],
        );
        let (_, _, _, before) = t.memory_parts();
        assert!(before >= 4 * std::mem::size_of::<LinkKey>());
        t.resolve_hardlinks();
        let (_, _, _, after) = t.memory_parts();
        assert_eq!(
            after,
            2 * std::mem::size_of::<LinkKey>() + 4,
            "only the pair (and its one duplicate) stays"
        );
        assert_eq!(t.node(r).logical, 300);
        // A refresh that adds a third link to the kept identity still counts it once.
        t.fill_dir(ids[4], &[file("a3", 1)]);
        t.resolve_hardlinks();
        assert_eq!(t.node(r).logical, 300);
        assert_eq!(t.hardlink_duplicates(), 2);
    }

    #[test]
    fn links_and_estimates() {
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/v")])[0];
        let mut l = EntryMeta::file("ln", 999);
        l.kind = Kind::Link;
        let mut f = EntryMeta::file("f", 10);
        f.allocated = None;
        t.fill_dir(r, &[l, f]);
        assert_eq!(t.node(r).logical, 10);
        assert!(t.allocated_estimated());
        assert_eq!(t.node(r).allocated, 4096 + 10); // the link itself, not its target
    }
}
