//! Number and time formatting for display.

use reclaim_core::units::{Bytes, Millis};

/// Formats an optional byte value ("not available" when unknown).
pub fn opt_bytes(b: Option<Bytes>) -> String {
    b.map_or_else(
        || crate::strings::tr("value.unknown").to_owned(),
        |b| b.to_string(),
    )
}

/// Share of `part` in `whole`, as a whole percent (0 when `whole` is zero).
pub fn percent(part: Bytes, whole: Bytes) -> u64 {
    if whole.get() == 0 {
        0
    } else {
        ((u128::from(part.get()) * 100 + u128::from(whole.get()) / 2) / u128::from(whole.get()))
            as u64
    }
}

/// `YYYY-MM-DD HH:MM` for a wall time (ms since the Unix epoch) shifted by the local
/// UTC offset in minutes.
pub fn date_time(wall: Millis, utc_offset_min: i32) -> String {
    let secs = (wall.0 / 1000) as i64 + i64::from(utc_offset_min) * 60;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_time(Millis(0), 0), "1970-01-01 00:00");
        // 2026-11-20 13:37:00 UTC.
        let t = Millis(1_795_181_820_000);
        assert_eq!(date_time(t, 0), "2026-11-20 13:37");
        assert_eq!(date_time(t, 180), "2026-11-20 16:37");
        assert_eq!(date_time(t, -14 * 60), "2026-11-19 23:37");
        assert_eq!(date_time(Millis(951_782_400_000), 0), "2000-02-29 00:00");
    }

    #[test]
    fn numbers() {
        assert_eq!(percent(Bytes(1), Bytes(3)), 33);
        assert_eq!(percent(Bytes(2), Bytes(3)), 67);
        assert_eq!(percent(Bytes(5), Bytes::ZERO), 0);
        assert_eq!(opt_bytes(None), "not available");
        assert_eq!(opt_bytes(Some(Bytes::from_mib(2))), "2.0 MiB");
    }
}
