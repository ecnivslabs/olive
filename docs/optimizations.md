# The Optimization Pipeline

The Olive compiler optimizes on the Middle Intermediate Representation (MIR). Passes are iterative and compositional; one pass often reveals opportunities for the next.

Two pipelines exist. Debug builds (`pit run`, `pit build` without `--release`) run only ownership inference, CFG cleanup, dead code elimination, and move elision, keeping compiles fast. Release builds (`--release`) run everything below. Ownership inference and generation checks run in both, because drops and safety checks are semantics, not optimizations.

## Ownership Inference

Runs first in every pipeline. Classifies each heap local as owner or view, promotes last-use copies to moves, deep-copies values stored into containers while still live elsewhere, and frees reassigned owners. Details in [Ownership](ownership.md) and [Internals](internals.md).

## Scalar Transformations

These run in a loop until the MIR reaches a fixed point.

### Copy Propagation
Replaces uses of a copied variable with the original, exposing more work to the passes below.

### Constant Propagation & Folding
Values known at compile time are substituted and evaluated immediately.
```rust
let x = 10
let y = x + 5  // becomes 15 at compile time
```

### Algebraic Simplification & Strength Reduction
Mathematical identities simplify expressions, and expensive operations become cheaper equivalents.
- `x + 0` becomes `x`
- `x * 1` becomes `x`
- `x - x` becomes `0`
- `x * 8` becomes `x << 3`

### Common Subexpression Elimination & Global Value Numbering
Both detect repeated computations and reuse the first result. GVN assigns an ID to every distinct computation and understands commutativity (`x + y` equals `y + x`), so it also catches redundancy across branches.

### Peephole Optimization
Local pattern rewrites over short instruction windows.

### Simplify CFG
Merges blocks that always follow each other, removes empty blocks, and turns conditional branches into direct jumps when the condition is known.

### Dead Code Elimination
Instructions whose results are never used are pruned, including entire unreachable paths.

### Move Elision
Identifies unnecessary moves (a value moved into a function and immediately returned, for example) and passes a pointer instead.

## Structural Transformations

### Inlining
Replaces a call with the callee's body, removing call overhead and letting scalar passes work across the former boundary. Small, frequently called functions are inlined; large ones are left alone to avoid code bloat. Profile data (from the JIT or PGO) biases the decision toward hot callees.

### Tuple and Struct Scalarization
Non-escaping tuples and structs are broken into their individual fields, which then live in registers instead of memory.

### Tail-Call Optimization
A function whose final action is a call becomes a jump, so recursive algorithms run with the memory profile of a loop.

### Loop-Invariant Code Motion
Computations that produce the same result on every iteration are hoisted out of the loop.

### SIMD Vectorization
Data-parallel loop patterns are rewritten to SIMD instructions (AVX2, NEON) where the target supports them.

### Loop Unrolling
Short counted loops are fully unrolled (up to 32 iterations); longer ones are partially unrolled by a factor of 4 to cut branch overhead and expose more scalar optimization.

## Late Passes

### Bounds-Check Elimination
Runs last among the optimizations, so no later pass can move an access it has already proven safe. An index proven in range by loop analysis drops its runtime check.

### Generation-Check Insertion
Runs after everything, in both pipelines. Inserts the runtime staleness checks that back ownership inference (see [Internals](internals.md)); a forward analysis elides every check it can prove unnecessary.

## Runtime Tiering

`pit run` also optimizes while the program executes: hot functions are recompiled at a higher tier, and `Any`-typed arithmetic sites that only ever see ints graduate to guarded inline integer ops. See [Internals](internals.md) for the mechanics.

## Inspecting the Pipeline

- `pit run --emit-mir`: prints the MIR after all optimizations.
- `pit build --emit-mir`: same, for the AOT pipeline.
- `pit run -t`: prints per-stage compile timings.
