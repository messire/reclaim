//! Export of scan results to JSON and CSV.
//!
//! Both formats state whether the scan is complete, list every unreadable location
//! (up to the index's error cap, with the number dropped) and give both sizes. Nothing
//! is silently left out: excluded, unreadable and unlisted directories appear with
//! their flags.

use std::io::{self, Write};

use serde::Serialize;

use reclaim_storage_core::entry::{Attrs, Kind};
use reclaim_storage_core::tree::{NodeId, TOP, Tree};

use crate::scan::{ScanState, ScanSummary};

/// What to include.
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    /// Include files (otherwise directories only).
    pub files: bool,
    /// Maximum depth below the roots (`None`: everything).
    pub max_depth: Option<usize>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            files: true,
            max_depth: None,
        }
    }
}

fn flags(a: Attrs) -> Vec<&'static str> {
    let mut v = Vec::new();
    for (bit, name) in [
        (Attrs::ERROR, "unreadable"),
        (Attrs::INCOMPLETE, "not_scanned"),
        (Attrs::EXCLUDED, "excluded"),
        (Attrs::OTHER_VOLUME, "other_volume"),
        (Attrs::HARDLINK, "hardlink"),
        (Attrs::HARDLINK_DUP, "hardlink_counted_elsewhere"),
        (Attrs::COMPRESSED, "compressed"),
        (Attrs::SPARSE, "sparse"),
        (Attrs::REPARSE, "reparse_point"),
        (Attrs::OFFLINE, "offline"),
        (Attrs::HIDDEN, "hidden"),
        (Attrs::SYSTEM, "system"),
    ] {
        if a.has(bit) {
            v.push(name);
        }
    }
    v
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::File => "file",
        Kind::Dir => "dir",
        Kind::Link => "link",
        Kind::Other => "other",
    }
}

fn state(s: Option<&ScanSummary>) -> (&'static str, bool) {
    match s {
        None => ("unknown", false),
        Some(s) => {
            let st = match s.state {
                ScanState::Complete => "complete",
                ScanState::Cancelled => "cancelled",
                ScanState::Running => "running",
                ScanState::Paused => "paused",
            };
            (st, s.state == ScanState::Complete && !s.partial)
        }
    }
}

fn visit(
    t: &Tree,
    o: &ExportOptions,
    mut f: impl FnMut(NodeId, &str) -> io::Result<()>,
) -> io::Result<()> {
    let mut err = Ok(());
    t.walk_paths(TOP, |id, path| {
        if id == TOP {
            return true;
        }
        let n = t.node(id);
        if !o.files && !n.is_dir() {
            return true;
        }
        if o.max_depth.is_some_and(|d| t.depth(id) > d + 1) {
            return true;
        }
        match f(id, path) {
            Ok(()) => true,
            Err(e) => {
                err = Err(e);
                false
            }
        }
    });
    err
}

#[derive(Serialize)]
struct Header<'a> {
    format: &'a str,
    tool: &'a str,
    version: &'a str,
    generated_unix_ms: u64,
    state: &'a str,
    complete: bool,
    roots: Vec<String>,
    totals: Totals,
    errors: Errors,
}

#[derive(Serialize)]
struct Totals {
    logical_bytes: u64,
    allocated_bytes: u64,
    files: u64,
    dirs: u64,
    unreadable_dirs: u64,
    hardlink_names_counted_once: usize,
    allocated_estimated: bool,
}

#[derive(Serialize)]
struct Errors {
    kept: usize,
    dropped: u64,
    items: Vec<ErrorItem>,
}

#[derive(Serialize)]
struct ErrorItem {
    path: String,
    kind: String,
}

#[derive(Serialize)]
struct Entry<'a> {
    path: &'a str,
    kind: &'a str,
    logical_bytes: u64,
    allocated_bytes: u64,
    files: u64,
    dirs: u64,
    modified_unix: i64,
    flags: Vec<&'a str>,
}

fn header<'a>(t: &Tree, s: Option<&ScanSummary>, version: &'a str, now_ms: u64) -> Header<'a> {
    let (st, complete) = state(s);
    let top = t.node(TOP);
    let c = t.counts(TOP);
    Header {
        format: "reclaim-storage-scan/1",
        tool: "Reclaim",
        version,
        generated_unix_ms: now_ms,
        state: st,
        complete,
        roots: t.roots().into_iter().map(|r| t.path(r)).collect(),
        totals: Totals {
            logical_bytes: top.logical,
            allocated_bytes: top.allocated,
            files: c.files,
            dirs: c.dirs,
            unreadable_dirs: c.errors,
            hardlink_names_counted_once: t.hardlink_duplicates(),
            allocated_estimated: t.allocated_estimated(),
        },
        errors: Errors {
            kept: t.errors().len(),
            dropped: t.errors_dropped(),
            items: t
                .errors()
                .iter()
                .map(|e| ErrorItem {
                    path: e.path.clone(),
                    kind: e.kind.id(),
                })
                .collect(),
        },
    }
}

/// Writes JSON: one header object with `entries` streamed as an array.
pub fn json(
    t: &Tree,
    summary: Option<&ScanSummary>,
    version: &str,
    now_ms: u64,
    o: &ExportOptions,
    w: &mut dyn Write,
) -> io::Result<()> {
    let h =
        serde_json::to_string(&header(t, summary, version, now_ms)).map_err(io::Error::other)?;
    // Open the header object and append the entries array before its closing brace.
    let head = h.strip_suffix('}').unwrap_or(&h);
    write!(w, "{head},\"entries\":[")?;
    let mut first = true;
    visit(t, o, |id, path| {
        let n = t.node(id);
        let c = t.counts(id);
        let e = Entry {
            path,
            kind: kind(n.kind),
            logical_bytes: n.logical,
            allocated_bytes: n.allocated,
            files: c.files,
            dirs: c.dirs,
            modified_unix: n.mtime(),
            flags: flags(n.attrs),
        };
        if !first {
            w.write_all(b",\n")?;
        }
        first = false;
        serde_json::to_writer(&mut *w, &e).map_err(io::Error::other)
    })?;
    w.write_all(b"]}\n")
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(['=', '+', '-', '@']) {
        // Quote; a leading formula character is neutralised for spreadsheet safety.
        let s = if s.starts_with(['=', '+', '-', '@']) {
            format!("'{s}")
        } else {
            s.to_owned()
        };
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// Writes CSV with `#` metadata lines first (state, completeness, totals), then one
/// row per entry, then one `error` row per unreadable location.
pub fn csv(
    t: &Tree,
    summary: Option<&ScanSummary>,
    version: &str,
    now_ms: u64,
    o: &ExportOptions,
    w: &mut dyn Write,
) -> io::Result<()> {
    let h = header(t, summary, version, now_ms);
    writeln!(
        w,
        "# Reclaim {version} storage scan, format reclaim-storage-scan/1"
    )?;
    writeln!(w, "# state={} complete={}", h.state, h.complete)?;
    writeln!(w, "# roots={}", h.roots.join(" | "))?;
    writeln!(
        w,
        "# logical_bytes={} allocated_bytes={} files={} dirs={} unreadable_dirs={} errors_kept={} errors_dropped={} allocated_estimated={}",
        h.totals.logical_bytes,
        h.totals.allocated_bytes,
        h.totals.files,
        h.totals.dirs,
        h.totals.unreadable_dirs,
        h.errors.kept,
        h.errors.dropped,
        h.totals.allocated_estimated
    )?;
    writeln!(
        w,
        "path,kind,logical_bytes,allocated_bytes,files,dirs,modified_unix,flags"
    )?;
    visit(t, o, |id, path| {
        let n = t.node(id);
        let c = t.counts(id);
        writeln!(
            w,
            "{},{},{},{},{},{},{},{}",
            csv_field(path),
            kind(n.kind),
            n.logical,
            n.allocated,
            c.files,
            c.dirs,
            n.mtime(),
            flags(n.attrs).join("|")
        )
    })?;
    for e in t.errors() {
        writeln!(w, "{},error,,,,,,{}", csv_field(&e.path), e.kind.id())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFs;
    use crate::scan::{ScanControl, ScanOptions, ScanTarget, scan_blocking};
    use reclaim_storage_core::path::PathStyle;
    use std::sync::{Arc, RwLock};

    fn scanned() -> (Arc<RwLock<Tree>>, ScanSummary) {
        let mut f = FakeFs::sample();
        f.deny("/r/b/e");
        f.add_file("/r", "we,ird\"name", 3);
        f.add_file("/r", "=cmd", 3);
        let t = Arc::new(RwLock::new(Tree::new(PathStyle::Posix)));
        let s = scan_blocking(
            &f,
            &t,
            ScanTarget::Roots(vec!["/r".into()]),
            &ScanOptions::default(),
            &ScanControl::default(),
        );
        (t, s)
    }

    #[test]
    fn json_is_valid_and_honest() {
        let (t, s) = scanned();
        let t = t.read().unwrap();
        let mut out = Vec::new();
        json(
            &t,
            Some(&s),
            "9.9.9",
            5,
            &ExportOptions::default(),
            &mut out,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["state"], "complete");
        assert_eq!(v["complete"], false); // one directory unreadable
        assert_eq!(v["errors"]["kept"], 1);
        assert_eq!(v["errors"]["items"][0]["kind"], "access_denied");
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 8);
        let e = entries.iter().find(|e| e["path"] == "/r/b/e").unwrap();
        assert_eq!(e["flags"][0], "unreadable");
        assert_eq!(v["totals"]["logical_bytes"], 1 + 20 + 300 + 6);

        let mut dirs = Vec::new();
        let o = ExportOptions {
            files: false,
            max_depth: Some(1),
        };
        json(&t, None, "x", 0, &o, &mut dirs).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&dirs).unwrap();
        assert_eq!(v["state"], "unknown");
        assert_eq!(v["entries"].as_array().unwrap().len(), 2); // /r and /r/b
    }

    #[test]
    fn csv_quotes_and_lists_errors() {
        let (t, s) = scanned();
        let t = t.read().unwrap();
        let mut out = Vec::new();
        csv(
            &t,
            Some(&s),
            "9.9.9",
            5,
            &ExportOptions::default(),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("# state=complete complete=false"));
        assert!(text.contains("\"/r/we,ird\"\"name\",file,3"));
        assert!(text.lines().any(|l| l == "/r/b/e,error,,,,,,access_denied"));
        assert!(text.contains("path,kind,logical_bytes"));
        assert_eq!(csv_field("=x"), "\"'=x\"");
        assert_eq!(csv_field("plain"), "plain");
    }
}
