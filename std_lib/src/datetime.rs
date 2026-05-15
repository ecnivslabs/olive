use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(year) { 29 } else { 28 },
        _ => 30,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn ymd_to_unix(year: i64, month: i64, day: i64, h: i64, min: i64, sec: i64) -> i64 {
    // Gregorian proleptic calendar
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + h * 3600 + min * 60 + sec
}

fn unix_to_parts(ts: i64) -> (i64, i64, i64, i64, i64, i64) {
    crate::unix_to_ymd_hms(ts)
}

fn local_offset_secs() -> i64 {
    #[cfg(unix)]
    return unix_tz_offset();
    #[cfg(windows)]
    return windows_tz_offset();
    #[cfg(not(any(unix, windows)))]
    return 0;
}

#[cfg(unix)]
fn unix_tz_offset() -> i64 {
    unsafe {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        let local = libc::localtime(&ts);
        if local.is_null() { return 0; }
        (*local).tm_gmtoff as i64
    }
}

#[cfg(windows)]
fn windows_tz_offset() -> i64 {
    unsafe {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        let mut ltm: libc::tm = std::mem::zeroed();
        let mut utm: libc::tm = std::mem::zeroed();
        if libc::localtime_s(&mut ltm, &ts) != 0 { return 0; }
        if libc::gmtime_s(&mut utm, &ts) != 0 { return 0; }
        let l = ltm.tm_hour as i64 * 3600 + ltm.tm_min as i64 * 60 + ltm.tm_sec as i64;
        let u = utm.tm_hour as i64 * 3600 + utm.tm_min as i64 * 60 + utm.tm_sec as i64;
        let day_diff = (ltm.tm_yday - utm.tm_yday) as i64;
        let day_secs = if day_diff > 1 { -86400 } else if day_diff < -1 { 86400 } else { day_diff * 86400 };
        l - u + day_secs
    }
}

fn parse_datetime_str(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() >= 10 {
        let year = s[0..4].parse::<i64>().ok()?;
        if s.as_bytes().get(4) != Some(&b'-') { return None; }
        let month = s[5..7].parse::<i64>().ok()?;
        if s.as_bytes().get(7) != Some(&b'-') { return None; }
        let day = s[8..10].parse::<i64>().ok()?;

        let (h, min, sec, tz_offset) = if s.len() > 10 {
            let sep = s.as_bytes().get(10);
            if sep != Some(&b'T') && sep != Some(&b' ') { return None; }
            if s.len() < 19 { return None; }
            let h = s[11..13].parse::<i64>().ok()?;
            if s.as_bytes().get(13) != Some(&b':') { return None; }
            let min = s[14..16].parse::<i64>().ok()?;
            if s.as_bytes().get(16) != Some(&b':') { return None; }
            let sec = s[17..19].parse::<i64>().ok()?;
            let tz_offset = if s.len() > 19 {
                parse_tz_suffix(&s[19..])
            } else {
                0
            };
            (h, min, sec, tz_offset)
        } else {
            (0, 0, 0, 0)
        };

        let ts = ymd_to_unix(year, month, day, h, min, sec);
        return Some(ts - tz_offset);
    }
    None
}

fn parse_tz_suffix(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() || s == "Z" || s == "z" {
        return 0;
    }
    let sign = match s.as_bytes().first() {
        Some(b'+') => 1i64,
        Some(b'-') => -1i64,
        _ => return 0,
    };
    let rest = &s[1..];
    if rest.len() >= 5 {
        let hh = rest[0..2].parse::<i64>().unwrap_or(0);
        let mm = rest[3..5].parse::<i64>().unwrap_or(0);
        sign * (hh * 3600 + mm * 60)
    } else if rest.len() >= 2 {
        let hh = rest[0..2].parse::<i64>().unwrap_or(0);
        sign * hh * 3600
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_parse(s: i64) -> f64 {
    if s == 0 {
        return -1.0;
    }
    let text = olive_str_from_ptr(s);
    match parse_datetime_str(&text) {
        Some(ts) => ts as f64,
        None => -1.0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_now() -> f64 {
    crate::olive_time_now()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_utcnow() -> f64 {
    crate::olive_time_now()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_local_offset() -> i64 {
    local_offset_secs()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_to_local(ts: f64) -> f64 {
    ts + local_offset_secs() as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_from_local(ts: f64) -> f64 {
    ts - local_offset_secs() as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_parts(ts: f64) -> i64 {
    let (year, month, day, h, min, sec) = unix_to_parts(ts as i64);
    let dow = day_of_week(ts as i64);
    let mut fields = HashMap::default();
    fields.insert("year".to_string(), year);
    fields.insert("month".to_string(), month);
    fields.insert("day".to_string(), day);
    fields.insert("hour".to_string(), h);
    fields.insert("minute".to_string(), min);
    fields.insert("second".to_string(), sec);
    fields.insert("weekday".to_string(), dow); // 0=Mon
    fields.insert("timestamp".to_string(), ts as i64);
    Box::into_raw(Box::new(OliveObj { kind: crate::KIND_OBJ, fields })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_from_parts(
    year: i64, month: i64, day: i64,
    hour: i64, minute: i64, second: i64,
) -> f64 {
    ymd_to_unix(year, month, day, hour, minute, second) as f64
}

fn day_of_week(ts: i64) -> i64 {
    // 1970-01-01 was a Thursday (3, where Mon=0)
    let days = if ts >= 0 {
        ts / 86400
    } else {
        (ts - 86399) / 86400
    };
    (days + 3).rem_euclid(7)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_weekday(ts: f64) -> i64 {
    day_of_week(ts as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_weekday_name(ts: f64) -> i64 {
    let names = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let dow = day_of_week(ts as i64) as usize;
    olive_str_internal(names[dow.min(6)])
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_month_name(ts: f64) -> i64 {
    let names = ["", "January", "February", "March", "April", "May", "June",
                  "July", "August", "September", "October", "November", "December"];
    let (_, month, _, _, _, _) = unix_to_parts(ts as i64);
    let m = month as usize;
    olive_str_internal(names[m.clamp(1, 12)])
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_days(ts: f64, days: i64) -> f64 {
    ts + (days * 86400) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_hours(ts: f64, hours: i64) -> f64 {
    ts + (hours * 3600) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_minutes(ts: f64, minutes: i64) -> f64 {
    ts + (minutes * 60) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_seconds(ts: f64, seconds: i64) -> f64 {
    ts + seconds as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_months(ts: f64, months: i64) -> f64 {
    let (mut year, mut month, day, h, min, sec) = unix_to_parts(ts as i64);
    month += months;
    year += (month - 1) / 12;
    month = (month - 1).rem_euclid(12) + 1;
    let clamped_day = day.min(days_in_month(year, month));
    ymd_to_unix(year, month, clamped_day, h, min, sec) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_add_years(ts: f64, years: i64) -> f64 {
    let (year, month, day, h, min, sec) = unix_to_parts(ts as i64);
    let new_year = year + years;
    let clamped_day = day.min(days_in_month(new_year, month));
    ymd_to_unix(new_year, month, clamped_day, h, min, sec) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_diff_days(a: f64, b: f64) -> i64 {
    ((a - b) / 86400.0) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_diff_seconds(a: f64, b: f64) -> i64 {
    (a - b) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_start_of_day(ts: f64) -> f64 {
    let (year, month, day, _, _, _) = unix_to_parts(ts as i64);
    ymd_to_unix(year, month, day, 0, 0, 0) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_end_of_day(ts: f64) -> f64 {
    let (year, month, day, _, _, _) = unix_to_parts(ts as i64);
    ymd_to_unix(year, month, day, 23, 59, 59) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_start_of_month(ts: f64) -> f64 {
    let (year, month, _, _, _, _) = unix_to_parts(ts as i64);
    ymd_to_unix(year, month, 1, 0, 0, 0) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_is_leap_year(year: i64) -> i64 {
    if is_leap(year) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_days_in_month(year: i64, month: i64) -> i64 {
    days_in_month(year, month)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_datetime_format(ts: f64, fmt: i64) -> i64 {
    let (year, month, day, h, min, sec) = unix_to_parts(ts as i64);
    let dow = day_of_week(ts as i64);
    let fmt_str = if fmt == 0 {
        "%Y-%m-%dT%H:%M:%S".to_string()
    } else {
        olive_str_from_ptr(fmt)
    };
    let weekday_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let weekday_full = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let month_names = ["", "Jan", "Feb", "Mar", "Apr", "May", "Jun",
                       "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month_full = ["", "January", "February", "March", "April", "May", "June",
                      "July", "August", "September", "October", "November", "December"];
    let mut out = String::with_capacity(fmt_str.len() + 16);
    let mut chars = fmt_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", year)),
                Some('y') => out.push_str(&format!("{:02}", year % 100)),
                Some('m') => out.push_str(&format!("{:02}", month)),
                Some('d') => out.push_str(&format!("{:02}", day)),
                Some('H') => out.push_str(&format!("{:02}", h)),
                Some('M') => out.push_str(&format!("{:02}", min)),
                Some('S') => out.push_str(&format!("{:02}", sec)),
                Some('A') => out.push_str(weekday_full[dow as usize]),
                Some('a') => out.push_str(weekday_names[dow as usize]),
                Some('B') => out.push_str(month_full[month as usize]),
                Some('b') | Some('h') => out.push_str(month_names[month as usize]),
                Some('e') => out.push_str(&format!("{:2}", day)),
                Some('I') => out.push_str(&format!("{:02}", if h % 12 == 0 { 12 } else { h % 12 })),
                Some('p') => out.push_str(if h < 12 { "AM" } else { "PM" }),
                Some('P') => out.push_str(if h < 12 { "am" } else { "pm" }),
                Some('j') => {
                    let doy = day_of_year(year, month, day);
                    out.push_str(&format!("{:03}", doy));
                }
                Some('u') => out.push_str(&format!("{}", dow + 1)),
                Some('w') => out.push_str(&format!("{}", (dow + 1) % 7)),
                Some('%') => out.push('%'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(x) => { out.push('%'); out.push(x); }
                None => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    olive_str_internal(&out)
}

fn day_of_year(year: i64, month: i64, day: i64) -> i64 {
    let months = [0i64, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let base = months[(month as usize - 1).min(11)];
    let leap_add = if month > 2 && is_leap(year) { 1 } else { 0 };
    base + day + leap_add
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn parse_iso8601() {
        assert_eq!(olive_datetime_parse(s("2024-01-15T11:50:45")), 1705319445.0);
        assert_eq!(olive_datetime_parse(s("2024-01-15T11:50:45Z")), 1705319445.0);
        assert_eq!(olive_datetime_parse(s("2024-01-15 11:50:45")), 1705319445.0);
    }

    #[test]
    fn parse_with_tz_offset() {
        let ts_utc = olive_datetime_parse(s("2024-01-15T11:50:45Z"));
        let ts_plus5 = olive_datetime_parse(s("2024-01-15T16:50:45+05:00"));
        assert!((ts_utc - ts_plus5).abs() < 1.0);
    }

    #[test]
    fn parse_date_only() {
        let ts = olive_datetime_parse(s("2024-01-15"));
        assert_eq!(ts, ymd_to_unix(2024, 1, 15, 0, 0, 0) as f64);
    }

    #[test]
    fn parse_invalid_returns_minus_one() {
        assert_eq!(olive_datetime_parse(s("not a date")), -1.0);
        assert_eq!(olive_datetime_parse(0), -1.0);
    }

    #[test]
    fn parts_roundtrip() {
        let ts = 1705319445.0f64;
        let parts_ptr = olive_datetime_parts(ts);
        assert_ne!(parts_ptr, 0);
        let obj = unsafe { &*(parts_ptr as *const OliveObj) };
        assert_eq!(*obj.fields.get("year").unwrap(), 2024);
        assert_eq!(*obj.fields.get("month").unwrap(), 1);
        assert_eq!(*obj.fields.get("day").unwrap(), 15);
        assert_eq!(*obj.fields.get("hour").unwrap(), 11);
        assert_eq!(*obj.fields.get("minute").unwrap(), 50);
        assert_eq!(*obj.fields.get("second").unwrap(), 45);
    }

    #[test]
    fn from_parts_epoch() {
        let ts = olive_datetime_from_parts(1970, 1, 1, 0, 0, 0);
        assert_eq!(ts, 0.0);
    }

    #[test]
    fn add_days_result() {
        let ts = olive_datetime_from_parts(2024, 1, 15, 0, 0, 0);
        let next = olive_datetime_add_days(ts, 1);
        let parts_ptr = olive_datetime_parts(next);
        let obj = unsafe { &*(parts_ptr as *const OliveObj) };
        assert_eq!(*obj.fields.get("day").unwrap(), 16);
    }

    #[test]
    fn add_months_clamps_day() {
        let ts = olive_datetime_from_parts(2024, 1, 31, 0, 0, 0);
        let next = olive_datetime_add_months(ts, 1);
        let parts_ptr = olive_datetime_parts(next);
        let obj = unsafe { &*(parts_ptr as *const OliveObj) };
        assert_eq!(*obj.fields.get("month").unwrap(), 2);
        assert_eq!(*obj.fields.get("day").unwrap(), 29); // 2024 is a leap year
    }

    #[test]
    fn weekday_known_date() {
        // 2024-01-15 is a Monday (0)
        let ts = olive_datetime_from_parts(2024, 1, 15, 0, 0, 0);
        assert_eq!(olive_datetime_weekday(ts), 0);
    }

    #[test]
    fn format_basic() {
        let ts = 1705319445.0;
        let result = from_ptr(olive_datetime_format(ts, s("%Y-%m-%d")));
        assert_eq!(result, "2024-01-15");
    }

    #[test]
    fn format_default_iso() {
        let ts = 1705319445.0;
        let result = from_ptr(olive_datetime_format(ts, 0));
        assert_eq!(result, "2024-01-15T11:50:45");
    }

    #[test]
    fn leap_year_check() {
        assert_eq!(olive_datetime_is_leap_year(2024), 1);
        assert_eq!(olive_datetime_is_leap_year(2023), 0);
        assert_eq!(olive_datetime_is_leap_year(2000), 1);
        assert_eq!(olive_datetime_is_leap_year(1900), 0);
    }

    #[test]
    fn days_in_month_feb_leap() {
        assert_eq!(olive_datetime_days_in_month(2024, 2), 29);
        assert_eq!(olive_datetime_days_in_month(2023, 2), 28);
    }

    #[test]
    fn start_end_of_day() {
        let ts = 1705319445.0; // 2024-01-15T11:50:45
        let sod = olive_datetime_start_of_day(ts);
        let eod = olive_datetime_end_of_day(ts);
        let sod_parts = unsafe { &*(olive_datetime_parts(sod) as *const OliveObj) };
        let eod_parts = unsafe { &*(olive_datetime_parts(eod) as *const OliveObj) };
        assert_eq!(*sod_parts.fields.get("hour").unwrap(), 0);
        assert_eq!(*eod_parts.fields.get("hour").unwrap(), 23);
        assert_eq!(*eod_parts.fields.get("second").unwrap(), 59);
    }

    #[test]
    fn diff_days() {
        let a = olive_datetime_from_parts(2024, 1, 20, 0, 0, 0);
        let b = olive_datetime_from_parts(2024, 1, 15, 0, 0, 0);
        assert_eq!(olive_datetime_diff_days(a, b), 5);
    }
}
