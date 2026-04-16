# Ownership and Memory Safety

Olive manages memory using a compile-time tracking system called **Ownership**. Instead of using a garbage collector or requiring manual deallocation, the compiler tracks when data is no longer needed and drops it automatically. This yields native execution speeds with complete memory safety.

## The Rules of Ownership

The compiler strictly enforces three rules:

1. **Every value has a single owner (a variable).**
2. **There can only be one owner at a time.**
3. **When the owner goes out of scope, the value is dropped.**

## Moves

Assigning a variable to another or passing it to a function transfers ownership (a **move**). Once ownership is transferred, the original variable becomes invalid.

```rust
let list1 = [1, 2, 3]
let list2 = list1  // list2 now owns the data. list1 is invalid.

// print(list1)     // Compile-time error
```

Moves prevent double-free errors. Simple types like `int` and `bool` implement copy semantics rather than move semantics because they are cheap to duplicate in registers.

## Borrowing (References)

To access data without taking ownership, you can **borrow** it using references (`&`):

### Immutable References (`&`)

Multiple parts of a program can borrow a resource concurrently for read access.

```rust
let list = [1, 2, 3]
let r1 = &list
let r2 = &list

print(r1[0])  // OK
```

### Mutable References (`&mut`)

To modify borrowed data, use `&mut`. To prevent data races and use-after-free bugs, a mutable reference enforces exclusive access. While a mutable reference is active, no other references (immutable or mutable) can exist.

```rust
let mut list = [1, 2, 3]
let r = &mut list
r[0] = 10     // OK

// let r2 = &list // Compile-time error: cannot borrow as immutable while mutably borrowed.
```

### Aliasing Rules

You can have **many readers** OR **one writer**, but never both simultaneously.

## Move Elision

The compiler optimizes unnecessary moves. When a value is moved into a function and immediately returned, the optimizer performs move elision, passing pointers rather than copying data blocks.

## Lifetimes

Olive determines the lifetime of a borrow based on its last actual usage point (Non-Lexical Lifetimes). You do not need to manually write lifetime annotations; the compiler tracks the scopes automatically to verify safety.


