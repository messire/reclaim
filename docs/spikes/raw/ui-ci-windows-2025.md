# UI spike, CI run (windows-2025, elevated runner, no GPU, no interactive user)

Commit e1a0417, workflow run 44734128969, job "UI toolkit spike (M0)". Command:
`ui-bench --exe <spike>.exe --label <label> --runs 10 --idle-secs 60`.

## UI spike: Native Win32

| Metric | Value |
|---|---|
| Executable size | 0.30 MiB |
| Warm start to usable UI (N=10) | median 46.9645ms, p95 79.2635ms, min 43.0455ms |
| Private bytes at ready | 2.3 MiB |
| Idle, window visible (60 s) | CPU 0.052 % of one core; private bytes mean 2.3 MiB, max 2.4 MiB |
| Idle, closed to tray (60 s) | CPU 0.000 % of one core; private bytes mean 2.3 MiB, max 2.3 MiB |

## UI spike: Slint (software renderer)

| Metric | Value |
|---|---|
| Executable size | 11.55 MiB |
| Warm start to usable UI (N=10) | median 20.1411ms, p95 197.8504ms, min 19.6139ms |
| Private bytes at ready | 2.7 MiB |
| Idle, window visible (60 s) | CPU 0.000 % of one core; private bytes mean 5.5 MiB, max 5.6 MiB |
| Idle, closed to tray (60 s) | CPU 0.000 % of one core; private bytes mean 3.6 MiB, max 3.7 MiB |

Slint printed "ready marker: rendering notifier unavailable, using first event-loop turn" on every
run: the software renderer does not support `set_rendering_notifier`, so "ready" is the first
event-loop turn after `show()`, not a confirmed painted frame.

## Previous run (commit 27adae6, run 44732081818), for reference

The Slint tray had no icon and failed to create ("Failed to create a rgba8 buffer from an icon
image"). Win32: start median 61.8 ms / p95 111.7 ms, idle visible 0.052 % CPU. Slint: start median
19.7 ms / p95 5.23 s (one outlier), idle visible 0.130 % CPU / 5.4 MiB, tray 0.000 % / 3.6 MiB.
The 0.13 % idle CPU did not reproduce once the tray icon existed.
