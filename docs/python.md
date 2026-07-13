# Python Interoperability

Olive features native, bidirectional Python integration. You can import any Python module directly, call its functions with native Olive values, and get back typed results with full compile-time type information derived automatically from Python type stubs.

## Importing Python Modules

```olive
import py "glm" as glm
import py "math" as math
import py "numpy" as np
```

That single line is enough. If the module ships `.pyi` type stubs, Olive reads them at compile time and registers all exported types, functions, and class members automatically. No manual stub blocks needed.

## Typed Python Objects

When Olive can determine the type of a Python value from stubs, it tracks it as a qualified type (`glm.vec3`, `glm.mat4`, etc.) rather than collapsing everything to `PyObject`. This means the compiler catches type mismatches and resolves return types without any annotations from you.

```olive
import py "glm" as glm

let v = glm.vec3(1.0, 0.0, 0.0)   // type: glm.vec3
let n = glm.normalize(v)            // type: glm.vec3
let x = v.x                         // type: float
let w = v + n                        // type: glm.vec3
let proj = glm.perspective(fov, ar, near, far)  // type: glm.mat4
```

The type checker resolves:

- **Constructors**: `glm.vec3(...)` returns `glm.vec3`
- **Module-level functions**: return types from stubs, with TypeVar constraints expanded into concrete per-type overloads
- **Arithmetic operators**: `+`, `-`, `*`, `/` look up the appropriate dunder method on the left operand's class
- **Field access**: `v.x` returns `float` when the class stub annotates `x: float`
- **Method calls**: return types resolved from class method stubs
- **Type mismatches**: assigning a `glm.mat4` where a `glm.vec3` is expected is a compile error

## PyObject

`PyObject` represents any Python value whose type is unknown or unknowable at compile time. It behaves like `Any`: operations on it are always permitted and always return `PyObject`. Use it when working with highly dynamic Python APIs where stub-based inference isn't possible.

```olive
import py "json" as json

let raw: PyObject = json.loads(data)   # dynamic, type unknown
let val = raw["key"]                   # PyObject, resolved at runtime
```

`PyObject` and qualified types like `glm.vec3` unify freely. You can always widen a typed Python value to `PyObject` when interoperability requires it.

## Type Coercions

Python values can be coerced into Olive primitives using the standard built-in constructors:

```olive
let n = float(py_val)   # PyObject -> Olive float
let i = int(py_val)     # PyObject -> Olive int
let s = str(py_val)     # PyObject -> Olive string
```

Going the other direction, Olive primitives are automatically converted when passed to Python functions.

## Type Conversion Reference

| Olive Type | Python Type | Notes |
| :--- | :--- | :--- |
| `int` / `i64` | `int` | Coerces via `c_long` |
| `float` / `f64` | `float` | Coerces via `c_double` |
| `str` | `str` | UTF-8 |
| `list` | `OliveListProxy` | Zero-copy wrapper, mutations propagate both ways |
| `dict` | `OliveDictProxy` | Zero-copy wrapper, mutations propagate both ways; supports `get`, `setdefault`, `pop`, `keys`, `values`, `items`, `in` |
| `None` | `None` | `Py_None` |
| `glm.vec3` etc. | native Python object | Tracked type; erased to `PyObject` at runtime boundary |

When an Olive value is assigned to a native-typed slot (`i64`, `f64`, `str`, struct field, collection element), the compiler inserts the correct runtime unboxer automatically. No manual coercion call needed. The reverse (native to PyObject) is also automatic when passing native values to Python functions.

## Manual Stub Blocks

For modules without `.pyi` stubs, or when you want explicit control over which types are exposed, you can declare types and functions inline:

```olive
import py "mymodule" as mm:
    type Foo
    type Bar
    fn create(x: float) -> Foo
    fn process(f: Foo) -> Bar
```

Manual stub blocks take priority over auto-introspection. `PyObject` in a stub declaration is the explicit escape hatch for return types you don't want to track.

## Deep Conversion with `realize`

Olive collections (lists, dicts) passed to Python are wrapped in `OliveListProxy` / `OliveDictProxy` by default. These are zero-copy wrappers: mutations in either language propagate to the other. Some Python libraries call `isinstance(x, dict)` and reject proxies. Use `realize(val)` to deep-convert an Olive value into a real Python object:

```olive
import py "json" as json

let data = {"key": [1, 2, 3]}
let real = realize(data)      # deep-converts to a real Python dict
json.dumps(real)               # works because isinstance(real, dict) is True
```

`realize` is the inverse of the automatic coercion path. Native Olive values (int, float, str, None) pass through unchanged. Collections are recursively converted to real Python objects on the Python heap.

## Runtime Library Discovery

Olive locates the active Python shared library at startup using a four-tier fallback:

1. **`OLIVE_PYTHON_PATH`** or **`PYTHON_LIBRARY`** environment variables: checked first; set either to an absolute path to force a specific installation.
2. **Subprocess query**: Olive spawns `python3` and reads `sysconfig` to find the exact library for the active environment (`venv`, `pyenv`, `conda`, system).
3. **Dynamic directory scan**: Olive scans standard library directories (`/usr/lib`, `/usr/local/lib`, `/opt/homebrew/lib`, etc.) for any `libpython3.X.so` or `libpython3.X.dylib`, picking the highest version. This handles any Python 3.x version without hardcoding.
4. **Final fallback**: Bare name search via the dynamic linker for common DLL names on Windows (`python3.dll`, `python312.dll`, etc.).

```bash
export OLIVE_PYTHON_PATH="/usr/lib/libpython3.14.so"
```
