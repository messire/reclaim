//! Duplicate-file model: grouping by size, hard-link collapsing and the removal rule.
//!
//! The pipeline (engine) is: group by size → drop groups that are one file under
//! several hard-link names → hash the first 64 KiB → hash the whole content (BLAKE3).
//! Files whose prefix matches but whose full hash was not computed (cancelled run,
//! read error) are only *probable* duplicates; a full-content hash match is *confirmed*.

use serde::{Deserialize, Serialize};

/// A file taking part in duplicate detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DupFile {
    /// Full path.
    pub path: String,
    /// Logical size.
    pub size: u64,
    /// Allocated size.
    pub allocated: u64,
    /// Identity (volume, file id), when known: two names with the same identity are one
    /// file (hard links), not duplicates.
    pub identity: Option<(u64, u64)>,
    /// Last write, Unix seconds.
    pub mtime: i64,
}

/// How certain a group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// Same size and same first 64 KiB; the rest was not compared.
    Probable,
    /// Same size and same BLAKE3 hash of the complete content.
    Confirmed,
}

/// Files with identical content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DupGroup {
    /// Common size.
    pub size: u64,
    /// Content hash (hex) for confirmed groups, prefix hash for probable ones.
    pub hash: String,
    /// Certainty.
    pub confidence: Confidence,
    /// The files (distinct identities), at least two.
    pub files: Vec<DupFile>,
}

impl DupGroup {
    /// Bytes freed by keeping one copy (allocated sizes of all but the largest).
    pub fn savings(&self) -> u64 {
        let total: u64 = self.files.iter().map(|f| f.allocated).sum();
        let keep = self.files.iter().map(|f| f.allocated).max().unwrap_or(0);
        total - keep
    }
}

/// Groups files of equal size (sizes below `min_size` and empty files are ignored),
/// after collapsing hard-link names of one identity into one entry. Only groups with
/// at least two distinct files remain, largest size first.
pub fn size_groups(mut files: Vec<DupFile>, min_size: u64) -> Vec<Vec<DupFile>> {
    files.retain(|f| f.size > 0 && f.size >= min_size);
    files.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.identity.cmp(&b.identity))
            .then_with(|| a.path.cmp(&b.path))
    });
    // Collapse hard links: same size and same identity.
    files.dedup_by(|b, a| a.size == b.size && a.identity.is_some() && a.identity == b.identity);
    let mut groups = Vec::new();
    let mut i = 0;
    while i < files.len() {
        let mut j = i + 1;
        while j < files.len() && files[j].size == files[i].size {
            j += 1;
        }
        if j - i > 1 {
            groups.push(files[i..j].to_vec());
        }
        i = j;
    }
    groups
}

/// Why a removal selection is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    /// Every copy of a group is selected; one must stay.
    WouldRemoveAllCopies {
        /// Group index.
        group: usize,
    },
    /// A selected index does not exist.
    Unknown,
}

/// Validates a selection of `(group, file)` pairs: at least one file of every group
/// must stay. Returns the selected paths.
pub fn validate_selection(
    groups: &[DupGroup],
    selected: &[(usize, usize)],
) -> Result<Vec<String>, SelectionError> {
    let mut per_group = vec![0usize; groups.len()];
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &(g, f) in selected {
        let file = groups
            .get(g)
            .and_then(|gr| gr.files.get(f))
            .ok_or(SelectionError::Unknown)?;
        if seen.insert((g, f)) {
            per_group[g] += 1;
            paths.push(file.path.clone());
        }
    }
    for (g, n) in per_group.iter().enumerate() {
        if *n > 0 && *n >= groups[g].files.len() {
            return Err(SelectionError::WouldRemoveAllCopies { group: g });
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, size: u64, id: Option<(u64, u64)>) -> DupFile {
        DupFile {
            path: path.into(),
            size,
            allocated: size,
            identity: id,
            mtime: 0,
        }
    }

    #[test]
    fn grouping_collapses_hardlinks() {
        let g = size_groups(
            vec![
                f("a", 10, Some((1, 1))),
                f("a-link", 10, Some((1, 1))),
                f("b", 10, Some((1, 2))),
                f("c", 20, None),
                f("d", 20, None),
                f("e", 30, Some((1, 3))),
                f("e2", 30, Some((1, 3))),
                f("empty1", 0, None),
                f("empty2", 0, None),
            ],
            1,
        );
        assert_eq!(g.len(), 2);
        assert_eq!(g[0][0].size, 20);
        assert_eq!(g[1].len(), 2); // a (or its link) and b
        let small = size_groups(vec![f("x", 5, None), f("y", 5, None)], 6);
        assert!(small.is_empty());
    }

    #[test]
    fn savings_and_selection() {
        let group = DupGroup {
            size: 10,
            hash: "h".into(),
            confidence: Confidence::Confirmed,
            files: vec![f("a", 10, None), f("b", 10, None), f("c", 10, None)],
        };
        assert_eq!(group.savings(), 20);
        let gs = vec![group];
        assert_eq!(
            validate_selection(&gs, &[(0, 0), (0, 1), (0, 1)]).unwrap(),
            ["a", "b"]
        );
        assert_eq!(
            validate_selection(&gs, &[(0, 0), (0, 1), (0, 2)]),
            Err(SelectionError::WouldRemoveAllCopies { group: 0 })
        );
        assert_eq!(
            validate_selection(&gs, &[(1, 0)]),
            Err(SelectionError::Unknown)
        );
        assert!(validate_selection(&gs, &[]).unwrap().is_empty());
    }
}
