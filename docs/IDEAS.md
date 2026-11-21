# Ideas

Possible features, not a plan. Based on the roadmap in the README and `docs/PLAN.md`.

## Memory
- Memory leak watch: flag processes whose private bytes grow steadily over hours.
- Per-process history graphs (working set, private bytes, page faults).
- Trim a single process from its context menu.
- Standby list purge as an explicit, admin-only action with a warning.

## Storage
- Safe discovery of caches and temp files, with review before cleanup.
- Large-file finder and disk space map.
- Cleanup of old Windows Update leftovers and crash dumps, read-only first.

## Automation
- Pressure-aware trimming with cooldowns and exclusions, off by default.
- Scheduled reports of memory pressure.

## System insight
- Active network connections per process.
- Startup items overview (read-only).
- Handle and GDI object counts to spot leaks.

## Platforms and distribution
- Linux and macOS adapters where the OS allows it.
- Installer and code signing; winget package.
- Localization (EN/RU).
