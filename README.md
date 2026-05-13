<h1 align="center">Olive</h1>
<p align="center"><em>The Dream Programming Language</em></p>

<p align="center">
  <a href="https://github.com/ecnivs/olive/stargazers">
    <img src="https://img.shields.io/github/stars/ecnivs/olive?style=flat-square"">
  </a>
  <a href="https://github.com/ecnivs/olive/issues">
    <img src="https://img.shields.io/github/issues/ecnivs/olive?style=flat-square"">
  </a>
  <a href="https://github.com/ecnivs/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/ecnivs/olive?style=flat-square">
  </a>
  <img src="https://img.shields.io/github/languages/top/ecnivs/olive?style=flat-square"
</p>

## Overview

Olive is a systems-oriented programming language with a clean, indentation-based syntax, an ownership-based memory model, and a JIT compiler built on Cranelift. It's designed for developers who want safety and performance without sacrificing readable code.

The goal is straightforward: expressive code that runs close to native speed and catches memory errors at compile time, not in production.

## Features

- **Indentation-based syntax**: Blocks are defined by whitespace, keeping code clean and consistent.
- **Memory safety without a GC**: Ownership-Based Resource Management (OBRM) and a borrow checker with Non-Lexical Lifetimes (NLL) catch memory errors at compile time.
- **JIT compilation via Cranelift**: Generates optimized native code at runtime.
- **True stackless async**: An `async`/`await` model backed by a multi-threaded executor. Futures are compiled state machines with no heap allocation per suspension point.
- **MIR optimization pipeline**: Global Value Numbering, Tail-Call Optimization, Loop-Invariant Code Motion, inlining, and more, all running before codegen.
- **Standard library modules**: `math`, `io`, `aio`, `net`, `http`, `random`, loaded dynamically at runtime.
- **Detailed diagnostics**: Colorized, context-aware error reports that point to the problem and suggest a fix.
- **Unified toolchain**: `pit` handles building, running, testing, and formatting.

## The `pit` Toolchain

`pit` is the single entry point for all development tasks:

- `pit new <name>`: Creates a new project with the standard directory layout.
- `pit run`: Runs your project through the full pipeline: borrow checking, optimization, JIT, execution.
- `pit build`: Performs semantic analysis and builds without running.
- `pit test`: Finds and runs all functions marked with `#[test]`.
- `pit fmt`: Formats all `.liv` files to the standard Olive style.
- `pit shell`: Starts an interactive REPL for running Olive code line by line.
- `pit build --time`: Shows a timing breakdown for each compiler phase.
- `pit run --emit-mir`: Prints the optimized MIR so you can see exactly what the compiler produced.

## Optimization Pipeline

The compiler runs an iterative optimization suite on the Middle Intermediate Representation (MIR). These passes compose well: Constant Propagation can expose a branch that's always taken, which Simplify CFG turns into a direct jump, which Dead Code Elimination then prunes. The result is that each pass benefits from the work of the others.

See the [High-Performance Optimizations](docs/optimizations.md) guide for details.

## Documentation

- [Introduction to Olive](docs/introduction.md)
- [Getting Started](docs/getting_started.md)
- [Basic Syntax and Types](docs/basics.md)
- [Functions](docs/functions.md)
- [Enums and Pattern Matching](docs/enums.md)
- [Structs and Composition](docs/structs.md)
- [Ownership and Safety](docs/ownership.md)
- [Async and Concurrency](docs/async.md)
- [High-Performance Optimizations](docs/optimizations.md)
- [Modules and Standard Library](docs/modules.md)
- [Compiler Internals](docs/internals.md)
- [Full Index](docs/index.md)

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/my-feature`)
3. Commit your changes (`git commit -m 'Add my feature'`)
4. Push to the branch (`git push origin feature/my-feature`)
5. Open a pull request

#### *Feedback and code reviews are always welcome.*
