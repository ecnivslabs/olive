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

A variant's own payload positions accept `_` too, for fields you don't need:

```rust
enum Shape:
    Circle(float)
    Rectangle(float, float)

fn is_circle(s: Shape) -> bool:
    match s:
        Circle(_):
            return True
        Rectangle(_, _):
            return False

fn main():
    print(is_circle(Circle(2.0))) // True
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

Here, `code` matches any value and makes it available as a variable inside that branch. A bare binding is a catch-all, exactly like `_`, except the value stays reachable by name.

### Match Guards

A pattern can carry an `if` condition. The arm only matches when the guard
holds, and later arms are tried otherwise:

```rust
fn describe(status: int) -> str:
    match status:
        200:
            return "OK"
        code if code >= 500:
            return "server error"
        code:
            return "other"

fn main():
    print(describe(200)) // OK
    print(describe(503)) // server error
```

A guarded arm never counts toward exhaustiveness on its own — the guard might
not hold at runtime, so the compiler still requires an unguarded arm (or
another guarded one) able to cover the same value.

The compiler enforces exhaustive pattern matching. Failing to match a variant triggers a compile-time error.

## Pattern Forms

Beyond a bare variant name, a pattern can destructure tuples, structs, and
lists, match numeric ranges, and combine alternatives with `|`. Every form
below nests inside a variant's payload positions too, not just at the
top level of a `match`.

### Tuple Patterns

```rust
fn describe_point(p: (int, int)) -> str:
    match p:
        (0, 0):
            return "origin"
        (0, y):
            return f"on the y-axis at {y}"
        (x, 0):
            return f"on the x-axis at {x}"
        (x, y):
            return f"at {x},{y}"

fn main():
    print(describe_point((0, 0))) // origin
    print(describe_point((3, 4))) // at 3,4
```

### Struct Field Patterns

Match a struct by naming the fields you care about; unnamed fields are
ignored:

```rust
struct Point:
    x: int
    y: int

fn describe(p: Point) -> str:
    match p:
        Point(x=0, y=0):
            return "origin"
        Point(x=0, y=y):
            return f"on the y-axis at {y}"
        Point(x=x, y=y):
            return f"at {x},{y}"

fn main():
    print(describe(Point(x=0, y=5))) // on the y-axis at 5
```

### List Patterns

Match a list by its length, or peel off a fixed prefix/suffix and bind the
remainder with `*name`:

```rust
fn summarize(xs: [int]) -> str:
    match xs:
        []:
            return "empty"
        [x]:
            return f"one: {x}"
        [first, *rest]:
            return f"first {first} rest {rest}"

fn main():
    print(summarize([])) // empty
    print(summarize([5])) // one: 5
    print(summarize([1, 2, 3, 4])) // first 1 rest [2, 3, 4]
```

### Range Patterns

`a..b` matches up to (not including) `b`; `a..=b` includes it:

```rust
fn bucket(n: int) -> str:
    match n:
        0..10:
            return "small"
        10..=20:
            return "medium"
        n:
            return f"other: {n}"

fn main():
    print(bucket(5)) // small
    print(bucket(20)) // medium
```

### Or-Patterns

`|` matches any of several alternatives in one arm. Every alternative must
bind the same names:

```rust
enum Shape:
    Circle(float)
    Square(float)

fn area(s: Shape) -> float:
    match s:
        Circle(r) | Square(r):
            return r * r

fn main():
    print(area(Circle(3.0))) // 9.0
    print(area(Square(3.0))) // 9.0
```

### Nested Patterns

Every pattern form recurses, so a variant's payload can itself be a tuple,
a list, or a range, and or-patterns can combine whole variants:

```rust
enum Node:
    Leaf(int)
    Pair(int, int)

fn classify(n: Node) -> str:
    match n:
        Leaf(0):
            return "leaf-zero"
        Leaf(1..10):
            return "leaf-small"
        Pair(0, 0) | Pair(0, _) | Pair(_, 0):
            return "pair-has-zero"
        Pair(a, b) if a == b:
            return "pair-equal"
        _:
            return "other"

fn main():
    print(classify(Leaf(0))) // leaf-zero
    print(classify(Pair(0, 7))) // pair-has-zero
    print(classify(Pair(4, 4))) // pair-equal
```

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

## Methods on Enums

`impl` blocks work on enums the same way they do on structs, including a
`self` receiver that a `match` can destructure:

```rust
enum Shape:
    Circle(float)
    Square(float)
    Rectangle(float, float)

impl Shape:
    fn area(self) -> float:
        match self:
            Circle(r):
                return 3.14159 * r * r
            Square(s):
                return s * s
            Rectangle(w, h):
                return w * h

fn main():
    print(Circle(2.0).area()) // 12.56636
    print(Rectangle(4.0, 5.0).area()) // 20.0
```
