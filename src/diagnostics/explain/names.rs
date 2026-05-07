use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0001",
        title: "name not found in scope",
        summary: "A name was used that is not bound anywhere reachable from here. \
                  It was never declared, it is spelled differently from its \
                  declaration, or it lives in a scope that has already ended.",
        wrong: "fn main():\n    print(total)",
        fixed: "fn main():\n    let total = 0\n    print(total)",
        notes: &[
            "A `let` binding is visible only after the line that introduces it, \
             and only within the block it was declared in.",
            "If the name is exported by another module, import that module first.",
        ],
    },
    Explanation {
        code: "E0002",
        title: "parameter declared twice",
        summary: "A function lists the same parameter name more than once. Each \
                  parameter must be unique so every use inside the body refers to \
                  exactly one value.",
        wrong: "fn add(x: i64, x: i64) -> i64:\n    return x + x",
        fixed: "fn add(x: i64, y: i64) -> i64:\n    return x + y",
        notes: &["Rename one of the parameters so the two values stay distinct."],
    },
    Explanation {
        code: "E0003",
        title: "assignment to an undefined name",
        summary: "An assignment targets a name that was never introduced. Olive \
                  does not create a binding on first assignment; a variable must \
                  be declared with `let` before it can be assigned.",
        wrong: "fn main():\n    count = 1",
        fixed: "fn main():\n    let mut count = 0\n    count = 1",
        notes: &[
            "Use `let mut` when the binding will be reassigned later, plain `let` \
             when it is assigned once.",
        ],
    },
    Explanation {
        code: "E0004",
        title: "use of a private name",
        summary: "A name prefixed with an underscore is module-private: it is \
                  visible only inside the module that defines it. Accessing it from \
                  another module is rejected.",
        wrong: "import util\n\nfn main():\n    util._helper()",
        fixed: "import util\n\nfn main():\n    util.helper()",
        notes: &[
            "Drop the leading underscore in the definition to make the name public, \
             or expose a public wrapper around it.",
        ],
    },
    Explanation {
        code: "E0006",
        title: "constant index out of bounds",
        summary: "A list whose length is fixed at compile time is indexed with a \
                  constant that falls outside it. This is a guaranteed runtime \
                  panic, so it is reported during compilation instead.",
        wrong: "fn main():\n    let xs = [10, 20, 30]\n    print(xs[5])",
        fixed: "fn main():\n    let xs = [10, 20, 30]\n    print(xs[2])",
        notes: &[
            "Negative indices are not supported; use an index in `0..len`.",
            "When the index is computed at runtime, guard it with a length check.",
        ],
    },
];
