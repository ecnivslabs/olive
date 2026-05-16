# Introduction to Olive

Olive is a general-purpose systems language built to solve the conflict between the speed of a low-level language and the ease of a high-level one. Usually, the choice is between managing every byte manually for performance or relying on a garbage collector that might pause the program at critical moments.

Olive bridges this gap. It provides a systems language with a clean, indentation-based syntax that feels like a scripting language, but compiles to native code that runs close to the metal.

## Philosophy

**Performance without the overhead.**
Prototype code should be production code. Olive is designed to produce optimized machine code right from the start. There is no heavy runtime, no garbage collector, and no hidden costs.

**Safety by design.**
Memory leaks and data races are often the result of human error. The Olive compiler acts as a partner, catching these errors during the development phase. If it compiles, it is memory-safe.

**Code is for humans.**
Significantly more time is spent reading code than writing it. Olive removes the noise - no semicolons, no braces, no boilerplate. The logic of the program is structured clearly by indentation.

## Core Concepts

- **Ownership and Borrowing**: This is the heart of Olive's memory safety. Instead of a garbage collector, the compiler tracks who "owns" a piece of data and ensures it's cleaned up the moment it's no longer needed.
- **The Pit Toolchain**: Developer tools should be fast and helpful. `pit` handles everything from creating new projects to running tests and benchmarks, usually in a matter of milliseconds.
- **Fearless Concurrency**: Building high-performance services shouldn't be scary. Olive has first-class support for `async` and `await`, allowing you to write concurrent code that looks and behaves like regular synchronous logic.
- **Native Interop**: Olive doesn't live in a vacuum. It's designed to play well with existing C and C++ libraries, allowing you to use the right tool for the job without jumping through hoops.

## The Journey of a Program

When you run an Olive program, it goes through a few intentional stages:

1. **Analysis**: The compiler builds a representation of your logic, checking for type consistency and structural errors.
2. **The Borrow Checker**: This is where the magic happens. The compiler validates that your memory usage follows the rules of ownership, preventing crashes before they can happen.
3. **The Optimizer**: Your code is refined. The compiler eliminates redundant steps, hoists loops, and prepares the logic for the backend.
4. **JIT Codegen**: Finally, Olive uses the Cranelift backend to generate machine code tailored for your specific processor, then executes it immediately.

