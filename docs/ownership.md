# Ownership and Memory Safety

Olive manages memory through Ownership-Based Resource Management (OBRM). There's no garbage collector. The compiler tracks ownership at compile time and inserts deallocation exactly where it's needed. You get deterministic memory management without writing `free`.

## The Three Rules of Ownership

1. Every value has exactly one **owner**: the variable that holds it.
2. There can only be **one owner** at a time.
3. When the owner goes out of scope, the value is **dropped**.

These rules are enforced by the compiler. Violations are caught before the program runs.

## Move Semantics

Assigning a variable to another variable, or passing it to a function, **moves** ownership. The original binding becomes invalid:

```python
let list1 = [1, 2, 3]
let list2 = list1  # ownership moves to list2

# print(list1)     # Error: use of moved variable `list1`
```

This applies to heap-allocated types like lists, dicts, and structs. Primitive types like `int` and `float` are copied automatically.

## Borrowing

Borrowing lets you access a value without taking ownership. You hold a reference, use it, and the owner retains its value when you're done.

### Immutable Borrows (`&`)

Multiple immutable references can coexist. While any immutable reference is live, the value cannot be modified or moved:

```python
let list = [1, 2, 3]
let r1 = &list
let r2 = &list

print(r1[0])  # OK
print(r2[0])  # OK
```

### Mutable Borrows (`&mut`)

A mutable reference allows modification, but there can be only one in a given scope. You can't hold a mutable reference alongside any other reference to the same value:

```python
let mut list = [1, 2, 3]
let r = &mut list
r[0] = 10     # OK

# let r2 = &list # Error: cannot borrow as immutable because it's already borrowed as mutable
```

### The Core Rule: Aliasing XOR Mutation

> You can have many readers OR one writer, but never both at the same time.

This rule eliminates an entire category of bugs at the language level: data races, use-after-free through aliases, and concurrent mutation.

## Non-Lexical Lifetimes (NLL)

Olive's borrow checker understands that a borrow ends when the reference is last used, not when the enclosing scope ends. This avoids false positives that would otherwise force unnecessary restructuring:

```python
let mut x = 5
let r = &x
print(r)    # r is used here for the last time

x = 10      # OK: the borrow by `r` ended after the print
```

## Initialization Tracking

The compiler tracks initialization state for every variable. Using a variable before it's assigned is a compile-time error:

```python
let x: int
# print(x)  # Error: use of possibly uninitialized variable `x`
x = 10
print(x)    # OK
```

## Conditional Borrow Checking

Borrow checking is thorough, and thorough analysis has a cost. To minimize JIT startup time, the compiler skips the borrow checking pass entirely for functions that meet all of the following:

1. Only use primitive types (`int`, `float`, `bool`) that follow copy semantics.
2. Don't use any move-only types (`list`, `dict`, struct instances).
3. Don't create or use any references (`&` or `&mut`).

These functions can't violate memory safety by construction, so running the borrow checker on them would add overhead for no benefit. Simple compute kernels and pure math functions get compiled with no safety-analysis overhead. Complex data structure code gets the full analysis.
