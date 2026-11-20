# Manual test checklist (PLAN M6.1)

Run on a clean VirtualBox VM for each of Windows 10 22H2 and Windows 11 23H2+:
- as a **standard user** (the owner works elevated, so this matrix is only covered here);
- with the release zip from the Release workflow, not a local build.

Record the build, VM, date and the result of each line (✓, ✗ with a note, or n/a). Each line names the ACCEPTANCE item it gives evidence for.

## Install and start

| # | Step | Expected | ACC |
|---|---|---|---|
| 1 | Unzip to a folder and start `reclaim.exe` on a VM with no VC++ redistributable | Window opens, no runtime prompt | §1.1 |
| 2 | Look at Overview for 10 s | Available, installed, physical memory %, committed of limit and pressure are filled and change plausibly against Task Manager | §1.2 |
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
| 9 | Overview: Dry run…, then Start | Result shows "Dry run: nothing was trimmed"; working sets unchanged | §3 |
| 10 | Overview: Trim working sets… | Confirmation states the count, the total and that committed memory is not freed | §1.3, §1.9 |
| 11 | Start; type into another app during the run | UI stays responsive; progress shows; the per-process table fills | §1.7 |
| 12 | Wait 60 s | Report shows commit change, available change, rebound at 10 s and 60 s, faults; it ends with the responsiveness note | §1.8, §1.9 |
| 13 | Start a run and press Cancel during the follow-up | Status "Follow-up cut short…"; the record is in History | §1.7 |
| 14 | Kill a target app (Task Manager) during a run on many processes | Its row says "the process exited"; no crash | §4.2 |
| 15 | Check that no process was terminated (compare the process count before and after) | Same processes | §1.4 |

## Tray and lifecycle

| # | Step | Expected | ACC |
|---|---|---|---|
| 16 | Close (X) | Window closes, tray icon stays; Reclaim's private bytes drop (Task Manager) | §4.5 |
| 17 | Hover the tray icon | Tooltip "Reclaim: … available" | §4.5 |
| 18 | Restart Explorer (Task Manager → Windows Explorer → Restart) | Tray icon comes back | §4.5 |
| 19 | Tray → Quit during a follow-up | Process exits within 2 s; History has the partial run | §4.5 |
| 20 | Move the window between monitors with different scaling | Text stays sharp, layout scales | — |

## Privacy and storage

| # | Step | Expected | ACC |
|---|---|---|---|
| 21 | Run with the network disconnected | Everything works | §1.10 |
| 22 | Inspect `%LOCALAPPDATA%\Reclaim` | `settings.toml`, `history.jsonl`; no log unless enabled; no paths beyond image names | §1.10, §4.6 |
| 23 | History → Clear history and logs | Files removed; History empty | §4.6 |
| 24 | Corrupt `settings.toml` (type garbage) and start | Defaults used, a warning is shown, the file is kept as `settings.toml.bad` | §4.2 |
