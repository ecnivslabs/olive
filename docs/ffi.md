# C / Rust Interop (FFI)

Olive calls external libraries written in C, C++, or Rust, as long as they expose a C-compatible ABI. The calls compile down to direct foreign function calls with no runtime overhead.

## Native Imports

Use the `import` statement to load a shared library (`.so`, `.dll`, or `.dylib`) and declare the signatures you need from it:

```rust
import "libc.so.6" as libc:
    fn printf(fmt: str, *args) -> int
    fn malloc(size: int) -> *void
    fn free(ptr: *void)
```

The compiler binds each declared signature to a direct call at compile time. A trailing `*args` marks a variadic function such as `printf`.

### Strings

Olive strings are UTF-8. When you pass a `str` to a parameter that a C function expects as `char*`, the compiler hands over a null-terminated copy automatically, so you declare the parameter as `str` and call it with an ordinary Olive string:

```rust
import "libc.so.6" as libc:
    fn puts(s: str) -> int

fn main():
    unsafe:
        libc.puts("written through libc")
```

## Structs and Unions

Declare the layout of native structs and unions inside the import block so it matches the C memory layout. A union is written as `union struct`:

```rust
import "libfoo.so" as foo:
    struct Settings:
        name: str
        is_bare: int

    union struct Value:
        b: bool
        i: int
        f: float
```

### Bitfields

Inside an import block, give a struct field an explicit bit width with `@`:

```rust
import "libfoo.so" as foo:
    struct Flags:
        is_ready: int @ 1
        error_code: int @ 3
        reserved: int @ 4
```

## Calling Conventions

The C calling convention is the default. To name a different one, put a convention directive above the function. This matters mainly on Windows:

```rust
import "user32.dll" as win:
    @stdcall
    fn MessageBoxA(hWnd: *void, text: str, caption: str, type: int) -> int
```

The directives are `@cdecl`, `@stdcall`, and `@fastcall`. `@stdcall` and `@fastcall` only apply to 32-bit Windows; on every other target they carry no meaning, and the compiler warns if you use them there.

## Unsafe Blocks

The borrow checker cannot reason about memory across the FFI boundary or through raw pointers, so foreign calls and pointer dereferences must sit inside an `unsafe:` block:

```rust
import "libc.so.6" as libc:
    fn malloc(size: int) -> *void
    fn free(ptr: *void)

fn allocate_example():
    unsafe:
        let ptr = libc.malloc(1024)
        libc.free(ptr)
```

Keep `unsafe` scopes small and wrap pointer work behind a safe interface.

## Pointers vs References

* **References** (`&T` and `&mut T`): safe, tracked, and validated by the compiler.
* **Raw pointers** (`*T` and `*void`): unchecked addresses, only usable inside `unsafe` blocks.
