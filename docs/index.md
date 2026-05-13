# Olive Documentation

This is the reference documentation for the Olive programming language.

Olive is a systems-oriented language with a clean, indentation-based syntax, ownership-based memory safety, and a JIT compiler built on Cranelift. It's meant for developers who need real performance and safety guarantees without abandoning readability.

## Documentation Sections

- [**Introduction**](introduction.md): The core philosophy and design goals behind Olive.
- [**Getting Started**](getting_started.md): Install the compiler and run your first program.
- [**Basic Syntax and Types**](basics.md): Variables, types, control flow, and comprehensions.
- [**Functions**](functions.md): Defining functions, parameter types, decorators, and directives.
- [**Enums and Pattern Matching**](enums.md): Enum variants with data, `match`, wildcards, and pattern bindings.
- [**Structs and Composition**](structs.md): Struct definitions, `impl` blocks, and composition over inheritance.
- [**Ownership and Safety**](ownership.md): OBRM, borrowing, NLL, and how the borrow checker works.
- [**Async and Concurrency**](async.md): `async fn`, `await`, `gather`, `select`, `cancel`, and the stackless executor.
- [**High-Performance Optimizations**](optimizations.md): The MIR optimization pipeline and JIT startup model.
- [**Modules and Standard Library**](modules.md): Module imports, visibility rules, and the built-in standard library.
- [**Compiler Internals**](internals.md): How the compiler pipeline works from source to machine code.

---

*Olive is in active development. Contributions and bug reports are welcome.*
