//! Storage benchmark: scan throughput, index memory, treemap layout, search and
//! duplicate hashing, on a generated tree (default) or an existing directory.
//!
//! ```text
//! storage-bench [--dirs N] [--files N] [--runs N] [--path DIR] [--fast VOLUME]
//! ```
//!
//! Prints one JSON object per measurement. Every number is measured on the machine it
//! runs on; nothing is extrapolated. `--fast` (Windows, elevated) times the NTFS scan.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use reclaim_storage_core::ports::Fs;
use reclaim_storage_core::query::{Filter, TextMatch, extension_stats};
use reclaim_storage_core::tree::{SizeBasis, TOP, Tree};
use reclaim_storage_core::treemap::{LayoutOptions, Rect, icicle, treemap};
use reclaim_storage_engine::dupes::{DupOptions, files_from_tree, find};
use reclaim_storage_engine::scan::{ScanControl, ScanOptions, ScanTarget, scan_blocking};

fn arg(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn fs() -> Arc<dyn Fs> {
    #[cfg(windows)]
    {
        Arc::new(reclaim_storage_windows::WinFs)
    }
    #[cfg(not(windows))]
    {
        Arc::new(reclaim_storage_engine::stdfs::StdFs)
    }
}

/// `dirs` directories of `files` files each (sizes 1..=8 KiB, every 50th duplicated
/// content), two levels deep.
fn generate(root: &std::path::Path, dirs: usize, files: usize) -> std::io::Result<u64> {
    let mut n = 0;
    for d in 0..dirs {
        let dir = root.join(format!("g{}", d % 16)).join(format!("d{d}"));
        std::fs::create_dir_all(&dir)?;
        for f in 0..files {
            let size = 1024 * (1 + (d * 7 + f) % 8);
            let byte = if f % 50 == 0 {
                0xAB
            } else {
                (d * 31 + f) as u8
            };
            let ext = ["txt", "log", "bin", "dat", "json"][f % 5];
            std::fs::write(dir.join(format!("f{f}.{ext}")), vec![byte; size])?;
            n += 1;
        }
    }
    Ok(n)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs: usize = arg(&args, "--runs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
        .max(1);
    let fs = fs();
    let (root, generated) = match arg(&args, "--path") {
        Some(p) => (p, None),
        None => {
            let dirs = arg(&args, "--dirs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(400);
            let files = arg(&args, "--files")
                .and_then(|v| v.parse().ok())
                .unwrap_or(50);
            let dir =
                std::env::temp_dir().join(format!("reclaim-storage-bench-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let t = Instant::now();
            let n = generate(&dir, dirs, files).expect("generate test tree");
            println!(
                "{{\"bench\":\"generate\",\"files\":{n},\"ms\":{}}}",
                t.elapsed().as_millis()
            );
            (dir.to_string_lossy().into_owned(), Some(dir))
        }
    };

    let mut last = None;
    for threads in [1usize, 4] {
        let mut times = Vec::new();
        for _ in 0..runs {
            let tree = Arc::new(RwLock::new(Tree::new(fs.style())));
            let opts = ScanOptions {
                threads,
                ..ScanOptions::default()
            };
            let t = Instant::now();
            let s = scan_blocking(
                &*fs,
                &tree,
                ScanTarget::Roots(vec![root.clone()]),
                &opts,
                &ScanControl::default(),
            );
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            last = Some((tree, s));
        }
        let (tree, s) = last.as_ref().expect("at least one run");
        // As the app does once a scan has finished.
        tree.write()
            .unwrap_or_else(|p| p.into_inner())
            .shrink_to_fit();
        let t = tree.read().unwrap_or_else(|p| p.into_inner());
        let ms = median(times);
        println!(
            "{{\"bench\":\"scan\",\"threads\":{threads},\"runs\":{runs},\"median_ms\":{ms:.1},\"files\":{},\"dirs\":{},\"errors\":{},\"files_per_s\":{:.0},\"index_bytes\":{},\"nodes\":{}}}",
            s.progress.files,
            s.progress.dirs,
            s.progress.errors,
            s.progress.files as f64 / (ms / 1000.0),
            t.memory_bytes(),
            t.len()
        );
        let (nodes, names, counts, links) = t.memory_parts();
        println!(
            "{{\"bench\":\"index_parts\",\"threads\":{threads},\"node_bytes\":{},\"nodes\":{nodes},\"names\":{names},\"dir_counters\":{counts},\"links\":{links},\"bytes_per_entry\":{:.1}}}",
            std::mem::size_of::<reclaim_storage_core::tree::Node>(),
            t.memory_bytes() as f64 / t.len().max(1) as f64
        );
    }
    let (tree, _) = last.expect("scan ran");
    let t = tree.read().unwrap_or_else(|p| p.into_inner());

    let o = LayoutOptions::default();
    let area = Rect::new(0.0, 0.0, 1200.0, 700.0);
    let mut times = Vec::new();
    let mut cells = 0;
    for _ in 0..runs {
        let s = Instant::now();
        cells = treemap(&t, TOP, area, &o).len();
        times.push(s.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "{{\"bench\":\"treemap\",\"cells\":{cells},\"median_ms\":{:.2}}}",
        median(times)
    );
    let s = Instant::now();
    let n = icicle(&t, TOP, area, 24.0, &o).len();
    println!(
        "{{\"bench\":\"icicle\",\"cells\":{n},\"ms\":{:.2}}}",
        s.elapsed().as_secs_f64() * 1000.0
    );
    let s = Instant::now();
    let e = extension_stats(&t, TOP, SizeBasis::Allocated).len();
    println!(
        "{{\"bench\":\"extensions\",\"extensions\":{e},\"ms\":{:.2}}}",
        s.elapsed().as_secs_f64() * 1000.0
    );
    let f = Filter {
        name: TextMatch::parse("*.log").ok().flatten(),
        ..Filter::default()
    };
    let s = Instant::now();
    let r = f.run(&t, TOP, 1000);
    println!(
        "{{\"bench\":\"search_glob\",\"matches\":{},\"ms\":{:.2}}}",
        r.total,
        s.elapsed().as_secs_f64() * 1000.0
    );
    let f = Filter {
        path: TextMatch::parse("/d1[0-9]+/").ok().flatten(),
        ..Filter::default()
    };
    let s = Instant::now();
    let r = f.run(&t, TOP, 1000);
    println!(
        "{{\"bench\":\"search_path_regex\",\"matches\":{},\"ms\":{:.2}}}",
        r.total,
        s.elapsed().as_secs_f64() * 1000.0
    );

    let files = files_from_tree(&t, TOP, 1);
    drop(t);
    let s = Instant::now();
    let r = find(
        &*fs,
        files,
        &DupOptions {
            min_size: 1,
            readers: 2,
        },
        &AtomicBool::new(false),
        &|_| {},
    );
    let ms = s.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{{\"bench\":\"duplicates\",\"candidates\":{},\"groups\":{},\"bytes_read\":{},\"ms\":{ms:.1},\"mib_per_s\":{:.1}}}",
        r.candidates,
        r.groups.len(),
        r.bytes_read,
        r.bytes_read as f64 / 1_048_576.0 / (ms / 1000.0)
    );

    #[cfg(windows)]
    if let Some(vol) = arg(&args, "--fast") {
        let tree = RwLock::new(Tree::new(fs.style()));
        let s = Instant::now();
        match reclaim_storage_windows::fast::fast_scan(
            &vol,
            &tree,
            &ScanControl::default(),
            &|_| {},
        ) {
            Ok(f) => println!(
                "{{\"bench\":\"fast_ntfs_scan\",\"files\":{},\"dirs\":{},\"orphans\":{},\"ms\":{}}}",
                f.files,
                f.dirs,
                f.orphans,
                s.elapsed().as_millis()
            ),
            Err(e) => println!("{{\"bench\":\"fast_ntfs_scan\",\"error\":\"{e}\"}}"),
        }
    }

    if let Some(dir) = generated {
        let _ = std::fs::remove_dir_all(dir);
    }
}
