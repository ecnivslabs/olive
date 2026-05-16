# Async and Concurrency

Olive is designed for the modern web, where thousands of concurrent tasks are often handled simultaneously. Instead of heavy threads that consume significant memory, Olive uses a lightweight `async` system for writing concurrent code that remains as readable as a standard script.

## The `async` Function

To make a function asynchronous, just add the `async` keyword. Inside these functions, you can `await` tasks that take time (like network requests or file I/O).

```python
async fn fetch_user(id: int) -> User:
    # This pauses the function, but not the program
    let raw = await http.get(f"https://api.example.com/users/{id}")
    return User.parse(raw)
```

When you call an `async` function, it doesn't run immediately. It returns a **Future** - a promise that the work will happen. The work only begins when you `await` the future.

## Async Blocks

Sometimes you want to run a small piece of code asynchronously without defining a whole new function. You can use an `async:` block for this.

```python
fn main():
    let data = [1, 2, 3]

    # This starts a task in the background
    async:
        process_data(data)

    print("This runs while data is processing!")
```

## Running Tasks in Parallel

### `gather`: All at once

If you have multiple tasks and want to wait for all of them to finish, use `gather`. It runs them in parallel and returns all their results as a list.

```python
let [site1, site2] = await gather([
    fetch_data("https://site1.com"),
    fetch_data("https://site2.com")
])
```

### `select`: The first to finish

If you're racing multiple tasks and only care about the winner, use `select`. It returns the result of the first task that completes and cancels the others.

```python
let winner = await select([task_a(), task_b()])
```

## Why it's different

- **Zero-Cost Pauses**: Most languages use extra memory to save the "state" of a task when it pauses. Olive's compiler calculates this state at compile-time, making pauses almost free.
- **True Parallelism**: Olive automatically spreads your tasks across every core of your CPU. You don't have to manage a thread pool; the language handles it for you.
- **Safety**: The borrow checker applies to async code just like synchronous code. You can't have two tasks changing the same data at once, preventing data races by design.

