//! Parsing of `FILE_NOTIFY_INFORMATION` chains (`ReadDirectoryChangesW`).

use super::{rd_u32, rd_utf16};

const NEXT: usize = 0;
const NAME_LENGTH: usize = 8;
const NAME: usize = 12;

/// The directory that contains `rel` (a path relative to `root`, `\`-separated).
pub(crate) fn parent_dir(root: &str, rel: &str) -> String {
    match rel.rsplit_once('\\') {
        Some((dir, _)) if !dir.is_empty() => {
            let base = root.trim_end_matches('\\');
            format!("{base}\\{dir}")
        }
        _ => root.to_owned(),
    }
}

/// Calls `f` with the parent directory (full path) of every name in one buffer. Stops at
/// a truncated record.
pub(crate) fn for_each_parent(buf: &[u8], root: &str, mut f: impl FnMut(String)) {
    let mut at = 0usize;
    loop {
        let Some(next) = rd_u32(buf, at + NEXT) else {
            return;
        };
        let Some(len) = rd_u32(buf, at + NAME_LENGTH) else {
            return;
        };
        let Some(name) = rd_utf16(buf, at + NAME, len as usize) else {
            return;
        };
        f(parent_dir(root, &name));
        if next == 0 {
            return;
        }
        match at.checked_add(next as usize) {
            Some(a) => at = a,
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::utf16le;

    fn rec(name: &str, last: bool) -> Vec<u8> {
        let n = utf16le(name);
        let mut r = vec![0u8; NAME];
        r[NAME_LENGTH..NAME_LENGTH + 4].copy_from_slice(&(n.len() as u32).to_le_bytes());
        r.extend(n);
        while !r.len().is_multiple_of(4) {
            r.push(0);
        }
        if !last {
            let l = r.len() as u32;
            r[..4].copy_from_slice(&l.to_le_bytes());
        }
        r
    }

    #[test]
    fn parents() {
        assert_eq!(parent_dir("C:\\w", "a.txt"), "C:\\w");
        assert_eq!(parent_dir("C:\\w\\", "sub\\a.txt"), "C:\\w\\sub");
        assert_eq!(parent_dir("C:\\", "x\\y\\z"), "C:\\x\\y");
        assert_eq!(parent_dir("C:\\", "z"), "C:\\");
        let mut buf = rec("sub\\new.txt", false);
        buf.extend(rec("top.txt", true));
        let mut got = Vec::new();
        for_each_parent(&buf, "C:\\w", |p| got.push(p));
        assert_eq!(got, ["C:\\w\\sub", "C:\\w"]);
        let mut n = 0;
        for_each_parent(&buf[..20], "C:\\w", |_| n += 1);
        assert_eq!(n, 0);
    }
}
