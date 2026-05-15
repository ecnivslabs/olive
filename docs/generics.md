# Generics

Generics allow the creation of code that is flexible and reusable across different types without sacrificing type safety or performance. Instead of writing separate functions for `int`, `str`, and custom structs, a single generic function can be used instead.

## Type Parameters

Type parameters are placeholders for types, usually represented by single capital letters like `T`, `U`, or `V`. They are defined in square brackets `[]`.

### Generic Functions

```python
fn swap[T](a: T, b: T) -> (T, T):
    return (b, a)

let (x, y) = swap(10, 20)        # T is int
let (s1, s2) = swap("a", "b")    # T is str
```

The Olive compiler uses **monomorphization**. This means it generates a specialized version of the function for every type you use it with, so there is zero runtime overhead for using generics.

### Generic Structs

Structs can also be generic, allowing them to act as containers for any data type.

```python
struct Result[T, E]:
    value: T | None
    error: E | None

impl Result[T, E]:
    fn is_ok(self) -> bool:
        return self.error == None
```

## Trait Bounds (Constraints)

Sometimes you need a generic type to support certain operations. For example, if you want to compare two values, they must implement a trait that defines comparison.

```python
trait Comparable:
    fn compare(self, other: self) -> int

fn max[T: Comparable](a: T, b: T) -> T:
    if a.compare(b) > 0:
        return a
    return b
```

The `: Comparable` syntax is a **trait bound**, restricting `T` to types that implement the `Comparable` trait.

## Generic Traits

Traits themselves can be generic, allowing them to define behavior that relates multiple types.

```python
trait Converter[T, U]:
    fn convert(self, input: T) -> U

struct IntToStringConverter:
    pass

impl Converter[int, str] for IntToStringConverter:
    fn convert(self, input: int) -> str:
        return str(input)
```

## Type Inference

In most cases, specifying types when calling a generic function is unnecessary. The compiler looks at the arguments passed and "fills in" the type parameters.

```python
let list = [1, 2, 3]
let item = first(list) # The compiler knows T is int because list is [int]
```

If the compiler cannot determine the types, or if a more explicit definition is desired, types can be provided manually:

```python
let item = first[int](list)
```
