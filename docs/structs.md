# Structs and Composition

Olive uses `struct` and `impl` blocks to define data types and their behavior. There's no class hierarchy; composition replaces inheritance.

## Defining a Struct

Structs describe the data layout. Fields are listed with their types:

```python
struct Person:
    name: str
    age: int
```

## Adding Behavior with `impl`

Methods go in a separate `impl` block. Methods that need to access the struct's data take `self` as the first parameter:

```python
impl Person:
    fn greet(self):
        print("Hi, I'm " + self.name)
```

## Creating Instances

Call the struct name like a function, passing field values in order:

```python
let p = Person("Alice", 30)
p.greet()
```

For custom initialization logic, define an `__init__` method in the `impl` block:

```python
struct Rectangle:
    width: int
    height: int
    area: int

impl Rectangle:
    fn __init__(self, w: int, h: int):
        self.width = w
        self.height = h
        self.area = w * h

let rect = Rectangle(10, 20)
print(f"Area: {rect.area}")
```

## Composition over Inheritance

Olive doesn't support inheritance. If you want one type to include the behavior of another, embed it as a field:

```python
struct Student:
    person: Person
    student_id: int

impl Student:
    fn study(self):
        print(self.person.name + " is studying")
```

## Attribute Access

Fields and nested fields are accessed with the dot operator:

```python
let s = Student(Person("Bob", 20), 12345)
print(s.person.name)  # Accessing nested field
s.study()             # Calling a method
```

The type checker tracks field types throughout and will catch type mismatches before the code runs.

## Visibility

By convention, names starting with an underscore are private to the module. The compiler enforces this: you can't import or access a `_`-prefixed member from outside its defining module:

```python
struct Secret:
    _data: str

impl Secret:
    fn _internal_method(self):
        pass
```

## Implementing Traits

A struct can implement a trait using `impl TraitName for TypeName`. This guarantees the struct provides all the methods the trait requires:

```python
trait Printable:
    fn display(self) -> str

impl Printable for Person:
    fn display(self) -> str:
        return f"{self.name}, age {self.age}"
```

A struct can implement multiple traits by having multiple `impl ... for` blocks. Regular `impl` blocks (without `for`) can coexist alongside them.

See [Traits](traits.md) for a full walkthrough.

## Method Decorators

Methods can use the same decorators as standalone functions. The `@memo` decorator is useful on methods that do expensive computation with the same inputs:

```python
struct Calculator:
    pass

impl Calculator:
    @memo
    fn expensive_computation(self, n: int) -> int:
        if n <= 1:
            return n
        return self.expensive_computation(n - 1) + self.expensive_computation(n - 2)
```
