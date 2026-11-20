# ADR-0002: Process and privilege model

- **Status:** Proposed
- **Date:** 2026-11-20
- **Context documents:** PRODUCT §4 (User experience), §7; ACCEPTANCE §1, §4

## Context

PRODUCT requires that normal operation needs no administrator rights and that elevation is requested only for a specific justified capability. ACCEPTANCE requires no hidden resident helpers, predictable tray lifecycle and that Quit stops background work. Many large processes are only trimmable from an elevated context.

## Decision

1. **Single unelevated process.** `reclaim.exe` runs with an `asInvoker` manifest, as one process with a UI thread and one engine thread. No service, no scheduled task, no helper process in the MVP.
2. **No whole-application elevation.** Reclaim never offers "restart as administrator". Running the full UI elevated would put a large attack surface (UI toolkit, file parsing, settings) at high integrity for a marginal benefit.
3. **Elevated targets are reported, not chased.** Processes Reclaim cannot open are shown with the reason `RequiresElevation`, `OtherUser` or `Protected`, and never counted as success.
4. **Future narrow broker (designed, not built).** If the owner later wants elevated trimming, it is added as a separate, minimal `reclaim-broker.exe`:
   - launched on demand via `ShellExecuteExW("runas")` for one batch, exits when done (no residency);
   - accepts exactly one request type over an anonymous pipe inherited from the parent: a list of `(pid, start_time)` to trim, plus a nonce;
   - re-validates each target and re-applies the system-critical list itself (does not trust the caller's policy);
   - never enables `SeDebugPrivilege`, never touches protected processes, writes no files, has no UI toolkit dependency;
   - returns per-process outcomes; the unelevated app remains the only thing that renders and stores results.
   This keeps the elevated code to a few hundred audited lines.
5. **Autostart (post-MVP, opt-in):** per-user `HKCU\...\Run` entry with `--tray`, created only from an explicit setting and removed when the setting is turned off or on "Quit and remove autostart". Never a service.

## Consequences

- MVP delivers full value for the user's own medium-integrity processes, which covers browsers, editors, chat apps and most desktop software.
- Some heavy consumers (elevated tools, other users' sessions, services, anti-cheat-protected games) remain untrimmed in the MVP; the UI must explain this clearly.
- The broker, if built, needs its own threat review and integration tests (confused-deputy, PID-reuse, malformed input).
