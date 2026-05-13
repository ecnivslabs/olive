# Error Handling

Olive doesn't use exceptions. Errors are values, and functions that can fail say so in their return type. The caller decides what to do with them.

## Union Return Types

A function signals failure by returning a union type. The first member is the success value; any other member is an error:

```python
fn divide(a: int, b: int) -> int | str:
    if b == 0:
        return "division by zero"
    return a / b
```

The caller gets back either an `int` or a `str`. The type system tracks which.

## Handling Results with `match`

`match` on a union lets you handle each case explicitly:

```python
fn safe_divide(a: int, b: int):
    match divide(a, b):
        int(result):
            print(f"Result: {result}")
        str(msg):
            print(f"Error: {msg}")
```

The compiler checks that you've covered every member of the union. If you miss one, you'll get a non-exhaustive match error before the code runs.

## Propagating Errors with `try`

If you don't want to handle an error locally — just pass it up to the caller — use `try` or the `?` postfix. Both are identical:

```python
fn load_config() -> dict | str:
    let raw = try read_file("config.txt")
    # or: let raw = read_file("config.txt")?
    return parse(raw)
```

When `read_file` returns an error, `try` immediately returns that error from `load_config`. When it succeeds, execution continues with the unwrapped value.

For this to work, the calling function's return type must include the error type that could propagate.

## Chaining

`try` composes naturally. Each step either continues or exits early:

```python
fn process(path: str) -> Result | str:
    let raw    = try read_file(path)
    let parsed = try parse_json(raw)
    let result = try validate(parsed)
    return result
```

If any step fails, the rest don't run. The error travels back to whoever called `process`.

## Returning Multiple Error Types

A function can return more than one kind of error:

```python
enum ParseError:
    InvalidSyntax
    UnexpectedEnd

fn parse(input: str) -> int | str | ParseError:
    if input == "":
        return ParseError.UnexpectedEnd
    if input == "bad":
        return "malformed input"
    return int(input)
```

The caller matches on all three cases.

## Assertions

For conditions that should never be false in correct code, use `assert`:

```python
assert index >= 0, "index must be non-negative"
```

An assertion failure crashes the program immediately with the message. Use this for invariants you want to catch during development, not for recoverable errors.
