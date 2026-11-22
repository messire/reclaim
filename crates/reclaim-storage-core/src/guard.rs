//! Path safety policy for every delete or move Reclaim performs (ACCEPTANCE §5).
//!
//! Two checks, both pure and evaluated on normalized paths:
//! - [`Guard::check_cleanup`]: a cleanup target must lie strictly inside the allowed
//!   root of its category, never inside a protected tree, never be a protected folder
//!   itself and never contain one.
//! - [`Guard::check_user_delete`]: a file or folder the user picked in the analyzer may
//!   be moved to the Recycle Bin unless it is a system location, a volume root, a
//!   protected folder itself or an ancestor of one.
//!
//! The platform adapter evaluates the same checks a second time on the *final* path of
//! the opened handle right before deleting (revalidation), so a junction or a renamed
//! parent cannot redirect the operation.

use std::fmt;

use crate::path::{NormPath, PathStyle};

/// Known folders the policy and the cleanup rules refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Folder {
    /// `%LOCALAPPDATA%` (`~/.cache` on POSIX).
    LocalAppData,
    /// `%APPDATA%` (`~/.config` on POSIX).
    RoamingAppData,
    /// `%TEMP%` of the user.
    Temp,
    /// `%USERPROFILE%` (`$HOME`).
    Profile,
    /// Downloads.
    Downloads,
    /// Documents.
    Documents,
    /// Desktop.
    Desktop,
    /// Pictures.
    Pictures,
    /// Music.
    Music,
    /// Videos.
    Videos,
    /// OneDrive root, when present.
    OneDrive,
    /// `%WINDIR%`.
    Windows,
    /// `%ProgramFiles%`.
    ProgramFiles,
    /// `%ProgramFiles(x86)%`.
    ProgramFilesX86,
    /// `%ProgramData%`.
    ProgramData,
    /// `%SystemDrive%\` (the volume root).
    SystemDrive,
    /// `%LOCALAPPDATA%Low` (`AppData\LocalLow`).
    LocalLow,
}

/// Resolved known folders (display paths).
#[derive(Debug, Clone, Default)]
pub struct KnownFolders {
    entries: Vec<(Folder, String)>,
}

impl KnownFolders {
    /// Builds from pairs; later duplicates are ignored.
    pub fn new(entries: impl IntoIterator<Item = (Folder, String)>) -> Self {
        let mut k = Self::default();
        for (f, p) in entries {
            if !p.is_empty() && k.get(f).is_none() {
                k.entries.push((f, p));
            }
        }
        k
    }

    /// The path of a folder.
    pub fn get(&self, f: Folder) -> Option<&str> {
        self.entries
            .iter()
            .find(|(g, _)| *g == f)
            .map(|(_, p)| p.as_str())
    }

    /// All entries.
    pub fn iter(&self) -> impl Iterator<Item = (Folder, &str)> {
        self.entries.iter().map(|(f, p)| (*f, p.as_str()))
    }
}

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// The path could not be normalized (relative, dotted, device path).
    BadPath,
    /// Not strictly inside the category's allowed root.
    OutsideRoot,
    /// A volume root.
    VolumeRoot,
    /// The path is a protected folder itself.
    ProtectedFolder(String),
    /// The path lies inside a protected location.
    InsideProtected(String),
    /// The path contains a protected folder.
    ContainsProtected(String),
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Denied::BadPath => f.write_str("not a valid absolute path"),
            Denied::OutsideRoot => f.write_str("outside the cleanup location"),
            Denied::VolumeRoot => f.write_str("a drive root"),
            Denied::ProtectedFolder(p) => write!(f, "protected folder {p}"),
            Denied::InsideProtected(p) => write!(f, "inside protected location {p}"),
            Denied::ContainsProtected(p) => write!(f, "contains protected folder {p}"),
        }
    }
}

/// The protected-path policy.
#[derive(Debug, Clone)]
pub struct Guard {
    style: PathStyle,
    /// Folders that may never be deleted themselves (their contents may be, by rule).
    exact: Vec<NormPath>,
    /// Trees nothing inside of may be deleted by cleanup or by the user.
    trees: Vec<NormPath>,
    /// File names at a volume root that are never touched.
    root_files: Vec<&'static str>,
}

const WINDOWS_ROOT_FILES: &[&str] = &[
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "dumpstack.log",
    "dumpstack.log.tmp",
    "bootmgr",
    "bootsect.bak",
    "$recycle.bin",
    "system volume information",
    "recovery",
    "boot",
    "efi",
    "$windows.~bt",
    "$windows.~ws",
    "$winreagent",
];

impl Guard {
    fn norm_all(style: PathStyle, paths: impl IntoIterator<Item = String>) -> Vec<NormPath> {
        paths
            .into_iter()
            .filter_map(|p| NormPath::new(&p, style).ok())
            .collect()
    }

    /// Windows policy from known folders.
    pub fn windows(k: &KnownFolders) -> Self {
        use Folder::*;
        let style = PathStyle::Windows;
        let get = |f| k.get(f).map(str::to_owned);
        let exact = [
            LocalAppData,
            RoamingAppData,
            LocalLow,
            Temp,
            Profile,
            Downloads,
            Documents,
            Desktop,
            Pictures,
            Music,
            Videos,
            OneDrive,
            Windows,
            ProgramFiles,
            ProgramFilesX86,
            ProgramData,
            SystemDrive,
        ]
        .into_iter()
        .filter_map(get)
        .collect::<Vec<_>>();
        let mut exact = Self::norm_all(style, exact);
        // The parent of the profile (C:\Users) is protected too.
        if let Some(p) = k
            .get(Profile)
            .and_then(|p| NormPath::new(p, style).ok())
            .and_then(|p| p.parent())
        {
            exact.push(p);
        }
        let mut trees: Vec<String> = [
            Documents,
            Desktop,
            Pictures,
            Music,
            Videos,
            OneDrive,
            Windows,
            ProgramFiles,
            ProgramFilesX86,
            ProgramData,
        ]
        .into_iter()
        .filter_map(get)
        .collect();
        if let Some(sd) = k.get(SystemDrive) {
            let sd = sd.trim_end_matches('\\');
            for n in [
                "$Recycle.Bin",
                "System Volume Information",
                "Recovery",
                "Boot",
                "EFI",
            ] {
                trees.push(format!("{sd}\\{n}"));
            }
        }
        if let Some(l) = k.get(LocalAppData) {
            // Store apps' and the shell's own data, and installed per-user programs.
            for n in ["Packages", "Programs", "Microsoft\\WindowsApps"] {
                trees.push(format!("{}\\{n}", l.trim_end_matches('\\')));
            }
        }
        Self {
            style,
            exact,
            trees: Self::norm_all(style, trees),
            root_files: WINDOWS_ROOT_FILES.to_vec(),
        }
    }

    /// POSIX policy (Linux, macOS) from known folders plus the system directories.
    pub fn posix(k: &KnownFolders) -> Self {
        use Folder::*;
        let style = PathStyle::Posix;
        let mut exact: Vec<String> = [
            LocalAppData,
            RoamingAppData,
            Temp,
            Profile,
            Downloads,
            Documents,
            Desktop,
            Pictures,
            Music,
            Videos,
        ]
        .into_iter()
        .filter_map(|f| k.get(f).map(str::to_owned))
        .collect();
        exact.push("/home".into());
        exact.push("/tmp".into());
        let mut trees: Vec<String> = [Documents, Desktop, Pictures, Music, Videos]
            .into_iter()
            .filter_map(|f| k.get(f).map(str::to_owned))
            .collect();
        for s in [
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/lib",
            "/lib32",
            "/lib64",
            "/opt",
            "/proc",
            "/root",
            "/sbin",
            "/srv",
            "/sys",
            "/usr",
            "/var",
            "/System",
            "/Library",
            "/Applications",
        ] {
            trees.push(s.into());
        }
        Self {
            style,
            exact: Self::norm_all(style, exact),
            trees: Self::norm_all(style, trees),
            root_files: Vec::new(),
        }
    }

    /// Path style.
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// Normalizes a path for checks.
    pub fn norm(&self, p: &str) -> Result<NormPath, Denied> {
        NormPath::new(p, self.style).map_err(|_| Denied::BadPath)
    }

    fn common(&self, t: &NormPath) -> Result<(), Denied> {
        if t.is_root() {
            return Err(Denied::VolumeRoot);
        }
        if let Some(p) = self.exact.iter().find(|p| *p == t) {
            return Err(Denied::ProtectedFolder(p.to_string()));
        }
        if let Some(p) = self.trees.iter().find(|p| t.starts_with(p)) {
            return Err(Denied::InsideProtected(p.to_string()));
        }
        if let Some(p) = self
            .exact
            .iter()
            .chain(&self.trees)
            .find(|p| p.is_strictly_inside(t))
        {
            return Err(Denied::ContainsProtected(p.to_string()));
        }
        if let Some(parent) = t.parent()
            && parent.is_root()
            && self.root_files.contains(&t.file_name())
        {
            return Err(Denied::InsideProtected(t.to_string()));
        }
        Ok(())
    }

    /// Whether a cleanup rule may delete `target` (inside `allowed_root`).
    pub fn check_cleanup(&self, target: &str, allowed_root: &str) -> Result<(), Denied> {
        let t = self.norm(target)?;
        let root = self.norm(allowed_root)?;
        if !t.is_strictly_inside(&root) {
            return Err(Denied::OutsideRoot);
        }
        self.common(&t)
    }

    /// Whether an allowed root itself is acceptable for a cleanup rule (it is never
    /// deleted, only its contents): not a volume root and not inside a protected tree.
    pub fn check_root(&self, allowed_root: &str) -> Result<(), Denied> {
        let r = self.norm(allowed_root)?;
        if r.is_root() {
            return Err(Denied::VolumeRoot);
        }
        if let Some(p) = self.trees.iter().find(|p| r.starts_with(p)) {
            return Err(Denied::InsideProtected(p.to_string()));
        }
        Ok(())
    }

    /// Whether the user may move `target` to the Recycle Bin (or delete it) from the
    /// analyzer or the duplicate finder.
    pub fn check_user_delete(&self, target: &str) -> Result<(), Denied> {
        let t = self.norm(target)?;
        self.common(&t)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn win_folders() -> KnownFolders {
        use Folder::*;
        KnownFolders::new([
            (LocalAppData, "C:\\Users\\me\\AppData\\Local".to_owned()),
            (RoamingAppData, "C:\\Users\\me\\AppData\\Roaming".to_owned()),
            (LocalLow, "C:\\Users\\me\\AppData\\LocalLow".to_owned()),
            (Temp, "C:\\Users\\me\\AppData\\Local\\Temp".to_owned()),
            (Profile, "C:\\Users\\me".to_owned()),
            (Downloads, "C:\\Users\\me\\Downloads".to_owned()),
            (Documents, "C:\\Users\\me\\Documents".to_owned()),
            (Desktop, "C:\\Users\\me\\Desktop".to_owned()),
            (Pictures, "C:\\Users\\me\\Pictures".to_owned()),
            (Music, "C:\\Users\\me\\Music".to_owned()),
            (Videos, "C:\\Users\\me\\Videos".to_owned()),
            (OneDrive, "C:\\Users\\me\\OneDrive".to_owned()),
            (Windows, "C:\\Windows".to_owned()),
            (ProgramFiles, "C:\\Program Files".to_owned()),
            (ProgramFilesX86, "C:\\Program Files (x86)".to_owned()),
            (ProgramData, "C:\\ProgramData".to_owned()),
            (SystemDrive, "C:\\".to_owned()),
        ])
    }

    #[test]
    fn known_folders() {
        let k = KnownFolders::new([
            (Folder::Temp, "a".to_owned()),
            (Folder::Temp, "b".to_owned()),
            (Folder::Music, String::new()),
        ]);
        assert_eq!(k.get(Folder::Temp), Some("a"));
        assert_eq!(k.get(Folder::Music), None);
        assert_eq!(k.iter().count(), 1);
    }

    #[test]
    fn cleanup_inside_root_only() {
        let g = Guard::windows(&win_folders());
        let temp = "C:\\Users\\me\\AppData\\Local\\Temp";
        assert_eq!(
            g.check_cleanup("C:\\Users\\me\\AppData\\Local\\Temp\\x.tmp", temp),
            Ok(())
        );
        assert_eq!(
            g.check_cleanup("c:/users/ME/appdata/local/temp/sub/y", temp),
            Ok(())
        );
        assert_eq!(g.check_cleanup(temp, temp), Err(Denied::OutsideRoot));
        assert_eq!(
            g.check_cleanup("C:\\Users\\me\\AppData\\Local\\TempX\\a", temp),
            Err(Denied::OutsideRoot)
        );
        assert_eq!(
            g.check_cleanup(
                "C:\\Users\\me\\AppData\\Local\\Temp\\..\\..\\Roaming\\x",
                temp
            ),
            Err(Denied::BadPath)
        );
        assert_eq!(g.check_cleanup("Temp\\x", temp), Err(Denied::BadPath));
    }

    #[test]
    fn protected_trees_win_over_rules() {
        let g = Guard::windows(&win_folders());
        // A misconfigured rule pointing into Documents is still refused.
        let r = g.check_cleanup(
            "C:\\Users\\me\\Documents\\cache\\a",
            "C:\\Users\\me\\Documents\\cache",
        );
        assert!(matches!(r, Err(Denied::InsideProtected(_))));
        assert!(matches!(
            g.check_cleanup("C:\\Windows\\Temp\\a", "C:\\Windows\\Temp"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_root("C:\\Windows\\Temp"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(g.check_root("C:\\"), Err(Denied::VolumeRoot)));
        assert_eq!(g.check_root("C:\\Users\\me\\AppData\\Local\\Temp"), Ok(()));
        let pkg = g.check_cleanup(
            "C:\\Users\\me\\AppData\\Local\\Packages\\App\\x",
            "C:\\Users\\me\\AppData\\Local",
        );
        assert!(matches!(pkg, Err(Denied::InsideProtected(_))));
    }

    #[test]
    fn user_delete_policy() {
        let g = Guard::windows(&win_folders());
        assert_eq!(g.check_user_delete("D:\\Games\\old.iso"), Ok(()));
        assert_eq!(
            g.check_user_delete("C:\\Users\\me\\Downloads\\big.zip"),
            Ok(())
        );
        assert_eq!(g.check_user_delete("C:\\Users\\me\\Videos2\\a.mkv"), Ok(()));
        assert_eq!(g.check_user_delete("D:\\"), Err(Denied::VolumeRoot));
        assert!(matches!(
            g.check_user_delete("C:\\Users\\me\\Downloads"),
            Err(Denied::ProtectedFolder(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\Users"),
            Err(Denied::ProtectedFolder(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\Windows\\System32\\x.dll"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\Program Files\\App"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\pagefile.sys"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\$Recycle.Bin\\S-1"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\Users\\me\\AppData"),
            Err(Denied::ContainsProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("C:\\Users\\me\\Pictures\\a.jpg"),
            Err(Denied::InsideProtected(_))
        ));
        assert_eq!(g.check_user_delete("relative"), Err(Denied::BadPath));
        assert_eq!(g.style(), PathStyle::Windows);
    }

    #[test]
    fn posix_policy() {
        let k = KnownFolders::new([
            (Folder::Profile, "/home/me".to_owned()),
            (Folder::LocalAppData, "/home/me/.cache".to_owned()),
            (Folder::Documents, "/home/me/Documents".to_owned()),
        ]);
        let g = Guard::posix(&k);
        assert_eq!(
            g.check_cleanup("/home/me/.cache/pip/x", "/home/me/.cache/pip"),
            Ok(())
        );
        assert!(matches!(
            g.check_user_delete("/etc/passwd"),
            Err(Denied::InsideProtected(_))
        ));
        assert!(matches!(
            g.check_user_delete("/home/me"),
            Err(Denied::ProtectedFolder(_))
        ));
        assert!(matches!(
            g.check_user_delete("/home"),
            Err(Denied::ProtectedFolder(_))
        ));
        assert_eq!(g.check_user_delete("/home/me/big.iso"), Ok(()));
        assert!(matches!(g.check_user_delete("/"), Err(Denied::VolumeRoot)));
        assert!(
            Denied::ContainsProtected("x".into())
                .to_string()
                .contains("x")
        );
    }
}
