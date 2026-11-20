# Reclaim — Implementation Plan (Memory MVP)

**Status:** proposed. Based on [ARCHITECTURE.md](architecture/ARCHITECTURE.md) r2 and the review in [REVIEW.md](architecture/REVIEW.md).
**Principle:** each milestone ends in something measurable and keeps `main` releasable. Risky unknowns are front-loaded into M0.

## 1. Milestone overview

| # | Milestone | Outcome | Depends on | Relative size |
|---|---|---|---|---|
| M0 | Foundations and spikes | Workspace, CI, license; UI toolkit chosen on evidence; API behaviour verified | Owner decisions Q1–Q4 | M |
| M1 | Core domain | Pure, tested model, policy, analysis and report wording | M0 workspace | M |
| M2 | Windows platform adapter | Real metrics, enumeration, identity, trimming with integration tests | M1 ports | M |
| M3 | Engine and CLI | Threaded engine, run state machine, follow-up, persistence; `reclaim-cli` usable end-to-end | M1, M2 | L |
| M4 | Desktop UI and tray | All MVP screens, tray lifecycle, single instance | M3, toolkit from M0 | L |
| M5 | Benchmarks and effectiveness | Harness, probes, scenario runs, first published results | M3 (M4 for UI metrics) | M |
| M6 | Hardening and release | Accessibility pass, soak, manual checklist, reproducible release | M4, M5 | M |

M2 and the M5 probe tooling can proceed in parallel once M1's ports exist. M4 can start on ViewModels against the fake engine during M3.

## 2. Milestones in detail

### M0 — Foundations and spikes

Tasks:
1. Choose project license (owner) and add `LICENSE`, `THIRD_PARTY_NOTICES` scaffold.
2. Cargo workspace with empty crates per ARCHITECTURE §3; `rust-toolchain.toml`; rustfmt/clippy config; `#![forbid(unsafe_code)]` in core/engine.
3. CI: Windows + Linux jobs (fmt, clippy `-D warnings`, test), `cargo deny` (licenses, advisories, ban networking crates), binary-size report artifact.
4. **API spike** (throwaway, in `reclaim-bench`): verify every **[verify]** item in OS-CAPABILITIES §2 on Windows 10 22H2 and 11; measure `SetProcessWorkingSetSize` duration vs WS size; check PDH init cost; record access results for elevated/other-user/protected targets.
5. **UI toolkit spike:** identical minimal UI in Slint (software renderer) and native Win32; measure ADR-0001 gates per BENCHMARKS B1–B3, B6; Narrator/NVDA check.
6. Update ADR-0001 to Accepted with measurements; update OS-CAPABILITIES with verified facts.

Exit criteria: CI green on empty workspace; ADR-0001 accepted; spike report committed in `docs/spikes/`.
DoD coverage: ACC §6 (ADR, technology comparison with evidence).

### M1 — Core domain (`reclaim-core`, `reclaim-platform-fake`)

1. `Bytes` and formatting (IEC), metric ids and glossary text keys.
2. `ProcessKey`, `ProcessInfo`, `MemorySnapshot`, `SystemSnapshot`, `Capabilities`.
3. Protection lists as versioned data; user exclusion rules.
4. Eligibility evaluator with ordered rules and reasons (ARCH §5.4).
5. Plan builder (selection, ordering largest-first, counts by reason).
6. Outcome model and run record; serialization schema v1.
7. Analysis: deltas, rebound, cost indicators, late-sample flags.
8. Report statement generator and wording rules; property tests for forbidden phrases and totals; snapshot tests.
9. Fake platform with scripting (access denied, exit at step, PID reuse, slow calls, metric sequences).

Exit: ≥ 90 % line coverage in `core` (pure logic), all rules covered by named tests.
DoD coverage: ACC §1 (definitions, per-process outcomes, WS ≠ commit ≠ performance), §6 (unit tests).

### M2 — Windows adapter (`reclaim-platform-windows`)

1. RAII `OwnedHandle`; error mapping to `PlatformError`/`OpenError`/`ReclaimError`.
2. System metrics (`GlobalMemoryStatusEx`, `GetPerformanceInfo`), pressure notification handle.
3. Lazy PDH reader with English counter names; graceful absence.
4. Enumeration (Toolhelp) + per-process memory, start time, critical flag, session, integrity hint; foreground process.
5. Open-and-validate by `ProcessKey`; trim via `SetProcessWorkingSetSize(-1, -1)`.
6. Capability discovery.
7. Integration tests on CI Windows runner: probe child trim (WS decreases, private bytes unchanged), access denied (System/csrss), exit race, PID-reuse defence (simulated via start-time mismatch), unelevated run.
8. CI lint: fail if banned APIs (`TerminateProcess`, `NtSetSystemInformation`, `SetSystemFileCacheSize`, `AdjustTokenPrivileges` with debug privilege) appear outside `reclaim-bench`.

Exit: integration suite green on Windows 10 and 11 (CI image + one local machine).
DoD coverage: ACC §1 (skip/report, no elevation, never terminate), §4 (races), §6 (integration tests).

### M3 — Engine and CLI (`reclaim-engine`, `reclaim-cli`)

1. Engine thread with command queue + waitable timer + pressure handle wait loop.
2. Sampling cadence per view state (ARCH §4.1).
3. Run state machine with cancel, follow-up (+10 s, +60 s), late-sample flags, held handles, `catch_unwind`.
4. `DryRun` mode.
5. Settings store (TOML, schema version, atomic write, corrupt-file fallback); history store (JSONL, caps, clear).
6. Local log sink (off by default).
7. `reclaim-cli`: `status`, `list [--eligible]`, `plan`, `trim [--dry-run] [--json]`.
8. Tests: state machine with fake platform and fake clock; persistence round trips; soak B8 (first run).

Exit: a full trim with follow-up works from the CLI on a real machine; soak shows no growth.
DoD coverage: ACC §1 (before/after/follow-up, cancel), §4 (no leaks, local logs).

### M4 — Desktop UI and tray (`reclaim-app`)

1. ViewModels (Overview, Processes, Plan, Result, History, Settings) with tests against the fake engine.
2. Toolkit adapter for the chosen toolkit; string resources; formatter.
3. Startup sequence with "usable UI" marker; `--tray` mode.
4. Tray icon, hover tooltip, menu, `TaskbarCreated` handling; Close vs Quit semantics; single instance and activation.
5. Destroy window on hide (if supported) and verify B3.
6. Keyboard navigation, focus order, high contrast, Per-Monitor V2 DPI.

Exit: all MVP screens usable by keyboard and screen reader; B1–B3 within targets or deviations documented.
DoD coverage: ACC §1 (responsive UI), §2, §4 (tray lifecycle), PROD §7 (accessibility, localization-ready).

### M5 — Benchmarks and effectiveness (`reclaim-bench`)

1. Harness for B1–B8 with result archiving.
2. Probes `alloc-touch`, `latency`, `exit-after`.
3. Scenario runner S1–S8 with control/trim arms.
4. First full run on RH-LOW and RH-MID; publish `docs/benchmarks/<date>-summary.md`.
5. Tune defaults (threshold, caution list, pacing) from evidence; record changes in an ADR if behaviour changes.

Exit: published summary covering all scenarios; any regression documented with a mitigation or an explicit acceptance.
DoD coverage: ACC §2, §3.

### M6 — Hardening and release

1. Accessibility review (Narrator, NVDA, keyboard-only, high contrast).
2. Manual test checklist `docs/testing/MANUAL_CHECKLIST.md` executed on VM-CLEAN (Win10, Win11) as standard user.
3. Final soak B8; clean-VM launch without runtimes.
4. Release workflow: tagged build in CI, zip + `SHA256SUMS`, third-party notices generated, reproducibility check documented.
5. README update: what Reclaim does and does not do, with links to benchmark summaries; privacy statement.
6. Optional: code signing (owner decision).

Exit: MVP release candidate tagged; all ACCEPTANCE §1, §2 (or documented deviations), §3, §4 items checked with evidence links.

## 3. First tasks to open as issues (M0/M1)

1. Decide and add project license.
2. Create Cargo workspace skeleton and CI (fmt, clippy, test, deny, size report).
3. API verification spike on Windows 10 22H2 / 11 — produce `docs/spikes/windows-api.md`.
4. UI toolkit spike: Slint vs native Win32 — produce `docs/spikes/ui-toolkit.md`, finalize ADR-0001.
5. `reclaim-core`: units, metric glossary, identity and snapshot types.
6. `reclaim-core`: eligibility evaluator and protection lists with tests.
7. `reclaim-platform-fake`: scriptable fake platform.

## 4. After the MVP (not planned in detail)

- **Phase 2 Storage:** `reclaim-storage` per ARCHITECTURE §12.1; start with a read-only scanner and Roomy category study (license notices if adapting).
- **Phase 3 Automation:** trigger/policy evaluator per §12.2; requires the M5 evidence to pick safe defaults.
- **Phase 4 Platforms:** Linux adapter first (PSI + `process_madvise`); macOS monitoring + storage only.
- **Optional:** elevated broker per ADR-0002, installer, autostart.

## 5. Open questions for the owner

| # | Question | Why it matters | Proposed default |
|---|---|---|---|
| Q1 | Project license? | Determines whether Slint can be used under GPLv3 or needs the royalty-free/commercial license; release notices | MIT or Apache-2.0 for the code, Slint royalty-free with attribution if Slint wins |
| Q2 | Minimum supported Windows version? | API availability, test matrix | Windows 10 22H2 and Windows 11 23H2+ |
| Q3 | Which physical machines can serve as RH-LOW / RH-MID? | Performance targets are unverifiable without them | Owner's laptop as RH-LOW, desktop as RH-MID |
| Q4 | Preference if both toolkits pass the gates? | Portability vs smallest footprint | Slint (one UI for all platforms) |
| Q5 | Close (X) behaviour by default? | Tray UX | Ask once on first close, remember |
| Q6 | Is an elevated broker wanted later? | Scope of post-MVP work and threat review | Not planned until users ask |
| Q7 | Distribution: portable zip only, or installer? Code signing? | Release workflow, SmartScreen | Portable zip for MVP; signing decided before public release |
| Q8 | Keep project docs in English? | Repository convention | Yes (existing docs are English) |
