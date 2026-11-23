# Changelog

Versions follow the rule in [README → Versioning](README.md#versioning): patch for fixes only, minor for new features (and, while 0.x, breaking changes), major from 1.0.0 for breaking changes.

## [0.3.1] — folder dialog fix
- Disk usage: the *Browse…* folder dialog opens centred over the window and in front of it; before, it could open behind the window and the app looked frozen.

## [0.3.0] — storage analyzer and cleaner
- Disk usage tab: scan drives or folders (several at once) with progress, pause, resume and cancel; partial results stay and are marked. Directory tree with logical and on-disk size, share of parent and total, file and folder counts. Squarified treemap and icicle view coloured by file type, hover details, click to select, double-click to zoom, back, forward, up. Largest files and folders, extension statistics, list of unreadable locations. Search by name, path, extension, size, age and type (text, `*.glob` or `/regex/`) without rescanning. Open in Explorer, Properties, rescan a folder, move to the Recycle Bin. Export to JSON (CSV from the CLI). Optional watching for changes.
- Hard links counted once; compressed, sparse, offline and link entries flagged; links and other volumes never entered.
- Fast NTFS scan of whole drives when Reclaim is started as administrator; Reclaim never asks for elevation.
- Cleanup tab: temporary files, browser and app caches, developer and GPU shader caches, crash dumps, Recycle Bin, old installers and large old downloads (both file by file, to the Recycle Bin), and Windows system files through Windows Disk Cleanup. Each category says what it holds, what happens afterwards and how it is removed. Examining changes nothing; every file is checked again right before removal; locked, changed and protected files are skipped and reported. The report compares the estimate with the free space Windows measures. Local history without file names.
- Duplicates tab: size, then file start, then full content (BLAKE3); probable and confirmed groups; nothing is marked automatically and one copy always stays.
- Settings: scan and cleanup exclusions, scan threads, sizes on disk or file lengths, fast scan, duplicate minimum size, cleanup history.
- CLI: `reclaim-cli storage drives | scan | dupes | cleanup`; cleanup only examines unless `--run --yes`.
- Larger default window (1120×780).
- Installer: waits until the running Reclaim has really exited before replacing files, and if a copy started as administrator is still running, says so instead of failing with "DeleteFile failed; code 5".
- Lists: rows keep clear of the scroll bar; the Files column in the tree is wide enough for millions of files.
- Disk usage: the map rectangle follows the selection (or the folder that contains it) and disappears when the selection is outside the zoomed folder; clicks and double-clicks in the tree are no longer lost when the list refreshes; selecting no longer rebuilds the map; the tree and map halves keep their width. Cleanup: the category list has a fixed width, so picking a category no longer resizes the panels.
- Treemap and icicle: the cell budget goes to the largest folders first, so a big folder such as Users is always subdivided instead of being drawn as one solid block. Double-click on the map zooms one level deeper toward the click; right-click zooms out.
- Disk usage: a status lamp starts the button row (grey before the first scan, pulsing amber while scanning, steady amber when paused or cancelled, green when done); the scan line moved to the status bar; "Watch for changes" has its own row.
- Disk index: about a third smaller (48-byte records, compact counters, no growth slack after a scan) and freed after the window has been in the tray for 10 minutes (Settings; 0 keeps it). On Windows the hard-link table no longer keeps an entry for every file once the scan is done (24 bytes per file less). The window now uses the 16 px icon from the exe instead of a shrunk 48 px one.
- Disk usage: a Disk / Folder switch picks what to scan and shows only that mode's controls (drive list and *Scan drive*, or folder field, *Browse…* and *Scan folders*); the knob and lit label are teal for Disk and amber for Folder. *Export CSV…* removed from the window; `reclaim-cli storage scan --csv` still writes CSV.

## [0.2.0] — installer, simpler window
- Installer (Inno Setup, per-user, no admin): choose the folder, optional start with Windows; a newer version replaces the installed one, never side by side. Built with `scripts\build-installer.cmd` from a fresh clone; the portable zip stays. CI tests install, upgrade, move and uninstall.
- Settings → *Start Reclaim with Windows (in the tray)*; like every setting it takes effect on *Save*.
- Overview bar shows memory in use (installed minus available) with a teal → amber → red gradient.
- Settings and About are icons on the right of the tab row; About is the rightmost.
- Removed the Dry run button and the Result and History tabs; a status bar shows run progress (with Cancel) and the last trim, e.g. "30 trimmed · 4.5 GiB". *Clear history and logs* moved to Settings. Dry runs and the full report stay in the CLI and `history.jsonl`.
- About: the Slint attribution (AboutSlint, required by its royalty-free license) is smaller and labelled as a third-party toolkit.

## [0.1.0] — memory MVP (Windows)
- Core domain and fake platform for tests (line coverage ≥ 90 %).
- Windows adapter: system memory, per-process working set and private bytes, trim eligibility with reasons.
- Engine and CLI: `status`, `list`, `plan`, `trim` with dry run, confirmation and JSON output.
- Result report immediately, after 10 s and 60 s: reduced working set, rebound, page faults, commit change.
- Slint desktop UI, tray mode, single instance.
- Dark monospace window style: memory bars on the overview, custom process table, dark title bar.
- App icon in the exe and window; tray icon shows the state (normal, high pressure, working, unavailable).
- Benchmark harness: probes, scenarios S1–S8, soak test, CI smoke run.
- Tag-driven release workflow: zip, `SHA256SUMS`, third-party notices.
- MIT license.
