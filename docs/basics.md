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

Integer and float literals accept `_` as a digit separator for readability, in decimal, hex, octal, and binary literals alike. It must sit strictly between two digits -- not leading, trailing, doubled, or next to the `.`:

```rust
let population = 8_100_000_000
let price = 19_99.99
let mask = 0xFF_FF_00_00
```

### Arithmetic Operators

`+ - * /` work as expected, with one deliberate divergence from Python:
integer `/` truncates toward zero (Rust semantics) rather than flooring.
`**` is exponentiation, right-associative, binding tighter than unary minus
so `-2 ** 2` is `-4`, not `4`:

```rust
print(2 ** 10)     // 1024
print(-2 ** 2)     // -4
print(2 ** 3 ** 2) // 512, ** groups right: 2 ** (3 ** 2)
print(2.0 ** 0.5)  // 1.4142135623730951
```

`/`'s truncation means `-7 / 2` is `-3`, not Python's `-4`. Reach for
`math.floordiv` (int) or `math.ffloordiv` (float) when you want the floor
instead:

```rust
import math

print(-7 / 2)               // -3, truncated
print(math.floordiv(-7, 2)) // -4, floored
```

### Union Types

You can allow a variable or parameter to accept one of multiple specified types using a union (`|`):

```rust
let mut result: int | str = 10
result = "Error"
```

Union types are commonly resolved using pattern matching.

### Type Aliases

`type Name = TypeExpr` gives an existing type a second name. It has no runtime
identity of its own: the alias and its target are the same type everywhere,
interchangeably. This is mainly useful for naming a union that would
otherwise be repeated at every call site:

```rust
struct ParseError:
    msg: str

type ParseResult = int | ParseError

fn parse(s: str) -> ParseResult:
    if s == "":
        return ParseError("empty input")
    return len(s)
```

An alias may reference another alias, but not itself, directly or through a
cycle (`type A = B` / `type B = A` is a compile error).

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

A trailing `=` inside the braces is the debug form: it prints the source
text of the expression, an `=`, and its value, useful for quick print
debugging. A format spec still applies after the `=`:

```rust
let x = 5
print(f"{x=}")       // x=5
print(f"{x=:04d}")   // x=0005
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
print("ab" * 3)                     // ababab
```

The full method set: `upper`, `lower`, `strip`/`lstrip`/`rstrip` (optionally
`strip(chars)` to trim a specific character set instead of whitespace),
`split()` (no argument splits on whitespace runs) / `split(sep)`, `join`,
`replace`, `find` / `rfind` (search from the end) / `count`, `contains`,
`startswith` / `endswith`, `removeprefix` / `removesuffix`, `repeat(n)`
(same as `s * n`), `splitlines`, `title` / `capitalize`, `zfill(width)`,
`ljust(width, fillchar=" ")` / `rjust(width, fillchar=" ")` /
`center(width, fillchar=" ")`, `partition(sep)` (returns a
`(before, sep, after)` tuple), and the `isdigit` / `isalpha` / `isspace` /
`isupper` / `islower` family.

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

Lists also support `insert(index, value)`, `remove(index)`, `extend(other)`,
`sort()`, `reverse()`, `count(x)`, `index(x)` (faults if `x` is absent; pair
it with `in` to check first), and `clear()`. Two lists join with `+`;
`xs * n` repeats a list `n` times (`n <= 0` gives an empty list), deep-copying
elements so each repetition is independent.

### Slicing

Lists and strings slice with `[start:stop:step]`. Any part can be omitted, and
negative steps walk backwards:

```rust
let xs = [1, 2, 3, 4, 5]
print(xs[1:4])     // [2, 3, 4]
print(xs[::-1])    // [5, 4, 3, 2, 1]
print("hello"[1:3])  // el
```

A slice is a new value; mutating it does not touch the original.

### Fixed Arrays

Fixed-size arrays with a known length at compile time. The length is structural; to actually allocate a fixed-size buffer, use `bytes_new(n)` or a list with `list_new(n)`.

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

Dicts and sets are hash-backed, so iteration order is unspecified and may differ from insertion order. Do not rely on it; sort the keys if you need a stable order.

A dict supports `get(key)` / `get(key, default)`, `keys()`, `values()`,
`items()`, `remove(key)`, `pop(key)` (faults if absent) / `pop(key, default)`,
`setdefault(key, default)` (returns the existing value, or inserts and
returns `default`), `update(other)` (merges `other` in, overwriting on a key
conflict; `other` itself is untouched), and `clear()`. Iterate the keys
directly, or the key-value pairs with `items()`:

```rust
for name in scores:
    print(name)

for name, score in scores.items():
    print(f"{name}: {score}")
```

### Sets

Unordered collections of unique elements:

```rust
let valid_ids = {101, 102, 103}
```

A set supports `add(x)`, `contains(x)`, `remove(x)` (faults if `x` is
absent), `discard(x)` (never faults), and `clear()`. The algebra operators
work directly on two sets: `&` intersection, `|` union, `-` difference, `^`
symmetric difference.

### Tuples

Fixed-size, heterogeneous collections:

```rust
let pair: (int, str) = (1, "Active")
let id, status = pair  // Destructuring assignment
```

A `*name` target gathers everything not claimed by a plain name into a
list; unlike a tuple's fixed arity, this works against a list of any
runtime length, and faults if there aren't enough elements for the plain
names. At most one `*name` per target list, in `let` or plain assignment:

```rust
let scores = [90, 85, 72, 68, 55]
let highest, *rest = scores
print(highest)   // 90
print(rest)      // [85, 72, 68, 55]

let first, *middle, last = scores
print(first, last)  // 90 55
print(middle)        // [85, 72, 68]

let mut a = 0
let mut b = [0]
a, *b = scores   // plain assignment works the same way
```

Parentheses around a `let` target list are pure grouping, with no effect
on meaning; `pit fmt` normalizes them away:

```rust
let (id, status) = pair   // identical to `let id, status = pair`
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

### Conditional Expressions

An `if` can be used inline as an expression:

```rust
let grade = "pass" if score >= 50 else "fail"
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

A range steps by 1 by default; `by` sets a different, contextual step (not a
reserved word -- it only means anything right after a range). A negative step
walks backward, which is how a descending range is written:

```rust
for i in 0..10 by 2:    // 0, 2, 4, 6, 8
    print(i)

for i in 10..0 by -1:   // 10, 9, ..., 1
    print(i)

for i in 10..=0 by -2:  // 10, 8, 6, 4, 2, 0
    print(i)
```

A literal `by 0` is a compile error; a step computed at runtime that turns
out to be 0 faults instead (it would never advance). A stepless descending
range (`5..0`) is simply empty, matching Python's `range(5, 0)`.

`enumerate`/`zip` exist only written directly as a loop's (or comprehension
clause's) iterable -- not as a value to assign, pass around, or return:

```rust
let fruits = ["apple", "banana", "cherry"]
for i, fruit in enumerate(fruits):
    print(i, fruit)          // 0 apple / 1 banana / 2 cherry

for i, fruit in enumerate(fruits, 1):
    print(i, fruit)          // 1 apple / 2 banana / 3 cherry

let prices = [3, 1, 4]
for fruit, price in zip(fruits, prices):
    print(fruit, price)
```

A range also works directly as the right side of `in`/`not in`, testing
membership without building a loop. A stepped range can't be tested this way
(there's no cheap way to check membership against a step without walking
it) -- write the step check alongside a plain range instead:

```rust
let n = 5
print(n in 0..10)     // True
print(15 not in 0..10) // True
print(n in 0..10 and n % 2 == 0) // stepped membership, written out
```

Iteration borrows the collection (`enumerate`/`zip` borrow every iterable
they're given), never copies it: the iterable stays usable after the loop,
and mutating or reassigning it while the loop is still running is a compile
error, not a race:

```rust
let names = ["a", "b"]
for n in names:
    print(n)
print(len(names))     // names is still here

let mut xs = [1, 2, 3]
for x in xs:
    xs.append(x)       // compile error: xs is borrowed by the loop
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

Comprehensions borrow the iterable (never copy it), so the collection stays
usable afterward.

## Built-in Functions

* `print(a, b, ...)`: Writes any number of values to standard out, space-separated.
* `len(obj)`: Returns the number of elements in a collection.
* `type(obj)`: Returns the type name as a string.
* `assert(condition, message)`: Aborts execution with a message if the condition is false.
* `abs(n)`: Absolute value; works on `int` and `float`.
* `round(x)` / `round(x, ndigits)`: Rounds a float, to the nearest int or to `ndigits` decimal places.
* `input(prompt: str = "")`: Writes `prompt` with no trailing newline, reads and returns one line from stdin.
* `sum(xs)`, `min(xs)`, `max(xs)`: Reduce a list; `min`/`max` also take two plain arguments, `min(a, b)`.
* `sorted(xs)`: Returns a new sorted list; the source list is untouched.
* `reversed(xs)`: Returns a new list in reverse order; the source list is untouched.
* `any(xs)` / `all(xs)`: Whether any/all elements of a `[bool]` (or `[Any]` holding bools) are true.

Integer ranges are written with the `..` and `..=` operators rather than a function, for example `0..n` or `1..=n`. A range also works directly on the right of `in`/`not in`: `x in 0..10`.
