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
