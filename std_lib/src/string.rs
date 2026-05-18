use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_len(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    unsafe {
        std::ffi::CStr::from_ptr((s & !1) as *const std::ffi::c_char)
            .to_bytes()
            .len() as i64
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let ptr = (s & !1) as *const u8;
    let byte = unsafe { *ptr.add(i as usize) };
    if byte == 0 {
        return 0;
    }
    let buf = [byte, 0u8];
    let c_str = unsafe { std::ffi::CString::from_vec_unchecked(buf.to_vec()) };
    c_str.into_raw() as i64 | 1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char(s: i64, i: i64) -> i64 {
    olive_str_get(s, i)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_slice(s: i64, start: i64, end: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let start = start as usize;
    let end = end as usize;
    if start <= end && end <= text.len() {
        olive_str_internal(&text[start..end])
    } else {
        0
    }
}

pub fn olive_str_internal(s: &str) -> i64 {
    let c_str = std::ffi::CString::new(s).unwrap_or_else(|_| {
        let safe: String = s.chars().filter(|&c| c != '\0').collect();
        std::ffi::CString::new(safe).unwrap()
    });
    c_str.into_raw() as i64 | 1
}

pub fn olive_str_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let p = ptr & !1;
    unsafe {
        std::ffi::CStr::from_ptr(p as *const std::ffi::c_char)
            .to_string_lossy()
            .into_owned()
    }
}

pub fn olive_str_as_str<'a>(ptr: i64) -> Option<&'a str> {
    if ptr == 0 {
        return None;
    }
    let p = ptr & !1;
    unsafe {
        let c_str = std::ffi::CStr::from_ptr(p as *const std::ffi::c_char);
        c_str.to_str().ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_start(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_start())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_end(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_end())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_upper(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_uppercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_lower(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_lowercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_replace(s: i64, from: i64, to: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let from_str = olive_str_from_ptr(from);
    let to_str = olive_str_from_ptr(to);
    olive_str_internal(&text.replace(&from_str, &to_str))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_find(s: i64, needle: i64) -> i64 {
    if s == 0 || needle == 0 {
        return -1;
    }
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(needle);
    match text.find(&pat) {
        Some(i) => i as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_contains(s: i64, needle: i64) -> i64 {
    if s == 0 || needle == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).contains(&olive_str_from_ptr(needle)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_starts_with(s: i64, prefix: i64) -> i64 {
    if s == 0 || prefix == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).starts_with(&olive_str_from_ptr(prefix)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_ends_with(s: i64, suffix: i64) -> i64 {
    if s == 0 || suffix == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).ends_with(&olive_str_from_ptr(suffix)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_repeat(s: i64, n: i64) -> i64 {
    if s == 0 || n <= 0 {
        return olive_str_internal("");
    }
    olive_str_internal(&olive_str_from_ptr(s).repeat(n as usize))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_split(s: i64, sep: i64) -> i64 {
    let text = if s == 0 {
        String::new()
    } else {
        olive_str_from_ptr(s)
    };
    let parts: Vec<i64> = if sep == 0 {
        text.split_whitespace().map(olive_str_internal).collect()
    } else {
        let sep_str = olive_str_from_ptr(sep);
        text.split(&sep_str).map(olive_str_internal).collect()
    };
    let mut v = parts;
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_join(list_ptr: i64, sep: i64) -> i64 {
    if list_ptr == 0 {
        return olive_str_internal("");
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    let sep_str = if sep == 0 {
        String::new()
    } else {
        olive_str_from_ptr(sep)
    };
    let parts: Vec<String> = (0..s.len)
        .map(|i| olive_str_from_ptr(unsafe { *s.ptr.add(i) }))
        .collect();
    olive_str_internal(&parts.join(&sep_str))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_fmt(template: i64, args: i64) -> i64 {
    if template == 0 {
        return olive_str_internal("");
    }
    let tmpl = olive_str_from_ptr(template);
    let arg_strs: Vec<String> = if args == 0 {
        vec![]
    } else {
        let sv = unsafe { &*(args as *const StableVec) };
        (0..sv.len)
            .map(|i| olive_str_from_ptr(unsafe { *sv.ptr.add(i) }))
            .collect()
    };
    let mut result = String::with_capacity(tmpl.len());
    let mut arg_idx = 0;
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if arg_idx < arg_strs.len() {
                result.push_str(&arg_strs[arg_idx]);
                arg_idx += 1;
            }
        } else {
            result.push(c);
        }
    }
    olive_str_internal(&result)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char_count(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).chars().count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_is_ascii(s: i64) -> i64 {
    if s == 0 {
        return 1;
    }
    if olive_str_from_ptr(s).is_ascii() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_grapheme_count(s: i64) -> i64 {
    use unicode_segmentation::UnicodeSegmentation;
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).graphemes(true).count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_graphemes(s: i64) -> i64 {
    use unicode_segmentation::UnicodeSegmentation;
    if s == 0 {
        let v = Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        });
        return Box::into_raw(v) as i64;
    }
    let text = olive_str_from_ptr(s);
    let mut ptrs: Vec<i64> = text.graphemes(true).map(olive_str_internal).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}
