# Traits

A `trait` is a way to define a set of rules that a type must follow. Think of it as a contract: if a type says it implements a trait, it's promising to provide certain functions. This lets you write code that can work with many different types, as long as they follow the same rules.

## Defining a Trait

A trait body contains method signatures. No implementations go here; just names and types:

```python
trait Drawable:
    fn draw(self) -> None
    fn bounding_box(self) -> (int, int, int, int)
```

The `pass` keyword works too, for a trait with no required methods:

```python
trait Marker:
    pass
```

## Implementing a Trait

Use `impl TraitName for TypeName` to implement a trait on a struct:

```python
struct Circle:
    x: int
    y: int
    radius: int

impl Drawable for Circle:
    fn draw(self) -> None:
        print(f"Circle at {self.x}, {self.y} with radius {self.radius}")

    fn bounding_box(self) -> (int, int, int, int):
        return (self.x - self.radius, self.y - self.radius,
                self.x + self.radius, self.y + self.radius)
```

If you forget a method, the compiler tells you which one is missing:

```
`Rectangle` does not implement `Rectangle::draw` required by trait `Drawable`
```

## Multiple Types, One Trait

Any number of types can implement the same trait. Each provides its own behavior:

```python
struct Rectangle:
    x: int
    y: int
    width: int
    height: int

impl Drawable for Rectangle:
    fn draw(self) -> None:
        print(f"Rect at {self.x}, {self.y}, size {self.width}x{self.height}")

    fn bounding_box(self) -> (int, int, int, int):
        return (self.x, self.y, self.x + self.width, self.y + self.height)
```

## Non-Trait `impl` Blocks

Regular `impl` blocks (without `for`) still work the same way. Traits are opt-in:

```python
impl Circle:
    fn area(self) -> float:
        return 3.14159 * float(self.radius * self.radius)
```

A type can have both a regular `impl` block and one or more trait `impl` blocks.
