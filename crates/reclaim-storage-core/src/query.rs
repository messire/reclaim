//! Queries over a finished (or partial) index: extension statistics, largest files and
//! directories, and filters. None of them touches the filesystem: they read the index
//! only, so changing a filter never needs a rescan.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use regex::{Regex, RegexBuilder};

use crate::entry::{Attrs, Kind};
use crate::path::{PathStyle, glob_match};
use crate::tree::{NodeId, SizeBasis, TOP, Tree};

/// Totals of one extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtStat {
    /// Extension id in the tree (0 = none).
    pub ext: u32,
    /// Extension text, lower case, without the dot (empty = none).
    pub name: String,
    /// Files with this extension.
    pub files: u64,
    /// Logical bytes.
    pub logical: u64,
    /// Allocated bytes.
    pub allocated: u64,
}

/// Number of extensions that get their own treemap colour; the rest share one.
pub const PALETTE_SIZE: usize = 14;

/// Extension statistics below `from`, largest first by `basis`. Hard-link duplicates
/// are not counted (they were counted at their first name).
pub fn extension_stats(t: &Tree, from: NodeId, basis: SizeBasis) -> Vec<ExtStat> {
    let mut acc: Vec<(u64, u64, u64)> = vec![(0, 0, 0); t.extensions().len()];
    for id in t.descendants(from) {
        let n = t.node(id);
        if n.kind != Kind::File || n.attrs.has(Attrs::HARDLINK_DUP) {
            continue;
        }
        let e = &mut acc[t.ext_id(id) as usize];
        e.0 += 1;
        e.1 += n.logical;
        e.2 += n.allocated;
    }
    let mut out: Vec<ExtStat> = acc
        .into_iter()
        .enumerate()
        .filter(|(_, a)| a.0 > 0)
        .map(|(i, a)| ExtStat {
            ext: i as u32,
            name: t.extensions().name(i as u32).to_owned(),
            files: a.0,
            logical: a.1,
            allocated: a.2,
        })
        .collect();
    out.sort_by(|a, b| {
        let (x, y) = match basis {
            SizeBasis::Allocated => (a.allocated, b.allocated),
            SizeBasis::Logical => (a.logical, b.logical),
        };
        y.cmp(&x).then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Colour slot per extension id: the `PALETTE_SIZE` largest extensions get slots
/// `1..=PALETTE_SIZE` in size order, everything else slot 0. Shared by the treemap and
/// the extension list so their colours always agree.
pub fn palette(stats: &[ExtStat], ext_count: usize) -> Vec<u8> {
    let mut slots = vec![0u8; ext_count];
    for (rank, s) in stats.iter().take(PALETTE_SIZE).enumerate() {
        if let Some(slot) = slots.get_mut(s.ext as usize) {
            *slot = (rank + 1) as u8;
        }
    }
    slots
}

/// What to rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    /// Files.
    Files,
    /// Directories (aggregate size; scan roots excluded).
    Dirs,
}

/// The `limit` largest files or directories below `from` by `basis`, largest first.
/// O(n log limit).
pub fn largest(
    t: &Tree,
    from: NodeId,
    what: TopKind,
    basis: SizeBasis,
    limit: usize,
) -> Vec<NodeId> {
    if limit == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, Reverse<NodeId>)>> = BinaryHeap::new();
    for id in t.descendants(from) {
        let n = t.node(id);
        let take = match what {
            TopKind::Files => n.kind == Kind::File && !n.attrs.has(Attrs::HARDLINK_DUP),
            TopKind::Dirs => n.is_dir() && id != TOP && n.parent != TOP,
        };
        if !take {
            continue;
        }
        let key = (n.size(basis), Reverse(id));
        if heap.len() < limit {
            heap.push(Reverse(key));
        } else if let Some(Reverse(min)) = heap.peek()
            && key > *min
        {
            heap.pop();
            heap.push(Reverse(key));
        }
    }
    let mut v: Vec<(u64, Reverse<NodeId>)> = heap.into_iter().map(|Reverse(k)| k).collect();
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().map(|(_, Reverse(id))| id).collect()
}

/// Sort keys for result lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Size under the basis.
    Size,
    /// Full path.
    Path,
    /// Extension, then size.
    Extension,
    /// Last write time.
    Modified,
}

/// Sorts node ids in place.
pub fn sort_nodes(t: &Tree, ids: &mut [NodeId], key: SortKey, basis: SizeBasis, ascending: bool) {
    match key {
        SortKey::Size => ids.sort_by_key(|&i| (t.node(i).size(basis), i)),
        SortKey::Modified => ids.sort_by_key(|&i| (t.node(i).mtime, i)),
        SortKey::Extension => ids.sort_by(|&a, &b| {
            t.extensions()
                .name(t.ext_id(a))
                .cmp(t.extensions().name(t.ext_id(b)))
                .then(t.node(a).size(basis).cmp(&t.node(b).size(basis)))
        }),
        SortKey::Path => {
            let mut keyed: Vec<(String, NodeId)> = ids
                .iter()
                .map(|&i| {
                    let p = t.path(i);
                    (
                        if t.style() == PathStyle::Windows {
                            p.to_lowercase()
                        } else {
                            p
                        },
                        i,
                    )
                })
                .collect();
            keyed.sort();
            for (slot, (_, id)) in ids.iter_mut().zip(keyed) {
                *slot = id;
            }
        }
    }
    if !ascending {
        ids.reverse();
    }
}

/// How a text criterion matches.
#[derive(Debug, Clone)]
pub enum TextMatch {
    /// Case-insensitive substring.
    Contains(String),
    /// Glob with `*` and `?` over the whole text (case-insensitive).
    Glob(String),
    /// Regular expression (case-insensitive), searched anywhere.
    Regex(Regex),
}

impl TextMatch {
    /// Parses user input: `/expr/` is a regex, text with `*` or `?` a glob, anything
    /// else a substring. An invalid regex is an error message for the user.
    pub fn parse(input: &str) -> Result<Option<TextMatch>, String> {
        let s = input.trim();
        if s.is_empty() {
            return Ok(None);
        }
        if s.len() >= 2 && s.starts_with('/') && s.ends_with('/') {
            let r = RegexBuilder::new(&s[1..s.len() - 1])
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .map_err(|e| format!("invalid regular expression: {e}"))?;
            return Ok(Some(TextMatch::Regex(r)));
        }
        if s.contains('*') || s.contains('?') {
            return Ok(Some(TextMatch::Glob(s.to_owned())));
        }
        Ok(Some(TextMatch::Contains(s.to_lowercase())))
    }

    /// Whether `text` matches.
    pub fn is_match(&self, text: &str) -> bool {
        match self {
            TextMatch::Contains(n) => text.to_lowercase().contains(n.as_str()),
            TextMatch::Glob(g) => glob_match(g, text, true),
            TextMatch::Regex(r) => r.is_match(text),
        }
    }
}

/// Which kinds a filter returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    /// Files and directories.
    #[default]
    Any,
    /// Files only.
    Files,
    /// Directories only.
    Dirs,
}

/// A combination of criteria; all given criteria must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// File name criterion.
    pub name: Option<TextMatch>,
    /// Full path criterion.
    pub path: Option<TextMatch>,
    /// Allowed extensions (lower case, no dot; "" = none). Empty = any. Files only.
    pub extensions: Vec<String>,
    /// Minimum size (inclusive) under the basis.
    pub min_size: Option<u64>,
    /// Maximum size (inclusive) under the basis.
    pub max_size: Option<u64>,
    /// Modified at or after (Unix seconds).
    pub modified_after: Option<i64>,
    /// Modified at or before (Unix seconds).
    pub modified_before: Option<i64>,
    /// Kinds.
    pub kind: KindFilter,
    /// Size basis for the size criteria.
    pub basis: SizeBasis,
}

/// Result of a filter run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterResult {
    /// Matching ids in index order (at most the limit).
    pub ids: Vec<NodeId>,
    /// All matches, including those beyond the limit.
    pub total: u64,
    /// Sum of the matches' sizes under the basis (hard-link duplicates excluded).
    pub bytes: u64,
}

impl Filter {
    /// Whether no criterion is set.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.path.is_none()
            && self.extensions.is_empty()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.modified_after.is_none()
            && self.modified_before.is_none()
            && self.kind == KindFilter::Any
    }

    /// Runs the filter below `from`. Paths are built only when a path criterion is set.
    pub fn run(&self, t: &Tree, from: NodeId, limit: usize) -> FilterResult {
        let mut res = FilterResult::default();
        let ext_ids: Vec<u32> = self
            .extensions
            .iter()
            .filter_map(|e| t.extensions().id(&e.trim_start_matches('.').to_lowercase()))
            .collect();
        if !self.extensions.is_empty() && ext_ids.is_empty() {
            return res;
        }
        let mut check = |id: NodeId, path: Option<&str>| {
            if id == TOP || !self.matches(t, id, &ext_ids, path) {
                return;
            }
            res.total += 1;
            let n = t.node(id);
            if !n.attrs.has(Attrs::HARDLINK_DUP) {
                res.bytes += n.size(self.basis);
            }
            if res.ids.len() < limit {
                res.ids.push(id);
            }
        };
        if self.path.is_some() {
            t.walk_paths(from, |id, p| {
                check(id, Some(p));
                true
            });
        } else {
            for id in t.descendants(from) {
                check(id, None);
            }
        }
        res
    }

    fn matches(&self, t: &Tree, id: NodeId, ext_ids: &[u32], path: Option<&str>) -> bool {
        let n = t.node(id);
        match self.kind {
            KindFilter::Files if n.is_dir() => return false,
            KindFilter::Dirs if !n.is_dir() => return false,
            _ => {}
        }
        if !ext_ids.is_empty() && (n.is_dir() || !ext_ids.contains(&t.ext_id(id))) {
            return false;
        }
        let size = n.size(self.basis);
        if self.min_size.is_some_and(|m| size < m) || self.max_size.is_some_and(|m| size > m) {
            return false;
        }
        if self.modified_after.is_some_and(|m| n.mtime < m)
            || self.modified_before.is_some_and(|m| n.mtime > m)
        {
            return false;
        }
        if let Some(m) = &self.name
            && !m.is_match(t.name(id))
        {
            return false;
        }
        if let (Some(m), Some(p)) = (&self.path, path)
            && !m.is_match(p)
        {
            return false;
        }
        true
    }
}

/// Parses a size like `10`, `10M`, `1.5 GiB`, `200kb` into bytes (binary units).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().ok()?;
    let mult: u64 = match unit.trim().trim_end_matches("ib").trim_end_matches('b') {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return None,
    };
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some((num * mult as f64).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryMeta;
    use crate::tree::tests::sample;

    #[test]
    fn ext_stats_and_palette() {
        let (t, r) = sample();
        let s = extension_stats(&t, r, SizeBasis::Logical);
        assert_eq!(s[0].name, "bin");
        assert_eq!(s[0].logical, 5000);
        let txt = s.iter().find(|e| e.name == "txt").unwrap();
        assert_eq!((txt.files, txt.logical), (2, 110));
        let none = s.iter().find(|e| e.name.is_empty()).unwrap();
        assert_eq!(none.files, 1);
        let p = palette(&s, t.extensions().len());
        assert_eq!(p[s[0].ext as usize], 1);
        assert_eq!(p.len(), t.extensions().len());
        let by_alloc = extension_stats(&t, r, SizeBasis::Allocated);
        assert_eq!(by_alloc[0].name, "bin");
    }

    #[test]
    fn largest_files_and_dirs() {
        let (t, r) = sample();
        let f = largest(&t, TOP, TopKind::Files, SizeBasis::Logical, 2);
        assert_eq!(f.len(), 2);
        assert_eq!(t.name(f[0]), "b.BIN");
        assert_eq!(t.name(f[1]), "a.txt");
        let d = largest(&t, r, TopKind::Dirs, SizeBasis::Logical, 10);
        assert_eq!(d.len(), 1);
        assert_eq!(t.name(d[0]), "sub");
        assert!(largest(&t, r, TopKind::Files, SizeBasis::Logical, 0).is_empty());
    }

    #[test]
    fn sorting() {
        let (t, r) = sample();
        let mut ids: Vec<_> = t.children(r).collect();
        sort_nodes(&t, &mut ids, SortKey::Size, SizeBasis::Logical, false);
        assert_eq!(t.name(ids[0]), "b.BIN");
        sort_nodes(&t, &mut ids, SortKey::Path, SizeBasis::Logical, true);
        assert_eq!(t.name(ids[0]), "a.txt");
        sort_nodes(&t, &mut ids, SortKey::Extension, SizeBasis::Logical, true);
        assert_eq!(t.name(ids[0]), "sub"); // no extension sorts first
        sort_nodes(&t, &mut ids, SortKey::Modified, SizeBasis::Logical, true);
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn filters() {
        let (t, _) = sample();
        let run = |f: Filter| f.run(&t, TOP, 100);
        let names = |r: &FilterResult| {
            r.ids
                .iter()
                .map(|&i| t.name(i).to_owned())
                .collect::<Vec<_>>()
        };

        let f = Filter {
            name: TextMatch::parse("*.txt").unwrap(),
            ..Filter::default()
        };
        assert_eq!(names(&run(f)), ["a.txt", "c.txt"]);

        let f = Filter {
            name: TextMatch::parse("/^[ab]\\./").unwrap(),
            ..Filter::default()
        };
        assert_eq!(names(&run(f)), ["a.txt", "b.BIN"]);

        let f = Filter {
            path: TextMatch::parse("sub").unwrap(),
            kind: KindFilter::Files,
            ..Filter::default()
        };
        let r = run(f);
        assert_eq!(names(&r), ["c.txt", ".hidden"]);
        assert_eq!(r.total, 2);

        let f = Filter {
            extensions: vec![".BIN".into()],
            ..Filter::default()
        };
        assert_eq!(names(&run(f)), ["b.BIN"]);

        let f = Filter {
            extensions: vec!["zip".into()],
            ..Filter::default()
        };
        assert_eq!(run(f).total, 0);

        let f = Filter {
            min_size: Some(50),
            max_size: Some(200),
            basis: SizeBasis::Logical,
            kind: KindFilter::Files,
            ..Filter::default()
        };
        let r = run(f);
        assert_eq!(names(&r), ["a.txt"]);
        assert_eq!(r.bytes, 100);

        let f = Filter {
            kind: KindFilter::Dirs,
            ..Filter::default()
        };
        assert_eq!(run(f).total, 2); // the root and sub
        assert!(Filter::default().is_empty());

        let limited = Filter::default().run(&t, TOP, 2);
        assert_eq!(limited.ids.len(), 2);
        assert_eq!(limited.total, 6);

        let f = Filter {
            modified_after: Some(1),
            ..Filter::default()
        };
        assert_eq!(run(f).total, 0);
        assert!(TextMatch::parse("/(/").is_err());
        assert!(TextMatch::parse("  ").unwrap().is_none());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("10"), Some(10));
        assert_eq!(parse_size("10k"), Some(10240));
        assert_eq!(parse_size("1.5 GiB"), Some(1_610_612_736));
        assert_eq!(parse_size("200KB"), Some(204_800));
        assert_eq!(parse_size("3 mb"), Some(3 << 20));
        assert_eq!(parse_size("1T"), Some(1 << 40));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("5 parsecs"), None);
    }

    #[test]
    fn hardlink_dups_not_double_counted() {
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/v")])[0];
        let link = |n: &str| EntryMeta {
            file_id: Some(1),
            nlink: Some(2),
            ..EntryMeta::file(n, 100)
        };
        t.fill_dir(r, &[link("a.x"), link("b.x")]);
        t.resolve_hardlinks();
        let s = extension_stats(&t, r, SizeBasis::Logical);
        assert_eq!((s[0].files, s[0].logical), (1, 100));
        assert_eq!(
            largest(&t, r, TopKind::Files, SizeBasis::Logical, 5).len(),
            1
        );
    }
}
