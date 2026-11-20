# Reclaim — Research and References

These are references and research starting points, not instructions to copy implementations or assume their claims are verified.

## Wise Memory Optimizer

- Website: https://www.wisecleaner.com/wise-memory-optimizer.html
- Role: benchmark for fast launch, small footprint, straightforward RAM optimization and tray UX.
- Research questions: Which Windows memory APIs does it actually call? What changes in working sets, available RAM, commit, page faults and user-visible latency? Does its behavior vary with privileges?
- Do not assume the proprietary implementation is known. Compare observed behavior with documented APIs and measure effects.

## Roomy

- Repository: https://github.com/krezio/roomy
- License: MIT (verify the license in the version used).
- Role: inspiration for transparent storage cleanup, categorized caches, safe file operations, space maps, local-only behavior and opt-in background checks.
- Useful areas to study: `src/Roomy/Core/Scanner.cs`, `CacheFinder.cs`, `Cleaner.cs`, `SafeFs.cs`, `FileOps.cs`, `SpaceScanner.cs`.
- Roomy uses C#/.NET/WPF. Its technology stack is not a requirement for Reclaim.
- If copying or adapting code, preserve required copyright and license notices and review safety carefully.

## Microsoft Windows documentation

- EmptyWorkingSet: https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-emptyworkingset
- SetProcessWorkingSetSize: https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-setprocessworkingsetsize
- GetProcessMemoryInfo: https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo
- GlobalMemoryStatusEx: https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-globalmemorystatusex
- Windows memory performance: https://learn.microsoft.com/en-us/windows/win32/memory/memory-management

Read current documentation, permissions, limitations and OS support before implementing.

## Future platform research

- Linux: `/proc`, `process_madvise`, `MADV_COLD` / `MADV_PAGEOUT`, capability and ptrace restrictions; these are not guaranteed equivalents of Windows working-set trimming.
- macOS: Mach VM APIs, memory pressure and compression; public APIs for controlling unrelated processes are limited.

## Evaluation principles

- Prefer documented, supported APIs; isolate any undocumented mechanism behind a clearly marked experimental adapter.
- Do not equate reclaiming physical pages with deallocating another process's committed memory.
- Evaluate safety and latency costs alongside immediate RAM changes.
- Verify reference-project licenses before incorporating code.
