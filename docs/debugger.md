# Debugger

Olive has one debugging engine with two frontends: `pit dap` speaks the
Debug Adapter Protocol for VS Code (and any other DAP client), and `pit
debug <file>` speaks a flatter newline-delimited JSON protocol built for AI
agents and scripts that don't want the DAP handshake ceremony. Both give you
breakpoints, stepping, full variable inspection, expression evaluation,
fault stops with the runtime's own error code at the fault site, conditional
breakpoints, hit counts, and logpoints.

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

* **Condition** -- an expression like `i == 500` or `name == "bob" and not
  done`. The breakpoint only stops when it evaluates to `true`.
* **Hit Count** -- `%10` stops on every 10th hit, `>=5` stops from the 5th
  hit onward, a bare number stops on that exact hit.
* **Log Message** -- text with `{expr}` interpolations, printed as an output
  event on every hit; the breakpoint never stops.

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
  `{"cmd":"stepOut"}`, `{"cmd":"pause"}` -- no `id`, no response.
* `{"id":3,"cmd":"stack"}` -- `{"id":3,"ok":true,"frames":[{"id":0,"fn":"main","file":"foo.liv","line":6},...]}`,
  innermost frame first.
* `{"id":4,"cmd":"vars","frame":0,"ref":0}` -- named locals of frame 0
  (`ref:0` means "the frame itself"); pass a nonzero `ref` from a previous
  response to expand that value's children. `{"id":4,"ok":true,"vars":[{"name":"xs","type":"[int]","value":"[1, 2, 3]","ref":7},...]}`.
* `{"id":5,"cmd":"eval","frame":0,"expr":"xs[1].name"}` -- evaluates a
  path expression (`ident`, `.field`, `[index]`, `["key"]`, chained).
  `{"id":5,"ok":true,"value":"...","type":"...","ref":0}`.
* `{"id":6,"cmd":"quit"}` -- ends the session; the process exits after
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

## Fault stops

An uncaught runtime fault (an out-of-bounds index, a division by zero, and
so on) parks the debuggee at the fault site instead of exiting immediately:
you get a `stopped` event with reason `exception`, followed by a `fault`
event (headless) or an `exceptionInfo` response (DAP) carrying the same
error code you'd see in a plain `pit run`. Frames are intact, so you can
inspect locals up the call stack before deciding to resume. Resuming means
the process runs `abort_with` to completion and exits 1, same as an
undebugged crash.

## Limitations

* Async functions are not instrumented: breakpoints and stepping inside an
  `async fn` don't fire, and its locals aren't visible. Non-async code
  calling into one steps over it like any other call.
* The debuggee always reports as a single thread named `main`, regardless
  of how many OS threads your program actually spawns.
* One debug session per process. Launching a second session while one is
  active fails; end the first with `disconnect` (DAP) or `quit` (headless)
  first.
