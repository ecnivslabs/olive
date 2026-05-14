<img width="1452" height="352" alt="olive_logo" src="https://github.com/user-attachments/assets/4e8923b3-0943-4a8f-b288-8abf497b900d" />

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

# Olive

**A systems language that is easy to write, fast to run, and keeps your memory safe.**

Olive is for when you want the speed of a systems language without the headache of complex syntax. It uses a clean, indentation-based structure and a smart memory model to give you consistent speed and stability without needing a garbage collector.

## Why Olive?

- **Clean Syntax**: No braces, no semicolons. Indentation defines the structure, keeping your code readable and consistent.
- **Safety by Default**: A borrow checker catches memory errors and data races at compile time. No crashes in production because of a null pointer or a double-free.
- **Blazing Fast**: Optimized to native code at runtime via a JIT compiler. It's designed to run close to the metal.
- **Modern Concurrency**: True async/await that's easy to use and extremely efficient.
- **Smart Speed**: Olive remembers your code. The first run is fast, but the second is instant because of built-in caching.
- **Friendly Errors**: When things go wrong, the compiler tells you exactly where and why, with suggestions on how to fix it. No more cryptic errors.

## A Taste of Olive

```python
fn calculate_stats(numbers: list[int]) -> (int, float):
    let mut total = 0
    for n in numbers:
        total += n
    
    let average = float(total) / float(len(numbers))
    return (total, average)

fn main():
    let data = [10, 20, 30, 40, 50]
    let (sum, avg) = calculate_stats(data)
    print(f"Total: {sum}, Average: {avg}")

main()
```

## Getting Started

1. **Install**: Follow the [Installation Guide](docs/getting_started.md).
2. **New Project**: `pit new my_app`
3. **Build**: `pit build`
4. **Run**: `pit run`

## Documentation

- [Introduction](docs/introduction.md): Philosophy and goals.
- [Basics](docs/basics.md): Variables, types, and control flow.
- [Ownership](docs/ownership.md): How memory safety works.
- [Async](docs/async.md): Concurrent programming.
- [Standard Library](docs/modules.md): What's in the box.

## Contributing

We love help! Fork the repo, make a branch, and open a PR. Keep it simple, keep it clean.