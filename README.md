# Reclaim

**Reclaim your resources.**

Reclaim is a lightweight, offline-first system utility for monitoring and reclaiming system resources without becoming a resource hog itself.

## Product principles

- **Fast by default.** Launch quickly, remain responsive, and consume minimal CPU and memory at idle.
- **Measure real outcomes.** Report changes in memory availability and process working sets, but do not mistake lower displayed memory usage for better performance.
- **Safe and transparent.** Explain operations, their limitations and possible side effects. Prefer documented APIs and least privilege.
- **User control.** Manual actions first; optional, conservative automation later.
- **Portable architecture.** Windows is the initial target, with Linux and macOS considered in core interfaces and design.
- **Offline and private.** No account, advertising, telemetry or mandatory network access.

## Roadmap

1. **Memory MVP (Windows):** monitoring, per-process diagnostics, manual working-set trimming, exclusions, result measurements and a lightweight desktop/tray experience.
2. **Storage:** safe cache and temporary-file discovery, reviewed cleanup, large-file analysis and a disk space map.
3. **Smart automation:** pressure-aware, conservative policies with cooldowns, exclusions and measurable outcomes.
4. **Additional platforms:** Linux and macOS implementations subject to platform capabilities and safety constraints.

## What Reclaim does (memory MVP, Windows 10 22H2 / 11 23H2+)

- Shows system memory: available and installed physical memory, committed memory against the commit limit, and memory pressure. Optional details add the standby and modified lists and the paging rate.
- Lists processes with their working set, private bytes, and whether they can be trimmed.
  - If a process can't be trimmed, the list says why: a system process, protected by default, in the foreground, needs administrator rights, excluded by you, or below the size threshold.
- On your request only, after a confirmation, trims the working sets of eligible processes (`SetProcessWorkingSetSize(-1, -1)`). A dry run (CLI only) measures the same steps without trimming, as a control.
- Reports what changed, immediately and 10 s and 60 s later:
  - the working set reduced, and how much came back (rebound);
  - page faults and the paging rate;
  - the change in committed memory, which trimming does not release.

## Storage (0.3.0)

- **Disk usage:** scan drives or folders and see where the space goes: a directory tree with sizes and shares, a treemap or icicle view coloured by file type, the largest files and folders, extension statistics and a list of what could not be read. Search the results by name, path, extension, size or age without scanning again. Export to JSON (CSV from the CLI).
- **Cleanup:** examine temporary files, caches, crash dumps, the Recycle Bin, old installers and large old downloads. Each category explains what it holds and what happens after cleaning. Nothing changes until you confirm; caches are deleted, your own files go to the Recycle Bin, and the report shows the estimate next to the free space Windows actually measured.
- **Duplicates:** find identical files in the scanned area; you choose which copies go to the Recycle Bin, and one copy always stays.
- Works without administrator rights. If you start Reclaim as administrator yourself, whole NTFS drives are read with a faster method.

See [docs/storage/ARCHITECTURE.md](docs/storage/ARCHITECTURE.md), [ADR-0004](docs/adr/0004-storage.md) and [benchmarks](docs/benchmarks/storage.md).

## What it does not do

- It does not free committed memory, make programs faster, or "clean" RAM. Trimmed pages come back when processes use them, and the report shows this.
- It never terminates processes or changes system settings, and it needs no administrator rights.
- It runs no automatic or scheduled trimming in this release.

## Privacy

Reclaim works offline and has no account, telemetry or network access. Settings and run history stay in `%LOCALAPPDATA%\Reclaim` (`settings.toml`, `history.jsonl`, `storage.toml`, `cleanup-history.jsonl`; the cleanup history holds categories and byte counts, no file names). Only image names are stored, never paths or command lines. An error log is off by default. Settings → *Clear history and logs* deletes them.

## Installing

Download `reclaim-<version>-windows-x64-setup.exe` (installer: choose a folder, optionally start with Windows; a newer version replaces the installed one) or the portable `reclaim-<version>-windows-x64.zip`. To build both from source on Windows, run `scripts\build-installer.cmd` in the repository folder (or double-click it); they land in `dist\`. See [installer/README.md](installer/README.md).

## Using it

- `reclaim.exe` opens the window, and `reclaim.exe --tray` starts in the notification area. Close (X) keeps Reclaim in the tray; tray → *Quit* exits.
- `reclaim-cli.exe status | list [--eligible] | plan | trim [--dry-run] [--yes] [--json]` is the headless front end.
- `reclaim-cli.exe storage drives | scan <path>... [--fast] [--top N] [--json F] [--csv F] | dupes <path>... | cleanup [--category ID]... [--run --yes]` analyses and cleans storage.

Effectiveness claims must cite a published benchmark summary under [`docs/benchmarks/`](docs/benchmarks/). [How to run the benchmarks](docs/benchmarks/RUNNING.md) explains how they are produced.

## Development status

M0 to M4 are implemented (core, Windows adapter, engine, CLI, Slint UI and tray). M5 (benchmark harness) and M6 (release) are in progress; see the [implementation plan](docs/PLAN.md). Rust, a single unelevated process, documented Win32 APIs only.

Further documents:
- Spike results: [Windows API](docs/spikes/windows-api.md), [UI toolkit](docs/spikes/ui-toolkit.md) (Slint chosen).
- [Product requirements](docs/PRODUCT.md), [Acceptance criteria](docs/ACCEPTANCE.md), [References](docs/REFERENCES.md).
- The architecture set:
  - [Gap analysis](docs/architecture/ANALYSIS.md)
  - [Architecture](docs/architecture/ARCHITECTURE.md) and its [critical review](docs/architecture/REVIEW.md)
  - [OS capabilities and risks](docs/architecture/OS-CAPABILITIES.md)
  - [Benchmark methodology](docs/architecture/BENCHMARKS.md)
  - [Architecture decision records](docs/adr/)
  - [Manual test checklist](docs/testing/MANUAL_CHECKLIST.md)

## Versioning

Versions are `X.Y.Z` ([SemVer](https://semver.org)). A release bumps the most significant part that applies to the changes since the previous release:

- **Z (patch)**: fixes only. No new features, and no change to the format of settings or history files (`settings.toml`, `storage.toml`, `history.jsonl`, `cleanup-history.jsonl`).
- **Y (minor)**: a new feature or tab. While the version is 0.x, also any breaking change: settings or history format, CLI commands and options, default behaviour.
- **X (major)**: 1.0.0 is set when the owner declares the product stable. After that, X goes up only for breaking changes, and new features go to Y.

Changes collect under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md); a release renames that heading to the new version.

## Building

Requires the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by `rustup`).

```sh
cargo test --workspace                                 # platform-independent crates, any OS
cargo build --release -p reclaim-app -p reclaim-cli    # Windows: reclaim.exe, reclaim-cli.exe
python scripts/third_party_notices.py                  # THIRD_PARTY_NOTICES.txt for a release
```

The workspace layout follows [ARCHITECTURE §3](docs/architecture/ARCHITECTURE.md#3-logical-architecture). Pushing a tag `vX.Y.Z` runs the Release workflow, which produces a draft release with the zip, `SHA256SUMS` and the notices.

## License

Reclaim is released under the [MIT License](LICENSE) (decided 2026-11-20: free distribution, permissive). Reference projects have their own licenses; comply with their terms if incorporating code.
