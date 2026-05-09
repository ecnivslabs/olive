<h1 align="center">Olive</h1>
<p align="center"><em>The Dream Programming Language</em></p>

<p align="center">
  <a href="https://github.com/ecnivs/olive/stargazers">
    <img src="https://img.shields.io/github/stars/ecnivs/olive?style=for-the-badge&color=green">
  </a>
  <a href="https://github.com/ecnivs/olive/issues">
    <img src="https://img.shields.io/github/issues/ecnivs/olive?style=for-the-badge&color=blue">
  </a>
  <a href="https://github.com/ecnivs/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/ecnivs/olive?style=for-the-badge&color=orange">
  </a>
</p>

## Overview

Olive is a modern, high-performance programming language designed for developers who value both expressive clarity and performance. It bridges the gap between high-level productivity and low-level control through an ownership model and a JIT-based compilation pipeline.

Whether you are building low-latency systems, complex backends, or high-performance tools, Olive provides a safe and efficient environment where your code runs as fast as it reads.

## Features

- **Indentation-Based Syntax**: A clean, readable structure that focuses on clarity while maintaining the efficiency of a systems language.
- **Memory Safety**: Ownership-Based Resource Management (OBRM) and a borrow checker with Non-Lexical Lifetimes (NLL) provide safety at compile-time without a garbage collector.
- **JIT-Accelerated Execution**: The Cranelift compilation engine generates optimized machine code on the fly for performance matching native languages.
- **Advanced Optimizations**: A multi-stage MIR optimization pipeline includes Global Value Numbering, Tail-Call Optimization, and Loop-Invariant Code Motion.
- **Detailed Diagnostics**: Colorized, context-aware error reports provide clear feedback and suggestions for resolving issues.
- **Unified Toolchain**: The `pit` tool manages the entire development lifecycle, from project creation to testing and formatting.

## The `pit` Toolchain

`pit` serves as the primary interface for development:

- `pit new <name>`: Initializes a new project.
- `pit run`: Builds, optimizes, and executes projects.
- `pit build`: Performs deep semantic checks and builds the project.
- `pit test`: Executes the test suite and generates reports.
- `pit format`: Standardizes codebase formatting.


## Optimization Pipeline

The compiler (olivc) includes a optimization suite operating on the Middle Intermediate Representation (MIR). These passes are iterative; for example, Constant Propagation may reveal a branch that is always taken, which Simplify CFG can convert into a direct jump, enabling further pruning via Dead Code Elimination.

Detailed information on these transformations is available in the [High-Performance Optimizations](docs/optimizations.md) guide.

## Documentation

Comprehensive guides are available to assist with development:

- [Introduction to Olive](docs/introduction.md)
- [Getting Started](docs/getting_started.md)
- [Ownership and Safety](docs/ownership.md)
- [Basic Syntax and Types](docs/basics.md)
- [Compiler Internals](docs/internals.md)
- [Full Index](docs/index.md)

## Contributing

Feel free to:
1. Fork the repository
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Submit a pull request

#### *I'd appreciate any feedback or code reviews you might have!*