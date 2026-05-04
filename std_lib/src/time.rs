use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_monotonic() -> f64 {
    static START: OnceLock<SystemTime> = OnceLock::new();
    let start = START.get_or_init(SystemTime::now);
    SystemTime::now()
        .duration_since(*start)
        .unwrap()
        .as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_sleep(secs: f64) {
    thread::sleep(Duration::from_secs_f64(secs));
}

/// Converts a Unix timestamp to `(year, month, day, hour, minute, second)`.
///
/// # Examples
///
/// ```
/// use olive_std::unix_to_ymd_hms;
/// let (y, m, d, h, mn, s) = unix_to_ymd_hms(0);
/// assert_eq!((y, m, d, h, mn, s), (1970, 1, 1, 0, 0, 0));
/// ```
pub fn unix_to_ymd_hms(ts: i64) -> (i64, i64, i64, i64, i64, i64) {
    let mut d = ts / 86400;
    let sec = ts.rem_euclid(86400);
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    if ts < 0 && (ts % 86400) != 0 {
        d -= 1;
    }
    d += 719468;
    let era = if d >= 0 { d } else { d - 146096 } / 146097;
    let doe = d - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_now_positive() {
        let t = olive_time_now();
        assert!(t > 1_700_000_000.0);
    }

    #[test]
    fn time_monotonic_increasing() {
        let a = olive_time_monotonic();
        let b = olive_time_monotonic();
        assert!(b >= a);
    }

    #[test]
    fn time_sleep_short() {
        let start = olive_time_monotonic();
        olive_time_sleep(0.001);
        let elapsed = olive_time_monotonic() - start;
        assert!(elapsed >= 0.0005);
    }

    #[test]
    fn unix_to_ymd_hms_epoch() {
        let (y, m, d, h, mn, s) = unix_to_ymd_hms(0);
        assert_eq!(y, 1970);
        assert_eq!(m, 1);
        assert_eq!(d, 1);
        assert_eq!(h, 0);
        assert_eq!(mn, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn unix_to_ymd_hms_known() {
        let (y, m, d, h, mn, s) = unix_to_ymd_hms(1700000000);
        assert_eq!(y, 2023);
        assert_eq!(m, 11);
        assert_eq!(d, 14);
        assert_eq!(h, 22);
        assert_eq!(mn, 13);
        assert_eq!(s, 20);
    }

    #[test]
    fn unix_to_ymd_hms_negative() {
        let (y, m, d, _, _, _) = unix_to_ymd_hms(-1);
        assert_eq!(y, 1969);
        assert_eq!(m, 12);
        assert_eq!(d, 31);
    }
}
