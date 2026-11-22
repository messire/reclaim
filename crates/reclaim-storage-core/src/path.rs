//! Path normalization and containment checks with explicit platform semantics.
//!
//! The safety policy (`crate::cleanup::guard`) compares paths as strings so that it is
//! pure and testable on any OS. Windows paths are compared case-insensitively with `\`
//! separators and without the `\\?\` prefix; POSIX paths case-sensitively with `/`.
//! Paths with `.` or `..` components are rejected rather than resolved: the policy never
//! guesses what a relative or dotted path means.

use std::fmt;

/// Path rules of a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// Drive letters and UNC, `\` separators, case-insensitive.
    Windows,
    /// `/` separators, case-sensitive.
    Posix,
}

impl PathStyle {
    /// The style of the platform this crate is compiled for.
    pub const fn native() -> Self {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }

    /// Preferred separator.
    pub const fn sep(self) -> char {
        match self {
            PathStyle::Windows => '\\',
            PathStyle::Posix => '/',
        }
    }

    fn is_sep(self, c: char) -> bool {
        match self {
            PathStyle::Windows => c == '\\' || c == '/',
            PathStyle::Posix => c == '/',
        }
    }
}

/// Why a path could not be normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Empty string.
    Empty,
    /// Not absolute (no drive or UNC prefix, or no leading `/`).
    Relative,
    /// Contains a `.` or `..` component.
    Dotted,
    /// Contains a NUL or another character that is never valid in a path.
    Invalid,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PathError::Empty => "empty path",
            PathError::Relative => "relative path",
            PathError::Dotted => "path with . or .. components",
            PathError::Invalid => "invalid character in path",
        })
    }
}

impl std::error::Error for PathError {}

/// An absolute, normalized path used for comparisons (never for display).
///
/// Windows: lower case, `\` separators, no `\\?\` prefix, no trailing separator except
/// for a drive root (`c:\`). UNC paths keep their `\\server\share` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NormPath {
    style_windows: bool,
    s: String,
}

impl NormPath {
    /// Normalizes `raw` under `style`.
    pub fn new(raw: &str, style: PathStyle) -> Result<Self, PathError> {
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.contains('\0') {
            return Err(PathError::Invalid);
        }
        match style {
            PathStyle::Windows => Self::windows(raw),
            PathStyle::Posix => Self::posix(raw),
        }
    }

    fn windows(raw: &str) -> Result<Self, PathError> {
        let mut s = raw.replace('/', "\\");
        // Long-path and device prefixes.
        if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
            s = format!("\\\\{rest}");
        } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
            s = rest.to_owned();
        } else if s.starts_with("\\\\.\\") {
            // Device namespace: never a cleanup or comparison target.
            return Err(PathError::Invalid);
        }
        let (prefix, rest) = if let Some(body) = s.strip_prefix("\\\\") {
            // \\server\share\rest
            let mut parts = body.splitn(3, '\\');
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            if server.is_empty() || share.is_empty() {
                return Err(PathError::Relative);
            }
            (
                format!("\\\\{server}\\{share}"),
                parts.next().unwrap_or_default().to_owned(),
            )
        } else {
            let b = s.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                if b.len() > 2 && b[2] != b'\\' {
                    return Err(PathError::Relative); // "C:foo" is drive-relative.
                }
                (s[..2].to_owned(), s.get(3..).unwrap_or_default().to_owned())
            } else {
                return Err(PathError::Relative);
            }
        };
        let mut out = prefix;
        let mut any = false;
        for comp in rest.split('\\').filter(|c| !c.is_empty()) {
            if comp == "." || comp == ".." {
                return Err(PathError::Dotted);
            }
            if comp
                .chars()
                .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
            {
                return Err(PathError::Invalid);
            }
            out.push('\\');
            out.push_str(comp);
            any = true;
        }
        if !any && !out.starts_with("\\\\") {
            out.push('\\'); // drive root keeps its separator
        }
        Ok(Self {
            style_windows: true,
            s: out.to_lowercase(),
        })
    }

    fn posix(raw: &str) -> Result<Self, PathError> {
        if !raw.starts_with('/') {
            return Err(PathError::Relative);
        }
        let mut out = String::with_capacity(raw.len());
        for comp in raw.split('/').filter(|c| !c.is_empty()) {
            if comp == "." || comp == ".." {
                return Err(PathError::Dotted);
            }
            out.push('/');
            out.push_str(comp);
        }
        if out.is_empty() {
            out.push('/');
        }
        Ok(Self {
            style_windows: false,
            s: out,
        })
    }

    /// The normalized string.
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// Path style.
    pub fn style(&self) -> PathStyle {
        if self.style_windows {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }

    /// Whether this is a volume root (`c:\`, `\\server\share`, `/`).
    pub fn is_root(&self) -> bool {
        if self.style_windows {
            (self.s.len() == 3 && self.s.ends_with(":\\"))
                || (self.s.starts_with("\\\\") && self.s[2..].matches('\\').count() == 1)
        } else {
            self.s == "/"
        }
    }

    /// Whether `self` is `other` or lies below it.
    pub fn starts_with(&self, other: &NormPath) -> bool {
        if self.style_windows != other.style_windows {
            return false;
        }
        if self.s == other.s {
            return true;
        }
        let sep = self.style().sep();
        let o = other.s.trim_end_matches(sep);
        self.s.starts_with(o) && self.s[o.len()..].starts_with(sep)
    }

    /// Whether `self` lies strictly below `other`.
    pub fn is_strictly_inside(&self, other: &NormPath) -> bool {
        self != other && self.starts_with(other)
    }

    /// The last component (empty for a root).
    pub fn file_name(&self) -> &str {
        if self.is_root() {
            return "";
        }
        let sep = self.style().sep();
        self.s.rsplit(sep).next().unwrap_or_default()
    }

    /// The parent path, if any.
    pub fn parent(&self) -> Option<NormPath> {
        if self.is_root() {
            return None;
        }
        let sep = self.style().sep();
        let i = self.s.rfind(sep)?;
        let mut p = self.s[..i].to_owned();
        if self.style_windows && p.len() == 2 && p.ends_with(':') {
            p.push('\\');
        }
        if !self.style_windows && p.is_empty() {
            p.push('/');
        }
        Some(Self {
            style_windows: self.style_windows,
            s: p,
        })
    }

    /// Appends one component (rejects separators and dotted names).
    pub fn join(&self, name: &str) -> Result<NormPath, PathError> {
        let sep = self.style().sep();
        if name.is_empty() || name == "." || name == ".." {
            return Err(PathError::Dotted);
        }
        if name.chars().any(|c| self.style().is_sep(c)) {
            return Err(PathError::Invalid);
        }
        let mut s = self.s.clone();
        if !s.ends_with(sep) {
            s.push(sep);
        }
        if self.style_windows {
            s.push_str(&name.to_lowercase());
        } else {
            s.push_str(name);
        }
        Ok(Self {
            style_windows: self.style_windows,
            s,
        })
    }
}

impl fmt::Display for NormPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.s)
    }
}

/// Case-insensitive (ASCII and Unicode simple folding) glob match with `*` and `?`, used
/// for exclusion patterns and path templates. Linear time in practice (no backtracking
/// blow-up: the classic two-pointer algorithm).
pub fn glob_match(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let fold = |s: &str| -> Vec<char> {
        if case_insensitive {
            s.chars().flat_map(char::to_lowercase).collect()
        } else {
            s.chars().collect()
        }
    };
    let p = fold(pattern);
    let t = fold(text);
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> NormPath {
        NormPath::new(s, PathStyle::Windows).unwrap()
    }

    #[test]
    fn windows_normalization() {
        assert_eq!(w("C:\\Users\\Me\\").as_str(), "c:\\users\\me");
        assert_eq!(w("c:/users//me").as_str(), "c:\\users\\me");
        assert_eq!(w("\\\\?\\C:\\Users").as_str(), "c:\\users");
        assert_eq!(w("\\\\?\\UNC\\srv\\share\\x").as_str(), "\\\\srv\\share\\x");
        assert_eq!(w("C:\\").as_str(), "c:\\");
        assert_eq!(w("C:").as_str(), "c:\\");
        assert!(w("C:\\").is_root());
        assert!(w("\\\\srv\\share").is_root());
        assert!(!w("\\\\srv\\share\\x").is_root());
    }

    #[test]
    fn windows_rejections() {
        let e = |s| NormPath::new(s, PathStyle::Windows).unwrap_err();
        assert_eq!(e(""), PathError::Empty);
        assert_eq!(e("Users\\x"), PathError::Relative);
        assert_eq!(e("C:foo"), PathError::Relative);
        assert_eq!(e("C:\\a\\..\\Windows"), PathError::Dotted);
        assert_eq!(e("C:\\a\\.\\b"), PathError::Dotted);
        assert_eq!(e("\\\\.\\PhysicalDrive0"), PathError::Invalid);
        assert_eq!(e("C:\\a*b"), PathError::Invalid);
        assert_eq!(e("C:\\a\0"), PathError::Invalid);
        assert_eq!(e("\\\\server"), PathError::Relative);
    }

    #[test]
    fn containment() {
        let users = w("C:\\Users");
        assert!(w("c:\\users\\me").starts_with(&users));
        assert!(w("C:\\USERS").starts_with(&users));
        assert!(!w("C:\\Users2").starts_with(&users));
        assert!(!w("C:\\Use").starts_with(&users));
        assert!(w("C:\\x").starts_with(&w("C:\\")));
        assert!(w("C:\\x").is_strictly_inside(&w("C:\\")));
        assert!(!users.is_strictly_inside(&users));
        assert!(!w("D:\\x").starts_with(&w("C:\\")));
        let p = NormPath::new("/home/me", PathStyle::Posix).unwrap();
        assert!(!p.starts_with(&users));
    }

    #[test]
    fn posix() {
        let p = |s| NormPath::new(s, PathStyle::Posix);
        assert_eq!(p("/a//b/").unwrap().as_str(), "/a/b");
        assert_eq!(p("/").unwrap().as_str(), "/");
        assert!(p("/").unwrap().is_root());
        assert_eq!(p("a/b").unwrap_err(), PathError::Relative);
        assert_eq!(p("/a/../b").unwrap_err(), PathError::Dotted);
        assert!(p("/A/b").unwrap() != p("/a/b").unwrap());
        assert!(p("/a/b").unwrap().starts_with(&p("/").unwrap()));
        assert!(!p("/ab").unwrap().starts_with(&p("/a").unwrap()));
    }

    #[test]
    fn parent_join_name() {
        assert_eq!(w("C:\\a\\b").parent().unwrap().as_str(), "c:\\a");
        assert_eq!(w("C:\\a").parent().unwrap().as_str(), "c:\\");
        assert!(w("C:\\").parent().is_none());
        assert_eq!(w("C:\\a\\Bc").file_name(), "bc");
        assert_eq!(w("C:\\").file_name(), "");
        assert_eq!(w("C:\\").join("Temp").unwrap().as_str(), "c:\\temp");
        assert_eq!(w("C:\\a").join("B").unwrap().as_str(), "c:\\a\\b");
        assert!(w("C:\\a").join("..").is_err());
        assert!(w("C:\\a").join("x\\y").is_err());
        let p = NormPath::new("/a", PathStyle::Posix).unwrap();
        assert_eq!(p.parent().unwrap().as_str(), "/");
        assert_eq!(p.join("B").unwrap().as_str(), "/a/B");
        assert_eq!(p.style(), PathStyle::Posix);
    }

    #[test]
    fn globbing() {
        assert!(glob_match("*.tmp", "a.TMP", true));
        assert!(!glob_match("*.tmp", "a.TMP", false));
        assert!(glob_match("Profile *", "Profile 12", true));
        assert!(glob_match("a?c", "abc", false));
        assert!(!glob_match("a?c", "ac", false));
        assert!(glob_match("*", "", false));
        assert!(glob_match("**a**", "xxaxx", false));
        assert!(!glob_match("*a*b", "xxaxx", false));
        assert!(glob_match("thumbcache_*.db", "thumbcache_256.db", true));
    }
}
