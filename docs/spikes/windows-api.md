# M0 spike: Windows API behaviour

**Status:** CI and reference-laptop results recorded 2026-11-20. The owner works as administrator, so the laptop run was elevated too; the **standard-user access matrix is still pending** and moves to a VirtualBox VM (`VM-CLEAN`).
**Tool:** `cargo run --release -p reclaim-bench --bin api-spike -- --out report.md`
**Raw reports:** [CI windows-2022](raw/ci-windows-2022.md) (Server 2022, build 20348) · [CI windows-2025](raw/ci-windows-2025.md) (Server 2025, build 26100, the Windows 11 24H2 code base)

## 1. Findings

| # | Question (OS-CAPABILITIES `[verify]`) | Result | Consequence |
|---|---|---|---|
| A1 | Are memory-resource notifications level-triggered? | **Yes.** The High handle was signaled on two consecutive zero-timeout waits. | Confirms REVIEW F27; the low/high handle swap in ARCH §6.1 is required. |
| A2 | Is `PROCESS_MEMORY_COUNTERS_EX2` (private working set) available? | Yes on builds 20348 and 26100, for every process that could be opened (148/148, 120/120). | Private WS can be shown by default on supported builds; keep the `Option` fallback for Windows 10 22H2 (build 19045, still unverified). |
| A3 | Does `GetProcessIoCounters` work with `PROCESS_QUERY_LIMITED_INFORMATION`? | Yes, for all openable processes. | Per-process I/O cost indicator needs no extra rights. |
| A4 | Is `IsProcessCritical` enough to identify system-critical processes? | **No.** It flags smss, csrss, wininit, services and some svchost instances, but **not** `lsass.exe` or `winlogon.exe`. | The built-in system-critical list (OS-CAP §4) is mandatory, not a fallback. The flag is only an extra signal. |
| A5 | How expensive is enumeration? | 2.4–3.4 ms median for 120–150 processes (snapshot + open + memory info + start time). | The documented path is fast enough; no need for `NtQuerySystemInformation`. The F20 concern (process table every 3 s) is ~0.1 % of one core, acceptable only while that screen is visible. |
| A6 | What do the system metric calls cost? | `GlobalMemoryStatusEx` ≈ 0.7 µs; `GetPerformanceInfo` ≈ 0.56 ms (≈ 800× more). | Periodic sampling uses `GlobalMemoryStatusEx` (commit charge = `ullTotalPageFile − ullAvailPageFile`, limit = `ullTotalPageFile`). `GetPerformanceInfo` only at run snapshots. |
| A7 | What does PDH cost? | `LoadLibraryExW(pdh.dll)` 0.5 ms; opening the query, adding 8 English counters and the first collect take **235–290 ms** and add **≈ 5–6 MiB** private bytes. After `PdhCloseQuery` + `FreeLibrary` (module really unloaded), **≈ 2 MiB stays** in the process. | PDH must never be on the startup or idle path. See §2 for the design change. |
| A8 | Are the standby / modified / paging counters readable? | Yes via `PdhAddEnglishCounterW`; all 8 counters returned values. | Data is available when PDH is worth loading. |
| A9 | How long does a trim take? | `SetProcessWorkingSetSize(-1, -1)`: ≈ 30 ms for 256 MiB and ≈ 105–120 ms for 1 GiB of dirty private memory (≈ 110 ms/GiB). | The < 1 s target holds for a few GiB in total; a run over ~8 GiB of eligible WS will exceed it. Report elapsed time honestly; consider a per-run time budget later. |
| A10 | Does trimming change commit? | No. Private bytes identical before and after (257.6 / 1027.1 MiB). WS went to 0 and stayed there for 1 s while untouched. | Confirms the report wording rules (ARCH §5.6). |
| A11 | What does the rebound cost? | Re-touching all pages took ≈ 1 ms (256 MiB) and ≈ 4–6 ms (1 GiB) while resident, versus **≈ 60 ms and ≈ 240–260 ms after the trim**: about 50× slower, one soft fault per page (fault count doubled). These were soft faults because 14 GiB was free; under pressure they become hard faults (disk reads). | The cost of trimming a process that is still in use is real and measurable even without pressure. Supports excluding the foreground app by default and the latency probe in BENCHMARKS. |
| A12 | Does the `SYNCHRONIZE` exit check work? | Yes: `WAIT_TIMEOUT` while alive, `WAIT_OBJECT_0` after exit. | Confirms REVIEW F28. |
| A13 | What happens when trimming an exited process through a held handle? | `SetProcessWorkingSetSize` fails with **`ERROR_ACCESS_DENIED` (5)**, not a dedicated "exited" error. `GetProcessMemoryInfo` still succeeds (WS 0). | Classification must check exit **before** mapping error 5: wait(0) signaled → `Failed(Exited)`, otherwise → `Failed(AccessDenied)`. Otherwise an exit race would be misreported as a permission problem. |
| A14 | Which processes deny the trim open on an elevated CI runner? | System, Registry, smss, csrss, wininit, services, MsMpEng (protected), plus ~5 others. lsass, winlogon, dwm, explorer **were** openable when elevated. | Elevated contexts can reach critical processes, so policy, not access control, must protect them (another reason for A4's list). The standard-user picture is pending the laptop run. |

### Reference laptop (RH-OWNER, Windows 11 Home 26H2 build 26300, elevated, under real memory pressure)

[Raw report](raw/rh-owner-api.md). Confirms the CI findings on a client SKU:
- A1: the High notification was signaled on both waits at 86 % load (level-triggered, and a real pressure case).
- A2/A3: EX2 and I/O counters succeeded for all 400 openable processes.
- A4: `IsProcessCritical` again misses `lsass.exe` and `winlogon.exe`; elevated, `winlogon.exe`, `dwm.exe`, `audiodg.exe` and a critical `svchost.exe` were openable for trim, so the policy lists are the only protection in an elevated session.
- A5: 401 processes enumerated in 10.5 ms median.
- A7: PDH query setup 285 ms; ≈ 1.8 MiB stays after unload.
- A9–A13: trim 26 ms / 256 MiB and 98 ms / 1 GiB; re-touch after trim 57 ms and 240 ms versus 0.7 ms and 2.9 ms resident; commit unchanged; exit and error-5 behaviour identical.

## 2. Design changes from this spike

1. **PDH becomes on-demand only** (A7). Overview shows Available, Committed / limit and Pressure from `GlobalMemoryStatusEx` and the notification handles; the standby / modified breakdown moves to an expandable "Memory details" panel that loads PDH when opened. The paging-input cost indicator loads PDH only for a run's follow-up window. A first use costs ≈ 250 ms (off the UI thread) and leaves ≈ 2 MiB resident; the footprint benchmark B2/B3 is measured without opening the panel and reported separately with it.
2. **Sampling uses `GlobalMemoryStatusEx`** for commit as well (A6).
3. **Outcome classification** checks process exit before interpreting `ERROR_ACCESS_DENIED` (A13).
4. **System-critical list** is authoritative; `IsProcessCritical` is additive only (A4).

These are applied to ARCHITECTURE §5.1, §5.4, §6.1 and OS-CAPABILITIES §2.

## 3. Still open (needs the laptop run)

- Access matrix as a standard user (medium integrity): which of the user's own processes deny `PROCESS_SET_QUOTA`, how elevated processes report.
- Windows 11 Home 24H2 client behaviour (CI used Server SKUs), and Windows 10 22H2 in a VM for A2.
- Real pressure: low-memory notification and hard-fault rebound in VM-LOW.

To run it on the laptop without installing Rust: open the latest CI run of the PR or `main` under *Actions*, download the `api-spike-exe` artifact, unzip it, and in a **normal** (not "Run as administrator") PowerShell window run:

```powershell
.\api-spike.exe --out api-spike-laptop.md
```

The spike only reads system information and trims its own probe child processes. Then attach `api-spike-laptop.md` to the thread or commit it under `docs/spikes/raw/`.
