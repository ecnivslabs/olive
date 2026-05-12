use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0100",
        title: "invalid token",
        summary: "The lexer hit a character that is not part of Olive's syntax, so \
                  the source could not be turned into tokens.",
        wrong: "fn main():\n    let x = 1 $ 2",
        fixed: "fn main():\n    let x = 1 + 2",
        notes: &[
            "A stray symbol, an unterminated string, or a non-ASCII lookalike of an \
             operator are the usual causes.",
        ],
    },
    Explanation {
        code: "E0200",
        title: "syntax error",
        summary: "The tokens are individually valid but do not form a construct the \
                  grammar accepts at this position: a missing colon, an unbalanced \
                  bracket, or a statement where an expression was expected.",
        wrong: "fn main()\n    print(1)",
        fixed: "fn main():\n    print(1)",
        notes: &[
            "Block headers (`fn`, `if`, `for`, `while`, `match`, `case`) end with a \
             colon and indent their body.",
        ],
    },
    Explanation {
        code: "E0300",
        title: "module not found",
        summary: "An `import` names a module that could not be located in the project \
                  directory, the standard library, or any installed pod.",
        wrong: "import graphis\n\nfn main():\n    graphis.draw()",
        fixed: "import graphics\n\nfn main():\n    graphics.draw()",
        notes: &[
            "Create `<module>.liv` next to the importing file, or install the pod \
             that provides it with `pit add`.",
        ],
    },
    Explanation {
        code: "E0301",
        title: "executable statement at module top level",
        summary: "A module that is imported by another file may contain only \
                  declarations: functions, types, enums, traits, constants, and \
                  imports. Loose statements that run are allowed only in the entry \
                  file you launch directly. The snippet below is rejected once some \
                  other file does `import helper`.",
        wrong: "print(\"loading helper\")\n\nfn greet():\n    print(\"hi\")",
        fixed: "fn greet():\n    print(\"loading helper\")\n    print(\"hi\")",
        notes: &["Move the runnable statements into a function the importer can call."],
    },
    Explanation {
        code: "E0423",
        title: "capturing closure used as a value",
        summary: "A nested function that reads a variable from its enclosing function \
                  is lifted to a function taking those variables as hidden arguments. \
                  It has no standalone value, so it cannot be returned, assigned, or \
                  passed as an argument. Call it in place instead.",
        wrong: "fn main():\n    let n = 1\n    fn inc() -> i64:\n        return n\n    let g = inc",
        fixed: "fn main():\n    let n = 1\n    fn inc() -> i64:\n        return n\n    print(inc())",
        notes: &[
            "Call the nested function directly where it is defined.",
            "If you need a value to pass around, make it a top-level function that takes \
             the captured data as parameters.",
        ],
    },
    Explanation {
        code: "E0424",
        title: "capturing closure called outside its defining scope",
        summary: "A capturing nested function reads variables from the function that \
                  defines it. It can only be called where those variables are in \
                  scope: the defining function, or a deeper one that captures the same \
                  variables. Calling it from a sibling that lacks them has nothing to \
                  pass for the captures.",
        wrong: "fn main():\n    let p = 1\n    fn a() -> i64:\n        return p\n    fn b() -> i64:\n        return a()\n    print(b())",
        fixed: "fn main():\n    let p = 1\n    fn a() -> i64:\n        return p\n    print(a())",
        notes: &[
            "Call it from the function that defines it.",
            "Or pass the captured values explicitly as parameters so any caller can supply them.",
        ],
    },
];
