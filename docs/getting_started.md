t# Getting Started

Ready to try Olive? Here's how to get up and running.

## Installation

Right now, Olive is built from source. You'll need to have the Rust toolchain installed. If you don't have it, you can find instructions at [rustup.rs](https://rustup.rs/).

1. **Grab the code**:
   ```bash
   git clone https://github.com/ecnivs/olive.git
   cd olive
   ```

2. **Build and install**:
   ```bash
   cargo build --release
   cp target/release/pit /usr/local/bin/pit
   ```

3. **Check it works**:
   ```bash
   pit --help
   ```

## Your First Project

Creating a new Olive project is easy with the `pit` tool:

```bash
pit new my_app
cd my_app
```

This sets up everything you need:
- `src/main.liv`: Where your code lives.
- `pit.toml`: Settings for your project.

## Running Your Code

To run your app, just type:

```bash
pit run
```

### How it runs

By default, Olive uses a **Hybrid** mode. It compiles your code once and saves a copy in `target/.cache`. The next time you run it, if the code hasn't changed, it skips the compiling part and starts immediately.

If you want to control how it runs, you can use these flags:

- `pit run --jit`: Runs your code directly in memory every time. Good for quick tests.
- `pit run --aot`: Compiles your code, runs it, and then cleans up.
- `pit build`: Builds a permanent, standalone binary file you can share with others.

## Hello, World!

Open `src/main.liv` and you'll see a simple starting point:

```python
fn main():
    print("Hello from Olive!")

main()
```

Try changing the message and running `pit run` again!

## The Interactive Shell

If you just want to play around with some code without creating a project, you can use the interactive shell:

```bash
pit shell
```

It's a great way to test out ideas or learn the syntax:

```
>>> let x = 10
>>> print(x * 2)
20
```

## Useful Tools

- `pit test`: Runs your tests.
- `pit fmt`: Automatically formats your code so it looks clean and consistent.

## Package Management

Olive has a built-in package manager to handle your dependencies:

- `pit add package@version`: Add a dependency to your project.
- `pit remove package`: Remove a dependency.
- `pit install`: Install all dependencies listed in `pit.toml`.
- `pit publish`: Share your package with the world by publishing it to the registry.

Dependencies are stored in the `.pit_modules/` directory at your project root.

