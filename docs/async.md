# Async and Concurrency

Olive makes it easy to write programs that do many things at once. Whether you're building a web server or a data processing tool, Olive's `async` and `await` keywords let you write concurrent code that is both fast and easy to read.

## What is Async?

Normally, if your code has to wait for something (like downloading a file) the whole program stops until it's done. With `async`, your program can do other work while it waits.

## `async fn` and `await`

To make a function asynchronous, just add `async` before `fn`. Inside that function, you use `await` to wait for a task to finish.

```python
async fn fetch_data(url: str) -> str:
    # This won't stop the whole program; it just pauses this function
    let response = await http.http_get(url)
    return response
```

Calling an `async` function doesn't run it immediately. It returns a "future", which is like a promise that the work will eventually be done. The work only starts when you `await` it.

## Running Multiple Tasks at Once

### `gather`: Wait for everything

If you have a bunch of tasks and you want to run all of them at the same time, use `gather`:

```python
let results = await gather([
    fetch_data("https://site1.com"),
    fetch_data("https://site2.com"),
])
# All fetches happen at once!
```

### `select`: Wait for the winner

Sometimes you just want the result of the first task that finishes. Use `select`:

```python
let [index, result] = await select([task_a(), task_b()])
print(f"Task {index} won the race!")
```

### `cancel`: Stopping a task

If you decide you don't need a task anymore, you can stop it with `cancel`:

```python
let task = fetch_data("https://slow-site.com")
cancel(task)
```

## How it Works (Simply)

Olive's concurrency is built to be extremely efficient.

- **Lightweight**: Unlike "threads" in other languages, which can use a lot of memory, Olive's async tasks are tiny. You can run thousands of them at once without slowing down your computer.
- **Fast**: Olive uses all the cores of your CPU automatically. It spreads your tasks across your processor to get the work done as fast as possible.
- **No Waste**: Most async systems use extra memory every time they pause. Olive doesn't. It's designed to be as lean as possible, so your programs stay fast even under heavy load.

