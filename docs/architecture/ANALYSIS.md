# Reclaim — Gap Analysis

**Status:** input to the architecture work (November 2026).
**Scope:** what the repository already defines, what is missing before implementation can start, and which ambiguities in the existing requirements need a decision.

## 1. What exists

| Document | Content | Assessment |
|---|---|---|
| `README.md` | Principles, 4-phase roadmap, status "Planning", initial engineering task | Clear direction. Names Rust + Slint as a candidate only. |
| `docs/PRODUCT.md` | Vision, audience, functional requirements for the memory MVP, future storage/automation, NFRs, out-of-scope list, open questions | Good and honest about trimming side effects. Requirements are mostly testable; several are qualitative ("noticeable", "where feasible"). |
| `docs/ACCEPTANCE.md` | Functional acceptance checklist, preliminary performance targets, effectiveness/regression method, reliability/safety, storage acceptance, delivery expectations (the general DoD) | Strong base for a DoD. Targets are explicitly aspirational; reference hardware and procedures are not yet defined. |
| `docs/REFERENCES.md` | Wise Memory Optimizer, Roomy, Windows API links, Linux/macOS research pointers, evaluation principles | Useful. No verified findings yet (which APIs WMO calls, what the effects are). |

There is no code, build system, CI, license, issue tracker content or UX material.

## 2. What the DoD requires before full implementation

`ACCEPTANCE.md §6` lists the pre-implementation deliverables. Status after this work:

| Required deliverable | Where it is now |
|---|---|
| Architecture decision record | [`docs/adr/`](../adr/) — ADR-0001 (stack), ADR-0002 (process and privilege model), ADR-0003 (API policy) |
| Technology comparison | [ADR-0001](../adr/0001-technology-stack.md) |
| OS capability / risk analysis | [`OS-CAPABILITIES.md`](OS-CAPABILITIES.md) |
| Implementation milestones | [`docs/PLAN.md`](../PLAN.md) |
| Benchmark methodology | [`BENCHMARKS.md`](BENCHMARKS.md) |
| Automated tests for pure logic, integration tests for platform operations | Strategy in [`ARCHITECTURE.md §10`](ARCHITECTURE.md#10-testing-strategy); built in milestones M1–M3 |
| Reproducible release builds, manual test checklist | Approach in [`ARCHITECTURE.md §11`](ARCHITECTURE.md#11-build-packaging-and-release); delivered in M6 |

## 3. Gaps

### 3.1 Blocking decisions (must be made before or during M0)

1. **Project license.** *Decided 2026-11-20: MIT (free distribution).* Originally not selected. It constrains the UI toolkit: Slint is available under GPLv3, a royalty-free license with attribution requirements, or a paid license. It also governs whether Roomy (MIT) code can be adapted later (it can under almost any license, with notice).
2. **UI toolkit.** "Rust + Slint is a candidate". The 15 MiB idle private memory and < 300 ms startup targets cannot be judged on paper; a measured spike is required (see ADR-0001).
3. **Reference hardware.** `ACCEPTANCE.md` asks for it but none is defined. Without it, no performance target is verifiable. `BENCHMARKS.md` proposes two profiles; the owner must confirm which physical machines exist. *Update 2026-11-20: only the owner's machine is available; see BENCHMARKS §1.*
4. **Minimum Windows version.** *Decided 2026-11-20: Windows 10 22H2 and Windows 11 23H2+.* "Windows 10/11 x64" is broad. Windows 10 22H2 is the last Windows 10 release (mainstream support ended October 2025). Some useful APIs (for example `PROCESS_MEMORY_COUNTERS_EX2`) only exist on newer builds. Proposal: Windows 10 22H2 and Windows 11 23H2+, with graceful degradation where an API is missing.

### 3.2 Missing definitions

- **Metric glossary.** The PRODUCT document correctly demands distinguishing working set, private bytes/commit, standby/cache and available RAM, but no single definition table exists that UI, reports and tests share. Defined in `ARCHITECTURE.md §5`.
- **"Usable UI"** for the startup metric. Defined in `BENCHMARKS.md` as "window shown with system metrics rendered and accepting input"; the process list may still be loading.
- **Idle state.** Not specified whether idle means window visible, minimized or tray-only. `BENCHMARKS.md` measures all three; targets apply to the tray-only and visible-idle states separately.
- **"Typical manual optimization".** The process count and workload are undefined. Proposed: 150–300 processes, of which typically 20–60 are eligible, on reference hardware.
- **Default protected processes.** Listed as an open question. A concrete proposal is in `OS-CAPABILITIES.md §4`.
- **Memory pressure "where supported".** Proposed sources and their limits are in `OS-CAPABILITIES.md §2`.
- **What "cancel" means** for a trim run. Defined in `ARCHITECTURE.md §7`.
- **Close vs Quit behavior** of the main window and tray. Defined in `ARCHITECTURE.md §8`; the default is an owner decision.

### 3.3 Missing engineering infrastructure

- Repository layout, Cargo workspace, toolchain pinning, lint/format policy.
- CI (Windows runner), dependency license audit, release workflow.
- Test helpers: probe processes with controllable memory behavior, fake platform for unit tests.
- Benchmark harness that produces comparable, archived results.
- Privacy statement for local logs and history (what is stored, where, how to clear).
- Contribution conventions (commit style, ADR process). Lightweight: ADRs in `docs/adr/`, one PR per milestone task.

### 3.4 Ambiguities and tensions in the requirements

| Topic | Tension | Proposed resolution |
|---|---|---|
| Tray status vs near-zero idle CPU | A live tray tooltip implies periodic polling | No polling while hidden; refresh tooltip on hover; rely on OS low-memory notification for events |
| "Optimize" as primary action vs honesty | A big button invites "boost" expectations | Action is labelled "Trim working sets"; result screen leads with measured facts and rebound, not a score |
| Cancellation "where feasible" | A single `EmptyWorkingSet` call cannot be interrupted | Cancellation is cooperative between processes; documented as such |
| Cross-platform core vs Windows-first | Abstractions modelled on Windows APIs will not fit Linux/macOS | Capability-based ports (`ARCHITECTURE.md §6`), optional metric fields |
| Elevation only "for a specific justified capability" | Users will want to trim elevated processes too | MVP never elevates; elevated processes are reported as "requires administrator". A narrow broker is designed but deferred (ADR-0002) |
| Comparison with Wise Memory Optimizer | Proprietary behaviour; it likely purges standby lists, which Reclaim excludes | Comparison is optional and observational; Reclaim does not chase its numbers |
| Small binary vs localization/accessibility | Toolkits with good accessibility are bigger | Originally: accessibility hard, size yields. 2026-11-20: owner removed screen-reader accessibility from the requirements |

## 4. Risks identified at analysis time

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Working-set trimming gives little or negative real benefit on modern Windows | Core value proposition weak | Measure honestly from M0; product messaging already avoids claims; storage phase adds value independent of trimming |
| R2 | Chosen UI toolkit misses idle-memory target | NFR failure, late rewrite | UI behind a view-model boundary; toolkit spike gates the choice in M0 |
| R3 | Most "big" processes are not trimmable without elevation | Users perceive the tool as weak | Clear per-process reasons; design (not build) a narrow elevated broker |
| R4 | Toolkit license incompatible with the chosen project license | Legal / redistribution issue | Decide license in M0; `cargo deny` license check in CI |
| R5 | Trimming foreground/compositor/audio processes causes visible stutter | Trust damage | Protected defaults, foreground exclusion, latency probe in benchmarks |
| R6 | PID reuse or exit races lead to acting on the wrong process | Safety bug | Identity = PID + creation time; act only through a handle held since validation |
| R7 | Scope creep from storage/automation into MVP | Delay | Phase boundaries in PLAN; storage/automation interfaces reserved but not implemented |
