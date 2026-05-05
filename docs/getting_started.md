# Getting Started

This guide covers installing Olive and running your first program.

## Installation

### Linux and macOS

Install the `pit` toolchain (the all-in-one compiler and package manager) using the installer script:

```bash
curl -sSL https://raw.githubusercontent.com/ecnivs-labs/olive/master/install.sh | sh
```

### Windows

1. Download `pit-windows-x86_64.exe` from the [releases page](https://github.com/ecnivs-labs/olive/releases/latest).
2. Rename the binary to `pit.exe`.
3. Add the directory containing the binary to your system `PATH`.

### Verify Installation

Verify that the toolchain is accessible from your shell:

```bash
pit --version
```

## Creating a Project

`pit` manages project creation, compilation, testing, and dependency resolution. To scaffold a new project:

```bash
pit new my_app
cd my_app
```

This generates a minimal project structure:
- `src/main.liv`: The primary source file.
- `pit.toml`: The project configuration and dependency manifest.

## Running the Code

To compile and run your application:

```bash
pit run
```

The compiler builds your source code, caching the intermediate build artifacts. Subsequent runs load the cached executable, starting instantly.

## Hello, World!

Open `src/main.liv`. It contains a basic entry point:

```rust
fn main():
    print("Hello from Olive!")
```

Modify the string and execute `pit run` to see the changes.

## Interactive Shell (REPL)

To evaluate expressions or test snippets without creating a project:

```bash
pit shell
```

## Upgrading the Toolchain

To update to the latest compiler and standard library release:

```bash
pit upgrade
```

## Package Management (Pods)

Olive packages are called **pods**. You declare them in your project manifest:

* `pit add pod_name`: Adds the specified pod to `pit.toml`.
* `pit install`: Downloads and installs all dependencies.

All dependencies are resolved and stored in the local `.pit_pods/` directory, ensuring builds are self-contained.

