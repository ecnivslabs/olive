# Basic Syntax and Types

Olive's syntax is heavily inspired by Python, emphasizing readability and structural clarity through indentation. However, it introduces explicit variable declarations and a robust type system.

## Variables and Mutability

In Olive, variables are declared using the `let` keyword. By default, all variables are **immutable**.

```python
let name = "Olive"
# name = "New Name"  <-- Error: cannot reassign immutable variable
```

To create a mutable variable, use the `let mut` syntax:

```python
let mut count = 0
count = 1  # This is allowed
```

### The Walrus Operator

Olive supports the walrus operator (`:=`), which allows you to assign values to variables within an expression:

```python
if (n := len(items)) > 10:
    print("Too many items")
```

## Data Types

Olive is statically typed but features powerful type inference.

### Primitive Types

- `int` / `i64`: 64-bit integers (`42`, `-7`).
- `i32`, `i16`, `i8`: 32, 16, and 8-bit signed integers.
- `u64`, `u32`, `u16`, `u8`: Unsigned integers.
- `float` / `f64`: 64-bit floating-point numbers (`3.14`, `-0.5`).
- `f32`: 32-bit floating-point numbers.
- `str`: UTF-8 encoded strings (`"Hello"`).
- `bool`: Boolean values (`True`, `False`).
- `None`: Represents the absence of a value.

### Collection Types

Collections are generic and can be type-annotated:

- **Lists**: `let names: list[str] = ["Alice", "Bob"]`
- **Sets**: `let unique_ids: set[int] = {1, 2, 3}`
- **Dictionaries**: `let scores: dict[str, int] = {"Alice": 10, "Bob": 20}`
- **Tuples**: `let pair: (int, str) = (1, "One")`

## Control Flow

### Comparisons

Olive distinguishes between value equality and object identity:

- `==`: Checks if two values are equal.
- `!=`: Checks if two values are not equal.
- `is`: Checks for **object identity** (whether two references point to the same object).
- `is not`: Checks if two references point to different objects.

```python
let a = [1, 2]
let b = [1, 2]
let c = a

print(a == b)   # True (same values)
print(a is b)   # False (different objects)
print(a is c)   # True (same object reference)
```

### If Statements

Standard Pythonic `if`, `elif`, and `else` structure:

```python
if score >= 90:
    print("Grade: A")
elif score >= 80:
    print("Grade: B")
else:
    print("Grade: C")
```

### Loops

Olive supports `while` and `for` loops, both of which can have an optional `else` block that executes if the loop finishes naturally (without a `break`).

#### While Loop

```python
let mut i = 0
while i < 5:
    print("Looping...")
    i += 1
else:
    print("Done!")
```

#### For Loop

The `for` loop iterates over collections or ranges:

```python
for item in ["apple", "banana", "cherry"]:
    print(item)
```

### Error Handling

Olive uses a Rust-inspired `Result` type pattern for error handling, moving away from traditional exceptions. The `try` expression can be used to handle errors gracefully:

```python
let value = try risky_operation()
```

You can also use union types for errors and explicitly match on them if needed.

### Assertions

You can use `assert` to verify assumptions during development. If the condition is false, the program will raise an error (and optionally print a custom message).

```python
assert x > 0, "x must be positive"
```

## Comprehensions

Olive supports powerful comprehension syntax for lists, sets, and dictionaries, allowing you to create new collections from existing ones concisely.

### List Comprehensions

```python
let numbers = [1, 2, 3, 4, 5]
let squares = [x * x for x in numbers if x % 2 == 0]
# squares is [4, 16]
```

### Set and Dictionary Comprehensions

```python
let unique_chars = {c for c in "abracadabra"}
let square_map = {x: x * x for x in range(5)}
```
