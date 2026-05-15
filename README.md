<img width="1452" height="352" alt="olive_logo" src="https://github.com/user-attachments/assets/4e8923b3-0943-4a8f-b288-8abf497b900d" />

<p align="center">
  <a href="https://github.com/olive-language/olive/stargazers">
    <img src="https://img.shields.io/github/stars/olive-language/olive?style=flat-square">
  </a>
  <a href="https://github.com/olive-language/olive/issues">
    <img src="https://img.shields.io/github/issues/olive-language/olive?style=flat-square">
  </a>
  <a href="https://github.com/olive-language/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/olive-language/olive?style=flat-square">
  </a>
  <img src="https://img.shields.io/github/languages/top/olive-language/olive?style=flat-square">
</p>

## Overview

**A general-purpose systems language that's easy to read, fast to run, and keeps your memory safe.**

Olive was built for when you want the speed of a low-level language without the headache of complex syntax. It uses a clean, indentation-based structure and a smart ownership model to provide consistent performance without a garbage collector.

## Why Olive?

- **Clean Syntax**: No braces, no semicolons. Indentation defines the structure, keeping your code readable and consistent.
- **Fearless Safety**: A borrow checker catches memory errors and data races at compile time. No null pointers, no double-frees.
- **Blazing Fast**: Optimized to native code via the Cranelift backend. It's designed to run close to the metal with zero-cost abstractions.
- **Modern Concurrency**: True async/await that's easy to use and extremely efficient.
- **Native Interop**: Interface with C or Rust libraries through a C-compatible ABI with built-in FFI support.
- **Friendly Errors**: When things go wrong, the compiler tells you exactly where and why, with suggestions on how to fix it.

## A Taste of Olive

```python
# A generic function to calculate average
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
    # Spawning an async task
    async:
        await process_data(data)

main()
```

## Getting Started

**Linux and macOS:**

```bash
curl -sSL https://raw.githubusercontent.com/olive-language/olive/master/install.sh | sh
```

**Windows:** download from the [releases page](https://github.com/olive-language/olive/releases/latest).

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
- [Native Interop](docs/ffi.md): Calling C code and using `unsafe`.
- [Standard Library](docs/modules.md): What's in the box.

## Contributing

Contributions are welcome! Fork the repo, make a branch, and open a PR. Keep it simple, keep it clean.
