# Functions

Functions are first-class citizens in Olive. They are defined using the `fn` keyword and support optional type annotations for parameters and return values.

## Defining Functions

A basic function definition looks like this:

```python
fn greet(name: str) -> None:
    print("Hello, " + name)
```

If the return type is omitted, Olive will attempt to infer it.

## Function Arguments

### Parameters and Types

Parameters can be annotated with types and can also be marked as mutable if the function needs to modify its own local copy of the parameter:

```python
fn increment(mut value: int) -> int:
    value += 1
    return value
```

### Advanced Parameter Types

Olive is designed to support various parameter kinds, similar to Python:

- **Regular Parameters**: Standard positional or keyword arguments.
- **VarArgs (`*args`)**: For accepting a variable number of positional arguments.
- **KwArgs (`**kwargs`)**: For accepting a variable number of keyword arguments.

```python
fn sum_all(*numbers: int) -> int:
    let mut total = 0
    for n in numbers:
        total += n
    return total

fn configure(**options: str):
    pass

let s = sum_all(1, 2, 3, 4, 5)
configure(debug=True, verbose=False)
```

> **Note**: Currently, the compiler is optimized for regular parameters, with full support for variadic and keyword arguments being actively expanded in the backend.

## First-Class Functions

You can pass functions as arguments to other functions or assign them to variables:

```python
fn apply(f: (int) -> int, val: int) -> int:
    return f(val)

fn square(x: int) -> int:
    return x * x

let result = apply(square, 5)  # result is 25
```

## Closures and Lambdas

Olive supports anonymous functions (lambdas) and closures that can capture variables from their environment:

```python
let multiplier = 2
let double = lambda x: x * multiplier
print(double(10)) # 20
```

> **Note**: Lambda syntax and closures are planned for future versions of the MIR implementation.

## Recursion

Functions can call themselves recursively:

```python
fn fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

## Tail-Call Optimization (TCO)

Olive's compiler automatically identifies and optimizes tail-recursive functions. This means you can write recursive algorithms (like state machines or certain mathematical functions) without worrying about stack overflow errors, as they are transformed into efficient loops at the machine level.

## Decorators and Directives

Olive distinguishes between runtime decorators and compile-time directives:

- **@decorators**: Used for runtime behavior or meta-programming (e.g., `@memo`).
- **#[directives]**: Used for compiler transforms or metadata (e.g., `#[test]`, `#[derive]`).

### Runtime Decorators (@)

A great example is the built-in `@memo` decorator. If you have a computationally expensive recursive function—like our Fibonacci example above—you can dramatically speed it up by caching its results. 

```python
@memo
fn fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

Under the hood, `@memo` seamlessly hooks into a highly optimized, integer-keyed cache system. It intercepts function calls, returning the cached result if it exists, or running the function and storing the new result if it doesn't.

### Compiler Directives (#)

Directives like `#[test]` tell the `pit` toolchain how to handle specific items:

```python
#[test]
fn test_math():
    assert 1 + 1 == 2
```

