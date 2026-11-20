# Running the benchmarks

How to produce the numbers that `BENCHMARKS.md` defines. Use release builds from CI and never debug builds.

The harness comes from two CI artifacts on a green run of `main`:
- `reclaim-windows-build`: `reclaim.exe`, `reclaim-cli.exe` and `ui-bench.exe`.
- `reclaim-bench-exe`: `reclaim-probe.exe`, `reclaim-scenario.exe` and `reclaim-soak.exe`.

Put all six files in one folder. The harness finds `reclaim-probe.exe` next to itself.

## Footprint and start-up (B1w, B2, B3)

```powershell
.\ui-bench.exe --exe .\reclaim.exe --label "Reclaim" --runs 30 --idle-secs 300 --out ui-reclaim.md
```

- Do not count the first launch after a download: Windows scans a new unsigned binary, which distorts it (spike finding).
- B1 (cold start) needs a reboot before each run.

## Soak (B8)

```powershell
.\reclaim-soak.exe --cycles 1000 --out soak.md
```

- This takes about 20 minutes. It trims only its own three probe processes.
- The follow-up runs 60× faster.
- Exit code 0 means no upward trend in handles, GDI objects, USER objects or private bytes.

## Effectiveness scenarios (S1–S8)

```powershell
.\reclaim-scenario.exe --scenario S2 --label RH-OWNER
```

- **What a run does:**
  - It runs a control arm (dry run) and a trim arm in random order, with 2 minutes of settle time between them.
  - The follow-up is in real time (+10 s and +60 s).
  - A latency probe of 256 MiB runs throughout as the responsiveness proxy.
- **What it trims:**
  - Without `--probes-only`, the trim arm trims every eligible process under your saved settings, exactly like the app.
  - Close anything you would not want trimmed.
- **Output:**
  - Results go to `bench-results\<scenario>-<label>-<time>\`, with one JSON record per arm and `summary.md`.
  - Commit summaries worth keeping as `docs/benchmarks/<date>-summary.md`.

| Scenario | Prepare the workload first | Extra options |
|---|---|---|
| S1 | Freshly logged-in desktop, no apps | |
| S2 | Browser with ~20 tabs, editor, chat, file manager | |
| S3 | As S2; the harness adds a 2 GiB probe that touched its memory and is idle | `--size 2048` |
| S4 | As S2; put the latency probe's console in the foreground to test the foreground exclusion | |
| S5 | **VM-LOW only.** The harness allocates until less than 10 % is available | `--allow-pressure` |
| S6 | As S2, plus an elevated app if you run the harness non-elevated | |
| S7 | Any; the harness starts 24 probes that exit 10–500 ms later | |
| S8 | As S2, on battery | |

- **Exit codes:** 0 means correct and safe behaviour; 3 means a fault (for example a control arm that changed memory, or internal failures).
- **Regressions:** a latency p95 more than 20 % above the control after trimming is listed under *Findings* in the summary. It is a result, not a harness failure.

## CI smoke

On every pull request, the `Benchmark smoke` job runs:
- the soak with 200 cycles;
- S3, S6 and S7 with `--quick --probes-only`.

These runs check that the harness and engine stay safe and leak-free on a clean Windows image. Their numbers are not for publication.
