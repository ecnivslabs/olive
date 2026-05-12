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

### Constants

Constants are declared using the `const` keyword. They must be initialized with a value that can be determined at compile-time and cannot be mutable.

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

Olive is statically typed but features powerful type inference.

### Primitive Types

- `int` / `i64`: 64-bit integers (`42`, `-7`).
- `i32`, `i16`, `i8`: 32, 16, and 8-bit signed integers.
- `u64`, `u32`, `u16`, `u8`: Unsigned integers.
- `float` / `f64`: 64-bit floating-point numbers (`3.14`, `-0.5`).
- `f32`: 32-bit floating-point numbers.
- `str`: UTF-8 encoded strings (`"Hello"`).
- `bool`: Boolean values (`True`, `False`).
- `None`: Represents the absence of a value (maps to the internal `Null` type).

### F-Strings (Formatted Strings)

Olive supports f-strings for easy string interpolation. Prefix a string with `f` and use `{}` to embed expressions:

```python
let name = "Olive"
let greeting = f"Hello, {name}!"
print(greeting) # Hello, Olive!
```

### Collection Types

Collections are generic and can be type-annotated:

- **Lists**: `let names: list[str] = ["Alice", "Bob"]`
- **Sets**: `let unique_ids: set[int] = {1, 2, 3}`
- **Dictionaries**: `let scores: dict[str, int] = {"Alice": 10, "Bob": 20}`
- **Tuples**: `let pair: (int, str) = (1, "One")`

## Control Flow

### Comparisons

Olive uses standard comparison operators:

- `==`: Checks if two values are equal.
- `!=`: Checks if two values are not equal.
- `<`: Less than.
- `>`: Greater than.
- `<=`: Less than or equal to.
- `>=`: Greater than or equal to.

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

#### Tuple Unpacking in Loops

You can unpack tuples directly in the `for` loop header:

```python
let points = [(1, 2), (3, 4), (5, 6)]
for (x, y) in points:
    print(f"Point at {x}, {y}")
```

### Error Handling

Olive uses a Rust-inspired `Result` type pattern for error handling. A `Result` is typically a union type like `Type | Error`.

#### The `try` Expression

The `try` keyword (or the `?` postfix operator) can be used to handle errors gracefully by propagating them up the call stack if they occur:

```python
# Both are equivalent
let value = try risky_operation()
let value = risky_operation()?
```

#### Union Types

You can define functions that return multiple types to represent success or failure:

```python
fn divide(a: int, b: int) -> int | str:
    if b == 0:
        return "Division by zero"
    return a / b
```

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
