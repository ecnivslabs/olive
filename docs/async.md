# Async and Concurrency

Olive has built-in support for asynchronous programming through `async fn` and `await`. The model is stackless: async functions compile to state machines, not to functions that run on their own stack. Suspension points don't require heap allocation. The executor is multi-threaded, backed by a thread pool sized to the CPU count.

## `async fn`

Declare an asynchronous function by adding the `async` keyword before `fn`:

```python
async fn fetch_data(url: str) -> str:
    let response = await http.http_get(url)
    return response
```

Calling an `async fn` produces a future. The function doesn't start running until the future is awaited.

## `await`

Use `await` to suspend the current async function until a future completes:

```python
let result = await some_future
```

`await` can only appear inside an `async fn` or an `async:` block. The suspension is cooperative; the executor can run other tasks while waiting.

## `async:` Blocks

An `async:` block creates an inline async context. It can be used as a statement or expression where you need async behavior without defining a separate function:

```python
async:
    let data = await fetch_data("https://example.com/api")
    process(data)
```

## `gather`: Wait for All

`gather` takes a list of futures and waits for all of them to complete. It returns a list of results in the same order as the input:

```python
let results = await gather([
    fetch_data("https://example.com/page1"),
    fetch_data("https://example.com/page2"),
    fetch_data("https://example.com/page3"),
])
# results[0] is the response from page1, results[1] from page2, etc.
```

All futures run concurrently. `gather` doesn't return until the last one finishes.

## `select`: Wait for the First

`select` takes a list of futures and returns as soon as any one of them completes. The return value is a two-element list `[index, result]`, where `index` is which future finished first:

```python
let [idx, val] = await select([task_a(), task_b(), task_c()])
print(f"Task {idx} finished first with: {val}")
```

The remaining futures are left running. If you want to stop them, cancel them explicitly.

## `cancel`: Cancel a Future

`cancel` stops a future from running. If it hasn't started yet, it never will. If it's suspended at an `await`, it won't be resumed:

```python
let f = fetch_data("https://example.com/slow-endpoint")
cancel(f)
```

A common pattern is to race a task against a timeout using `select`, then cancel whichever one didn't win:

```python
let [idx, result] = await select([fetch_data(url), sleep_future(5.0)])
if idx == 1:
    # The timeout finished first; the fetch is still running, cancel it
    cancel(fetch_data(url))
```

## Spawning Background Tasks

Async functions can start background tasks that run independently of the caller. The caller doesn't need to `await` them; they're dispatched to the executor and run when the scheduler gets to them.

This is useful for fire-and-forget work, like logging or non-critical background processing. Keep in mind that a spawned task that panics or errors won't propagate that error to the caller automatically.

## The Executor

Olive's async executor is stackless and multi-threaded.

**Stackless**: Each `async fn` compiles to a state machine. When it hits an `await`, it saves its current state and yields control back to the executor. There's no separate stack per future; the future is just a struct holding the current state and local variables. Suspension and resumption are cheap.

**Multi-threaded**: The executor runs on a thread pool with one thread per logical CPU. Futures are dispatched across threads and run in parallel where possible. There's no global lock on the event loop.

**No heap per suspension**: A future allocates memory once when it's created. Suspending and resuming at `await` points doesn't require additional allocation.

This design keeps async code fast at scale. Many concurrent futures have a low memory footprint, and the thread pool keeps all CPUs busy without the overhead of OS threads per task.
