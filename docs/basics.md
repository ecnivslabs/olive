# Syntax and Basic Types

Olive is statically typed with a clean, indentation-based syntax. Type annotations are optional in most cases because the compiler infers them.

## Variables and Mutability

Declare variables using the `let` keyword. Variables are immutable by default:

```rust
let name = "Olive"
// name = "New Name"  // Compile-time error
```

To define a mutable variable, use `let mut`:

```rust
let mut count = 0
count = 1
```

### Constants

Use `const` for values that must be evaluated at compile-time:

```rust
const MAX_RETRIES = 5
```

## Data Types

### Primitive Types

* `int`: 64-bit signed integer.
* `i8`, `i16`, `i32`, `i64`: Specific-width signed integers.
* `u8`, `u16`, `u32`, `u64`, `usize`: Unsigned integers.
* `float`: 64-bit floating-point number.
* `f32`, `f64`: Specific-width floating-point numbers.
* `str`: UTF-8 encoded string.
* `bool`: Boolean (`True` or `False`).
* `None`: Represents the absence of a value.

### Union Types

You can allow a variable or parameter to accept one of multiple specified types using a union (`|`):

```rust
let mut result: int | str = 10
result = "Error"
```

Union types are commonly resolved using pattern matching.

### String Formatting

Format strings by prefixing them with `f` and enclosing expressions in curly braces:

```rust
let name = "Olive"
let version = 1.0
print(f"Welcome to {name} v{version:.2f}")
```

## Collections

### Lists

Ordered, growable sequences of a single type:

```rust
let mut numbers = [1, 2, 3]
numbers.push(4)
let first = numbers[0]
```

### Fixed Arrays

Fixed-size arrays with a known length at compile time:

```rust
let mut matrix: [int; 16]
```

### Bytes

Mutable, growable byte buffers for binary data. Indexing reads and writes single bytes and compiles to direct memory access. Passing a `bytes` value to Python converts it to a Python `bytes` object:

```rust
let mut buf = bytes_new(16)        // zero-filled, length 16
buf[0] = 255
let first = buf[0]                 // 255
bytes_push(buf, 7)                 // append one byte
bytes_push_u16_le(buf, 513)        // append u16, little-endian
bytes_push_u32_le(buf, 70000)      // append u32, little-endian
let size = len(buf)
```

### Dictionaries

Hash-map key-value collections:

```rust
let scores = {"Alice": 95, "Bob": 88}
print(scores["Alice"])
```

### Sets

Unordered collections of unique elements:

```rust
let valid_ids = {101, 102, 103}
```

### Tuples

Fixed-size, heterogeneous collections:

```rust
let pair: (int, str) = (1, "Active")
let (id, status) = pair  // Destructuring assignment
```

## Control Flow

### If Statements

Conditional branches use `if`, `elif`, and `else`:

```rust
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

```rust
for item in ["apple", "banana", "cherry"]:
    print(item)

for i in range(5):
    print(i)
```

#### While Loops

```rust
let mut i = 0
while i < 5:
    print(i)
    i += 1
```

## Comprehensions

Generate lists, sets, or dictionaries from iterables:

```rust
let numbers = [1, 2, 3, 4]
let squares = [x * x for x in numbers if x % 2 == 0]  // Evaluates to [4, 16]
let unique_squares = {x * x for x in numbers}         // Evaluates to {1, 4, 9, 16}
```

## Built-in Functions

* `print(...)`: Writes output to standard out.
* `len(obj)`: Returns the number of elements in a collection.
* `type(obj)`: Returns the type name as a string.
* `range(stop)` / `range(start, stop)`: Generates an integer range iterator.
* `assert(condition, message)`: Aborts execution with a message if the condition is false.

