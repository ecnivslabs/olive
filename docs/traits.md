# Traits

A **trait** defines shared interface behavior that types can implement. It functions as a compile-time contract: any type implementing a trait must provide implementations for its declared methods. This allows writing generic functions constrained by specific trait bounds.

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

## Polymorphism and Shared Behavior

Traits allow functions to accept any type that satisfies a specific interface, enabling polymorphic dispatch:

```python
fn render_all(items: [Drawable]):
    for item in items:
        item.draw()
```

Any struct that implements `Drawable` can be passed into this function, regardless of what other data it holds.
