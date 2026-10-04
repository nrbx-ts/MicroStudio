# Architecture

MicroStudio is two layers with one narrow seam between them: a Rust runtime that
owns the simulation, and a TypeScript developer layer that owns tooling. They
speak newline-delimited JSON-RPC over a child process's stdio.

```
                 ┌───────────────────────────────────────────┐
  TypeScript     │ cli ── runtime ── roblox-ts ── test       │
  (developer     │  dev/test/repl  driver  Rojo    helpers   │
   layer)        └──────────────────┬────────────────────────┘
                                    │ JSON-RPC over stdin/stdout
                 ┌──────────────────┴────────────────────────┐
  Rust           │ microstudio-runtime (sidecar binary)       │
  (simulation)   │   protocol ── runtime facade ── server    │
                 │        └── microstudio-luau ──┐            │
                 │              └── datamodel ── services     │
                 │                    └── types              │
                 └───────────────────────────────────────────┘
```

## Why a sidecar rather than napi

The seam is `RuntimeDriver` in `packages/runtime/src/driver.ts`. Today the only
implementation spawns the sidecar (`StdioRuntimeDriver`); a native binding would
implement the same interface without touching callers. Two reasons this order
was chosen:

1. **A crash is contained.** Luau is memory-safe, but a bug in our bindings or a
   user's runaway `while true do end` must not take the developer's test runner
   with it. A separate process is a real boundary.
2. **The runtime stays language-agnostic.** Anything that can spawn a process
   and write JSON lines can drive it — a future debugger, a Rojo plugin, a
   CI runner. No Node addon build step is needed to try it.

The cost is one `JSON.stringify` per call, measured in microseconds against a
run measured in milliseconds.

## Crates, bottom-up

Each crate only depends on the ones above it; tests live with the layer they
cover, and each layer is independently testable.

| Crate | Owns | Does not know about |
| --- | --- | --- |
| `microstudio-types` | `Vector2/3`, `CFrame`, `Color3`, `BrickColor`, `UDim/UDim2`, `Rect`, `Ray` | instances, Lua |
| `microstudio-datamodel` | The instance tree, class table, properties, attributes, signals | Lua, scheduling |
| `microstudio-services` | `World` plus `Players`, `RunService`, `CollectionService`, logs/errors | Lua |
| `microstudio-luau` | The VM, bindings, prelude, scheduler, script loader | the wire protocol |
| `microstudio-runtime` | Protocol, `Runtime` facade, stdio server, `main.rs` | nothing above it |

`World` is the join point: the DataModel tree plus the per-service state that
does not live on an instance (player id allocation, tag signals, RunService
flags, the state store, memory store entries). It is deliberately plain Rust so
it can be asserted on directly.

## The service layer

Every service in Roblox's API dump exists, because
`crates/microstudio-datamodel/src/api/services.txt` is generated from that dump
(`scripts/api-dump.ts`) and the class table falls back to it. A service is
therefore always found by `game:GetService`, and its declared members always
resolve: a function becomes a stand-in returning a value of its declared type,
an event becomes a real signal, and a property reads a zero value (or the
documented `true`, see `default_property_value`). Each stand-in warns **once per
member** through `scheduler.warn`, so a game that leans on something
unimplemented says so instead of failing silently.

Real behaviour is layered on top, and always wins:

- `crates/microstudio-services/` holds the state that is not an instance
  (`memory.rs`, `store.rs`, `players.rs`, ...) with unit tests and no Lua
  dependency, following the borrow contract above.
- `crates/microstudio-luau/src/bind/services/` holds one module per group of
  services. Each `install`s its methods into the shared
  `microstudio.instance_methods` table, which is consulted before properties,
  children and the generated fallback, and each checks its receiver's class.
- A method name that more than one class declares (`GetAsync` on a `DataStore`
  and on a `MemoryStoreSortedMap`) is registered **once** and dispatches on the
  receiver's class, because the table is keyed by name alone.
- Anything that persists goes through `Store`, which is either ephemeral (tests,
  `VmOptions::default()`) or rooted at the state directory the runtime resolved.
  A module that deviates from Roblox in a way a user can notice logs that once
  and carries a `// deviation:` comment saying what a real server does instead.

## The borrow contract

This is the single most important invariant in the codebase.

`RuntimeState` is held behind `Rc<RefCell<_>>` and cloned into every Rust
callback that Luau can call. Calling back into Luau while a borrow of that state
is alive re-enters the same `RefCell` and panics, so:

> **Never call into Luau while a `RuntimeState` (or `World`) borrow is held.**

Everything that mutates follows the same shape:

```rust
// 1. borrow, mutate, collect what the mutation implies
let events = {
    let mut state = ctx.state.borrow_mut();
    state.world.dm.set_parent(child, Some(parent))?   // returns Vec<PendingEvent>
};
// 2. borrow is gone here
// 3. now it is safe to run arbitrary user code
vm::dispatch(lua, ctx, events)?;
```

DataModel mutations therefore *return* `Vec<PendingEvent>` and never fire
listeners inline. Every mutation has exactly one dispatch site, which is what
makes "signals fire synchronously, before the assignment returns" true from
Luau's point of view without any risk of a nested borrow.

The same rule shows up in a subtler place: mlua's `add_meta_method` keeps a
`RefCell` borrow on the userdata for the whole duration of the call, so
`workspace.Part.Parent` (an access that re-enters the same instance) panics with
"already borrowed". Instance metatable methods are therefore registered with
`add_meta_function` and take `AnyUserData`, cloning the handle out immediately.
`instance_of()` is the helper that does this.

## The yield protocol

mlua's non-async API cannot suspend a Rust frame, so **every yielding construct
is written in Luau**, in `crates/microstudio-luau/src/prelude.rs`. Rust never
yields; it only resumes and inspects.

Every piece of user code — scripts, `task.spawn`ed functions, signal listeners —
runs inside an `mlua::Thread` the scheduler owns. When user code yields, it
yields a table the Rust side understands:

```lua
coroutine.yield({ __microstudio = "wait", seconds = 0.5 })
coroutine.yield({ __microstudio = "signal", signal = self })
```

`handle_yield` reads that table, parks the entry (timer queue or signal waiter
list) and records how it should be resumed. When the reason becomes true,
`resume_once` resumes the thread — passing the event arguments of a signal, or
the actually-elapsed simulated time of a wait, as the resume values.

Three consequences worth knowing before editing the VM:

- **Listeners are threads, not `Function::call`.** Calling a listener through
  `Function::call` puts a Rust frame between the coroutine and the yield, which
  fails with "attempt to yield across a C-call boundary". Each listener gets its
  own thread, so `task.wait` inside a handler behaves.
- **`task.spawn` never wraps the user's function in a Rust closure**, for the
  same reason. The thread is created from the user's function directly and the
  arguments are passed as resume values.
- **`Signal:Wait` needs no connection.** It is `coroutine.yield` + `table.unpack`
  of the fired arguments, so nothing has to be registered and later cleaned up.

Signals and connections are plain Luau tables (holding numeric ids in the
registry) rather than userdata, precisely so `:Wait` can be defined in the
prelude with `coroutine.yield`.

## Determinism and the clock

`Clock` is a trait with two implementations: `VirtualClock` (default) and
`RealClock`. The scheduler, timers and `task.wait` all consult it, so the same
script produces the same interleaving under the virtual clock, run after run.
`epoch` is fixed (`1_700_000_000.0`) for the same reason.

A real-clock session needs something to drive the scheduler, because timers only
fire when someone looks. `dev` pumps every 16 ms; `pump()` does the actual work
and never sleeps. `advanceTime` walks timer deadlines in order, with
`DEFAULT_STEP_BUDGET` (500 000) steps so a self-rescheduling loop cannot hang the
process; on a real clock it sleeps instead, because wall time cannot be skipped.

Both session shapes the CLI offers come from that same pair of primitives, so
there is no "live mode" versus "test mode" in the runtime itself:

| Session | Clock | How time moves | What it is for |
| --- | --- | --- | --- |
| `microstudio` (no arguments) | virtual | `:advance <seconds>` | a scratchpad: snippets, no project |
| `microstudio <file>` | virtual | the script's own waits, then done | running a program |
| `dev` / `dev --interactive` | real | the pump, in 16 ms steps | watching a server, poking at it |
| `dev --interactive --clock virtual` | virtual | `:advance <seconds>` | stepping a world, asserting on it |
| `dev --watch`, `microstudio <file> --watch` | either | as above, re-armed on reload | the edit → compile → save loop |
| `test`, `@microstudio/test` | virtual | `advanceTime`, driven by the test | determinism |
| one spec file | virtual | the test drives it; a timeout replaces the runtime | isolated tests that can yield |

A standalone script is not a game loop, so it runs on the virtual clock and
finishes as fast as the CPU allows: `task.wait(0.5)` completes immediately
unless you ask for `--clock real`, where it means half a second of wall time.
Work the script *scheduled* is still part of the program, which is why the entry
chunk completing is followed by `drain()` — `run_until_idle` in the scheduler —
rather than by exiting. Parked signal waiters are not self-resolving work, so
they end the drain instead of blocking it.

The interactive session is a `readline` loop on the **client** side: it sends
`eval` requests down the same JSON-RPC channel the scripts' output comes up, so
the prompt and the game share one process, one DataModel and one scheduler. That
is why `:player` fires the live script's `PlayerAdded` handler, and why a
`task.wait` started by a script completes while the prompt is idle.

## Reloading

`dev --watch` watches the directories the Rojo mounts actually read from
(`mountDirectories`), plus the project file, and re-runs the project when a
`.luau` file changes. Events are debounced and coalesced, because a compiler
rewrites every output file in one burst.

A reload **replaces the sidecar** rather than patching the live one:

```
dispose(runtime) → spawn a new sidecar → loadTree(fresh entries) → run()
```

Patching would be faster but wrong at this stage: re-running a script over an
existing world duplicates the instances it creates, re-connects its listeners to
signals that still hold the old ones, and returns stale values from the
`require` cache. Replacing the process makes the new world indistinguishable
from a fresh start, at the cost of a restart (tens of milliseconds, since the
sidecar is small). The seam is the same one that contains a crash: if the new
world fails to load, the session is still there and the next save reloads again.

## Rojo is the source of truth for paths

`packages/roblox-ts/src/rojo.ts` reads an existing Rojo project file and expands
it into a flat, parent-first list of `{path, className, source, properties}`
entries. The runtime's `loadTree` then materialises exactly those paths. Mount
points are never guessed: a `TS` folder, an `@rbxts` segment and a
`node_modules` tree all come from the project file, and a directory holding
`init.luau` becomes that script rather than a folder, which is what Rojo does.

`globIgnorePaths` and Rojo's built-in ignores (`package.json`, `tsconfig.json`)
are honoured, because compiled output routinely contains them and loading them
as instances would be wrong.

## Testing, and why it is shaped this way

`microstudio test` is not a JavaScript test runner that happens to call into
Luau. The assertions are Luau, and they run in the world:

```
  packages/test/src/luau/framework.luau        ← describe/it/expect/hooks, as [framework]
              ▲ injected as a chunk
  packages/test/src/framework/runner.ts        ← one runtime per spec file
              │
   for each test:  __microstudio_test_run(i)    ← one JSON-RPC round trip per test
```

Three decisions follow from the layering:

- **One request per test.** `__microstudio_test_plan()` enumerates the tests and
  `__microstudio_test_run(index)` runs exactly one, including its hooks. That
  leaves the runner holding the world between two tests, which is what makes
  `--isolate` possible at all: a design that ran the whole file inside Luau could
  not reset the world mid-file without also losing the framework's bookkeeping.
- **The framework's state lives in the Lua registry**, not in the DataModel, for
  the same reason: `reset_world` replaces `RuntimeState` wholesale and leaves the
  registry alone, so the list of tests still to run survives a `reset()`.
- **A timeout replaces the runtime.** Luau cannot be interrupted from outside, so
  a wedged test is detected rather than stopped: the sidecar is disposed, a new
  one is booted, the framework and the spec are reloaded, and the run continues
  with the next test. This is the same "replace, do not patch" trade `dev
  --watch` makes, applied to one process per spec file.

- **A TypeScript spec is compiled by the project's own compiler, then loaded as
  an instance.** Nothing here reimplements roblox-ts: `@microstudio/roblox-ts`
  finds the project's `rbxtsc`, writes a `tsconfig.test.json` that extends the
  project's config and adds the framework's globals, and runs it. The compiled
  spec is loaded through `runScripts` rather than evaluated as a string, because
  roblox-ts's runtime library keys its import cache by `script`.

Failures are values, not crashes: an assertion calls `error`, the framework
catches it with `xpcall` and returns a status. Nothing a test asserts reaches the
runtime's error stream, which is why a failing run does not look like a crashing
one — while an error raised in a thread the test *spawned* is picked up from that
stream and fails the test that spawned it.

`addPlayer` is the one piece of the framework that needed the runtime: it is a
binding over `World::add_mock_player`, the same function behind the REPL's
`:player` and the TypeScript API's `addMockPlayer`.

## The sidecar has to be found

A runtime that needs a Rust binary to run Luau has one packaging problem
(MicroStudio has no other): the TypeScript layer has to locate a process the user
did not install.

`packages/runtime/src/targets.ts` is the table that both halves are derived
from — `scripts/stage-binary.ts` turns a row into a publishable npm package, and
`resolve-binary.ts` turns the same row into a path to look for. The two never
call each other, so the table *is* the contract, and a test fails if
`optionalDependencies` stops covering it.

Resolution itself is a fixed order with one rule behind it: **the more specific
the source, the later it is consulted.** An explicit override beats a build this
checkout made, which beats the package an install provided. That ordering is what
makes a published install and a developer's checkout the same code path:
whatever is present wins, and the error at the end names the one thing the user
can install.

[docs/publishing.md](publishing.md) has the packages, the platform table and how
a release is cut — including the `tsc` pass that turns each code package into
JavaScript on the way to npm, because Node refuses to type-strip anything under
`node_modules`.
