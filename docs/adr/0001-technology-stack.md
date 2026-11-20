# ADR-0001: Technology stack

- **Status:** Proposed. Language decision proposed as accepted; UI toolkit decision **gated by the M0 spike**.
- **Date:** 2026-11-20
- **Context documents:** PRODUCT §7–8, ACCEPTANCE §2

## Context

Reclaim must start in < 300 ms, idle at ≈ 0 % CPU and preferably < 15 MiB private memory, ship without a separately installed runtime, be accessible and localization-ready, keep Linux/macOS open, and contain safe, auditable code around cross-process Win32 calls. No stack is mandated; Rust + Slint is a candidate.

## Options — language

| Criterion | Rust | C++ (MSVC/Clang) | Zig | C# / .NET (NativeAOT) |
|---|---|---|---|---|
| Runtime dependency | None (static CRT possible) | None | None | None with NativeAOT; WinForms/WPF not AOT-friendly |
| Memory safety in our code | Strong; `unsafe` confined to FFI crate | Manual | Manual, better defaults | Strong |
| Win32 bindings | `windows-rs` (official Microsoft, generated from metadata) | Native headers | Manual / translate-c | P/Invoke, CsWin32 |
| Idle footprint | Small (1–4 MiB baseline) | Smallest | Smallest | Larger (GC heap, 10–30 MiB typical) **[measure if considered]** |
| Cross-platform story | Excellent (one toolchain) | Good, build systems heavier | Good, ecosystem young | Good runtime, weak native UI story |
| Ecosystem maturity for this app | High (serde, proptest, windows-rs, toolkits) | High | Low (pre-1.0) | High, but UI options either heavy or Windows-only |
| Maintainability for a small team | High | Medium | Medium–low | High |

**Decision (proposed): Rust**, stable channel, pinned toolchain. Rationale: no runtime, strong safety where our risk lives (handles, races, error paths), first-party Win32 bindings, one language for all future platforms. Zig is promising but pre-1.0; C++ adds risk with no measurable gain for this workload; .NET conflicts with the footprint target.

## Options — UI toolkit

| Toolkit | Footprint expectation | Startup | Accessibility (UIA) | Portability | License | Risk |
|---|---|---|---|---|---|---|
| **Slint** (software renderer or Skia/FemtoVG) | Moderate; software renderer avoids GPU context | Fast expected | Via AccessKit on Windows **[verify completeness: tables, lists]** | Windows, Linux, macOS | GPLv3, royalty-free (attribution required), or commercial **[verify current terms]** | Medium: license fit; table widget maturity; memory while hidden |
| **Native Win32** (direct `windows-rs`, common controls) | Smallest | Fastest | Built-in for standard controls | Windows only; per-platform UI later | n/a | Medium: more code per screen; custom drawing via GDI/Direct2D |
| egui (eframe) | GPU context (glow/wgpu) likely adds tens of MiB | Fast | AccessKit, partial | All | MIT/Apache | High: footprint, immediate-mode redraw discipline |
| Iced | wgpu context, larger | Medium | Limited **[verify]** | All | MIT | High: footprint, a11y |
| Qt 6 (C++ or via bindings) | 20–40 MiB distribution, higher idle | Medium | Good | All | LGPL/commercial | High: size, C++ interop |
| WinUI 3 / Windows App SDK | Requires runtime package | Medium | Excellent | Windows only | MIT | Rejected: runtime dependency |
| Tauri / WebView2 / Electron | Browser engine, 50–150 MiB | Slow | Good | All | Various | Rejected by NFR (embedded browser engine) |

### Decision procedure (M0 spike)

Build the same minimal UI twice — **Slint (software renderer)** and **native Win32** — containing: system panel, a 300-row sortable table, a modal confirmation, tray icon, hide-to-tray and restore. Measure per `BENCHMARKS.md` on reference hardware:

| Gate | Threshold |
|---|---|
| Cold start to usable UI | median < 300 ms, p95 < 450 ms |
| Idle private bytes, window visible | < 15 MiB preferred, < 25 MiB hard limit |
| Idle private bytes, tray only | < 8 MiB preferred |
| Idle CPU, 5 min, window visible and tray | < 0.1 % average, no periodic wake-ups attributable to the toolkit |
| Screen reader (Narrator, NVDA) | All controls named; table navigable; actions reachable by keyboard |
| Release binary size | < 10 MiB preferred |
| License | Compatible with the chosen project license |

**Selection rule:** choose Slint if it passes all gates (portability benefit). Otherwise choose native Win32 for Windows, and decide the per-platform UI approach for phase 4 then. The ViewModel layer (ARCHITECTURE §3) keeps either choice from leaking into core or engine.

## Other dependencies (proposed)

| Need | Choice | Reason |
|---|---|---|
| Win32 FFI | `windows` / `windows-sys` | Official; `windows-sys` for lower compile cost where COM is not needed |
| Serialization | `serde` + `toml` (settings), `serde_json` (history) | Mature; modest size |
| Threads / channels | `std` (`mpsc`) or `crossbeam-channel` | No async runtime needed |
| Localization | `fluent` or the toolkit's built-in (Slint `@tr`) | Decided with the toolkit |
| Logging | `log` + a tiny local file sink | Avoid heavy tracing stacks in the shipping binary |
| Testing | `proptest`, `insta` (report snapshots) | Dev-only |
| Audit | `cargo-deny` | Licenses, advisories, banned crates (networking) |

No async runtime (`tokio`), no HTTP/TLS crates, no telemetry SDKs.

## Consequences

- One language across all phases; contributors need Rust and basic Win32 knowledge.
- The toolkit decision is postponed by at most the length of M0, but is made on evidence.
- If Slint is chosen under its royalty-free license, the About screen must carry the required attribution; under GPLv3 the project itself must be GPLv3-compatible. This couples to the project license decision.
