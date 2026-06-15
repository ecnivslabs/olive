<img width="1452" height="352" alt="olive_logo" src="https://github.com/user-attachments/assets/4e8923b3-0943-4a8f-b288-8abf497b900d" />

<p align="center">
  <a href="https://github.com/ecnivs-labs/olive/stargazers">
    <img src="https://img.shields.io/github/stars/ecnivs-labs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivs-labs/olive/issues">
    <img src="https://img.shields.io/github/issues/ecnivs-labs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivs-labs/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/ecnivs-labs/olive?style=flat-square">
  </a>
  <img src="https://img.shields.io/github/languages/top/ecnivs-labs/olive?style=flat-square">
</p>

## Overview

**A general-purpose systems language that's easy to read, fast to run, and keeps your memory safe.**

Olive was built for when you want the speed of a low-level language without the headache of complex syntax. It uses a clean, indentation-based structure and a smart ownership model to provide consistent performance without a garbage collector.

## Why Olive?

- **Clean Syntax**: No braces, no semicolons. Indentation defines the structure, keeping your code readable and consistent.
- **Fearless Safety**: A borrow checker catches memory errors and data races at compile time. No null pointers, no double-frees.
- **Blazing Fast**: Optimized to native code via the Cranelift backend. It's designed to run close to the metal with zero-cost abstractions.
- **Modern Concurrency**: True async/await that's easy to use and extremely efficient.
- **C / Rust Interop**: Interface with C or Rust libraries through a C-compatible ABI with built-in FFI support.
- **Python Interop**: Import any Python module directly and pass native collections with zero-copy bidirectional proxies.
- **Friendly Errors**: When things go wrong, the compiler tells you exactly where and why, with suggestions on how to fix it.

## A Taste of Olive

```rust
// A generic function to calculate average
fn average[T: Numeric](numbers: [T]) -> float:
    let mut total = 0.0
    for n in numbers:
        total += float(n)
    return total / float(len(numbers))

async fn process_data(data: [int]):
    print(f"Processing {len(data)} items...")
    let avg = average(data)
    print(f"Result: {avg:.2f}")

fn main():
    let data = [10, 20, 30, 40, 50]
    // Spawning an async task
    async:
        await process_data(data)
```

## Getting Started

**Linux and macOS:**

```bash
curl -sSL https://raw.githubusercontent.com/ecnivs-labs/olive/master/install.sh | sh
```

**Windows:** download from the [releases page](https://github.com/ecnivs-labs/olive/releases/latest).

Then:

```bash
pit new my_app
cd my_app
pit run
```

## Documentation

- [Introduction](docs/introduction.md): Philosophy and goals.
- [Basics](docs/basics.md): Variables, types, and control flow.
- [Ownership](docs/ownership.md): How memory safety works.
- [Generics](docs/generics.md): Writing reusable code.
- [Traits](docs/traits.md): Defining shared behavior between types.
- [C / Rust Interop (FFI)](docs/ffi.md): Calling C or Rust code and using `unsafe`.
- [Python Interop](docs/python.md): High-performance zero-copy Python integration.
- [Standard Library](docs/modules.md): What's in the box.
- [Full Index](docs/index.md): Everything in one place.

## Contributing

Contributions are welcome! Fork the repo, make a branch, and open a PR. Keep it simple, keep it clean.
