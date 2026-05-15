# Traits

A **trait** is a set of rules that a type can choose to follow. Think of it as a contract: if a type implements a trait, it's promising to provide certain functions. This lets you write generic code that can work with any type, as long as it follows the rules.

## Defining a Trait

A trait definition lists the names and types of the methods that a type must provide.

```python
trait Drawable:
    fn draw(self)
    fn area(self) -> float
```

The `self` keyword refers to whatever type is implementing the trait.

## Implementing a Trait

Use `impl TraitName for TypeName` to fulfill the contract.

```python
struct Circle:
    radius: float

impl Drawable for Circle:
    fn draw(self):
        print(f"Drawing a circle with radius {self.radius}")

    fn area(self) -> float:
        return 3.14 * self.radius * self.radius
```

If you miss a method required by the trait, the Olive compiler will tell you exactly what's missing and where.

## Generic Traits

Traits can also be generic, which allows them to define behavior that relates multiple types together.

```python
trait Converter[T, U]:
    fn convert(self, input: T) -> U

struct IntToString:
    pass

impl Converter[int, str] for IntToString:
    fn convert(self, input: int) -> str:
        return str(input)
```

## Default Method Implementations

Traits can provide a "default" way of doing something. If a type doesn't provide its own version, it will use the default.

```python
trait Logger:
    fn log(self, msg: str):
        print(f"[LOG]: {msg}")

struct SimpleApp:
    pass

impl Logger for SimpleApp:
    # The log() method does not need to be defined if the default is sufficient
    pass
```

## Shared Behavior

By using traits, you can write functions that accept any type that implements a specific trait. This is the key to writing flexible and reusable Olive code.

```python
fn render_all(items: [Drawable]):
    for item in items:
        item.draw()
```

Any struct that implements `Drawable` can be passed into this function, regardless of what other data it holds.
