## The Optimization Pipeline

The optimization suite operates on the Middle Intermediate Representation (MIR) and is designed to run iteratively. This allows different passes to "feed" each other—for example, **Constant Propagation** might reveal a branch is always taken, which **Simplify CFG** can then turn into a direct jump, allowing **Dead Code Elimination** to prune the unreachable branch.

### Scalar Transformations (Iterative)

These passes run in a loop (up to a fixed point) to improve the efficiency of scalar logic:

#### 1. Constant Propagation & Folding
The compiler tracks the values of variables that are known at compile-time and "folds" operations on them.
*   **Example**:
    ```python
    let x = 10
    let y = x + 5
    print(y) # Becomes print(15)
    ```

#### 2. Global Value Numbering (GVN)
GVN is a sophisticated pass that identifies redundant computations across different parts of a function by assigning a unique "Value Number" to each unique expression. Unlike simple CSE, GVN understands that `x + y` is the same as `y + x` and can identify redundancies even if they are separated by complex control flow.

#### 3. Copy Propagation
Eliminates redundant variable assignments. If the compiler sees `let a = b; let c = a + 1`, it will transform it into `let c = b + 1`, reducing the number of local variables and easing the burden on the JIT's register allocator.

#### 4. Dead Code Elimination (DCE)
DCE prunes instructions whose results are never used and code paths that are mathematically proven to be unreachable. This keeps the binary lean and prevents the CPU from wasting cycles on unnecessary work.

#### 5. Strength Reduction & Peephole
These passes replace expensive machine operations with cheaper ones.
*   **Multiplication to Shift**: `x * 8` becomes `x << 3`.
*   **Modulo to AND**: `x % 8` (for unsigned) becomes `x & 7`.
*   **Peephole**: Replaces small sequences of instructions (e.g., `push; pop`) with faster equivalents.

### Late-Stage Transformations

Once the scalar logic is refined, Olive applies structural transformations:

#### Inlining
Inlining is a foundational optimization. By replacing a function call with the actual body of the function, Olive eliminates the overhead of the call (stack manipulation, register saving) and allows all scalar optimizations to work across the former function boundary.
*   **Heuristic**: The compiler uses a size-based heuristic to inline small "hot" functions while avoiding excessive binary bloat.

#### Loop-Invariant Code Motion (LICM)
LICM identifies computations inside a loop that produce the same result every iteration and "hoists" them to the loop header (outside the loop).
*   **Example**:
    ```python
    for i in range(1000):
        let val = x * y # Hoisted outside!
        print(i + val)
    ```

#### Tail-Call Optimization (TCO)
If a function's last action is to call itself (or another function), Olive transforms that call into a direct jump. This allows recursive algorithms that are just as efficient as iterative loops, without the risk of stack overflow.

#### SIMD Vectorization
For data-parallel tasks, the vectorizer can group multiple operations into a single SIMD instruction (e.g., AVX2 or NEON).
*   **Manual SIMD**: You can use `__vector_add(a, b)` intrinsics for precise control.
*   **Auto-Vectorization**: The compiler identifies simple loops and automatically emits vector instructions where safe and profitable.

## Zero-Cost JIT Startup

Olive is optimized for the "Just-In-Time" environment. Traditional AOT (Ahead-of-Time) compilers can afford to spend minutes optimizing, but a JIT must be fast.

- **Conditional Borrow Checking**: The most expensive part of Olive's safety analysis is the borrow checker. For functions that only use "copyable" types (like `int` or `float`) and don't use references, Olive **skips the borrow checker entirely**. This results in faster startup for compute-heavy scalar code.
- **Lazy Runtime Discovery**: Instead of resolving every possible runtime hook at startup, the compiler resolves them "on-demand," ensuring that the application starts in milliseconds.

## Performance Monitoring

Developers can audit the compiler's work using the `pit` toolchain:

- `pit build --time`: Displays a breakdown of how much time was spent in each optimization phase.
- `pit run --emit-mir`: Generates a human-readable text representation of the optimized MIR, allowing you to verify that loops were hoisted and constants were folded.
