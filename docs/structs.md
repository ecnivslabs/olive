# Structs and Composition

Olive utilizes a static, composition-based data model using `struct` and `impl` blocks, similar to Rust, instead of traditional class-based inheritance.

## Defining a Struct

Structs are defined using the `struct` keyword and describe the data layout.

```python
struct Person:
    name: str
    age: int
```

## Adding Behavior with `impl`

To add methods to a struct, use an `impl` block. The first parameter of a method should be `self` if it needs to access the struct's data.

```python
impl Person:
    fn greet(self):
        print("Hi, I'm " + self.name)
```

## Creating Instances

To create an instance of a struct, call the struct name as if it were a function, passing the required fields in order:

```python
let p = Person("Alice", 30)
p.greet()
```

If a custom constructor is needed, you can define an `__init__` method in the `impl` block. The default constructor behavior takes the fields defined in the struct, but `__init__` allows you to perform custom initialization logic.

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

Olive does not support inheritance. Instead, it encourages composition by embedding structs within each other.

```python
struct Student:
    person: Person
    student_id: int

impl Student:
    fn study(self):
        print(self.person.name + " is studying")
```

## Attribute Access

Attributes (fields) are accessed using the dot (`.`) operator. Olive's type checker tracks the types of fields to ensure type safety.

```python
let s = Student(Person("Bob", 20), 12345)
print(s.person.name)  # Accessing nested attribute
s.study()             # Calling a method
```

## Visibility

By convention, attributes or methods starting with an underscore (`_`) are considered private to the module and should not be accessed from outside. The compiler enforces these visibility rules during the resolution phase.

```python
struct Secret:
    _data: str

impl Secret:
    fn _internal_method(self):
        pass
```

## Method Decorators

Just like standalone functions, struct methods can also be enhanced using decorators. This is particularly useful for things like memoizing expensive method calls.

```python
struct Calculator:
    pass

impl Calculator:
    @memo
    fn expensive_computation(self, n: int) -> int:
        # Some heavy calculation...
        if n <= 1:
            return n
        return self.expensive_computation(n - 1) + self.expensive_computation(n - 2)
```
