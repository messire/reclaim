# Changelog

## [Unreleased]
- Installer (Inno Setup, per-user, no admin): choose the folder, optional start with Windows; a newer version replaces the installed one, never side by side. Built with `scripts\build-installer.cmd` from a fresh clone; the portable zip stays. CI tests install, upgrade, move and uninstall.
- Settings → *Start Reclaim with Windows (in the tray)*; like every setting it takes effect on *Save*.
- Overview bar shows memory in use (installed minus available) with a teal → amber → red gradient.
- Settings and About are icons on the right of the tab row; About is the rightmost.
- Removed the Dry run button and the Result and History tabs; a status bar shows run progress (with Cancel) and the last trim, e.g. "30 trimmed · 4.5 GiB". *Clear history and logs* moved to Settings. Dry runs and the full report stay in the CLI and `history.jsonl`.
- About: the Slint attribution (AboutSlint, required by its royalty-free license) is smaller and labelled as a third-party toolkit.

## [0.1.0] — memory MVP (Windows)
- Core domain and fake platform for tests (line coverage ≥ 90 %).
- Windows adapter: system memory, per-process working set and private bytes, trim eligibility with reasons.
- Engine and CLI: `status`, `list`, `plan`, `trim` with dry run, confirmation and JSON output.
- Result report immediately, after 10 s and 60 s: reduced working set, rebound, page faults, commit change.
- Slint desktop UI, tray mode, single instance.
- Dark monospace window style: memory bars on the overview, custom process table, dark title bar.
- App icon in the exe and window; tray icon shows the state (normal, high pressure, working, unavailable).
- Benchmark harness: probes, scenarios S1–S8, soak test, CI smoke run.
- Tag-driven release workflow: zip, `SHA256SUMS`, third-party notices.
- MIT license.
