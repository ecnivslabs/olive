# The Optimization Pipeline

The Olive compiler uses a multi-stage optimization pipeline that operates on the Middle Intermediate Representation (MIR). These passes are iterative and compositional - one pass often reveals opportunities for the next.

## Scalar Transformations

These passes run in a tight loop until the MIR reaches a "fixed point" (meaning no more changes can be made).

### 1. Constant Propagation & Folding
The compiler tracks values that are known at compile-time and evaluates operations on them immediately.
```python
let x = 10
let y = x + 5  # Becomes 15 at compile-time
```

### 2. Algebraic Simplification
Uses mathematical identities to simplify expressions.
- `x + 0` → `x`
- `x * 1` → `x`
- `x - x` → `0`
- `(a + b) - a` → `b`

### 3. Global Value Numbering (GVN)
GVN assigns a unique ID to every distinct computation. It detects when the same value is computed multiple times, even across different branches, and eliminates the redundancy. Unlike simple CSE, GVN understands commutativity (`x + y` is the same as `y + x`).

### 4. Move Elision
In a borrow-checked language, "moving" data is a common operation. Move elision identifies when a move is unnecessary - for example, when a value is moved into a function and then immediately returned - and replaces the move with a simple pointer pass or avoids the copy entirely.

### 5. Dead Code Elimination (DCE)
Instructions whose results are never used are pruned. This includes removing entire code paths that the compiler can prove are unreachable.

## Structural Transformations

Once the scalar logic is clean, Olive applies transformations that change the structure of the program.

### Inlining
Replaces a function call with the actual body of the function. This removes the overhead of the call and allows scalar optimizations to work across the former function boundary. Olive uses an "effort-based" heuristic: small, frequently called functions are always inlined, while large functions are left alone to avoid code bloat.

### Loop-Invariant Code Motion (LICM)
Computations that produce the same result on every iteration of a loop are moved (hoisted) outside the loop.
```python
for i in range(1000):
    let val = x * y  # This is moved before the 'for' starts
    print(i + val)
```

### Simplify CFG
Cleans up the Control Flow Graph by:
- Merging blocks that always follow each other.
- Removing empty blocks.
- Turning conditional branches into direct jumps when the condition is known.

## Late-Stage & Hardware Optimizations

### Tail-Call Optimization (TCO)
When a function's final action is calling itself (or another function), Olive transforms the call into a jump. This allows recursive algorithms to run with the performance and memory profile of a simple loop.

### SIMD Vectorization
The compiler identifies patterns of data-parallel work and emits SIMD instructions (like AVX2 or NEON). This can result in a 4x-8x speedup for mathematical loops.

## Inspecting the Pipeline

You can use the `pit` toolchain to see what the optimizer is doing to your code:

- `pit run --emit-mir`: Prints the MIR after all optimizations.
- `pit build --stats`: Shows how many times each optimization pass was triggered and how much code was removed.
