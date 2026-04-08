use crate::olive_str_from_ptr;
use rustc_hash::FxHashMap as HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const LEVEL_DEBUG: i64 = 0;
const LEVEL_INFO: i64 = 1;
const LEVEL_WARN: i64 = 2;
const LEVEL_ERROR: i64 = 3;

const FMT_JSON: i64 = 1;
const FMT_COLOR: i64 = 2;

struct LogState {
    level: i64,
    format: i64,
    fields: HashMap<String, String>,
}

static LOG_STATE: OnceLock<Mutex<LogState>> = OnceLock::new();

fn state() -> &'static Mutex<LogState> {
    LOG_STATE.get_or_init(|| {
        Mutex::new(LogState {
            level: LEVEL_INFO,
            format: FMT_COLOR,
            fields: HashMap::default(),
        })
    })
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn level_name(level: i64) -> &'static str {
    match level {
        LEVEL_DEBUG => "DEBUG",
        LEVEL_INFO => "INFO",
        LEVEL_WARN => "WARN",
        LEVEL_ERROR => "ERROR",
        _ => "INFO",
    }
}

fn level_color(level: i64) -> &'static str {
    match level {
        LEVEL_DEBUG => "\x1b[36m",
        LEVEL_INFO => "\x1b[32m",
        LEVEL_WARN => "\x1b[33m",
        LEVEL_ERROR => "\x1b[31m",
        _ => "\x1b[0m",
    }
}

fn emit(level: i64, msg: &str) {
    let st = state().lock().unwrap();
    if level < st.level {
        return;
    }
    let ts = now_ts();
    match st.format {
        FMT_JSON => {
            let mut map = serde_json::Map::new();
            map.insert("ts".to_string(), serde_json::json!(ts));
            map.insert("level".to_string(), serde_json::json!(level_name(level)));
            map.insert("msg".to_string(), serde_json::json!(msg));
            for (k, v) in &st.fields {
                map.insert(k.clone(), serde_json::json!(v));
            }
            eprintln!("{}", serde_json::Value::Object(map));
        }
        FMT_COLOR => {
            let color = level_color(level);
            let reset = "\x1b[0m";
            let lname = level_name(level);
            if st.fields.is_empty() {
                eprintln!("[{color}{lname}{reset}] {msg}");
            } else {
                let fields: String = st.fields.iter().map(|(k, v)| format!(" {k}={v}")).collect();
                eprintln!("[{color}{lname}{reset}] {msg}{fields}");
            }
        }
        _ => {
            let lname = level_name(level);
            if st.fields.is_empty() {
                eprintln!("[{lname}] {msg}");
            } else {
                let fields: String = st.fields.iter().map(|(k, v)| format!(" {k}={v}")).collect();
                eprintln!("[{lname}] {msg}{fields}");
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_set_level(level: i64) {
    state().lock().unwrap().level = level;
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_set_format(fmt: i64) {
    state().lock().unwrap().format = fmt;
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_debug(msg: i64) {
    if msg == 0 {
        return;
    }
    emit(LEVEL_DEBUG, &olive_str_from_ptr(msg));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_info(msg: i64) {
    if msg == 0 {
        return;
    }
    emit(LEVEL_INFO, &olive_str_from_ptr(msg));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_warn(msg: i64) {
    if msg == 0 {
        return;
    }
    emit(LEVEL_WARN, &olive_str_from_ptr(msg));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_error(msg: i64) {
    if msg == 0 {
        return;
    }
    emit(LEVEL_ERROR, &olive_str_from_ptr(msg));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_with_field(key: i64, val: i64) {
    if key == 0 {
        return;
    }
    let k = olive_str_from_ptr(key);
    let v = if val == 0 {
        String::new()
    } else {
        olive_str_from_ptr(val)
    };
    state().lock().unwrap().fields.insert(k, v);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_clear_fields() {
    state().lock().unwrap().fields.clear();
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_log_level_from_str(s: i64) -> i64 {
    if s == 0 {
        return LEVEL_INFO;
    }
    match olive_str_from_ptr(s).to_uppercase().as_str() {
        "DEBUG" => LEVEL_DEBUG,
        "INFO" => LEVEL_INFO,
        "WARN" | "WARNING" => LEVEL_WARN,
        "ERROR" => LEVEL_ERROR,
        _ => LEVEL_INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_from_str_mapping() {
        let debug = crate::olive_str_internal("debug");
        let warn = crate::olive_str_internal("WARN");
        assert_eq!(olive_log_level_from_str(debug), LEVEL_DEBUG);
        assert_eq!(olive_log_level_from_str(warn), LEVEL_WARN);
        assert_eq!(olive_log_level_from_str(0), LEVEL_INFO);
    }

    #[test]
    fn set_level_filters_lower() {
        olive_log_set_level(LEVEL_ERROR);
        let msg = crate::olive_str_internal("should be filtered");
        olive_log_info(msg);
        olive_log_set_level(LEVEL_INFO);
    }

    #[test]
    fn fields_set_clear() {
        let key = crate::olive_str_internal("request_id");
        let val = crate::olive_str_internal("abc123");
        olive_log_with_field(key, val);
        olive_log_clear_fields();
        let st = state().lock().unwrap();
        assert!(st.fields.is_empty());
    }
}
