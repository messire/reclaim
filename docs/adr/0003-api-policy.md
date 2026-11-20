# ADR-0003: Documented APIs only; experimental adapters are explicit

- **Status:** Proposed
- **Date:** 2026-11-20
- **Context documents:** PRODUCT §4, §9; REFERENCES "Evaluation principles"

## Context

Competing tools often use undocumented native APIs (for example standby-list purging through `NtSetSystemInformation`). These can produce impressive numbers but change system state, require administrator rights, and can break between Windows builds.

## Decision

1. Production adapters use only documented, supported Win32 APIs (Microsoft Learn reference pages exist and state supported OS versions).
2. Any undocumented mechanism may only be evaluated in `reclaim-bench` (research) or, if ever shipped, in a separate adapter that:
   - is compiled behind a non-default Cargo feature `experimental`;
   - is labelled "Experimental" in the UI with an explanation of what it changes system-wide;
   - is never part of the default action, automation or "trim all";
   - has its own ADR before shipping.
3. Operations that change system configuration (file cache limits, services, power settings, page file, registry tweaks) are out of scope regardless of API status (PRODUCT §4, §9).

## Consequences

- Reclaim's numbers may look less dramatic than tools that purge standby memory. The product accepts this; the report explains what Reclaim does and does not do.
- Fewer breakages across Windows updates and fewer anti-malware false positives.
