# Command Line Interface (pit)

The `pit` toolchain is the unified compiler, package manager, and project management CLI for the Olive programming language.

## Project Management

* `pit new <name>`
  Scaffolds a new Olive project in a directory matching the specified `<name>`. Generates a `pit.toml` manifest, `src/main.liv`, and a `.gitignore`, then initializes a git repository in the project directory.
  * `--lib`: Scaffold a library pod instead of a binary. Generates `src/lib.liv` (no `fn main()`) and sets `entry` in `pit.toml` to `src/lib.liv`, so other pods can depend on it and `import <name>` resolves straight to it.

* `pit build [path]`
  Compiles the current project based on the `pit.toml` manifest, or compiles a single `.liv` file if the path points to one.
  * `-o, --output <path>`: Specify the output executable path (only applicable for single file builds).
  * `-t, --time`: Emit performance timings during compilation.
  * `--release`: Compile with optimizations enabled.
  * `--pgo <path>`: Use the profile at `<path>` for profile-guided inlining instead of auto-detecting one captured by an earlier `pit run`.
  * `--pymodule`: Compile project into a C-compatible Python extension module shared library (`.so`, `.dylib`, or `.pyd`).
  * `--module-name <name>`: Override the output Python extension module name (defaults to project `name` in `pit.toml`).
  * `--explain-copies`: Print every compiler-inserted deep-copy site with file:line, type, and reason.

* `pit run [file]`
  Compiles and executes the project or a specified file.
  * `-t, --time`: Emit performance timings.
  * `--emit-ast`: Output the Abstract Syntax Tree.
  * `--emit-mir`: Output the Mid-level Intermediate Representation.
  * `--jit`: Execute using the Just-In-Time compiler.
  * `--aot`: Execute using the Ahead-Of-Time compiler.
  * `--hybrid`: Execute using the hybrid compilation model.
  * `--release`: Compile with optimizations before running.
  * `--explain-copies`: Print every compiler-inserted deep-copy site with file:line, type, and reason.

* `pit test`
  Executes the test suite for the current project.
  * `-t, --time`: Emit performance timings.
  * `--release`: Run tests with optimizations enabled.

* `pit bench`
  Runs every `#[bench]`-tagged function in the current project: a fixed warmup, then a fixed number of timed samples, always at release optimization. Reports mean, standard deviation, and minimum per bench.
  * `--json`: Emit the results as a JSON array instead of the human-readable table.

* `pit doc [file]`
  Renders one module's public `fn`/`struct`/`enum` signatures and `///` doc comments as markdown into `target/doc/<module>.md`. No file given defaults to the current project's pod entry. A fenced Olive block inside a doc comment is compile-checked; a broken one is a warning, not a hard failure.

* `pit fmt [file]`
  Formats the current project or a specified file according to the standard Olive style guidelines. By default it rewrites files in place.
  * `--check`: Exit non-zero if any file is not already formatted, without writing changes.
  * `--diff`: Print the formatting changes as a unified diff instead of writing them.
  * `--stdin`: Read source from standard input and write the formatted result to standard output.

* `pit fix [file]`
  Applies the suggested fixes from the compiler's diagnostics to the current project or a specified file. Only fixes the compiler marks as safe are applied automatically.
  * `--dry-run`: Show the fixes that would be applied without writing changes.

* `pit explain <code>`
  Prints a detailed explanation of a diagnostic code, including what triggers it and how to resolve it. Codes look like `E0400` for errors or `W0610` for warnings.

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

* `pit lsp`
  Starts a Language Server Protocol server over stdio: diagnostics as you type, hover, go-to-definition, completion, and document formatting. Editors talk to it directly; see `editors/vscode/` for an in-repo VSCode client.

* `pit dap`
  Starts a Debug Adapter Protocol server over stdio for breakpoints, stepping, and variable inspection and editing. VS Code and any other DAP client talk to it directly; see `docs/debugger.md`.

* `pit debug <file>`
  Starts a debug session for `<file>` over a flat newline-delimited JSON protocol, for AI agents and scripts that don't want the DAP handshake. See `docs/debugger.md`.

* `pit upgrade`
  Upgrades the Olive toolchain (compiler and standard library) to the latest stable release.
