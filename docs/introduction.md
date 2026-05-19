# Introduction to Olive

Olive is a general-purpose systems language. It combines the speed of low-level control with a clean, indentation-based syntax that reads like a scripting language. You compile directly to native code, bypassing the choice between manual byte management and the runtime pauses of garbage collection.

## Philosophy

**Zero-overhead performance.**
Prototype code should be production-ready. Olive produces optimized machine code from the start. There is no heavy runtime, no garbage collector, and no hidden CPU costs in the core language.

**Safety without annotations.**
The compiler infers ownership and frees every value at a known point; no garbage collector, no lifetime syntax. Explicit references are borrow-checked at compile time, and anything the analysis cannot prove is validated at runtime by generation checks that stop the program with a precise fault instead of corrupting memory.

**Readability first.**
Code is read more often than it is written. Olive strips syntactic noise: no semicolons, no braces, no boilerplate. Program structure is defined entirely by indentation.

These principles, and the rest of Olive's creed, are its laws. Run `import olive` to read them.

## Core Concepts

- **Inferred Ownership**: Olive tracks memory statically. The compiler infers which variable owns each value, frees it at its last use, and copies where two places would otherwise share; you write code as if everything were a value. See [Ownership](ownership.md).
- **The Pit Toolchain**: Built to be fast. `pit` manages dependency resolution, builds, testing, and benchmarking, with compile times measured in milliseconds.
- **Structured Concurrency**: Writing high-performance, concurrent applications should be straightforward. Olive provides built-in `async`/`await` primitives that behave exactly like synchronous code.
- **C / Rust Interop (FFI)**: Olive integrates with the existing system ecosystem. You can import C and C++ libraries directly with no translation layers or foreign function wrappers.
- **Python Interoperability**: Call any Python library (like NumPy or PyTorch) dynamically with bidirectional, zero-copy collection sharing.

## Compilation Pipeline

Running an Olive program executes the following compiler stages:

1. **Analysis**: The front-end parses code, resolves symbols, and checks types.
2. **Borrow Checking**: Explicit references (`&`, `&mut`) are validated against the aliasing rules.
3. **Ownership Inference and Optimization**: On the Middle Intermediate Representation (MIR), the compiler infers ownership, places frees, and runs the optimization pipeline.
4. **Codegen**: The Cranelift backend generates machine code for your CPU. `pit run` JIT-compiles and executes immediately, upgrading hot functions while the program runs; `pit build` produces a native executable.
