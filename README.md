<h1 align="center">Olive</h1>
<p align="center"><em>The Dream Programming Language</em></p>

<p align="center">
  <a href="https://github.com/ecnivs/olive/stargazers">
    <img src="https://img.shields.io/github/stars/ecnivs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivs/olive/issues">
    <img src="https://img.shields.io/github/issues/ecnivs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivs/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/ecnivs/olive?style=flat-square">
  </a>
  <img src="https://img.shields.io/github/languages/top/ecnivs/olive?style=flat-square">
</p>

## Overview

Olive is a modern, high-performance programming language designed for developers who value both expressive clarity and uncompromising performance. It bridges the gap between high-level productivity and low-level control through an innovative ownership model and a cutting-edge JIT-based compilation pipeline.

Whether you are building low-latency systems, complex backends, or high-performance tools, Olive provides a safe and efficient environment where your code runs as fast as it reads.


## The `pit` Toolchain

Olive comes with a built-in package manager and build tool designed to streamline your workflow:

- `pit new <name>`: Scaffolds a new project with a standard structure and configuration.
- `pit run`: Builds and executes your project instantly.
- `pit build`: Compiles your project and performs deep semantic checks.
- `pit test`: Runs your test suite, discovering `@test` functions across your codebase.
- `pit format`: Keeps your code clean and consistent with an automated formatter.

## High-Performance Optimizations

The Olive compiler (olivc) doesn't just run your code; it refines it. Our MIR-level optimization suite includes:

- **Aggressive Inlining**: Reduces function call overhead for hot code paths.
- **Constant Folding**: Computes static values at compile-time.
- **Dead Code Elimination (DCE)**: Prunes unreachable logic to keep your binaries lean.
- **Copy Propagation**: Optimizes variable usage for better register allocation.

## Documentation

Dive deeper into the world of Olive:

- [Introduction](docs/introduction.md)
- [Getting Started](docs/getting_started.md)
- [Ownership and Safety](docs/ownership.md)
- [Basic Syntax and Types](docs/basics.md)
- [Full Index](docs/index.md)

## 🙌 Contributing

Feel free to:
1. Fork the repository
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Submit a pull request

#### *I'd appreciate any feedback or code reviews you might have!*
