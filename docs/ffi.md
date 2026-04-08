# C / Rust Interop (FFI)

Olive interfaces directly with external libraries written in C, C++, or Rust, provided they expose a C-compatible ABI. This allows you to call native shared libraries with zero runtime overhead.

## Native Imports

Use the `import` statement to load shared libraries (`.so`, `.dll`, or `.dylib`) and declare their signatures:

```python
import "libc.so.6" as libc:
    fn printf(fmt: cstr, *args) -> int
    fn malloc(size: int) -> *void
    fn free(ptr: *void)
```

Olive matches the declared signatures to direct foreign function calls at compile-time.

### C-Strings (`cstr`)

Olive strings are UTF-8 sequences. To interface with null-terminated C strings, use the `cstr` type. The compiler handles the null-termination conversions automatically when passing Olive string literals or variables to a parameter typed as `cstr`.

## Structs and Unions

Define the layout of native structs and unions inside the import block to match the C memory layouts:

```python
import "libgit2.so" as git:
    struct git_repository:
        path: cstr
        is_bare: int
    
    union config_value:
        b: bool
        i: int
        s: cstr
```

### Bitfields

Specify low-level C struct bitfield widths using the `@` symbol:

```python
struct Flags:
    is_ready: int @ 1
    error_code: int @ 3
    reserved: int @ 4
```

## Calling Conventions

The standard C calling convention is the default. If you need to specify a different calling convention (common on Windows), apply convention directives:

```python
import "user32.dll" as win:
    @stdcall
    fn MessageBoxA(hWnd: *void, text: cstr, caption: cstr, type: int) -> int
```

Supported annotations include `@cdecl`, `@stdcall`, and `@fastcall`.

## Unsafe Blocks

Because the borrow checker cannot analyze memory safety across FFI boundaries or raw pointer access, FFI calls and pointer dereferences must occur inside an `unsafe:` block.

```python
import "libc.so.6" as libc:
    fn malloc(size: int) -> *void
    fn free(ptr: *void)

fn allocate_example():
    unsafe:
        let ptr = libc.malloc(1024)
        # Raw pointer operations
        libc.free(ptr)
```

Keep `unsafe` scopes minimal and encapsulate pointer operations inside safe public interfaces.

## Pointers vs References

* **References** (`&T` and `&mut T`): Safe, tracked, and validated by the compiler.
* **Raw Pointers** (`*T` and `*void`): Unchecked addresses. These can only be dereferenced inside `unsafe` blocks.

