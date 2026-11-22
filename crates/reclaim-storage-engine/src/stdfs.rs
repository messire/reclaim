//! Portable filesystem adapter over `std::fs` (Linux, macOS; tests everywhere).
//!
//! On Windows the dedicated adapter in `reclaim-storage-windows` is used instead: it
//! reports allocated sizes and file ids from one directory handle per listing.

use std::fs;
use std::io::Read;

use reclaim_storage_core::entry::{Attrs, EntryMeta, ErrorKind, Kind};
use reclaim_storage_core::path::PathStyle;
use reclaim_storage_core::ports::Fs;

/// `std::fs` adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdFs;

fn mtime(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Converts `std` metadata (from `symlink_metadata`, links not followed).
pub fn meta(name: String, m: &fs::Metadata) -> EntryMeta {
    let ft = m.file_type();
    let mut attrs = Attrs::default();
    let kind = if ft.is_symlink() {
        attrs.set(Attrs::REPARSE);
        Kind::Link
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    if m.permissions().readonly() {
        attrs.set(Attrs::READONLY);
    }
    #[allow(unused_mut)]
    let mut e = EntryMeta {
        name,
        kind,
        logical: if kind == Kind::File { m.len() } else { 0 },
        allocated: None,
        mtime: mtime(m),
        attrs,
        file_id: None,
        nlink: None,
        volume: None,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        e.allocated = Some(m.blocks() * 512);
        e.file_id = Some(m.ino());
        e.nlink = Some(m.nlink() as u32);
        e.volume = Some(m.dev());
        if kind == Kind::File && m.blocks() * 512 < m.len() {
            e.attrs.set(Attrs::SPARSE);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let a = m.file_attributes();
        if a & 0x2 != 0 {
            e.attrs.set(Attrs::HIDDEN);
        }
        if a & 0x4 != 0 {
            e.attrs.set(Attrs::SYSTEM);
        }
        if a & 0x800 != 0 {
            e.attrs.set(Attrs::COMPRESSED);
        }
        if a & 0x200 != 0 {
            e.attrs.set(Attrs::SPARSE);
        }
        if a & 0x400 != 0 {
            e.attrs.set(Attrs::REPARSE);
            e.kind = Kind::Link;
            e.logical = 0;
        }
        if a & (0x1000 | 0x0040_0000) != 0 {
            e.attrs.set(Attrs::OFFLINE);
        }
    }
    e
}

impl Fs for StdFs {
    fn style(&self) -> PathStyle {
        PathStyle::native()
    }

    fn stat(&self, path: &str) -> Result<EntryMeta, ErrorKind> {
        let m = fs::symlink_metadata(path).map_err(|e| ErrorKind::from_io(&e))?;
        Ok(meta(path.to_owned(), &m))
    }

    fn read_dir(&self, path: &str, out: &mut Vec<EntryMeta>) -> Result<(), ErrorKind> {
        let rd = fs::read_dir(path).map_err(|e| ErrorKind::from_io(&e))?;
        for ent in rd {
            let Ok(ent) = ent else { continue };
            let name = ent.file_name().to_string_lossy().into_owned();
            // `DirEntry::metadata` does not follow links. A file removed meanwhile is
            // simply not listed (it is gone).
            match fs::symlink_metadata(ent.path()) {
                Ok(m) => out.push(meta(name, &m)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    let mut m = EntryMeta::file(&name, 0);
                    m.allocated = Some(0);
                    m.kind = Kind::Other;
                    out.push(m);
                }
            }
        }
        Ok(())
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, ErrorKind> {
        let f = fs::File::open(path).map_err(|e| ErrorKind::from_io(&e))?;
        Ok(Box::new(f))
    }
}
