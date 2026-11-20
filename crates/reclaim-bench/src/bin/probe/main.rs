//! Probe workloads for scenarios (BENCHMARKS §5). Each probe prints `ready` once its
//! memory is touched and exits when its stdin closes (or after its delay).
//!
//! ```text
//! reclaim-probe alloc-touch --size <MiB> [--pattern seq|random] [--dirty]
//! reclaim-probe latency --size <MiB> [--interval-ms 100]
//! reclaim-probe exit-after --ms <N> [--size <MiB>]
//! ```
//! `latency` prints one line per pass: `<ms since start> <pass µs>`.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reclaim_bench::probe::{Block, timed_pass};

fn arg(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn num(args: &[String], name: &str, default: u64) -> u64 {
    arg(args, name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Sets the flag when stdin reaches end of file (the harness dropped its pipe).
fn stdin_closed() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let f = flag.clone();
    std::thread::spawn(move || {
        let mut sink = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        f.store(true, Ordering::SeqCst);
    });
    flag
}

fn ready() {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "ready");
    let _ = out.flush();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size = num(&args, "--size", 64) as usize;
    match args.first().map(String::as_str) {
        Some("alloc-touch") => {
            let mut b = Block::new(size);
            let dirty = args.iter().any(|a| a == "--dirty");
            if arg(&args, "--pattern").as_deref() == Some("random") {
                b.touch_random(dirty);
            } else {
                b.touch(dirty);
            }
            ready();
            let closed = stdin_closed();
            while !closed.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
            std::hint::black_box(&b);
        }
        Some("latency") => {
            let interval = Duration::from_millis(num(&args, "--interval-ms", 100));
            let mut b = Block::new(size);
            ready();
            let closed = stdin_closed();
            let start = Instant::now();
            let mut out = std::io::stdout().lock();
            while !closed.load(Ordering::SeqCst) {
                let d = timed_pass(&mut b);
                if writeln!(out, "{} {}", start.elapsed().as_millis(), d.as_micros()).is_err() {
                    break;
                }
                let _ = out.flush();
                std::thread::sleep(interval.saturating_sub(d));
            }
        }
        Some("exit-after") => {
            let b = Block::new(size);
            ready();
            std::thread::sleep(Duration::from_millis(num(&args, "--ms", 100)));
            std::hint::black_box(&b);
        }
        _ => {
            eprintln!("usage: reclaim-probe <alloc-touch|latency|exit-after> [options]");
            std::process::exit(2);
        }
    }
}
