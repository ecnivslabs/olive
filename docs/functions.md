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

You can use function types to specify that a parameter must be a function with a specific signature.

```rust
fn apply(f: fn(int) -> int, val: int) -> int:
    return f(val)

fn square(x: int) -> int: return x * x

print(apply(square, 5))  // 25
```

## Decorators and Directives

Olive uses tags to modify the behavior of functions at different stages.

### Decorators (@)

Decorators modify the function's behavior at **runtime**. A common use case is caching results with `@memo`.

```rust
@memo
fn fibonacci(n: int) -> int:
    if n <= 1: return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

### Directives (#)

Directives are instructions for the **compiler** or tools. They don't affect runtime logic directly but change how the code is handled during the build process.

```rust
//[test]
fn test_math_logic():
    assert 2 + 2 == 4
```

When you run `pit test`, it identifies all functions tagged with `#[test]` and executes them.

