# Basic Syntax and Types

Olive's syntax is clean and structured. It's statically typed, but the compiler is smart enough to infer most types for you.

## Variables and Mutability

You declare variables with `let`. By default, variables are **immutable**—once a value is assigned, it cannot be changed. This simplifies reasoning about the program's state.

```python
let name = "Olive"
# name = "New Name"  <-- This would be an error
```

To allow a variable to change, use `let mut`:

```python
let mut count = 0
count = 1  # This is fine!
```

### Constants

For values that are known at compile-time and never change, use `const`. These are often optimized away entirely by the compiler.

```python
const MAX_RETRIES = 5
```

## Data Types

### Primitive Types

- `int`: Whole numbers (64-bit by default).
- `float`: Double-precision floating point numbers.
- `str`: UTF-8 encoded text.
- `bool`: `True` or `False`.
- `None`: Represents the absence of a value.

### Union Types

Sometimes a value could be one of several types. You can represent this using the pipe (`|`) symbol:

```python
let mut result: int | str = 10
result = "Error"  # This is valid because result can be an int or a str
```

Union types are particularly powerful when combined with pattern matching.

### String Formatting (F-Strings)

F-strings are the preferred way to build strings from variables. Just prefix the string with `f`:

```python
let name = "Olive"
let version = 1.0
print(f"Welcome to {name} v{version:.2f}")
```

You can even include basic expressions inside the braces.

## Collections

### Lists

Ordered, growable sequences of a single type.

```python
let mut numbers = [1, 2, 3]
numbers.push(4)
let first = numbers[0]
```

### Dictionaries

Key-value pairs for fast lookups.

```python
let scores = {"Alice": 95, "Bob": 88}
print(scores["Alice"])
```

### Tuples

Fixed-size groups of potentially different types.

```python
let pair: (int, str) = (1, "Active")
let (id, status) = pair  # Destructuring
```

## Control Flow

### If Statements

{{ ... }}

### Loops

#### For Loops

Used to iterate over collections or ranges.

```python
for item in ["apple", "banana", "cherry"]:
    print(item)

for i in range(5):
    print(i)
```

#### While Loops

```python
let mut i = 0
while i < 5:
    print(i)
    i += 1
```

## Comprehensions

A concise way to create new collections from existing ones:

```python
let numbers = [1, 2, 3, 4]
let squares = [x * x for x in numbers if x % 2 == 0]  # [4, 16]
```

## Built-in Functions

- `print(...)`: Outputs values to the console.
- `len(obj)`: Returns the length of a collection.
- `type(obj)`: Returns the type of the value as a string.
- `range(stop)` / `range(start, stop)`: Generates a sequence of integers.
- `assert(condition, message)`: Stops the program if the condition is false.
