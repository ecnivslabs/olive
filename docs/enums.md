# Enums and Pattern Matching

Enums (short for enumerations) allow you to define a type by enumerating its possible variants. They are incredibly useful for representing state, dealing with different types of data under a single type, or returning values that could be a success or an error.

## Defining Enums

You define an enum using the `enum` keyword. Here's a simple example representing different kinds of web responses:

```python
enum WebResponse:
    Success
    NotFound
    ServerError
```

Enums in Olive can also hold data. Each variant can specify the types of data it carries:

```python
enum Message:
    Quit
    Move(int, int)  # x and y coordinates
    Write(str)      # text to write
    ChangeColor(int, int, int) # r, g, b
```

## Pattern Matching with `match`

The true power of enums shines when you pair them with the `match` statement. `match` allows you to branch your logic based on the specific variant of an enum and easily extract its associated data.

```python
fn process_message(msg: Message) -> None:
    match msg:
        case Quit:
            print("Quitting...")
        case Move(x, y):
            print(f"Moving to {x}, {y}")
        case Write(text):
            print(text)
        case ChangeColor(r, g, b):
            print(f"Changing color to {r}, {g}, {b}")
```

### Wildcards

You can use a wildcard (`_`) if you only care about a few specific variants and want a catch-all for the rest:

```python
fn handle_response(res: WebResponse) -> None:
    match res:
        case Success:
            print("Everything went perfectly!")
        case _:
            print("Something went wrong.")
```

Using enums and pattern matching together brings an incredible amount of expressiveness and safety to your code, ensuring you explicitly handle all possible states of a value.
