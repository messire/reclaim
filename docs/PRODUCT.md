# Reclaim — Product Requirements

**Status:** initial product brief; architecture and implementation details are intentionally open.

## 1. Vision

Build a lightweight, responsive, trustworthy cross-platform system resource optimizer. It should make memory and storage usage understandable, offer targeted reclaim operations, and demonstrate their real impact without aggressive or misleading "boost" claims.

## 2. Audience and jobs to be done

- Everyday users: understand why their computer feels constrained and safely recover resources.
- Power users: inspect process-level memory metrics, choose exclusions, trigger specific actions and see before/after measurements.
- Users with limited storage: identify reclaimable caches, temporary files and large space consumers without risking personal data.

## 3. Platform and release scope

- **First implementation:** Windows 10/11 x64.
- **Future:** Linux and macOS. Core logic should avoid unnecessary platform coupling; capabilities may differ and must be explicitly reported.
- **MVP:** memory monitoring, safe manual optimization, reporting, simple UI/tray.
- Storage scanning, automation and other platforms are later phases, not MVP blockers.

## 4. Functional requirements — memory MVP

### Monitoring
- Display installed/usable RAM, available memory, system memory pressure where supported, and process-level working set / private memory metrics.
- Clearly distinguish working set, private bytes/commit, cached/standby memory and available physical RAM. Do not label all of these as "used RAM".
- Refresh without noticeable UI stalls; support a low-overhead idle mode.

### Manual optimization
- Provide an explicit user-triggered optimization action for eligible processes.
- Evaluate documented Windows APIs such as EmptyWorkingSet / SetProcessWorkingSetSize; assess permissions, protected processes and realistic effects.
- Offer per-process exclusions and sensible protected defaults. Never assume all processes are safe to trim.
- Do not kill processes, free another process's allocations, disable services or change system settings as part of a normal RAM optimization.
- Report skipped processes, access-denied failures and unsupported operations without treating them as success.

### Diagnostics and results
- Capture before/after system and process metrics, elapsed time and operation results.
- Sample memory again after a delay to detect rapid working-set rebound.
- Where feasible, assess page faults, disk I/O and application responsiveness to detect regressions.
- Clearly communicate that working-set trimming may induce hard faults, I/O and worse latency; reclaimed working set is not equivalent to freed committed allocations.
- Do not claim an operation improved performance without supporting evidence.

### User experience
- Fast, simple main window with an obvious manual action, readable status and optional tray access.
- UI must not block during process enumeration or optimization.
- Show clear explanations of operations and limitations, not marketing-style "speed boosts".
- Normal operation should not require administrator privileges. Request elevation only for a specific justified capability.

## 5. Future requirements — storage

Inspired by Roomy, but not intended as a one-to-one clone:
- Scan for temporary files, application/browser caches, crash reports and other known safe-to-review categories.
- Show size, purpose, risk and likely regeneration behavior per category.
- Provide large-file discovery and navigable storage visualization.
- Default to read-only scanning; deletion requires explicit review and confirmation.
- Protect user documents and critical OS locations. Never blindly recurse through junctions/symlinks or follow paths into protected locations.
- Skip locked files; avoid destructive retries; use OS-native cleanup or uninstall flows where appropriate.
- Treat shader caches, Downloads, games, hibernation and system cleanup as distinct higher-risk decisions.
- Design Windows-specific discovery rules as replaceable platform adapters.

## 6. Future requirements — automation

- Opt-in, disabled by default.
- Trigger based on measured pressure or meaningful thresholds rather than arbitrary frequent trimming.
- Respect active applications, exclusions, cooldowns, battery state and user activity when relevant.
- Provide a pause/disable control and an auditable local history.
- Automatic storage operations should default to notification/review, not silent deletion.

## 7. Non-functional requirements

- Prioritize low startup latency, small idle footprint, near-zero idle CPU and predictable resource use.
- No required external managed runtime or embedded browser engine unless benchmarks justify it.
- No mandatory network, account, telemetry, ads or background update service.
- Reliable operation under access errors, process exits, permission boundaries and high system load.
- Least privilege, explicit error handling, deterministic cleanup and testability.
- Localization-ready UI. Screen-reader accessibility is not a requirement (owner decision, 2026-11-20). Avoid animation or periodic polling that increases idle load.
- Preserve future cross-platform options without forcing lowest-common-denominator behavior.

## 8. Technology decision

No implementation stack is mandated. Compare Rust, C++, Zig or other credible native approaches and UI options (including Slint and native UI) against measured startup time, idle RAM, CPU, binary/dependency footprint, safety, maintenance and portability. Document the decision and alternatives.

## 9. Out of scope for the MVP

Registry cleaners, driver installation, kernel drivers, service disabling, generic "game boost" tweaks, process termination, automatic disk deletion, and promises of guaranteed FPS or speed improvements.

## 10. Open questions

- Which memory metrics best predict a useful trim, and how will benefit versus cost be measured?
- What processes should be excluded by default?
- What constitutes acceptable UI startup and idle footprint on reference hardware?
- Which native UI toolkit meets size, responsiveness and portability requirements?
- Which actions require elevation, and how can the normal app remain unprivileged?
