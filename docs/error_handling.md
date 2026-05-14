# Error Handling

Olive doesn't use "exceptions" like some other languages. Instead, errors are just values that a function can return. This makes it clear exactly which functions can fail and what kind of errors they might give you.

## Returning Errors

A function signals an error by returning a "union type." This just means it can return one of several different types of data.

```python
fn divide(a: int, b: int) -> int | str:
    if b == 0:
        return "division by zero"  # Returning an error message
    return a / b                   # Returning the result
```

In this case, the function returns either an `int` (the result) or a `str` (the error message).

## Handling Errors with `match`

The most common way to handle an error is with `match`. The compiler ensures you've handled every possible outcome.

```python
fn safe_divide(a: int, b: int):
    match divide(a, b):
        int(result):
            print(f"Result: {result}")
        str(error_msg):
            print(f"Error: {error_msg}")
```

## The `try` Operator

Sometimes you don't want to handle an error right away. You might want to just "pass it up" to whatever called your function. You can do this easily with `try` (or `?` at the end of the line).

```python
fn load_and_parse(path: str) -> dict | str:
    # If read_file fails, load_and_parse will exit early and return the error
    let raw = try read_file(path)
    
    # If we get here, raw is the actual file content
    return parse_json(raw)
```

## Multiple Error Types

A function can return several different kinds of errors. You just list them in the return type:

```python
enum FileError:
    NotFound
    NoPermission

fn read_config() -> str | FileError | str:
    # This could return the content, a FileError, or a plain error message
    pass
```

## When to use `assert`

For things that should *never* happen if your code is written correctly, use `assert`:

```python
assert score >= 0, "Score cannot be negative"
```

If the assertion fails, the program stops immediately. Use this to catch bugs during development.

