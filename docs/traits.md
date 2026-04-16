# Traits

A trait defines shared behavior that types can implement. It functions as a compile-time contract: any type implementing a trait must provide implementations for its declared methods. This allows writing generic functions constrained by specific trait bounds.

## Defining a Trait

A trait definition lists the method signatures.

```rust
trait Drawable:
    fn draw(self)
    fn area(self) -> float
```

The `self` parameter refers to the implementing type.

## Implementing a Trait

Use `impl TraitName for TypeName` to implement a trait.

```rust
struct Circle:
    radius: float

impl Drawable for Circle:
    fn draw(self):
        print(f"Drawing a circle with radius {self.radius}")

    fn area(self) -> float:
        return 3.14 * self.radius * self.radius
```

## Generic Traits

Traits can be generic.

```rust
trait Converter[T, U]:
    fn convert(self, input: T) -> U

struct IntToString:
    pass

impl Converter[int, str] for IntToString:
    fn convert(self, input: int) -> str:
        return str(input)
```

## Default Method Implementations

Traits can provide default method implementations. Types use the default if they do not override it.

```rust
trait Logger:
    fn log(self, msg: str):
        print(f"[LOG]: {msg}")

struct SimpleApp:
    pass

impl Logger for SimpleApp:
    pass
```

## Dynamic Dispatch (Trait Objects)

Functions can accept trait objects for dynamic dispatch. This allows passing any type that implements the trait, resolving the method call at runtime via a vtable.

```rust
fn render_all(items: [Drawable]):
    for item in items:
        item.draw()
```

Any struct that implements `Drawable` can be passed into this function.
