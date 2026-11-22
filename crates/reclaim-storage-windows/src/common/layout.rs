//! Parsing of `FSCTL_QUERY_FILE_LAYOUT` output and building the scan index from it.
//!
//! Layout (winioctl.h): `QUERY_FILE_LAYOUT_OUTPUT` at the start of the buffer, then a
//! chain of `FILE_LAYOUT_ENTRY` (first at `FirstFileOffset` from the buffer start, next
//! at `NextFileOffset` from the current entry; 0 ends). Each entry points (relative to
//! itself) to a chain of `FILE_LAYOUT_NAME_ENTRY`, a chain of `STREAM_LAYOUT_ENTRY` and
//! one `FILE_LAYOUT_INFO_ENTRY`; name and stream chains link relative to the current
//! record. Every read is bounds-checked: a malformed record ends the batch, it never
//! reads outside the buffer.
//!
//! Identity: names refer to their parent by its full 64-bit file reference number
//! (segment number in the low 48 bits, sequence number in the high 16). Parents are
//! matched on the **segment number only**: the root reference obtained from
//! `GetFileInformationByHandle` and the references in the layout output may disagree on
//! the sequence bits, and a segment number identifies a live MFT record uniquely. File
//! identities reported to the index keep the full 64-bit value (the same value the
//! directory-listing adapter reports), so hard-link deduplication matches both.

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

use reclaim_storage_core::entry::{Attrs, EntryMeta, Kind};
use reclaim_storage_core::tree::{NodeId, Tree};

use super::{attrs_from, fa, filetime_to_unix, rd_i64, rd_u32, rd_u64, rd_utf16};

/// Mask of the segment (MFT record) number in a file reference number.
pub(crate) const SEGMENT_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
/// `$BadClus`: its `$Bad` stream maps the whole volume (sparse); it is not disk usage.
const BADCLUS_SEGMENT: u64 = 8;
/// `FILE_LAYOUT_NAME_ENTRY_DOS`: a pure 8.3 alias.
const NAME_DOS: u32 = 2;
/// `$DATA`, `$INDEX_ROOT` and `$INDEX_ALLOCATION` attribute type codes.
const ATTR_DATA: u32 = 0x80;
const ATTR_INDEX_ROOT: u32 = 0x90;
const ATTR_INDEX_ALLOCATION: u32 = 0xA0;

/// Byte offsets of the layout structures (checked against `windows-sys` on Windows).
pub(crate) mod off {
    pub const OUT_FILE_ENTRY_COUNT: usize = 0;
    pub const OUT_FIRST_FILE_OFFSET: usize = 4;

    pub const FILE_NEXT: usize = 4;
    pub const FILE_ATTRIBUTES: usize = 12;
    pub const FILE_REFERENCE: usize = 16;
    pub const FILE_FIRST_NAME: usize = 24;
    pub const FILE_FIRST_STREAM: usize = 28;
    pub const FILE_EXTRA_INFO: usize = 32;
    pub const FILE_EXTRA_INFO_LEN: usize = 36;

    pub const NAME_NEXT: usize = 0;
    pub const NAME_FLAGS: usize = 4;
    pub const NAME_PARENT: usize = 8;
    pub const NAME_LENGTH: usize = 16;
    pub const NAME_TEXT: usize = 24;

    pub const STREAM_NEXT: usize = 4;
    pub const STREAM_ALLOCATION: usize = 16;
    pub const STREAM_EOF: usize = 24;
    pub const STREAM_TYPE: usize = 36;
    pub const STREAM_ID_LENGTH: usize = 44;
    pub const STREAM_ID: usize = 48;

    pub const INFO_LAST_WRITE: usize = 16;
}

const IS_PARENT: u8 = 1;
const HAS_I30: u8 = 2;

/// One file record, compact.
#[derive(Debug, Clone, Copy)]
struct FileRec {
    frn: u64,
    logical: u64,
    allocated: u64,
    mtime: i64,
    attr: u32,
    names: u16,
    flags: u8,
}

/// One (non-DOS) name of a file.
#[derive(Debug, Clone, Copy)]
struct NameRec {
    parent_seg: u64,
    file: u32,
    off: u32,
    len: u32,
}

/// Everything collected from the layout output.
#[derive(Debug, Default)]
pub(crate) struct Layout {
    files: Vec<FileRec>,
    names: Vec<NameRec>,
    arena: String,
}

/// Result of [`Layout::build`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BuildStats {
    pub files: u64,
    pub dirs: u64,
    pub orphans: u64,
    pub cancelled: bool,
}

fn write(tree: &RwLock<Tree>) -> std::sync::RwLockWriteGuard<'_, Tree> {
    tree.write().unwrap_or_else(|p| p.into_inner())
}

impl Layout {
    /// Files collected so far.
    pub(crate) fn file_count(&self) -> u64 {
        self.files.len() as u64
    }

    /// Parses one output buffer. Returns the number of file entries read; a malformed
    /// entry ends the batch.
    pub(crate) fn parse_batch(&mut self, buf: &[u8]) -> u64 {
        let (Some(count), Some(first)) = (
            rd_u32(buf, off::OUT_FILE_ENTRY_COUNT),
            rd_u32(buf, off::OUT_FIRST_FILE_OFFSET),
        ) else {
            return 0;
        };
        let mut at = first as usize;
        let mut n = 0;
        while n < u64::from(count) && at != 0 {
            let Some(next) = self.parse_entry(buf, at) else {
                break;
            };
            n += 1;
            if next == 0 {
                break;
            }
            let Some(a) = at.checked_add(next as usize) else {
                break;
            };
            at = a;
        }
        n
    }

    /// Parses one `FILE_LAYOUT_ENTRY`; returns its `NextFileOffset`.
    fn parse_entry(&mut self, buf: &[u8], e: usize) -> Option<u32> {
        let next = rd_u32(buf, e + off::FILE_NEXT)?;
        let attr = rd_u32(buf, e + off::FILE_ATTRIBUTES)?;
        let frn = rd_u64(buf, e + off::FILE_REFERENCE)?;
        let first_name = rd_u32(buf, e + off::FILE_FIRST_NAME)? as usize;
        let first_stream = rd_u32(buf, e + off::FILE_FIRST_STREAM)? as usize;
        let extra = rd_u32(buf, e + off::FILE_EXTRA_INFO)? as usize;
        let extra_len = rd_u32(buf, e + off::FILE_EXTRA_INFO_LEN)? as usize;

        let mut rec = FileRec {
            frn,
            logical: 0,
            allocated: 0,
            mtime: 0,
            attr,
            names: 0,
            flags: 0,
        };
        if first_stream != 0 {
            let mut s = e.checked_add(first_stream)?;
            loop {
                let alloc = rd_i64(buf, s + off::STREAM_ALLOCATION)?;
                let eof = rd_i64(buf, s + off::STREAM_EOF)?;
                let ty = rd_u32(buf, s + off::STREAM_TYPE)?;
                let id_len = rd_u32(buf, s + off::STREAM_ID_LENGTH)? as usize;
                let id = rd_utf16(buf, s + off::STREAM_ID, id_len)?;
                let unnamed = id.is_empty()
                    || id.eq_ignore_ascii_case("::$DATA")
                    || id.eq_ignore_ascii_case("$DATA");
                if ty == ATTR_DATA {
                    if unnamed {
                        rec.logical = u64::try_from(eof).unwrap_or(0);
                    }
                    if unnamed || frn & SEGMENT_MASK != BADCLUS_SEGMENT {
                        rec.allocated += u64::try_from(alloc).unwrap_or(0);
                    }
                } else if (ty == ATTR_INDEX_ROOT || ty == ATTR_INDEX_ALLOCATION)
                    && id.contains("$I30")
                {
                    rec.flags |= HAS_I30;
                }
                let next_s = rd_u32(buf, s + off::STREAM_NEXT)? as usize;
                if next_s == 0 {
                    break;
                }
                s = s.checked_add(next_s)?;
            }
        }
        if extra != 0 && extra_len >= off::INFO_LAST_WRITE + 8 {
            rec.mtime = filetime_to_unix(rd_i64(buf, e + extra + off::INFO_LAST_WRITE)?);
        }
        let file = u32::try_from(self.files.len()).ok()?;
        let names_before = self.names.len();
        let arena_before = self.arena.len();
        if first_name != 0 {
            let mut n = e.checked_add(first_name)?;
            loop {
                let parsed = (|| {
                    let flags = rd_u32(buf, n + off::NAME_FLAGS)?;
                    let parent = rd_u64(buf, n + off::NAME_PARENT)?;
                    let len = rd_u32(buf, n + off::NAME_LENGTH)? as usize;
                    let name = rd_utf16(buf, n + off::NAME_TEXT, len)?;
                    let next_n = rd_u32(buf, n + off::NAME_NEXT)? as usize;
                    Some((flags, parent, name, next_n))
                })();
                let Some((flags, parent, name, next_n)) = parsed else {
                    // Roll back this entry's names: the record is malformed.
                    self.names.truncate(names_before);
                    self.arena.truncate(arena_before);
                    return None;
                };
                if flags != NAME_DOS && !name.is_empty() {
                    let o = u32::try_from(self.arena.len()).ok()?;
                    self.arena.push_str(&name);
                    self.names.push(NameRec {
                        parent_seg: parent & SEGMENT_MASK,
                        file,
                        off: o,
                        len: name.len() as u32,
                    });
                    rec.names = rec.names.saturating_add(1);
                }
                if next_n == 0 {
                    break;
                }
                n = n.checked_add(next_n)?;
            }
        }
        self.files.push(rec);
        Some(next)
    }

    fn name(&self, n: &NameRec) -> &str {
        &self.arena[n.off as usize..(n.off + n.len) as usize]
    }

    /// Builds the index below one root entry named `volume_root`. Breadth first from the
    /// root's segment, each directory is filled exactly once with all its names; a name
    /// whose parent is never reached is an orphan (counted, dropped). Hard links appear
    /// once per name and are resolved at the end. The tree lock is held only while one
    /// directory is inserted.
    pub(crate) fn build(
        mut self,
        tree: &RwLock<Tree>,
        volume_root: &str,
        root_frn: u64,
        volume: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> BuildStats {
        let root_seg = root_frn & SEGMENT_MASK;
        let index: HashMap<u64, u32> = self
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.frn & SEGMENT_MASK, i as u32))
            .collect();
        for i in 0..self.names.len() {
            let n = self.names[i];
            let own = self.files[n.file as usize].frn & SEGMENT_MASK;
            if own != n.parent_seg
                && let Some(&p) = index.get(&n.parent_seg)
            {
                self.files[p as usize].flags |= IS_PARENT;
            }
        }
        // Names grouped by parent segment.
        let mut order: Vec<u32> = (0..self.names.len() as u32).collect();
        order.sort_unstable_by_key(|&i| self.names[i as usize].parent_seg);

        let mut root = EntryMeta::dir(volume_root);
        root.volume = Some(volume);
        match index.get(&root_seg) {
            Some(&r) => {
                let f = self.files[r as usize];
                root.file_id = Some(f.frn);
                root.mtime = f.mtime;
                root.attrs = attrs_from(f.attr & !fa::REPARSE_POINT);
            }
            None => root.file_id = Some(root_frn),
        }
        let root_id = write(tree).set_roots(&[root])[0];

        let mut stats = BuildStats::default();
        let mut visited = vec![false; self.files.len()];
        if let Some(&r) = index.get(&root_seg) {
            visited[r as usize] = true;
        }
        let mut inserted_names = 0u64;
        let mut self_names = 0u64;
        let mut queue: VecDeque<(NodeId, u64)> = VecDeque::new();
        queue.push_back((root_id, root_seg));
        let mut entries: Vec<EntryMeta> = Vec::new();
        let mut child_dirs: Vec<(usize, u64)> = Vec::new();
        while let Some((node, seg)) = queue.pop_front() {
            if cancelled() {
                let mut t = write(tree);
                t.mark(node, Attrs::INCOMPLETE);
                for (n, _) in queue.drain(..) {
                    t.mark(n, Attrs::INCOMPLETE);
                }
                stats.cancelled = true;
                break;
            }
            let lo = order.partition_point(|&i| self.names[i as usize].parent_seg < seg);
            let hi = order.partition_point(|&i| self.names[i as usize].parent_seg <= seg);
            entries.clear();
            child_dirs.clear();
            for &ni in &order[lo..hi] {
                let n = self.names[ni as usize];
                let f = self.files[n.file as usize];
                let fseg = f.frn & SEGMENT_MASK;
                if fseg == seg {
                    self_names += 1; // the root's "." name
                    continue;
                }
                let mut attrs = attrs_from(f.attr);
                let name = self.name(&n).to_owned();
                if seg == root_seg && name.starts_with('$') {
                    attrs.set(Attrs::SYSTEM);
                }
                let is_dir = f.attr & (fa::DIRECTORY | fa::DUP_FILE_NAME_INDEX_PRESENT) != 0
                    || f.flags & (HAS_I30 | IS_PARENT) != 0;
                let reparse = f.attr & fa::REPARSE_POINT != 0;
                if reparse {
                    attrs.set(Attrs::REPARSE);
                }
                // The layout output carries no reparse tag. A reparse directory with
                // children holds real data (cloud folders); one without children is
                // treated as a link (junctions, mount points and directory symlinks
                // must be empty directories).
                let kind = if is_dir && reparse && f.flags & IS_PARENT == 0 {
                    Kind::Link
                } else if is_dir {
                    Kind::Dir
                } else {
                    Kind::File
                };
                let (logical, allocated) = match kind {
                    Kind::File => (f.logical, f.allocated),
                    _ => (0, 0),
                };
                if kind == Kind::Dir {
                    child_dirs.push((entries.len(), fseg));
                }
                entries.push(EntryMeta {
                    name,
                    kind,
                    logical,
                    allocated: Some(allocated),
                    mtime: f.mtime,
                    attrs,
                    file_id: Some(f.frn),
                    nlink: if kind == Kind::File {
                        Some(u32::from(f.names))
                    } else {
                        None
                    },
                    volume: Some(volume),
                });
            }
            let ids = write(tree).fill_dir(node, &entries);
            inserted_names += entries.len() as u64;
            for e in &entries {
                if e.kind == Kind::Dir {
                    stats.dirs += 1;
                } else {
                    stats.files += 1;
                }
            }
            for &(ei, dseg) in &child_dirs {
                let Some(&fi) = index.get(&dseg) else {
                    continue;
                };
                if !std::mem::replace(&mut visited[fi as usize], true) {
                    queue.push_back((ids[ei], dseg));
                }
            }
        }
        if !stats.cancelled {
            stats.orphans = (self.names.len() as u64).saturating_sub(inserted_names + self_names);
        }
        write(tree).resolve_hardlinks();
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::utf16le;
    use reclaim_storage_core::path::PathStyle;
    use reclaim_storage_core::tree::TOP;

    struct Name<'a> {
        parent: u64,
        name: &'a str,
        flags: u32,
    }

    struct Stream<'a> {
        ty: u32,
        id: &'a str,
        alloc: i64,
        eof: i64,
    }

    fn pad8(v: &mut Vec<u8>) {
        while !v.len().is_multiple_of(8) {
            v.push(0);
        }
    }

    fn put_u32(v: &mut [u8], at: usize, x: u32) {
        v[at..at + 4].copy_from_slice(&x.to_le_bytes());
    }

    fn put_u64(v: &mut [u8], at: usize, x: u64) {
        v[at..at + 8].copy_from_slice(&x.to_le_bytes());
    }

    /// One FILE_LAYOUT_ENTRY with its names, streams and info entry.
    fn entry(
        frn: u64,
        attr: u32,
        names: &[Name<'_>],
        streams: &[Stream<'_>],
        mtime_ft: i64,
    ) -> Vec<u8> {
        let mut e = vec![0u8; 40];
        put_u32(&mut e, off::FILE_ATTRIBUTES, attr);
        put_u64(&mut e, off::FILE_REFERENCE, frn);
        // Names.
        let first_name = e.len();
        let mut prev: Option<usize> = None;
        for n in names {
            let at = e.len();
            if let Some(p) = prev {
                let d = (at - p) as u32;
                put_u32(&mut e, p + off::NAME_NEXT, d);
            }
            let text = utf16le(n.name);
            let mut r = vec![0u8; off::NAME_TEXT];
            put_u32(&mut r, off::NAME_FLAGS, n.flags);
            put_u64(&mut r, off::NAME_PARENT, n.parent);
            put_u32(&mut r, off::NAME_LENGTH, text.len() as u32);
            r.extend(text);
            e.extend(r);
            pad8(&mut e);
            prev = Some(at);
        }
        if !names.is_empty() {
            put_u32(&mut e, off::FILE_FIRST_NAME, first_name as u32);
        }
        // Streams.
        let first_stream = e.len();
        let mut prev: Option<usize> = None;
        for s in streams {
            let at = e.len();
            if let Some(p) = prev {
                let d = (at - p) as u32;
                put_u32(&mut e, p + off::STREAM_NEXT, d);
            }
            let id = utf16le(s.id);
            let mut r = vec![0u8; off::STREAM_ID];
            put_u64(&mut r, off::STREAM_ALLOCATION, s.alloc as u64);
            put_u64(&mut r, off::STREAM_EOF, s.eof as u64);
            put_u32(&mut r, off::STREAM_TYPE, s.ty);
            put_u32(&mut r, off::STREAM_ID_LENGTH, id.len() as u32);
            r.extend(id);
            e.extend(r);
            pad8(&mut e);
            prev = Some(at);
        }
        if !streams.is_empty() {
            put_u32(&mut e, off::FILE_FIRST_STREAM, first_stream as u32);
        }
        // Info entry.
        let info = e.len();
        let mut r = vec![0u8; 64];
        put_u64(&mut r, off::INFO_LAST_WRITE, mtime_ft as u64);
        e.extend(r);
        put_u32(&mut e, off::FILE_EXTRA_INFO, info as u32);
        put_u32(&mut e, off::FILE_EXTRA_INFO_LEN, 64);
        e
    }

    fn batch(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        put_u32(&mut b, off::OUT_FILE_ENTRY_COUNT, entries.len() as u32);
        put_u32(&mut b, off::OUT_FIRST_FILE_OFFSET, 16);
        let mut prev: Option<usize> = None;
        for e in entries {
            let at = b.len();
            if let Some(p) = prev {
                let d = (at - p) as u32;
                put_u32(&mut b, p + off::FILE_NEXT, d);
            }
            b.extend(e);
            pad8(&mut b);
            prev = Some(at);
        }
        b
    }

    const SEQ: u64 = 0x0005_0000_0000_0000;

    fn data(alloc: i64, eof: i64) -> Stream<'static> {
        Stream {
            ty: ATTR_DATA,
            id: "::$DATA",
            alloc,
            eof,
        }
    }

    fn i30() -> Stream<'static> {
        Stream {
            ty: ATTR_INDEX_ROOT,
            id: "$I30",
            alloc: 0,
            eof: 0,
        }
    }

    fn nm(parent: u64, name: &str) -> Name<'_> {
        Name {
            parent,
            name,
            flags: 1,
        }
    }

    fn sample() -> Vec<u8> {
        let root = 5 | SEQ;
        batch(&[
            // Root: names itself ".".
            entry(root, fa::DIRECTORY, &[nm(root, ".")], &[i30()], 0),
            entry(
                1 << 48,
                0x6,
                &[nm(root, "$MFT")],
                &[data(1 << 20, 1 << 20)],
                0,
            ),
            // $BadClus: the $Bad stream must not count.
            entry(
                8 | (8 << 48),
                0x6,
                &[nm(root, "$BadClus")],
                &[
                    data(0, 0),
                    Stream {
                        ty: ATTR_DATA,
                        id: ":$Bad:$DATA",
                        alloc: 1 << 40,
                        eof: 1 << 40,
                    },
                ],
                0,
            ),
            // Directory "Users" (attributes without the directory bit, but an $I30).
            entry(40 | SEQ, 0, &[nm(root, "Users")], &[i30()], 0),
            // A file with a long and a DOS name, and an alternate data stream.
            entry(
                41 | SEQ,
                fa::HIDDEN,
                &[
                    nm(40, "Long file name.txt"),
                    Name {
                        parent: 40 | SEQ,
                        name: "LONGFI~1.TXT",
                        flags: NAME_DOS,
                    },
                ],
                &[
                    data(8192, 5000),
                    Stream {
                        ty: ATTR_DATA,
                        id: ":ads:$DATA",
                        alloc: 4096,
                        eof: 10,
                    },
                ],
                116_444_736_000_000_000 + 20_000_000,
            ),
            // A hard-linked file: two names in two directories.
            entry(
                42 | SEQ,
                0,
                &[nm(40, "a"), nm(root, "b")],
                &[data(4096, 100)],
                0,
            ),
            // A junction (empty reparse directory) and a cloud folder with a child.
            entry(
                43 | SEQ,
                fa::DIRECTORY | fa::REPARSE_POINT,
                &[nm(40, "j")],
                &[],
                0,
            ),
            entry(
                44 | SEQ,
                fa::DIRECTORY | fa::REPARSE_POINT,
                &[nm(40, "OneDrive")],
                &[],
                0,
            ),
            entry(
                45 | SEQ,
                fa::REPARSE_POINT,
                &[nm(44, "doc")],
                &[data(0, 777)],
                0,
            ),
            // An orphan (parent never listed).
            entry(46 | SEQ, 0, &[nm(999, "lost")], &[data(4096, 1)], 0),
        ])
    }

    #[test]
    fn parses_and_builds() {
        let mut l = Layout::default();
        assert_eq!(l.parse_batch(&sample()), 10);
        assert_eq!(l.file_count(), 10);
        let tree = RwLock::new(Tree::new(PathStyle::Windows));
        let s = l.build(&tree, "C:\\", 5, 1234, &|| false);
        assert_eq!(s.orphans, 1);
        assert!(!s.cancelled);
        let t = tree.read().unwrap();
        let f = t.find("C:\\Users\\Long file name.txt").unwrap();
        let n = t.node(f);
        assert_eq!((n.logical, n.allocated, n.mtime), (5000, 8192 + 4096, 2));
        assert!(n.attrs.has(Attrs::HIDDEN));
        assert!(t.find("C:\\Users\\LONGFI~1.TXT").is_none());
        assert_eq!(t.node(t.find("C:\\Users\\j").unwrap()).kind, Kind::Link);
        let od = t.find("C:\\Users\\OneDrive").unwrap();
        assert_eq!(t.node(od).kind, Kind::Dir);
        assert!(t.node(od).attrs.has(Attrs::REPARSE));
        assert_eq!(
            t.node(t.find("C:\\Users\\OneDrive\\doc").unwrap()).logical,
            777
        );
        let mft = t.find("C:\\$MFT").unwrap();
        assert!(t.node(mft).attrs.has(Attrs::SYSTEM));
        assert_eq!(t.node(t.find("C:\\$BadClus").unwrap()).allocated, 0);
        // Hard link counted once.
        assert_eq!(t.hardlink_duplicates(), 1);
        assert_eq!(t.node(TOP).logical, (1 << 20) + 5000 + 100 + 777);
        assert!(t.find("C:\\b").is_some() && t.find("C:\\Users\\a").is_some());
        assert_eq!(s.dirs, 2);
        assert_eq!(s.files, 3 + 3 + 1); // root: $MFT $BadClus b; Users: file a j(link); OneDrive: doc
    }

    #[test]
    fn cancel_marks_incomplete() {
        let mut l = Layout::default();
        l.parse_batch(&sample());
        let tree = RwLock::new(Tree::new(PathStyle::Windows));
        let s = l.build(&tree, "C:\\", 5, 1, &|| true);
        assert!(s.cancelled);
        let t = tree.read().unwrap();
        assert!(t.has_gaps());
    }

    #[test]
    fn malformed_batches_are_safe() {
        let mut b = sample();
        let mut l = Layout::default();
        // Truncated at every length: never panics, never reads out of bounds.
        for cut in 0..b.len() {
            l.parse_batch(&b[..cut]);
        }
        // A chain that ends early stops the batch.
        put_u32(&mut b, 16 + off::FILE_NEXT, 0);
        assert_eq!(Layout::default().parse_batch(&b), 1);
    }
}
