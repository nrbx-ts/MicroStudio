# Testing

MicroStudio ships a test framework for Luau. It is shaped like vitest or jest,
with one difference that matters: **the assertions are written in Luau, and they
run inside the same world as the code under test.** A roblox-ts project can write
them in TypeScript instead — see [roblox-ts](#roblox-ts) — and they are compiled
by the project's own compiler before they run.

```lua
-- src/scoreboard.spec.luau
local Scoreboard = require(game.ReplicatedStorage.scoreboard)

describe("Scoreboard", function()
	local board

	beforeEach(function()
		board = Scoreboard.new():track()
	end)

	it("notices a player joining", function()
		addPlayer("Eddie")
		expect(board.joined).toBe(1)
	end)

	it("refuses an unknown player", function()
		expect(function()
			board:score("Nobody", 1)
		end).toThrow("no entry for Nobody")
	end)
end)
```

```
$ microstudio test examples/testing
✓ examples\testing\src\scoreboard.spec.luau (6 tests, 74ms)
    ✓ starts empty 1ms
    ✓ notices a player joining 0ms
    ✓ scores a known player 0ms
    ✓ refuses an unknown player 0ms
    ✓ keeps tests in separate worlds 0ms
    ☐ resets at the end of a round

 Test files  1 passed (1)
      Tests  5 passed | 1 todo (6)
 Assertions  9
   Duration  77ms
```

`examples/testing` is a complete, runnable project: a Rojo file, a module, and
that spec.

## Why it is not a JavaScript test runner calling into Luau

A `test()` that wraps each assertion in an RPC would have to give up on the
things that make a test worth writing:

- **`it` can yield.** `task.wait(1)`, `part.ChildAdded:Wait()`, `Players.PlayerAdded:Wait()`
  all work inside a test, because a test body is just a chunk on the same
  scheduler as the game.
- **A test sees the real world.** `require`, services, signals, datatypes and the
  scheduler are the runtime's own, not a mock. `expect(part).toBeA("BasePart")`
  is a real `IsA` on a real instance.
- **A failure is a value, not a crash.** Assertions fail with `error`, which the
  framework catches with `xpcall`, so the world, the other tests and the run
  survive it.
- **Time is virtual by default.** `task.wait(0.5)` costs microseconds, and the
  same spec produces the same result every time.

## The DSL

Everything is a global, injected before the spec loads.

| | |
| --- | --- |
| `describe(name, fn)` / `describe.skip` / `describe.only` | a suite, nestable |
| `it(name, fn)` / `test(name, fn)` | a test |
| `it.skip(name, fn)` | reported as skipped, body never runs |
| `it.only(name, fn)` | only the marked tests run |
| `it.todo(name)` | reported as a todo, no body |
| `beforeAll` / `afterAll` / `beforeEach` / `afterEach` | hooks for the enclosing suite |
| `expect(value[, message])` | the matcher chain |
| `reset()` | replace the world with an empty one |
| `addPlayer([name])` | make a mock player join, firing `PlayerAdded` |

`beforeEach` runs inside the test's own world, so a fixture is built before each
test it serves. A setup hook that fails fails the test **without running its
body**, which keeps a broken fixture from looking like a broken feature.

### Matchers

```lua
expect(1 + 1).toBe(2)                       -- identity
expect({ a = 1 }).toEqual({ a = 1 })        -- structural, reports the first key that differs
expect(0.1 + 0.2).toBeCloseTo(0.3)          -- 2 decimal places by default
expect(true).toBeTruthy()                   -- also toBeFalsy, toBeNil
expect(part).toBeA("BasePart")              -- IsA, so inherited classes count
expect(#list).toBeType("number")            -- typeof
expect(list).toHaveLength(3)                -- also toContain, toHaveProperty
expect("concatenated").toMatch("cat")       -- Lua pattern, or a plain substring
expect(f).toThrow("nope")                   -- calls f, expects it to error
expect(3).toBeGreaterThan(2)                -- also ...OrEqual, and toBeLessThan[OrEqual]
```

Negation is spelled `never`, with `isNot` as an alias:

```lua
expect(2).never.toBe(3)
expect(nil).isNot.toBeTruthy()
```

`expect(x).not.toBe(y)` does not compile: `not` is a Lua keyword, and a keyword
cannot follow a `.`.

A second argument to `expect` names the expectation, which a CI log will thank
you for:

```lua
expect(#players, "the mock player must have joined").toBe(1)
-- spec.luau:4: the mock player must have joined: Expected 0 to be 1
```

### Failures

A failure names the spec, the line, and what was compared. A call chain is added
when there is one worth showing, and anything the test printed comes last:

```
✗ src\wallet.spec.luau (3 tests, 16ms)
    ✗ rejects a negative amount 0ms
      src\wallet.spec.luau:10: Expected 90 to be 70
    ✗ spends through a helper 0ms
      src\wallet.spec.luau:15: balance is short
      at src\wallet.spec.luau:15 function spend
      at src\wallet.spec.luau:18 function spendAll
      at src\wallet.spec.luau:20
    ✗ prints while it works 0ms
      src\wallet.spec.luau:25: Expected 1 to be 2
      checking the ledger

 Test files  1 failed (1)
      Tests  3 failed (3)
 Assertions  2
   Duration  20ms
```

The framework's own frames are filtered out, so every line is a line you wrote.

### Isolation

Each spec file gets its own runtime and its own world, so one file cannot leak
instances, listeners or `require` cache entries into another.

Inside a file, the world persists between tests unless you say otherwise — which
is the fastest thing to run, and the reason `beforeEach` builds fixtures into it.
Three ways to get a clean world:

```lua
it("starts from nothing", function()
	reset()                          -- this test: throw the world away
	expect(#workspace:GetChildren()).toBe(0)
end)
```

```bash
microstudio test --isolate          # every test: rebuild the world from the project
microstudio test --watch            # re-run the whole suite when a .luau file changes
```

`--isolate` resets the world *and* re-materialises the project's instances before
each test, so each test starts from a freshly loaded place rather than an empty
one. `beforeAll` still runs once per suite: put world-building in `beforeEach`.

## The CLI

```
microstudio test [dir] [--project <file>] [--include <glob>] [--exclude <glob>]
                [--module path=file] [--isolate] [--timeout <ms>] [--json]
                [--watch] [--tsconfig <file>] [--no-compile] [--state-dir <path>]
```

| Flag | Meaning |
| --- | --- |
| `[dir]` | where to look for specs, and the directory a Rojo project is searched from. Defaults to the working directory. |
| `--include` / `--exclude` | comma-separated globs, replacing the defaults (`**/*.spec.luau`, `**/*.spec.lua`, `**/*.test.luau`, `**/*.test.lua`, and the `.ts`/`.tsx` spellings of those) |
| `--module` | inject a `ModuleScript` from a file: `--module ReplicatedStorage.Counter=src/counter.luau`. Repeatable by comma. This is how a box of Luau with no Rojo project gets something to `require`. |
| `--project` | the Rojo project to read, when it is not found automatically |
| `--isolate` | reset the world before every test |
| `--timeout` | per-test milliseconds; `5000` by default, `0` to disable |
| `--json` | the whole run as JSON: `{ summary, files }` |
| `--watch` | re-run everything when a `.lua`/`.luau`/`.ts` file in the tree changes, recompiling TypeScript specs |
| `--tsconfig` | the config a TypeScript spec is compiled with; `tsconfig.json` by default |
| `--state-dir` | where simulated services keep their JSON; a project defaults to `<project>/.microstudio`, and a run with no project keeps it in memory |
| `--no-compile` | do not run the project's `rbxtsc`: run the output of a build you ran yourself |

A spec shares that state directory with the project, so a test can seed a data
store, a badge or an asset and then assert on it: see
[services.md](./services.md).

The exit code is `0` when every test passed (or skipped or is a todo) and `1`
otherwise, so it drops straight into CI.

**The project's scripts are not run before the specs.** A unit test wants the
place, not a running game; the instances are loaded, so `require` and the tree
are real, but nothing has executed yet. If what you want is "does the project
boot", that is `microstudio dev --once`.

Specs are found by directory walk, and a spec inside a Rojo mount is just as good
as one outside it: a `.luau` file becomes a `ModuleScript`, which `dev` never
runs.

Two folders are skipped by the walk: the project's `outDir`, where a compiled
TypeScript spec comes from (it is run from there, so it is not also discovered as
a spec of its own), and the vendored `rbxts_include` folder.

## From TypeScript

Two APIs, depending on which language the test is really in.

### `luaTest` — one snippet, inside a `node:test` or vitest test

```ts
import { test } from "node:test";
import { luaTest } from "@microstudio/test";

test("the counter starts at zero", async () => {
  await luaTest("starts at zero", "expect(Counter.new().value).toBe(0)", {
    modules: { "ReplicatedStorage.Counter": counterSource },
  });
});
```

The snippet is the *body* of the test, so it can `expect`, `task.wait` and call
anything the world provides. A failure throws, which is how the surrounding
runner reports it.

### `runSuite` — a whole spec, from a string

```ts
import { runSuite, summarize, formatSummary, formatSuite } from "@microstudio/test";

const result = await runSuite({
  source: specSource,
  file: "counter.spec.luau",
  place: placeEntries,            // optional: instances to materialise first
  modules: { "ReplicatedStorage.Counter": counterSource },
  isolate: false,
  timeoutMs: 5000,
});

for (const line of formatSuite(result)) console.log(line);
console.log(formatSummary(summarize([result]), { color: false }).join("\n"));
```

| Option | Meaning |
| --- | --- |
| `source` | the spec, as Luau |
| `file` | how the spec is named in failures; defaults to `[string]` |
| `place` | `PlaceEntry[]` materialised before the spec loads |
| `modules` | `ModuleScript`s created from source, keyed by path (`ReplicatedStorage.Counter` or `ReplicatedStorage/Counter`) |
| `isolate` / `timeoutMs` | as the CLI flags |
| `stateDir` | where simulated services keep their JSON, for a spec that seeds its own |
| `runtimeFactory` | how to boot the runtime, for a caller that configures the sidecar |
| `onResult` | called as each test finishes, for streaming progress |

`runSpecFiles(files, options)` runs a list of files the same way, and
`summarize` / `formatSuite` / `formatSummary` / `renderJsonReport` turn the
results into the CLI's output.

### roblox-ts

A spec in TypeScript is an ordinary module, so it lives next to the code it
tests:

```ts
// src/scoreboard.spec.ts
import { Scoreboard } from "./scoreboard";

describe("Scoreboard", () => {
	it("counts a goal", () => {
		const board = new Scoreboard();
		board.score("Eddie", 1);
		expect(board.entries.get("Eddie")).toBe(1);
	});
});
```

```
$ microstudio test
✓ src\scoreboard.spec.ts (1 passed, 31ms)
    ✓ counts a goal 1ms
```

`microstudio test` compiles it with the project's own compiler — roblox-ts is
never replaced or bundled — and then runs the result:

1. the project's `tsconfig.json` is read;
2. `.microstudio/tsconfig.test.json` is written, extending it and adding
   [`packages/test/types/globals.d.ts`](../packages/test/types/globals.d.ts) to
   `include`, so the DSL typechecks without the project configuring anything;
3. `rbxtsc` runs with that config, the project file and the `rbxts_include`
   folder, writing Luau into the project's own `outDir`;
4. each compiled spec is run from its instance in the loaded project.

`.microstudio/` is where MicroStudio keeps its local state, so a project usually
ignores it already (this repository does). The project's own config is left
alone.

Three things are worth knowing:

- **A failure points at the compiled line.** roblox-ts 3 emits no source maps,
  so `out\scoreboard.spec.luau:6` is where it happened; the suite is named after
  the `.ts` file you edited, and the traceback names the instance.
- **`never`, not `not`.** Luau cannot have a `.not` field — `not` is a keyword —
  so the negated chain is `never`, with `isNot` as an alias, in TypeScript too.
- **`script` is bound while the spec loads.** `rbxtsc`'s runtime library keys its
  import cache by `script`, so the spec is run as its own instance. Test bodies
  run afterwards as a separate chunk, where `script` is nil: import at the top of
  the spec, which is what the compiler emits anyway.

`--no-compile` runs what a separate build already wrote, and `--tsconfig <file>`
reads a config other than `tsconfig.json`.

The compile uses the project's own settings, so a spec has to live inside the
project's `rootDir` to be compiled at all — the same rule as any other module in
the project.

## How it works

Worth knowing when something behaves oddly.

- **The framework is Luau**, in
  [`packages/test/src/luau/framework.luau`](../packages/test/src/luau/framework.luau).
  The runner injects it as a chunk named `[framework]` and loads the spec as
  another chunk named after the file, which is why failure locations read like
  paths.
- **A compiled spec is loaded from its instance.** `runSuite` takes a `source`
  string or a `script` path: a TypeScript spec passes the latter, so the runtime
  runs the instance `rbxtsc`'s output is mounted as, and `script` resolves. The
  compiler's output directory and the vendored `rbxts_include` folder are left
  out of spec discovery, or the Luau a spec compiled to would be discovered as a
  spec of its own.
- **The runner drives one test per request.** `__microstudio_test_plan()` lists
  the tests; `__microstudio_test_run(index)` runs exactly one, including its
  hooks. Between two calls the runner can reset the world, which a
  run-everything-inside-Luau design could not do.
- **Bookkeeping lives in the Lua registry**, not the world, so `reset()` cannot
  lose the list of tests still to run.
- **A timeout replaces the runtime.** Luau cannot be interrupted from outside, so
  a test that never finishes is *detected*, not stopped: the runtime is disposed,
  a new one is booted, the framework and spec are reloaded, and the run
  continues with the next test. Hooks re-run from scratch after that, which is
  the honest trade.
- **A background failure is a test failure.** An error raised in a thread the
  test spawned (or a connection it made) is picked up from the runtime's error
  stream and fails the test that caused it, because nothing else would notice.
- **`print` is captured, not lost.** Output is attached to the test that produced
  it and shown under a failure. A failing assertion itself never reaches the
  error stream, so a failing run does not look like a crashing one.
