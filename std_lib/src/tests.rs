use super::*;

fn s(text: &str) -> i64 {
    olive_str_internal(text)
}

fn from_ptr(ptr: i64) -> String {
    olive_str_from_ptr(ptr)
}

#[test]
fn str_trim() {
    assert_eq!(from_ptr(olive_str_trim(s("  hello  "))), "hello");
    assert_eq!(from_ptr(olive_str_trim(s("no spaces"))), "no spaces");
    assert_eq!(olive_str_trim(0), 0);
}

#[test]
fn str_upper_lower() {
    assert_eq!(from_ptr(olive_str_upper(s("hello"))), "HELLO");
    assert_eq!(from_ptr(olive_str_lower(s("WORLD"))), "world");
}

#[test]
fn str_replace() {
    assert_eq!(
        from_ptr(olive_str_replace(s("hello world"), s("world"), s("olive"))),
        "hello olive"
    );
    assert_eq!(from_ptr(olive_str_replace(s("aaa"), s("a"), s("b"))), "bbb");
}

#[test]
fn str_find() {
    assert_eq!(olive_str_find(s("hello"), s("ell")), 1);
    assert_eq!(olive_str_find(s("hello"), s("xyz")), -1);
    assert_eq!(olive_str_find(0, s("x")), -1);
}

#[test]
fn str_contains() {
    assert_eq!(olive_str_contains(s("hello world"), s("world")), 1);
    assert_eq!(olive_str_contains(s("hello"), s("xyz")), 0);
}

#[test]
fn str_starts_ends_with() {
    assert_eq!(olive_str_starts_with(s("hello"), s("hel")), 1);
    assert_eq!(olive_str_starts_with(s("hello"), s("llo")), 0);
    assert_eq!(olive_str_ends_with(s("hello"), s("llo")), 1);
    assert_eq!(olive_str_ends_with(s("hello"), s("hel")), 0);
}

#[test]
fn str_repeat() {
    assert_eq!(from_ptr(olive_str_repeat(s("ab"), 3)), "ababab");
    assert_eq!(from_ptr(olive_str_repeat(s("x"), 0)), "");
}

#[test]
fn str_split_by_sep() {
    let ptr = olive_str_split(s("a,b,c"), s(","));
    let list = unsafe { &*(ptr as *const StableVec) };
    assert_eq!(list.len, 3);
    assert_eq!(from_ptr(unsafe { *list.ptr }), "a");
    assert_eq!(from_ptr(unsafe { *list.ptr.add(1) }), "b");
    assert_eq!(from_ptr(unsafe { *list.ptr.add(2) }), "c");
}

#[test]
fn str_split_whitespace() {
    let ptr = olive_str_split(s("foo bar baz"), 0);
    let list = unsafe { &*(ptr as *const StableVec) };
    assert_eq!(list.len, 3);
}

#[test]
fn str_join() {
    let list_ptr = olive_str_split(s("a,b,c"), s(","));
    let joined = olive_str_join(list_ptr, s("-"));
    assert_eq!(from_ptr(joined), "a-b-c");
}

#[test]
fn set_add_contains_o1() {
    let set = olive_set_new(4);
    olive_set_add(set, 10);
    olive_set_add(set, 20);
    olive_set_add(set, 10);
    let s = unsafe { &*(set as *const OliveHashSet) };
    assert_eq!(s.len, 2);
    assert_eq!(olive_in_list(10, set), 1);
    assert_eq!(olive_in_list(20, set), 1);
    assert_eq!(olive_in_list(99, set), 0);
}

#[test]
fn set_len_via_list_len() {
    let set = olive_set_new(0);
    olive_set_add(set, 1);
    olive_set_add(set, 2);
    olive_set_add(set, 3);
    assert_eq!(olive_list_len(set), 3);
}

#[test]
fn set_iteration_order_stable() {
    let set = olive_set_new(0);
    for i in [5i64, 3, 7, 1, 9] {
        olive_set_add(set, i);
    }
    assert_eq!(olive_list_len(set), 5);
    let sv = unsafe { &*(set as *const OliveHashSet) };
    let items: Vec<i64> = (0..sv.len).map(|i| unsafe { *sv.ptr.add(i) }).collect();
    assert!(items.contains(&5));
    assert!(items.contains(&1));
}

#[test]
fn obj_keys_values() {
    let obj = olive_obj_new();
    olive_obj_set(obj, s("a"), 10);
    olive_obj_set(obj, s("b"), 20);
    let keys_ptr = olive_obj_keys(obj);
    let vals_ptr = olive_obj_values(obj);
    assert_eq!(olive_list_len(keys_ptr), 2);
    assert_eq!(olive_list_len(vals_ptr), 2);
}

#[test]
fn time_format_epoch() {
    let result = from_ptr(olive_time_format(0.0, 0));
    assert_eq!(result, "1970-01-01T00:00:00");
}

#[test]
fn time_format_known_date() {
    let result = from_ptr(olive_time_format(1705319445.0, 0));
    assert_eq!(result, "2024-01-15T11:50:45");
}

#[test]
fn time_format_custom() {
    let fmt = s("%Y/%m/%d");
    let result = from_ptr(olive_time_format(1705319445.0, fmt));
    assert_eq!(result, "2024/01/15");
}

#[test]
fn list_append_and_get() {
    let list = olive_list_new(0);
    olive_list_append(list, 42);
    olive_list_append(list, 99);
    assert_eq!(olive_list_len(list), 2);
    assert_eq!(olive_list_get(list, 0), 42);
    assert_eq!(olive_list_get(list, 1), 99);
}

#[test]
fn obj_set_get() {
    let obj = olive_obj_new();
    olive_obj_set(obj, s("key"), 777);
    assert_eq!(olive_obj_get(obj, s("key")), 777);
    assert_eq!(olive_obj_get(obj, s("missing")), 0);
}

#[test]
fn str_concat_and_eq() {
    let a = s("hello ");
    let b = s("world");
    let c = olive_str_concat(a, b);
    assert_eq!(from_ptr(c), "hello world");
    assert_eq!(olive_str_eq(c, s("hello world")), 1);
    assert_eq!(olive_str_eq(c, s("other")), 0);
}

#[test]
fn str_len_and_slice() {
    let text = s("hello");
    assert_eq!(olive_str_len(text), 5);
    assert_eq!(from_ptr(olive_str_slice(text, 1, 4)), "ell");
}

#[test]
fn time_now_positive() {
    assert!(olive_time_now() > 0.0);
}

#[test]
fn str_fmt_basic() {
    let tmpl = s("hello {}!");
    let args = crate::list::list_from_vec(vec![s("world")]);
    let result = from_ptr(olive_str_fmt(tmpl, args));
    assert_eq!(result, "hello world!");
}

#[test]
fn str_fmt_multiple_args() {
    let tmpl = s("{} + {} = {}");
    let args = crate::list::list_from_vec(vec![s("1"), s("2"), s("3")]);
    let result = from_ptr(olive_str_fmt(tmpl, args));
    assert_eq!(result, "1 + 2 = 3");
}

#[test]
fn str_fmt_no_placeholders() {
    let tmpl = s("no placeholders");
    let result = from_ptr(olive_str_fmt(tmpl, 0));
    assert_eq!(result, "no placeholders");
}

#[test]
fn str_char_count_ascii() {
    assert_eq!(olive_str_char_count(s("hello")), 5);
}

#[test]
fn str_char_count_unicode() {
    let emoji = s("café");
    assert_eq!(olive_str_char_count(emoji), 4);
}

#[test]
fn str_char_count_null() {
    assert_eq!(olive_str_char_count(0), 0);
}

#[test]
fn ffi_errmsg_known_errno() {
    let msg = from_ptr(olive_ffi_errmsg(s("libc::open"), 2));
    assert!(msg.starts_with("libc::open: "), "got: {msg}");
    assert!(msg.contains("os error 2"), "got: {msg}");
}

#[test]
fn ffi_errmsg_zero_errno_is_generic() {
    let msg = from_ptr(olive_ffi_errmsg(s("libc::open"), 0));
    assert_eq!(msg, "libc::open: call failed");
}

#[test]
fn ffi_clear_errno_then_read_is_zero() {
    olive_ffi_clear_errno();
    assert_eq!(olive_ffi_errno(), 0);
}

#[test]
fn ffi_errno_reads_snapshot_not_live_errno() {
    olive_ffi_clear_errno();
    // Set live errno, snapshot it, then clobber live errno the way an
    // intervening allocation would. The snapshot must survive.
    let loc = errno_location();
    if loc.is_null() {
        return;
    }
    unsafe { *loc = 13 };
    olive_ffi_snapshot_errno();
    unsafe { *loc = 0 };
    assert_eq!(olive_ffi_errno(), 13);
}

#[test]
fn any_eq_strict_kinds() {
    assert_eq!(olive_any_eq_strict(boxed::olive_box_int(7), s("7")), 0);
    assert_eq!(olive_any_eq_strict(s("7"), s("7")), 1);
    assert_eq!(olive_any_eq_strict(s("a"), s("b")), 0);
    assert_eq!(
        olive_any_eq_strict(boxed::olive_box_int(7), boxed::olive_box_int(7)),
        1
    );
    assert_eq!(
        olive_any_eq_strict(boxed::olive_box_int(7), boxed::olive_box_float(7.0)),
        1
    );
    assert_eq!(
        olive_any_eq_strict(boxed::olive_box_null(), boxed::olive_box_int(0)),
        0
    );
    assert_eq!(olive_any_eq_strict(boxed::olive_box_null(), s("")), 0);
    assert_eq!(
        olive_any_eq_strict(boxed::olive_box_null(), boxed::olive_box_null()),
        1
    );
    let wide = 1i64 << 61;
    assert_eq!(
        olive_any_eq_strict(boxed::olive_box_int(wide), boxed::olive_box_int(wide)),
        1
    );
    assert_eq!(olive_any_ne_strict(boxed::olive_box_int(1), s("1")), 1);
}
