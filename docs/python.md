# Python Interoperability

Olive features native, bidirectional, and highly performant Python integration. Unlike rigid foreign function interfaces, Olive allows you to import any Python module directly, invoke its functions, pass native Olive collections with zero copy, and retrieve results dynamically.

## Importing Python Modules

To import a Python module, use the `import py` syntax:

```python
import py "math" as py_math
import py "numpy" as np
```

The imported modules are bound as variables of type `PyObject`. All attribute lookups, indexing operations, and function calls on `PyObject` are resolved dynamically at runtime by communicating with Python's C API.

## Calling Python Functions

You can invoke functions and methods on Python objects directly, passing Olive primitives or collections as arguments:

```python
import py "math" as py_math

fn calculate_hypotenuse(a: float, b: float) -> float:
    # Arguments are automatically converted to Python objects
    let result_py = py_math.hypot(a, b)
    
    # Cast the dynamic PyObject back to an Olive float
    return float(result_py)
```

## Bidirectional Zero-Copy Proxies

When you pass native Olive collections (lists or dictionaries) to Python, Olive doesn't serialize or duplicate the data. Instead, it wraps the Olive collections in custom C-level Python types (`OliveListProxy` and `OliveDictProxy`).

* **Zero Memory Overhead**: Python reads and writes directly to the underlying Olive collection structure in memory.
* **Mutations Propagate**: Any changes made by the Python code are immediately visible in Olive (and vice-versa).

```python
import py "json" as json

fn format_config():
    let mut config = {"host": "localhost", "port": 8080}
    
    # config is passed as a zero-copy proxy
    let formatted = json.dumps(config, indent=4)
    print(str(formatted))
```

## Type Conversions

Primitive types and built-in structures are seamlessly converted between Olive and Python:

| Olive Type | Python Type | Conversion Direction | Notes |
| :--- | :--- | :--- | :--- |
| `int` | `int` | Bidirectional | Coerces to `c_long` |
| `float` | `float` | Bidirectional | Coerces to `c_double` |
| `str` | `str` | Bidirectional | Coerces to UTF-8 |
| `list` | `olive_proxies.OliveListProxy` | Olive -> Python | Zero-copy wrapper |
| `dict` | `olive_proxies.OliveDictProxy` | Olive -> Python | Zero-copy wrapper |
| `None` | `None` | Bidirectional | Coerces to `Py_None` |

### Explicit Coercions

To extract typed data from a dynamic `PyObject` back into Olive, use the built-in type constructors:

```python
let val_int = int(py_val)       # Coerces PyObject to Olive int
let val_float = float(py_val)   # Coerces PyObject to Olive float
let val_str = str(py_val)       # Coerces PyObject to Olive string
let val_list = list(py_val)     # Coerces PyObject to Olive list
let val_dict = dict(py_val)     # Coerces PyObject to Olive dict
```

## Runtime Environment

Under the hood, Olive dynamically locates and links `libpython` on your system. If you need to specify a particular Python installation or environment, set the `PYTHON_LIBRARY` environment variable:

```bash
export PYTHON_LIBRARY="/usr/lib/libpython3.11.so"
./your_olive_binary
```
