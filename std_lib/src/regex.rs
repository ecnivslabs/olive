use crate::{olive_str_from_ptr, olive_str_internal};
use regex::Regex;
use rustc_hash::FxHashMap as HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Bounded cache of compiled patterns behind the free functions
/// (`regex.match_`, `regex.find`, ...), so a hot loop calling one of them
/// repeatedly with the same pattern string compiles it once instead of on
/// every call. Capped by entry count with LRU eviction, not by memory size,
/// since a `Regex` is small and the point is to bound compile churn.
const CACHE_CAP: usize = 256;

struct PatternCache {
    map: HashMap<String, Regex>,
    order: VecDeque<String>,
}

impl PatternCache {
    fn new() -> Self {
        PatternCache {
            map: HashMap::default(),
            order: VecDeque::new(),
        }
    }

    fn get_or_compile(&mut self, pattern: &str) -> Option<Regex> {
        if let Some(re) = self.map.get(pattern) {
            return Some(re.clone());
        }
        let re = Regex::new(pattern).ok()?;
        if self.map.len() >= CACHE_CAP
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
        self.map.insert(pattern.to_string(), re.clone());
        self.order.push_back(pattern.to_string());
        Some(re)
    }
}

fn cache() -> &'static Mutex<PatternCache> {
    static CACHE: OnceLock<Mutex<PatternCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(PatternCache::new()))
}

fn compiled(pattern: &str) -> Option<Regex> {
    cache().lock().unwrap().get_or_compile(pattern)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_match(pattern: i64, text: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match compiled(&pat) {
        Some(re) => {
            if re.is_match(&txt) {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_find(pattern: i64, text: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match compiled(&pat) {
        Some(re) => match re.find(&txt) {
            Some(m) => olive_str_internal(m.as_str()),
            None => 0,
        },
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_find_all(pattern: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if pattern == 0 || text == 0 {
        return empty_list();
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match compiled(&pat) {
        Some(re) => {
            let matches: Vec<i64> = re
                .find_iter(&txt)
                .map(|m| olive_str_internal(m.as_str()))
                .collect();
            crate::list::list_from_vec(matches)
        }
        None => empty_list(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_replace(pattern: i64, text: i64, rep: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return text;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    let replacement = if rep == 0 {
        String::new()
    } else {
        olive_str_from_ptr(rep)
    };
    match compiled(&pat) {
        Some(re) => olive_str_internal(&re.replacen(&txt, 1, replacement.as_str())),
        None => text,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_replace_all(pattern: i64, text: i64, rep: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return text;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    let replacement = if rep == 0 {
        String::new()
    } else {
        olive_str_from_ptr(rep)
    };
    match compiled(&pat) {
        Some(re) => olive_str_internal(&re.replace_all(&txt, replacement.as_str())),
        None => text,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_captures(pattern: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if pattern == 0 || text == 0 {
        return empty_list();
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match compiled(&pat) {
        Some(re) => match re.captures(&txt) {
            Some(caps) => {
                let groups: Vec<i64> = caps
                    .iter()
                    .map(|m| match m {
                        Some(m) => olive_str_internal(m.as_str()),
                        None => 0,
                    })
                    .collect();
                crate::list::list_from_vec(groups)
            }
            None => empty_list(),
        },
        None => empty_list(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_split(pattern: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if pattern == 0 {
        return empty_list();
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = if text == 0 {
        String::new()
    } else {
        olive_str_from_ptr(text)
    };
    match compiled(&pat) {
        Some(re) => {
            let parts: Vec<i64> = re.split(&txt).map(olive_str_internal).collect();
            crate::list::list_from_vec(parts)
        }
        None => empty_list(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_is_valid(pattern: i64) -> i64 {
    if pattern == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    if Regex::new(&pat).is_ok() { 1 } else { 0 }
}

// --- Compiled Pattern handles -------------------------------------------
//
// A `Pattern` handle owns its own compiled `Regex` outside the shared
// cache, so a caller holding one is never evicted out from under it and
// pays the compile cost exactly once regardless of cache pressure from
// unrelated patterns elsewhere in the program.

fn pattern_table() -> &'static Mutex<HashMap<i64, Regex>> {
    static TABLE: OnceLock<Mutex<HashMap<i64, Regex>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::default()))
}

fn next_pattern_handle() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_compile(pattern: i64) -> i64 {
    if pattern == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    match Regex::new(&pat) {
        Ok(re) => {
            let handle = next_pattern_handle();
            pattern_table().lock().unwrap().insert(handle, re);
            handle
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_close(handle: i64) {
    pattern_table().lock().unwrap().remove(&handle);
}

fn with_pattern<T>(handle: i64, f: impl FnOnce(&Regex) -> T) -> Option<T> {
    let table = pattern_table().lock().unwrap();
    table.get(&handle).map(f)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_matches(handle: i64, text: i64) -> i64 {
    if text == 0 {
        return 0;
    }
    let txt = olive_str_from_ptr(text);
    with_pattern(handle, |re| re.is_match(&txt)).unwrap_or(false) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_find(handle: i64, text: i64) -> i64 {
    if text == 0 {
        return 0;
    }
    let txt = olive_str_from_ptr(text);
    with_pattern(handle, |re| match re.find(&txt) {
        Some(m) => olive_str_internal(m.as_str()),
        None => 0,
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_find_all(handle: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if text == 0 {
        return empty_list();
    }
    let txt = olive_str_from_ptr(text);
    with_pattern(handle, |re| {
        let matches: Vec<i64> = re
            .find_iter(&txt)
            .map(|m| olive_str_internal(m.as_str()))
            .collect();
        crate::list::list_from_vec(matches)
    })
    .unwrap_or_else(empty_list)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_captures(handle: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if text == 0 {
        return empty_list();
    }
    let txt = olive_str_from_ptr(text);
    with_pattern(handle, |re| match re.captures(&txt) {
        Some(caps) => {
            let groups: Vec<i64> = caps
                .iter()
                .map(|m| match m {
                    Some(m) => olive_str_internal(m.as_str()),
                    None => 0,
                })
                .collect();
            crate::list::list_from_vec(groups)
        }
        None => empty_list(),
    })
    .unwrap_or_else(empty_list)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_captures_all(handle: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    if text == 0 {
        return empty_list();
    }
    let txt = olive_str_from_ptr(text);
    with_pattern(handle, |re| {
        let rows: Vec<i64> = re
            .captures_iter(&txt)
            .map(|caps| {
                let groups: Vec<i64> = caps
                    .iter()
                    .map(|m| match m {
                        Some(m) => olive_str_internal(m.as_str()),
                        None => 0,
                    })
                    .collect();
                crate::list::list_from_vec(groups)
            })
            .collect();
        crate::list::list_from_vec(rows)
    })
    .unwrap_or_else(empty_list)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_capture_named(handle: i64, text: i64, name: i64) -> i64 {
    if text == 0 || name == 0 {
        return 0;
    }
    let txt = olive_str_from_ptr(text);
    let group_name = olive_str_from_ptr(name);
    with_pattern(handle, |re| match re.captures(&txt) {
        Some(caps) => match caps.name(&group_name) {
            Some(m) => olive_str_internal(m.as_str()),
            None => 0,
        },
        None => 0,
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_replace(handle: i64, text: i64, rep: i64) -> i64 {
    if text == 0 {
        return text;
    }
    let txt = olive_str_from_ptr(text);
    let replacement = if rep == 0 {
        String::new()
    } else {
        olive_str_from_ptr(rep)
    };
    with_pattern(handle, |re| {
        olive_str_internal(&re.replacen(&txt, 1, replacement.as_str()))
    })
    .unwrap_or(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_replace_all(handle: i64, text: i64, rep: i64) -> i64 {
    if text == 0 {
        return text;
    }
    let txt = olive_str_from_ptr(text);
    let replacement = if rep == 0 {
        String::new()
    } else {
        olive_str_from_ptr(rep)
    };
    with_pattern(handle, |re| {
        olive_str_internal(&re.replace_all(&txt, replacement.as_str()))
    })
    .unwrap_or(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_pattern_split(handle: i64, text: i64) -> i64 {
    let empty_list = || crate::list::list_from_vec(Vec::new());
    let txt = if text == 0 {
        String::new()
    } else {
        olive_str_from_ptr(text)
    };
    with_pattern(handle, |re| {
        let parts: Vec<i64> = re.split(&txt).map(olive_str_internal).collect();
        crate::list::list_from_vec(parts)
    })
    .unwrap_or_else(empty_list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StableVec;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn regex_match_basic() {
        assert_eq!(olive_regex_match(s(r"\d+"), s("abc123")), 1);
        assert_eq!(olive_regex_match(s(r"\d+"), s("abc")), 0);
    }

    #[test]
    fn regex_find_first() {
        let result = olive_regex_find(s(r"\d+"), s("abc123def456"));
        assert_eq!(from_ptr(result), "123");
    }

    #[test]
    fn regex_find_all_results() {
        let list = olive_regex_find_all(s(r"\d+"), s("abc123def456ghi789"));
        let sv = unsafe { &*(list as *const StableVec) };
        assert_eq!(sv.len, 3);
        assert_eq!(from_ptr(unsafe { *sv.ptr }), "123");
        assert_eq!(from_ptr(unsafe { *sv.ptr.add(1) }), "456");
        assert_eq!(from_ptr(unsafe { *sv.ptr.add(2) }), "789");
    }

    #[test]
    fn regex_replace_one() {
        let result = olive_regex_replace(s(r"\d+"), s("abc123def456"), s("NUM"));
        assert_eq!(from_ptr(result), "abcNUMdef456");
    }

    #[test]
    fn regex_replace_all_results() {
        let result = olive_regex_replace_all(s(r"\d+"), s("abc123def456"), s("NUM"));
        assert_eq!(from_ptr(result), "abcNUMdefNUM");
    }

    #[test]
    fn regex_captures_groups() {
        let list = olive_regex_captures(s(r"(\d{4})-(\d{2})-(\d{2})"), s("date: 2024-01-15"));
        let sv = unsafe { &*(list as *const StableVec) };
        assert_eq!(sv.len, 4);
        assert_eq!(from_ptr(unsafe { *sv.ptr }), "2024-01-15");
        assert_eq!(from_ptr(unsafe { *sv.ptr.add(1) }), "2024");
        assert_eq!(from_ptr(unsafe { *sv.ptr.add(2) }), "01");
        assert_eq!(from_ptr(unsafe { *sv.ptr.add(3) }), "15");
    }

    #[test]
    fn regex_split_result() {
        let list = olive_regex_split(s(r"\s+"), s("hello   world  foo"));
        let sv = unsafe { &*(list as *const StableVec) };
        assert_eq!(sv.len, 3);
        assert_eq!(from_ptr(unsafe { *sv.ptr }), "hello");
    }

    #[test]
    fn regex_invalid_pattern() {
        assert_eq!(olive_regex_match(s("[invalid"), s("test")), 0);
        assert_eq!(olive_regex_is_valid(s("[invalid")), 0);
        assert_eq!(olive_regex_is_valid(s(r"\d+")), 1);
    }

    #[test]
    fn regex_null_inputs() {
        assert_eq!(olive_regex_match(0, s("test")), 0);
        assert_eq!(olive_regex_find(0, s("test")), 0);
    }

    #[test]
    fn pattern_compile_and_reuse() {
        let h = olive_regex_compile(s(r"\d+"));
        assert_ne!(h, 0);
        assert_eq!(olive_regex_pattern_matches(h, s("abc123")), 1);
        assert_eq!(olive_regex_pattern_matches(h, s("abc")), 0);
        assert_eq!(from_ptr(olive_regex_pattern_find(h, s("x42y"))), "42");
        olive_regex_pattern_close(h);
    }

    #[test]
    fn pattern_compile_invalid_returns_zero() {
        assert_eq!(olive_regex_compile(s("[invalid")), 0);
    }

    #[test]
    fn pattern_named_group() {
        let h = olive_regex_compile(s(r"(?P<year>\d{4})-(?P<month>\d{2})"));
        let out = olive_regex_pattern_capture_named(h, s("2024-01"), s("year"));
        assert_eq!(from_ptr(out), "2024");
        let missing = olive_regex_pattern_capture_named(h, s("2024-01"), s("nope"));
        assert_eq!(missing, 0);
        olive_regex_pattern_close(h);
    }

    #[test]
    fn pattern_captures_all() {
        let h = olive_regex_compile(s(r"(\w)=(\d)"));
        let list = olive_regex_pattern_captures_all(h, s("a=1 b=2 c=3"));
        let sv = unsafe { &*(list as *const StableVec) };
        assert_eq!(sv.len, 3);
        olive_regex_pattern_close(h);
    }

    #[test]
    fn unknown_handle_is_safe() {
        assert_eq!(olive_regex_pattern_matches(999999, s("x")), 0);
        assert_eq!(olive_regex_pattern_find(999999, s("x")), 0);
        olive_regex_pattern_close(999999);
    }
}
