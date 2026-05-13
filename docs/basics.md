# Basic Syntax and Types

Olive uses indentation-based syntax: clean, structured, with a focus on readability. It adds explicit variable declarations and a static type system.

## Variables and Mutability

Variables are declared with `let`. By default, they're immutable.

```python
let name = "Olive"
# name = "New Name"  <-- Error: cannot reassign immutable variable
```

To make a variable mutable, use `let mut`:

```python
let mut count = 0
count = 1  # This is allowed
```

### Constants

Constants are declared with `const`. They must be initialized with a compile-time value and cannot be made mutable.

```python
const MAX_RETRIES = 5
const PI: float = 3.14159
```

### Augmented Assignment

Olive supports augmented assignment for all basic arithmetic operations:

```python
let mut x = 10
x += 5   # x is 15
x -= 3   # x is 12
x *= 2   # x is 24
x /= 4   # x is 6
x %= 5   # x is 1
x **= 3  # x is 1
```

## Data Types

Olive is statically typed with type inference. You usually don't need to write out the types unless you want to.

### Primitive Types

- `int` / `i64`: 64-bit signed integer (`42`, `-7`).
- `i32`, `i16`, `i8`: 32, 16, and 8-bit signed integers.
- `u64`, `u32`, `u16`, `u8`: Unsigned integers.
- `float` / `f64`: 64-bit floating-point (`3.14`, `-0.5`).
- `f32`: 32-bit floating-point.
- `str`: UTF-8 string (`"Hello"`).
- `bool`: Boolean (`True`, `False`).
- `None`: The absence of a value.

### F-Strings

Prefix a string literal with `f` and use `{}` to embed expressions:

```python
let name = "Olive"
let greeting = f"Hello, {name}!"
print(greeting) # Hello, Olive!
```

### Collection Types

Collections are generic and support type annotations:

- **Lists**: `let names: list[str] = ["Alice", "Bob"]`
- **Sets**: `let unique_ids: set[int] = {1, 2, 3}`
- **Dictionaries**: `let scores: dict[str, int] = {"Alice": 10, "Bob": 20}`
- **Tuples**: `let pair: (int, str) = (1, "One")`

## Control Flow

### Comparisons

Olive uses standard comparison operators:

- `==`: Equal.
- `!=`: Not equal.
- `<`, `>`: Less than, greater than.
- `<=`, `>=`: Less than or equal to, greater than or equal to.

### If Statements

```python
if score >= 90:
    print("Grade: A")
elif score >= 80:
    print("Grade: B")
else:
    print("Grade: C")
```

### Loops

Both `while` and `for` loops support an optional `else` block that runs if the loop finishes without hitting a `break`.

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

```python
for item in ["apple", "banana", "cherry"]:
    print(item)
```

#### Tuple Unpacking in Loops

```python
let points = [(1, 2), (3, 4), (5, 6)]
for (x, y) in points:
    print(f"Point at {x}, {y}")
```

### Error Handling

Olive handles errors through union return types and the `try` operator. See [Error Handling](error_handling.md) for the full walkthrough.

### Assertions

```python
assert x > 0, "x must be positive"
```

If the condition is false, the program halts immediately with the message.

## Comprehensions

Olive supports list, set, and dictionary comprehensions.

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
