use crate::string_slab::{str_body, str_is_heap};
use crate::*;

/// Converts an Olive string pointer (tagged or untagged) to a byte slice.
/// O(1) for slab-backed strings; O(N) fallback via CStr for literals.
pub fn olive_str_to_bytes<'a>(ptr: i64) -> &'a [u8] {
    olive_str_to_bytes_with(ptr, None)
}

/// Same as `olive_str_to_bytes`, but takes the caller's already-known
/// heap-vs-literal answer when it has one.
pub fn olive_str_to_bytes_with<'a>(ptr: i64, known_heap: Option<bool>) -> &'a [u8] {
    if ptr == 0 {
        return b"";
    }
    let p = str_body(ptr);
    let is_heap = known_heap.unwrap_or_else(|| str_is_heap(ptr));
    if is_heap {
        let header_val = unsafe { *(p as *const usize).sub(2) };
        let len = header_val & 0xFFFFFFFFFFFF;
        unsafe { std::slice::from_raw_parts(p as *const u8, len) }
    } else {
        unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char).to_bytes() }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_len(s: i64) -> i64 {
    olive_str_to_bytes(s).len() as i64
}

/// Interned single-byte strings, NUL-terminated like any literal. Indexing
/// and per-char iteration return these instead of allocating, and the free
/// path already ignores pointers outside the slab span.
#[repr(C, align(4))]
pub struct CharTable([[u8; 4]; 256]);

// 4-byte stride from a 4-aligned base keeps bit0 and bit1 clear on every
// entry, so an interned char pointer never reads as a tagged heap string.
// Exported because codegen indexes it directly to inline `s[i]`; the stride
// is part of that contract, so changing it means changing the emitted shift.
#[unsafe(export_name = "olive_char_table")]
pub static CHAR_STRS: CharTable = {
    let mut t = [[0u8; 4]; 256];
    let mut i = 0;
    while i < 256 {
        t[i][0] = i as u8;
        i += 1;
    }
    CharTable(t)
};

pub(crate) fn char_str(byte: u8) -> i64 {
    CHAR_STRS.0[byte as usize].as_ptr() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let ptr = str_body(s) as *const u8;
    let byte = unsafe { *ptr.add(i as usize) };
    if byte == 0 {
        return 0;
    }
    char_str(byte)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char(s: i64, i: i64) -> i64 {
    olive_str_get(s, i)
}

/// Bounds-checked single-character index. Length comes from the slab header
/// (or strlen for literals), so the read is O(1); panics with the source
/// location on a null receiver or an out-of-range index.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get_checked(s: i64, i: i64, loc: i64) -> i64 {
    if s == 0 {
        crate::panic::olive_nil_index_fail(loc);
    }
    let len = olive_str_len(s);
    let idx = if i < 0 { i + len } else { i };
    if idx < 0 || idx >= len {
        crate::panic::olive_bounds_fail(i, len, loc);
    }
    let ptr = str_body(s) as *const u8;
    let byte = unsafe { *ptr.add(idx as usize) };
    char_str(byte)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_getslice(s: i64, start: i64, stop: i64, step: i64, flags: i64) -> i64 {
    if s == 0 {
        return olive_str_internal("");
    }
    let chars: Vec<char> = olive_str_from_ptr(s).chars().collect();
    let idxs = crate::list::slice_indices(chars.len() as i64, start, stop, step, flags);
    let out: String = idxs.iter().map(|&i| chars[i]).collect();
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_slice(s: i64, start: i64, end: i64) -> i64 {
    let bytes = olive_str_to_bytes(s);
    let start = start as usize;
    let end = end as usize;
    if start <= end && end <= bytes.len() {
        olive_str_internal(unsafe { std::str::from_utf8_unchecked(&bytes[start..end]) })
    } else {
        0
    }
}

/// Creates a heap-allocated Olive string from a `&str`, returning an `i64` pointer.
///
/// # Examples
///
/// ```
/// use olive_std::olive_str_internal;
/// let ptr = olive_str_internal("hello");
/// assert!(ptr != 0);
/// ```
pub fn olive_str_internal(s: &str) -> i64 {
    // The terminator marks the end; an interior nul would make the free path
    // strlen short and pick the wrong size class, so drop them here.
    if s.as_bytes().contains(&0) {
        let safe: Vec<u8> = s.bytes().filter(|&b| b != 0).collect();
        return crate::string_slab::str_alloc(&safe);
    }
    crate::string_slab::str_alloc(s.as_bytes())
}

/// Converts an Olive string pointer back into an owned `String`.
///
/// # Examples
///
/// ```
/// use olive_std::{olive_str_internal, olive_str_from_ptr};
/// let ptr = olive_str_internal("hello");
/// assert_eq!(olive_str_from_ptr(ptr), "hello");
/// ```
pub fn olive_str_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    String::from_utf8_lossy(olive_str_to_bytes(ptr)).into_owned()
}

/// Returns an optional `&str` referencing the string pointed to by `ptr`.
///
/// # Examples
///
/// ```
/// use olive_std::{olive_str_internal, olive_str_as_str};
/// let ptr = olive_str_internal("hello");
/// assert_eq!(olive_str_as_str(ptr), Some("hello"));
/// ```
#[cfg(test)]
mod get_checked_tests {
    use super::*;

    #[test]
    fn in_bounds_returns_char() {
        let s = olive_str_internal("abc");
        let got = olive_str_get_checked(s, 1, 0);
        assert_eq!(olive_str_from_ptr(got), "b");
    }

    #[test]
    fn first_and_last_chars() {
        let s = olive_str_internal("xyz");
        assert_eq!(olive_str_from_ptr(olive_str_get_checked(s, 0, 0)), "x");
        assert_eq!(olive_str_from_ptr(olive_str_get_checked(s, 2, 0)), "z");
    }
}

pub fn olive_str_as_str<'a>(ptr: i64) -> Option<&'a str> {
    if ptr == 0 {
        return None;
    }
    std::str::from_utf8(olive_str_to_bytes(ptr)).ok()
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
    let v = parts;
    crate::list::list_from_vec(v)
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

/// Non-overlapping occurrences of `sub` in `s`, Python's `str.count` semantics
/// (an empty `sub` counts every gap, `len(s) + 1` positions).
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_count(s: i64, sub: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(sub);
    text.matches(&pat).count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_rfind(s: i64, needle: i64) -> i64 {
    if s == 0 || needle == 0 {
        return -1;
    }
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(needle);
    match text.rfind(&pat) {
        Some(i) => i as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_splitlines(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let parts: Vec<i64> = text.lines().map(olive_str_internal).collect();
    crate::list::list_from_vec(parts)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_title(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let mut out = String::with_capacity(text.len());
    let mut prev_alpha = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_capitalize(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let mut chars = text.chars();
    let out = match chars.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(&chars.as_str().to_lowercase());
            out
        }
        None => String::new(),
    };
    olive_str_internal(&out)
}

/// Left-pads with `0` to `width` chars, preserving a leading `+`/`-` sign.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_zfill(s: i64, width: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let width = width.max(0) as usize;
    let (sign, rest) = if let Some(r) = text.strip_prefix('-') {
        ("-", r)
    } else if let Some(r) = text.strip_prefix('+') {
        ("+", r)
    } else {
        ("", text.as_str())
    };
    let total = sign.chars().count() + rest.chars().count();
    let out = if total >= width {
        text.clone()
    } else {
        format!("{sign}{}{rest}", "0".repeat(width - total))
    };
    olive_str_internal(&out)
}

fn fill_char(fill: i64) -> char {
    olive_str_from_ptr(fill).chars().next().unwrap_or(' ')
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_ljust(s: i64, width: i64, fill: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let len = text.chars().count() as i64;
    let out = if len >= width {
        text
    } else {
        let pad: String = std::iter::repeat_n(fill_char(fill), (width - len) as usize).collect();
        format!("{text}{pad}")
    };
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_rjust(s: i64, width: i64, fill: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let len = text.chars().count() as i64;
    let out = if len >= width {
        text
    } else {
        let pad: String = std::iter::repeat_n(fill_char(fill), (width - len) as usize).collect();
        format!("{pad}{text}")
    };
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_center(s: i64, width: i64, fill: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let len = text.chars().count() as i64;
    let out = if len >= width {
        text
    } else {
        let total_pad = (width - len) as usize;
        let left = total_pad / 2;
        let right = total_pad - left;
        let c = fill_char(fill);
        let left_pad: String = std::iter::repeat_n(c, left).collect();
        let right_pad: String = std::iter::repeat_n(c, right).collect();
        format!("{left_pad}{text}{right_pad}")
    };
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_removeprefix(s: i64, prefix: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let pre = olive_str_from_ptr(prefix);
    olive_str_internal(text.strip_prefix(&pre).unwrap_or(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_removesuffix(s: i64, suffix: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let suf = olive_str_from_ptr(suffix);
    olive_str_internal(text.strip_suffix(&suf).unwrap_or(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isdigit(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_numeric())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isalpha(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_alphabetic())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isspace(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_whitespace())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isupper(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let cased: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    (!cased.is_empty() && cased.iter().all(|c| c.is_uppercase())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_islower(s: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let cased: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    (!cased.is_empty() && cased.iter().all(|c| c.is_lowercase())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_chars(s: i64, chars: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let set: std::collections::HashSet<char> = olive_str_from_ptr(chars).chars().collect();
    olive_str_internal(text.trim_matches(|c| set.contains(&c)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_start_chars(s: i64, chars: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let set: std::collections::HashSet<char> = olive_str_from_ptr(chars).chars().collect();
    olive_str_internal(text.trim_start_matches(|c| set.contains(&c)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_end_chars(s: i64, chars: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let set: std::collections::HashSet<char> = olive_str_from_ptr(chars).chars().collect();
    olive_str_internal(text.trim_end_matches(|c| set.contains(&c)))
}

/// `s.partition(sep)`: `(before, sep, after)` on the first match, or
/// `(s, "", "")` when `sep` doesn't occur. A tuple shares a list's raw
/// layout (see `translate_aggregate`'s tuple/list fallthrough), so the
/// result is built the same way a list literal is.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_partition(s: i64, sep: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(sep);
    let (before, mid, after) = if !pat.is_empty()
        && let Some(idx) = text.find(&pat)
    {
        (
            text[..idx].to_string(),
            pat.clone(),
            text[idx + pat.len()..].to_string(),
        )
    } else {
        (text.clone(), String::new(), String::new())
    };
    let out = crate::list::olive_list_new(3);
    crate::list::olive_list_set(out, 0, olive_str_internal(&before));
    crate::list::olive_list_set(out, 1, olive_str_internal(&mid));
    crate::list::olive_list_set(out, 2, olive_str_internal(&after));
    out
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

/// Builds a list of the string's characters, each as a one-character string.
/// Backs `for c in s` iteration.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_chars(s: i64) -> i64 {
    if s == 0 {
        return crate::list::olive_list_new(0);
    }
    let chars: Vec<String> = olive_str_from_ptr(s)
        .chars()
        .map(|c| c.to_string())
        .collect();
    let list = crate::list::olive_list_new(chars.len() as i64);
    for (i, c) in chars.iter().enumerate() {
        crate::list::olive_list_set(list, i as i64, crate::olive_str_internal(c));
    }
    list
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
        return crate::list::list_from_vec(Vec::new());
    }
    let text = olive_str_from_ptr(s);
    let ptrs: Vec<i64> = text.graphemes(true).map(olive_str_internal).collect();
    crate::list::list_from_vec(ptrs)
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
    fn len_basic() {
        assert_eq!(olive_str_len(s("hello")), 5);
    }

    #[test]
    fn len_empty() {
        assert_eq!(olive_str_len(s("")), 0);
    }

    #[test]
    fn len_null() {
        assert_eq!(olive_str_len(0), 0);
    }

    #[test]
    fn slice_full() {
        let result = from_ptr(olive_str_slice(s("hello"), 0, 5));
        assert_eq!(result, "hello");
    }

    #[test]
    fn slice_partial() {
        let result = from_ptr(olive_str_slice(s("hello"), 1, 4));
        assert_eq!(result, "ell");
    }

    #[test]
    fn slice_empty_range() {
        let result = from_ptr(olive_str_slice(s("hello"), 2, 2));
        assert_eq!(result, "");
    }

    #[test]
    fn slice_invalid_range() {
        assert_eq!(olive_str_slice(s("hello"), 3, 1), 0);
        assert_eq!(olive_str_slice(s("hello"), 10, 15), 0);
    }

    #[test]
    fn trim_whitespace() {
        assert_eq!(from_ptr(olive_str_trim(s("  hello  "))), "hello");
    }

    #[test]
    fn trim_no_change() {
        assert_eq!(from_ptr(olive_str_trim(s("hello"))), "hello");
    }

    #[test]
    fn trim_empty() {
        assert_eq!(from_ptr(olive_str_trim(s(""))), "");
    }

    #[test]
    fn trim_start_only() {
        assert_eq!(from_ptr(olive_str_trim_start(s("  hello  "))), "hello  ");
    }

    #[test]
    fn trim_end_only() {
        assert_eq!(from_ptr(olive_str_trim_end(s("  hello  "))), "  hello");
    }

    #[test]
    fn upper_case() {
        assert_eq!(from_ptr(olive_str_upper(s("hello"))), "HELLO");
    }

    #[test]
    fn lower_case() {
        assert_eq!(from_ptr(olive_str_lower(s("HELLO"))), "hello");
    }

    #[test]
    fn replace_substring() {
        let result = from_ptr(olive_str_replace(s("hello world"), s("world"), s("there")));
        assert_eq!(result, "hello there");
    }

    #[test]
    fn replace_no_match() {
        let result = from_ptr(olive_str_replace(s("hello"), s("x"), s("y")));
        assert_eq!(result, "hello");
    }

    #[test]
    fn find_substring() {
        assert_eq!(olive_str_find(s("hello world"), s("world")), 6);
    }

    #[test]
    fn find_not_found() {
        assert_eq!(olive_str_find(s("hello"), s("x")), -1);
    }

    #[test]
    fn find_null_inputs() {
        assert_eq!(olive_str_find(0, s("x")), -1);
        assert_eq!(olive_str_find(s("x"), 0), -1);
    }

    #[test]
    fn contains_true() {
        assert_eq!(olive_str_contains(s("hello world"), s("world")), 1);
    }

    #[test]
    fn contains_false() {
        assert_eq!(olive_str_contains(s("hello"), s("x")), 0);
    }

    #[test]
    fn starts_with_true() {
        assert_eq!(olive_str_starts_with(s("hello"), s("he")), 1);
    }

    #[test]
    fn starts_with_false() {
        assert_eq!(olive_str_starts_with(s("hello"), s("el")), 0);
    }

    #[test]
    fn ends_with_true() {
        assert_eq!(olive_str_ends_with(s("hello"), s("lo")), 1);
    }

    #[test]
    fn ends_with_false() {
        assert_eq!(olive_str_ends_with(s("hello"), s("el")), 0);
    }

    #[test]
    fn repeat_basic() {
        assert_eq!(from_ptr(olive_str_repeat(s("ab"), 3)), "ababab");
    }

    #[test]
    fn repeat_zero() {
        assert_eq!(from_ptr(olive_str_repeat(s("ab"), 0)), "");
    }

    #[test]
    fn repeat_negative() {
        assert_eq!(from_ptr(olive_str_repeat(s("ab"), -1)), "");
    }

    #[test]
    fn split_by_space() {
        let list_ptr = olive_str_split(s("a b c"), 0);
        assert_ne!(list_ptr, 0);
        let s = unsafe { &*(list_ptr as *const StableVec) };
        assert_eq!(s.len, 3);
        assert_eq!(crate::olive_str_from_ptr(unsafe { *s.ptr }), "a");
        assert_eq!(crate::olive_str_from_ptr(unsafe { *s.ptr.add(1) }), "b");
        assert_eq!(crate::olive_str_from_ptr(unsafe { *s.ptr.add(2) }), "c");
    }

    #[test]
    fn split_by_comma() {
        let sep = olive_str_internal(",");
        let list_ptr = olive_str_split(s("x,y,z"), sep);
        assert_ne!(list_ptr, 0);
        let s = unsafe { &*(list_ptr as *const StableVec) };
        assert_eq!(s.len, 3);
    }

    #[test]
    fn join_basic() {
        let list_ptr = crate::olive_list_new(3);
        crate::olive_list_set(list_ptr, 0, s("a"));
        crate::olive_list_set(list_ptr, 1, s("b"));
        crate::olive_list_set(list_ptr, 2, s("c"));
        let result = from_ptr(olive_str_join(list_ptr, s(",")));
        assert_eq!(result, "a,b,c");
    }

    #[test]
    fn join_empty_list() {
        assert_eq!(from_ptr(olive_str_join(0, s(","))), "");
    }

    #[test]
    fn char_count_ascii() {
        assert_eq!(olive_str_char_count(s("hello")), 5);
    }

    #[test]
    fn char_count_unicode() {
        assert_eq!(olive_str_char_count(s("héllo")), 5);
    }

    #[test]
    fn char_count_empty() {
        assert_eq!(olive_str_char_count(s("")), 0);
    }

    #[test]
    fn is_ascii_true() {
        assert_eq!(olive_str_is_ascii(s("hello")), 1);
    }

    #[test]
    fn is_ascii_false() {
        assert_eq!(olive_str_is_ascii(s("héllo")), 0);
    }

    #[test]
    fn fmt_basic() {
        let template = s("Hello, {}!");
        let args_list = crate::olive_list_new(1);
        crate::olive_list_set(args_list, 0, s("world"));
        assert_eq!(
            from_ptr(olive_str_fmt(template, args_list)),
            "Hello, world!"
        );
    }

    #[test]
    fn fmt_multiple_args() {
        let template = s("{} + {} = {}");
        let args_list = crate::olive_list_new(3);
        crate::olive_list_set(args_list, 0, s("1"));
        crate::olive_list_set(args_list, 1, s("2"));
        crate::olive_list_set(args_list, 2, s("3"));
        assert_eq!(from_ptr(olive_str_fmt(template, args_list)), "1 + 2 = 3");
    }

    #[test]
    fn fmt_no_placeholders() {
        assert_eq!(from_ptr(olive_str_fmt(s("hello"), 0)), "hello");
    }
}
