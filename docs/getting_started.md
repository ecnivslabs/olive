# Getting Started with Olive

## Installation

Olive is currently built from source. You'll need `cargo` installed. Get it from [rustup.rs](https://rustup.rs/) if you don't have it.

1. **Clone the repository**:
   ```bash
   git clone https://github.com/ecnivs/olive.git
   cd olive
   ```

2. **Build**:
   ```bash
   cargo build --release
   ```
   Then copy the binary somewhere on your PATH:
   ```bash
   cp target/release/pit /usr/local/bin/pit
   ```

3. **Verify**:
   ```bash
   pit --help
   ```

## Creating a Project

```bash
pit new my_app
cd my_app
```

This creates:
- `pit.toml`: Project configuration and metadata.
- `src/main.liv`: The entry point for your application.
- `.gitignore`: Pre-configured for Olive development.

## Running Your Code

```bash
pit run
```

Olive finds your entry point, runs the borrow checker, applies optimizations, and executes via the JIT engine.

## Your First Program

Open `src/main.liv` and you'll see:

```python
fn main():
    print("Hello from Olive!")

main()
```

## Interactive Shell

`pit shell` starts an interactive REPL where you can type and run Olive code line by line:

```bash
pit shell
```

```
Olive 0.1.0 (master, ...) on linux
>>> let x = 10
>>> print(x)
10
>>> fn double(n: int) -> int:
...     return n * 2
>>> double(x)
20
```

Function and variable definitions persist across lines for the duration of the session. Type `help` for a list of shell commands, or `quit` / `exit` to leave.

## Useful CLI Flags

- `pit build --time`: Build the project and show a timing breakdown for each compiler phase.
- `pit test`: Find and run all functions marked with `#[test]`.
- `pit fmt`: Format all `.liv` files in the project to the standard Olive style.
- `pit run --emit-mir`: Print the optimized MIR so you can see what the compiler produced before codegen.

## Core Concepts to Know

**Indentation defines blocks.** There are no braces. Consistent indentation is required; the compiler will tell you if it's wrong.

**Variables are immutable by default.** Use `let mut` when you need to reassign. This isn't a restriction so much as a signal in the code: if something is `mut`, it changes; if it isn't, it doesn't.

**Ownership is enforced.** When you assign a value to another variable or pass it to a function, ownership transfers. The original binding can no longer be used. The compiler will catch violations with a clear error message and point to exactly where the problem is.
