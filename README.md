<img width="1452" height="352" alt="olive_logo" src="https://github.com/user-attachments/assets/4e8923b3-0943-4a8f-b288-8abf497b900d" />

<p align="center">
  <a href="https://github.com/ecnivslabs/olive/stargazers">
    <img src="https://img.shields.io/github/stars/ecnivslabs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivslabs/olive/issues">
    <img src="https://img.shields.io/github/issues/ecnivslabs/olive?style=flat-square">
  </a>
  <a href="https://github.com/ecnivslabs/olive/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/ecnivslabs/olive?style=flat-square">
  </a>
  <img src="https://img.shields.io/github/languages/top/ecnivslabs/olive?style=flat-square">
</p>

[Olive](https://olive.ecnivs.com) is an indentation-based systems programming language. It compiles directly to native code via Cranelift, manages memory through compile-time inferred ownership without a garbage collector or lifetime annotations, and provides direct interop with C, Rust, and Python.

## At a Glance

### 1. Inferred Ownership

Every heap value has a single owner. Storing a value moves it when finished, or creates an independent copy when still in use. No hidden sharing, no lifetime annotations.

```rust
fn main():
    let mut a = [1, 2]
    let b = [a]      // b stores its own copy of a
    a[0] = 99
    print(a[0])      // 99
    print(b[0][0])   // 1, unchanged
```

### 2. Error Handling & Pattern Matching

Functions return values or error enums with `|`. The `?` operator propagates errors, and `match` handles variants.

```rust
enum ParseError:
    Invalid(str)

fn parse_port(s: str) -> int | ParseError:
    let n = int(s)
    if n <= 0 or n > 65535:
        return Invalid(s)
    return n

fn main():
    let inputs = ["8080", "99999"]
    for raw in inputs:
        match parse_port(raw):
            port:
                print(f"Valid port: {port}")
            Invalid(bad):
                print(f"Invalid port: {bad}")
```

### 3. Async & Concurrency

Asynchronous tasks run on a cooperative event loop with share-nothing task boundaries.

```rust
async fn fetch_count(id: int) -> int:
    return id * 10

fn main():
    let task = async:
        await fetch_count(42)
    let result = await task
    print(f"Result: {result}")
```

### 4. Python Interop

Import Python modules directly with automatic `.pyi` type introspection and zero-copy collection proxies.

```rust
fn main():
    import py "math" as math
    let val = math.sqrt(64.0)
    print(f"Square root: {val}")
```

### 5. C & Rust FFI

Call native C and Rust shared libraries directly through the C ABI within `unsafe` blocks.

```rust
import "libc.so.6" as libc:
    fn puts(s: str) -> int

fn main():
    unsafe:
        libc.puts("Hello from libc!")
```

### 6. Compiler Diagnostics

Errors pinpoint source locations with carets and built-in remediation via `pit explain`.

```text
[E0503] Error: cannot borrow `list` as immutable
   ╭─[ src/main.ol:4:14 ]
   │
 4 │     let r2 = &list
   │              ──┬──  
   │                ╰──── already borrowed as mutable here
   │ 
   │ Help 1: end the mutable borrow before taking a shared borrow
   │ Help 2: run `pit explain E0503` for a detailed explanation
───╯
```

## Getting Started

**Linux and macOS:**

```bash
curl -sSL https://raw.githubusercontent.com/ecnivslabs/olive/master/install.sh | sh
```

**Windows:** download from the [releases page](https://github.com/ecnivslabs/olive/releases/latest).

Then:

```bash
pit new my_app
cd my_app
pit run
```

## Documentation

- [Introduction](docs/introduction.md): Philosophy and goals.
- [Basics](docs/basics.md): Variables, types, and control flow.
- [Functions](docs/functions.md): Grouping code into reusable blocks.
- [Ownership](docs/ownership.md): How memory safety works.
- [Generics](docs/generics.md): Writing reusable code.
- [Traits](docs/traits.md): Defining shared behavior between types.
- [C / Rust Interop (FFI)](docs/ffi.md): Calling C or Rust code and using `unsafe`.
- [Python Interop](docs/python.md): Typed Python integration with automatic `.pyi` stub introspection.
- [Standard Library](docs/modules.md): What's in the box.
- [Full Index](docs/index.md): Everything in one place.

## Contributing

Contributions are welcome! Fork the repo, make a branch, and open a PR. Keep it simple, keep it clean.
