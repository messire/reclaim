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
| Index memory | 9.8 MB for 100 792 nodes (≈ 97 bytes per entry) |
| Treemap layout 1200×700 | 1.66 ms, 3 259 cells |
| Search `*.log` | 30.4 ms |

## Windows

CI runs `storage-bench` on the GitHub Windows runner (generated tree, `C:\Windows\System32`
and the fast NTFS scan of `C:`, which works there because the runner is elevated) and puts the
JSON lines in the job summary and the `bench-smoke` artifact. Those runner numbers are a smoke
test on shared virtual hardware, not a performance claim. No measurement on the reference
laptop (`reference-hardware.md`) has been made yet; Reclaim makes no claim of being faster than
other tools.
