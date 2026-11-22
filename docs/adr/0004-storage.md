# ADR-0004: Storage analyzer and cleaner

- **Status:** Accepted
- **Date:** 2026-11-22
- **Context documents:** PRODUCT §5, ACCEPTANCE §5, ARCHITECTURE §12.1, ADR-0002, ADR-0003

## Decision

1. **Same privilege model as the memory features (ADR-0002).** Reclaim stays a single unelevated process and never offers to restart elevated. The fast NTFS scan (`FSCTL_QUERY_FILE_LAYOUT`, documented) is used only when the user has started Reclaim as administrator; otherwise drives are scanned folder by folder with the same results, more slowly. System cleanup (Windows Update leftovers and other system files) is delegated to Windows Disk Cleanup (`cleanmgr.exe`), which asks for elevation itself.
2. **Delete only what was reviewed, and revalidate the object.** Deletion acts on a handle opened on the target, after checking file id, type, reparse state, final path and the protected-path guard. Links are never followed; volume roots, Windows, Program Files, ProgramData, user libraries (Documents, Desktop, Pictures, Music, Videos, OneDrive) and app packages are protected.
3. **Permanent deletion only for regenerable data.** Caches, temporary files and crash dumps are deleted permanently and the confirmation says so; user files (old downloads, installers, duplicates, items removed in the analyzer) go to the Recycle Bin. The Recycle Bin is emptied only as its own selected and confirmed category.
4. **Visualisations:** squarified treemap and an icicle view. A sunburst is not implemented: radial labels and hit-testing cost more on the software renderer and the icicle shows the same hierarchy with readable rectangles.
5. **No USN journal.** Incremental refresh rescans the affected folder; optional change watching uses `ReadDirectoryChangesW` on the scanned roots. The USN journal needs elevation and per-volume state.
6. **Pure-Rust BLAKE3** for duplicates (no C or assembly build step), two readers by default to keep disks responsive.

## Consequences

- An unelevated scan of a full system drive takes longer than tools that read the MFT; the status line says which method was used and why the fast scan was not.
- Some system space can only be freed through Windows' own tool; the report says when it was opened and that its results are not included.
