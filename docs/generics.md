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

Explicit type arguments at a call site (`first[int](...)`) are not
supported yet; the type parameter is always inferred from the arguments, as
above.

## Structural Requirements

You can annotate a type parameter with `: Trait` to document that the parameter must provide the trait's methods:

```rust
trait Comparable:
    fn rank(self) -> int:
        return 0

fn larger[T: Comparable](a: T, b: T) -> T:
    if a.rank() > b.rank():
        return a
    return b
```

The bound is structural: if `T` has the methods the body calls, it works. The compiler validates method resolution on the concrete type at instantiation time, not the trait bound itself. A type that happens to have `.rank()` works even without explicitly implementing `Comparable`, though implementing the trait is preferred for clarity.
