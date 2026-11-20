# Reclaim — Critical Review of the Architecture

**Status:** review of the architecture draft against `PRODUCT.md`, `ACCEPTANCE.md` (the general DoD) and `REFERENCES.md`, with the resolution applied in [`ARCHITECTURE.md`](ARCHITECTURE.md) r2.

The review was done in two passes:

- **Pass 1** — the first draft (r1), which had the same layering as r2 but several product-level decisions that conflicted with the requirements.
- **Pass 2** — a re-read of the revised design for remaining gaps, focusing on idle behaviour, lifecycle edge cases and honesty of reporting.

Severity: **Blocker** = violates a stated requirement or DoD item; **Major** = likely to fail acceptance or cause a safety issue; **Minor** = quality or clarity.

## 1. Pass 1 — findings on draft r1

| # | Severity | Draft r1 said | Requirement / DoD conflict | Resolution in r2 |
|---|---|---|---|---|
| F1 | Blocker | Sample system and process metrics every 1 s always, to keep the tray tooltip live | NFR: near-zero idle CPU, "avoid periodic polling that increases idle load"; ACCEPTANCE §2 idle CPU ≈ 0 % | No sampling while hidden; tooltip computed on hover; OS low-memory notification for events (ARCH §4.1, §8.2) |
| F2 | Blocker | Optional "Deep clean" that purges the standby list with elevation | PRODUCT §4: no changing system settings; least privilege; REFERENCES: prefer documented APIs | Removed. ADR-0003 confines undocumented mechanisms to research / experimental feature |
| F3 | Blocker | Result screen headline "Freed N MB" | PRODUCT §4: do not label all metrics "used RAM", trimmed WS ≠ freed commit; ACCEPTANCE §1 differentiate WS reduction / commit release / performance | Report wording rules in `core` (ARCH §5.6); commit always shown; forbidden phrases tested by property tests |
| F4 | Major | Processes identified by PID from the enumeration snapshot | ACCEPTANCE §4: termination races and invalid IDs must not cause harm | `ProcessKey = (pid, start_time)`, validated handle held through follow-up (ARCH §5.2) |
| F5 | Major | "Trim all accessible processes" including foreground, `dwm.exe`, `audiodg.exe` | PRODUCT §4: never assume all processes are safe; ACCEPTANCE §3 regressions in foreground apps | Ordered eligibility rules, system-critical and caution lists, foreground excluded by default (ARCH §5.4, OS-CAP §4) |
| F6 | Major | "Run as administrator" button relaunching the whole app elevated | PRODUCT §4: elevation only for a specific justified capability | ADR-0002: no whole-app elevation; future single-purpose broker designed only |
| F7 | Major | Platform trait `fn empty_working_set(pid)` | PRODUCT §3, §7: portable core, capabilities explicitly reported | `ReclaimAction` + `Capabilities` with `Availability` (ARCH §5.3, §6) |
| F8 | Major | No cost measurement after trimming | PRODUCT §4 Diagnostics: page faults, disk I/O, responsiveness where feasible | Page-fault and I/O deltas per process, system paging-input rate during follow-up (ARCH §5.5) |
| F9 | Major | No control measurement | ACCEPTANCE §3.5: compare against a no-op control | `DryRun` mode runs the identical pipeline without the OS call (ARCH §5.6, §7) |
| F10 | Major | Slint chosen as the toolkit | README/PRODUCT §8: no stack mandated; decide by measurement; license not chosen | ADR-0001: Slint vs native Win32 spike with explicit gates; license dependency recorded |
| F11 | Major | `tokio` runtime for background work | NFR: small idle footprint, predictable resource use | `std` threads, one engine thread, `WaitForMultipleObjects` (ARCH §4) |
| F12 | Major | Process list enumerated before the first window paint | ACCEPTANCE §2: < 300 ms to usable UI | First paint uses µs-cost system call only; process list async (ARCH §4.2) |
| F13 | Major | Cancel button with no defined semantics | ACCEPTANCE §1: cancel where technically feasible | Cooperative cancel between processes, defined outcome `Cancelled`, follow-up behaviour defined (ARCH §7) |
| F14 | Major | Close button hides the window; Quit only from tray | ACCEPTANCE §4: predictable tray lifecycle; Quit stops background work | Explicit Close vs Quit semantics; first-close choice; Quit releases everything (ARCH §8.2) |
| F15 | Minor | History stores executable full paths and command lines for diagnostics | ACCEPTANCE §4: logs must not expose sensitive filenames unnecessarily | Image names only; no command lines or titles; capped files; clear button (ARCH §9) |
| F16 | Minor | Accessibility and localization "later" | PRODUCT §7: accessible, localization-ready UI | Toolkit gate includes screen-reader test; strings externalized from day one (ARCH §8.3) |
| F17 | Minor | Policy checks embedded in the Windows adapter | ACCEPTANCE §6: automated tests for pure logic | Eligibility is a pure function in `core` with a fake platform (ARCH §3.1, §10) |
| F18 | Minor | Window hidden but kept alive in tray mode | Idle memory target | Destroy window on hide where toolkit allows; measured in M0 (ARCH §8.2) |
| F19 | Minor | No handle-leak test | ACCEPTANCE §4: repeated runs do not leak handles | Soak test with handle/GDI/USER counts (ARCH §10) |

## 2. Pass 2 — findings on the revised design

| # | Severity | Issue found | Requirement / DoD | Change applied |
|---|---|---|---|---|
| F20 | Major | With the window visible, refreshing the process table every 2 s means opening ~300 processes every 2 s: plausibly 0.2–0.5 % CPU, which fails the idle CPU gate if "idle" is measured with the window open | ACCEPTANCE §2 idle CPU ≈ 0 % | Process table refresh only while the Processes screen is visible, every 3 s, with row diffing; benchmark defines idle as Overview visible or tray, and reports the Processes screen separately as "active monitoring" (ARCH §4.1, BENCH §3) |
| F21 | Major | Report did not say anything about responsiveness, so silence could be read as "no regression" | PRODUCT §4: do not claim improvement without evidence; assess responsiveness where feasible | Report always includes "Responsiveness: not measured by Reclaim"; responsiveness is measured only in the benchmark harness via probe workloads (ARCH §5.6, BENCH §5) |
| F22 | Major | A panic inside a run would abort the process (release `panic = "abort"`) and lose the partial run record | ACCEPTANCE §4: no crash on races/permission failures; reliable operation | Keep unwinding; the engine wraps each command in `catch_unwind`, records an internal error outcome and stays alive (ARCH §9) |
| F23 | Minor | Tray icon disappears after Explorer restarts | ACCEPTANCE §4: predictable tray lifecycle | Handle the `TaskbarCreated` registered message and re-add the icon (ARCH §8.2) |
| F24 | Minor | Follow-up samples taken late after sleep/resume or heavy load would be presented as "+60 s" | Honest reporting | Samples carry actual timestamps; a sample more than 2 s late is flagged in the report (ARCH §7) |
| F25 | Minor | Held handles to processes that exit during follow-up keep kernel process objects alive up to 60 s | Resource hygiene | Accepted (bounded, small); handles released on Finalized or Quit; covered by the soak test (ARCH §7) |
| F27 | Major | Memory-resource notifications are level-triggered; waiting on a signaled low-memory handle again would busy-loop under sustained pressure (found in PR review) | NFR: near-zero idle CPU | Low/high handle swap plus a 30 s re-check timer while low (ARCH §6.1) |
| F28 | Minor | `WaitForSingleObject` exit check needs `SYNCHRONIZE`, which the trim handle did not request (found in PR review) | ACC §4: exit races handled | `SYNCHRONIZE` added to the open rights (ARCH §6.1, §7) |
| F26 | Minor | Users may consider image names in history sensitive | ACCEPTANCE §4 | Setting "Keep run history" (default on) and "Clear history" (ARCH §9) |

## 3. Residual risks accepted

- **Trimming may have little real benefit** on many machines (OS-CAP §1, risk O1). The architecture cannot fix this; it makes it visible. Product messaging and the storage phase carry the long-term value.
- **No whole-system responsiveness measurement in the product.** Reliable measurement needs ETW and elevation; it stays in the benchmark harness.
- **UI toolkit unknown until M0.** Mitigated by the ViewModel boundary.

## 4. Traceability: requirements and DoD → design → verification

| Requirement / DoD item | Design element | Verification | Milestone |
|---|---|---|---|
| No separately installed runtime (ACC §1) | Rust, static CRT, no WebView/.NET (ADR-0001, ARCH §11) | Clean-VM launch test | M0, M6 |
| Correct system and process metrics with definitions (ACC §1, PROD §4) | Metric glossary (ARCH §5.1) | Unit tests on conversions; cross-check against Task Manager / Resource Monitor | M1, M2 |
| Manual action with exclusions (ACC §1) | Plan → Confirm → Run; user rules (ARCH §5.4, §8.1) | Unit + integration tests; manual checklist | M1, M3, M4 |
| Never terminates processes or changes config (ACC §1) | Only `SetProcessWorkingSetSize`; ADR-0003 | Code review checklist; API usage lint (grep in CI for banned APIs) | M2 |
| Skips inaccessible/exited/protected; per-process outcomes (ACC §1) | Eligibility + `ProcessOutcome` (ARCH §5.4–5.5) | Integration tests: access denied, exit race | M2, M3 |
| No elevation for permitted operations (ACC §1) | ADR-0002, `asInvoker` | Run as standard user in CI and manually | M2, M6 |
| Responsive; cancel where feasible (ACC §1) | Engine thread, cooperative cancel (ARCH §4, §7) | Fake slow platform tests; manual | M3, M4 |
| Before/after, elapsed, follow-up for rebound (ACC §1, §3) | T0/T1/+10 s/+60 s, analysis (ARCH §5.5–5.6) | Unit tests with scripted metric sequences; probe integration test | M1, M3 |
| WS reduction ≠ commit release ≠ performance (ACC §1) | Report wording rules (ARCH §5.6) | Property + snapshot tests on report output | M1 |
| Local settings, no network/login/telemetry (ACC §1) | ARCH §2, §9; `cargo deny` bans | CI dependency check; firewall observation in manual test | M0, M6 |
| Performance targets (ACC §2) | Startup sequence, sampling cadence, lazy PDH (ARCH §4) | `BENCHMARKS.md` | M0 (spike), M5 |
| Effectiveness and regression tests incl. no-op control (ACC §3) | DryRun, cost indicators | Benchmark scenarios S1–S8 | M5 |
| No handle leaks / unbounded growth (ACC §4) | RAII handles | Soak test | M3, M6 |
| Races and invalid IDs do not crash (ACC §4) | `ProcessKey`, `catch_unwind` | Fake + integration tests | M2, M3 |
| Core system processes excluded (ACC §4) | System-critical list (OS-CAP §4) | Unit tests on list; integration attempt on csrss/System | M1, M2 |
| Automatic actions disabled (ACC §4) | No automatic trigger in MVP binary (ARCH §12.2) | Code review | — |
| Predictable tray lifecycle; Quit stops work (ACC §4) | ARCH §8.2, §7 | Manual checklist; process-exit check after Quit | M4 |
| Local, non-sensitive logs (ACC §4) | ARCH §9 | Review of log/history samples | M3 |
| Storage phase safety (ACC §5) | Reserved design (ARCH §12.1) | Future phase | Phase 2 |
| ADR, tech comparison, OS risk analysis, milestones, benchmark method (ACC §6) | `docs/adr/`, `OS-CAPABILITIES.md`, `PLAN.md`, `BENCHMARKS.md` | This PR | — |
| Unit + integration tests; reproducible builds; manual checklist (ACC §6) | ARCH §10–11 | CI; release workflow | M0–M6 |
| Accessible, localization-ready (PROD §7) | ARCH §8.3; toolkit gate | Narrator/NVDA checks | M0, M4, M6 |
