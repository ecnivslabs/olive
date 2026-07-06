# Modules and Standard Library

## Importing Modules

Use the `import` statement to bring in other Olive files. Dots in the module name map to directory separators:

```rust
import math
import utilities.network
import physics.gravity as gravity

let x = math.sqrt(16)
let g = gravity.G
```

By default, `import math` looks for `math.liv` in the same directory as the current file.

### Virtual Module: `import meta`

`import meta` is a compile-time virtual module that synthesizes constants from the project manifest:

```rust
import meta

print(meta.NAME)       // project name from pit.toml
print(meta.VERSION)    // project version from pit.toml
print(meta.AUTHOR)     // project author from pit.toml
print(meta.PIT_VERSION) // compiler version
```

These constants are evaluated at compile time and baked into the binary. `from meta import NAME, VERSION` also works.

### Top-Level Execution and Imported Modules

Olive separates imported modules from executable main scripts by placing strict restrictions on top-level execution:

* **Entry Point Only**: Only the file executed directly by the `pit run` command is treated as the "main" script. The compiler automatically executes its top-level statements and calls its `main()` function (if defined).
* **Imported Modules**: When a module is imported by another file, its top-level statements (such as prints, loops, or direct function calls) are **not** executed. The compiler only processes variable declarations, constants, structures, and function definitions.
* **Safe, Side-Effect-Free Imports**: This design ensures that importing a file never produces unexpected side effects (like initiating network calls, print outputs, or database connections). It makes modules completely safe, predictable, and modular.

## From-Imports

If you only need specific names from a module, use `from ... import`:

```rust
from math import sqrt, pi
from data.processing import clean_string as clean, parse_json as parse

print(sqrt(pi))
let data = parse(clean(raw_input))
```

## Native Imports

If a library is written in another language (like C or Rust), it can be used in Olive by defining its interface through a native import.

```rust
import "physics.so" as physics

let result = physics.compute_gravity(10.0, 5.0)
```

The compiler manages symbol resolution and calling convention compliance.

## Visibility and Privacy

Olive uses a naming convention for visibility:

- **Public**: Any name that doesn't start with an underscore. Accessible from other modules.
- **Private**: Names starting with `_` are private to the module where they're defined. The compiler enforces this.

```rust
// In utils.liv
fn _secret():
    pass

// In main.liv
import utils
// utils._secret()  // Error: cannot access private member `_secret`
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

```rust
import models
import utils.network
```

## Standard Library

The standard library is implemented in Olive and resolved via the built-in module loader. All modules live in the `lib/` directory of the toolchain.

### `math`

```rust
import math
```

**Constants**

```rust
math.PI    // 3.141592653589793
math.E     // 2.718281828459045
math.TAU   // 6.283185307179586
math.INF   // 1.0e308
```

**Trigonometry** (all angles in radians)

```rust
math.sin(x)         math.asin(x)
math.cos(x)         math.acos(x)
math.tan(x)         math.atan(x)
                    math.atan2(y, x)
math.degrees(x)     // radians -> degrees
math.radians(x)     // degrees -> radians
```

**Exponential and logarithm**

```rust
math.exp(x)         // e^x
math.log(x)         // natural log
math.log10(x)       // log base 10
math.pow(b, e)      // b^e (floats)
math.ipow(b, e)     // b^e (integers)
```

**Roots and rounding**

```rust
math.sqrt(x)
math.cbrt(x)
math.hypot(x, y)    // sqrt(x^2 + y^2)
math.floor(x)       // -> int
math.ceil(x)        // -> int
math.round(x)       // -> int
math.abs(x)
math.clamp(x, lo, hi)
math.fmod(x, y)
math.copysign(x, y)
```

**Hyperbolic**

```rust
math.sinh(x)    math.asinh(x)
math.cosh(x)    math.acosh(x)
math.tanh(x)    math.atanh(x)
```

**Number theory**

```rust
math.gcd(a, b)
math.lcm(a, b)
math.factorial(n)
math.comb(n, k)     // n choose k
math.perm(n, k)     // n permute k
```

**Utilities**

```rust
math.min(a, b)
math.max(a, b)
math.isclose(a, b)  // abs(a - b) < 1e-9
math.erf(x)
```

### `io`

Synchronous file and directory operations:

```rust
import io
```

**File I/O**

```rust
io.read_file(path) -> str
io.write_file(path, data) -> bool
io.append_file(path, data) -> bool
io.exists(path) -> bool
io.delete(path) -> bool
io.mkdir(path) -> bool
io.listdir(path) -> list
io.stat(path)
io.copy(src, dst) -> bool
io.rename(src, dst) -> bool
io.read_lines(path) -> list
io.read_n(handle, n) -> str
io.write_handle(handle, data) -> bool
io.seek(handle, offset, whence) -> int
io.tell(handle) -> int
io.close(handle)
```

**Path operations**

```rust
io.path_join(a, b) -> str
io.path_dirname(path) -> str
io.path_basename(path) -> str
io.path_ext(path) -> str
io.path_stem(path) -> str
io.path_is_absolute(path) -> bool
io.temp_dir() -> str
io.temp_file() -> str
```

**Standard input**

```rust
io.read_stdin() -> str
io.read_line() -> str
```

**File struct**

```rust
io.open(path, mode = "r") -> File
```

The `File` struct supports the `with` statement:

```rust
with io.open("data.txt") as f:
    let content = f.read()
```

File methods: `read()`, `write(data)`, `append(data)`, `close()`

**Buffered I/O**

```rust
io.bufread_open(path) -> int
io.bufread_line(br) -> str
io.bufread_close(br)
io.bufwrite_open(path) -> int
io.bufwrite_write(bw, data) -> bool
io.bufwrite_flush(bw) -> bool
io.bufwrite_close(bw)
```

### `aio`

Async runtime with channels, mutexes, and atomics:

```rust
import aio
```

**Async execution**

```rust
aio.run(future)                         // run a future to completion
aio.gather(futures)                     // run futures in parallel, resolve with all results in order
aio.select(futures)                     // run futures in parallel, resolve with the first result
aio.cancel(future)                      // cancel a running future
aio.free_future(future)                 // free a future handle
```

**Async file I/O**

```rust
aio.read_file(path)                     // async file read
aio.write_file(path, data)              // async file write
```

**Channels**

```rust
aio.chan_new() -> int
aio.chan_send(chan, val) -> bool
aio.chan_recv(chan)                     // blocks until value available
aio.chan_try_recv(chan)                 // non-blocking receive
aio.chan_len(chan) -> int
aio.chan_close(chan)
aio.chan_free(chan)
```

**Mutexes**

```rust
aio.mutex_new(val) -> int
aio.mutex_lock(m)
aio.mutex_unlock(m, new_val)
aio.mutex_free(m)
```

**Atomics**

```rust
aio.atomic_new(val) -> int
aio.atomic_get(ptr) -> int
aio.atomic_set(ptr, val)
aio.atomic_add(ptr, delta) -> int
aio.atomic_cas(ptr, expected, new_val) -> bool
aio.atomic_free(ptr)
```

**Worker pool**

```rust
aio.pool_size() -> int
aio.pool_run(fn_ptr, arg) -> int
aio.pool_run_sync(fn_ptr, arg) -> int
```

### `net`

TCP and UDP networking:

```rust
import net
```

**TCP**

```rust
net.tcp_connect(addr) -> int
net.tcp_send(conn, data) -> int
net.tcp_recv(conn, max_len) -> str
net.tcp_close(conn)
net.tcp_peer_addr(conn) -> str
net.tcp_set_timeout(conn, secs) -> bool
net.tcp_listen(addr) -> int
net.tcp_accept(server) -> int
net.tcp_listener_addr(server) -> str
net.tcp_listener_close(server)
```

**UDP**

```rust
net.udp_open(bind_addr) -> int
net.udp_send(sock, addr, data) -> int
net.udp_recv(sock, max_len)
net.udp_set_timeout(sock, secs) -> bool
net.udp_close(sock)
```

**DNS**

```rust
net.dns_lookup(hostname) -> str
net.dns_lookup_all(hostname) -> list
```

### `requests`

HTTP client:

```rust
import requests
```

```rust
requests.get(url) -> str
requests.post(url, body) -> str
requests.post_json(url, body) -> str
requests.put(url, body) -> str
requests.delete(url) -> int
requests.status(url) -> int
requests.get_with_headers(url, headers) -> str
```

### `json`

JSON parser and serializer, fully implemented in Olive:

```rust
import json
```

```rust
json.loads(s)           // parse JSON string -> value
json.dumps(obj) -> str  // serialize value -> JSON string
```

Supports parsing: strings, numbers, arrays, objects, booleans, null.

### `yaml`

YAML and TOML parsing:

```rust
import yaml
```

```rust
yaml.parse(s)                       // parse YAML string -> value
yaml.stringify(obj) -> str          // serialize value -> YAML string
yaml.toml_parse(s)                  // parse TOML string -> value
yaml.toml_stringify(obj) -> str     // serialize value -> TOML string
```

### `random`

Random number generation:

```rust
import random
```

```rust
random.seed(n)
random.random() -> float        // float in [0.0, 1.0)
random.randint(min, max) -> int // int in [min, max]
random.choice(lst)              // pick random element
random.shuffle(lst)             // shuffle list in place
```

### `datetime`

Date and time functions:

```rust
import datetime
```

```rust
datetime.now() -> float
datetime.utcnow() -> float
datetime.parse(s) -> float
datetime.format(ts, fmt) -> str
datetime.format_iso(ts) -> str
datetime.parts(ts)
datetime.from_parts(year, month, day, hour, minute, second) -> float
datetime.local_offset() -> int
datetime.to_local(ts) -> float
datetime.from_local(ts) -> float
datetime.weekday(ts) -> int
datetime.weekday_name(ts) -> str
datetime.month_name(ts) -> str
datetime.add_days(ts, n) -> float
datetime.add_hours(ts, n) -> float
datetime.add_minutes(ts, n) -> float
datetime.add_seconds(ts, n) -> float
datetime.add_months(ts, n) -> float
datetime.add_years(ts, n) -> float
datetime.diff_days(a, b) -> int
datetime.diff_seconds(a, b) -> int
datetime.start_of_day(ts) -> float
datetime.end_of_day(ts) -> float
datetime.start_of_month(ts) -> float
datetime.is_leap_year(year) -> bool
datetime.days_in_month(year, month) -> int
```

### `time`

Wall clock and monotonic time:

```rust
import time
```

```rust
time.now() -> float
time.monotonic() -> float
time.sleep(secs)
time.format(ts, fmt) -> str
time.format_iso(ts) -> str
```

### `crypto`

Cryptographic primitives:

```rust
import crypto
```

```rust
crypto.sha256(s) -> str
crypto.md5(s) -> str
crypto.aes_encrypt(key, plaintext) -> str
crypto.aes_decrypt(key, ciphertext) -> str
crypto.argon2_hash(password) -> str
crypto.argon2_verify(password, hash) -> bool
crypto.rsa_keygen()
crypto.rsa_encrypt(pub_key, data) -> str
crypto.rsa_decrypt(priv_key, data) -> str
```

### `encoding`

Encoding and decoding utilities:

```rust
import encoding
```

```rust
encoding.base64_encode(s) -> str
encoding.base64_decode(s) -> str
encoding.url_encode(s) -> str
encoding.url_decode(s) -> str
encoding.hex_encode(s) -> str
encoding.hex_decode(s) -> str
```

### `compress`

Data compression:

```rust
import compress
```

```rust
compress.gzip_compress(data) -> str
compress.gzip_decompress(data) -> str
compress.zstd_compress(data) -> str
compress.zstd_decompress(data) -> str
```

### `regex`

Regular expressions:

```rust
import regex
```

```rust
regex.regex_match(pattern, text) -> bool
regex.find(pattern, text) -> str
regex.find_all(pattern, text) -> list
regex.replace(pattern, text, rep) -> str
regex.replace_all(pattern, text, rep) -> str
regex.captures(pattern, text) -> list
regex.split(pattern, text) -> list
regex.is_valid(pattern) -> bool
```

### `bytes`

Byte buffer operations:

```rust
import bytes
```

```rust
bytes.new(cap) -> int
bytes.from_str(s) -> int
bytes.len(buf) -> int
bytes.push(buf, byte)
bytes.get(buf, idx) -> int
bytes.set(buf, idx, val)
bytes.to_str(buf) -> str
bytes.to_hex(buf) -> str
bytes.concat(a, b) -> int
bytes.slice(buf, start, end) -> int
bytes.free(buf)
bytes.read_u16_le(buf, offset) -> int
bytes.read_u16_be(buf, offset) -> int
bytes.read_u32_le(buf, offset) -> int
bytes.read_u32_be(buf, offset) -> int
bytes.read_u64_le(buf, offset) -> int
bytes.read_u64_be(buf, offset) -> int
bytes.write_u16_le(buf, offset, val)
bytes.write_u16_be(buf, offset, val)
bytes.write_u32_le(buf, offset, val)
bytes.write_u32_be(buf, offset, val)
bytes.write_u64_le(buf, offset, val)
bytes.write_u64_be(buf, offset, val)
```

### `string`

Advanced string operations:

```rust
import string
```

```rust
string.trim(s) -> str
string.trim_start(s) -> str
string.trim_end(s) -> str
string.upper(s) -> str
string.lower(s) -> str
string.replace(s, frm, to) -> str
string.find(s, needle) -> int
string.contains(s, needle) -> bool
string.starts_with(s, prefix) -> bool
string.ends_with(s, suffix) -> bool
string.repeat(s, n) -> str
string.split(s, sep) -> list
string.split_whitespace(s) -> list
string.join(parts, sep) -> str
string.fmt(template, args) -> str
string.char_count(s) -> int
string.is_ascii(s) -> bool
string.grapheme_count(s) -> int
string.graphemes(s) -> list
```

### `os`

Operating system process interface:

```rust
import os
```

```rust
os.getenv(name) -> str
os.setenv(name, val)
os.args() -> list
os.exit(code)
os.exec(cmd) -> str        // run command, capture stdout
os.exec_status(cmd) -> int // run command, return exit code
```

### `sys`

System information:

```rust
import sys
```

```rust
sys.hostname() -> str
sys.pid() -> int
sys.cpu_count() -> int
sys.platform() -> str
sys.arch() -> str
sys.memory_total() -> int
sys.memory_free() -> int
sys.uptime() -> float
sys.username() -> str
sys.home_dir() -> str
sys.cwd() -> str
sys.chdir(path) -> bool
```

### `logging`

Structured logging:

```rust
import logging
```

**Log levels**

```rust
logging.DEBUG  // 0
logging.INFO   // 1
logging.WARN   // 2
logging.ERROR  // 3
```

**Output formats**

```rust
logging.PLAIN  // 0
logging.JSON   // 1
logging.COLOR  // 2
```

**Functions**

```rust
logging.set_level(level)
logging.set_level_str(level)
logging.set_format(fmt)
logging.debug(msg)
logging.info(msg)
logging.warn(msg)
logging.error(msg)
logging.with_field(key, val)
logging.clear_fields()
```

### `uuid`

UUID generation:

```rust
import uuid
```

```rust
uuid.v4() -> str
uuid.nil() -> str
uuid.is_valid(s) -> bool
uuid.to_hex(s) -> str
```

### `websocket`

WebSocket client:

```rust
import websocket
```

```rust
websocket.connect(url) -> int
websocket.send(handle, msg) -> bool
websocket.send_binary(handle, buf) -> bool
websocket.recv(handle) -> str
websocket.recv_binary(handle) -> int
websocket.close(handle)
```

### `result`

Result type utilities for error handling:

```rust
import result
```

```rust
result.ok(val)
result.err(msg) -> obj
result.is_ok(r) -> bool
result.is_err(r) -> bool
result.unwrap(r)
result.unwrap_err(r) -> str
result.unwrap_or(r, default)
result.err_msg(r) -> str
```

### `reflect`

Runtime type introspection:

```rust
import reflect
```

```rust
reflect.typeof(val) -> str
reflect.is_null(val) -> bool
reflect.is_str(val) -> bool
reflect.is_list(val) -> bool
reflect.is_obj(val) -> bool
reflect.is_bytes(val) -> bool
reflect.fields(obj) -> list
reflect.panic(msg)
reflect.atexit(fn_ptr)
reflect.run_exit_hooks()
```

### `signal`

Signal handling:

```rust
import signal
```

```rust
signal.install_sigint_handler(message)
```
