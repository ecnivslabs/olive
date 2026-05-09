# Compiler Internals

The Olive compiler (olivc) is a high-performance compilation pipeline designed for the JIT era. It transforms high-level, expressive source code into optimized machine code through a series of increasingly lower-level representations.

## 1. Lexical Analysis (`lexer/`)
The lexer converts raw UTF-8 source code into a stream of structured **Tokens**.
*   **DFA-Based**: The lexer uses a deterministic finite automaton approach for high performance.
*   **Indentation Tracking**: Unlike many C-family languages, Olive uses whitespace for block structure. The lexer maintains an internal stack of indentation levels to emit `INDENT` and `DEDENT` tokens.
*   **F-Strings**: The lexer identifies interpolated strings and breaks them into a series of constant and expression tokens for the parser.

## 2. Parsing (`parser/`)
The parser consumes the token stream and builds an **Abstract Syntax Tree (AST)**.
*   **Recursive Descent**: It uses a handwritten recursive descent parser to provide quality error recovery and diagnostics.
*   **Operator Precedence**: Expressions are parsed using a Pratt parsing-inspired approach to manage complex operator hierarchies (e.g., walrus operator `:=` vs assignment `=`).
*   **Structured Grammar**: The parser is designed to be familiar to Python developers while enforcing the stricter structure required for a systems language.

## 3. Semantic Analysis (`semantic/`)
This stage is where the compiler analyzes the program structure. It consists of several critical phases:
*   **Name Resolution**: Builds a hierarchy of symbol tables. It handles shadowing, nested scopes, and module-level visibility (enforcing privacy for `_`-prefixed names).
*   **Type Inference**: Olive uses a Hindley-Milner-inspired type system with unification. While types are static, the compiler can often infer them, allowing you to omit annotations in many cases.
*   **Method Resolution**: Dispatches method calls to the correct implementation, handling inheritance and trait-like constraints.

## 4. Middle Intermediate Representation (MIR)
MIR is the core of the Olive compiler. It is a control-flow graph (CFG) where every block ends in a clear terminator.
*   **Basic Blocks**: A sequence of statements that are executed together. No jumps are allowed into or out of the middle of a block.
*   **Terminators**: Every block ends with a `Goto`, `SwitchInt`, `Return`, or `Unreachable`.
*   **Lowering**: High-level constructs like `for` loops, `with` statements, and comprehensions are lowered into simple jumps and assignments in MIR.

## 5. Borrow Checking (`borrow_check/`)
The borrow checker ensures memory safety without a garbage collector. It operates on the MIR CFG.
*   **Liveness Analysis**: Computes which variables are "live" at each point in the program.
*   **Dataflow Tracking**: Tracks the origin of every reference.
*   **NLL (Non-Lexical Lifetimes)**: Olive's borrow checker is fine-grained. It knows that a borrow ends as soon as the reference is no longer used, even if the scope hasn't ended.
*   **Ownership Rules**: Enforces that a value has exactly one owner and that mutable borrows are exclusive.

## 6. Codegen & JIT Runtime (`codegen/`)
The final stage translates MIR into machine code via the **Cranelift** engine.
*   **SSA Generation**: MIR is converted into Static Single Assignment (SSA) form, which is the native language of the Cranelift backend.
*   **Intrinsics**: The JIT runtime provides a set of optimized "intrinsics" for low-level operations like memory allocation, string manipulation, and SIMD.
*   **Execution**: Compiled functions are loaded into executable memory and invoked directly by the Olive runtime.

## Error Reporting & Diagnostics
Olive uses the `ariadne` crate to provide colorized error reports. Each diagnostic includes:
1.  **Error Code & Message**: A clear description of the issue.
2.  **Snippet**: The relevant source code with indicators pointing to the exact location.
3.  **Help Text**: Suggestions on how to resolve the error based on semantic analysis.

## Performance Philosophy
Every stage of the Olive compiler is built with **JIT-first** performance in mind. It prioritizes algorithms with low algorithmic complexity (often linear) to ensure that the time from `pit run` to execution is measured in milliseconds.
