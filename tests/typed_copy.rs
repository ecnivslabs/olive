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

#[test]
fn setdefault_hit_discards_struct_default() {
    assert_both(
        r#"struct R:
    s: str

fn main():
    let d: dict[int, R] = {1: R("kept")}
    let old = d.setdefault(1, R("orphan"))
    print(old.s)
    let nxt = d.setdefault(2, R("fresh"))
    print(nxt.s)
    print(d[2].s)
"#,
        "kept\nfresh\nfresh\n",
    );
}

#[test]
fn nullable_scalar_dict_get_preserves_tags_and_defaults() {
    assert_both(
        r#"fn main():
    let d: dict[int, int | None] = {1: 1, 0: None}
    print(d.get(1, None))
    print(d.get(0, None))
    print(d.get(2, None))
    print(d.setdefault(0, 9))
    print(d.setdefault(3, None))
    print(d[3])
    let b: dict[int, bool | None] = {1: True, 0: None}
    print(b.get(1, None))
    print(b.get(0, None))
    let f: dict[int, float | None] = {1: 1.5, 0: None}
    print(f.get(1, None))
    print(f.get(0, None))
"#,
        "1\nNone\nNone\nNone\nNone\nNone\nTrue\nNone\n1.5\nNone\n",
    );
}

#[test]
fn struct_set_iteration_keeps_owner_alive() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("a")}
    for x in st:
        print("saw "+x.s)
    print(len(st))
    print("done")
"#,
        "saw a\n1\ndone\ndrop a\n",
    );
}

#[test]
fn plain_struct_set_iteration_reads_fields() {
    assert_both(
        r#"struct P:
    n: int

fn main():
    let st = {P(41)}
    for x in st:
        print(x.n + 1)
    print("done")
"#,
        "42\ndone\n",
    );
}

#[test]
fn struct_set_enumerate_counts_elements() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("a")}
    for p in enumerate(st):
        print("saw "+p[1].s)
    print("done")
"#,
        "saw a\ndone\ndrop a\n",
    );
}

#[test]
fn struct_set_param_iteration_keeps_caller_alive() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn show(st: set[Res]):
    for x in st:
        print("saw "+x.s)

fn main():
    let st = {Res("a")}
    show(st)
    print(len(st))
    print("done")
"#,
        "saw a\n1\ndone\ndrop a\n",
    );
}

#[test]
fn struct_dict_values_snapshot_shares_owner() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let d = {"k": Res("v")}
    let vs = d.values()
    print("got "+vs[0].s)
    print("orig "+d["k"].s)
    print("done")
"#,
        "got v\norig v\ndone\ndrop v\n",
    );
}

#[test]
fn struct_dict_items_snapshot_shares_owner() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let d = {"k": Res("v")}
    let ps = d.items()
    print("saw "+ps[0][0]+" "+ps[0][1].s)
    print("orig "+d["k"].s)
    print("done")
"#,
        "saw k v\norig v\ndone\ndrop v\n",
    );
}

#[test]
fn struct_dict_items_direct_loop_no_false_giveaway() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let d = {"k": Res("v")}
    for p in d.items():
        print("saw "+p[0]+" "+p[1].s)
    print("done")
"#,
        "saw k v\ndone\ndrop v\n",
    );
}

#[test]
fn struct_set_remove_runs_hook_once() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("a")}
    st.remove(Res("a"))
    print(len(st))
    print("done")
"#,
        "drop a\n0\ndone\ndrop a\n",
    );
}

#[test]
fn struct_set_discard_runs_hook_once() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("a")}
    st.discard(Res("a"))
    print(len(st))
    print("done")
"#,
        "drop a\n0\ndone\ndrop a\n",
    );
}

#[test]
fn struct_containers_clear_runs_hooks() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let xs = [Res("a")]
    xs.clear()
    print(len(xs))
    let st = {Res("b")}
    st.clear()
    print(len(st))
    let d = {"k": Res("v")}
    d.clear()
    print(len(d))
    print("done")
"#,
        "drop a\n0\ndrop b\n0\ndrop v\n0\ndone\n",
    );
}

#[test]
fn dict_get_in_loop_reads_correct_values() {
    assert_both(
        r#"fn main():
    let d: dict[int, int] = {}
    let mut i = 0
    while i < 200000:
        d[i] = i * 2
        i = i + 1
    let mut acc = 0
    let mut j = 0
    while j < 100:
        acc = acc + d[j]
        j = j + 1
    print(acc)
"#,
        "9900\n",
    );
}
