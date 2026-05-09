# Classes and Object-Oriented Programming

Olive provides a powerful class-based object-oriented system that combines Python's flexibility with static type safety.

## Defining a Class

Classes are defined using the `class` keyword. They can contain methods and a special `__init__` method for initialization.

```python
class Person:
    fn __init__(self, name: str, age: int):
        self.name = name
        self.age = age

    fn greet(self):
        print("Hi, I'm " + self.name)
```

## Creating Instances

To create an instance of a class, call the class name as if it were a function:

```python
let p = Person("Alice", 30)
p.greet()
```

## Inheritance

Olive supports single and multiple inheritance. Base classes are listed in parentheses after the class name.

```python
class Student(Person):
    fn __init__(self, name: str, age: int, student_id: int):
        Person.__init__(self, name, age)
        self.student_id = student_id

    fn study(self):
        print(self.name + " is studying")
```

## Attribute Access

Attributes (fields) are accessed using the dot (`.`) operator. Olive's type checker tracks the types of fields assigned in `__init__` or other methods to ensure type safety.

```python
let s = Student("Bob", 20, 12345)
print(s.name)  # Accessing inherited attribute
s.study()          # Calling a method
```

## Visibility

By convention, attributes or methods starting with an underscore (`_`) are considered private to the module and should not be accessed from outside. The compiler enforces these visibility rules during the resolution phase.

```python
class Secret:
    fn __init__(self):
        self._data = "Top Secret"

    fn _internal_method(self):
        pass
```
