# Storage benchmarks

Produced by `storage-bench` (`crates/reclaim-bench/src/bin/storage_bench`). Every number below
was measured; nothing is extrapolated. Results depend heavily on the file system cache (all runs
here are warm) and on the disk.

## Linux development container, 2026-11-22

4 vCPU (Intel Xeon @ 2.30 GHz), Rust 1.97.0, release build, `StdFs` (standard library), warm cache.
This is **not** a Windows measurement and says nothing about NTFS or `WinFs` performance.

Generated tree (`storage-bench --runs 3`): 20 000 files of 1–8 KiB in 417 folders.

| Measurement | Result |
|---|---|
| Scan, 1 thread (median of 3) | 25.6 ms |
| Scan, 4 threads (median of 3) | 8.6 ms |
| Index memory | 2.1 MB for 20 418 nodes |
| Treemap layout 1200×700 (median of 3) | 0.24 ms, 6 043 cells |
| Icicle layout | 0.31 ms |
| Extension statistics | 0.14 ms |
| Search `*.log` (name glob) | 3.8 ms |
| Search `/d1[0-9]+/` (path regex) | 0.72 ms |
| Duplicates, all 20 000 files (prefix + full BLAKE3) | 109.6 ms, 92 MB read (802 MiB/s, cached) |

`/usr` of the container (`storage-bench --path /usr --runs 3`): 91 576 files, 9 215 folders.

| Measurement | Result |
|---|---|
| Scan, 1 thread (median of 3) | 172.1 ms |
| Scan, 4 threads (median of 3) | 77.3 ms |
| Index memory | 9.8 MB for 100 792 nodes (≈ 97 bytes per entry); 6.5 MB (≈ 64 bytes) since 2026-11-23, see below |
| Treemap layout 1200×700 | 1.66 ms, 3 259 cells |
| Search `*.log` | 30.4 ms |

## Windows

CI runs `storage-bench` on the GitHub Windows runner (generated tree, `C:\Windows\System32`
and the fast NTFS scan of `C:`, which works there because the runner is elevated) and puts the
JSON lines in the job summary and the `bench-smoke` artifact. Those runner numbers are a smoke
test on shared virtual hardware, not a performance claim. No measurement on the reference
laptop (`reference-hardware.md`) has been made yet; Reclaim makes no claim of being faster than
other tools.

### GitHub Windows runner, 2026-11-22 (PR #11 CI, one run)

Release build, `storage-bench --runs 3` on the generated tree, then `--runs 1` on
`C:\Windows\System32` and the fast scan of `C:`. Shared virtual hardware; single run; read as a
smoke test only.

Generated tree (20 000 files, 417 folders):

| Measurement | Result |
|---|---|
| Scan, 1 thread (median of 3) | 27.2 ms |
| Scan, 4 threads (median of 3) | 18.0 ms |
| Index memory | 2.9 MB for 20 418 nodes |
| Treemap layout 1200×700 | 0.52 ms, 6 043 cells |
| Search `*.log` / path regex | 6.84 ms / 1.01 ms |
| Duplicates, all 20 000 files | 405.4 ms, 92 MB read (216.8 MiB/s) |

`C:\Windows\System32` (21 538 files, 1 662 folders, 2 access errors):

| Measurement | Result |
|---|---|
| Scan, 1 thread (first, cold cache) | 1 186.8 ms |
| Scan, 4 threads (second, warm cache) | 52.7 ms |
| Index memory | 4.1 MB for 23 203 nodes |
| Treemap layout 1200×700 | 1.55 ms, 3 215 cells |
| Duplicates, 14 680 candidates | 92.4 s, 476 MB read (4.9 MiB/s, cold disk) |

The two System32 scans ran in order on the same tree, so the gap between them is mostly the
file system cache, not threading.

Fast NTFS scan of the whole `C:` volume: 1 215 935 files, 210 461 folders, 0 orphans, 34.7 s.
A folder-by-folder scan of the same volume was not measured, so there is no comparison.

## Index memory, 2026-11-23

A scan of a whole system drive (6.07 million files, 1.42 million folders) left Reclaim
at 666–759 MB, almost all of it the index (about 100 bytes per entry). Changes:

- node record 56 → 48 bytes (name length in 16 bits, last write time in 32 bits);
- directory counters 24 → 12 bytes;
- the arrays drop their growth slack when a scan finishes (`Tree::shrink_to_fit`);
- the whole index is freed after the window has been in the tray for a set time
  (default 10 minutes, Settings), and must be scanned again.

`storage-bench --path /usr` in the Linux container (same tree as above), `index_parts`:

| | Before | After |
|---|---|---|
| Node records | 7.3 MB (capacity) | 4.8 MB |
| Names | 2.1 MB (capacity) | 1.5 MB |
| Directory counters | 0.39 MB | 0.11 MB |
| Bytes per entry | 97.5 | 64.2 |

The saving on a Windows drive depends on name lengths and on how much slack the arrays
had when the scan ended; it has not been measured there yet.
