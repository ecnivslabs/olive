# Contributing to Olive

Olive tries to give you memory safety without a garbage collector, and readable syntax without giving up speed. The long-term goal is to become the language of choice for AI and ML workloads currently stuck with slow, dynamically-typed scripting, with best-in-class ergonomics and strong safety and performance. Before changing the compiler, it helps to understand why it's built the way it is.

## Philosophy first

These rules live in the compiler itself. Run `import olive` in any `.liv` file and it prints them:

```
No compromise.
Readability is not optional.
Complexity must justify itself.
Power should not require ceremony.
Safety should be free, not fought for.
The obvious solution should be obvious.
Simple things should be simple.
Complex things should be possible.
Purity must not outweigh practicality.
What would Olive do?
```

What that means day to day:

- Zero overhead is not negotiable. Olive compiles to native code through Cranelift. A feature that needs a runtime tax, boxing, a hidden allocation, a GC-shaped fallback, is the wrong design until proven otherwise. If it can't run at the speed of the equivalent hand-written unsafe code, it isn't done.
- Safety is inferred, not annotated. No lifetime syntax, no `Rc<RefCell<>>` ceremony. The compiler infers ownership, borrow-checks explicit `&`/`&mut` references against the MIR control-flow graph, and backstops whatever it can't prove statically with generation-checked runtime faults. If you're solving a soundness problem by asking the user to write more annotations, you're solving it at the wrong layer. Fix the inference instead.
- A fault beats corruption, always. Anything the static analysis can't prove sound has to fail as a precise, coded runtime fault (see `SAFETY.md`), never silent memory corruption.
- Readability is structural, not stylistic. No braces, no semicolons, indentation-defined blocks. Don't propose syntax that reintroduces punctuation noise to solve a parsing convenience problem.
- Complexity has to earn its place. If a feature needs a caveat in the docs, "works except for...", it's not ready.

If a change conflicts with one of these, it's wrong until the tradeoff is spelled out in the PR description, not assumed.

## Before you write code

Read, in order:

1. `docs/introduction.md`: philosophy and the compilation pipeline (lexer, parser, semantic analysis, borrow check, MIR ownership inference and optimization, Cranelift codegen).
2. `docs/ownership.md`: how inferred ownership and borrow checking work. Most correctness bugs in the compiler are ownership bugs.
3. `SAFETY.md`: the exact line between what's proven at compile time, what's caught at runtime, and what's out of scope. Every claim in there is backed by a specific test suite or CI lane. If you touch ownership, borrow checking, or the allocator, keep that mapping true.
4. `SECURITY.md`: scope for vulnerability reports. Don't file security-sensitive bugs as public issues.

## Repository layout

The compiler (`pit`) lives under `src/`:

- `lexer/`, `parser/`: tokenizing and building the AST.
- `semantic/`: name resolution, type checking, desugaring.
- `borrow_check/`: compile-time reference aliasing rules (E05xx).
- `mir/`: the mid-level IR. `builder/` constructs it, `optimizations/` is the pass pipeline (ownership inference, copy elision, loop unrolling, bounds-check elimination, vectorization, and more), `optimizer.rs` sequences them.
- `codegen/cranelift/`: MIR to native code.
- `compile/`: orchestration. Caching, the linker, lints, the fixer, diagnostic loading.
- `diagnostics/`: the error and warning catalog, backs `pit explain`.
- `tooling/`: `dap/` (debugger), `lsp/`, doc generation, the installer, package resolution.
- `commands/`: the `pit` CLI subcommands.
- `fmt/`: the formatter (`pit fmt`).

The standard library and runtime, allocator, slabs, generation checks, collections, FFI glue, lives in `std_lib/`, a separate crate so it can be built and linked independently into user programs.

`benchmark/` holds the dedicated benchmarking harness. Don't add one-off timing scripts elsewhere.

## Coding style

- Standard `rustfmt` defaults, no repo-specific config. Run it before every commit.
- Match the surrounding module's patterns before introducing a new one. This codebase already has strong conventions for error handling, MIR pass structure, and diagnostic construction. Don't invent a second way to do something it already does once.
- Comments are rare and short: one line, only for a why that isn't obvious from the code itself (a workaround, a non-obvious invariant, an ordering requirement). Don't restate what the code does. Don't leave commented-out code around.
- No `unwrap()`/`expect()` on paths reachable from user input. Compiler errors and runtime faults get diagnosed, not panicked. Internal invariants that truly can't be violated are the exception, and even those should carry a real `expect()` message.
- Keep files focused. If one sprawls past a few hundred lines and covers unrelated responsibilities, split it along the responsibility, not arbitrarily.
- New diagnostics get a real code (`E0xxx` for errors, `W0xxx` for warnings) registered in `diagnostics/`, an entry in the explain database (`pit explain <code>` has to work for it), and where the compiler can safely auto-correct it, a `pit fix` rule.

## Testing

- `cargo test --workspace` runs everything: lexer, parser, and borrow-check unit tests, MIR pass tests, the integration suites under `tests/` (DAP, LSP, Python FFI boundary, formatter round-trips, differential fuzzing), and `std_lib`'s own suite.
- Adding an Olive-level language feature? Add coverage as an Olive `#[test]` program in the right integration test, not just a Rust unit test of the pass in isolation. What has to keep working is the whole pipeline.
- `std_lib` runtime and allocator changes must pass clean under both ASAN and TSAN (`cargo test -p olive_std --lib -Zbuild-std`, nightly toolchain, see `.github/workflows/ci.yml` for the exact command). A memory-safety fix that isn't sanitizer-clean isn't fixed.
- CI runs on Linux, macOS, and Windows. Don't hardcode a path separator, a shell assumption, or an environment variable that only exists on one platform.

## Benchmarking

Performance claims are only real when they come from `pit bench` (or the `benchmark/` harness for whole-pipeline comparisons against Rust and Python baselines), run at `--release`, not eyeballed from a `time` call. Touching a hot path in MIR optimization, codegen, or the runtime allocator means benchmarking before and after and putting the numbers in the PR. A perf PR without numbers won't get reviewed as one.

Optimizations have to be general. A pass that special-cases the shape of one benchmark's input instead of a real class of programs gets rejected regardless of the number it produces.

## Commits and PRs

- Fork, branch, PR against `master`.
- Commit subject: lowercase, imperative, no period, no emoji, no roadmap or phase labels. Say what changed, and why if it isn't obvious. An optional `type(scope):` prefix (`fix(mangle): ...`, `perf(pyffi): ...`) is fine when it adds clarity; plenty of commits in this repo's history skip it entirely. Check `git log` before your first one.
- One logical change per commit. A perf commit and a correctness fix it depends on are two commits, not one.
- Before opening a PR: `cargo fmt`, `cargo test --workspace` clean, `pit fmt --check` on any `.liv` files you touched (examples, `std_lib` test fixtures, doc code blocks, `pit doc` compile-checks fenced Olive blocks in doc comments).
- If the change affects a guarantee described in `SAFETY.md`, update that file in the same PR. A safety claim that no longer matches the implementation is worse than no claim.

## Reporting bugs vs. vulnerabilities

Regular bugs, including most `unsafe` bugs that don't lead to exploitable corruption or a safety-check bypass, go through a GitHub issue. Exploitable memory corruption or a soundness hole in the ownership, borrow-check, or gencheck machinery goes through `SECURITY.md`, not a public issue.

## What gets a slow no

- Syntax additions that trade a small ergonomic win for parser or grammar complexity.
- Anything that closes a gap by adding ceremony, new annotations, new required boilerplate at call sites, instead of improving inference.
- Performance work without `pit bench` numbers, or that regresses one workload to win another.
- Safety-relevant changes without sanitizer coverage.

If you're unsure about direction before sinking real time in, open an issue first. Olive has strong, deliberate opinions. Better to find out you're rowing against one before the PR than after.
