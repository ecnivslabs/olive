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

Common mathematical functions:

```python
import math

let s = math.sin(3.14)
let c = math.cos(0.0)
let t = math.tan(0.785)

let a = math.asin(1.0)
let b = math.acos(0.0)
let r = math.atan(1.0)
let r2 = math.atan2(1.0, 1.0)

let l = math.log(2.718)
let l10 = math.log10(100.0)
let e = math.exp(1.0)
```

### `io`

Synchronous file operations:

```python
import io

let contents = io.file_read("data.txt")
io.file_write("output.txt", "hello")
```

### `aio`

Asynchronous file operations — these return futures and must be used with `await`:

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
