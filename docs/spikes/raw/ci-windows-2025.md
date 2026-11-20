# Windows API spike report

## Environment

| Item | Value |
|---|---|
| Product | Windows Server 2025 Datacenter |
| Display version | 24H2 |
| Build | 26100.33674 |
| Spike elevated | Ok(true) |
| Spike private bytes at start | 1.1 MiB |
| Logical CPUs | 4 |

> Note: Windows 11 reports ProductName "Windows 10" in the registry; use the build number (≥ 22000 = Windows 11).

## System metrics

| Metric | Value | Source | Avg call cost |
|---|---|---|---|
| Total physical | 16379 MiB | GlobalMemoryStatusEx | 504ns |
| Available physical | 13790 MiB | GlobalMemoryStatusEx | |
| Memory load | 15 % | GlobalMemoryStatusEx | |
| Commit charge | 2409 MiB | GetPerformanceInfo | 11.281µs |
| Commit limit | 19323 MiB | GetPerformanceInfo | |
| System cache | 3396 MiB | GetPerformanceInfo | |
| Processes | 139 | GetPerformanceInfo | |

## Memory resource notifications

| Handle | Query state | Wait(0) #1 | Wait(0) #2 |
|---|---|---|---|
| Low | not signaled | timeout | timeout |
| High | signaled | signaled | signaled |

If a handle is signaled on both waits, the notification is level-triggered (REVIEW F27).

## Foreground window

Foreground PID: 896

## Process access matrix (139 processes)

Error 5 = access denied, 87 = invalid parameter.

| Test | Results |
|---|---|
| OpenProcess(QUERY_LIMITED) | err 87: 1, ok: 138 |
| GetProcessMemoryInfo(EX2) | ok: 138 |
| GetProcessTimes | ok: 138 |
| IsProcessCritical | critical: 9, ok: 129 |
| GetProcessIoCounters (limited handle) | ok: 138 |
| OpenProcessToken + TokenElevation | elevated: 127, err 5: 1, not elevated: 10 |
| OpenProcess(QUERY_LIMITED + SET_QUOTA + SYNCHRONIZE) | err 5: 11, err 87: 1, ok: 127 |

### Named processes (first instance)

| PID | Image | Query | EX2 | Critical | IO | Token | Open for trim |
|---|---|---|---|---|---|---|---|
| 0 | [System Process] | err 87 |  |  |  |  | err 87 |
| 4 | System | ok | ok | ok | ok | err 5 | err 5 |
| 132 | Registry | ok | ok | ok | ok | not elevated | err 5 |
| 600 | smss.exe | ok | ok | critical | ok | elevated | err 5 |
| 764 | csrss.exe | ok | ok | critical | ok | elevated | err 5 |
| 888 | wininit.exe | ok | ok | critical | ok | elevated | err 5 |
| 980 | services.exe | ok | ok | critical | ok | elevated | err 5 |
| 1008 | lsass.exe | ok | ok | ok | ok | elevated | ok |
| 720 | svchost.exe | ok | ok | critical | ok | elevated | ok |
| 1032 | fontdrvhost.exe | ok | ok | ok | ok | not elevated | ok |
| 644 | MsMpEng.exe | ok | ok | ok | ok | elevated | err 5 |
| 7096 | winlogon.exe | ok | ok | ok | ok | elevated | ok |
| 5696 | dwm.exe | ok | ok | ok | ok | not elevated | ok |
| 5292 | sihost.exe | ok | ok | ok | ok | elevated | ok |
| 7724 | explorer.exe | ok | ok | ok | ok | elevated | ok |
| 8624 | ctfmon.exe | ok | ok | ok | ok | elevated | ok |

## Enumeration cost

Toolhelp snapshot + OpenProcess + GetProcessMemoryInfo + GetProcessTimes for 139 processes: median 2.4788ms, min 2.3545ms, max 2.7135ms (7 runs).

## PDH counters

| Counter | Value |
|---|---|
| `\Memory\Standby Cache Core Bytes` | 0 MiB |
| `\Memory\Standby Cache Normal Priority Bytes` | 810 MiB |
| `\Memory\Standby Cache Reserve Bytes` | 2499 MiB |
| `\Memory\Modified Page List Bytes` | 102 MiB |
| `\Memory\Free & Zero Page List Bytes` | 10464 MiB |
| `\Memory\Cache Bytes` | 76 MiB |
| `\Memory\Pages Input/sec` | 816.6 /s |
| `\Memory\Page Reads/sec` | 210.9 /s |

| Step | Time | Spike private bytes |
|---|---|---|
| Before (pdh.dll already loaded: false) | | 1.2 MiB |
| LoadLibraryExW(pdh.dll) | 476.9µs | 1.2 MiB |
| Open query + add counters + first collect | 288.4211ms | 6.4 MiB |
| After PdhCloseQuery | | 6.4 MiB |
| After FreeLibrary (ok: true, still loaded: false) | | 3.3 MiB |

## Trim experiment: probe with 256 MiB dirty private memory

| Point | WS MiB | Private WS MiB | Private bytes MiB | Page faults |
|---|---|---|---|---|
| before trim | 261.4 | 256.7 | 257.6 | 67099 |
| after warm re-touch | 261.4 | 256.7 | 257.6 | 67100 |
| immediately after trim | 0.0 | 0.0 | 257.6 | 67100 |
| +1 s, untouched | 0.0 | 0.0 | 257.6 | 67100 |
| after re-touch | 256.3 | 256.1 | 257.6 | 132711 |

- `SetProcessWorkingSetSize(-1, -1)` took 31.959ms, result: ok
- Re-touch of all pages while resident: 1.1419ms; after trim: 62.3063ms
- Exit check while alive: timeout (expect timeout); after exit: signaled (expect signaled), exit code 0
- Trim on exited process: err 5; memory query on exited process: ok, WS 0.0 MiB

## Trim experiment: probe with 1024 MiB dirty private memory

| Point | WS MiB | Private WS MiB | Private bytes MiB | Page faults |
|---|---|---|---|---|
| before trim | 1029.4 | 1024.7 | 1027.1 | 264095 |
| after warm re-touch | 1029.4 | 1024.7 | 1027.1 | 264095 |
| immediately after trim | 0.0 | 0.0 | 1027.1 | 264095 |
| +1 s, untouched | 0.0 | 0.0 | 1027.1 | 264095 |
| after re-touch | 1024.3 | 1024.1 | 1027.1 | 526314 |

- `SetProcessWorkingSetSize(-1, -1)` took 121.255ms, result: ok
- Re-touch of all pages while resident: 4.4603ms; after trim: 254.6704ms
- Exit check while alive: timeout (expect timeout); after exit: signaled (expect signaled), exit code 0
- Trim on exited process: err 5; memory query on exited process: ok, WS 0.0 MiB

