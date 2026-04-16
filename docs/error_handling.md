# Error Handling

Olive does not use exceptions. Instead, errors are represented as values returned from functions. This makes failure cases explicit in function signatures, forcing callers to handle them.

## The `Result` Enum

The standard library provides the `Result[T, E]` enum to represent operations that can fail. It consists of two variants:
* `Ok(T)`: Indicates success and wraps the returned value.
* `Err(E)`: Indicates failure and wraps the error details.

```rust
fn find_user(id: int) -> Result[User, str]:
    let user = db.query(id)
    if user == None:
        return Err("User not found")
    return Ok(user)
```

## Pattern Matching on `Result`

Because `Result` is an enum, use `match` blocks to handle success and failure paths:

```rust
match find_user(123):
    Ok(user):
        print(f"Found {user.name}")
    Err(msg):
        print(f"Failed: {msg}")
```

The compiler requires pattern matches to be exhaustive, ensuring you do not ignore the error variant.

## Propagating Errors (`try`)

To propagate an error to the caller, use the `try` keyword (or `?` shorthand). If the evaluated expression returns an `Err`, the current function returns early with that `Err`.

```rust
fn process_user(id: int) -> Result[None, str]:
    let user = try find_user(id)
    user.send_welcome_email()
    return Ok(None)
```

## Union Types for Simple Failures

For simple cases where a specific error payload is not required, use union types or optional variants:

```rust
fn get_config() -> dict | None:
    // Returns the configuration dictionary, or None if it is missing
    pass
```

## Assertions and Panic

Use assertions (`assert`) to catch invariant violations that represent logic bugs. Assertions are for unrecoverable errors.

```rust
assert len(items) > 0, "Cannot process an empty list"
```

If an assertion fails, execution aborts immediately, printing a diagnostic traceback.


