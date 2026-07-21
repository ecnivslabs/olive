# Debugger

Olive has one debugging engine with two frontends: `pit dap` speaks the
Debug Adapter Protocol for VS Code (and any other DAP client), and `pit
debug <file>` speaks a flatter newline-delimited JSON protocol built for AI
agents and scripts that don't want the DAP handshake ceremony. Both give you
breakpoints, stepping, full variable inspection, variable editing,
expression evaluation, fault stops with the runtime's own error code at the
fault site, conditional breakpoints, hit counts, and logpoints.

## Why JIT-only

Debugging compiles your program with the JIT, not the AOT compiler, even
if you'd normally ship it AOT. The JIT and AOT paths run identical MIR, so
nothing about your program's behavior changes; what differs is that a debug
session instruments that MIR with hooks (breakpoint checks, frame capture)
before handing it to the same code generator everything else uses. Programs
not run under a debugger never see these hooks at all -- `pit run` and AOT
builds are byte-for-byte unaffected, and the hooks add zero measurable
overhead to normal runs.

## VS Code

Install the Olive extension, open a `.liv` file, set a breakpoint in the
gutter, and press F5 (or use Run and Debug). The default launch
configuration runs the currently open file; add a `.vscode/launch.json` for
more control:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "olive",
      "request": "launch",
      "name": "Run main.liv",
      "program": "${workspaceFolder}/main.liv",
      "stopOnEntry": false
    }
  ]
}
```

Breakpoints support VS Code's standard condition, hit count, and log
message fields (right-click a breakpoint in the gutter to set them):

* **Condition** -- an expression like `i == 500`, `i + 1 == count * 2`, or
  `name == "bob" and not done`: paths, `int`/`float`/`bool`/string literals,
  `+ - * / %`, parens, and `and`/`or`/`not`/comparisons -- the same
  arithmetic olive source itself uses. The breakpoint only stops when it
  evaluates to `true`.
* **Hit Count** -- `%10` stops on every 10th hit, `>=5` stops from the 5th
  hit onward, a bare number stops on that exact hit.
* **Log Message** -- text with `{expr}` interpolations, printed as an output
  event on every hit; the breakpoint never stops.

## Editing variables

While stopped, `setVariable` (edit a value directly in the Variables panel)
and `setExpression` (edit via a watch-style expression, e.g. `xs[1]` or
`p.x`) work at any depth: a top-level local, or a field/element/entry
inside a struct, list, tuple, dict, or enum payload.

A scalar target (`int`, `float`, `f32`, `bool`, `str`, or `None`) parses the
new value against its own type: `true`/`false` for `bool`, `None` for a
nullable slot, a plain number for `int`/`float`/`f32`, and either a quoted
`"..."` string (backslash-escaping the next character) or the bare text
itself for `str`.

A list, vector, set, tuple, dict, struct, or enum target replaces the
*whole* value with a fresh one built from real olive expression syntax --
`[1, 2, 3]`, `(1, 2)`, `{"a": 1}`, `Point(1, 2)`, `Some(5)` -- arithmetic
and paths work inside it too (`[n, n + 1]`), resolved against the frame
you're editing in. A struct or enum constructor's name must match the
target's own type; a struct write fills every field (a reused heap slot
isn't zeroed, so a partial write would leak old data through) and an enum
write can freely switch which variant is active.

One thing is deliberately out of scope, reported as a normal failed request
rather than silently accepted: **a local in a frame other than the topmost
one.** A container edit is a direct write into shared heap memory, so it's
real and immediate regardless of which frame you're editing through. A
top-level local's storage isn't memory the debugger can address directly
(the JIT is free to keep it in a register); the write instead rides along
with that same local's next real read, which only the frame actually
parked right now is guaranteed to reach before it matters.

## `pit debug` for scripts and agents

`pit debug program.liv` starts a session that reads one JSON object per
line from stdin and writes one JSON object per line to stdout. A request
carries an `id` only when it expects a response; `continue`, `next`,
`stepIn`, `stepOut`, and `pause` are fire-and-forget, their effect observed
through events instead. A response is `{"id":N,"ok":true,...}` or
`{"id":N,"ok":false,"error":"..."}`; an event is `{"event":"...",...}` with
no `id`.

Requests:

* `{"id":1,"cmd":"launch","program":"foo.liv","stopOnEntry":false}` --
  compiles and starts the debuggee, parked before it runs. `program`
  defaults to the file passed on the command line.
* `{"id":2,"cmd":"break","source":"foo.liv","lines":[6,{"line":10,"cond":"i == 500"},{"line":14,"hits":"%10"},{"line":18,"log":"i is {i}"}]}` --
  replaces every breakpoint in `source`. Each entry in `lines` is either a
  bare line number or an object with `line` plus any of `cond`, `hits`,
  `log`. Responds with `{"id":2,"ok":true,"lines":[{"line":6,"verified":true},...]}`.
* `{"cmd":"continue"}`, `{"cmd":"next"}`, `{"cmd":"stepIn"}`,
  `{"cmd":"stepOut"}`, `{"cmd":"pause"}` -- no `id`, no response. Each takes
  an optional `"thread":N`, defaulting to `1` (the main debuggee thread);
  target whichever thread a `stopped` event named.
* `{"id":9,"cmd":"threads"}` -- every currently traced OS thread: the main
  thread plus any `aio` executor/spawn/pool worker that's called into
  instrumented code. `{"id":9,"ok":true,"threads":[{"id":1,"name":"main"},{"id":2,"name":"olive-spawn-task"}]}`.
* `{"id":3,"cmd":"stack"}` (optionally `"thread":N`) --
  `{"id":3,"ok":true,"frames":[{"id":0,"fn":"main","file":"foo.liv","line":6},...]}`,
  innermost frame first. Each frame's `id` already encodes which thread it
  belongs to, so `vars`/`eval`/`setVar`'s `frame` argument never needs a
  `thread` of its own once you have one from here.
* `{"id":4,"cmd":"vars","frame":0,"ref":0}` -- named locals of frame 0
  (`ref:0` means "the frame itself"); pass a nonzero `ref` from a previous
  response to expand that value's children. `{"id":4,"ok":true,"vars":[{"name":"xs","type":"[int]","value":"[1, 2, 3]","ref":7},...]}`.
* `{"id":5,"cmd":"eval","frame":0,"expr":"xs[1].name"}` -- evaluates a
  path (`ident`, `.field`, `[index]`, `["key"]`, chained) or an arithmetic
  expression over paths and literals (`n + 1`, `(a - b) * 2`, `xs[0] % 2`).
  A bare path stays expandable (`ref` nonzero for a struct/list/dict/enum);
  anything with an operator resolves to a plain scalar (`ref` always `0`).
  `{"id":5,"ok":true,"value":"...","type":"...","ref":0}`.
* `{"id":6,"cmd":"setVar","frame":0,"name":"i","value":"5"}` -- sets a
  top-level local (`ref` omitted or `0`) or, with a nonzero `ref` from a
  previous `vars`/`eval` response, a child of that container instead
  (`{"ref":7,"name":"0","value":"99"}` for a list element, `{"ref":7,
  "name":"x","value":"99"}` for a struct field). `{"cmd":"setVar","frame":0,
  "expr":"xs[1]","value":"99"}` does the same through a path instead of a
  reference. `value` is parsed against the target's own type -- see
  "Editing variables" above for the full grammar, scalar and whole-aggregate
  alike. Responds with the freshly re-read value,
  same shape as `vars`/`eval`: `{"id":6,"ok":true,"value":"5","type":"int","ref":0}`.
* `{"id":7,"cmd":"quit"}` -- ends the session; the process exits after
  responding.

Events:

* `{"event":"stopped","reason":"entry"|"breakpoint"|"step"|"pause"|"exception","fn":"main","file":"foo.liv","line":6}`
* `{"event":"fault","code":"E0701","message":"...","file":"foo.liv","line":6}`
  -- sent right after a `stopped` event whose reason is `exception`, with the
  runtime's own error code and message at the exact fault site.
* `{"event":"output","category":"stdout"|"stderr"|"console","text":"..."}`
  -- `stdout`/`stderr` are the debuggee's own prints; `console` is a
  logpoint firing or a one-time condition-evaluation error.
* `{"event":"exited","code":0}` -- the debuggee ran to completion (or
  resumed past a fault, in which case `code` is 1).
* `{"event":"threadStarted","thread":2}` / `{"event":"threadExited","thread":2}`
  -- an `aio` executor/spawn/pool worker thread began or finished running
  instrumented code.

## Fault stops

An uncaught runtime fault (an out-of-bounds index, a division by zero, and
so on) parks the debuggee at the fault site instead of exiting immediately:
you get a `stopped` event with reason `exception`, followed by a `fault`
event (headless) or an `exceptionInfo` response (DAP) carrying the same
error code you'd see in a plain `pit run`. Frames are intact, so you can
inspect locals up the call stack before deciding to resume. Resuming means
the process runs `abort_with` to completion and exits 1, same as an
undebugged crash.

## Multi-threaded programs

`aio`'s executor pool, a no-`await` `async fn` call (which runs via
`olive_spawn_task`), and `pool_run`/`pool_run_sync` all run real
olive-compiled code on their own OS thread; each one becomes a
traced thread the first time it enters instrumented code, with its own call
stack, breakpoints, and stepping, entirely independent of the main thread's
(the DAP protocol's `threads` request and the headless `threads` command list
every one; a `stopped` event names exactly which thread hit it).

## Async functions

An `async fn` that `await`s something compiles to a heap-frame state machine:
its body suspends back to the executor at every `await` and resumes later,
possibly on a different executor thread. Breakpoints, stepping, variable
inspection, `setVariable`, and fault stops all work inside such a function
just as they do in ordinary code. Named locals persist across a suspend --
a value set before an `await` reads back correctly after it -- because the
frame's shadow travels with the state machine rather than living on any one
native call stack.

The call stack of a stopped `async fn` continues past its own frames up the
chain of callers that are suspended awaiting it: each is another `async fn`
frame, parked on its own `await`, shown at the line it suspended on with its
own locals readable. This is the logical async stack (reconstructed from the
executor's await graph), not the executor's physical worker-thread stack.

A step that crosses an `await` follows the frame across the suspension: a
`next` over a line that awaits stops on the following line once the awaited
work resolves and the frame is polled again. A `step out` finishes the
`async fn` and stops wherever its own completion runs, not in the logical
awaiter -- that frame resumes as an independent executor task.

## Limitations

* One debug session per `pit` process. Launching a second session in the
  same process while one is active fails; end the first with `disconnect`
  (DAP) or `quit` (headless) first, or start a second `pit dap`/`pit debug`
  process for a genuinely concurrent session -- the same thing every DAP
  client (including VS Code) already does per debug session. This is intrinsic
  to the process-global, zero-overhead hook design, not a gap to close: the
  runtime hooks a debug session installs are process-wide, and threading a
  per-session handle through every statement hook would tax the hot path for
  a concurrency every real client already gets by running one process per
  session. `aio`'s executor pool is likewise process-global and binds to
  whichever session first spins it up, so a single session per process is the
  shape the whole model assumes.
