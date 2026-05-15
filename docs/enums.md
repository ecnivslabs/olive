# Enums and Pattern Matching

Sometimes data can be one of several different things. In Olive, `enums` are used for this. They are ideal for representing a set of options, such as the status of a web request or different types of messages.

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

## Union Types and Discrimination

A union type like `Shape | Color` holds a value that could be any of the listed enum types. `match` handles all of them in one place:

```python
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

Enums can also be generic, which is particularly useful for representing optional values or results of computations.

```python
enum Option[T]:
    Some(T)
    None

fn find_item(id: int) -> Option[str]:
    if id == 1:
        return Some("Found it")
    return None

match find_item(1):
    Some(val): print(val)
    None: print("Not found")
```

The `Option` and `Result` enums are so useful that they are built into the language, but you can define your own generic enums whenever you need a type that can hold a variety of different types.
