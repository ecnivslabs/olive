//! Enums held through `Any`: value behavior plus the leak half.
//!
//! An enum dropped through an `Any` slot used to release only its payload
//! buffer, stranding heap payloads (one 1KB string per iteration: 300K
//! iterations peaked ~642MB). The value now carries its `D_ENUM` descriptor
//! from construction, so the untyped free walks payloads precisely
//! (post-fix the same probe holds flat at ~280MB, the JIT baseline).

#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn any_held_enum_str_payload_round_trips_and_drops() {
    assert_both(
        r#"enum E:
    V(str)

fn show(e: E) -> str:
    match e:
        case V(s):
            return s
        case _:
            return "other"

fn main():
    let a: Any = V("hello")
    let mut i = 0
    while i < 20000:
        let b: Any = V("leak-probe-payload-0123456789abcdef")
        i = i + 1
    print(show(V("hi")))
    print("ok")
"#,
        "hi\nok\n",
    );
}

#[test]
fn any_held_multi_payload_variant_drops() {
    assert_both(
        r#"enum E:
    V(str)
    W(int, str)

fn show(e: E) -> str:
    match e:
        case V(s):
            return s
        case W(n, s):
            return str(n) + s
        case _:
            return "other"

fn main():
    let w: Any = W(7, "seven")
    print(show(W(8, "eight")))
    print("ok")
"#,
        "8eight\nok\n",
    );
}

#[test]
fn any_held_enum_with_struct_payload_drops_cleanly() {
    assert_both(
        r#"struct P:
    s: str

enum F:
    Hold(P)

fn main():
    let g = Hold(P("copyme"))
    let h: Any = g
    let p: P = g[0]
    print(p.s)
    print("ok")
"#,
        "copyme\nok\n",
    );
}

#[test]
fn enum_local_reassigned_in_loop_keeps_values() {
    assert_both(
        r#"enum E:
    V(str)
    N(int)

fn main():
    let mut e = V("first")
    print(e[0])
    e = N(1)
    print("switched")
    e = V("third")
    print(e[0])
"#,
        "\"first\"\nswitched\n\"third\"\n",
    );
}
