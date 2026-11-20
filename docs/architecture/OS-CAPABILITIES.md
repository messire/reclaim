# Reclaim — OS Capability and Risk Analysis

**Status:** desk analysis from Microsoft documentation and known Windows memory-manager behaviour. Items marked **[verify]** must be confirmed by the M0 API spike on Windows 10 22H2 and Windows 11 before the adapter depends on them.

## 1. What working-set trimming actually does on Windows

- `EmptyWorkingSet(h)` and `SetProcessWorkingSetSize(h, (SIZE_T)-1, (SIZE_T)-1)` remove as many pages as possible from the target's working set. Both are documented; the latter is the documented equivalent.
- Removed **clean** pages go to the standby list; **dirty** pages go to the modified list and are later written to the page file by the modified page writer. Nothing is deallocated: the process's commit (private bytes) is unchanged.
- When the process touches those pages again:
  - page still on standby/modified → **soft fault** (cheap, microseconds);
  - page already repurposed → **hard fault** (disk read, milliseconds on SSD, much worse on HDD).
- Consequences for the product:
  - "Available" memory may rise immediately because standby counts as available, but the same pages were often already reclaimable by the memory manager on demand.
  - Under low pressure, trimming is mostly cosmetic and can add soft faults; under high pressure it may help a foreground app at the cost of background apps' latency.
  - Writing modified pages creates disk I/O, which on battery or slow disks is a real cost.
- This is why the architecture treats trimming as an *experiment with measured outcome* and never as a "boost".

## 2. Capability matrix (Windows, unelevated, same user)

| Capability | API | Required access | Works for | Fails / limits | Notes |
|---|---|---|---|---|---|
| System RAM / commit | `GlobalMemoryStatusEx`, `GetPerformanceInfo` | none | — | — | µs cost; safe at startup |
| Memory pressure signal | `CreateMemoryResourceNotification`, `QueryMemoryResourceNotification` | none | — | Coarse (low/high thresholds chosen by OS) | Wait-able handle: zero-cost idle monitoring |
| Standby / modified breakdown | PDH counters under `\Memory\` | none **[verify]** | — | Counter names are localized; use `PdhAddEnglishCounterW`. PDH loads extra DLLs (~MBs) | Load lazily, unload when hidden |
| Paging input rate | PDH `\Memory\Pages Input/sec` | none | — | System-wide only | Cost indicator during follow-up |
| Process list | `CreateToolhelp32Snapshot` | none | all processes (names, PIDs) | — | Documented; returns image name only |
| Process memory | `GetProcessMemoryInfo` | `PROCESS_QUERY_LIMITED_INFORMATION` | own-user processes, many others | Some protected/system processes deny even limited query | `PROCESS_MEMORY_COUNTERS_EX2` (private WS) only on newer Windows 10/11 builds **[verify build]** |
| Process start time | `GetProcessTimes` | `PROCESS_QUERY_LIMITED_INFORMATION` | as above | — | Used for identity |
| Critical flag | `IsProcessCritical` | `PROCESS_QUERY_LIMITED_INFORMATION` | as above | Windows 8.1+ | Treat failure as "unknown → protected" |
| Per-process I/O | `GetProcessIoCounters` | `PROCESS_QUERY_INFORMATION` or limited **[verify]** | own-user | — | Optional cost indicator |
| Foreground process | `GetForegroundWindow` + `GetWindowThreadProcessId` | none | — | Null during secure desktop / lock screen | Exclude by default |
| Trim working set | `SetProcessWorkingSetSize` / `EmptyWorkingSet` | `PROCESS_SET_QUOTA` + query | own-user, same or lower integrity | Elevated (high-IL) processes from a medium-IL caller; other users; services; protected processes (PP/PPL) → `ERROR_ACCESS_DENIED` | Exactly the least-privilege boundary we want |
| Integrity level | `OpenProcessToken` + `GetTokenInformation(TokenIntegrityLevel)` | `PROCESS_QUERY_LIMITED_INFORMATION` + `TOKEN_QUERY` | own-user | Fails for many system processes | Used only to *predict* "requires elevation"; the open attempt is authoritative |

## 3. Capabilities deliberately not used in the MVP

| Mechanism | Why excluded | Status |
|---|---|---|
| Purging the standby list / combining memory pages (`NtSetSystemInformation(SystemMemoryListInformation)`) | Undocumented, requires administrator and `SeProfileSingleProcessPrivilege`, discards file cache for the whole system (hurts subsequent I/O). Changes system state. | Research only. Could exist later as an explicitly experimental adapter (ADR-0003). |
| `SetSystemFileCacheSize` | Changes system configuration; requires privilege. Forbidden by PRODUCT §4. | Excluded |
| Hard working-set limits (`SetProcessWorkingSetSizeEx` with `QUOTA_LIMITS_HARDWS_*`) | Persistent behavioural change to another process; can cause thrashing | Excluded |
| `NtQuerySystemInformation(SystemProcessInformation)` for fast enumeration | Partially documented, structure subject to change | Only if benchmarks show the documented path misses the < 1 s target (it is not expected to) |
| `SeDebugPrivilege` | Grants access to nearly all processes; far beyond least privilege | Excluded, including from any future broker |
| ETW per-process hard-fault tracing | Requires administrator or Performance Log Users group; complex | Benchmark harness only (optional), not the product |

## 4. Proposed default protection lists

**System-critical (never trimmed, not user-overridable):** PID 0 (Idle), PID 4 (System), `Registry`, `smss.exe`, `csrss.exe`, `wininit.exe`, `winlogon.exe`, `services.exe`, `lsass.exe`, `LsaIso.exe`, `Memory Compression`, `fontdrvhost.exe`, `MsMpEng.exe` and other PPL anti-malware, any process where `IsProcessCritical` is true or cannot be determined. Most of these are inaccessible anyway; listing them makes the reason explicit instead of "access denied".

**Caution (excluded by default, user can opt in with a warning):**

| Process | Why |
|---|---|
| `dwm.exe` | Desktop compositor; trimming causes visible frame drops on next interaction |
| `audiodg.exe` | Audio glitches |
| `explorer.exe`, `ShellExperienceHost.exe`, `StartMenuExperienceHost.exe`, `SearchHost.exe` | Shell latency on next use |
| `ctfmon.exe`, `TextInputHost.exe` | Input latency |
| `sihost.exe`, `taskhostw.exe`, `RuntimeBroker.exe` | Session infrastructure |
| The foreground process | User is interacting with it |
| Reclaim itself | Pointless; would also distort self-measurement |

Matching uses image name plus a check that the image is in `%SystemRoot%` for system entries (a user binary named `dwm.exe` elsewhere is not treated as the compositor). The lists are data in `reclaim-core`, versioned and unit-tested.

## 5. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| O1 | Trimming yields large WS numbers but no benefit and extra faults | High | Credibility | Rebound and cost reporting; dry-run control; no performance claims |
| O2 | Acting on a reused PID | Low | High (wrong process) | `ProcessKey` with start time; act only via validated handle held for the run |
| O3 | Most large consumers (browsers are usually OK; games/anticheat, elevated tools, services) are inaccessible | Medium | Perceived weakness | Transparent "requires administrator" reasons; ADR-0002 broker deferred |
| O4 | PDH initialization is slow or heavy | Medium | Startup / idle footprint | Lazy load after first paint; unload when hidden; metrics are optional |
| O5 | `PROCESS_MEMORY_COUNTERS_EX2` missing on supported builds | Medium | Less precise UI | Optional field; fall back to WS + private bytes |
| O6 | Trimming many processes at once causes a paging I/O burst | Medium | Disk stutter | Sequential trimming, threshold, largest-first; optional pacing setting if benchmarks show bursts |
| O7 | Anti-malware flags cross-process quota changes as suspicious | Low–Medium | False positive detection | Documented APIs only, signed binary (if owner approves), no injection or debug privilege |
| O8 | Low-memory notification thresholds too coarse for useful pressure display | Medium | UX | Combine with available/commit ratios; label clearly as "OS signal" vs "derived" |

## 6. Other platforms (for interface design only)

| Concern | Linux | macOS |
|---|---|---|
| System metrics | `/proc/meminfo`, PSI (`/proc/pressure/memory`) — excellent pressure signal | `host_statistics64`, `sysctl vm.*`, memory pressure via `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` |
| Process metrics | `/proc/<pid>/status`, `smaps_rollup` (needs same user or ptrace read access) | `proc_pid_rusage` (own user) |
| Reclaim action | `process_madvise(MADV_COLD / MADV_PAGEOUT)` via `pidfd`, kernel ≥ 5.10, requires `PTRACE_MODE_READ` + `CAP_SYS_NICE` | No public API to trim another process; `Unsupported` |
| Implication for ports | `ReclaimAction::PageOutCold` variant; capability may be `RequiresElevation` | Monitoring + storage only |

These differences are why `ReclaimAction` and `Capabilities` exist in the core instead of a `trim(pid)` function.
