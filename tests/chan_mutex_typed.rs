//! Typed channels and mutexes holding non-scalar values.
//!
//! Two cooperating fixes: nested generic calls inside a monomorphized method
//! body specialize with the concrete type (before, `_send_T` compiled fully
//! erased, so a struct sent through `Chan[P].send` hit `copy_any` with a
//! `D_ANY` descriptor and segfaulted in `copy_any_node`), and channel/mutex
//! teardown drains through each value's own descriptor (before, a pending
//! 1-field struct aborted with `free(): invalid pointer` via a `KIND_LIST`
//! misread). Leak half: 20K pending 1KB-struct sends hold flat at ~280MB
//! (the JIT baseline) instead of crashing.

#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn chan_struct_send_recv_round_trips() {
    assert_both(
        r#"import aio

struct P:
    s: str
    t: str

fn main():
    let c = aio.chan[P]()
    if c == 0:
        print("nochan")
        return
    c.send(P("a", "b"))
    let v = c.recv()
    print(v.s + v.t)
"#,
        "ab\n",
    );
}

#[test]
fn chan_single_field_struct_send_recv_round_trips() {
    assert_both(
        r#"import aio

struct Q:
    s: str

fn main():
    let c = aio.chan[Q]()
    if c == 0:
        print("nochan")
        return
    c.send(Q("hello"))
    let v = c.recv()
    print(v.s)
"#,
        "hello\n",
    );
}

#[test]
fn chan_pending_struct_drain_drops_without_crashing() {
    assert_both(
        r#"import aio

struct Q:
    s: str

fn main():
    let c = aio.chan[Q]()
    if c == 0:
        print("nochan")
        return
    c.send(Q("pending"))
    print("sent")
"#,
        "sent\n",
    );
}

#[test]
fn chan_pending_drop_hook_runs_once() {
    assert_both(
        r#"import aio

struct Loud:
    s: str
impl Loud:
    fn __drop__(self):
        print("dropped " + self.s)

fn main():
    let c = aio.chan[Loud]()
    if c == 0:
        print("nochan")
        return
    c.send(Loud("pending"))
    print("sent")
"#,
        "sent\ndropped pending\n",
    );
}

#[test]
fn chan_pending_enum_drain_drops_without_crashing() {
    assert_both(
        r#"import aio

enum E:
    V(str)

fn main():
    let c = aio.chan[E]()
    if c == 0:
        print("nochan")
        return
    c.send(V("pending"))
    print("sent")
"#,
        "sent\n",
    );
}

#[test]
fn mutex_struct_lock_unlock_round_trips() {
    assert_both(
        r#"import aio

struct P:
    s: str
    t: str

fn main():
    let m = aio.mutex[P](P("x", "y"))
    if m == 0:
        print("nomutex")
        return
    let lv = m.lock()
    print(lv.s + lv.t)
    m.unlock(P("p", "q"))
    let lv2 = m.lock()
    print(lv2.s + lv2.t)
    m.unlock(lv2)
    print("ok")
"#,
        "xy\npq\nok\n",
    );
}

#[test]
fn mutex_held_struct_drain_drops_without_crashing() {
    assert_both(
        r#"import aio

struct Q:
    s: str

fn main():
    let m = aio.mutex[Q](Q("held"))
    if m == 0:
        print("nomutex")
        return
    print("made")
"#,
        "made\n",
    );
}
