# Error Handling

Olive does not use exceptions. Errors are ordinary values returned from functions, so every failure case shows up in the function signature and the caller has to deal with it.

The examples below build one small program: parsing three strings into a packed RGB colour. The full version lives in `examples/error_handling_try_propagation.liv`.

## Errors as Union Returns

A function that can fail returns a union: the success type on one side, an error type on the other. The error type is an `enum`, which lets you spell out every way the operation can go wrong and carry data with each one.

```rust
enum ByteError:
    Empty
    OutOfRange(int)

fn parse_byte(s: str) -> int | ByteError:
    if len(s) == 0:
        return Empty()
    let n = int(s)
    if n < 0 or n > 255:
        return OutOfRange(n)
    return n
```

Construct a variant by calling it. Variants that carry data take their fields as arguments (`OutOfRange(n)`); variants that carry none still use empty parentheses (`Empty()`).

## Propagating Errors (`try` and `?`)

Most code wants to pass an error up rather than handle it at every call. Put `try` before the call, or the `?` shorthand after it: if the value is an error variant the current function returns it immediately, otherwise evaluation continues with the success value. Both forms are equivalent.

```rust
fn parse_rgb(r: str, g: str, b: str) -> int | ByteError:
    let red = try parse_byte(r)
    let green = parse_byte(g)?
    let blue = parse_byte(b)?
    return red * 65536 + green * 256 + blue
```

`parse_rgb` never mentions the individual failures; the first bad channel short-circuits the whole function. The caller's return type has to include the error being propagated, so it has somewhere to go.

## Handling the Result

At the point where you actually deal with failure, use `match` to split the success and error paths. A capitalized pattern matches that error variant; a lowercase name binds the success value.

```rust
fn report(r: str, g: str, b: str):
    match parse_rgb(r, g, b):
        Empty:
            print(f"({r}, {g}, {b}) -> a channel is missing")
        OutOfRange(n):
            print(f"({r}, {g}, {b}) -> {n} does not fit in a byte")
        packed:
            print(f"({r}, {g}, {b}) -> packed {packed}")
```

The capitalization is what tells them apart: a pattern starting with an uppercase letter matches that variant, while a lowercase name is a binding that catches whatever is left.

## Optional Values with `None`

When the only failure is "there is nothing here", return `T | None` and check for `None` directly. This is lighter than a dedicated error type when no message is needed.

```rust
fn first_even(xs: [int]) -> int | None:
    for x in xs:
        if x % 2 == 0:
            return x
    return None

let found = first_even([1, 3, 4])
if found != None:
    print(f"found {found}")
```

## Parsing Input

`int(s)` and `float(s)` assert that `s` parses: a malformed string panics, so they're for input you already trust (a config value you control, a literal you just formatted). For input you don't control, use `s.to_int() -> int | None` and `s.to_float() -> float | None` instead. Same parsing grammar, no panic on failure:

```rust
fn double_it(s: str) -> int | None:
    let n = s.to_int()
    if n == None:
        return None
    return n * 2
```

`to_int`/`to_float` return `T | None`, not `T | Error`, so `try`/`?` (which only propagate an *error* variant) pass a `None` result straight through unchanged rather than short-circuiting. Reach for the `None`-specific idioms instead: `??` to fill a default, a guard to bail out, or `match`:

```rust
let n = s.to_int() ?? 0             // fill a default

let m = s.to_int()
if m == None:                       // bail out explicitly
    return None

match s.to_int():
    None:
        print("not a number")
    n:
        if n != None:
            print(f"parsed {n}")
```

## Assertions and Panic

Assertions catch invariant violations that represent bugs, not recoverable conditions. Use them when a false condition means the program is in a state it was never meant to reach.

```rust
assert len(items) > 0, "cannot process an empty list"
```

A failed assertion aborts execution immediately and prints the failing location. When the asserted expression is a top-level comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`), the fault also reports the actual `left`/`right` values, not just the source text:

```rust
assert xs == ys
// [E0712] panic: assertion failed (left: [1, 2, 3], right: [1, 2, 4])
```

Under `pit run` (the debug pipeline), a fault also prints the call chain that led to it, innermost frame first -- which function called which, and from where. AOT release builds (`pit build --release`) skip this: it costs nothing there, but also shows nothing beyond the caret at the fault site itself. Reach for `pit run` when you need to see how execution got somewhere, not just where it ended up.

## Diagnostic Codes

Every compiler error and warning carries a code, such as `E0400` for an error or `W0610` for a warning. Runtime faults like a failed assertion, an integer divide by zero, or an uncaught Python exception are coded too, and each one points at the source line that caused it.

Run `pit explain <code>` to read what a code means and how to resolve it:

```sh
pit explain E0400
```

Many diagnostics suggest a concrete fix. Run `pit fix` to apply the fixes the compiler marks as safe across your project.
