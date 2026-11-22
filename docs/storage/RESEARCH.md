# Storage research: WinDirStat and Roomy (functional reference only)

Both projects were studied for **what** they offer and how they behave, not for code.
No source code, text, icons or data files were copied. WinDirStat is GPL v3 (its code
cannot be used in MIT-licensed Reclaim at all); Roomy is MIT, but its category lists were
not copied either: Reclaim's categories are written from Windows' own documented folder
locations and each one carries its own safety class and explanation.

## Feature matrix

| Capability | WinDirStat | Roomy | Reclaim 0.3.0 |
|---|---|---|---|
| Scan drives and folders, several at once | yes | — | yes (drive list or folders separated by `;`) |
| Pause, resume, cancel scan | cancel | — | pause, resume, cancel; partial results kept and marked |
| Fast whole-volume scan | MFT reading when elevated | — | `FSCTL_QUERY_FILE_LAYOUT` when the user runs elevated; otherwise folder by folder (never asks for elevation) |
| Directory tree with sizes and shares | yes | — | yes: logical and allocated size, % of parent and of total, file and folder counts, lazy expansion |
| Treemap | squarified, cushion shading | — | squarified, flat tiles (cheap for the software renderer), click, zoom, back/forward |
| Other visualisation | sunburst/flame (recent versions) | — | icicle (top-down flame graph); sunburst not implemented, see ADR-0004 |
| Extension statistics with treemap colours | yes | — | yes, same palette in tree, map and list |
| Largest files and folders | via tree | — | dedicated lists, sortable by size, path, extension, modified |
| Search and filter without rescan | limited | — | name, path, extension, size range, age, type; text, glob or `/regex/` |
| Hard links counted once | yes | — | yes, and flagged |
| Logical vs. allocated size | yes | — | both, user-selectable basis; estimated allocation flagged |
| Cleanup of caches and temporary files | user-defined commands | Safe / Review categories, guard rules | 10 categories with safety class, consequence, regeneration note, per-file review where needed |
| Recycle Bin | empty | empties first | measured; emptied only when selected and confirmed |
| System (Windows Update, etc.) | — | admin category | handed to Windows Disk Cleanup (`cleanmgr`), which elevates itself |
| Duplicates | opt-in | — | size grouping → 64 KiB prefix → full BLAKE3; probable vs confirmed; one copy always stays |
| Export | CSV/JSON | — | JSON and CSV with completeness flag and error list |
| Change monitoring | — | — | optional `ReadDirectoryChangesW`, refreshes the affected folder |

## Lessons taken

- Scans must report what they could not read; a size total without the unreadable count is misleading.
- Cleanup must explain consequences per category and must not offer user documents as "junk": downloads and old installers are review-only, file by file, and go to the Recycle Bin.
- Elevation should be the user's choice; the analyzer works fully without it.
