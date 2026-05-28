# Compiler Internals

The Olive compiler transforms source code into optimized native machine code through a sequence of representations. The architecture prioritizes low compilation latency; the pipeline from invocation to execution typically completes in milliseconds.

## 1. Lexical Analysis (`lexer/`)

The lexer converts raw UTF-8 source into a stream of structured tokens.

- **DFA-based**: Uses a deterministic finite automaton for consistent, high-speed tokenization.
- **Indentation tracking**: Olive uses whitespace for block structure. The lexer maintains an indentation stack and emits `INDENT` and `DEDENT` tokens, so the parser doesn't have to deal with whitespace directly.
- **F-strings**: Interpolated strings are split into alternating constant and expression tokens during lexing, so the parser can handle the embedded expressions naturally.

## 2. Parsing (`parser/`)

The parser consumes the token stream and produces an Abstract Syntax Tree (AST).

- **Recursive descent**: A handwritten recursive descent parser provides good error recovery and lets the compiler emit useful diagnostics when the input is malformed.
- **Pratt parsing for expressions**: Expressions use a Pratt (top-down operator precedence) approach to handle complex operator hierarchies, including distinctions like the walrus operator `:=` vs. assignment `=`.

## 3. Semantic Analysis (`semantic/`)

This stage verifies the program's structure and types.

- **Name resolution**: Builds a hierarchy of symbol tables, handling shadowing, nested scopes, and module-level visibility. The `_` prefix convention for private names is enforced here.
- **Type inference**: Olive uses a Hindley-Milner-inspired type system with unification. Types are static, but annotations are often optional; the compiler infers them from usage.
- **Method resolution**: Dispatches method calls to the correct `impl` block implementation.

## 4. Middle Intermediate Representation (MIR)

MIR is the central representation in the compiler. It models the program as a Control Flow Graph (CFG) where each node is a basic block.

- **Basic blocks**: A sequence of statements with no internal jumps. Execution enters at the top and exits at the bottom.
- **Terminators**: Every block ends with exactly one terminator: `Goto`, `SwitchInt`, `Return`, or `Unreachable`. This makes control flow explicit and easy to analyze.
- **Lowering**: High-level constructs (`for` loops, comprehensions, `with` statements) are lowered into simple assignments and jumps before any optimization runs.
- **Argument packing**: Named, variadic, and keyword arguments are packed into their final forms at the MIR level.

## 5. Borrow Checking (`borrow_check/`)

The borrow checker validates explicit references (`&`, `&mut`) on the MIR CFG.

- **Liveness analysis**: Computes which variables are live at each program point.
- **Aliasing rules**: Many readers or one writer, never both. Violations are compile errors (`E0503` and friends).
- **NLL (Non-Lexical Lifetimes)**: Borrows end when the reference is last used, not at the end of the lexical scope.

Plain assignments and function arguments are not checked here; those are handled by ownership inference below, which keeps them safe without rejecting programs.

## 6. Ownership Inference (`mir/optimizations/ownership/`)

Olive has no garbage collector. The builder lowers every use of a heap value as a copy and every scope exit as a drop; this pass then reclassifies each local as an owner, a view, or dynamically owned, and rewrites the drops to match.

- **Whole-program summaries**: A fixpoint analysis computes which function parameters escape into longer-lived storage and which functions may return a borrow. Callers use these summaries to decide moves and copies.
- **Move promotion**: A copy whose source is dead afterwards becomes a move; the stale drop is removed.
- **Copy on escape**: A value stored into a container while still live elsewhere is deep-copied at the store, so no value ever has two owners. See [Ownership](ownership.md) for the user-facing semantics.
- **Reassignment frees**: A sole owner that gets reassigned frees the old value first instead of leaking it.
- **Drop guards**: Locals whose ownership depends on the path taken get a shadow flag; their drops test it at runtime.

## 7. Generation Checks (`mir/optimizations/gencheck/`)

The runtime backstop for what inference cannot prove. Heap objects live in generational slab allocators (`std_lib`): every slot carries a generation counter that increments on allocation and free. This pass runs after all other optimizations and inserts checks on suspect borrows: the generation is captured at borrow time and re-validated before uses a free could precede. A failed check aborts with `E0707` and a source caret. Checks are elided when a forward analysis proves no free can intervene, so well-typed hot paths pay nothing. When a must-free lattice proves staleness is certain on every path through the function, the site is promoted to a compile-time error (`E0708`) instead of a runtime check.

## 8. Codegen (`codegen/`)

The final stage compiles MIR to native machine code through Cranelift.

- **SSA generation**: MIR is converted to Static Single Assignment form, Cranelift's native input format.
- **Two backends**: `pit run` JIT-compiles into executable memory and runs immediately; `pit build` emits object files through the same MIR pipeline and links a native executable.
- **Standard library**: Runtime symbols (`math`, `io`, `aio`, `net`, `requests`, `random`, allocators) are resolved from a dynamically loaded shared library rather than baked into the JIT. This keeps startup fast and the binary lean.

## 9. JIT Tiering and Profiling

The JIT starts every function at a fast-to-compile tier and upgrades hot code while the program runs.

- **Dispatch cells**: Calls go through a per-function pointer cell, so a recompiled body can be swapped in without patching call sites.
- **Hot counts**: Per-function entry counters decide when a function is worth retiering.
- **Any-op specialization**: Call sites record the runtime kinds flowing through `Any` arithmetic. A site that has only seen ints graduates to a guarded fast path: inline integer ops with a fallback branch for anything else.
- **PGO**: `pit build` can consume a profile from a previous run to make the same decisions ahead of time.

## Error Reporting & Diagnostics

Olive uses the `ariadne` library for error formatting. Each diagnostic includes:

1. **Error code and message**: A clear description of what went wrong.
2. **Source snippet**: The relevant code with markers pointing to the exact location.
3. **Help text**: A suggestion for how to fix it, derived from semantic analysis.

`pit explain <code>` prints a longer explanation with a wrong and a fixed example for every code, including the runtime faults (`E0700` and up).

## Performance Approach

Every stage is written to minimize algorithmic complexity. Passes are generally linear in the size of the input. The goal is that the overhead of running the compiler is small enough to be invisible in the development loop.
