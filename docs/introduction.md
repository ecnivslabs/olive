# Introduction to Olive

Olive is a systems-oriented programming language designed for the modern era. It is built for developers who demand the speed of low-level languages but refuse to sacrifice the expressive power and safety of high-level environments.

At its heart, Olive is about **deterministic control**. Instead of relying on a garbage collector that can introduce unpredictable latency, Olive uses **Ownership-Based Resource Management (OBRM)**. This approach allows the compiler to manage memory and resources at compile-time, ensuring that your applications are both lean and incredibly fast.

## Key Features

- **Expressive Syntax**: A clean, indentation-based structure that makes code feel like poetry. No unnecessary symbols, just logic.
- **Strictly Safe**: A state-of-the-art borrow checker with Non-Lexical Lifetimes (NLL) ensures that memory errors are caught before your code even runs.
- **JIT Acceleration**: Leveraging the Cranelift compilation engine, Olive generates optimized machine code on the fly, providing near-native performance from the first execution.
- **Industrial Diagnostics**: Errors are not just messages; they are guides. Olive provides beautiful, context-aware diagnostics that help you fix issues instantly.
- **Built-in Tooling**: A unified CLI for building, testing, and formatting your code, so you can spend more time writing and less time configuring.

## Why Olive?

Olive was created to solve a fundamental trade-off: the choice between developer productivity and runtime efficiency. In many languages, you have to pick one. Olive gives you both.

- **Systems Performance**: Perfect for latency-sensitive applications where every microsecond counts.
- **Safe by Default**: The compiler acts as a guardian, preventing entire classes of bugs (like data races and use-after-free) without requiring manual memory management.
- **Joyful Development**: We believe coding should be fun. Olive's syntax and tooling are designed to get out of your way and let you build great things.

## The Architecture

The Olive compiler pipeline is a masterpiece of modern language engineering:

1. **Lexical Analysis**: Source code is carefully tokenized into meaningful units.
2. **Parsing**: Tokens are organized into a logical Abstract Syntax Tree (AST).
3. **Semantic Analysis**: Symbol resolution and type checking ensure the integrity of your code.
4. **MIR Lowering**: The AST is lowered to a Middle Intermediate Representation (MIR) designed for high-level optimizations.
5. **Borrow Checking**: The MIR is analyzed to enforce strict memory safety and ownership rules.
6. **Optimized Codegen**: The MIR is passed through an optimization suite (inlining, constant folding, etc.) before being compiled to machine code via Cranelift.
