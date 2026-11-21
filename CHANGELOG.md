# Changelog

## [Unreleased] — memory MVP (Windows)
- Core domain and fake platform for tests (line coverage ≥ 90 %).
- Windows adapter: system memory, per-process working set and private bytes, trim eligibility with reasons.
- Engine and CLI: `status`, `list`, `plan`, `trim` with dry run, confirmation and JSON output.
- Result report immediately, after 10 s and 60 s: reduced working set, rebound, page faults, commit change.
- Slint desktop UI, tray mode, single instance.
- Benchmark harness: probes, scenarios S1–S8, soak test, CI smoke run.
- Tag-driven release workflow: zip, `SHA256SUMS`, third-party notices.
- MIT license.
