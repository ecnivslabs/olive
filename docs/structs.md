# Structs and Objects

Structs are the primary way to define custom data structures in Olive. They enable the grouping of related data and the definition of behavior that operates on that data.

## Defining a Struct

A struct defines the layout of your data. Fields must have explicit types.

```python
struct User:
    username: str
    email: str
    is_active: bool = True  # Default value
```

## Adding Behavior with `impl`

Methods are defined in an `impl` block for the struct. Methods that operate on an instance must take `self` as their first parameter.

```python
impl User:
    fn deactivate(self):
        self.is_active = False
```

## Initialization (`__init__`)

When an instance of a struct is created, Olive calls the `__init__` method if it's defined. This is where setup logic or validation is performed.

```python
struct Rectangle:
    width: float
    height: float
    area: float

impl Rectangle:
    fn __init__(self, w: float, h: float):
        assert w > 0 and h > 0, "Dimensions must be positive"
        self.width = w
        self.height = h
        self.area = w * h

let r = Rectangle(10.0, 5.0)
```

If no `__init__` is defined, Olive generates a default constructor that takes all fields in order.

## Generics (Type Parameters)

Structs can be generic, allowing any type of data to be stored.

```python
struct Box[T]:
    content: T

impl Box[T]:
    fn get(self) -> T:
        return self.content

let int_box = Box(42)      # T is int
let str_box = Box("item")  # T is str
```

## Composition

Olive encourages composition over inheritance. To reuse the data or behavior of another struct, it can be included as a field.

```python
struct Admin:
    user: User
    permissions: [str]

impl Admin:
    fn can_access(self, resource: str) -> bool:
        return resource in self.permissions
```

## Visibility and Privacy

Fields and methods starting with an underscore are **private**. They can only be accessed within the module where the struct is defined.

```python
struct Account:
    _balance: float

impl Account:
    fn get_balance(self) -> float:
        return self._balance  # OK: internal access
```

## Implementing Traits

You can implement traits for your structs to provide standardized behavior.

```python
trait Describable:
    fn describe(self) -> str

impl Describable for User:
    fn describe(self) -> str:
        return f"User({self.username}, active={self.is_active})"
```

See [Traits](traits.md) for more details.
