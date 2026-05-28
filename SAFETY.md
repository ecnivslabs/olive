# Safety

This document states what Olive guarantees about memory safety, what it proves
statically, what it catches at runtime, and what it does not cover. Each claim
names the test, CI lane, or fuzzer that enforces it.

The honest summary: no silent corruption on Olive-managed heap values; most
ownership violations caught at compile time; violations the compiler cannot
prove caught at runtime as deterministic coded faults; no data races on
Olive-managed memory. This is not the same as Rust's compile-time guarantees
and is never claimed to be.

## Compile-time guarantees

### Reference borrow rules

Explicit references (`&`, `&mut`) are checked at compile time by the borrow
checker on the MIR control-flow graph.

- Any number of shared borrows (`&`) may coexist; no mutable borrow may be
  active at the same time (E0503).
- A mutable borrow (`&mut`) requires exclusive access; no other borrow,
  shared or mutable, may be live concurrently (E0501, E0503).
- Borrows end at their last use, not at the enclosing scope boundary
  (non-lexical lifetimes).

A program that violates these rules does not compile. There is no runtime
fallback for reference rule violations.

Plain assignment, function arguments, and container stores are not checked
here; those are handled by ownership inference and the runtime backstop.

Enforced by: `cargo test --workspace` (borrow-check test suite in
`src/borrow_check/`).

### Definite staleness detection (E0708)

When the gencheck solver can prove, by must-free dataflow, that a borrow
outlives its owner on every control-flow path through a function, the program
is rejected at compile time with E0708 ("use of a value after it was given
away"). The diagnostic carries a three-way caret: where the value was
borrowed, where it escaped, and where it was subsequently used.

Sites where staleness depends on which branch runs are not covered by E0708;
those fall to the runtime generation check (E0707). Sites where the ownership
roots are not fully known to the static analysis also remain runtime checks.

Enforced by: definite-staleness test cases in
`src/mir/optimizations/gencheck/tests.rs`; the differential fuzzer
(`tests/differential_fuzz.rs`) confirms both pipelines agree on every
generated program.

Run `pit explain E0708` for a worked example.

## Runtime guarantees

### Generation checks (E0707)

Every heap object lives in a generational slab allocator (`std_lib/src/slab.rs`,
`std_lib/src/string_slab.rs`). A generation counter in the slot header is
incremented atomically (Release) on each allocation and free. Borrows the
compiler could not prove safe have their generation captured at borrow time;
the check re-reads the counter (Relaxed load) before any subsequent use. If
the counter has advanced since the capture, the slot was freed or reallocated,
and the program stops with E0707 and a source caret.

Covered types: `list`, `dict`, `set`, `tuple`, `str`, `bytes`, `Result`, and
non-FFI structs. These all allocate from a `GenSlab` and carry a generation
word at `ptr - 8`.

Excluded from generation checks (no generation word exists):
- Futures (`OliveSmFuture`, `OliveFuture`): `Box::into_raw` allocations,
  not slab slots.
- FFI structs and Python objects: allocated outside the slab.

Consequences:
- A use-after-free on a covered type can never silently read or write
  recycled memory.
- The failure points at the access that would have been unsafe, not at a
  crash site elsewhere.
- On paths where a forward may-free analysis proves no free can intervene
  between borrow and use, the check is elided entirely. Well-typed hot paths
  pay nothing.

Enforced by: staleness test cases in `src/mir/optimizations/gencheck/tests.rs`;
ASAN lane in CI (the `sanitizers` job in `.github/workflows/ci.yml`) confirms
no heap corruption escapes the runtime.

Run `pit explain E0707` for a worked example.

### Double-free absorption

Freeing a slot whose generation counter does not match the generation stored in
the pointer is silently ignored. The slab checks the generation before writing
the free-list link; a mismatched free is a no-op. No undefined behavior results.

Enforced by: `recycled_body_keeps_tail_words` and related slab tests in
`std_lib/`.

### Share-nothing task boundaries

Tasks do not share Olive-managed heap memory. A value passed across a task
boundary via `chan_send`, `mutex_new`, `mutex_unlock`, or `aio.pool_run` is
either moved exclusively (the sending local is provably dead at the boundary)
or deep-copied before crossing. The ownership pass makes this decision using
the `RUNTIME_ESCAPES` table in
`src/mir/optimizations/ownership/summaries.rs`, which lists every runtime
function that stores a pointer argument into longer-lived storage.

No managed value is ever reachable from two tasks at the same time.

The atomics on slab alloc and free (Release store, Relaxed load) are not
synchronization between tasks; they make the generation counter access
defined under the C++ memory model. Synchronization is unnecessary because
the share-nothing invariant ensures two tasks never access the same slot
concurrently.

Enforced by: task-isolation and channel tests in `std_lib/`; TSAN lane in CI
(the `sanitizers` job in `.github/workflows/ci.yml`).

## Out of scope

The following are not covered by any guarantee in this document.

**`unsafe` blocks.** The Olive runtime is implemented in Rust and uses
`unsafe` internally. Those sites are the implementation's responsibility and
are audited separately. The language-level guarantees above assume the
runtime implementation is correct.

**FFI memory.** Values passed to or returned from C or Python code live
outside the slab allocator and outside the generation-check scheme. Memory
management of FFI-allocated values is the caller's responsibility.

**Python-object internals.** Python objects are reference-counted by CPython.
Olive does not instrument that refcount. Dropping an Olive binding that was
the last reference to a Python object frees it via CPython; Olive has no
visibility into subsequent accesses from the Python side.

**Indirect calls through function-typed values.** The escape analysis in
`summaries.rs` only tracks escapes through `Constant::Function` callees.
Escapes through function-typed values are invisible to the static pass and
fall to the runtime generation check (E0707). This is documented in the
`summaries.rs` module comment.

## Test and CI coverage

| Guarantee | Enforcing artifact |
|---|---|
| Reference borrow rules | `cargo test --workspace` (borrow-check suite) |
| E0708 compile-time staleness | `gencheck/tests.rs`; differential fuzzer |
| E0707 runtime generation check | `gencheck/tests.rs`; ASAN CI lane |
| Double-free absorption | `std_lib` slab tests |
| Share-nothing task boundaries | `std_lib` task tests; TSAN CI lane |
| Pipeline output equivalence | `tests/differential_fuzz.rs` (fixed + rotating seeds) |
