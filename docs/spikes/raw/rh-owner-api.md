# API spike on the reference laptop (RH-OWNER), 2026-11-20

Run by the owner from PowerShell with the CI artifact `api-spike-exe` (commit e1a0417).
**Caveat:** the spike reports `elevated: true` although PowerShell was not started "as administrator",
so this session's token is elevated (UAC likely disabled; to be confirmed). The access matrix is
therefore **not** the standard-user picture. The machine was under real memory pressure (86 % load,
commit 41.6 of 55 GiB, High notification signaled).

## Environment

| Item | Value |
|---|---|
| Product | Windows 10 Home Single Language (registry name; build ≥ 22000 = Windows 11) |
| Display version | 26H2 |
| Build | 26300.9974 |
| Spike elevated | Ok(true) |
| Spike private bytes at start | 1.1 MiB |
| Logical CPUs | 16 |

## System metrics

| Metric | Value | Source | Avg call cost |
|---|---|---|---|
| Total physical | 16086 MiB | GlobalMemoryStatusEx | 364ns |
| Available physical | 2250 MiB | GlobalMemoryStatusEx | |
| Memory load | 86 % | GlobalMemoryStatusEx | |
| Commit charge | 41640 MiB | GetPerformanceInfo | 30.548µs |
| Commit limit | 54998 MiB | GetPerformanceInfo | |
| System cache | 1727 MiB | GetPerformanceInfo | |
| Processes | 401 | GetPerformanceInfo | |

## Memory resource notifications

| Handle | Query state | Wait(0) #1 | Wait(0) #2 |
|---|---|---|---|
| Low | not signaled | timeout | timeout |
| High | signaled | signaled | signaled |

## Process access matrix (401 processes)

| Test | Results |
|---|---|
| OpenProcess(QUERY_LIMITED) | err 87: 1, ok: 400 |
| GetProcessMemoryInfo(EX2) | ok: 400 |
| GetProcessTimes | ok: 400 |
| IsProcessCritical | critical: 9, ok: 391 |
| GetProcessIoCounters (limited handle) | ok: 400 |
| OpenProcessToken + TokenElevation | elevated: 219, err 5: 1, not elevated: 180 |
| OpenProcess(QUERY_LIMITED + SET_QUOTA + SYNCHRONIZE) | err 5: 15, err 87: 1, ok: 385 |

| PID | Image | Query | EX2 | Critical | IO | Token | Open for trim |
|---|---|---|---|---|---|---|---|
| 0 | [System Process] | err 87 |  |  |  |  | err 87 |
| 4 | System | ok | ok | ok | ok | err 5 | err 5 |
| 280 | Registry | ok | ok | ok | ok | not elevated | err 5 |
| 912 | smss.exe | ok | ok | critical | ok | elevated | err 5 |
| 1284 | csrss.exe | ok | ok | critical | ok | elevated | err 5 |
| 1408 | wininit.exe | ok | ok | critical | ok | elevated | err 5 |
| 1492 | winlogon.exe | ok | ok | ok | ok | elevated | ok |
| 1520 | services.exe | ok | ok | critical | ok | elevated | err 5 |
| 1568 | lsass.exe | ok | ok | ok | ok | elevated | err 5 |
| 1720 | svchost.exe | ok | ok | critical | ok | elevated | ok |
| 1776 | fontdrvhost.exe | ok | ok | ok | ok | not elevated | ok |
| 3236 | Memory Compression | ok | ok | ok | ok | elevated | err 5 |
| 5240 | MsMpEng.exe | ok | ok | ok | ok | elevated | err 5 |
| 5728 | sihost.exe | ok | ok | ok | ok | not elevated | ok |
| 7756 | explorer.exe | ok | ok | ok | ok | not elevated | ok |
| 19256 | ctfmon.exe | ok | ok | ok | ok | not elevated | ok |
| 35608 | dwm.exe | ok | ok | ok | ok | not elevated | ok |
| 38384 | audiodg.exe | ok | ok | ok | ok | elevated | ok |

## Enumeration cost

401 processes: median 10.46 ms, min 9.77 ms, max 10.94 ms (7 runs).

## PDH

| Counter | Value |
|---|---|
| `\Memory\Standby Cache Core Bytes` | 104 MiB |
| `\Memory\Standby Cache Normal Priority Bytes` | 1807 MiB |
| `\Memory\Standby Cache Reserve Bytes` | 332 MiB |
| `\Memory\Modified Page List Bytes` | 32 MiB |
| `\Memory\Free & Zero Page List Bytes` | 3 MiB |
| `\Memory\Cache Bytes` | 247 MiB |
| `\Memory\Pages Input/sec` | 433.9 /s |
| `\Memory\Page Reads/sec` | 92.0 /s |

| Step | Time | Spike private bytes |
|---|---|---|
| Before | | 1.4 MiB |
| LoadLibraryExW(pdh.dll) | 0.89 ms | 1.4 MiB |
| Open query + add counters + first collect | 284.7 ms | 5.5 MiB |
| After PdhCloseQuery | | 5.5 MiB |
| After FreeLibrary (unloaded) | | 3.2 MiB |

## Trim experiments

| Probe | Trim time | Re-touch resident | Re-touch after trim | Commit change | Exit checks |
|---|---|---|---|---|---|
| 256 MiB | 25.6 ms | 0.69 ms | 56.7 ms | none (257.6 MiB) | as expected; trim on exited → err 5 |
| 1024 MiB | 97.7 ms | 2.94 ms | 239.6 ms | none (1027.1 MiB) | as expected; trim on exited → err 5 |
