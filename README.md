# Reclaim

**Reclaim your resources.**

Reclaim is a lightweight, offline-first system utility for monitoring and reclaiming system resources without becoming a resource hog itself.

## Product principles

- **Fast by default.** Launch quickly, remain responsive, and consume minimal CPU and memory at idle.
- **Measure real outcomes.** Report changes in memory availability and process working sets, but do not mistake lower displayed memory usage for better performance.
- **Safe and transparent.** Explain operations, their limitations and possible side effects. Prefer documented APIs and least privilege.
- **User control.** Manual actions first; optional, conservative automation later.
- **Portable architecture.** Windows is the initial target, with Linux and macOS considered in core interfaces and design.
- **Offline and private.** No account, advertising, telemetry or mandatory network access.

## Roadmap

1. **Memory MVP (Windows):** monitoring, per-process diagnostics, manual working-set trimming, exclusions, result measurements and a lightweight desktop/tray experience.
2. **Storage:** safe cache and temporary-file discovery, reviewed cleanup, large-file analysis and a disk space map.
3. **Smart automation:** pressure-aware, conservative policies with cooldowns, exclusions and measurable outcomes.
4. **Additional platforms:** Linux and macOS implementations subject to platform capabilities and safety constraints.

## Development status

**Planning.** No language, UI toolkit or architecture has been selected yet. Rust + Slint is a candidate, not a requirement. See [Product requirements](docs/PRODUCT.md), [Acceptance criteria](docs/ACCEPTANCE.md), and [References](docs/REFERENCES.md).

## Initial engineering task

Evaluate suitable native technology stacks and OS capabilities; propose an architecture, risks, benchmarks and a phased MVP implementation plan. Document tradeoffs and open decisions before building the full application.

## License

A project license has not yet been selected. Reference projects have their own licenses; comply with their terms if incorporating code.
