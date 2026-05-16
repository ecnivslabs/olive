use crate::{olive_str_from_ptr, olive_str_internal};
use regex::Regex;

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_match(pattern: i64, text: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match Regex::new(&pat) {
        Ok(re) => {
            if re.is_match(&txt) {
                1
            } else {
                0
            }
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_regex_find(pattern: i64, text: i64) -> i64 {
    if pattern == 0 || text == 0 {
        return 0;
    }
    let pat = olive_str_from_ptr(pattern);
    let txt = olive_str_from_ptr(text);
    match Regex::new(&pat) {
        Ok(re) => match re.find(&txt) {
            Some(m) => olive_str_internal(m.as_str()),
            None => 0,
        },
        Err(_) => 0,
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
    match Regex::new(&pat) {
        Ok(re) => {
            let matches: Vec<i64> = re
                .find_iter(&txt)
                .map(|m| olive_str_internal(m.as_str()))
                .collect();
            crate::list::list_from_vec(matches)
        }
        Err(_) => empty_list(),
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
    match Regex::new(&pat) {
        Ok(re) => olive_str_internal(&re.replacen(&txt, 1, replacement.as_str())),
        Err(_) => text,
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
    match Regex::new(&pat) {
        Ok(re) => olive_str_internal(&re.replace_all(&txt, replacement.as_str())),
        Err(_) => text,
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
    match Regex::new(&pat) {
        Ok(re) => match re.captures(&txt) {
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
        Err(_) => empty_list(),
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
    match Regex::new(&pat) {
        Ok(re) => {
            let parts: Vec<i64> = re.split(&txt).map(olive_str_internal).collect();
            crate::list::list_from_vec(parts)
        }
        Err(_) => empty_list(),
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
}
