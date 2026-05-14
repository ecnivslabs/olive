# Getting Started

Here's how to get Olive up and running on your machine.

## Installation

**Linux and macOS:**

```bash
curl -sSL https://raw.githubusercontent.com/olive-language/olive/main/install.sh | sh
```

This downloads the latest `pit` binary for your OS and places it in `~/.local/bin`. You can override the install location:

```bash
OLIVE_INSTALL_DIR=/usr/local/bin curl -sSL https://raw.githubusercontent.com/olive-language/olive/main/install.sh | sh
```

**Windows:**

Download the binary directly from the [releases page](https://github.com/olive-language/olive/releases/latest) (`pit-windows-x86_64.exe`), rename it to `pit.exe`, and add it to your PATH.

**Verify the install:**

```bash
pit --version
```

## Your First Project

```bash
pit new my_app
cd my_app
```

This creates two files:
- `src/main.liv`: your code
- `pit.toml`: project config

## Running Your Code

```bash
pit run
```

By default, Olive uses hybrid mode. It compiles your code and caches the result in `target/.cache`. On the next run, if nothing changed, it skips compilation and starts immediately.

Other run modes:
- `pit run --jit`: compiles and runs in memory, no cache
- `pit run --aot`: compiles, runs, then deletes the binary
- `pit build`: produces a standalone binary in `target/`

> **Note:** `pit build` requires a C compiler (`cc`) on Linux/macOS. On Windows, AOT builds are not yet supported.

## Hello, World!

`src/main.liv` starts with:

```python
fn main():
    print("Hello from Olive!")

main()
```

Change the message and run `pit run` to see it.

## Interactive Shell

No project needed -- just run:

```bash
pit shell
```

```
>>> let x = 10
>>> print(x * 2)
20
```

## Updating Olive

```bash
pit upgrade
```

This checks for a newer release and replaces the current binary in-place. No need to re-run the install script.

## Other Commands

- `pit test`: run tests
- `pit fmt`: format your code

## Package Management

- `pit add package@version`: add a dependency
- `pit remove package`: remove a dependency
- `pit install`: install all dependencies from `pit.toml`
- `pit publish`: publish your package to the registry

Dependencies go into `.pit_modules/` at your project root.
