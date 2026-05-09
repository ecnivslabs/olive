# Modules and Project Structure

Olive features a simple yet powerful module system that allows you to organize your code into reusable components.

## Importing Modules

You can import other Olive files using the `import` statement. Olive maps dots in the module name to directory separators.

```python
import math
import utilities.network

let x = math.sqrt(16)
```

By default, an `import math` statement will look for a file named `math.liv` in the same directory as the current file.

## From-Imports

If you only need specific members from a module, use the `from ... import` syntax:

```python
from math import sqrt, pi

print(sqrt(pi))
```

## Visibility and Privacy

Olive uses a naming convention to enforce visibility:

- **Public**: Any name that does **not** start with an underscore is public and can be accessed from other modules.
- **Private**: Any name starting with an underscore (e.g., `_internal_helper`) is private to the module where it is defined.

The compiler will raise an error if you attempt to import or access a private member from another module.

```python
# In utils.liv
fn _secret():
    pass

# In main.liv
import utils
# utils._secret()  # Error: cannot access private member `_secret`
```

## Project Organization

A typical Olive project might look like this:

```text
my_project/
├── main.liv
├── models.liv
└── utils/
    ├── __init__.liv (optional)
    └── network.liv
```

In `main.liv`, you would import these as:
```python
import models
import utils.network
```
