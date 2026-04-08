# Introduction to Olive

Olive is a general-purpose systems language. It combines the speed of low-level control with a clean, indentation-based syntax that reads like a scripting language. You compile directly to native code, bypassing the choice between manual byte management and the runtime pauses of garbage collection.

## Philosophy

**Zero-overhead performance.**
Prototype code should be production-ready. Olive produces optimized machine code from the start. There is no heavy runtime, no garbage collector, and no hidden CPU costs.

**Compile-time safety.**
Memory leaks and data races are caught at compile time. If the compiler accepts the code, it is memory-safe.

**Readability first.**
Code is read more often than it is written. Olive strips syntactic noise: no semicolons, no braces, no boilerplate. Program structure is defined entirely by indentation.

## Core Concepts

- **Ownership and Borrowing**: Olive tracks memory statically. The compiler knows exactly when a resource goes out of scope and frees it immediately, avoiding garbage collection overhead.
- **The Pit Toolchain**: Built to be fast. `pit` manages dependency resolution, builds, testing, and benchmarking, with compile times measured in milliseconds.
- **Structured Concurrency**: Writing high-performance, concurrent applications should be straightforward. Olive provides built-in `async`/`await` primitives that behave exactly like synchronous code.
- **C / Rust Interop (FFI)**: Olive integrates with the existing system ecosystem. You can import C and C++ libraries directly with no translation layers or foreign function wrappers.
- **Python Interoperability**: Call any Python library (like NumPy or PyTorch) dynamically with bidirectional, zero-copy collection sharing.

## Compilation Pipeline

Running an Olive program executes the following compiler stages:

1. **Analysis**: The front-end parses code, resolves symbols, and checks types.
2. **Borrow Checking**: The compiler validates that memory references follow strict access rules, preventing data races and invalid pointer access.
3. **Optimization**: Redundant operations are eliminated, loops are hoisted, and blocks are simplified on the Middle Intermediate Representation (MIR).
4. **JIT Codegen**: The Cranelift backend generates machine code optimized for your local CPU architecture, executing it immediately.


