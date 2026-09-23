use crate::string_slab::{str_body, str_is_heap, str_is_literal};
use crate::*;

/// Converts an Olive string pointer (tagged or untagged) to a byte slice.
/// Heap and length-bearing literal strings are O(1); foreign C strings use a
/// bounded NUL-terminated fallback.
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
    if is_interned_char(ptr) {
        let base = CHAR_STRS.0.as_ptr() as i64;
        let index = (p - base) as usize;
        if index < 256 {
            return &CHAR_STRS.0[index][..1];
        }
    }
    let is_heap = known_heap.unwrap_or_else(|| str_is_heap(ptr));
    if is_heap {
        let header_val = unsafe { *(p as *const usize).sub(2) };
        let len = header_val & 0xFFFFFFFFFFFF;
        unsafe { std::slice::from_raw_parts(p as *const u8, len) }
    } else if str_is_literal(ptr) {
        let len = unsafe { *(p as *const usize).sub(1) };
        unsafe { std::slice::from_raw_parts(p as *const u8, len) }
    } else {
        unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char).to_bytes() }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_len(s: i64) -> i64 {
    let s = expect_str(s, "len");
    olive_str_to_bytes(s).len() as i64
}

/// Three-way lexicographic compare over raw bytes, for `<`/`<=`/`>`/`>=`.
/// Same byte order `olive_list_sort_str` sorts by, without allocating.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_cmp(a: i64, b: i64) -> i64 {
    let a = expect_str(a, "cmp");
    let b = expect_str(b, "cmp");
    match olive_str_to_bytes(a).cmp(olive_str_to_bytes(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Validates a word about to be read as a string: a tagged heap or literal
/// pointer, or an interned char. Method calls through an `Any` receiver
/// reach these entries with arbitrary words, which would otherwise read out
/// of bounds (the whole surface segfaulted on an `Any`-held int). Faults
/// instead, naming the method.
pub(crate) fn expect_str(word: i64, method: &str) -> i64 {
    // `0` (`None`) keeps its existing per-function behavior below; only
    // nonzero impostors fault, so no non-crashing input changes meaning.
    if word == 0 || is_interned_char(word) || (word & 1 == 1 && (word & !1) > 0x10000) {
        return word;
    }
    crate::panic::abort(&format!("`{method}` requires a string argument"), None)
}

fn str_char_at(s: i64, i: i64, loc: i64, checked: bool) -> i64 {
    let s = expect_str(s, "indexing");
    if s == 0 {
        if checked {
            crate::panic::olive_nil_index_fail(loc);
        }
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let char_len = text.chars().count() as i64;
    let idx = if i < 0 {
        i.checked_add(char_len).unwrap_or(i)
    } else {
        i
    };
    if idx < 0 || idx >= char_len {
        if checked {
            crate::panic::olive_bounds_fail(i, char_len, loc);
        }
        return 0;
    }
    let byte_idx = text
        .char_indices()
        .nth(idx as usize)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len());
    let ch = text[byte_idx..].chars().next().unwrap_or('\0');
    if ch.is_ascii() {
        char_str(ch as u8)
    } else {
        olive_str_internal(&ch.to_string())
    }
}

/// Interned single-byte strings, NUL-terminated like any literal. Indexing
/// and per-char iteration return these instead of allocating, and the free
/// path already ignores pointers outside the slab span.
#[repr(C, align(8))]
pub struct CharTable([[u8; 8]; 256]);

// Eight-byte stride from an eight-aligned base keeps all low tag bits clear on
// every entry, so an interned char pointer never reads as a tagged string.
#[unsafe(export_name = "olive_char_table")]
pub static CHAR_STRS: CharTable = {
    let mut t = [[0u8; 8]; 256];
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

/// Whether a word points into the interned single-char table. These pointers
/// are deliberately untagged (4-byte stride keeps bits 0-1 clear), so the
/// magnitude heuristic reads them as scalars. For hashing and equality
/// they are the one-character string they point at, and must classify as
/// `Str` or cross-representation lookups (`d[s[i]]` vs `d["a"]`) miss.
pub(crate) fn is_interned_char(v: i64) -> bool {
    let base = CHAR_STRS.0.as_ptr() as i64;
    let body = v & !(crate::string_slab::STR_TAG
        | crate::string_slab::STR_HEAP
        | crate::string_slab::STR_LITERAL);
    body >= base && body < base + 2048
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    str_char_at(s, i, 0, false)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char(s: i64, i: i64) -> i64 {
    let s = expect_str(s, "indexing");
    olive_str_get(s, i)
}

/// Bounds-checked single-character index. Length comes from the slab header
/// (or strlen for literals), so the read is O(1); panics with the source
/// location on a null receiver or an out-of-range index.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get_checked(s: i64, i: i64, loc: i64) -> i64 {
    str_char_at(s, i, loc, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_getslice(s: i64, start: i64, stop: i64, step: i64, flags: i64) -> i64 {
    let s = expect_str(s, "slicing");
    if s == 0 {
        return olive_str_internal("");
    }
    let text = olive_str_from_ptr(s);
    let char_len = text.chars().count() as i64;
    // The common forward-contiguous slice is one char-boundary byte range;
    // only a stepped or backwards walk needs the per-char Vec.
    let (nstart, nstop, nstep) = crate::list::slice_bounds(char_len, start, stop, step, flags);
    if nstep == 1 {
        // A reversed range (nstart >= nstop) is empty under CPython rules;
        // slicing the bytes directly would panic instead.
        if nstart >= nstop {
            return olive_str_internal("");
        }
        let from = match nstart {
            0 => 0,
            _ => text
                .char_indices()
                .nth(nstart as usize)
                .map(|(b, _)| b)
                .unwrap_or(text.len()),
        };
        let to = text
            .char_indices()
            .nth(nstop as usize)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        return olive_str_internal(&text[from..to]);
    }
    // Raw operands: slice_indices renormalizes, and a pre-normalized negative
    // step's stop of -1 must survive as its "include index 0" sentinel.
    let chars: Vec<char> = text.chars().collect();
    let idxs = crate::list::slice_indices(char_len, start, stop, step, flags);
    let out: String = idxs.iter().map(|&i| chars[i]).collect();
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_slice(s: i64, start: i64, end: i64) -> i64 {
    olive_str_getslice(s, start, end, 1, 3)
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
    fn interned_char_is_reflected_as_string() {
        let c = char_str(b'a');
        assert_eq!(crate::olive_is_str(c), 1);
        assert_eq!(crate::olive_str_from_ptr(crate::olive_typeof_str(c)), "str");
    }

    #[test]
    fn interned_nul_char_has_length_one() {
        let c = char_str(0);
        assert!(is_interned_char(c));
        assert_eq!(olive_str_to_bytes(c), &[0]);
        assert_eq!(olive_str_len(c), 1);
        assert_eq!(olive_str_len(olive_str_get_checked(c, 0, 0)), 1);
        let source = olive_str_internal("a\0b");
        assert_eq!(olive_str_len(olive_str_get_checked(source, 0, 0)), 1);
        assert_eq!(olive_str_len(olive_str_get_checked(source, 1, 0)), 1);
        crate::olive_free_str(source);
    }

    #[test]
    fn internal_strings_preserve_embedded_nul() {
        let s = olive_str_internal("a\0b");
        assert_eq!(olive_str_to_bytes(s), b"a\0b");
        assert_eq!(olive_str_len(s), 3);
        let copy = crate::olive_copy(s);
        assert_eq!(olive_str_to_bytes(copy), b"a\0b");
        crate::olive_free_str(copy);
        crate::olive_free_str(s);
    }

    #[test]
    fn first_and_last_chars() {
        let s = olive_str_internal("xyz");
        assert_eq!(olive_str_from_ptr(olive_str_get_checked(s, 0, 0)), "x");
        assert_eq!(olive_str_from_ptr(olive_str_get_checked(s, 2, 0)), "z");
    }
}

pub fn olive_str_as_str<'a>(ptr: i64) -> Option<&'a str> {
    if ptr == 0 || crate::olive_is_str(ptr) != 1 {
        return None;
    }
    let body = str_body(ptr);
    if str_is_heap(ptr) {
        if !crate::slab::ptr_is_slab_body(body) {
            return None;
        }
    } else if body & 3 != 0 {
        return None;
    }
    std::str::from_utf8(olive_str_to_bytes(ptr)).ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim(s: i64) -> i64 {
    let s = expect_str(s, "strip");
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_start(s: i64) -> i64 {
    let s = expect_str(s, "lstrip");
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_start())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_end(s: i64) -> i64 {
    let s = expect_str(s, "rstrip");
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_end())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_upper(s: i64) -> i64 {
    let s = expect_str(s, "upper");
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_uppercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_lower(s: i64) -> i64 {
    let s = expect_str(s, "lower");
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_lowercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_replace(s: i64, from: i64, to: i64) -> i64 {
    let s = expect_str(s, "replace");
    let from = expect_str(from, "replace");
    let to = expect_str(to, "replace");
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
    let s = expect_str(s, "find");
    let needle = expect_str(needle, "find");
    if s == 0 || needle == 0 {
        return -1;
    }
    let text = match olive_str_as_str(s) {
        Some(t) => t,
        None => return -1,
    };
    let pat = match olive_str_as_str(needle) {
        Some(p) => p,
        None => return -1,
    };
    match text.find(pat) {
        Some(byte_idx) => text[..byte_idx].chars().count() as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_contains(s: i64, needle: i64) -> i64 {
    let s = expect_str(s, "contains");
    let needle = expect_str(needle, "contains");
    if s == 0 || needle == 0 {
        return 0;
    }
    // Byte-slice search: UTF-8 is self-synchronizing, so a byte-substring
    // match is exactly a character-substring match, with no owned copies.
    let hay = olive_str_to_bytes(s);
    let pat = olive_str_to_bytes(needle);
    (pat.is_empty() || hay.windows(pat.len()).any(|w| w == pat)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_starts_with(s: i64, prefix: i64) -> i64 {
    let s = expect_str(s, "startswith");
    let prefix = expect_str(prefix, "startswith");
    if s == 0 || prefix == 0 {
        return 0;
    }
    if olive_str_to_bytes(s).starts_with(olive_str_to_bytes(prefix)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_ends_with(s: i64, suffix: i64) -> i64 {
    let s = expect_str(s, "endswith");
    let suffix = expect_str(suffix, "endswith");
    if s == 0 || suffix == 0 {
        return 0;
    }
    if olive_str_to_bytes(s).ends_with(olive_str_to_bytes(suffix)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_repeat(s: i64, n: i64) -> i64 {
    let s = expect_str(s, "repeat");
    if s == 0 || n <= 0 {
        return olive_str_internal("");
    }
    olive_str_internal(&olive_str_from_ptr(s).repeat(n as usize))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_split(s: i64, sep: i64) -> i64 {
    // `sep == 0` is the whitespace-split sentinel, not a missing string.
    let s = expect_str(s, "split");
    let sep = if sep == 0 {
        0
    } else {
        expect_str(sep, "split")
    };
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
    let sep = expect_str(sep, "join");
    let s = unsafe { &*(list_ptr as *const StableVec) };
    let sep_bytes = olive_str_to_bytes(sep);
    let mut out = Vec::new();
    for i in 0..s.len {
        if i > 0 {
            out.extend_from_slice(sep_bytes);
        }
        let elem = unsafe { *s.ptr.add(i) };
        // A dynamically-typed list can hold non-strings: reading one as a
        // string pointer runs out of bounds, so fault instead.
        let elem = expect_str(elem, "join");
        out.extend_from_slice(olive_str_to_bytes(elem));
    }
    // SAFETY: every element is a well-formed Olive string, so their
    // concatenation is valid UTF-8.
    olive_str_internal(unsafe { std::str::from_utf8_unchecked(&out) })
}

/// Non-overlapping occurrences of `sub` in `s`, Python's `str.count` semantics
/// (an empty `sub` counts every gap, `len(s) + 1` positions).
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_count(s: i64, sub: i64) -> i64 {
    let s = expect_str(s, "count");
    let sub = expect_str(sub, "count");
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(sub);
    text.matches(&pat).count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_rfind(s: i64, needle: i64) -> i64 {
    let s = expect_str(s, "rfind");
    let needle = expect_str(needle, "rfind");
    if s == 0 || needle == 0 {
        return -1;
    }
    let text = match olive_str_as_str(s) {
        Some(t) => t,
        None => return -1,
    };
    let pat = match olive_str_as_str(needle) {
        Some(p) => p,
        None => return -1,
    };
    match text.rfind(pat) {
        Some(byte_idx) => text[..byte_idx].chars().count() as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_splitlines(s: i64) -> i64 {
    let s = expect_str(s, "splitlines");
    let text = olive_str_from_ptr(s);
    let parts: Vec<i64> = text.lines().map(olive_str_internal).collect();
    crate::list::list_from_vec(parts)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_title(s: i64) -> i64 {
    let s = expect_str(s, "title");
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
    let s = expect_str(s, "capitalize");
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
    let s = expect_str(s, "zfill");
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
    olive_str_from_ptr(expect_str(fill, "fill"))
        .chars()
        .next()
        .unwrap_or(' ')
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_ljust(s: i64, width: i64, fill: i64) -> i64 {
    let s = expect_str(s, "ljust");
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
    let s = expect_str(s, "rjust");
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
    let s = expect_str(s, "center");
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
    let s = expect_str(s, "removeprefix");
    let prefix = expect_str(prefix, "removeprefix");
    let text = olive_str_from_ptr(s);
    let pre = olive_str_from_ptr(prefix);
    olive_str_internal(text.strip_prefix(&pre).unwrap_or(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_removesuffix(s: i64, suffix: i64) -> i64 {
    let s = expect_str(s, "removesuffix");
    let suffix = expect_str(suffix, "removesuffix");
    let text = olive_str_from_ptr(s);
    let suf = olive_str_from_ptr(suffix);
    olive_str_internal(text.strip_suffix(&suf).unwrap_or(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isdigit(s: i64) -> i64 {
    let s = expect_str(s, "isdigit");
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_numeric())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isalpha(s: i64) -> i64 {
    let s = expect_str(s, "isalpha");
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_alphabetic())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isspace(s: i64) -> i64 {
    let s = expect_str(s, "isspace");
    let text = olive_str_from_ptr(s);
    (!text.is_empty() && text.chars().all(|c| c.is_whitespace())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_isupper(s: i64) -> i64 {
    let s = expect_str(s, "isupper");
    let text = olive_str_from_ptr(s);
    let cased: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    (!cased.is_empty() && cased.iter().all(|c| c.is_uppercase())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_islower(s: i64) -> i64 {
    let s = expect_str(s, "islower");
    let text = olive_str_from_ptr(s);
    let cased: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    (!cased.is_empty() && cased.iter().all(|c| c.is_lowercase())) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_chars(s: i64, chars: i64) -> i64 {
    let s = expect_str(s, "strip");
    let chars = expect_str(chars, "strip");
    let text = olive_str_from_ptr(s);
    let set: std::collections::HashSet<char> = olive_str_from_ptr(chars).chars().collect();
    olive_str_internal(text.trim_matches(|c| set.contains(&c)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_start_chars(s: i64, chars: i64) -> i64 {
    let s = expect_str(s, "lstrip");
    let chars = expect_str(chars, "lstrip");
    let text = olive_str_from_ptr(s);
    let set: std::collections::HashSet<char> = olive_str_from_ptr(chars).chars().collect();
    olive_str_internal(text.trim_start_matches(|c| set.contains(&c)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_end_chars(s: i64, chars: i64) -> i64 {
    let s = expect_str(s, "rstrip");
    let chars = expect_str(chars, "rstrip");
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
    let s = expect_str(s, "partition");
    let sep = expect_str(sep, "partition");
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
    let template = expect_str(template, "fmt");
    if template == 0 {
        return olive_str_internal("");
    }
    let tmpl = match olive_str_as_str(template) {
        Some(t) => t,
        // A non-UTF-8 template keeps the historical lossy rendering.
        None => return olive_str_internal(&olive_str_from_ptr(template)),
    };
    // Borrowed views: the list outlives this call and each part is only
    // concatenated once, so no owned copy per argument is needed.
    let arg_bytes: Vec<&[u8]> = if args == 0 {
        Vec::new()
    } else {
        let sv = unsafe { &*(args as *const StableVec) };
        (0..sv.len)
            .map(|i| olive_str_to_bytes(unsafe { *sv.ptr.add(i) }))
            .collect()
    };
    let mut result = Vec::with_capacity(tmpl.len());
    let mut parts = tmpl.split("{}").peekable();
    let mut arg_idx = 0;
    while let Some(part) = parts.next() {
        result.extend_from_slice(part.as_bytes());
        if parts.peek().is_some() && arg_idx < arg_bytes.len() {
            result.extend_from_slice(arg_bytes[arg_idx]);
            arg_idx += 1;
        }
    }
    // SAFETY: template is valid UTF-8 and every argument is a well-formed
    // Olive string, so the assembled bytes are valid UTF-8.
    olive_str_internal(unsafe { std::str::from_utf8_unchecked(&result) })
}

/// Builds a list of the string's characters, each as a one-character string.
/// Backs `for c in s` iteration.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_chars(s: i64) -> i64 {
    let s = expect_str(s, "chars");
    if s == 0 {
        return crate::list::olive_list_new(0);
    }
    let text = olive_str_from_ptr(s);
    let list = crate::list::olive_list_new(text.chars().count() as i64);
    let mut buf = [0u8; 4];
    for (i, c) in text.chars().enumerate() {
        crate::list::olive_list_set(
            list,
            i as i64,
            crate::string_slab::str_alloc(c.encode_utf8(&mut buf).as_bytes()),
        );
    }
    list
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char_count(s: i64) -> i64 {
    let s = expect_str(s, "len");
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).chars().count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_is_ascii(s: i64) -> i64 {
    let s = expect_str(s, "is_ascii");
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
    let s = expect_str(s, "graphemes");
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).graphemes(true).count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_graphemes(s: i64) -> i64 {
    use unicode_segmentation::UnicodeSegmentation;
    let s = expect_str(s, "graphemes");
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
    fn slice_invalid_range_clamps_to_empty() {
        assert_eq!(from_ptr(olive_str_slice(s("hello"), 3, 1)), "");
        assert_eq!(from_ptr(olive_str_slice(s("hello"), 10, 15)), "");
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
    fn find_returns_char_index_not_byte_index() {
        assert_eq!(olive_str_find(s("❯ hello"), s("hello")), 2);
    }

    #[test]
    fn rfind_returns_char_index_not_byte_index() {
        assert_eq!(olive_str_rfind(s("❯ ab ❯ cd"), s("❯")), 5);
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

    #[test]
    fn getslice_char_indexed_contiguous() {
        assert_eq!(from_ptr(olive_str_getslice(s("héllo"), 0, 2, 1, 7)), "hé");
        assert_eq!(from_ptr(olive_str_getslice(s("héllo"), 2, 5, 1, 7)), "llo");
        assert_eq!(
            from_ptr(olive_str_getslice(s("héllo"), 0, -1, 1, 7)),
            "héll"
        );
        assert_eq!(from_ptr(olive_str_getslice(s("héllo"), -3, -1, 1, 7)), "ll");
    }

    #[test]
    fn getslice_stepped_and_reversed() {
        assert_eq!(from_ptr(olive_str_getslice(s("abcdef"), 0, 6, 2, 7)), "ace");
        assert_eq!(from_ptr(olive_str_getslice(s("abcdef"), 0, 6, -1, 7)), "");
        // An explicit negative stop normalizes to len-1, so this is empty.
        assert_eq!(from_ptr(olive_str_getslice(s("abcdef"), 3, -1, -1, 7)), "");
        assert_eq!(
            from_ptr(olive_str_getslice(s("abcdef"), 5, 0, -2, 7)),
            "fdb"
        );
        // Emitter shape for `s[3::-1]`: stop omitted (flag clear, raw 0),
        // whose normalization is the -1 sentinel meaning "down to index 0".
        assert_eq!(
            from_ptr(olive_str_getslice(s("abcdef"), 3, 0, -1, 5)),
            "dcba"
        );
    }

    #[test]
    fn getslice_empty_ranges_do_not_panic() {
        assert_eq!(from_ptr(olive_str_getslice(s("abc"), 2, 1, 1, 7)), "");
        assert_eq!(from_ptr(olive_str_getslice(s("abc"), 3, 3, 1, 7)), "");
        assert_eq!(from_ptr(olive_str_getslice(s(""), 0, 0, 1, 7)), "");
    }

    #[test]
    fn getslice_omitted_bounds_default_to_full_string() {
        // flags = SLICE_HAS_STEP only: both endpoints omitted.
        assert_eq!(
            from_ptr(olive_str_getslice(s("héllo"), 0, 0, 1, 4)),
            "héllo"
        );
        assert_eq!(
            from_ptr(olive_str_getslice(s("héllo"), 0, 0, -1, 4)),
            "olléh"
        );
    }

    #[test]
    fn slice_mid_char_boundary_uses_scalar_range() {
        let ptr = olive_str_internal("é");
        let bytes = crate::olive_str_to_bytes(ptr);
        let got = olive_str_slice(ptr, 0, (bytes.len() - 1) as i64);
        assert_eq!(from_ptr(got), "é");
    }

    #[test]
    fn contains_multibyte_matches_char_semantics() {
        assert_eq!(olive_str_contains(s("aéz"), s("é")), 1);
        assert_eq!(olive_str_contains(s("aéz"), s("ez")), 0);
        assert_eq!(olive_str_contains(s(""), s("x")), 0);
        // Null needle keeps the historical "absent" reading, not "".
        assert_eq!(olive_str_contains(s("x"), 0), 0);
    }
}
