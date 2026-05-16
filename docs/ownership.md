# Ownership and Memory Safety

One of the most powerful features of Olive is how it handles memory. Usually, you have to choose: you either manage memory yourself (which is fast but error-prone) or use a garbage collector (which is safe but adds overhead).

Olive uses a system called **Ownership**. It gives you the speed of manual management with the safety of a garbage collector. The compiler tracks who is using what and automatically cleans up data the moment it's no longer needed.

## The Three Rules

Ownership follows three simple rules that the compiler enforces strictly:

1. **Every value has a variable called its owner.**
2. **There can only be one owner at a time.**
3. **When the owner goes out of scope, the value is dropped (deleted).**

## Moving Data

When you assign a variable to another, or pass it to a function, the ownership **moves**. The original variable becomes invalid because it no longer "owns" that memory.

```python
let list1 = [1, 2, 3]
let list2 = list1  # list2 now owns the data. list1 is empty/invalid.

# print(list1)     # This would be a compile-time error!
```

This prevents "double-free" errors because the compiler knows exactly who is responsible for cleaning up the data. Simple types like `int` and `bool` are copied instead of moved because they are cheap to duplicate.

## Borrowing

If you just need to access some data without taking ownership, you can **borrow** it using the `&` symbol. Think of this as taking a reference to the data.

### Immutable Borrows (`&`)

You can have as many people reading the data as you want.

```python
let list = [1, 2, 3]
let r1 = &list
let r2 = &list

print(r1[0])  # OK
```

### Mutable Borrows (`&mut`)

If you need to change the data, you can borrow it with `&mut`. However, while a mutable borrow is active, nobody else can access the data - not even for reading. This prevents **data races**, where one part of your code reads data while another is halfway through changing it.

```python
let mut list = [1, 2, 3]
let r = &mut list
r[0] = 10     # OK

# let r2 = &list # Error: cannot borrow as immutable while mutably borrowed.
```

## The Golden Rule

You can have **many readers** OR **one writer**, but never both at the same time.

## Move Elision

The Olive optimizer is smart enough to see through many moves. If it detects that a value is moved into a function and then immediately returned, it can "elide" the move entirely, passing a pointer instead of copying the data structure. This ensures that Olive's safety features don't come at a performance cost.

## Lifetimes

You don't have to manually annotate "lifetimes" in Olive. The compiler looks at where you last use a variable to determine when a borrow ends. This allows you to write natural, readable code while the compiler handles the complex safety analysis behind the scenes.

