# Introduction to Olive

Olive is a systems-oriented programming language designed for the modern era. It is built for developers who require the uncompromising speed of low-level languages without sacrificing the expressive power, safety, and productivity of high-level environments.

At its heart, Olive provides **deterministic control without the cognitive load**. Instead of relying on a garbage collector that can introduce unpredictable latency, Olive uses **Ownership-Based Resource Management (OBRM)**. This approach allows the compiler to manage memory and resources at compile-time, ensuring that applications are both lean and fast.

## Philosophy

1. **Performance is a Feature**: The goal is that developers shouldn't have to rewrite code in another language just to make it fast. Olive is designed from the ground up to match the performance of C++ and Rust.
2. **Safety is Mandatory**: Memory errors and data races are handled at compile-time. Olive's strict borrow checker ensures that code is safe by construction.
3. **Productivity is Paramount**: A language is only as good as its tooling and syntax. Olive features a clean, readable syntax and a unified toolchain that stays out of the way.

## Key Features

- **Elegant, Indentation-Based Syntax**: Clean, readable code that executes with the efficiency of a systems language.
- **Memory Safety**: A borrow checker with Non-Lexical Lifetimes (NLL) ensures memory errors are caught before execution.
- **JIT-Accelerated Execution**: Leveraging the Cranelift compilation engine, Olive generates optimized machine code on the fly for near-native performance.
- **Advanced MIR Optimizations**: From Loop-Invariant Code Motion to Tail-Call Optimization, the compiler is designed to maximize performance.
- **Detailed Diagnostics**: Context-aware error reports that don't just tell you what went wrong, but show you how to fix it.
- **Unified Toolchain (`pit`)**: A single tool for building, testing, formatting, and managing your projects.

## The Architecture

The Olive compiler pipeline is structured as a modern compilation pipeline:

1. **Lexical Analysis**: Source code is tokenized into meaningful units with support for features like F-strings and SIMD intrinsics.
2. **Parsing**: Organized into a logical Abstract Syntax Tree (AST) using an efficient recursive descent parser.
3. **Semantic Analysis**: Symbol resolution and type checking ensure the integrity of your code's logic and types.
4. **MIR Lowering**: The AST is lowered to a Middle Intermediate Representation (MIR) designed for high-level, CFG-based analysis.
5. **Borrow Checking**: The MIR is analyzed to enforce strict memory safety and ownership rules.
6. **Optimized Codegen**: The MIR passes through an extensive optimization suite before being compiled to machine code via Cranelift.

Olive aims to provide a faster and safer development experience without compromising on performance.
