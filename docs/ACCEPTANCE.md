# Reclaim — Acceptance Criteria and Benchmark Plan

**Status:** proposed engineering targets, not verified guarantees. Define reference hardware and repeatable measurement procedures before claiming compliance.

## 1. Windows memory MVP — functional acceptance

- [ ] Launches on supported Windows 10/11 x64 systems without requiring a separately installed application runtime.
- [ ] Shows correct system memory metrics and process working sets with documented units and definitions.
- [ ] Supports a manual, explicit optimization action with process exclusions.
- [ ] Never terminates processes or silently changes system configuration.
- [ ] Skips inaccessible, exited and protected processes safely; reports per-process outcomes.
- [ ] Works without elevation for permitted operations and does not require admin just to view metrics.
- [ ] Remains responsive during scanning and trimming; operations can be cancelled where technically feasible.
- [ ] Shows before/after measurements, elapsed time and a follow-up sample to reveal rebound.
- [ ] Differentiates working-set reduction from committed memory release and from actual performance improvement.
- [ ] Keeps settings local; does not require network, login or telemetry.

## 2. Preliminary performance targets

| Metric | Initial target | Validation |
|---|---|---|
| Cold UI startup | < 300 ms | Repeated cold-start trials on defined reference hardware; measure to usable UI |
| Idle private memory | < 15 MiB preferred | Stabilized idle samples; distinguish private bytes from working set |
| Idle CPU | Approximately 0% | Observe over a multi-minute idle interval |
| Typical manual optimization | < 1 s preferred | Define process count and workload; measure elapsed time and failures |
| Binary distribution | Prefer < 10 MiB | Report executable and packaged distribution sizes separately |
| Runtime dependency | No separately installed runtime | Test clean supported Windows environment |

Targets are aspirational and subject to feasibility review. Do not game them by deferring required work or hiding resident helper processes.

## 3. Effectiveness and regression tests

For each representative workload, capture:
1. Available physical RAM, system commit and relevant process working sets before operation.
2. Actual actions attempted, errors, skipped targets and elapsed time.
3. Immediate post-operation values and samples after 10 s and 60 s.
4. Working-set rebound, page-fault / hard-fault indicators where measurable, disk activity and observable application latency.
5. Comparison against a no-op control; optionally compare against Wise Memory Optimizer on the same machine and workload.

Test scenarios should include idle desktop, browser with multiple tabs, a recently memory-intensive app, foreground interactive application, low-memory pressure, high-memory pressure, protected/inaccessible processes and concurrent process exits.

**Pass condition:** correct, safe behavior and honest reporting. A lower working-set number alone is not a performance pass. If a workload becomes less responsive, record a regression even if more RAM appears available.

## 4. Reliability and safety

- [ ] Repeated runs do not leak handles or exhibit unbounded memory growth.
- [ ] Process termination races, permission failures and invalid IDs do not crash the application.
- [ ] Core system processes are excluded by policy where appropriate.
- [ ] Automatic actions are disabled until explicitly enabled in a later release.
- [ ] UI closure and tray lifecycle are predictable; Quit stops background work.
- [ ] Logs, if any, remain local and do not expose sensitive filenames or user data unnecessarily.

## 5. Storage phase — future acceptance

- [ ] Scanning is read-only; every destructive operation requires review and confirmation.
- [ ] Personal data and critical system paths are protected.
- [ ] Symlinks, junctions, mount points and path races cannot redirect deletion into protected locations.
- [ ] Locked files are skipped; failures are reported.
- [ ] Space map and candidate sizes are accurate enough to explain disk use.
- [ ] No silent background deletion.

## 6. Delivery expectations

Before full implementation, produce an architecture decision record, technology comparison, OS capability/risk analysis, implementation milestones and benchmark methodology. Include automated tests for pure logic and integration tests for platform-specific operations. Provide reproducible release builds and a concise manual test checklist.
