# Functions

Functions are the building blocks of your Olive program. They're easy to define and can be passed around just like any other value.

## Defining Functions

You define a function using the `fn` keyword. You can also specify what types of data it takes and what it returns.

```python
fn greet(name: str) -> str:
    return "Hello, " + name
```

If you don't specify the return type, Olive will try to figure it out for you.

## Arguments

### Simple Arguments

```python
fn add(a: int, b: int) -> int:
    return a + b
```

### Flexible Arguments (*args and **kwargs)

Sometimes you don't know how many arguments a function will get. Olive supports flexible arguments:

```python
fn sum_all(*numbers: int) -> int:
    let mut total = 0
    for n in numbers:
        total += n
    return total

fn configure(**options: str):
    # options is a dictionary of the arguments passed
    pass

sum_all(1, 2, 3)
configure(debug="true", theme="dark")
```

## Passing Functions Around

In Olive, functions are "first-class," which means you can pass them as arguments to other functions or save them in variables.

```python
fn apply_operation(f: (int) -> int, value: int) -> int:
    return f(value)

fn double(x: int) -> int:
    return x * 2

let result = apply_operation(double, 5)  # result is 10
```

## Smart Recursion

Olive is smart about recursive functions (functions that call themselves). If a function is structured in a way that its last action is calling itself (called a "tail call"), the compiler will automatically optimize it so it runs just as fast as a regular loop and never runs out of memory.

## Special Tags (@ and #)

Olive uses two types of tags to add extra behavior to functions:

### Decorators (@)

Decorators change how a function works at **runtime**. For example, the `@memo` decorator can speed up your code by remembering the results of previous calls:

```python
@memo
fn expensive_calculation(n: int) -> int:
    # This will now remember results so it doesn't have to recalculate them
    pass
```

### Directives (#)

Directives give instructions to the **compiler** or tools. The most common one is `#[test]`, which tells the `pit` tool that this function is a test:

```python
#[test]
fn test_addition():
    assert 1 + 1 == 2
```

When you run `pit test`, it will find all functions tagged with `#[test]` and run them for you.

