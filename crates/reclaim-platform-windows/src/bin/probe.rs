//! Test probe for the Windows integration tests.
//!
//! `reclaim-test-probe <MiB>` allocates and touches `<MiB>` of private memory, prints
//! `ready`, then answers each `touch` line on stdin by touching every page again and
//! printing `done`. It exits when stdin closes.

use std::io::{BufRead, Write};

fn main() {
    let mib: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(64);
    let mut buf = vec![0u8; mib * 1024 * 1024];
    let touch = |b: &mut [u8], v: u8| {
        for page in b.chunks_mut(4096) {
            page[0] = v;
        }
    };
    touch(&mut buf, 1);
    let out = std::io::stdout();
    let _ = writeln!(out.lock(), "ready");
    let mut v = 2u8;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim() == "touch" {
            touch(&mut buf, v);
            v = v.wrapping_add(1);
            let _ = writeln!(out.lock(), "done");
        }
    }
    std::hint::black_box(&buf);
}
