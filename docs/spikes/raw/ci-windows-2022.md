# Windows API spike report

## Environment

| Item | Value |
|---|---|
| Product | Windows Server 2022 Datacenter |
| Display version | 21H2 |
| Build | 20348.5870 |
| Spike elevated | Ok(true) |
| Spike private bytes at start | 1.1 MiB |
| Logical CPUs | 4 |

> Note: Windows 11 reports ProductName "Windows 10" in the registry; use the build number (≥ 22000 = Windows 11).

## System metrics

| Metric | Value | Source | Avg call cost |
|---|---|---|---|
| Total physical | 16379 MiB | GlobalMemoryStatusEx | 656ns |
| Available physical | 14114 MiB | GlobalMemoryStatusEx | |
| Memory load | 13 % | GlobalMemoryStatusEx | |
| Commit charge | 2259 MiB | GetPerformanceInfo | 558.293µs |
| Commit limit | 19323 MiB | GetPerformanceInfo | |
| System cache | 3734 MiB | GetPerformanceInfo | |
| Processes | 121 | GetPerformanceInfo | |

## Memory resource notifications

| Handle | Query state | Wait(0) #1 | Wait(0) #2 |
|---|---|---|---|
| Low | not signaled | timeout | timeout |
| High | signaled | signaled | signaled |

If a handle is signaled on both waits, the notification is level-triggered (REVIEW F27).

## Foreground window

Foreground PID: 4392

## Process access matrix (121 processes)

Error 5 = access denied, 87 = invalid parameter.

| Test | Results |
|---|---|
| OpenProcess(QUERY_LIMITED) | err 87: 1, ok: 120 |
| GetProcessMemoryInfo(EX2) | ok: 120 |
| GetProcessTimes | ok: 120 |
| IsProcessCritical | critical: 9, ok: 111 |
| GetProcessIoCounters (limited handle) | ok: 120 |
| OpenProcessToken + TokenElevation | elevated: 108, err 5: 1, not elevated: 11 |
| OpenProcess(QUERY_LIMITED + SET_QUOTA + SYNCHRONIZE) | err 5: 12, err 87: 1, ok: 108 |

### Named processes (first instance)

| PID | Image | Query | EX2 | Critical | IO | Token | Open for trim |
|---|---|---|---|---|---|---|---|
| 0 | [System Process] | err 87 |  |  |  |  | err 87 |
| 4 | System | ok | ok | ok | ok | err 5 | err 5 |
| 124 | Registry | ok | ok | ok | ok | not elevated | err 5 |
| 492 | smss.exe | ok | ok | critical | ok | elevated | err 5 |
| 628 | csrss.exe | ok | ok | critical | ok | elevated | err 5 |
| 756 | wininit.exe | ok | ok | critical | ok | elevated | err 5 |
| 868 | services.exe | ok | ok | critical | ok | elevated | err 5 |
| 896 | lsass.exe | ok | ok | ok | ok | elevated | ok |
| 1020 | svchost.exe | ok | ok | critical | ok | elevated | ok |
| 536 | fontdrvhost.exe | ok | ok | ok | ok | not elevated | ok |
| 3536 | MsMpEng.exe | ok | ok | ok | ok | elevated | err 5 |
| 6732 | winlogon.exe | ok | ok | ok | ok | elevated | ok |
| 6828 | dwm.exe | ok | ok | ok | ok | not elevated | ok |
| 6456 | sihost.exe | ok | ok | ok | ok | elevated | ok |
| 6168 | ctfmon.exe | ok | ok | ok | ok | elevated | ok |
| 5608 | explorer.exe | ok | ok | ok | ok | elevated | ok |

## Enumeration cost

Toolhelp snapshot + OpenProcess + GetProcessMemoryInfo + GetProcessTimes for 121 processes: median 3.4401ms, min 3.307ms, max 4.0083ms (7 runs).

## PDH counters

| Counter | Value |
|---|---|
| `\Memory\Standby Cache Core Bytes` | 0 MiB |
| `\Memory\Standby Cache Normal Priority Bytes` | 865 MiB |
| `\Memory\Standby Cache Reserve Bytes` | 2784 MiB |
| `\Memory\Modified Page List Bytes` | 231 MiB |
| `\Memory\Free & Zero Page List Bytes` | 10437 MiB |
| `\Memory\Cache Bytes` | 66 MiB |
| `\Memory\Pages Input/sec` | 10.0 /s |
| `\Memory\Page Reads/sec` | 10.0 /s |

| Step | Time | Spike private bytes |
|---|---|---|
| Before (pdh.dll already loaded: false) | | 1.1 MiB |
| LoadLibraryExW(pdh.dll) | 499.5µs | 1.1 MiB |
| Open query + add counters + first collect | 234.971ms | 7.0 MiB |
| After PdhCloseQuery | | 5.6 MiB |
| After FreeLibrary (ok: true, still loaded: false) | | 2.9 MiB |

## Trim experiment: probe with 256 MiB dirty private memory

| Point | WS MiB | Private WS MiB | Private bytes MiB | Page faults |
|---|---|---|---|---|
| before trim | 260.3 | 0.0 | 257.5 | 66825 |
| after warm re-touch | 260.3 | 0.0 | 257.5 | 66826 |
| immediately after trim | 0.0 | 0.0 | 257.5 | 66826 |
| +1 s, untouched | 0.0 | 0.0 | 257.5 | 66826 |
| after re-touch | 256.3 | 0.0 | 257.5 | 132438 |

- `SetProcessWorkingSetSize(-1, -1)` took 28.4433ms, result: ok
- Re-touch of all pages while resident: 940.9µs; after trim: 64.6787ms
- Exit check while alive: timeout (expect timeout); after exit: signaled (expect signaled), exit code 0
- Trim on exited process: err 5; memory query on exited process: ok, WS 0.0 MiB

## Trim experiment: probe with 1024 MiB dirty private memory

| Point | WS MiB | Private WS MiB | Private bytes MiB | Page faults |
|---|---|---|---|---|
| before trim | 1028.4 | 0.0 | 1027.0 | 263823 |
| after warm re-touch | 1028.4 | 0.0 | 1027.0 | 263823 |
| immediately after trim | 0.0 | 0.0 | 1027.0 | 263823 |
| +1 s, untouched | 0.0 | 0.0 | 1027.0 | 263823 |
| after re-touch | 1024.3 | 0.0 | 1027.0 | 526042 |

- `SetProcessWorkingSetSize(-1, -1)` took 111.9307ms, result: ok
- Re-touch of all pages while resident: 4.0668ms; after trim: 260.7152ms
- Exit check while alive: timeout (expect timeout); after exit: signaled (expect signaled), exit code 0
- Trim on exited process: err 5; memory query on exited process: ok, WS 0.0 MiB

