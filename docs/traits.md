# Traits

A trait is a set of methods that several types can share. A type takes on a trait by implementing it, and code can then treat any implementing type through that trait.

## Defining a Trait

A trait lists its methods, each with a body. That body is the default: a type that implements the trait gets it for free unless it provides its own.

```rust
trait Drawable:
    fn draw(self):
        print("a shape")
```

The `self` parameter is the implementing value.

## Implementing a Trait

Write `impl Trait for Type` and override the methods you want to change:

```rust
struct Circle:
    radius: float

impl Drawable for Circle:
    fn draw(self):
        print(f"circle r={self.radius}")
```

## Inheriting the Default

A type that is happy with the defaults implements the trait with `pass` and inherits every method as written:

```rust
struct Blank:
    pass

impl Drawable for Blank:
    pass

Blank().draw()    // prints "a shape"
```

You can also override some methods and inherit the rest: only the methods you write replace their defaults.

## Dynamic Dispatch (Trait Objects)

A function can take a collection typed by the trait. Each element keeps its own concrete type, and the right method runs for each one, resolved at runtime:

```rust
fn render_all(items: [Drawable]):
    for item in items:
        item.draw()

fn main():
    render_all([Circle(1.5), Blank(), Circle(9.0)])
```

This prints the circle output for the circles and the default for the blank. Any type that implements `Drawable` can go in the list.
