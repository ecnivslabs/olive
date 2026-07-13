# Functions

Olive functions are first-class values. They can be assigned to variables, passed as arguments, and returned from other functions.

## Defining Functions

Functions are defined using the `fn` keyword. Parameters and return types can be explicitly annotated for clarity and compile-time boundary checks:

```rust
fn greet(name: str) -> str:
    return f"Hello, {name}"
```

## Arguments

### Default Values

You can provide default values for arguments, making them optional when calling the function.

```rust
import math

fn power(base: int, exponent: int = 2) -> int:
    return math.ipow(base, exponent)

print(power(10))     // 100
print(power(10, 3))  // 1000
```

### Variadic Arguments (*args and **kwargs)

To handle an unknown number of arguments, use `*` for positional arguments (captured as a list) and `**` for keyword arguments (captured as a dictionary).

```rust
fn log(message: str, *tags: str, **metadata: str):
    print(f"[{' | '.join(tags)}] {message}")
    for k, v in metadata.items():
        print(f"  {k}: {v}")

log("Server started", "info", "network", port="8080", host="localhost")
```

`tags` is a list and `metadata` is a dict, so they support the usual list and dict operations.

## Generics (Type Parameters)

Olive supports generics, enabling the creation of functions that work with any type. Type parameters are defined in square brackets after the function name.

```rust
fn first[T](items: [T]) -> T:
    return items[0]

let n = first([1, 2, 3])      // T is inferred as int
let s = first(["a", "b"])    // T is inferred as str
```

## Function Types

Function types describe a function's signature -- `fn(param types) -> return
type` -- so a parameter can require any function with that shape:

```rust
fn apply(f: fn(int) -> int, val: int) -> int:
    return f(val)

fn square(x: int) -> int: return x * x

print(apply(square, 5))  // 25
```

A call to a plain function by name (`square`, above) is always direct, at
full speed, whether it's the target of a normal call or being passed around
as a value. Calling *through* a `fn`-typed value (a variable, a struct
field, a list element) is an indirect call, resolved at runtime:

```rust
struct Op:
    apply: fn(int) -> int

fn double(x: int) -> int: return x * 2

let ops = [square, double]
print(ops[0](5))          // 25
print(Op(double).apply(5)) // 10
```

## Lambda Expressions

`lambda` writes a small function inline, without a name. Parameters can be
annotated in parentheses, or left bare when the type is inferable from
context:

```rust
let square = lambda (x: int): x * x
print(square(6))  // 36

fn apply(f: fn(int) -> int, val: int) -> int:
    return f(val)

print(apply(lambda x: x + 1, 41))  // 42, x inferred from apply's own signature
```

An unannotated parameter with no inferable context (no call-site hint and
no use that pins its type) is a compile error rather than a silent `Any`.

## Nested Functions and Closures

Functions can be defined inside other functions, and lambdas can read
variables from the function they're written in -- both are closures. Calling
one directly, from inside the function that defines it, costs nothing extra:
the captured variables are passed as ordinary trailing arguments, no
allocation, no heap record.

```rust
fn scale_all(values: [int], factor: int) -> [int]:
    fn scale(x: int) -> int:
        return x * factor
    return [scale(v) for v in values]
```

A closure that *escapes* -- returned, stored in a variable read later,
stored in a struct or list, or passed as a plain argument -- builds a small
heap record holding a copy of each variable it captured, the one-time cost
first-class closures have in any language. From then on it's an ordinary
`fn`-typed value: call it, store it, pass it around, exactly like a bare
function.

```rust
fn make_adder(n: int) -> fn(int) -> int:
    return lambda x: x + n

let add5 = make_adder(5)
print(add5(3))  // 8
```

Captures are copied at the moment the closure is built, not read live from
the original variable afterward:

```rust
fn make_reader() -> fn() -> int:
    let mut n = 1
    let g = lambda: n
    n = 99          // g already has its own copy of n's value
    return g

print(make_reader()())  // 1, not 99
```

Calling a closure directly by name only works from inside its defining
function (or a nested one that captures the same variables); calling it
from a sibling scope that never captured them is a compile error, since the
values it needs are not there. Only an escaped value can be called from
anywhere.

## Decorators and Directives

Olive uses tags to modify the behavior of functions at different stages.

### Decorators (@)

Decorators modify the function's behavior at **runtime** or affect code generation. Common decorators:

```rust
@memo
fn fibonacci(n: int) -> int:
    if n <= 1: return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

`@safe` marks an FFI function as safe to call without an `unsafe` block:

```rust
import "libm.so" as math:
    @safe
    fn sqrt(x: float) -> float
```

### Directives (#)

Directives are instructions for the **compiler** or tools. They don't affect runtime logic directly but change how the code is handled during the build process.

```rust
#[test]
fn test_math_logic():
    assert 2 + 2 == 4
```

When you run `pit test`, it identifies all functions tagged with `#[test]` and executes them.

`#[bench]` marks a function for `pit bench` instead:

```rust
#[bench]
fn fib_bench() -> int:
    return fib(20)
```

`pit bench` runs each `#[bench]` function through a fixed warmup, then samples a fixed number of timed calls, always at release optimization, and reports the mean, standard deviation, and minimum -- no more eyeballing timings. `pit bench --json` emits the same numbers as a JSON array for scripting.

### Doc Comments (///)

A `///` comment directly above a `fn`, `struct`, or `enum` documents it. A run of consecutive `///` lines is one doc comment; a decorator (`#[test]`, `@memo`) between the comment and the item doesn't break the association.

```rust
/// Adds two numbers.
fn add(a: int, b: int) -> int:
    return a + b
```

`pit doc [file]` renders a module's documented signatures as markdown into `target/doc/<module>.md`. Hovering a name in an editor shows the same doc text alongside its type.

