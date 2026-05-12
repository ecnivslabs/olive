# Getting Started with Olive

Follow these steps to set up the Olive toolchain and start building high-performance applications.

## Installation

Currently, Olive is built from source. Ensure you have the [Rust toolchain](https://rustup.rs/) installed on your machine.

1. **Clone the repository**:
   ```bash
   git clone https://github.com/ecnivs/olive.git
   cd olive
   ```

2. **Build and Install**:
   ```bash
   cargo build --release
   ```
   For convenience, you can add the binary to your path:
   ```bash
   cp target/release/pit /usr/local/bin/pit
   ```

3. **Verify the installation**:
   ```bash
   pit --help
   ```

## Creating Your First Project

Olive features a built-in package manager that makes starting a new project effortless.

1. **Initialize a new project**:
   ```bash
   pit new my_app
   cd my_app
   ```

This creates a standard project structure:
- `pit.toml`: Your project's configuration and metadata.
- `src/main.liv`: The entry point for your application.
- `.gitignore`: Pre-configured for Olive development.

## Running Your Code

Inside your project directory, simply run:
```bash
pit run
```
Olive will automatically find your entry point, perform optimizations, run the borrow checker, and execute the code via the JIT engine.

## Writing Your First Program

Open `src/main.liv` and you'll see a basic function:

```python
fn main():
    print("Hello from Olive!")

main()
```

### Advanced CLI Options

The `pit` toolchain provides powerful flags for developers:

- `pit build --time`: Build the project and show detailed timing reports for optimization, borrow checking, and codegen.
- `pit test`: Automatically find and run all functions decorated with `#[test]`.
- `pit format`: Format all files in your project to match the standard Olive style.
- `pit run --emit-mir`: View the Middle Intermediate Representation of your code to see the optimizer in action.

## Core Concepts

As you start writing more Olive, keep these concepts in mind:

- **Indentation**: Blocks are defined by whitespace. Keep it clean!
- **Ownership**: Olive uses Ownership-Based Resource Management (OBRM). If you move a value, it's gone from the original location.
- **Strict Safety**: The compiler is your partner. If it identifies a potential memory issue, it will provide a detailed diagnostic to guide you.
