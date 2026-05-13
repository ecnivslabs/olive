# Compiler Internals

The Olive compiler (`olivc`) transforms source code into optimized native machine code through a sequence of representations. Each stage is designed to run fast. The target from `pit run` to execution is milliseconds, not seconds.

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

The borrow checker enforces memory safety on the MIR CFG without a garbage collector.

- **Liveness analysis**: Computes which variables are live at each program point.
- **Dataflow tracking**: Follows references back to their origin to verify aliasing and mutation rules.
- **NLL (Non-Lexical Lifetimes)**: Borrows end when the reference is last used, not at the end of the lexical scope. This avoids false positives that would force unnecessary code restructuring.
- **Ownership rules**: Enforces single ownership and exclusive mutable borrowing.

## 6. Codegen & JIT Runtime (`codegen/`)

The final stage compiles MIR to native machine code through Cranelift.

- **SSA generation**: MIR is converted to Static Single Assignment form, which is Cranelift's native input format.
- **Intrinsics**: The JIT runtime provides optimized intrinsics for memory allocation, string operations, SIMD, and built-in standard library calls.
- **Standard library**: Built-in runtime symbols (`math`, `io`, `aio`, `net`, `http`, `random`) are resolved from a dynamically loaded shared library rather than being baked into the JIT. This keeps startup fast and the binary lean.
- **Execution**: Compiled functions are loaded into executable memory and invoked directly by the Olive runtime.

## Error Reporting & Diagnostics

Olive uses the `ariadne` crate for error formatting. Each diagnostic includes:

1. **Error code and message**: A clear description of what went wrong.
2. **Source snippet**: The relevant code with markers pointing to the exact location.
3. **Help text**: A suggestion for how to fix it, derived from semantic analysis.

## Performance Approach

Every stage is written to minimize algorithmic complexity. Passes are generally linear in the size of the input. The goal is that the overhead of running the compiler is small enough to be invisible in the development loop.
