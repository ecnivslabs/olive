use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0500",
        title: "conflicting assignment",
        summary: "A binding is assigned while a reference to it is still outstanding, \
                  so the reference and the write disagree about its value.",
        wrong: "fn main():\n    let mut x = 1\n    let r = &x\n    x = 2\n    print(r)",
        fixed: "fn main():\n    let mut x = 1\n    x = 2\n    let r = &x\n    print(r)",
        notes: &["Use a binding directly before or after it is borrowed, not during."],
    },
    Explanation {
        code: "E0501",
        title: "use of a moved value",
        summary: "A value was used after the compiler transferred it elsewhere. \
                  Ownership is inferred: a value only moves at its last use, so \
                  ordinary code cannot reach this. Seeing it means the compiler's \
                  ownership inference went wrong and it should be reported as a bug.",
        wrong: "fn consume(xs: [i64]):\n    print(len(xs))\n\n\
                fn main():\n    let data = [1, 2, 3]\n    consume(data)\n    print(len(data))",
        fixed: "fn consume(xs: [i64]):\n    print(len(xs))\n\n\
                fn main():\n    let data = [1, 2, 3]\n    print(len(data))\n    consume(data)",
        notes: &[
            "Arguments are borrowed, aliases share the value, and the owning \
                  binding frees it at scope end; none of these move a value away \
                  from a later use.",
        ],
    },
    Explanation {
        code: "E0502",
        title: "borrowed before it holds a value",
        summary: "A binding was used before it was given a value, so the read would \
                  observe a value that does not exist yet.",
        wrong: "fn main():\n    print(y)\n    let y = 5",
        fixed: "fn main():\n    let y = 5\n    print(y)",
        notes: &["Assign the binding before reading or borrowing it."],
    },
    Explanation {
        code: "E0503",
        title: "cannot borrow as immutable while mutably borrowed",
        summary: "A shared (immutable) reference was requested while a mutable \
                  reference to the same value is still live. The two cannot coexist.",
        wrong: "fn main():\n    let mut x = 1\n    let m = &mut x\n    let s = &x\n    *m = 2",
        fixed: "fn main():\n    let mut x = 1\n    let m = &mut x\n    print(m)\n    let s = &x\n    print(s)",
        notes: &["Let the mutable borrow end before taking a shared one."],
    },
    Explanation {
        code: "E0504",
        title: "cannot mutate while borrowed",
        summary: "A value is read or copied while a mutable reference to it is still \
                  live, which could observe a half-finished mutation.",
        wrong: "fn main():\n    let mut x = 1\n    let r = &mut x\n    let y = x\n    print(y)\n    print(r)",
        fixed: "fn main():\n    let mut x = 1\n    let y = x\n    let r = &mut x\n    print(y)\n    print(r)",
        notes: &["Finish the other use before taking the mutable borrow."],
    },
    Explanation {
        code: "E0505",
        title: "cannot mutably borrow an immutable binding",
        summary: "The borrow checker reached a mutable borrow of a binding that was \
                  not declared `mut`. Mutating through it would break its immutability.",
        wrong: "fn main():\n    let x = 1\n    let r = &mut x",
        fixed: "fn main():\n    let mut x = 1\n    let r = &mut x",
        notes: &["Declare the binding `mut` before borrowing it mutably."],
    },
    Explanation {
        code: "E0506",
        title: "field may be left uninitialized",
        summary: "A struct's `__init__` returns without assigning every field, leaving \
                  some field without a defined value.",
        wrong: "struct Point:\n    x: i64\n    y: i64\n\n    fn __init__(self):\n        \
                self.x = 1\n\nfn main():\n    let p = Point()\n    print(p.x)",
        fixed: "struct Point:\n    x: i64\n    y: i64\n\n    fn __init__(self):\n        \
                self.x = 1\n        self.y = 0\n\nfn main():\n    let p = Point()\n    print(p.y)",
        notes: &["Assign every field in `__init__` before it returns."],
    },
    Explanation {
        code: "E0507",
        title: "value used after its lifetime ends",
        summary: "A value is used after it was moved away. It was moved into a call \
                  once, then used again past the point its ownership ended.",
        wrong: "struct Box:\n    v: i64\n\nfn take(b: Box) -> i64:\n    return b.v\n\n\
                fn main():\n    let b = Box(1)\n    let x = take(b)\n    print(take(b))",
        fixed: "struct Box:\n    v: i64\n\nfn peek(b: Box) -> i64:\n    return b.v\n\n\
                fn main():\n    let b = Box(1)\n    print(peek(b))",
        notes: &["Use the value once after a move, or pass it by reference instead of moving it."],
    },
];
