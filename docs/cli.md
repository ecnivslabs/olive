# Command Line Interface (pit)

The `pit` toolchain is the unified compiler, package manager, and project management CLI for the Olive programming language.

## Project Management

* `pit new <name>`
  Scaffolds a new Olive project in a directory matching the specified `<name>`. Generates a basic `pit.toml` manifest and `src/main.liv`.

* `pit build [path]`
  Compiles the current project based on the `pit.toml` manifest, or compiles a single `.liv` file if the path points to one.
  * `-o, --output <path>`: Specify the output executable path (only applicable for single file builds).
  * `-t, --time`: Emit performance timings during compilation.
  * `--release`: Compile with optimizations enabled.

* `pit run [file]`
  Compiles and executes the project or a specified file.
  * `-t, --time`: Emit performance timings.
  * `--emit-ast`: Output the Abstract Syntax Tree.
  * `--emit-mir`: Output the Mid-level Intermediate Representation.
  * `--jit`: Execute using the Just-In-Time compiler.
  * `--aot`: Execute using the Ahead-Of-Time compiler.
  * `--hybrid`: Execute using the hybrid compilation model.
  * `--release`: Compile with optimizations before running.

* `pit test`
  Executes the test suite for the current project.
  * `-t, --time`: Emit performance timings.
  * `--release`: Run tests with optimizations enabled.

* `pit fmt [file]`
  Formats the current project or a specified file according to the standard Olive style guidelines.

## Package Management

* `pit add <pod>`
  Adds a specified dependency (pod) to the `pit.toml` manifest and installs it.

* `pit remove <pod>`
  Removes a dependency from the `pit.toml` manifest.

* `pit install`
  Resolves and installs all dependencies declared in the project's `pit.toml`.

* `pit update [pod]`
  Updates a specific pod or all dependencies to their latest compatible versions.

* `pit publish`
  Publishes the current project to the package registry.

## Toolchain

* `pit shell`
  Starts the interactive Read-Eval-Print Loop (REPL) for evaluating Olive expressions.

* `pit upgrade`
  Upgrades the Olive toolchain (compiler and standard library) to the latest stable release.
