# Ownership and Memory Safety

Olive's most distinctive feature is its **Ownership-Based Resource Management (OBRM)**. This architectural pillar ensures total memory safety and eliminates data races at compile-time, providing the performance of manual memory management without the associated risks.

## The Three Rules of Ownership

1. Each value in Olive has a variable that's called its **owner**.
2. There can only be **one owner** at a time.
3. When the owner goes out of scope, the value is **dropped**.

## Move Semantics

When you assign a variable to another or pass it to a function, the ownership is **moved**. The original variable can no longer be used.

```python
let list1 = [1, 2, 3]
let list2 = list1  # ownership moves to list2

# print(list1)     # Error: use of moved variable `list1`
```

## Borrowing

Borrowing allows you to access a value without taking ownership. This is done using references.

### Immutable Borrows (`&`)

You can create multiple immutable references to a value. While a value is borrowed immutably, it cannot be modified or moved.

```python
let list = [1, 2, 3]
let r1 = &list
let r2 = &list

print(r1[0])  # OK
print(r2[0])  # OK
```

### Mutable Borrows (`&mut`)

If you need to modify a borrowed value, you can use a mutable reference. However, you can have **only one** mutable reference to a piece of data in a particular scope.

```python
let mut list = [1, 2, 3]
let r = &mut list
r[0] = 10     # OK

# let r2 = &list # Error: cannot borrow as immutable because it's already borrowed as mutable
```

### The Golden Rule: Aliasing XOR Mutation

Olive enforces the core principle of memory safety:
> You can have many readers OR exactly one writer, but never both at the same time.

## Non-Lexical Lifetimes (NLL)

Unlike older borrow checkers that release borrows at the end of a block, Olive's borrow checker is "smart." It uses **Non-Lexical Lifetimes** to release a borrow as soon as the reference is no longer used.

```python
let mut x = 5
let r = &x
print(r)    # r is used here for the last time

x = 10      # OK! The borrow of `x` by `r` ended after the print statement
```

## Initialization Tracking

Olive tracks the state of every variable to ensure you never use uninitialized memory.

```python
let x: int
# print(x)  # Error: use of possibly uninitialized variable `x`
x = 10
print(x)    # OK
```

## Optimization: Conditional Borrow Checking

While memory safety is paramount, safety analysis can be expensive for JIT compilation. To ensure the fastest possible startup, the Olive compiler employs **Conditional Borrow Checking**.

If a function:
1.  Only uses primitive types (like `int`, `float`, `bool`) that follow copy semantics.
2.  Does not use any move-only types (like `list`, `dict`, `class` instances).
3.  Does not create or use any references (`&` or `&mut`).

The compiler **skips the borrow checking pass entirely** for that function. This allows simple compute kernels and utility functions to be JIT-compiled and executed with zero safety-analysis overhead, matching the startup latency of non-memory-safe JITs while maintaining total safety for complex data structures.
