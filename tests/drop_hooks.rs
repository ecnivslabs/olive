#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn list_element_hooks_run_once_each() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let xs = [Res("a"), Res("b")]
    print("mid")
"#,
        "mid\ndrop a\ndrop b\n",
    );
}

#[test]
fn tuple_element_hooks_run_once_each() {
    assert_both(
        r#"struct A:
    s: str
impl A:
    fn __drop__(self):
        print("dropA "+self.s)
struct B:
    n: int
impl B:
    fn __drop__(self):
        print("dropB "+str(self.n))

fn main():
    let t = (A("x"), 1, B(7), A("y"))
    print("mid")
"#,
        "mid\ndropA x\ndropB 7\ndropA y\n",
    );
}

#[test]
fn dict_value_hooks_run_once_each() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let d = {"x": Res("a")}
    print("mid")
"#,
        "mid\ndrop a\n",
    );
}

#[test]
fn set_element_hooks_run_once_each() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("c")}
    print("mid")
"#,
        "mid\ndrop c\n",
    );
}

#[test]
fn shared_tuple_element_hook_runs_once() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let a = Res("x")
    let t = (a, 0)
    let n = a.s
    print("use "+n)
"#,
        "use x\ndrop x\n",
    );
}

#[test]
fn hook_with_if_tail_reclaims_and_exits() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        if self.s != "":
            print("drop "+self.s)

fn main():
    let mut i = 0
    while i < 2:
        let r = Res("hello")
        print("iter "+str(i))
        i = i + 1
    print("done")
"#,
        "iter 0\ndrop hello\niter 1\ndrop hello\ndone\n",
    );
}

#[test]
fn shared_channel_in_tuple_drops_once_and_exits() {
    assert_both(
        r#"import aio
fn main():
    let ch = aio.chan[str]()
    if ch == 0:
        print("alloc-fail")
        return
    let t = (ch, 1)
    print("made")
    print("done")
"#,
        "made\ndone\n",
    );
}

#[test]
fn nested_list_and_enum_payload_hooks_run() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

enum Box:
    Empty
    Full(Res)

fn main():
    let inner = [Res("in")]
    let outer = [inner]
    let e = Full(Res("x"))
    print("mid")
"#,
        "mid\ndrop x\ndrop in\n",
    );
}

#[test]
fn generic_drop_hook_runs_direct() {
    assert_both(
        r#"struct Box[T]:
    v: T
impl Box[T]:
    fn __drop__(self):
        print("dropBox")

fn main():
    let b = Box(1)
    print("made")
"#,
        "made\ndropBox\n",
    );
}

#[test]
fn generic_drop_hook_runs_in_list_and_chains_inner() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

struct Box[T]:
    v: T
impl Box[T]:
    fn __drop__(self):
        print("dropBox")

fn main():
    let xs = [Box(Res("r1"))]
    print("made")
"#,
        "made\ndropBox\ndrop r1\n",
    );
}
