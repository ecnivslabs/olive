# Basic Syntax and Types

Olive's syntax is designed to be clean and easy to read. The language is "statically typed," which means the compiler helps you keep track of what kind of data your variables are holding.

## Variables and Mutability

You declare variables with `let`. By default, once you give a variable a value, you can't change it (it's "immutable").

```python
let name = "Olive"
# name = "New Name"  <-- This would be an error
```

If you know a variable's value will need to change, use `let mut`:

```python
let mut count = 0
count = 1  # This is fine!
```

### Constants

For values that never change and are known when you write the code, use `const`:

```python
const PI = 3.14159
```

## Data Types

Olive can usually figure out the type of a variable on its own, but you can also be explicit if you want.

### Simple Types

- `int`: Whole numbers (`42`, `-7`).
- `float`: Numbers with decimals (`3.14`).
- `str`: Text (`"Hello"`).
- `bool`: True or False (`True`, `False`).
- `None`: Represents "no value."

### F-Strings

You can easily build strings with variables inside them by putting an `f` before the quotes:

```python
let name = "Olive"
print(f"Hello, {name}!")
```

### Collections

- **Lists**: Ordered groups of items. `[1, 2, 3]`
- **Sets**: Groups of unique items. `{1, 2, 3}`
- **Dictionaries**: Pairs of keys and values. `{"name": "Olive", "age": 1}`
- **Tuples**: Fixed-size groups of different items. `(1, "Olive")`

## Control Flow

### If Statements

```python
if score >= 90:
    print("A")
elif score >= 80:
    print("B")
else:
    print("C")
```

### Loops

#### For Loops

```python
for item in ["apple", "banana", "cherry"]:
    print(item)
```

#### While Loops

```python
let mut i = 0
while i < 5:
    print(i)
    i += 1
```

## Comprehensions

Comprehensions are a quick way to create new collections from existing ones:

```python
let numbers = [1, 2, 3, 4]
let squares = [x * x for x in numbers]  # [1, 4, 9, 16]
```

## Useful Built-ins

Olive comes with several functions you can use anywhere:

- `print(value)`: Shows a value in the console.
- `len(collection)`: Returns the number of items in a list, string, etc.
- `range(stop)` or `range(start, stop)`: Generates a sequence of numbers.
- `str(value)`, `int(value)`, `float(value)`: Converts a value to a different type.
- `type(value)`: Returns a string describing the type of the value.
- `assert(condition, message)`: Stops the program if the condition is false.

