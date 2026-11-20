//! Local persistence: settings (TOML), run history (JSONL) and the optional log
//! (ARCHITECTURE §9). Nothing leaves the machine.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use reclaim_core::outcome::RunRecord;
use reclaim_core::policy::{ExclusionRule, Policy};
use reclaim_core::units::Bytes;

/// Current settings schema.
pub const SETTINGS_SCHEMA: u32 = 1;
/// History keeps at most this many runs.
pub const HISTORY_MAX_RUNS: usize = 100;
/// History file size cap.
pub const HISTORY_MAX_BYTES: u64 = 1024 * 1024;
/// Log file size before rotation.
pub const LOG_MAX_BYTES: u64 = 512 * 1024;

/// User settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version of this file.
    pub schema_version: u32,
    /// Skip processes with a smaller working set (MiB).
    pub min_working_set_mib: u64,
    /// User exclusions (image names).
    pub exclusions: Vec<String>,
    /// Caution images the user opted in to.
    pub allowed_caution: Vec<String>,
    /// Ask before every run.
    pub confirm_runs: bool,
    /// Store run history.
    pub keep_history: bool,
    /// Write errors to a local log file.
    pub log_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA,
            min_working_set_mib: 32,
            exclusions: Vec::new(),
            allowed_caution: Vec::new(),
            confirm_runs: true,
            keep_history: true,
            log_enabled: false,
        }
    }
}

impl Settings {
    /// The policy these settings describe.
    pub fn policy(&self) -> Policy {
        let mut p = Policy {
            min_working_set: Bytes::from_mib(self.min_working_set_mib),
            exclusions: self
                .exclusions
                .iter()
                .map(|e| ExclusionRule::image(e))
                .collect(),
            ..Policy::default()
        };
        for c in &self.allowed_caution {
            p.allow_caution(c);
        }
        p
    }
}

/// Why settings fell back to defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsWarning {
    /// The file could not be parsed; defaults are used and the file is kept as `.bad`.
    Corrupt(String),
    /// The file was written by a newer Reclaim; defaults are used, the file is untouched.
    Newer(u32),
}

/// Local storage rooted at a directory (`%LOCALAPPDATA%\Reclaim` in the app).
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

/// Writes a file atomically: temp file in the same directory, then rename over the target.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

impl Store {
    /// Uses `dir`, creating it on first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Default location: `%LOCALAPPDATA%\Reclaim` (or `$XDG_DATA_HOME/reclaim`, `~/.local/share/reclaim`).
    pub fn default_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(d).join("Reclaim");
        }
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(d).join("reclaim");
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/share/reclaim"))
            .unwrap_or_else(|| PathBuf::from("reclaim-data"))
    }

    /// The storage directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Loads settings; a missing file gives defaults silently, a broken one gives defaults
    /// and a warning.
    pub fn load_settings(&self) -> (Settings, Option<SettingsWarning>) {
        let path = self.path("settings.toml");
        let Ok(text) = fs::read_to_string(&path) else {
            return (Settings::default(), None);
        };
        match toml::from_str::<Settings>(&text) {
            Ok(s) if s.schema_version > SETTINGS_SCHEMA => (
                Settings::default(),
                Some(SettingsWarning::Newer(s.schema_version)),
            ),
            Ok(s) => (s, None),
            Err(e) => {
                let _ = fs::rename(&path, path.with_extension("toml.bad"));
                (
                    Settings::default(),
                    Some(SettingsWarning::Corrupt(e.message().to_owned())),
                )
            }
        }
    }

    /// Saves settings atomically.
    pub fn save_settings(&self, s: &Settings) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let text = toml::to_string(s).map_err(std::io::Error::other)?;
        write_atomic(&self.path("settings.toml"), text.as_bytes())
    }

    /// Appends a run, then trims history to the caps (oldest first).
    pub fn append_history(&self, r: &RunRecord) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path("history.jsonl");
        {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            writeln!(f, "{}", r.to_json())?;
        }
        let text = fs::read_to_string(&path)?;
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut keep = lines.len().saturating_sub(HISTORY_MAX_RUNS);
        let mut size: u64 = lines[keep..].iter().map(|l| l.len() as u64 + 1).sum();
        while size > HISTORY_MAX_BYTES && keep < lines.len() - 1 {
            size -= lines[keep].len() as u64 + 1;
            keep += 1;
        }
        if keep > 0 {
            let mut out = lines[keep..].join("\n");
            out.push('\n');
            write_atomic(&path, out.as_bytes())?;
        }
        Ok(())
    }

    /// Reads history, newest last. Unreadable lines are skipped.
    pub fn history(&self) -> Vec<RunRecord> {
        fs::read_to_string(self.path("history.jsonl"))
            .map(|t| {
                t.lines()
                    .filter_map(|l| RunRecord::from_json(l).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Deletes history and logs.
    pub fn clear_history_and_logs(&self) -> std::io::Result<()> {
        for name in ["history.jsonl", "reclaim.log", "reclaim.log.1"] {
            match fs::remove_file(self.path(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    /// Appends a line to the log, rotating at [`LOG_MAX_BYTES`] (two files).
    pub fn log(&self, line: &str) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path("reclaim.log");
        if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > LOG_MAX_BYTES {
            let _ = fs::rename(&path, self.path("reclaim.log.1"));
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_core::model::SystemSnapshot;
    use reclaim_core::outcome::RunMode;
    use reclaim_core::plan::Plan;
    use reclaim_core::units::Millis;

    fn tmp(name: &str) -> Store {
        let d = std::env::temp_dir().join(format!("reclaim-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        Store::new(d)
    }

    fn record(i: u64) -> RunRecord {
        let plan = Plan {
            id: i,
            lists_version: 1,
            targets: vec![],
            skipped: vec![],
        };
        RunRecord::new(plan, RunMode::Trim, Millis(i), SystemSnapshot::default())
    }

    #[test]
    fn settings_round_trip_and_fallbacks() {
        let st = tmp("settings");
        assert_eq!(st.load_settings(), (Settings::default(), None));
        let s = Settings {
            min_working_set_mib: 64,
            exclusions: vec!["game.exe".into()],
            allowed_caution: vec!["explorer.exe".into()],
            log_enabled: true,
            ..Settings::default()
        };
        st.save_settings(&s).unwrap();
        assert_eq!(st.load_settings(), (s.clone(), None));
        let p = s.policy();
        assert_eq!(p.min_working_set, Bytes::from_mib(64));
        assert_eq!(p.exclusions, vec![ExclusionRule::image("game.exe")]);
        assert!(p.allowed_caution.contains("explorer.exe"));

        fs::write(st.dir().join("settings.toml"), "schema_version = 9\n").unwrap();
        assert_eq!(st.load_settings().1, Some(SettingsWarning::Newer(9)));
        assert!(
            st.dir().join("settings.toml").exists(),
            "newer file untouched"
        );

        fs::write(
            st.dir().join("settings.toml"),
            "min_working_set_mib = \"x\"",
        )
        .unwrap();
        let (loaded, warn) = st.load_settings();
        assert_eq!(loaded, Settings::default());
        assert!(matches!(warn, Some(SettingsWarning::Corrupt(_))));
        assert!(st.dir().join("settings.toml.bad").exists());

        // Missing fields take defaults (forward compatible within a schema).
        fs::write(st.dir().join("settings.toml"), "keep_history = false\n").unwrap();
        assert!(!st.load_settings().0.keep_history);
        let _ = fs::remove_dir_all(st.dir());
    }

    #[test]
    fn history_caps_and_clear() {
        let st = tmp("history");
        assert!(st.history().is_empty());
        for i in 0..105 {
            st.append_history(&record(i)).unwrap();
        }
        let h = st.history();
        assert_eq!(h.len(), HISTORY_MAX_RUNS);
        assert_eq!(h[0].plan.id, 5, "oldest dropped");
        assert_eq!(h.last().unwrap().plan.id, 104);
        fs::OpenOptions::new()
            .append(true)
            .open(st.dir().join("history.jsonl"))
            .unwrap()
            .write_all(b"garbage\n")
            .unwrap();
        assert_eq!(st.history().len(), HISTORY_MAX_RUNS, "bad lines skipped");
        st.log("error: x").unwrap();
        st.clear_history_and_logs().unwrap();
        assert!(st.history().is_empty());
        assert!(!st.dir().join("reclaim.log").exists());
        st.clear_history_and_logs().unwrap();
        let _ = fs::remove_dir_all(st.dir());
    }

    #[test]
    fn history_size_cap() {
        let st = tmp("history-size");
        let mut big = record(0);
        big.app_version = "x".repeat(300 * 1024);
        for _ in 0..5 {
            st.append_history(&big).unwrap();
        }
        let len = fs::metadata(st.dir().join("history.jsonl")).unwrap().len();
        assert!(len <= HISTORY_MAX_BYTES, "{len}");
        assert_eq!(st.history().len(), 3);
        let _ = fs::remove_dir_all(st.dir());
    }

    #[test]
    fn log_rotates() {
        let st = tmp("log");
        let line = "e".repeat(1024);
        for _ in 0..600 {
            st.log(&line).unwrap();
        }
        assert!(st.dir().join("reclaim.log.1").exists());
        assert!(fs::metadata(st.dir().join("reclaim.log")).unwrap().len() <= LOG_MAX_BYTES + 2048);
        let _ = fs::remove_dir_all(st.dir());
        assert!(
            Store::default_dir().ends_with("Reclaim")
                || Store::default_dir().to_string_lossy().contains("reclaim")
        );
    }
}
