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
* `None`: The absence of a value. `None` is both the type and its single value, the same word used in a type annotation and in an expression.
* `Any`: A value of unknown or mixed type, resolved at runtime.

### Union Types

You can allow a variable or parameter to accept one of multiple specified types using a union (`|`):

```rust
let mut result: int | str = 10
result = "Error"
```

Union types are commonly resolved using pattern matching.

### The `Any` Type

When a value's type is not known until runtime, annotate it as `Any`. This is what lets a single collection hold a mix of types, such as the values returned when decoding JSON:

```rust
let row: [Any] = [1, "Olive", True, None]
```

A literal list with mixed element types widens to `[Any]` automatically. Use `type(value)` to inspect what an `Any` holds, and `None` for the absent case. Comparing an `Any` against `None` tests for the absent value:

```rust
if value == None:
    print("missing")
```

Annotating a list as `[T]` for a concrete `T` still enforces that every element is a `T`.

### String Formatting

Format strings by prefixing them with `f` and enclosing expressions in curly braces:

```rust
let name = "Olive"
let version = 1.0
print(f"Welcome to {name} v{version:.2f}")
```

### String Methods

Strings carry the common text operations:

```rust
print("HeLLo".upper())              // HELLO
print("HeLLo".lower())              // hello
print("  hi  ".strip())            // hi
print("a,b,c".split(","))          // [a, b, c]
print(",".join(["x", "y", "z"]))   // x,y,z
print("hello".replace("l", "L"))   // heLLo
print("hello".find("ll"))          // 2
print("hello".startswith("he"))    // True
```

Iterate a string by character:

```rust
for ch in "hi":
    print(ch)
```

## Collections

### Lists

Ordered, growable sequences of a single type:

```rust
let mut numbers = [1, 2, 3]
numbers.append(4)         // grows in place: [1, 2, 3, 4]
let first = numbers[0]
let last = numbers.pop()  // removes and returns 4
```

Lists also support `insert(index, value)`, `remove(index)`, `extend(other)`, `sort()`, and `reverse()`. Two lists join with `+`.

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
print(scores.get("Bob"))
```

A dict supports `get(key)`, `keys()`, `values()`, `items()`, and `remove(key)`. Iterate the keys directly, or the key-value pairs with `items()`:

```rust
for name in &scores:
    print(name)

for name, score in scores.items():
    print(f"{name}: {score}")
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
let id, status = pair  // Destructuring assignment
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

Iterate over a collection, or over an integer range written with `..` (exclusive of the end) or `..=` (inclusive):

```rust
for item in ["apple", "banana", "cherry"]:
    print(item)

for i in 0..5:        // 0, 1, 2, 3, 4
    print(i)

for i in 1..=5:       // 1, 2, 3, 4, 5
    print(i)
```

Iterating a collection by name consumes it. To keep it usable afterward, iterate over a borrow with `&`:

```rust
let names = ["a", "b"]
for n in &names:
    print(n)
print(len(names))     // names is still here
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
let squares = [x * x for x in &numbers if x % 2 == 0]  // Evaluates to [4, 16]
let unique_squares = {x * x for x in &numbers}         // Evaluates to {1, 4, 9, 16}
```

Iterating over `&numbers` borrows the list rather than consuming it, so it stays usable afterward. Iterating over `numbers` directly would move it into the comprehension.

## Built-in Functions

* `print(...)`: Writes output to standard out.
* `len(obj)`: Returns the number of elements in a collection.
* `type(obj)`: Returns the type name as a string.
* `assert(condition, message)`: Aborts execution with a message if the condition is false.

Integer ranges are written with the `..` and `..=` operators rather than a function, for example `0..n` or `1..=n`.
