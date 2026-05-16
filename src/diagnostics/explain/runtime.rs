use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0700",
        title: "panic",
        summary: "An `assert` failed at runtime and the program aborted. This is the \
                  generic runtime fault for an explicitly signalled unrecoverable state.",
        wrong: "fn main():\n    let x = 1\n    assert x == 2",
        fixed: "fn main():\n    let x = 2\n    assert x == 2",
        notes: &["Assert only what must always hold; return a result for expected failures."],
    },
    Explanation {
        code: "E0701",
        title: "index out of bounds",
        summary: "A list or string was indexed outside `0..len` at runtime. The fault \
                  reports both the length and the offending index.",
        wrong: "fn third(xs: [i64]) -> i64:\n    return xs[2]",
        fixed: "fn third(xs: [i64]) -> i64:\n    if len(xs) > 2:\n        return xs[2]\n    return 0",
        notes: &["Guard the access with a length check; negative indices are not supported."],
    },
    Explanation {
        code: "E0702",
        title: "indexing a null value",
        summary: "Indexing was attempted on a value that is null, an uninitialised \
                  container rather than an actual list.",
        wrong: "fn main():\n    let xs = None\n    print(xs[0])",
        fixed: "fn main():\n    let xs = [1, 2, 3]\n    print(xs[0])",
        notes: &["Assign a real container, or check the value against `None` before indexing."],
    },
    Explanation {
        code: "E0703",
        title: "divide by zero",
        summary: "The right-hand side of an integer `/` or `%` was zero at runtime. \
                  Hardware would trap with no context; Olive reports the operation \
                  and points at the source instead.",
        wrong: "fn ratio(a: i64, b: i64) -> i64:\n    return a / b",
        fixed: "fn ratio(a: i64, b: i64) -> i64:\n    if b == 0:\n        return 0\n    return a / b",
        notes: &["Guard the divisor so it is non-zero before dividing or taking a remainder."],
    },
    Explanation {
        code: "E0704",
        title: "unwrap on the wrong variant",
        summary: "`unwrap` was called on an error result. The value did not hold the \
                  success the unwrap assumed.",
        wrong: "import result\n\nfn main():\n    let r = result.err(\"boom\")\n    let n = result.unwrap(r)",
        fixed: "import result\n\nfn main():\n    let r = result.err(\"boom\")\n    let n = result.unwrap_or(r, 0)",
        notes: &["Handle the error case with `?` or `unwrap_or` instead of unwrapping blindly."],
    },
    Explanation {
        code: "E0705",
        title: "uncaught Python exception",
        summary: "A call into Python raised an exception that propagated back across \
                  the FFI boundary without being caught. The fault carries the Python \
                  traceback and the Olive call site.",
        wrong: "import py \"json\" as json\n\nfn main():\n    json.loads(\"not json\")",
        fixed: "import py \"json\" as json\n\nfn main():\n    json.loads(\"{}\")",
        notes: &["Validate the inputs so the exception cannot arise, or propagate it with `?`."],
    },
    Explanation {
        code: "E0706",
        title: "Python value cannot become the required native type",
        summary: "A value crossing back from Python did not fit the native type the \
                  surrounding Olive code requires, and no lossless conversion exists.",
        wrong: "import py \"json\" as json\n\nfn main():\n    let n: i64 = json.loads(\"\\\"text\\\"\")\n    print(n)",
        fixed: "import py \"json\" as json\n\nfn main():\n    let v = json.loads(\"\\\"text\\\"\")\n    print(v)",
        notes: &[
            "Read the value into an untyped binding first, or convert it explicitly \
             on the Python side.",
        ],
    },
    Explanation {
        code: "E0707",
        title: "stale reference",
        summary: "A borrowed value outlived its owner. The generation check caught the \
                  access before the freed memory was read, so the program stops with \
                  this fault instead of corrupting the heap.",
        wrong: "fn sink(v):\n    let tmp = [v]\n\nfn main():\n    let a = [1, 2]\n    sink(a)\n    print(a[0])",
        fixed: "fn sink(v):\n    let tmp = [v]\n\nfn main():\n    let a = [1, 2]\n    print(a[0])\n    sink(a)",
        notes: &[
            "Once a value is stored somewhere else, that place owns it: finish reading \
             through the old name first, or store a copy instead.",
        ],
    },
];
