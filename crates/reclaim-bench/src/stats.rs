//! Small statistics used by every summary (BENCHMARKS §2: median and p95 for start-up,
//! mean and max for idle).

/// Median (mean of the middle two for an even count); `None` when empty.
pub fn median(v: &[f64]) -> Option<f64> {
    let s = sorted(v);
    let n = s.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(s[n / 2]),
        _ => Some((s[n / 2 - 1] + s[n / 2]) / 2.0),
    }
}

/// Nearest-rank percentile `p` in 0..=100; `None` when empty.
pub fn percentile(v: &[f64], p: f64) -> Option<f64> {
    let s = sorted(v);
    if s.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    Some(s[rank.clamp(1, s.len()) - 1])
}

/// Arithmetic mean; `None` when empty.
pub fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Largest value; `None` when empty.
pub fn max(v: &[f64]) -> Option<f64> {
    v.iter().copied().reduce(f64::max)
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(f64::total_cmp);
    s
}

/// True if a resource count trends upward over a soak (BENCHMARKS B8): the last sample
/// exceeds the second one (after warm-up) by more than `slack_pct` percent plus
/// `slack_abs`, and the series never comes back down to the warm-up level.
pub fn trends_upward(series: &[u64], slack_pct: u64, slack_abs: u64) -> bool {
    let (Some(&base), Some(&last)) = (series.get(1).or(series.first()), series.last()) else {
        return false;
    };
    let limit = base + base * slack_pct / 100 + slack_abs;
    last > limit && series.iter().skip(1).all(|&x| x >= base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_stats() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 95.0), Some(95.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&[7.0], 95.0), Some(7.0));
        assert_eq!(percentile(&[], 95.0), None);
        assert_eq!(mean(&[1.0, 2.0, 3.0]), Some(2.0));
        assert_eq!(max(&[1.0, 5.0, 3.0]), Some(5.0));
        assert_eq!(max(&[]), None);
        assert_eq!(median(&[f64::NAN, 1.0]), Some(1.0));
    }

    #[test]
    fn trend() {
        assert!(!trends_upward(&[], 10, 0));
        assert!(!trends_upward(&[100], 10, 0));
        assert!(!trends_upward(&[90, 100, 104, 108], 10, 0), "within slack");
        assert!(trends_upward(&[90, 100, 150, 200], 10, 0), "leak");
        assert!(
            !trends_upward(&[90, 100, 95, 200], 10, 0),
            "came back down: noise"
        );
        assert!(!trends_upward(&[90, 100, 120], 10, 50), "absolute slack");
    }
}
