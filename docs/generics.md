# Generics

Generics let one definition work across many types while keeping full compile-time type checking. The compiler monomorphizes each generic definition, emitting a specialized version for every concrete type it is used with, so there is no runtime cost.

## Type Parameters

Type parameters stand in for types. They are written in square brackets after the name, usually as single capitals like `T`, `U`, or `V`.

### Generic Functions

```rust
fn swap[T](a: T, b: T) -> (T, T):
    return (b, a)

let x, y = swap(10, 20)        // T is int
let s1, s2 = swap("a", "b")    // T is str
```

Because of monomorphization, the `int` call and the `str` call compile to separate, fully typed functions.

### Generic Structs

A struct can take type parameters too, which makes it a container for any type:

```rust
struct Holder[T]:
    value: T

impl Holder[T]:
    fn get(self) -> T:
        return self.value

let int_holder = Holder(99)     // T is int
let str_holder = Holder("hi")   // T is str
```

## Type Inference

You rarely name the type parameter at a call. The compiler reads it from the arguments:

```rust
fn first[T](items: [T]) -> T:
    return items[0]

let item = first([1, 2, 3])    // T is int, inferred from the list
```

You can name the type explicitly when you want to, though it is rarely needed:

```rust
let item = first[int]([1, 2, 3])
```

## Trait Bounds

Constrain a type parameter with `: Trait` so the parameter is expected to provide that trait's methods:

```rust
trait Comparable:
    fn rank(self) -> int:
        return 0

fn larger[T: Comparable](a: T, b: T) -> T:
    if a.rank() > b.rank():
        return a
    return b
```

Any type implementing `Comparable` can be passed to `larger`.
