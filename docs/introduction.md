# Introduction to Olive

Olive is a systems programming language designed to be both fast and friendly. Usually, you have to choose: you get the speed and safety of a low-level language but deal with complex syntax, or you get the ease of a high-level language but give up performance. Olive is our attempt to bridge that gap.

The syntax is clean and indentation-based, making it easy to read and write. But under the hood, it's a true systems language. There's no garbage collector slowing things down; instead, Olive uses a system called "ownership" to manage memory automatically and safely.

## Philosophy

**Performance without the pain.** We believe you shouldn't have to rewrite your prototype in another language just to make it fast. Olive is designed to produce optimized machine code right from the start.

**Safety you can trust.** Memory leaks, double-frees, and data races are some of the hardest bugs to track down. Olive's compiler catches these errors while you're writing the code, not after you've deployed it.

**Code is for humans.** We spend more time reading code than writing it. Olive's design prioritizes readability. No unnecessary symbols or boilerplate—just the logic of your program.

## Key Concepts

- **Indentation is meaningful**: Your code's structure is defined by how it's indented, keeping things clean and consistent.
- **Ownership and Borrowing**: This is how Olive stays safe without a garbage collector. The compiler keeps track of who "owns" a piece of data and ensures it's cleaned up when it's no longer needed.
- **Instant Feedback**: The `pit` toolchain is fast. From running `pit run` to seeing your code execute takes only milliseconds, even with the full safety checks and optimizations running.
- **Modern Concurrency**: Building high-performance network services should be easy. Olive has built-in support for `async` and `await`, making concurrent code look and feel like regular synchronous code.

## How it Works

When you run an Olive program, it goes through a few stages:

1. **Understanding**: The compiler reads your code and builds a structured map of what you're trying to do.
2. **Safety Check**: This is where the "borrow checker" looks for potential memory issues. If it finds one, it tells you exactly what's wrong and how to fix it.
3. **Optimization**: The compiler cleans up your code—removing unnecessary steps and streamlining the logic—to make it as fast as possible.
4. **Execution**: Finally, it turns your program into machine code that runs directly on your processor.

