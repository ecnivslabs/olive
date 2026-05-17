# Syntax and Basic Types

Olive is statically typed with a clean, indentation-based syntax. Type annotations are optional in most cases because the compiler infers them.

## Variables and Mutability

Declare variables using the `let` keyword. Variables are immutable by default:

```python
let name = "Olive"
# name = "New Name"  # Compile-time error
```

To define a mutable variable, use `let mut`:

```python
let mut count = 0
count = 1
```

### Constants

Use `const` for values that must be evaluated at compile-time:

```python
const MAX_RETRIES = 5
```

## Data Types

### Primitive Types

* `int`: 64-bit signed integer.
* `float`: 64-bit floating-point number.
* `str`: UTF-8 encoded string.
* `bool`: Boolean (`True` or `False`).
* `None`: Represents the absence of a value.

### Union Types

You can allow a variable or parameter to accept one of multiple specified types using a union (`|`):

```python
let mut result: int | str = 10
result = "Error"
```

Union types are commonly resolved using pattern matching.

### String Formatting

Format strings by prefixing them with `f` and enclosing expressions in curly braces:

```python
let name = "Olive"
let version = 1.0
print(f"Welcome to {name} v{version:.2f}")
```

## Collections

### Lists

Ordered, growable sequences of a single type:

```python
let mut numbers = [1, 2, 3]
numbers.push(4)
let first = numbers[0]
```

### Dictionaries

Hash-map key-value collections:

```python
let scores = {"Alice": 95, "Bob": 88}
print(scores["Alice"])
```

### Tuples

Fixed-size, heterogeneous collections:

```python
let pair: (int, str) = (1, "Active")
let (id, status) = pair  # Destructuring assignment
```

## Control Flow

### If Statements

Conditional branches use `if`, `elif`, and `else`:

```python
if score >= 90:
    print("A")
elif score >= 80:
    print("B")
else:
    print("C")
```

### Loops

#### For Loops

Iterate over collections, iterators, or ranges:

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

Generate lists or dictionaries from iterables:

```python
let numbers = [1, 2, 3, 4]
let squares = [x * x for x in numbers if x % 2 == 0]  # Evaluates to [4, 16]
```

## Built-in Functions

* `print(...)`: Writes output to standard out.
* `len(obj)`: Returns the number of elements in a collection.
* `type(obj)`: Returns the type name as a string.
* `range(stop)` / `range(start, stop)`: Generates an integer range iterator.
* `assert(condition, message)`: Aborts execution with a message if the condition is false.

