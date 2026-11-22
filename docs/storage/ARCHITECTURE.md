# Storage architecture (0.3.0)

The storage features follow the memory MVP's layering (ports and adapters, ARCHITECTURE §3):

| Crate | Role | Platform |
|---|---|---|
| `reclaim-storage-core` | Paths and normalisation, the scan index (`Tree`), queries (largest, extensions, filters), treemap and icicle layout, protected-path `Guard`, cleanup categories and reports, duplicate grouping and selection rules, ports `Fs`, `Volumes`, `Remover` | any, no I/O |
| `reclaim-storage-engine` | Multi-threaded scanner, cleanup discovery and execution, duplicate hashing (BLAKE3, pure Rust), JSON/CSV export, settings and history store, `StdFs`, in-memory `FakeFs` for tests | any |
| `reclaim-storage-windows` | `WinFs` (handle-based listing, `FILE_ID_BOTH_DIR_INFO`), `WinVolumes`, `WinRemover` (revalidating delete and recycle), known folders, shell helpers, fast NTFS scan, change watcher | Windows |
| `reclaim-app::storage_vm` | Screen state for Disk usage, Cleanup, Duplicates; background jobs | any (tested on Linux) |
| `reclaim-app::ui::storage` | Slint adapter | Windows |

## Index

An arena of nodes; each directory's children are one contiguous range, so a directory listing
is appended once and totals are propagated incrementally to the ancestors. Per node: parent,
child range, kind, attribute bits, logical and allocated size, modification time, extension id,
and counts. Hard links are resolved after a scan by (volume, file id): the first name counts,
the others are flagged `HARDLINK_DUP` and excluded from totals. A refresh clears a directory's
range (old nodes become dead), rescans it and adjusts the ancestors; deletions do the same for
a single node. About 100 bytes per entry were measured (see `docs/benchmarks/storage.md`).

## Scanning

`std::thread::scope` workers share a job stack of directories; threads default by drive kind
(SSD: up to 4, HDD/optical: 1, network: 8). Pause parks the workers, cancel stops them; the
summary says whether the index is complete. Reparse points are never followed; other volumes
are not entered (`stay_on_volume`); exclusions match paths or name globs. Errors (access denied,
too long, vanished) are recorded with the path (up to 10 000, then counted).

The fast NTFS scan reads the volume's file layout with `FSCTL_QUERY_FILE_LAYOUT`. It needs a
volume handle, which needs administrator rights; Reclaim uses it only when the user already
runs it elevated (ADR-0004) and the target is a whole NTFS volume, and falls back otherwise.

## Cleanup

Discovery is read-only: each category resolves its documented locations, lists candidates
(never entering links), applies age and size guards and user exclusions, and estimates
reclaimable bytes (hard-linked files count zero). Execution takes the reviewed plan and, for
every target, opens the object itself, checks identity (file id), type, that it is not a link,
its final path, and the `Guard` (allowed root, protected folders, volume roots); only then it
deletes by handle (POSIX semantics) or recycles. Locked or changed files are skipped and
reported. Free space is measured before and after per volume, so the report shows the estimate,
what was deleted, what was recycled and what Windows reports as freed. History stores
categories, counts and byte totals only, never paths.

## Duplicates

Files are grouped by size (hard-link duplicates and offline files excluded), then by a 64 KiB
prefix hash, then by full-content hash. Groups only partly hashed (cancelled) are "probable".
Nothing is selected automatically; marking the last copy of a group is refused in the view model
and again by `validate_selection`; marked files go to the Recycle Bin after confirmation.

## Resource use

No storage code runs at start-up: the state is created on the first visit to a storage screen,
and drive information is read then. Every job is a short-lived thread that ends with its result;
no async runtime, no resident thread. The optional change watcher (off by default) is the only
long-lived thread and stops when switched off.
