# UI spike on the reference laptop (RH-OWNER), 2026-11-20

Run by the owner from an elevated PowerShell with the CI artifact `ui-spike` (commit e1a0417):
`ui-bench --exe <spike>.exe --runs 10 --idle-secs 300`. The machine was under memory pressure
during the session (83–86 % load, commit ≈ 41.6 of 55 GiB, ≈ 430 pages input/s), with normal
desktop use otherwise paused.

## UI spike: Native Win32

| Metric | Value |
|---|---|
| Executable size | 0.30 MiB |
| Warm start to usable UI (N=10) | median 58.0071ms, p95 668.1682ms, min 53.5632ms |
| Private bytes at ready | 2.3 MiB |
| Idle, window visible (300 s) | CPU 0.130 % of one core; private bytes mean 2.3 MiB, max 2.4 MiB |
| Idle, closed to tray (300 s) | CPU 0.000 % of one core; private bytes mean 2.3 MiB, max 2.4 MiB |

## UI spike: Slint

| Metric | Value |
|---|---|
| Executable size | 11.55 MiB |
| Warm start to usable UI (N=10) | median 25.1042ms, p95 2.5789047s, min 23.6552ms |
| Private bytes at ready | 2.8 MiB |
| Idle, window visible (300 s) | CPU 0.125 % of one core; private bytes mean 5.5 MiB, max 5.7 MiB |
| Idle, closed to tray (300 s) | CPU 0.000 % of one core; private bytes mean 3.7 MiB, max 3.8 MiB |

With N = 10, "p95" is the slowest run. This version of `ui-bench` did not list individual runs, so
it is not known whether the slow run was the first (cold) one.

## Start-up only, 20 runs each (fresh download, unzipped into a new folder, run in this order)

`ui-bench --runs 20 --idle-secs 0`, artifact from commit d5da9ea.

| Spike | Median | p95 | Min | Each start, in order (ms) |
|---|---|---|---|---|
| Slint | 25.2 ms | 104.6 ms | 23.7 ms | 981, 25, 30, 25, 26, 24, 31, 25, 25, 24, 27, 25, 30, 25, 26, 26, 105, 24, 24, 24 |
| Native Win32 | 72.1 ms | 157.9 ms | 58.3 ms | 10131, 59, 63, 63, 66, 77, 60, 75, 72, 58, 69, 69, 104, 68, 158, 84, 83, 89, 98, 72 |

Private bytes at ready: Slint 2.7 MiB, Win32 2.4 MiB.
