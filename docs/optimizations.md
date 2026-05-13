# The Optimization Pipeline

The optimization suite operates on the Middle Intermediate Representation (MIR) and runs iteratively. Passes are designed to compose: **Constant Propagation** can reveal that a branch is always taken, which **Simplify CFG** turns into a direct jump, which then lets **Dead Code Elimination** prune the now-unreachable code. Each pass creates opportunities for the others.

## Scalar Transformations (Iterative)

These passes run in a loop until no further changes occur:

### 1. Constant Propagation & Folding

The compiler tracks variables whose values are known at compile time and evaluates operations on them at compile time:

```python
let x = 10
let y = x + 5
print(y) # Becomes print(15)
```

### 2. Global Value Numbering (GVN)

GVN assigns a unique identifier to each distinct computed value and detects when the same value is computed more than once. Unlike simple common subexpression elimination, GVN understands commutativity (`x + y` and `y + x` produce the same value) and can find redundancies across complex control flow.

### 3. Copy Propagation

Eliminates unnecessary variable aliases. If the compiler sees `let a = b; let c = a + 1`, it rewrites this to `let c = b + 1`, reducing the number of local variables and easing the register allocator's job.

### 4. Dead Code Elimination (DCE)

Prunes instructions whose results are never used, and removes code paths that are provably unreachable. This keeps the generated code lean.

### 5. Strength Reduction & Peephole

Replaces expensive operations with cheaper equivalents:

- **Multiplication to shift**: `x * 8` becomes `x << 3`.
- **Modulo to AND**: `x % 8` (for unsigned values) becomes `x & 7`.
- **Peephole**: Small instruction sequences like `push; pop` are replaced with faster alternatives.

## Late-Stage Transformations

Once scalar logic is refined, the compiler applies structural transformations:

### Inlining

Replaces a function call with the body of the called function. This eliminates call overhead and allows all scalar passes to work across the former call boundary. The compiler uses a size-based heuristic to decide when inlining is worth it. Small, frequently-called functions are good candidates; large functions are not.

### Loop-Invariant Code Motion (LICM)

Moves computations that produce the same result on every loop iteration outside the loop:

```python
for i in range(1000):
    let val = x * y  # Hoisted to before the loop
    print(i + val)
```

### Tail-Call Optimization (TCO)

When a function's final action is a call to itself (or to another function), that call is transformed into a direct jump. Tail-recursive functions compile to iterative code with no stack growth, which means no risk of stack overflow.

### SIMD Vectorization

For data-parallel work, the vectorizer groups multiple operations into a single SIMD instruction (AVX2 or NEON, depending on the target):

- **Manual SIMD**: Use `__vector_add(a, b)` and similar intrinsics for direct control.
- **Auto-vectorization**: The compiler identifies loops that are safe to vectorize and emits vector instructions automatically.

## Zero-Cost JIT Startup

Olive is built for the JIT environment, where startup time matters. Two mechanisms keep it fast:

- **Conditional Borrow Checking**: For functions that only use primitive types and no references, the borrow checker is skipped entirely. These functions can't violate memory safety by construction, so the analysis would be wasted work.
- **Lazy Runtime Discovery**: Runtime hooks are resolved on demand, not all at startup. The application becomes executable in milliseconds.

## Performance Monitoring

The `pit` toolchain exposes compiler internals for inspection:

- `pit build --time`: Shows a per-phase timing breakdown: how long was spent in borrow checking, each optimization pass, and codegen.
- `pit run --emit-mir`: Prints the optimized MIR as readable text. You can verify that constants were folded, loops were hoisted, and tail calls were identified.
