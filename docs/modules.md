# Modules and Standard Library

## Importing Modules

Use the `import` statement to bring in other Olive files. Dots in the module name map to directory separators:

```python
import math
import utilities.network
import physics.gravity as gravity

let x = math.sqrt(16)
let g = gravity.G
```

By default, `import math` looks for `math.liv` in the same directory as the current file.

## From-Imports

If you only need specific names from a module, use `from ... import`:

```python
from math import sqrt, pi
from data.processing import clean_string as clean, parse_json as parse

print(sqrt(pi))
let data = parse(clean(raw_input))
```

## Visibility and Privacy

Olive uses a naming convention for visibility:

- **Public**: Any name that doesn't start with an underscore. Accessible from other modules.
- **Private**: Names starting with `_` are private to the module where they're defined. The compiler enforces this.

```python
# In utils.liv
fn _secret():
    pass

# In main.liv
import utils
# utils._secret()  # Error: cannot access private member `_secret`
```

## Project Organization

A typical project layout:

```text
my_project/
├── main.liv
├── models.liv
└── utils/
    ├── __init__.liv (optional)
    └── network.liv
```

In `main.liv`:

```python
import models
import utils.network
```

## Standard Library

Olive ships with a standard library loaded dynamically at runtime. These modules are available without any additional setup.

### `math`

```python
import math
```

**Constants**

```python
math.PI    # 3.141592653589793
math.E     # 2.718281828459045
math.TAU   # 6.283185307179586
math.INF   # 1.0e308
```

**Trigonometry** (all angles in radians)

```python
math.sin(x)         math.asin(x)
math.cos(x)         math.acos(x)
math.tan(x)         math.atan(x)
                    math.atan2(y, x)
math.degrees(x)     # radians -> degrees
math.radians(x)     # degrees -> radians
```

**Exponential and logarithm**

```python
math.exp(x)         # e^x
math.log(x)         # natural log
math.log10(x)       # log base 10
math.pow(b, e)      # b^e (floats)
math.ipow(b, e)     # b^e (integers)
```

**Roots and rounding**

```python
math.sqrt(x)
math.cbrt(x)
math.hypot(x, y)    # sqrt(x² + y²)
math.floor(x)       # -> int
math.ceil(x)        # -> int
math.round(x)       # -> int
math.abs(x)
math.clamp(x, lo, hi)
math.fmod(x, y)
math.copysign(x, y)
```

**Hyperbolic**

```python
math.sinh(x)    math.asinh(x)
math.cosh(x)    math.acosh(x)
math.tanh(x)    math.atanh(x)
```

**Number theory**

```python
math.gcd(a, b)
math.lcm(a, b)
math.factorial(n)
math.comb(n, k)     # n choose k
math.perm(n, k)     # n permute k
```

**Utilities**

```python
math.min(a, b)
math.max(a, b)
math.isclose(a, b)  # abs(a - b) < 1e-9
math.erf(x)
```

### `io`

Synchronous file operations:

```python
import io

let contents = io.file_read("data.txt")
io.file_write("output.txt", "hello")
```

### `aio`

Asynchronous file operations that return futures. Must be used with `await`:

```python
import aio

async fn read_file():
    let contents = await aio.async_file_read("data.txt")
    await aio.async_file_write("output.txt", contents)
```

### `net`

Low-level TCP networking:

```python
import net

let stream = net.tcp_connect("127.0.0.1:8080")
net.tcp_send(stream, "GET / HTTP/1.0\r\n\r\n")
let response = net.tcp_recv(stream, 4096)
net.tcp_close(stream)
```

### `http`

Simple HTTP client:

```python
import http

let body = http.http_get("https://example.com/api/data")
let resp = http.http_post("https://example.com/api/submit", "{\"key\": \"value\"}")
```

### `random`

Random number generation:

```python
import random

random.random_seed(42)
let f = random.random_get()          # float in [0.0, 1.0)
let n = random.random_int(1, 100)    # int in [1, 100]
```
