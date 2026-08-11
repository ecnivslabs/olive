//! Typed container copies keep raw scalar keys hashable.
//!
//! A large odd int is bit-identical to a tagged string pointer, so the
//! magnitude heuristic misreads it as a string and dereferences the raw
//! bits. User-level stores go through `_typed` ops (descriptor-authoritative
//! hashing), but the copy walk re-inserted through untyped adds and faulted
//! (`misaligned pointer` in `olive_str_to_bytes`); copies now insert typed.

#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn copied_int_set_keeps_large_odd_members() {
    assert_both(
        r#"import aio

struct W:
    xs: set[int]

fn main():
    let c = aio.chan[W]()
    if c == 0:
        print("nochan")
        return
    let s: set[int] = {99999, 2}
    c.send(W(s))
    print("sent")
    let t = c.recv()
    print(len(t.xs))
    print(99999 in t.xs)
"#,
        "sent\n2\nTrue\n",
    );
}

#[test]
fn copied_int_keyed_dict_keeps_entries() {
    assert_both(
        r#"import aio

struct W:
    d: dict[int, str]

fn main():
    let c = aio.chan[W]()
    if c == 0:
        print("nochan")
        return
    let d: dict[int, str] = {99999: "big", 3: "three"}
    c.send(W(d))
    print("sent")
    let t = c.recv()
    print(t.d[99999])
    print(t.d[3])
"#,
        "sent\nbig\nthree\n",
    );
}
