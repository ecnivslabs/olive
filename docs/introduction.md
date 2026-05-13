# Introduction to Olive

Olive is a systems-oriented programming language built for developers who need real performance and memory safety, but don't want to give up readable code to get there.

The syntax is indentation-based, expressive, and easy to scan. The memory model is ownership-based, enforced through a compiler that tries to give useful error messages rather than cryptic ones. Under the hood, a JIT compiler built on Cranelift turns your code into optimized native instructions.

## Philosophy

**Performance is not optional.** Olive doesn't ask you to prototype in one language and rewrite in another. The compiler is designed to produce fast code from the start, through an iterative MIR optimization pipeline, JIT compilation, and SIMD vectorization where it's safe to do so.

**Safety is enforced at compile time.** Memory errors and data races are caught before the program runs, not in production. The borrow checker is part of the language model, not an optional tool.

**Readability is a design constraint.** If the code is hard to read, it's harder to reason about, harder to review, and harder to maintain. Olive's syntax is deliberately clean. The toolchain handles formatting so the style question is settled from day one.

## Key Features

- **Indentation-based syntax**: Block structure is defined by whitespace, keeping noise low.
- **Memory safety**: A borrow checker with Non-Lexical Lifetimes (NLL) catches errors at compile time without a garbage collector.
- **JIT compilation**: The Cranelift backend generates optimized native code at runtime, measured in milliseconds from `pit run` to execution.
- **True stackless async**: `async fn` and `await` backed by a multi-threaded, state-machine executor with no heap allocation per suspension point.
- **MIR optimization pipeline**: Loop-Invariant Code Motion, Tail-Call Optimization, Global Value Numbering, inlining, and more.
- **Detailed diagnostics**: Error reports that show the relevant code, point to the exact location, and suggest how to fix it.
- **Unified toolchain (`pit`)**: Build, run, test, and format from one tool.

## The Compiler Pipeline

Olive compiles through a sequence of representations, each suited to a different kind of analysis:

1. **Lexical Analysis**: Source code is tokenized. The lexer handles indentation tracking, F-strings, and SIMD intrinsics.
2. **Parsing**: Tokens become an Abstract Syntax Tree using a handwritten recursive descent parser.
3. **Semantic Analysis**: Symbol resolution and type checking verify the structure and types of your program.
4. **MIR Lowering**: The AST is lowered to a Control Flow Graph-based Middle Intermediate Representation, designed for analysis and optimization.
5. **Borrow Checking**: The MIR is analyzed to enforce ownership and aliasing rules.
6. **Optimization**: An iterative suite of passes transforms the MIR before codegen.
7. **Codegen**: The optimized MIR is compiled to native machine code via Cranelift.
