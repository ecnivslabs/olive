use super::Explanation;

pub(super) const ENTRIES: &[Explanation] = &[
    Explanation {
        code: "E0400",
        title: "mismatched types",
        summary: "A value of one type was supplied where another is required, at an \
                  annotation, assignment, return, or argument. Olive does not convert \
                  silently between distinct types.",
        wrong: "fn main():\n    let n: i64 = \"hello\"",
        fixed: "fn main():\n    let n: i64 = 42",
        notes: &[
            "An untyped numeric literal takes its type from context; cast it with \
             `value as T` when the inferred type is wrong.",
        ],
    },
    Explanation {
        code: "E0401",
        title: "tuple length mismatch",
        summary: "A tuple value has a different number of elements than the tuple \
                  type it is being matched against.",
        wrong: "fn main():\n    let pair: (i64, i64) = (1, 2, 3)",
        fixed: "fn main():\n    let pair: (i64, i64) = (1, 2)\n    print(pair[0])",
        notes: &["Tuple arity is part of the type; the lengths must agree exactly."],
    },
    Explanation {
        code: "E0402",
        title: "function signature mismatch",
        summary: "A function value was passed to a function-typed slot whose signature \
                  it does not match. The parameter types or the return type differ.",
        wrong: "fn apply(f: fn(i64) -> i64) -> i64:\n    return f(1)\n\n\
                fn takes_two(a: i64, b: i64) -> i64:\n    return a + b\n\n\
                fn main():\n    apply(takes_two)",
        fixed: "fn apply(f: fn(i64) -> i64) -> i64:\n    return f(1)\n\n\
                fn inc(a: i64) -> i64:\n    return a + 1\n\n\
                fn main():\n    apply(inc)",
        notes: &["The passed function must accept and return exactly what the slot declares."],
    },
    Explanation {
        code: "E0403",
        title: "wrong number of fields for a struct",
        summary: "A struct was constructed positionally with more or fewer values \
                  than it has fields.",
        wrong: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(1)\n    print(p.x)",
        fixed: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(1, 2)\n    print(p.x)",
        notes: &["Supply exactly one value per declared field, in order."],
    },
    Explanation {
        code: "E0404",
        title: "value cannot be used as a condition",
        summary: "The expression in an `if` or `while` has a type with no truth value, \
                  so it cannot decide a branch. An uncalled function is the usual case.",
        wrong: "fn ready() -> bool:\n    return True\n\n\
                fn main():\n    if ready:\n        print(\"yes\")",
        fixed: "fn ready() -> bool:\n    return True\n\n\
                fn main():\n    if ready():\n        print(\"yes\")\n    else:\n        print(\"no\")",
        notes: &["A bare function name is a value; call it with `()` to get its `bool` result."],
    },
    Explanation {
        code: "E0405",
        title: "`await` requires a future",
        summary: "`await` was applied to a value that is not a future, so there is \
                  nothing asynchronous to wait on.",
        wrong: "fn main():\n    let x = await 5",
        fixed: "async fn fetch() -> i64:\n    return 5\n\n\
                async fn main():\n    let x = await fetch()",
        notes: &["Only the result of calling an `async fn` (or another future) can be awaited."],
    },
    Explanation {
        code: "E0406",
        title: "`?` cannot propagate this error type",
        summary: "The `?` operator forwards an error out of the current function, but \
                  the function's return type cannot carry one. Propagation needs a \
                  result-typed return.",
        wrong: "fn parse(s: str) -> i64:\n    let n = int(s)?\n    return n",
        fixed: "import result\n\nfn parse(s: str) -> obj:\n    let n = int(s)?\n    return result.ok(n)",
        notes: &["The enclosing function must return a result the `?` can flow out through."],
    },
    Explanation {
        code: "E0407",
        title: "missing return value",
        summary: "A function declares a return type but a path through it ends \
                  without returning a value of that type.",
        wrong: "fn sign(n: i64) -> i64:\n    if n > 0:\n        return 1",
        fixed: "fn sign(n: i64) -> i64:\n    if n > 0:\n        return 1\n    return -1",
        notes: &["Every path must return; cover the remaining branches or add a final return."],
    },
    Explanation {
        code: "E0408",
        title: "dereferencing a raw pointer is unsafe",
        summary: "Reading or writing through a raw pointer can violate memory safety, \
                  so it is only allowed inside an `unsafe` block where you take \
                  responsibility for the pointer's validity.",
        wrong: "import \"libc.so.6\" as libc:\n    fn malloc(n: int) -> *void\n\n\
                fn main():\n    unsafe:\n        let p = libc.malloc(8)\n    let first = *p",
        fixed: "import \"libc.so.6\" as libc:\n    fn malloc(n: int) -> *void\n\n\
                fn main():\n    unsafe:\n        let p = libc.malloc(8)\n        let first = *p",
        notes: &["Keep the unsafe region as small as the dereference itself."],
    },
    Explanation {
        code: "E0409",
        title: "call to an FFI function is unsafe",
        summary: "Foreign functions imported from a C library bypass Olive's checks, \
                  so calling one must happen inside an `unsafe` block.",
        wrong: "import \"libc.so.6\" as libc:\n    fn malloc(size: int) -> *void\n\n\
                fn main():\n    let p = libc.malloc(1024)",
        fixed: "import \"libc.so.6\" as libc:\n    fn malloc(size: int) -> *void\n\n\
                fn main():\n    unsafe:\n        let p = libc.malloc(1024)",
        notes: &["The block marks where C's contract, not Olive's, is being upheld."],
    },
    Explanation {
        code: "E0410",
        title: "cannot assign twice to an immutable binding",
        summary: "A `let` binding without `mut` is immutable: it can be initialised \
                  once and never reassigned.",
        wrong: "fn main():\n    let count = 0\n    count = 1",
        fixed: "fn main():\n    let mut count = 0\n    count = 1",
        notes: &["Add `mut` only when the value genuinely needs to change."],
    },
    Explanation {
        code: "E0411",
        title: "cannot mutably borrow an immutable binding",
        summary: "Taking a mutable reference to an immutable binding would allow it to \
                  change through the reference, defeating its immutability.",
        wrong: "fn bump(n: &mut i64):\n    pass\n\n\
                fn main():\n    let x = 1\n    bump(&mut x)",
        fixed: "fn bump(n: i64) -> i64:\n    return n + 1\n\n\
                fn main():\n    let mut x = 1\n    x = bump(x)\n    print(x)",
        notes: &["Pass the value and return the new one, or declare the binding `mut`."],
    },
    Explanation {
        code: "E0412",
        title: "field given twice in an initializer",
        summary: "A struct literal sets the same field more than once, leaving its \
                  value ambiguous.",
        wrong: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(x=1, x=2, y=3)",
        fixed: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(x=1, y=3)",
        notes: &["Remove the redundant assignment so each field is set once."],
    },
    Explanation {
        code: "E0413",
        title: "no such field on this struct",
        summary: "A struct literal or field access names a field the struct does not \
                  declare. Usually a typo, or a field that belongs to another type.",
        wrong: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(x=1, z=2)",
        fixed: "struct Point:\n    x: i64\n    y: i64\n\n\
                fn main():\n    let p = Point(x=1, y=2)",
        notes: &["Check the spelling against the type's declared fields."],
    },
    Explanation {
        code: "E0414",
        title: "non-exhaustive patterns",
        summary: "A `match` does not cover every possible value of the matched type, \
                  so some input would fall through with no arm to handle it.",
        wrong: "enum Color:\n    Red\n    Green\n    Blue\n\n\
                fn name(c: Color) -> str:\n    match c:\n        case Red:\n            \
                return \"red\"\n        case Green:\n            return \"green\"\n\n\
                fn main():\n    print(name(Red()))",
        fixed: "enum Color:\n    Red\n    Green\n    Blue\n\n\
                fn name(c: Color) -> str:\n    match c:\n        case Red:\n            \
                return \"red\"\n        case Green:\n            return \"green\"\n        \
                case Blue:\n            return \"blue\"\n\n\
                fn main():\n    print(name(Red()))",
        notes: &["Add the missing arms, or a `case _:` wildcard to catch the rest."],
    },
    Explanation {
        code: "E0415",
        title: "type does not implement the required trait",
        summary: "A value is used where a trait is required, but its type has no `impl` \
                  for that trait.",
        wrong: "trait Draw:\n    fn name(self) -> str:\n        return \"shape\"\n\n\
                struct Circle:\n    r: i64\n\n\
                fn render(d: Draw) -> str:\n    return d.name()\n\n\
                fn main():\n    print(render(Circle(1)))",
        fixed: "trait Draw:\n    fn name(self) -> str:\n        return \"shape\"\n\n\
                struct Circle:\n    r: i64\n\n\
                impl Draw for Circle:\n    fn name(self) -> str:\n        return \"circle\"\n\n\
                fn render(d: Draw) -> str:\n    return d.name()\n\n\
                fn main():\n    print(render(Circle(1)))",
        notes: &["Provide an `impl <Trait> for <Type>` covering the trait's methods."],
    },
    Explanation {
        code: "E0416",
        title: "trait not found in scope",
        summary: "A trait name in a bound or `impl` does not refer to any trait \
                  reachable here. It is misspelled or not imported.",
        wrong: "struct S:\n    n: i64\n\nimpl Draww for S:\n    fn f(self):\n        pass\n\n\
                fn main():\n    print(1)",
        fixed: "trait Draw:\n    fn f(self):\n        pass\n\n\
                struct S:\n    n: i64\n\nimpl Draw for S:\n    fn f(self):\n        pass\n\n\
                fn main():\n    print(1)",
        notes: &["Define the trait, fix the spelling, or import the module that declares it."],
    },
    Explanation {
        code: "E0417",
        title: "tuple unpacking length mismatch",
        summary: "A destructuring `let` binds a different number of names than the \
                  tuple on the right has elements.",
        wrong: "fn main():\n    let a, b = (1, 2, 3)",
        fixed: "fn main():\n    let a, b, c = (1, 2, 3)",
        notes: &["Bind exactly as many names as the tuple has elements."],
    },
    Explanation {
        code: "E0418",
        title: "wrong number of fields for variant",
        summary: "A variant pattern binds a different number of fields than the enum \
                  variant actually carries.",
        wrong: "enum Shape:\n    Circle(i64)\n    Rect(i64, i64)\n\n\
                fn area(s: Shape) -> i64:\n    match s:\n        case Circle(r, x):\n            \
                return r\n        case Rect(w, h):\n            return w * h",
        fixed: "enum Shape:\n    Circle(i64)\n    Rect(i64, i64)\n\n\
                fn area(s: Shape) -> i64:\n    match s:\n        case Circle(r):\n            \
                return r\n        case Rect(w, h):\n            return w * h",
        notes: &["Match the field count declared by the variant."],
    },
    Explanation {
        code: "E0419",
        title: "type cannot be destructured by a variant pattern",
        summary: "A variant pattern was used on a value whose type is not an enum, so \
                  it has no variants to destructure.",
        wrong: "fn main():\n    let n = 5\n    match n:\n        case Some(x):\n            print(x)",
        fixed: "fn main():\n    let n = 5\n    match n:\n        case 5:\n            print(n)\n        \
                case _:\n            print(0)",
        notes: &["Use literal, binding, or wildcard patterns for non-enum values."],
    },
    Explanation {
        code: "E0420",
        title: "recursive type contains itself",
        summary: "A type embeds itself by value, which would require infinite storage. \
                  The recursion must go through an indirection that has a fixed size.",
        wrong: "struct Node:\n    value: i64\n    next: Node",
        fixed: "struct Node:\n    value: i64\n    next: [Node]",
        notes: &["Put the recursive field behind an indirection like a list or a pointer."],
    },
    Explanation {
        code: "E0421",
        title: "type is not representable in C",
        summary: "A foreign function declared in a native import uses an Olive managed \
                  type with no C ABI representation: a list, dict, set, tuple, enum, \
                  closure, or Python value.",
        wrong: "import \"libnums.so\" as nums:\n    fn total(values: [i64]) -> i64",
        fixed: "import \"libnums.so\" as nums:\n    fn total(values: *i64, count: i64) -> i64",
        notes: &[
            "Pass a raw pointer and a length, a C struct, or a scalar across the boundary.",
            "A raw `ptr` is always allowed; you own the meaning of the address.",
        ],
    },
    Explanation {
        code: "E0422",
        title: "no such method on this type",
        summary: "A method was called on a built-in type (a list, set, tuple, dict, or \
                  string) that does not define it. These types expose a fixed method \
                  set, so an unknown name cannot resolve.",
        wrong: "fn main():\n    let xs = [1, 2, 3]\n    print(xs.first())",
        fixed: "fn main():\n    let xs = [1, 2, 3]\n    print(xs[0])",
        notes: &["Check the spelling, or use indexing/slicing for element access."],
    },
    Explanation {
        code: "E0425",
        title: "element type cannot alias an `Any` container",
        summary: "A list/set element or dict key/value was inferred as a scalar (an \
                  int, float, bool, null, or Python value) at one use and as `Any` at \
                  another. `Any` stores scalars boxed; a plain scalar does not, so the \
                  same container cannot be both without an explicit `Any` annotation \
                  where it is created.",
        wrong: "fn f() -> [Any]:\n    let mut xs = list_new(1)\n    xs[0] = 1\n    return xs",
        fixed: "fn f() -> [Any]:\n    let mut xs: [Any] = list_new(1)\n    xs[0] = 1\n    return xs",
        notes: &[
            "Annotating at creation binds the element type to `Any` from the start.",
            "Pointer-backed elements (str, list, struct, ...) are unaffected: only \
             scalars change representation inside `Any`.",
        ],
    },
    Explanation {
        code: "E0426",
        title: "type alias refers to itself",
        summary: "A `type Name = ...` alias's target refers back to `Name`, directly \
                  or through another alias. Aliases are pure substitution with no \
                  runtime identity, so there is no indirection to break the cycle.",
        wrong: "type A = B\ntype B = A",
        fixed: "type A = int\ntype B = A | None",
        notes: &[
            "A struct or enum can be self-referential (it has a runtime layout); \
             an alias cannot, since resolving it is just text substitution.",
        ],
    },
    Explanation {
        code: "E0428",
        title: "possibly-`None` value indexed or accessed directly",
        summary: "A `T | None` value was indexed (`x[i]`) or had a field/method \
                  accessed (`x.attr`) without first ruling out `None`. The value may \
                  really be absent at runtime, so the access is rejected until it is \
                  narrowed.",
        wrong: "fn f(xs: [int] | None) -> int:\n    return xs[0]",
        fixed: "fn f(xs: [int] | None) -> int:\n    if xs != None:\n        return xs[0]\n    return -1",
        notes: &[
            "`x?.attr` (with `??` for a default) accesses a field without a \
             preceding narrow.",
            "Narrowing a plain identifier (`if x != None:`) is enough; no cast is needed.",
        ],
    },
    Explanation {
        code: "E0429",
        title: "`enumerate`/`zip` used outside a `for` loop head",
        summary: "`enumerate(...)` and `zip(...)` are `for`-loop desugars, not real \
                  calls: they only exist written directly as a loop's or \
                  comprehension clause's iterable. Assigning one to a variable, \
                  passing it to a function, or using it in any other expression \
                  position has no runtime implementation.",
        wrong: "fn f(xs: [int]):\n    let pairs = enumerate(xs)\n    print(pairs)",
        fixed: "fn f(xs: [int]):\n    for i, x in enumerate(xs):\n        print(i, x)",
        notes: &[
            "The same restriction applies to `zip(a, b)`.",
            "Nest a comprehension's own `for` clause the same way: \
             `[i for i, x in enumerate(xs)]`.",
        ],
    },
    Explanation {
        code: "E0430",
        title: "range step is a literal `0`",
        summary: "A range's `by` step was written as the literal `0`. Such a range \
                  would never advance, so it is rejected at compile time rather \
                  than looping forever (or faulting) at runtime.",
        wrong: "fn f():\n    for i in 0..10 by 0:\n        print(i)",
        fixed: "fn f():\n    for i in 0..10 by 2:\n        print(i)",
        notes: &[
            "A step computed at runtime (not a literal) is checked when the loop \
             starts and faults with E0709 if it comes out to 0.",
        ],
    },
    Explanation {
        code: "E0431",
        title: "stepped range used with `in`",
        summary: "`x in a..b by s` was rejected: membership on a stepped range needs \
                  step-aware arithmetic this check does not perform. `in` only \
                  accepts a plain (unstepped) range.",
        wrong: "fn f(x: int) -> bool:\n    return x in 0..10 by 2",
        fixed: "fn f(x: int) -> bool:\n    return x in 0..10 and x % 2 == 0",
        notes: &[
            "A stepped range still works everywhere else: as a `for`-loop head, \
             in a comprehension, or assigned directly (`let xs = 0..10 by 2`).",
        ],
    },
    Explanation {
        code: "E0432",
        title: "`*` used outside an assignment target",
        summary: "`*name` only means anything as one element of a `let` or plain \
                  assignment's target list (`a, *rest = xs`). Written anywhere else, \
                  there is nothing for it to gather into.",
        wrong: "fn f(xs: [int]):\n    print(*xs)",
        fixed: "fn f(xs: [int]):\n    let first, *rest = xs\n    print(first, rest)",
        notes: &["At most one `*name` is allowed per target list."],
    },
    Explanation {
        code: "E0433",
        title: "cannot infer type of lambda parameter",
        summary: "An unannotated lambda parameter needs either an expected function \
                  type at the use site (a typed variable, a typed call argument) or \
                  usage inside the body that pins its type. Neither happened here.",
        wrong: "fn f():\n    let g = lambda x: x\n    print(g(1))",
        fixed: "fn f():\n    let g = lambda (x: int): x\n    print(g(1))",
        notes: &[
            "A lambda passed directly to a typed parameter (`sorted(xs, key=lambda x: \
             x.name)`) infers from that parameter's declared type instead.",
        ],
    },
];
