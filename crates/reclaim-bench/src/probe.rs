//! Probe workloads (BENCHMARKS §5), in portable `std` so they also run in tests.

use std::time::{Duration, Instant};

/// Page size used for touching (4 KiB pages on x86-64 Windows).
pub const PAGE: usize = 4096;

/// A block of touched memory.
#[derive(Debug)]
pub struct Block {
    buf: Vec<u8>,
    seed: u64,
}

impl Block {
    /// Allocates `mib` MiB and touches every page.
    pub fn new(mib: usize) -> Block {
        let mut b = Block {
            buf: vec![0u8; mib * 1024 * 1024],
            seed: 0x9e37_79b9_7f4a_7c15,
        };
        b.touch(true);
        b
    }

    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True when the block is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Touches every page in order. With `dirty`, writes (pages become modified and must
    /// be written to the page file when trimmed); otherwise only reads.
    pub fn touch(&mut self, dirty: bool) -> u64 {
        let mut sum = 0u64;
        for i in (0..self.buf.len()).step_by(PAGE) {
            if dirty {
                self.buf[i] = self.buf[i].wrapping_add(1);
            }
            sum = sum.wrapping_add(u64::from(self.buf[i]));
        }
        std::hint::black_box(sum)
    }

    /// Touches every page once in a pseudo-random order (defeats prefetching).
    pub fn touch_random(&mut self, dirty: bool) -> u64 {
        let pages = self.buf.len() / PAGE;
        if pages == 0 {
            return 0;
        }
        // Full-period LCG walk over a power-of-two range, skipping indices past the end.
        let range = pages.next_power_of_two() as u64;
        let mut x = self.seed % range;
        let mut sum = 0u64;
        for _ in 0..range {
            x = (x.wrapping_mul(5).wrapping_add(1)) % range;
            let p = x as usize;
            if p < pages {
                let i = p * PAGE;
                if dirty {
                    self.buf[i] = self.buf[i].wrapping_add(1);
                }
                sum = sum.wrapping_add(u64::from(self.buf[i]));
            }
        }
        self.seed = self.seed.wrapping_add(1);
        std::hint::black_box(sum)
    }
}

/// Time for one read pass over the block: the responsiveness proxy. After a trim the first
/// passes pay soft or hard faults.
pub fn timed_pass(b: &mut Block) -> Duration {
    let t = Instant::now();
    b.touch(false);
    t.elapsed()
}

/// Parses a `latency` probe output line `<ms since start> <pass µs>`.
pub fn parse_latency_line(line: &str) -> Option<(u64, u64)> {
    let mut it = line.split_whitespace();
    let t = it.next()?.parse().ok()?;
    let us = it.next()?.parse().ok()?;
    Some((t, us))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_touch_every_page() {
        let mut b = Block::new(1);
        assert_eq!(b.len(), 1024 * 1024);
        assert!(!b.is_empty());
        // After `new` (one dirty pass) every page holds 1.
        assert_eq!(b.touch(false), 256);
        assert_eq!(b.touch(true), 512);
        assert_eq!(
            b.touch_random(false),
            512,
            "random walk visits each page once"
        );
        assert!(timed_pass(&mut b) < Duration::from_secs(1));
        let mut empty = Block::new(0);
        assert_eq!(empty.touch_random(true), 0);
    }

    #[test]
    fn latency_lines() {
        assert_eq!(parse_latency_line("1200 350"), Some((1200, 350)));
        assert_eq!(parse_latency_line("ready"), None);
        assert_eq!(parse_latency_line("12"), None);
    }
}
