# Reclaim — Benchmark and Effectiveness Methodology

**Status:** proposed procedure for verifying `ACCEPTANCE.md §2–3`. Results are only claims once produced by this procedure on declared reference hardware and archived.

## 1. Reference hardware

Only one physical machine is available (owner decision, 2026-11-20). Profiles are therefore built on it:

| Profile | Purpose | Setup |
|---|---|---|
| **RH-OWNER** | The reference machine for all footprint targets (B1–B8) | The owner's machine as is; specs in [`docs/benchmarks/reference-hardware.md`](../benchmarks/reference-hardware.md): laptop, i7-11800H, 16 GiB, NVMe PCIe 4.0, Windows 11 Home 24H2 |
| **VM-LOW** | Memory-constrained case, where trimming matters most | VirtualBox VM on RH-OWNER: 4 vCPU, 8 GiB (and 4 GiB) RAM, Windows 11 23H2+. Used for effectiveness scenarios S5 and pressure tests |
| **VM-CLEAN** | Clean-install compatibility, no runtimes | VirtualBox VMs on RH-OWNER: Windows 10 22H2 and Windows 11, no VC++ redistributable, no .NET extras |

Limitations, stated in every summary: VM results are not equivalent to a real low-end laptop (virtualized disk, no battery), and one machine cannot show hardware variance. RH-OWNER is a laptop, so scenario S8 (battery) is included. The host runs Windows 11 Home, which has no Hyper-V, so VMs use VirtualBox.

Each result file records: CPU, RAM, disk, Windows build, power plan, AC/battery, Reclaim commit hash, toolchain version, background apps.

## 2. Measurement rules

- Release build from CI, never a debug build.
- Close other measurement tools while measuring footprint; the harness samples via `GetProcessMemoryInfo` / `GetProcessTimes` from a separate process.
- Report median and p95 (startup), mean and max (idle), with N stated.
- **Private bytes** (`PrivateUsage`) is the footprint metric. Working set is reported alongside but is not the target, and the app must not trim itself to look smaller.
- Any resident helper process (there should be none) counts toward footprint.

## 3. Footprint and responsiveness procedures

| ID | Metric | Procedure |
|---|---|---|
| B1 | Cold start to usable UI | Reboot, wait 2 min for boot activity to settle, launch via harness. Usable UI = the instrumentation marker emitted after the first frame containing system metrics, when the window accepts input. Repeat with reboots, N = 10 |
| B1w | Warm start | Launch, quit, relaunch; N = 30 |
| B2 | Idle — Overview visible | Launch, open Overview, leave foreground, wait 60 s, sample private bytes and CPU time every 1 s for 5 min |
| B3 | Idle — tray only | As B2, window closed to tray |
| B4 | Active monitoring — Processes screen | As B2 on the Processes screen (reported, not an idle target) |
| B5 | Manual trim duration | Workload W-STD (§4), run "Trim working sets" with defaults; elapsed time from Start to RunCompleted, N = 10 |
| B6 | Binary and package size | Size of `reclaim.exe` and of the zip, reported separately |
| B7 | UI responsiveness during run | During B5, measure UI input-to-paint latency of a scripted keypress (harness posts input, app emits marker); must stay < 100 ms |
| B8 | Soak | 1 000 cycles of plan → trim → finalize (follow-up shortened to 1 s via test flag). Handle, GDI and USER object counts and private bytes at cycles 10, 100, 1 000 must not trend upward |

Targets for B1–B6 are those of `ACCEPTANCE.md §2` and the ADR-0001 gates.

## 4. Workloads

| ID | Description |
|---|---|
| W-IDLE | Fresh login desktop, no user apps |
| W-STD | Browser with 20 tabs (mixed sites, saved offline copies for repeatability), an editor/IDE, a chat client, a file manager window: ~150–300 processes |
| W-HOT | W-STD plus a recently memory-intensive app (probe `alloc-touch` allocated 2 GiB, touched, then idle) |
| W-FG | W-STD with an interactive foreground probe (§5) |
| W-PRESSURE | VM-LOW with probes allocating until Available < 10 % (high pressure) |
| W-PROT | Includes elevated process (probe started elevated), another-user process, and protected system processes |
| W-RACE | Probes that exit at random intervals (10–500 ms) during planning and trimming |

## 5. Probe processes (`reclaim-bench probe ...`)

- `alloc-touch --size N --pattern seq|random --dirty`: allocates and touches memory; option to keep pages dirty (forces modified-list writes when trimmed).
- `latency --size N --interval 100ms`: touches its whole working set every interval and records the time per pass to a CSV. After a trim, the first passes show soft/hard-fault costs; this is the **responsiveness proxy** for a foreground or background app.
- `exit-after --ms N`: exits after a delay (race tests).

## 6. Effectiveness scenarios

For each scenario, run three arms in randomized order with a 2-minute settle between them:

1. **Control:** Reclaim `DryRun` (same pipeline, no OS trim call).
2. **Reclaim trim** with default policy.
3. **Optional comparison:** Wise Memory Optimizer default optimize, same workload (manual; record version; observational only).

Captured per arm (matches ACCEPTANCE §3):

1. T0: available physical, commit, standby/modified (if available), per-process WS/private bytes.
2. Actions attempted, skipped with reason, failures, elapsed time.
3. T1, T+10 s, T+60 s samples (plus T+5 min in VM-LOW scenarios).
4. Rebound per process and in total; page-fault deltas; system `Pages Input/sec` time series; disk bytes; `latency` probe pass times.
5. Arm-to-arm comparison table.

| Scenario | Workload | Question answered |
|---|---|---|
| S1 | W-IDLE | Does trimming do anything useful on an idle system? (expected: no) |
| S2 | W-STD | Typical result and rebound on a normal session |
| S3 | W-HOT | Does trimming recover memory from a previously heavy app with low rebound? |
| S4 | W-FG | What does the foreground app pay if not excluded (override) vs excluded (default)? |
| S5 | W-PRESSURE | Under high pressure, does trimming help the foreground probe's latency or hurt it? |
| S6 | W-PROT | Are protected/elevated/other-user processes skipped and reported correctly? |
| S7 | W-RACE | No crash, no wrong-process action, outcomes `Exited` reported |
| S8 | RH-OWNER on battery | Cost of modified-page writes (disk activity) |

**Pass condition** (from ACCEPTANCE §3): correct and safe behaviour plus honest reporting. A lower working set alone is not a pass. Any arm where the `latency` probe's p95 pass time after trimming exceeds the control by > 20 % is recorded as a **regression**, regardless of available memory.

## 7. Output and archiving

- Harness writes `bench-results/<date>-<profile>-<commit>/` with raw CSV/JSON and a generated `summary.md`.
- Summaries for releases are committed under `docs/benchmarks/`; raw data can be attached to the release.
- A claim in README or UI copy must cite a summary file.
