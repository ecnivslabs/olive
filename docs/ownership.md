# Ownership and Memory Safety

Olive manages memory without a garbage collector and without lifetime
annotations. The compiler infers ownership: it tracks which variable owns each
heap value, frees the value at its owner's last use, and inserts copies where
two places would otherwise share one value. A generation-checked allocator
backs the analysis at runtime, so a reference the compiler could not prove
safe stops the program with a clear fault instead of corrupting memory.

## The model

Three rules define how values behave:

1. **Every heap value has exactly one owner.** Scalars (`int`, `float`,
   `bool`) are copied freely and are not tracked.
2. **Reads never invalidate.** Assigning a variable to another, passing it to
   a function, or printing it leaves the original usable. You never restructure
   code to satisfy the compiler.
3. **Stores copy or move, never share.** Putting a value into a list, dict,
   struct field, or another container either transfers ownership (when the
   source is provably at its last use) or stores an independent deep copy.

Rule 3 is observable. A container never aliases a variable you can still
reach:

```rust
fn main():
    let mut a = [1, 2]
    let b = [a]      // b stores its own copy of a
    a[0] = 99
    print(a[0])      // 99
    print(b[0][0])   // 1, unchanged
```

If `a` were never used after `let b = [a]`, the compiler would move it into
`b` instead and no copy would happen. Whether a store copies or moves is a
performance detail; the visible behavior is always value semantics.

## Assignment

```rust
let list1 = [1, 2, 3]
let list2 = list1
print(list1)   // fine
print(list2)   // fine
```

When `list1` is not used again, `list2` takes ownership directly and no data
is copied. When both stay live, they are still two usable names; the compiler
keeps the program safe either way. This is the main ergonomic difference from
Rust: use-after-assignment is not an error you have to code around.

## References

Explicit references borrow a value without owning it, and these are checked
at compile time.

```rust
let list = [1, 2, 3]
let r1 = &list
let r2 = &list
print(r1[0])   // any number of readers
```

A mutable reference needs exclusive access. Taking one while another borrow
is live is a compile error:

```rust
let mut list = [1, 2, 3]
let r = &mut list
let r2 = &list   // E0503: already borrowed as mutable
```

Borrows end at their last use, not at the end of the scope, so a reference
you are done with never blocks later code.

## The runtime backstop

Compile-time inference cannot see across every boundary (dynamic `Any` values,
Python objects, FFI). For the cases it cannot prove, Olive falls back to a
generational allocator: every heap object carries a generation counter, and a
borrow that might outlive its owner is validated before use. A stale access
stops the program with error `E0707` and a source location:

- a use-after-free can never read or write recycled memory
- a double free is absorbed as a no-op
- the failure is deterministic and points at the access, not at a crash later

Run `pit explain E0707` for details.

## Frees are deterministic

A value is freed at a known point: its owner's last use, or the owner's
reassignment when the old value is unreachable. There is no collector, no
pause, and no reference counting on the hot path.

## Move elision

When a value is passed into a function and consumed there, or returned
straight through, the compiler passes a pointer instead of copying. The
optimizer promotes copies to moves wherever liveness allows, so most of the
copies rule 3 describes never happen in compiled code.
