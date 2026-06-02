# Structs and Objects

Structs are the compound data types in Olive. They group related fields and carry methods that operate on those fields.

## Defining a Struct

A struct lists its fields, each with an explicit type. A field may give a default value, which lets it be omitted when constructing the struct. Fields with defaults must come last.

```rust
struct User:
    username: str
    email: str
    is_active: bool = True
```

## Constructing a Struct

Without a custom initializer, a struct is built by passing its fields in order. Trailing fields that have a default may be left out:

```rust
let u = User("vince", "v@example.com")          // is_active defaults to True
let banned = User("mallory", "m@example.com", False)
```

## Adding Behavior with `impl`

Methods live in an `impl` block. A method that works on an instance takes `self` as its first parameter:

```rust
impl User:
    fn deactivate(self):
        self.is_active = False

    fn describe(self) -> str:
        return f"{self.username} active={self.is_active}"
```

## Custom Initialization (`__init__`)

Define `__init__` when construction needs validation or derived fields. Olive calls it when the struct is built:

```rust
struct Rectangle:
    width: float
    height: float
    area: float

impl Rectangle:
    fn __init__(self, w: float, h: float):
        assert w > 0.0 and h > 0.0, "dimensions must be positive"
        self.width = w
        self.height = h
        self.area = w * h

let r = Rectangle(10.0, 5.0)
```

With an `__init__`, the constructor takes the parameters that `__init__` declares rather than the raw fields.

## Generic Structs

A struct can take type parameters in `[...]`, so it can hold any type:

```rust
struct Box[T]:
    content: T

impl Box[T]:
    fn get(self) -> T:
        return self.content

let int_box = Box(42)      // T is int
let str_box = Box("item")  // T is str
```

## Composition

Olive composes structs rather than inheriting between them. A struct holds other structs to reuse their data and behavior:

```rust
struct Admin:
    user: User
    permissions: [str]

impl Admin:
    fn can_access(self, resource: str) -> bool:
        return resource in self.permissions
```

## Visibility and Privacy

A field or method whose name starts with an underscore is private. It is reachable only from within the module that defines the struct:

```rust
struct Account:
    _balance: float

impl Account:
    fn balance(self) -> float:
        return self._balance
```

## Operator and Formatting Protocol

A struct opts into operators and formatting by defining specific dunder
methods in its `impl` block. The supported set is exact; any other
`__`-prefixed method (besides `__init__`) is a compile error.

**Arithmetic** — `__add__`, `__sub__`, `__mul__`, `__truediv__`, `__mod__`.
Both operands must be the struct's own type; there is no reflected form
(no `__radd__`). The method consumes both operands, the same as Rust's
`Add`:

```rust
struct Vec2:
    x: float
    y: float

impl Vec2:
    fn __add__(self, other: Vec2) -> Vec2:
        return Vec2(self.x + other.x, self.y + other.y)

let v = Vec2(1.0, 2.0) + Vec2(3.0, 4.0)  // Vec2(4.0, 6.0)
```

**Equality and ordering** — `__eq__` and `__lt__`. Unlike arithmetic, these
take `self`/`other` **by reference** (`self: &Vec2, other: &Vec2`): the
compiler may call them many times on the same values (a `sort`, a
container lookup), and a by-value `self` would be freed after the first
call. `!=`, `>`, `<=`, `>=` all derive from these two — defining one of
those four directly is a compile error, matching name it derives from:

```rust
impl Vec2:
    fn __eq__(self: &Vec2, other: &Vec2) -> bool:
        return self.x == other.x and self.y == other.y

    fn __lt__(self: &Vec2, other: &Vec2) -> bool:
        let a = self.x * self.x + self.y * self.y
        let b = other.x * other.x + other.y * other.y
        return a < b
```

`__eq__` overrides the derived structural `==` for that type outright,
even where every field would already support it. `sort`/`sorted` with no
`key=` argument on a list of structs use `__lt__` for ordering; sorting a
struct list without one is a compile error naming the missing method.

**String conversion** — `__str__`, `fn(self) -> str`. Wires into `print`,
`str()`, and f-string interpolation; a struct without one falls back to an
automatic field-by-field representation:

```rust
impl Vec2:
    fn __str__(self) -> str:
        return f"({self.x}, {self.y})"

print(Vec2(1.0, 2.0))       // (1.0, 2.0)
```

## Implementing Traits

A struct can implement a trait to gain a shared set of methods. See [Traits](traits.md) for the full picture:

```rust
trait Describable:
    fn describe(self) -> str:
        return "an object"

impl Describable for User:
    fn describe(self) -> str:
        return f"User({self.username})"
```
