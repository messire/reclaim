# Reclaim — Desktop Application Architecture

**Status:** proposed, revision 2 (after the critical review in [`REVIEW.md`](REVIEW.md)).
**Scope:** Windows memory MVP, with explicit extension points for storage, automation and Linux/macOS.
**Related:** [Gap analysis](ANALYSIS.md) · [OS capabilities and risks](OS-CAPABILITIES.md) · [Benchmarks](BENCHMARKS.md) · [ADRs](../adr/) · [Plan](../PLAN.md)

## 1. Goals and architectural drivers

Ordered by priority when they conflict:

1. **Safety and honesty.** Never act on the wrong process, never misreport an outcome, never change system configuration as part of trimming.
2. **Small, quiet footprint.** Near-zero idle CPU, < 15 MiB idle private memory (preferred), < 300 ms to usable UI. The app must not become a resource hog.
3. **Least privilege.** Fully functional for the current user's processes without elevation.
4. **Testability.** Every decision rule is pure and unit-tested; every OS call is behind a port with a fake.
5. **Portability without lowest-common-denominator.** Platform differences are explicit capabilities, not hidden behind a fake-uniform API.
6. **Replaceable UI.** The toolkit decision is gated by measurement (ADR-0001), so the UI must be swappable without touching core logic.

## 2. System context

```
            ┌───────────────────────── user session ─────────────────────────┐
  user ───▶ │  reclaim.exe (single process, unelevated, asInvoker manifest)  │
            │                                                                 │
            │   UI thread ◀──events── Engine thread ──▶ Win32 / PSAPI / PDH   │
            │       │                     │                                   │
            │       └──commands──────────▶│──▶ %LOCALAPPDATA%\Reclaim\        │
            │                                  settings.toml, history.jsonl   │
            └─────────────────────────────────────────────────────────────────┘
  No network. No service. No resident helper. No driver. No telemetry.
```

- One executable, one process, started on demand (optional user-level autostart later).
- No Windows service, scheduled task, helper process or kernel component in the MVP.
- No network access at all: no update check, no crash upload. Updates are manual.

## 3. Logical architecture

Hexagonal ("ports and adapters") layout. Dependencies point inwards only.

```
┌──────────────────────────────────────────────────────────────────────┐
│ reclaim-app (binary)                                                 │
│   UI adapter (toolkit-specific) · tray · single instance · i18n      │
│   ViewModels (toolkit-agnostic, unit-testable)                       │
└───────────────▲──────────────────────────────────────────────────────┘
                │ Command / Event protocol (plain Rust types)
┌───────────────┴──────────────────────────────────────────────────────┐
│ reclaim-engine                                                       │
│   EngineLoop (worker thread) · Sampler · RunCoordinator (state       │
│   machine) · FollowUpScheduler · SettingsStore · HistoryStore        │
└───────────────▲──────────────────────────────────────────────────────┘
                │ uses ports (traits)
┌───────────────┴──────────────────────────────────────────────────────┐
│ reclaim-core (no OS dependencies, #![forbid(unsafe_code)])           │
│   units · metrics model · process identity · capabilities            │
│   policy (exclusions, eligibility) · run plan · outcome model        │
│   analysis (deltas, rebound, cost indicators) · report wording       │
│   ports: SystemMetrics, ProcessSource, Reclaimer, SessionInfo, Clock │
└───────────────▲──────────────────────────────────────▲───────────────┘
                │ implements                           │ implements
┌───────────────┴──────────────┐        ┌──────────────┴───────────────┐
│ reclaim-platform-windows     │        │ reclaim-platform-fake        │
│ windows-rs, all `unsafe` here│        │ scripted, deterministic      │
└──────────────────────────────┘        └──────────────────────────────┘
```

Additional workspace members:

- `reclaim-cli` — headless front end over the same engine (`status`, `list`, `trim --dry-run`, `trim`). Used by integration tests and the benchmark harness, and useful to power users. Shares 100% of logic with the GUI.
- `reclaim-bench` — benchmark harness (startup, idle, effectiveness scenarios) and probe workloads. Never shipped in the release package.
- `xtask` — build, package and release automation.

### 3.1 Crate responsibilities and rules

| Crate | Owns | Must not |
|---|---|---|
| `reclaim-core` | Domain types, all decision logic, the report text model | Call the OS, spawn threads, do I/O, use `unsafe` |
| `reclaim-platform-windows` | FFI, handle lifetimes (RAII), error mapping to core error kinds | Contain policy ("should this be trimmed?") |
| `reclaim-engine` | Threading, scheduling, orchestration, persistence | Contain toolkit types or OS FFI |
| `reclaim-app` | Presentation, tray, OS shell integration | Decide eligibility or compute results |

`unsafe` is confined to `reclaim-platform-windows` (and the tray/shell code in the app if the toolkit requires it), each block with a `// SAFETY:` comment, and reviewed as a unit.

## 4. Runtime and threading model

- **UI thread** — owned by the toolkit. Never performs OS enumeration or trimming. Receives `Event`s through the toolkit's thread-safe "invoke on UI thread" mechanism.
- **Engine thread** — one long-lived worker. Owns all OS handles and the run state machine. Blocks in `WaitForMultipleObjects` (Windows adapter) on: command queue event, one OS memory-resource notification (low or high, see §6.1), and a single waitable timer. No busy loops, no async runtime.
- **No thread pool.** Trimming is sequential per process: parallel trimming would only increase paging I/O bursts and makes measurements harder to attribute.

Commands (UI → engine), selected:

```
Subscribe{ view: Visible | Hidden }      // controls sampling cadence
RefreshNow
PlanRun{ selection: Selection }          // returns a Plan for confirmation
StartRun{ plan_id, mode: Trim | DryRun }
CancelRun
UpdateExclusions{ .. } · UpdateSettings{ .. }
ClearHistory
Shutdown
```

Events (engine → UI): `SystemSample`, `ProcessTable`, `PlanReady`, `RunProgress`, `RunCompleted`, `FollowUpSample{+10s|+60s}`, `RunFinalized`, `Warning`, `Capabilities`.

### 4.1 Sampling cadence (idle behaviour)

| UI state | System metrics | Process table | Notes |
|---|---|---|---|
| Overview visible, foreground | 1 s | none | System sample costs microseconds |
| Processes screen visible, foreground | 1 s | 3 s | "Active monitoring"; rows diffed, only changed rows pushed to the UI |
| Window visible, not foreground | 5 s | paused | |
| Minimized / tray only | **none** | **none** | Tray tooltip refreshed on hover only |
| Run in progress / follow-up | as required by the run | targeted (run members only) | Independent of view state |

The process table is the only expensive sample (one `OpenProcess` per process), so it runs only when the user is looking at it. Benchmarks treat "Overview visible" and "tray only" as idle, and report the Processes screen separately (review F20).

Timers are coalesced (`SetWaitableTimerEx` with tolerable delay) so the app does not cause extra wake-ups. When hidden, the engine thread has no timer armed at all; the only wake sources are commands and the OS low-memory notification.

### 4.2 Startup sequence (target < 300 ms to usable UI)

1. Single-instance check (named mutex scoped to the session). A second launch signals the first instance to show its window and exits.
2. Load settings (small TOML file; corrupt or missing → defaults, with a non-blocking warning).
3. Create the window and render with system metrics from one `GlobalMemoryStatusEx`/`GetPerformanceInfo` call (microseconds).
4. Mark "usable UI" (instrumentation point for benchmarks).
5. Start the engine thread; process enumeration and PDH counters initialize asynchronously; the process list shows a loading state.

`--tray` (used by optional autostart) skips window creation entirely.

## 5. Domain model

### 5.1 Metric glossary (shared by UI, reports and tests)

All values are stored as `u64` bytes (`Bytes` newtype). Display uses IEC units (MiB, GiB). Every metric has a stable id, a short label and a one-sentence explanation shown in the UI.

| Id | Label | Definition | Windows source |
|---|---|---|---|
| `sys.phys_total` | Installed memory | Physical RAM visible to the OS | `GlobalMemoryStatusEx.ullTotalPhys` |
| `sys.phys_available` | Available | Free + zeroed + standby pages; immediately reusable | `ullAvailPhys` |
| `sys.commit_charge` | Committed | Virtual memory promised to processes and the system; trimming does **not** reduce it | `GetPerformanceInfo.CommitTotal` |
| `sys.commit_limit` | Commit limit | RAM + page file(s) | `CommitLimit` |
| `sys.standby` | Standby (cached) | Cached pages that can be repurposed; part of Available | PDH `\Memory\Standby Cache *` (optional) |
| `sys.modified` | Modified | Dirty pages waiting to be written to disk; trimming can increase it | PDH `\Memory\Modified Page List Bytes` (optional) |
| `sys.pressure` | Memory pressure | Low / Normal / High signal | `QueryMemoryResourceNotification` (+ derived from available/commit ratio) |
| `proc.working_set` | Working set | Physical pages currently mapped for the process, shared pages included | `PROCESS_MEMORY_COUNTERS.WorkingSetSize` |
| `proc.private_ws` | Private working set | Part of the working set not shared | `PROCESS_MEMORY_COUNTERS_EX2.PrivateWorkingSetSize` (newer builds; optional) |
| `proc.private_bytes` | Private bytes (commit) | Private committed memory; trimming does **not** reduce it | `PrivateUsage` |
| `proc.page_faults` | Page faults | Cumulative soft + hard faults | `PageFaultCount` |
| `proc.io` | I/O | Cumulative read/write bytes and operations | `GetProcessIoCounters` (optional) |
| `sys.hard_faults_rate` | Pages read from disk /s | System-wide paging input, used as a cost indicator | PDH `\Memory\Pages Input/sec` (optional) |

Optional metrics are `Option<T>` in the model. The UI renders an unavailable metric as "not available" with the reason, never as zero.

### 5.2 Process identity

`ProcessKey = (pid: u32, start_time: u64 /* FILETIME */)`. A PID alone is never used to act. The Windows adapter validates the key after `OpenProcess` (via `GetProcessTimes`) and the engine keeps the handle open from validation until the last follow-up sample, so the PID cannot be reused while Reclaim refers to it.

`ProcessInfo` carries: key, image name (no full path, no command line), session id, same-user flag, elevation/integrity hint, `critical` flag, foreground flag, and a `MemorySnapshot`.

### 5.3 Capabilities

```rust
enum Availability { Available, RequiresElevation, Unsupported { reason: &'static str } }

struct Capabilities {
    trim_working_set: Availability,
    private_working_set: Availability,
    standby_breakdown: Availability,
    paging_io_rate: Availability,
    pressure_signal: Availability,
    per_process_io: Availability,
}
```

Discovered at runtime once per session and re-checked on failure. The UI and the report wording read capabilities rather than `cfg(windows)`.

### 5.4 Policy and eligibility

Eligibility is a pure function: `fn evaluate(p: &ProcessInfo, policy: &Policy, ctx: &Context) -> Eligibility`.

Evaluation order (first match wins; every result carries a reason shown to the user):

1. `Self_` — the Reclaim process itself.
2. `SystemCritical` — PID 0/4, processes reporting `IsProcessCritical`, and the built-in protected list (`OS-CAPABILITIES.md §4`). Not overridable from the UI.
3. `UserExcluded(rule)` — user rule match (image name, optionally with path hash; wildcards not supported in the MVP).
4. `Protected(rule)` — default "caution" list (compositor, audio, shell, input). Overridable by the user with an explicit warning.
5. `Foreground` — owns the foreground window. Overridable per run.
6. `OtherUser` / `RequiresElevation` — predicted from session/integrity; also confirmed at open time.
7. `BelowThreshold` — working set under `policy.min_working_set` (default 32 MiB): trimming is not worth the fault cost.
8. `Eligible`.

The default selection for a run is "all eligible". The plan is shown to the user before execution (counts by reason, list of targets); confirmation is one click and can be remembered.

### 5.5 Run outcome model

```rust
enum ProcessOutcome {
    Trimmed { before: MemorySnapshot, after: MemorySnapshot },
    Skipped(SkipReason),                 // decided before acting
    Failed(FailureKind),                 // AccessDenied | Exited | Unsupported | Os(code)
    Cancelled,                           // not attempted because the run was cancelled
}
```

A run record holds: plan, mode (`Trim` or `DryRun`), start/end timestamps, elapsed time, per-process outcomes, system snapshots at T0 (before), T1 (immediately after), T+10 s and T+60 s, and cost indicators (page-fault deltas, I/O deltas, system paging-input rate during the follow-up window). `Failed` and `Skipped` are never counted as success in any total.

### 5.6 Analysis and report wording

`reclaim-core::analysis` computes, for trimmed processes: working-set delta at T1, rebound at T+10 s and T+60 s (absolute and percent of the trimmed amount), private-bytes delta (expected ≈ 0), page-fault delta, and system-level available/commit deltas.

`reclaim-core::report` turns analysis into a structured list of statements, each with an evidence reference. Rules:

- Always say "working set reduced", never "memory freed" or "RAM cleaned".
- Always show committed memory next to available memory, and state that trimming does not release committed memory.
- If rebound at T+60 s > 50 % of the trimmed amount, say so prominently.
- If page faults or paging input increased materially during follow-up, report it as a cost.
- Always include "Responsiveness: not measured by Reclaim"; silence must not read as "no regression" (review F21).
- Never state that performance improved. The strongest positive statement allowed is a measured increase in available memory under the observed pressure level.
- In `DryRun` mode, the report is the control measurement ("what changed without trimming").

Statement templates are localizable keys with typed parameters, so wording rules are tested in `core`, not in the UI.

## 6. Platform ports

```rust
trait SystemMetrics   { fn sample(&mut self) -> Result<SystemSnapshot, PlatformError>; }
trait ProcessSource   { fn enumerate(&mut self) -> Result<Vec<ProcessInfo>, PlatformError>;
                        fn open(&mut self, key: ProcessKey) -> Result<Box<dyn ProcessHandle>, OpenError>; }
trait ProcessHandle   { fn key(&self) -> ProcessKey;
                        fn memory(&self) -> Result<MemorySnapshot, PlatformError>; }
trait Reclaimer       { fn capabilities(&self) -> Capabilities;
                        fn reclaim(&mut self, h: &dyn ProcessHandle, action: ReclaimAction)
                            -> Result<(), ReclaimError>; }
trait SessionInfo     { fn foreground_process(&self) -> Option<ProcessKey>;
                        fn pressure_events(&self) -> Option<PressureSource>; }
trait Clock           { fn now(&self) -> Instant; }
```

`ReclaimAction` is an enum describing intent, not an API: `TrimWorkingSet` on Windows; future `PageOutCold` (Linux `process_madvise`) can be added without changing the engine. An adapter reports unsupported actions through `Capabilities`, never by silently doing nothing.

### 6.1 Windows adapter (summary; details in `OS-CAPABILITIES.md`)

- Enumeration: `CreateToolhelp32Snapshot` (names, PIDs) then `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetProcessMemoryInfo` per process. Processes that cannot be opened appear with metrics "not accessible".
- Trimming: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | SYNCHRONIZE)` (`SYNCHRONIZE` is required for the zero-timeout exit check in §7) → validate start time → `SetProcessWorkingSetSize(h, usize::MAX, usize::MAX)` (equivalent to `EmptyWorkingSet`; both documented). No hard working-set limits are ever set.
- Pressure: two handles, `LowMemoryResourceNotification` and `HighMemoryResourceNotification`. They are **level-triggered** (they stay signaled while the condition holds), so the engine never waits on a handle that is already signaled:
  - state *Normal*: wait on the low-memory handle only;
  - when it signals: publish `Pressure::Low`, remove the low handle from the wait set, wait on the high-memory handle instead, and arm one coalesced re-check timer (30 s) that calls `QueryMemoryResourceNotification` to detect a return to the middle band, where neither handle is signaled;
  - when the high handle signals or the re-check finds low no longer signaled: publish the new state and go back to waiting on the low handle.
  The engine therefore wakes at most once per state change, plus once every 30 s while memory stays low.
- PDH: opened lazily only while the window is visible or a run follow-up is active; closed when hidden.
- All handles are wrapped in an RAII type; no raw `HANDLE` escapes the crate.

## 7. Run state machine

```
Idle ──PlanRun──▶ Planned ──StartRun──▶ Running ──done──▶ FollowUp ──+60s──▶ Finalized ──▶ Idle
  ▲                 │                     │                   │
  └───── discard ───┘          CancelRun  ▼        CancelRun  ▼
                              Cancelling ──▶ FollowUp     Finalized (partial follow-up recorded)
```

- **Running:** sequential loop over targets, largest working set first. Before each target, check the cancel flag; per-process time is bounded by the OS call itself.
- **Cancel semantics:** a call in progress completes; remaining targets become `Cancelled`; follow-up sampling still runs for already-trimmed processes (so the user sees the rebound of what was done) unless the user cancels follow-up too.
- **Concurrency:** only one run at a time. During FollowUp, a new run can be planned; starting it finalizes the current run early and marks the follow-up as truncated.
- **Process exit during run:** detected by `WaitForSingleObject(h, 0)` on the `SYNCHRONIZE` handle (or `GetExitCodeProcess` as a fallback) / failed calls → `Failed(Exited)`; never an error dialog.
- **Late samples:** every follow-up sample stores its actual timestamp; a sample more than 2 s later than its target (sleep/resume, heavy load) is flagged in the report instead of being presented as "+60 s" (review F24).
- **Held handles:** handles of trimmed processes are kept until Finalized so identities stay valid; a process that exits meanwhile keeps only a small kernel object alive, released at Finalized or Quit (review F25).
- **Internal errors:** each command is executed inside `catch_unwind`; a panic becomes an internal-error outcome in the run record and the engine stays alive (review F22).
- **Quit during run:** cancel, wait at most 2 s for the current call, persist the partial record, release handles, exit.

## 8. User interface

### 8.1 Screens

1. **Overview** (default): system panel (Available, Committed / limit, Standby, Pressure) with one-line explanations; primary action "Trim working sets…"; last run summary.
2. **Processes:** sortable table (name, working set, private WS, private bytes, eligibility reason). Per-row exclude/include. Search by name.
3. **Plan confirmation:** counts by reason, target list, warning text about faults and rebound, buttons "Trim", "Dry run", "Cancel".
4. **Result:** statements from `report` (§5.6), table of per-process outcomes with filters (trimmed/skipped/failed), live follow-up at +10 s/+60 s.
5. **History:** last N runs, clear history.
6. **Settings:** close behaviour, exclusions, threshold, logging level, about/licenses.

No animations except progress indication during a run. No charts in the MVP (avoids continuous redraw and keeps the footprint small); a small sparkline can be considered once idle targets are met.

### 8.2 Tray and lifecycle

- Tray icon via `Shell_NotifyIconW` (`NOTIFYICON_VERSION_4`), menu: Open, Trim working sets…, Quit.
- Tooltip text is computed on hover (`NIN_POPUPOPEN`), not by polling.
- The `TaskbarCreated` registered window message is handled to re-add the icon after Explorer restarts (review F23).
- **Close (X)** behaviour (owner decision, 2026-11-20): the window goes to the tray. The first time, a one-time tray notification says that Reclaim keeps running and how to quit. A setting allows "Close quits" for users who prefer it.
- Hiding to tray destroys the window and frees renderer resources where the toolkit allows it (verified in the M0 spike); the engine stays, idle.
- **Quit** always stops all work, releases handles, removes the tray icon and exits the process. No background work survives Quit.

### 8.3 Accessibility and localization

- Accessibility: keyboard-only operation, visible focus, screen-reader names for all controls (UI Automation), high-contrast mode, Per-Monitor V2 DPI awareness. Toolkit acceptance depends on this (ADR-0001).
- Localization: all user-visible strings in resource files from day one (English only in MVP). Numbers and units formatted via a single formatter. Report statements are keys + parameters.

## 9. Persistence, logging and privacy

| Data | Location | Format | Limits |
|---|---|---|---|
| Settings and exclusions | `%LOCALAPPDATA%\Reclaim\settings.toml` | TOML with `schema_version` | Atomic write (temp file + `ReplaceFileW`/rename) |
| Run history | `%LOCALAPPDATA%\Reclaim\history.jsonl` | One JSON object per run | Last 100 runs or 1 MiB, oldest dropped |
| Log | `%LOCALAPPDATA%\Reclaim\reclaim.log` | Text | Off by default (errors only); 2 × 512 KiB rotation |

- Stored: image names, PIDs, numbers, error codes. Not stored: full paths, command lines, window titles, user names.
- Uninstall/portable removal = delete the folder.
- No data leaves the machine. There is no network code in the binary (verified by a dependency audit and a CI check that no networking crate is linked).
- Release builds keep `panic = "unwind"` so the engine can contain a failing command (§7); a panic hook writes a short local note to the log. Nothing is uploaded.
- "Keep run history" setting (default on) and "Clear history and logs" button (review F26).

## 10. Testing strategy

| Level | What | Where |
|---|---|---|
| Unit | Units, eligibility order, policy edge cases, analysis math, rebound, report wording rules, state machine transitions, settings migration | `reclaim-core`, `reclaim-engine` with `reclaim-platform-fake`; run on any OS in CI |
| Property | Outcome totals never count skipped/failed as success; report never emits forbidden phrases | `proptest` in `core` |
| Integration (Windows) | Enumerate, open, validate identity, trim a probe child process and observe WS drop; access denied on protected/elevated process; exit race (probe exits between plan and trim); PID-reuse defence | `reclaim-platform-windows/tests`, Windows CI runner |
| Soak | 1 000 plan/trim/follow-up cycles: handle count, private bytes and GDI/USER object counts stable | `reclaim-bench soak`, nightly or manual |
| UI | ViewModel tests without the toolkit; smoke test that the binary starts and exits with `--self-test` | `reclaim-app` |
| Manual | Checklist for UX, accessibility, tray lifecycle and high-pressure scenarios | `docs/testing/MANUAL_CHECKLIST.md` (M6) |
| Performance / effectiveness | `BENCHMARKS.md` procedures | `reclaim-bench` on reference hardware |

The fake platform can script: access denied, process exit at a given step, start-time mismatch (PID reuse), slow calls (for cancellation tests) and metric sequences (for rebound analysis).

## 11. Build, packaging and release

- Pinned toolchain (`rust-toolchain.toml`), `Cargo.lock` committed, builds with `--locked`.
- Release profile: `opt-level = "s"` or `3` (decided by benchmark), `lto = "fat"`, `codegen-units = 1`, `strip = true`, static CRT (`-C target-feature=+crt-static`) so no VC++ redistributable is required.
- Application manifest: `asInvoker`, Per-Monitor V2 DPI, supported OS GUIDs, common controls v6.
- Deliverables: portable `reclaim.exe` in a zip with `LICENSE`, third-party notices and `SHA256SUMS`. An installer (MSI or MSIX) is a post-MVP decision.
- CI (GitHub Actions, `windows-latest` + `ubuntu-latest` for platform-independent crates): fmt, clippy (deny warnings), tests, `cargo deny` (licenses, advisories, banned crates incl. networking), binary size report.
- Reproducibility: release built from a tag in CI; build-twice comparison of hashes is attempted (MSVC linker output may need `/Brepro`); deviations are documented rather than hidden.
- Code signing: owner decision (cost); unsigned builds will trigger SmartScreen warnings.

## 12. Extension points for later phases

### 12.1 Storage (phase 2)

Reserved crate `reclaim-storage` with its own ports: `FsWalker` (handle-based, never follows reparse points: opens with `FILE_FLAG_OPEN_REPARSE_POINT`, compares volume and file IDs), `CategoryRules` (data-driven, per platform), `Deleter` (deletes by handle after re-validating the path is under an allowed root; no recursive retries). Scans are read-only; deletion requires a reviewed plan, mirroring the memory `Plan → Confirm → Run → Report` flow. The engine's run coordinator is generic over "operation kinds" to reuse the state machine.

### 12.2 Automation (phase 3)

A `Trigger` abstraction (`Manual` only in MVP) and a policy evaluator in `core` (pressure signal, cooldown, battery/AC, user activity, foreground). The MVP binary contains no automatic trigger code path that can be enabled from settings, satisfying "automatic actions disabled until a later release". History records are already auditable for later automation.

### 12.3 Linux and macOS (phase 4)

New adapter crates implement the same ports. Expected capability differences (`OS-CAPABILITIES.md §6`): Linux `process_madvise(MADV_PAGEOUT/COLD)` needs `CAP_SYS_NICE`/ptrace access and kernel ≥ 5.10; macOS offers no public API to trim unrelated processes, so `trim_working_set = Unsupported` and the product focuses on monitoring and storage there. The UI toolkit choice (ADR-0001) records whether the UI layer itself is portable.

## 13. Decisions and open questions

Decisions are captured as ADRs: [ADR-0001 Technology stack](../adr/0001-technology-stack.md), [ADR-0002 Process and privilege model](../adr/0002-process-and-privilege-model.md), [ADR-0003 Documented APIs and experimental adapters](../adr/0003-api-policy.md). Open questions for the owner are listed in [`PLAN.md §5`](../PLAN.md#5-open-questions-for-the-owner).

## Revision history

- **r1** — initial draft (not published separately).
- **r2** — revised after [`REVIEW.md`](REVIEW.md) pass 1 (F1–F19): removed idle polling, standby purge and whole-app elevation; added process identity rules, foreground/protected defaults, capability model, dry-run control, cost indicators, cancel semantics, privacy limits, toolkit gating. Pass 2 (F20–F26): process table sampled only on its screen, responsiveness disclaimer, contained panics, Explorer-restart handling, late-sample flags, history opt-out.
