# Async and Concurrency

Olive compiles `async` functions into state machines at compile time, so
suspending a task does not allocate. Tasks run on a fixed pool of worker
threads sized to the machine's CPU count, sharing one ready queue.

## Asynchronous Functions

Declare asynchronous functions with the `async` keyword. Use `await` inside
them to yield execution during I/O:

```rust
import aio
import requests

async fn fetch(url: str) -> str:
    return requests.get(url)
```

Calling an `async` function returns a **Future**: a description of the work,
not the work itself. Execution starts when the future is awaited or handed to
the runtime.

## Async Blocks

To run a block of code asynchronously without defining a separate function,
use `async:`. The block evaluates to a future:

```rust
import aio

fn main():
    let fut = async:
        let body = await fetch("https://example.com")
        print(len(body))
    aio.run(fut)
```

`aio.run(future)` drives a future to completion from synchronous code and
returns its result.

## Task Parallelism

### Waiting on Multiple Tasks (`aio.gather`)

`aio.gather` runs a list of futures concurrently and resolves with all their
results, in the same order:

```rust
async fn work(n: int) -> int:
    return n * 2

let results = await aio.gather([work(1), work(2), work(3)])
print(results)   // [2, 4, 6]
```

The result type follows from the futures: gathering `Future[str]` values
gives `[str]`.

### Racing Tasks (`aio.select`)

`aio.select` runs a list of futures concurrently and resolves with the value
of the first one to complete:

```rust
let winner = await aio.select([slow_mirror(), fast_mirror()])
```

The remaining futures are not cancelled automatically; use `aio.cancel` if
they must not finish.

## Channels, Mutexes, Atomics

The `aio` module also provides channels (`chan_new`, `chan_send`,
`chan_recv`, `chan_close`) for passing values between tasks, mutexes for
shared state, and atomic integers for counters. See the [module
reference](modules.md) for the full list.

## Runtime Characteristics

- **State machines, not heap frames**: the compiler turns each `async`
  function into a state machine; suspending stores the live locals in a fixed
  frame instead of allocating per await.
- **Thread-pool executor**: ready tasks are pulled from a shared queue by one
  worker thread per CPU core. A task that blocks a worker (a synchronous call
  inside async code) occupies that thread until it returns, so keep blocking
  work out of hot async paths or hand it to `aio.pool_run`.
- **Share-nothing memory model**: values crossing a task boundary (via
  `chan_send`, `mutex_new`, `mutex_unlock`, or `aio.pool_run`) are either
  moved exclusively or deep-copied before the boundary. No Olive-managed heap
  value is ever reachable from two tasks at the same time.
