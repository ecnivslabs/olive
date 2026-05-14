# Ownership and Memory Safety

One of the most powerful features of Olive is how it handles memory. Most languages either make you manage memory yourself (which is fast but dangerous) or use a "garbage collector" (which is safe but can be slow). 

Olive uses a system called **Ownership**. It gives you the speed of manual management with the safety of a garbage collector. The compiler simply tracks who is using what and cleans up for you automatically.

## The Three Rules

1. Every piece of data has an **owner** (a variable).
2. There can only be **one owner** at a time.
3. When the owner goes away (out of scope), the data is **deleted**.

## Moving Data

When you assign a variable to another one, or pass it to a function, the "ownership" moves. The original variable can't be used anymore because it no longer owns that data.

```python
let list1 = [1, 2, 3]
let list2 = list1  # list2 now owns the data. list1 is empty/invalid.

# print(list1)     # This would be an error!
```

This applies to things like lists, dictionaries, and custom objects. Simple things like numbers and booleans are copied automatically instead of moved.

## Borrowing

If you just want to *look* at some data without taking ownership, you can **borrow** it using the `&` symbol.

### Immutable Borrows (`&`)

You can have as many people looking at (reading) the data as you want.

```python
let list = [1, 2, 3]
let r1 = &list
let r2 = &list

print(r1[0])  # OK
```

### Mutable Borrows (`&mut`)

If you want to *change* the data, you can borrow it with `&mut`. However, while you're changing it, nobody else can be looking at it. This prevents "data races" where one part of your code is reading data while another part is changing it.

```python
let mut list = [1, 2, 3]
let r = &mut list
r[0] = 10     # OK

# let r2 = &list # Error! Someone is already changing it.
```

## The Golden Rule

You can have **many readers** OR **one writer**, but never both at once.

## Smart Lifetimes

Olive's compiler is smart. It knows exactly when you're done with a borrow. You don't have to worry about complex rules; the compiler figures out when a borrow ends by looking at where you last use a variable. This means you can write natural code and the compiler handles the safety checks behind the scenes.

