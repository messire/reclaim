//! Local storage settings (`storage.toml`) and cleanup history (`cleanup-history.jsonl`),
//! next to the memory settings in `%LOCALAPPDATA%\Reclaim`. The history holds category
//! ids, counts, byte totals and free-space deltas, never file paths.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use reclaim_storage_core::cleanup::CleanupReport;

/// At most this many cleanup runs are kept.
pub const HISTORY_MAX: usize = 200;

/// Storage settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    /// Schema version.
    pub schema_version: u32,
    /// Scan exclusions (paths or name globs).
    pub scan_exclusions: Vec<String>,
    /// Cleanup exclusions (paths or name globs).
    pub cleanup_exclusions: Vec<String>,
    /// Worker threads for scans (0: chosen per drive).
    pub scan_threads: usize,
    /// Use the fast NTFS scan for whole drives when Reclaim runs as administrator.
    pub fast_ntfs_scan: bool,
    /// Show sizes on disk (allocated) rather than file lengths.
    pub allocated_sizes: bool,
    /// Duplicate finder: ignore files smaller than this many KiB.
    pub duplicate_min_kib: u64,
    /// Keep a local cleanup history.
    pub keep_cleanup_history: bool,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scan_exclusions: Vec::new(),
            cleanup_exclusions: Vec::new(),
            scan_threads: 0,
            fast_ntfs_scan: true,
            allocated_sizes: true,
            duplicate_min_kib: 1024,
            keep_cleanup_history: true,
        }
    }
}

/// Files under one directory.
#[derive(Debug, Clone)]
pub struct StorageStore {
    dir: PathBuf,
}

impl StorageStore {
    /// Uses `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("storage.toml")
    }

    fn history_path(&self) -> PathBuf {
        self.dir.join("cleanup-history.jsonl")
    }

    /// Loads settings; a missing or unreadable file gives defaults (and the reason).
    pub fn load(&self) -> (StorageSettings, Option<String>) {
        match fs::read_to_string(self.settings_path()) {
            Err(_) => (StorageSettings::default(), None),
            Ok(s) => match toml::from_str(&s) {
                Ok(v) => (v, None),
                Err(e) => (StorageSettings::default(), Some(e.to_string())),
            },
        }
    }

    /// Saves settings atomically.
    pub fn save(&self, s: &StorageSettings) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let text = toml::to_string(s).map_err(std::io::Error::other)?;
        write_atomic(&self.settings_path(), text.as_bytes())
    }

    /// Appends a cleanup report (paths stripped) and trims the file to the last
    /// [`HISTORY_MAX`] runs.
    pub fn append_history(&self, r: &CleanupReport) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let line = serde_json::to_string(&r.for_history()).map_err(std::io::Error::other)?;
        let mut lines = self.history_lines();
        lines.push(line);
        let start = lines.len().saturating_sub(HISTORY_MAX);
        let mut text = lines[start..].join("\n");
        text.push('\n');
        write_atomic(&self.history_path(), text.as_bytes())
    }

    fn history_lines(&self) -> Vec<String> {
        match fs::File::open(self.history_path()) {
            Err(_) => Vec::new(),
            Ok(f) => std::io::BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .filter(|l| !l.trim().is_empty())
                .collect(),
        }
    }

    /// Stored runs, oldest first (unparseable lines are skipped).
    pub fn history(&self) -> Vec<CleanupReport> {
        self.history_lines()
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Deletes the history file.
    pub fn clear_history(&self) -> std::io::Result<()> {
        match fs::remove_file(self.history_path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_storage_core::cleanup::CategoryResult;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "reclaim-storage-store-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn settings_round_trip_and_corrupt() {
        let d = dir("settings");
        let s = StorageStore::new(&d);
        assert_eq!(s.load(), (StorageSettings::default(), None));
        let mut v = StorageSettings::default();
        v.scan_exclusions.push("node_modules".into());
        v.scan_threads = 3;
        s.save(&v).unwrap();
        assert_eq!(s.load().0, v);
        fs::write(d.join("storage.toml"), "scan_threads = \"x\"").unwrap();
        let (v2, warn) = s.load();
        assert_eq!(v2, StorageSettings::default());
        assert!(warn.is_some());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn history_is_bounded_and_path_free() {
        let d = dir("history");
        let s = StorageStore::new(&d);
        let r = CleanupReport {
            categories: vec![CategoryResult {
                id: "user-temp".into(),
                deleted: 2,
                notes: vec!["skipped: C:\\secret\\path".into()],
                ..CategoryResult::default()
            }],
            ..CleanupReport::default()
        };
        for _ in 0..HISTORY_MAX + 5 {
            s.append_history(&r).unwrap();
        }
        let h = s.history();
        assert_eq!(h.len(), HISTORY_MAX);
        assert!(h[0].categories[0].notes.is_empty());
        let text = fs::read_to_string(d.join("cleanup-history.jsonl")).unwrap();
        assert!(!text.contains("secret"));
        s.clear_history().unwrap();
        s.clear_history().unwrap();
        assert!(s.history().is_empty());
        let _ = fs::remove_dir_all(&d);
    }
}
