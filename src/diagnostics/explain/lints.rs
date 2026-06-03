use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "W0610",
        title: "unused import",
        summary: "A module is imported but nothing from it is used. The import adds \
                  noise and a needless dependency edge.",
        wrong: "import math\n\nfn main():\n    print(1)",
        fixed: "fn main():\n    print(1)",
        notes: &["Remove the import, or use a name from the module."],
    },
    Explanation {
        code: "W0620",
        title: "unreachable statement",
        summary: "A statement can never run because control always leaves the block \
                  before reaching it, typically code after an unconditional `return`, \
                  `break`, or `continue`.",
        wrong: "fn f() -> i64:\n    return 1\n    print(\"never runs\")",
        fixed: "fn f() -> i64:\n    print(\"runs first\")\n    return 1",
        notes: &["Delete the dead code, or move it before the statement that exits."],
    },
    Explanation {
        code: "W0630",
        title: "calling convention has no effect on this target",
        summary: "A calling-convention decorator was put on a foreign function, but the \
                  current target ignores it (for example `stdcall` outside 32-bit \
                  Windows), so it does nothing.",
        wrong: "import \"libc.so.6\" as libc:\n    @stdcall\n    fn puts(s: str) -> int",
        fixed: "import \"libc.so.6\" as libc:\n    fn puts(s: str) -> int",
        notes: &["Drop the decorator, or build for a target that honours it."],
    },
    Explanation {
        code: "W0640",
        title: "unused variable or parameter",
        summary: "A binding or parameter is introduced but never read. It is usually \
                  a leftover or a sign of a forgotten use.",
        wrong: "fn area(w: i64, h: i64) -> i64:\n    return w",
        fixed: "fn area(w: i64, _h: i64) -> i64:\n    return w",
        notes: &["Prefix the name with `_` to mark it intentionally unused, or remove it."],
    },
    Explanation {
        code: "W0650",
        title: "function is never used",
        summary: "A function is defined but never called from anywhere reachable, so \
                  it contributes nothing to the program.",
        wrong: "fn helper():\n    print(\"unused\")\n\nfn main():\n    print(1)",
        fixed: "fn helper():\n    print(\"used\")\n\nfn main():\n    helper()",
        notes: &["Call it, export it if it is public API, or delete it."],
    },
    Explanation {
        code: "W0660",
        title: "unknown decorator",
        summary: "A decorator name is not one Olive recognizes. Olive decorators are a \
                  fixed set, not arbitrary wrappers, so an unknown name is silently \
                  dropped and the function runs exactly as if it were absent.",
        wrong: "@cache\nfn slow(n: int) -> int:\n    return n\n\nfn main():\n    print(slow(1))",
        fixed: "@memo\nfn slow(n: int) -> int:\n    return n\n\nfn main():\n    print(slow(1))",
        notes: &[
            "Recognized decorators are `@memo`, `#[test]`, `#[bench]`, and `@safe`; remove the rest.",
        ],
    },
];
