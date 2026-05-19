# Enums and Pattern Matching

Enums (enumerations) represent data that can be one of several distinct variants. Variants can be simple flags or carry structured associated values.

## Defining Enums

```rust
enum WebResponse:
    Success
    NotFound
    ServerError
```

Enum variants can carry data. Each variant specifies the types of its associated values:

```rust
enum Message:
    Quit
    Move(int, int)          // x and y coordinates
    Write(str)              // text to write
    ChangeColor(int, int, int)  // r, g, b
```

## Pattern Matching with `match`

`match` lets you branch on enum variants and extract their associated data in one step:

```rust
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

```rust
fn handle_response(res: WebResponse) -> None:
    match res:
        Success:
            print("Everything went fine.")
        _:
            print("Something went wrong.")
```

### Pattern Bindings

You can bind a matched value to a name and use it inside the branch:

```rust
fn log_status(status: int):
    match status:
        200:
            print("OK")
        code:
            print(f"Received non-200 status: {code}")
```

Here, `code` matches any value and makes it available as a variable inside that branch.

### Match Guards

A pattern can carry an `if` condition. The arm only matches when the guard
holds, and later arms are tried otherwise:

```rust
match status:
    200:
        print("OK")
    code if code >= 500:
        print("server error")
    code:
        print(f"other {code}")
```

The compiler enforces exhaustive pattern matching. Failing to match a variant triggers a compile-time error.

## Union Types and Discrimination

A union type like `Shape | Color` holds a value that could be any of the listed enum types. `match` handles all of them in one place:

```rust
enum Shape:
    Circle(float)
    Square(float)

enum Color:
    Red
    Blue

fn describe(val: Shape | Color) -> str:
    match val:
        Circle(r):
            return f"circle with radius {r}"
        Square(s):
            return f"square with side {s}"
        Red:
            return "red"
        Blue:
            return "blue"
```

The compiler checks that every variant from every enum in the union is handled. If you add a new variant to `Shape` and forget to update the match, you'll get a compile error.

## Generic Enums

Enums can also be generic.

```rust
enum Response[T]:
    Data(T)
    Error(str)
    Empty

fn find_item(id: int) -> Response[str]:
    if id == 1:
        return Data("Found it")
    return Empty

match find_item(1):
    Data(val): print(val)
    Error(msg): print(msg)
    Empty: print("Not found")
```
