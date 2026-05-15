# Error Handling

Errors should not be "surprises" that crash a program. For this reason, Olive does not use exceptions. Instead, errors are values that a function returns, making it clear which functions can fail and how those failures should be managed.

## The `Result` Idiom

The most common way to handle errors in Olive is using the `Result[T, E]` enum. It can be either `Ok(T)` (the successful result) or `Err(E)` (the error).

```python
fn find_user(id: int) -> Result[User, str]:
    let user = db.query(id)
    if user == None:
        return Err("User not found")
    return Ok(user)
```

## Handling Errors with `match`

Because `Result` is an enum, you can use `match` to handle both the success and error cases. The compiler will ensure you don't forget to handle the error.

```python
match find_user(123):
    Ok(user):
        print(f"Found {user.name}")
    Err(msg):
        print(f"Failed: {msg}")
```

## The `try` Operator

If you're in a function that also returns a `Result`, you can use the `try` keyword (or the `?` shorthand) to pass an error up to the caller if something fails.

```python
fn process_user(id: int) -> Result[None, str]:
    # If find_user returns Err, this function returns early with that same error
    let user = try find_user(id)
    
    # Otherwise, user is the actual User object
    user.send_welcome_email()
    return Ok(None)
```

## Union Types for Simple Errors

For simpler cases, you can use union types directly. This is useful when you just want to return a value or a specific error type.

```python
fn get_config() -> dict | None:
    # returns the config dictionary or None if it's missing
    pass
```

## When to use `assert`

Use `assert` for things that should **never** happen in a correctly written program. These are "unrecoverable" errors that indicate a bug in your logic.

```python
assert len(items) > 0, "Cannot process an empty list"
```

If an assertion fails, Olive stops the program immediately and shows you exactly where the failure occurred. This is your best friend during development.

