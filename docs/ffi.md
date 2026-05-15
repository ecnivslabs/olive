# Native Interop (FFI)

Olive is designed to be a "good citizen" in the systems ecosystem. It can interface with libraries written in C, C++, or Rust, provided they expose a C-compatible ABI. This allows for the high-performance reuse of existing systems code within the Olive environment.

## Native Imports

The `import` statement can load shared libraries (`.so`, `.dll`, or `.dylib`) and define their interface directly in Olive.

```python
import "libc.so.6" as libc:
    fn printf(fmt: cstr, *args) -> int
    fn malloc(size: int) -> *void
    fn free(ptr: *void)
```

Within a native import block, the signatures of the functions to be used are described. Olive handles the data conversion behind the scenes.

### C-Strings (`cstr`)

Since Olive strings are UTF-8 and C strings are null-terminated byte arrays, Olive provides the `cstr` type for FFI. The compiler automatically converts Olive strings to `cstr` when passing them to native functions.

## Structs and Unions

You can define the layout of native structs and unions within the import block. This ensures that Olive and the native library agree on how data is structured in memory.

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

For low-level C structs that use bitfields, you can specify the bit width using the `@` symbol:

```python
struct Flags:
    is_ready: int @ 1
    error_code: int @ 3
    reserved: int @ 4
```

## Calling Conventions

By default, Olive uses the standard C calling convention. If you need to use a specific convention (common on Windows), you can use directives:

```python
import "user32.dll" as win:
    @stdcall
    fn MessageBoxA(hWnd: *void, text: cstr, caption: cstr, type: int) -> int
```

Supported conventions include `@cdecl`, `@stdcall`, and `@fastcall`.

## The `unsafe` Block

Interacting with native libraries often involves pointers and manual memory management, which the Olive borrow checker cannot validate. To perform these operations, you must use an `unsafe:` block.

```python
import "libc.so.6" as libc:
    fn malloc(size: int) -> *void
    fn free(ptr: *void)

fn allocate_example():
    unsafe:
        let ptr = libc.malloc(1024)
        # ... do something with raw memory ...
        libc.free(ptr)
```

The `unsafe` block tells the compiler (and other developers) that you are taking responsibility for memory safety within that scope. It's best practice to keep `unsafe` blocks as small as possible and wrap them in safe Olive functions.

## Pointers vs References

In regular Olive code, you use **references** (`&T` and `&mut T`), which are tracked and validated by the borrow checker. 

In FFI, you often deal with **raw pointers** (`*T` or `*void`). These are not checked by the compiler and can only be used inside `unsafe` blocks.

- `&T`: Safe, checked reference.
- `*T`: Unsafe, unchecked raw pointer.
