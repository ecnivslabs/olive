# Enums and Pattern Matching

Enums let you define a type as a fixed set of named variants. They're useful for representing state, modeling data that can take multiple distinct forms, and returning success or error values from functions.

## Defining Enums

```python
enum WebResponse:
    Success
    NotFound
    ServerError
```

Enum variants can carry data. Each variant specifies the types of its associated values:

```python
enum Message:
    Quit
    Move(int, int)          # x and y coordinates
    Write(str)              # text to write
    ChangeColor(int, int, int)  # r, g, b
```

## Pattern Matching with `match`

`match` lets you branch on enum variants and extract their associated data in one step:

```python
fn process_message(msg: Message) -> None:
    match msg:
        Quit:
            print("Quitting...")
        Move(x, y):
            print(f"Moving to {x}, {y}")
        Write(text):
            print(text)
        ChangeColor(r, g, b):
            print(f"Changing color to {r}, {g}, {b}")
```

### Wildcards

Use `_` as a catch-all when you only care about specific variants:

```python
fn handle_response(res: WebResponse) -> None:
    match res:
        Success:
            print("Everything went fine.")
        _:
            print("Something went wrong.")
```

### Pattern Bindings

You can bind a matched value to a name and use it inside the branch:

```python
fn log_status(status: int):
    match status:
        200:
            print("OK")
        code:
            print(f"Received non-200 status: {code}")
```

Here, `code` matches any value and makes it available as a variable inside that branch.

Enums and `match` work well together because the compiler knows all possible variants. If you forget to handle one, the compiler can catch it before the code runs.
