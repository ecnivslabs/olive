# Compiler Internals

Olive's compiler is built as a series of lowering passes, moving from expressive, high-level syntax to low-level machine code via a Middle Intermediate Representation (MIR).

## The Compilation Pipeline

The Olive compiler is structured as a series of distinct passes, each refining the representation of the program.

### 1. Lexical Analysis (`lexer/`)
The lexer reads the raw source code and converts it into a stream of tokens. It handles indentation and dedentation, which are crucial for Olive's block structure.

### 2. Parsing (`parser/`)
The parser consumes tokens and builds an **Abstract Syntax Tree (AST)**. It uses a recursive descent approach to handle Olive's grammar, including complex expressions like list comprehensions and the walrus operator.

### 3. Semantic Analysis (`semantic/`)
This stage consists of two main parts:
- **Resolution**: Maps identifiers to their definitions and enforces scope and visibility rules.
- **Type Checking**: Infers and validates types across the entire program using a unification-based system.

### 4. MIR Lowering (`mir/`)
The AST is "lowered" into a **Middle Intermediate Representation (MIR)**. MIR is a lower-level, control-flow graph (CFG) representation where complex constructs (like loops and if-statements) are broken down into simple basic blocks and jumps.

### 5. Borrow Checking (`borrow_check/`)
The borrow checker operates on the MIR. It performs dataflow analysis to track the ownership and borrow state of every local variable. By using the CFG, it can precisely determine lifetimes (Non-Lexical Lifetimes) and ensure that memory safety rules are never violated.

### 6. Codegen (`codegen/`)
The final MIR is handed off to the **Cranelift** backend. Cranelift is a high-performance JIT compiler that translates the MIR into highly optimized machine code for the host architecture.

## Why MIR?

Lowering to MIR before borrow checking and codegen provides several advantages:
- **Precision**: MIR's explicit control flow allows the borrow checker to be much more accurate than if it operated on the AST.
- **Optimization**: MIR provides a clean target for middle-end optimizations.
- **Backend Agnostic**: By having a robust intermediate representation, Olive can easily support different backends (like LLVM) in the future.

## Error Reporting

Olive uses the `ariadne` library to provide "industrial-grade" diagnostics. Every error in the compiler is associated with a `Span` (file, line, and column) to provide clear and helpful feedback to the developer.
