# Manual test checklist (PLAN M6.1)

Run on a clean VirtualBox VM for each of Windows 10 22H2 and Windows 11 23H2+:
- as a **standard user** (the owner works elevated, so this matrix is only covered here);
- with the release zip or installer from the Release workflow, not a local build.

Record the build, VM, date and the result of each line (✓, ✗ with a note, or n/a). Each line names the ACCEPTANCE item it gives evidence for.

## Install and start

| # | Step | Expected | ACC |
|---|---|---|---|
| 1 | Unzip to a folder and start `reclaim.exe` on a VM with no VC++ redistributable | Window opens, no runtime prompt | §1.1 |
| 2 | Look at Overview for 10 s | Used of installed (bar turns from teal to amber/red as it fills), physical memory %, committed of limit and pressure are filled and change plausibly against Task Manager | §1.2 |
| 3 | Start `reclaim.exe` again while it runs | No second instance; the open window comes to the front | §4.5 |
| 4 | `reclaim.exe --tray` (after Quit) | Only the tray icon appears; Open shows the window | §4.5 |

## Processes and policy

| # | Step | Expected | ACC |
|---|---|---|---|
| 5 | Processes tab, sort by each column | Sorting is correct; working set and private bytes are shown | §1.2 |
| 6 | Find System, csrss.exe, dwm.exe, Reclaim itself and an elevated app | Status: System process / Protected by default / Reclaim itself / Needs administrator rights; none is selectable | §1.5, §4.3 |
| 7 | Settings: add an app's image name to "Never trim" and save | The app shows "Excluded in settings" | §1.3 |
| 8 | Settings: set a threshold of 100000 and save | Validation message, settings unchanged | §1.3 |

## Run

| # | Step | Expected | ACC |
|---|---|---|---|
| 9 | CLI: `reclaim-cli trim --dry-run` (the window has no dry run) | Report says nothing was trimmed; working sets unchanged | §3 |
| 10 | Overview: Trim working sets… | Confirmation states the count, the total and that committed memory is not freed | §1.3, §1.9 |
| 11 | Start; type into another app during the run | UI stays responsive; the status bar shows "Trimming n of m…" with Cancel | §1.7 |
| 12 | Wait for the run to end | Status bar shows "N trimmed · amount"; the full report (rebound at 10 s and 60 s, faults) is in `history.jsonl` and in `reclaim-cli trim` | §1.8, §1.9 |
| 13 | Start a run on many processes and press Cancel in the status bar | Remaining processes are not attempted; the record is in `history.jsonl` | §1.7 |
| 14 | Kill a target app (Task Manager) during a run on many processes | Its row says "the process exited"; no crash | §4.2 |
| 15 | Check that no process was terminated (compare the process count before and after) | Same processes | §1.4 |

## Tray and lifecycle

| # | Step | Expected | ACC |
|---|---|---|---|
| 16 | Close (X) | Window closes, tray icon stays; Reclaim's private bytes drop (Task Manager) | §4.5 |
| 17 | Hover the tray icon | Tooltip "Reclaim: … available" | §4.5 |
| 18 | Restart Explorer (Task Manager → Windows Explorer → Restart) | Tray icon comes back | §4.5 |
| 19 | Tray → Quit during a follow-up | Process exits within 2 s; `history.jsonl` has the partial run | §4.5 |
| 20 | Move the window between monitors with different scaling | Text stays sharp, layout scales | — |

## Privacy and storage

| # | Step | Expected | ACC |
|---|---|---|---|
| 21 | Run with the network disconnected | Everything works | §1.10 |
| 22 | Inspect `%LOCALAPPDATA%\Reclaim` | `settings.toml`, `history.jsonl`; no log unless enabled; no paths beyond image names | §1.10, §4.6 |
| 23 | Settings → Clear history and logs | Files removed; status bar says "No run yet." | §4.6 |
| 24 | Corrupt `settings.toml` (type garbage) and start | Defaults used, a warning is shown, the file is kept as `settings.toml.bad` | §4.2 |

## Installer and start with Windows

| # | Step | Expected | ACC |
|---|---|---|---|
| 25 | Run `reclaim-<version>-windows-x64-setup.exe` as a standard user, pick a folder, tick *Start Reclaim with Windows* | No UAC prompt; files in the chosen folder; Start menu entry; Reclaim starts in the tray after sign-out and sign-in | — |
| 26 | Settings: untick *Start Reclaim with Windows*, close the window (X) without saving, open it again from the tray | Box is ticked again: nothing changes until Save (Revert also restores it) | — |
| 27 | Untick it and press Save | "Settings saved."; Task Manager → Startup apps no longer lists Reclaim | — |
| 28 | Run a newer setup while Reclaim runs | Folder page shows the installed folder; Reclaim is closed and replaced; Settings → Apps lists one Reclaim with the new version | — |
| 29 | Uninstall from Settings → Apps | Files, shortcut and the start-with-Windows entry are gone; `%LOCALAPPDATA%\Reclaim` is kept | — |

## Storage (0.3.0)

| # | Step | Expected | ACC |
|---|---|---|---|
| 30 | Start Reclaim, stay on Overview, watch Task Manager | No disk activity from Reclaim; nothing is scanned until a storage tab is used | §5 |
| 31 | Disk usage → pick C:, Scan drive (unelevated) | Progress counts rise; Pause stops them, Resume continues; status says "folder by folder" was used (no fast scan note unless elevated) | §5 |
| 32 | Cancel a scan half way | Partial tree and map remain; status says the results are partial | §5 |
| 33 | Click a large tile, then double-click it | Tree row is selected and scrolled into view; map zooms in; Back/Forward/Up work | §5 |
| 34 | Hover tiles | Tooltip with path, size, share; colours match the Extensions list | §5 |
| 35 | Select a file you own, Recycle… → confirm | File is in the Recycle Bin; sizes and lists update without a rescan | §5 |
| 36 | Select `C:\Windows` or a drive root | Recycle… is disabled | §5 |
| 37 | Search `*.log`, min size `10 MB` | Results appear without rescanning; clicking one selects it in tree and map | §5 |
| 38 | Errors tab after scanning C: unelevated | Lists access-denied folders (e.g. `System Volume Information`) | §5 |
| 39 | Export JSON in the window and CSV with `reclaim-cli storage scan <path> --csv FILE`, open them | JSON parses; both state whether the scan was complete and list unreadable folders; CSV opens in a spreadsheet with no formulas executed | §5 |
| 40 | Cleanup → Examine | Each category shows a size or "not examined"; nothing is deleted | §5 |
| 41 | Select Temporary files, Clean selected… | Confirmation says the files are deleted permanently; report shows deleted, skipped (in use) and the measured free-space change | §5 |
| 42 | Old installers: tick two files only, clean | Only those two files are in the Recycle Bin | §5 |
| 43 | Windows system files | Opens Windows Disk Cleanup, which asks for administrator approval itself | §5 |
| 44 | Duplicates → Find, mark all copies of a group | The last copy cannot be marked; "One copy of every group always stays." | §5 |
| 45 | Start Reclaim with "Run as administrator", Scan drive C: | Status says "fast NTFS scan"; totals agree with the unelevated scan within the size of files that changed | §5 |
| 46 | Turn on Watch for changes, create a large file in a scanned folder | Within a few seconds the folder is rescanned and the file appears | §5 |
