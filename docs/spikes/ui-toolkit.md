# M0 spike: UI toolkit (Slint vs native Win32)

**Status:** CI results recorded 2026-11-20. **Pending on the reference laptop (`RH-OWNER`):** the screen-reader check (Narrator, NVDA), a 5-minute idle run and a cold start. CI runners are elevated, headless-ish VMs without a GPU or a real user session, so the numbers below are indicative, not the acceptance measurement.
**Prototypes:** `crates/spike-ui-slint` (Slint 1.19, software renderer, winit backend, AccessKit) and `crates/spike-ui-win32` (windows-sys, common controls, virtual ListView). Both contain the ADR-0001 spike scope: system panel, 300-row sortable table, modal confirmation, tray icon, close (X) to tray and restore, and they destroy the window when it goes to the tray.
**Harness:** `crates/reclaim-bench/src/bin/ui_bench` (`ui-bench --exe <spike>.exe --runs 10 --idle-secs 60 --out report.md`). "Ready" is a named event the spike sets once its window is shown; hiding is triggered by a second named event that behaves like pressing Close.
**Raw report:** [CI windows-2025](raw/ui-ci-windows-2025.md)

## 1. Results against the ADR-0001 gates

| Gate | Threshold | Native Win32 | Slint (software) | Note |
|---|---|---|---|---|
| Start to usable UI | median < 300 ms, p95 < 450 ms | 47 / 79 ms ✅ | 20 / 198 ms ✅ | Warm start (N = 10). Cold start pending on the laptop. Slint "ready" is the first event-loop turn after `show()` (its software renderer has no rendering notifier), so its figure may be slightly optimistic. |
| Idle private bytes, window visible | < 15 MiB preferred, < 25 MiB hard | 2.3 MiB ✅ | 5.5 MiB ✅ | |
| Idle private bytes, tray only | < 8 MiB preferred | 2.3 MiB ✅ | 3.6 MiB ✅ | Slint keeps its runtime after the window is dropped. |
| Idle CPU, window visible and tray | < 0.1 % average | 0.052 % / 0.000 % ✅ | 0.000 % / 0.000 % ✅ | 60 s on CI; the gate asks for 5 min on the laptop. An earlier Slint run showed 0.13 % while its tray icon failed to create; it did not reproduce after the fix. |
| Screen reader | all controls named, table navigable, keyboard reachable | pending | pending | Must be done by a person on the laptop (§3). |
| Release binary size | < 10 MiB preferred | 0.30 MiB ✅ | 11.55 MiB ⚠️ | Slint misses the *preference* by 1.5 MiB. Not yet tried: `opt-level = "s"` for Slint crates, dropping unused std-widget styles. |
| License | compatible with MIT | n/a ✅ | Royalty-free 2.0 ✅ | Attribution required in the About screen and THIRD_PARTY_NOTICES. |

## 2. Reading

- Both toolkits pass every measured hard gate on CI. Win32 is about 2–3× smaller in memory and 38× smaller on disk; Slint starts faster and is the only option with a path to Linux/macOS without a second UI.
- Slint's only miss so far is the binary-size *preference* (11.55 vs 10 MiB); it is not a hard limit.
- Development cost differs more than the numbers: the Win32 prototype needed hand-written pixel layout scaled by DPI, owner data for the table, and still lacks `WM_DPICHANGED` handling for moving between monitors; Slint's needed none of that, but its tray component is new (it failed without an explicit icon, which is not documented as mandatory).

**Recommendation:** Slint, per the selection rule and the owner's default for Q4 (portability), **provided the screen-reader check passes on the laptop**. If Narrator or NVDA cannot read the table or reach the actions, the rule falls back to native Win32 for Windows. ADR-0001 stays *Proposed* until that check is done.

## 3. Laptop run (owner, about 15 minutes)

1. Download the `ui-spike` artifact of the latest green CI run on the PR and unzip it (contains `spike-ui-win32.exe`, `spike-ui-slint.exe`, `ui-bench.exe`).
2. In a normal (not administrator) PowerShell, in that folder:
   ```
   .\ui-bench.exe --exe .\spike-ui-win32.exe --label "Native Win32" --runs 10 --idle-secs 300 --out ui-win32-laptop.md
   .\ui-bench.exe --exe .\spike-ui-slint.exe --label "Slint" --runs 10 --idle-secs 300 --out ui-slint-laptop.md
   ```
   Each takes about 11 minutes; do not use the laptop meanwhile.
3. Screen reader, for each `.exe` started by double-click: turn on Narrator (Win + Ctrl + Enter), then with the keyboard only (Tab, arrows, Enter, Space, Esc) check that it reads the memory labels and the button, that it can move through table rows and reads the cells, that a header can be sorted, that the confirmation dialog opens and closes, and that the tray icon menu opens (Win + B, arrows, Enter). Note what is not read or not reachable.
4. Send the two `.md` files and the notes.
