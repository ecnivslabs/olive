# Functions

Functions are first-class values in Olive. They're defined with the `fn` keyword and support optional type annotations on parameters and return values.

## Defining Functions

```python
fn greet(name: str) -> None:
    print("Hello, " + name)
```

If the return type annotation is omitted, Olive infers it.

## Function Arguments

### Parameters and Types

Parameters can carry type annotations. If a function needs to modify its own local copy of a parameter, mark it `mut`:

```python
fn increment(mut value: int) -> int:
    value += 1
    return value
```

### Variadic and Keyword Arguments

Olive supports variadic (`*args`) and keyword (`**kwargs`) parameters, fully implemented in the MIR backend:

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

## First-Class Functions

Functions can be passed as arguments or assigned to variables. The type of a function is written as `(ArgTypes) -> ReturnType`:

```python
fn apply(f: (int) -> int, val: int) -> int:
    return f(val)

fn square(x: int) -> int:
    return x * x

let result = apply(square, 5)  # result is 25
```

## Lambdas

Lambda (anonymous function) syntax is planned but not yet implemented. Use named functions in the meantime.

## Recursion

```python
fn fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

## Tail-Call Optimization (TCO)

The compiler automatically identifies tail-recursive calls and transforms them into direct jumps. Recursive functions structured as tail calls won't overflow the stack; they compile to the same code as an iterative loop.

## Decorators and Directives

Olive distinguishes between two kinds of function annotations:

- **`@decorators`**: Applied at runtime. Used for meta-programming, caching, and other runtime behavior.
- **`#[directives]`**: Applied at compile time. Used to pass instructions to the compiler or toolchain.

### Runtime Decorators (`@`)

The built-in `@memo` decorator caches function results by argument. It's useful for expensive recursive computations:

```python
@memo
fn fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)
```

`@memo` hooks into an integer-keyed cache. On each call, it checks whether the result for those arguments is already stored. If so, it returns it immediately. If not, it runs the function and stores the result before returning.

### Compiler Directives (`#`)

Directives tell the `pit` toolchain how to treat specific items. The most common is `#[test]`, which marks a function for the test runner:

```python
#[test]
fn test_math():
    assert 1 + 1 == 2
```

Running `pit test` finds all functions with `#[test]` and executes them, reporting results.
