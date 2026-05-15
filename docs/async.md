# Async and Concurrency

Olive uses a lightweight, cooperative concurrency model instead of OS-level threads. This allows you to run thousands of concurrent tasks with minimal memory overhead while using a sequential, highly readable syntax.

## Asynchronous Functions

Declare asynchronous functions with the `async` keyword. Use the `await` keyword within these functions to yield execution during I/O operations (such as network or file access):

```rust
async fn fetch_user(id: int) -> User:
    let raw = requests.get(f"https://api.example.com/users/{id}")
    return User.parse(raw)
```

Calling an `async` function returns a **Future** (an un-evaluated task definition). Execution only starts when the future is `await`ed or passed to the runtime executor.

## Async Blocks

To execute a block of code asynchronously without defining a separate function, use `async:`:

```rust
fn main():
    let data = [1, 2, 3]

    // Starts task execution concurrently in the background
    async:
        process_data(data)

    print("This runs while data is processing!")
```

## Task Parallelism

### Waiting on Multiple Tasks (`gather`)

To execute multiple futures concurrently and block until all have completed, use `gather`:

```rust
let [site1, site2] = await gather([
    fetch_data("https://site1.com"),
    fetch_data("https://site2.com")
])
```

### Racing Tasks (`select`)

To execute multiple tasks concurrently and resolve as soon as the first one completes (canceling the remaining tasks), use `select`:

```rust
let winner = await select([task_a(), task_b()])
```

## Concurrency Runtime Characteristics

* **Zero-Overhead State Machines**: Olive's compiler generates structural state machines for async functions at compile-time. This avoids high dynamic heap allocation costs when pausing tasks.
* **Work-Stealing Executor**: The runtime automatically schedules active tasks across all available CPU cores using a work-stealing thread pool.
* **Thread Safety**: The compiler's borrow checker enforces ownership rules on references passed across async boundaries, eliminating data races at compile-time.
