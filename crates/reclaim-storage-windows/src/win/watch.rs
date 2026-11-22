//! Change notifications below a scanned folder (`ReadDirectoryChangesW`), so a refresh
//! can re-list only the directories that changed.
//!
//! One thread per watcher, and only while it runs. The thread blocks in a synchronous
//! `ReadDirectoryChangesW`; stopping sets a flag and cancels that call with `CancelIoEx`
//! (repeated until the thread has exited, which also covers a cancel that lands just
//! before the next call starts). The watcher owns the directory handle and closes it
//! only after the thread has been joined.

use std::collections::BTreeSet;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use reclaim_storage_core::entry::ErrorKind;
use windows_sys::Win32::Foundation::{ERROR_NOTIFY_ENUM_DIR, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, CancelSynchronousIo};

use super::{Handle, SHARE_ALL, as_bytes, last_error, map_error, map_io, open};
use crate::common::long_path;
use crate::common::notify::for_each_parent;

/// Notification buffer: 64 KiB, DWORD aligned (u64 words).
const NOTIFY_BUF_WORDS: usize = 64 * 1024 / 8;

#[derive(Debug, Default)]
struct Shared {
    stop: AtomicBool,
    overflowed: AtomicBool,
    changed: Mutex<BTreeSet<String>>,
}

/// A running watch of one directory tree.
#[derive(Debug)]
pub struct Watcher {
    shared: Arc<Shared>,
    /// The directory handle as an integer, shared with the thread for its blocking
    /// call; owned (and closed) by the watcher after the thread is joined.
    handle: Arc<AtomicIsize>,
    thread: Option<JoinHandle<()>>,
}

fn run(shared: &Shared, handle: HANDLE, root: &str) {
    let mut buf = vec![0u64; NOTIFY_BUF_WORDS];
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE;
    while !shared.stop.load(Ordering::SeqCst) {
        let mut returned = 0u32;
        // SAFETY: `handle` is a directory handle opened for listing and kept open by
        // the owning `Watcher` until this thread has been joined; the buffer is live,
        // aligned and sized as passed; the call is synchronous (no OVERLAPPED, no
        // completion routine).
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr().cast(),
                std::mem::size_of_val(buf.as_slice()) as u32,
                1,
                filter,
                &mut returned,
                std::ptr::null_mut(),
                None,
            )
        };
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        if ok == 0 {
            shared.overflowed.store(true, Ordering::SeqCst);
            if last_error() == ERROR_NOTIFY_ENUM_DIR {
                continue;
            }
            // The directory went away or the handle broke: report and stop watching.
            return;
        }
        if returned == 0 {
            // More changes than the buffer holds: the caller must rescan.
            shared.overflowed.store(true, Ordering::SeqCst);
            continue;
        }
        let bytes = as_bytes(&buf);
        let mut set = shared.changed.lock().unwrap_or_else(|p| p.into_inner());
        for_each_parent(&bytes[..(returned as usize).min(bytes.len())], root, |p| {
            set.insert(p);
        });
    }
}

impl Watcher {
    /// Starts watching `root` and everything below it.
    pub fn start(root: &str) -> Result<Watcher, ErrorKind> {
        let h = open(
            &long_path(root),
            FILE_LIST_DIRECTORY,
            SHARE_ALL,
            FILE_FLAG_BACKUP_SEMANTICS,
        )
        .map_err(map_error)?;
        let shared = Arc::new(Shared::default());
        let handle = Arc::new(AtomicIsize::new(h.into_raw() as isize));
        let display = {
            let t = root.trim_end_matches(['\\', '/']);
            if t.len() == 2 && t.ends_with(':') {
                format!("{t}\\")
            } else {
                t.to_owned()
            }
        };
        let (s, hh) = (shared.clone(), handle.clone());
        let thread = std::thread::Builder::new()
            .name("reclaim-watch".into())
            .spawn(move || run(&s, hh.load(Ordering::SeqCst) as HANDLE, &display));
        match thread {
            Ok(t) => Ok(Watcher {
                shared,
                handle,
                thread: Some(t),
            }),
            Err(e) => {
                drop(Handle::from_raw(handle.swap(0, Ordering::SeqCst) as HANDLE));
                Err(map_io(&e))
            }
        }
    }

    /// Directories with changes since the last call (full paths, sorted) and whether
    /// notifications were lost (the caller should rescan everything).
    pub fn take_changed(&self) -> (Vec<String>, bool) {
        let set = std::mem::take(
            &mut *self
                .shared
                .changed
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        );
        let overflowed = self.shared.overflowed.swap(false, Ordering::SeqCst);
        (set.into_iter().collect(), overflowed)
    }

    /// Stops the watch and waits for its thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.shared.stop.store(true, Ordering::SeqCst);
        let h = self.handle.load(Ordering::SeqCst) as HANDLE;
        while !thread.is_finished() {
            // SAFETY: `h` stays open until after the join below, so it cannot have been
            // closed or reused; cancelling all I/O on it only ends our own pending call.
            unsafe { CancelIoEx(h, std::ptr::null()) };
            // SAFETY: the thread handle is valid until the JoinHandle is joined; the
            // thread only ever blocks in ReadDirectoryChangesW, so this cancels nothing
            // else. (Covers builds where CancelIoEx does not reach synchronous I/O.)
            unsafe { CancelSynchronousIo(thread.as_raw_handle()) };
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = thread.join();
        let raw = self.handle.swap(0, Ordering::SeqCst);
        if raw != 0 {
            drop(Handle::from_raw(raw as HANDLE));
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}
