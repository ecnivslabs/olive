use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0600",
        title: "Python module cannot be imported",
        summary: "An `import py` names a module the active interpreter cannot find. \
                  Olive introspects Python modules at compile time to type-check \
                  their use, so an absent module is a hard error.",
        wrong: "import py \"reqeusts\" as reqeusts\n\nfn main():\n    reqeusts.get(\"https://example.com\")",
        fixed: "import py \"requests\" as requests\n\nfn main():\n    requests.get(\"https://example.com\")",
        notes: &[
            "Install the package (e.g. `pip install requests`) into the interpreter \
             Olive uses, or add an explicit stub block.",
        ],
    },
    Explanation {
        code: "E0601",
        title: "Python module has no such attribute",
        summary: "A name accessed on an imported Python module is not present in the \
                  stub Olive built for it. It is a typo, or a name the module does not \
                  expose.",
        wrong: "import py \"json\" as json\n\nfn main():\n    json.dump_string(\"{}\")",
        fixed: "import py \"json\" as json\n\nfn main():\n    json.dumps(\"{}\")",
        notes: &["Only names introspected from the module are type-checked; check the spelling."],
    },
    Explanation {
        code: "E0602",
        title: "wrong number of arguments to a Python callable",
        summary: "A Python function was called with an argument count its introspected \
                  signature does not accept.",
        wrong: "import py \"math\" as math\n\nfn main():\n    let x = math.sqrt(4, 9)",
        fixed: "import py \"math\" as math\n\nfn main():\n    let x = math.sqrt(4)",
        notes: &["Match the parameter list Olive read from the module at compile time."],
    },
    Explanation {
        code: "E0603",
        title: "function can't cross into a Python callable",
        summary: "An Olive function value was assigned to a `PyObject` slot or passed as \
                  an argument to a Python call (`sorted(xs, key=f)`, `dataset.map(f)`, a \
                  callback parameter on an imported module). Exporting it as a real \
                  `PyCFunction` needs at most 4 parameters, and every parameter and the \
                  return type must be a scalar (`int`, `float`, `bool`), `str`, or \
                  `PyObject` -- the runtime trampoline that decodes Python's call \
                  arguments only knows those shapes.",
        wrong: "fn process(xs: [int]) -> int:\n    return xs[0]\n\nfn main():\n    let cb: PyObject = process",
        fixed: "fn process(x: int) -> int:\n    return x\n\nfn main():\n    let cb: PyObject = process",
        notes: &[
            "Unwrap or flatten a list/dict/struct parameter into scalars before handing \
             the function to Python.",
            "A function with more than 4 parameters can't cross either; wrap it in a \
             smaller adapter first.",
        ],
    },
    Explanation {
        code: "W0601",
        title: "module could not be introspected (python3 missing)",
        summary: "`python3` was not found on PATH, so the imported module could not be \
                  introspected. Calls into it fall back to dynamic typing instead of \
                  being statically checked.",
        wrong: "import py \"numpy\" as numpy\n\nfn main():\n    numpy.zeros(3)",
        fixed: "import py \"numpy\" as numpy:\n    fn zeros(n: i64) -> PyObject\n\n\
                fn main():\n    numpy.zeros(3)",
        notes: &[
            "Install a `python3` reachable on PATH, or add an explicit stub block to \
             recover static checks.",
        ],
    },
    Explanation {
        code: "W0602",
        title: "module introspection failed",
        summary: "The interpreter was found but introspecting the module failed (an \
                  import-time error inside the module, for instance). Its calls fall \
                  back to dynamic typing.",
        wrong: "import py \"brokenmod\" as brokenmod\n\nfn main():\n    brokenmod.run()",
        fixed: "import py \"brokenmod\" as brokenmod:\n    fn run()\n\nfn main():\n    brokenmod.run()",
        notes: &[
            "Fix what makes the module fail to import, or declare an explicit stub \
             block so its names are still checked.",
        ],
    },
];
