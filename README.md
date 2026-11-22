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

## What it does not do

- It does not free committed memory, make programs faster, or "clean" RAM. Trimmed pages come back when processes use them, and the report shows this.
- It never terminates processes or changes system settings, and it needs no administrator rights.
- It runs no automatic or scheduled trimming in this release.

## Privacy

Reclaim works offline and has no account, telemetry or network access. Settings and run history stay in `%LOCALAPPDATA%\Reclaim` (`settings.toml`, `history.jsonl`). Only image names are stored, never paths or command lines. An error log is off by default. Settings → *Clear history and logs* deletes them.

## Using it

- `reclaim.exe` opens the window, and `reclaim.exe --tray` starts in the notification area. Close (X) keeps Reclaim in the tray; tray → *Quit* exits.
- `reclaim-cli.exe status | list [--eligible] | plan | trim [--dry-run] [--yes] [--json]` is the headless front end.

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
