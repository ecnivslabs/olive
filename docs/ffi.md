# C / Rust Interop (FFI)

Olive calls external libraries written in C, C++, or Rust, as long as they expose a C-compatible ABI. The calls compile down to direct foreign function calls with no runtime overhead.

## Native Imports

Use the `import` statement to load a shared library (`.so`, `.dll`, or `.dylib`) and declare the signatures you need from it:

```olive
import "libc.so.6" as libc:
    fn printf(fmt: str, *args) -> int
    fn malloc(size: int) -> *void
    fn free(ptr: *void)
```

The compiler binds each declared signature to a direct call at compile time. A trailing `*args` marks a variadic function such as `printf`.

### Strings

Olive strings are UTF-8. When you pass a `str` to a parameter that a C function expects as `char*`, the compiler hands over a null-terminated copy automatically, so you declare the parameter as `str` and call it with an ordinary Olive string:

```olive
import "libc.so.6" as libc:
    fn puts(s: str) -> int

fn main():
    unsafe:
        libc.puts("written through libc")
```

## Structs and Unions

Declare the layout of native structs and unions inside the import block so it matches the C memory layout. A union is written as `union struct`:

```olive
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

```olive
import "libfoo.so" as foo:
    struct Flags:
        is_ready: int @ 1
        error_code: int @ 3
        reserved: int @ 4
```

## Calling Conventions

The C calling convention is the default. To name a different one, put a convention directive above the function. This matters mainly on Windows:

```olive
import "user32.dll" as win:
    @stdcall
    fn MessageBoxA(hWnd: *void, text: str, caption: str, type: int) -> int
```

The directives are `@cdecl`, `@stdcall`, and `@fastcall`. `@stdcall` and `@fastcall` only apply to 32-bit Windows; on every other target they carry no meaning, and the compiler warns if you use them there.

## Unsafe Blocks

The borrow checker cannot reason about memory across the FFI boundary or through raw pointers, so foreign calls and pointer dereferences must sit inside an `unsafe:` block:

```olive
import "libc.so.6" as libc:
    fn malloc(size: int) -> *void
    fn free(ptr: *void)

fn allocate_example():
    unsafe:
        let ptr = libc.malloc(1024)
        libc.free(ptr)
```

Keep `unsafe` scopes small and wrap pointer work behind a safe interface.

### Marking FFI as Safe (`@safe`)

If a native import block or a specific function is known to be safe (no memory risks), mark it with `@safe`. This skips the `unsafe:` requirement:

```olive
// All functions in this block are safe to call
@safe
import "libm.so" as math:
    fn sqrt(x: float) -> float
    fn sin(x: float) -> float

// Or mark individual functions
import "libfoo.so" as foo:
    @safe
    fn get_version() -> int
    fn set_buffer(ptr: *void, len: int)  // still requires unsafe
```

## Pointers vs References

* **References** (`&T` and `&mut T`): safe, tracked, and validated by the compiler.
* **Raw pointers** (`*T` and `*void`): unchecked addresses, only usable inside `unsafe` blocks.

## Pod Native Libraries

A pod that wraps a C or Rust library ships prebuilt binaries through `[native]` in `pit.toml` instead of asking users to install a system library by hand:

```toml
[native]
lib = "tokenizer"
```

`lib` is the stem. `pit` derives every filename from it: `libtokenizer.so` on Linux, `libtokenizer.dylib` on macOS, `libtokenizer.dll` on Windows. The pod source keeps one portable line:

```olive
import "libtokenizer.so" as native:
    fn tokenizer_version() -> str
```

At compile time `pit` resolves that bare name to the owning pod's `native/` directory, stages the library beside the output binary, and links with a relocatable rpath (`$ORIGIN` on Linux, `@loader_path` on macOS). The output binary runs wherever its directory goes. No system install, no `LD_LIBRARY_PATH`.

Optional keys: `build` (argv run directly with no shell, defaults to `["cargo", "build", "--release"]`), `dir` (where the build leaves the library, defaults to `"target/release"`), `targets` (subset of the five supported targets, defaults to all). `[native].build` runs only for the root project, never for an installed dependency.
