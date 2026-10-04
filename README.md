<div align="center" id="top">
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
    <br />
    <br />
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
</div>

<hr />

## MicroStudio

> A local, headless Roblox development runtime.

MicroStudio runs Roblox Luau on your machine, without Roblox Studio and without
the Roblox client. It works two ways:

- **As an interpreter**, like `node` or `python` — run a Luau file, or start a
  prompt, with `Instance.new`, `game`, `workspace`, `Players` and `task` already
  there. No project, no configuration.
- **As your project's runtime** — point it at a roblox-ts + Rojo project and it
  loads the compiled Luau into a live DataModel and runs the server scripts.
- **As a test runner** — `microstudio test` finds your specs, gives each file
  its own world, and reports failures with the file, the line and the call chain.
  Specs are Luau, or TypeScript in a roblox-ts project: those are compiled by the
  project's own `rbxtsc` first.

```
src/*.ts  --rbxtsc-->  out/*.luau  --Rojo-->  DataModel  --MicroStudio-->  your code, running
```

Nothing is emulated, stubbed or mocked away: a real Luau VM executes the real
compiled output, and the runtime is deterministic enough to assert on.

## Requirements

An installed MicroStudio needs Node, and nothing else. Building this repository
needs Rust as well:

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | 1.88+ | only to build the sidecar; an npm install ships a prebuilt one |
| Node.js | 22.18+ | Node strips TypeScript natively, so there is no build step |
| roblox-ts | any | only needed to compile *your* project |
| Rojo | any | only the project file format is read; the Rojo binary is not required |
| Roblox Studio | none | that is the point |

## Quickstart

Using MicroStudio needs nothing but Node — [the install below](#installing-it-with-no-toolchain).
Working on MicroStudio itself needs Rust and Node, and nothing else:

```bash
git clone <this repo> && cd MicroStudio
corepack enable                        # once: makes `yarn` the pinned release
cargo build --bin microstudio-runtime   # or: yarn build:runtime (release)
yarn install
```

### Installing it, with no toolchain

The command is on npm under two names, and the runtime it drives comes with it: a
prebuilt sidecar for linux (x64, arm64), macOS (Intel, Apple silicon) and Windows
(x64, arm64). Nothing is compiled — no Rust, no MSVC, no build step.

```bash
npm install -g microstudio          # or: npm install -g @microstudio/cli
npx microstudio --version           # or: npx @microstudio/cli --version
```

A platform with no prebuilt sidecar installs the rest of the tool and names the
package that is missing, rather than failing in the middle of a run.

The examples below run it from a checkout, as `node packages/cli/src/index.ts`.
`microstudio` in its place is the same command.

### As an interpreter, with no project

Ninety percent of "why does this behave like that" questions are answered by
running three lines of Luau. No Rojo file is needed:

```bash
node packages/cli/src/index.ts -e "
  local part = Instance.new('Part')
  part.Name = 'Probe'
  part.Parent = workspace
  print(part:GetFullName(), typeof(part.Position))
"
```

```text
game.Workspace.Probe Vector3
```

```bash
node packages/cli/src/index.ts script.luau     # a file
node packages/cli/src/index.ts -               # a script on stdin
node packages/cli/src/index.ts                 # a prompt, on a fresh world
```

```text
MicroStudio 0.1.0 — Luau 0.740 — :help for commands
> workspace
Instance<Workspace> Workspace
> Instance.new("Part").Size
4, 1, 2
> CFrame.Angles(0, math.pi / 2, 0)
0, 0, 0, 0, 0, 1, 0, 1, 0, -1, 0, 0
> game:GetService("CollectionService"):GetTags(workspace)
[]
```

A script is treated as a program: when its entry chunk returns, so does
MicroStudio — after the work the script *scheduled* finishes.

```bash
node packages/cli/src/index.ts -e "
  task.spawn(function() task.wait(0.25) print('background finished') end)
  print('started')
"
```

```text
started
background finished
```

Errors read like a normal interpreter's — naming the file as you typed it, with
a non-zero exit code:

```bash
node packages/cli/src/index.ts script.luau
```

```text
before the failure
[server] script.luau:3
Error: attempt to index nil with 'Position'
Stack:
  stack traceback:
  script.luau:3: in function <script.luau:1>
```

Add `--interactive` to run the file and *stay* in the world it created, or
`--watch` to re-run on save (a Luau scratchpad):

```bash
node packages/cli/src/index.ts script.luau --interactive
node packages/cli/src/index.ts script.luau --watch
node packages/cli/src/index.ts script.luau --interactive --watch
```

### Against a project

Run the bundled example — a project with a Rojo file, a server script and a
shared module:

```bash
node packages/cli/src/index.ts dev examples/hello --once
```

```text
MicroStudio dev — .../examples/hello/default.project.json
loaded 6 instances
hello, MicroStudio
running on the server: true
tagged a checkpoint: Checkpoint1
checkpoint position: 0, 0, 0
tagged instances: 1
util.sum: 6
[server] ServerScriptService.TS.game
```

`dev` uses a real clock, so `task.wait(0.5)` really waits. `dev --once` boots the
project, drives it for `--timeout` seconds (0.1 by default) and exits non-zero if
anything errored — this is the "does my project still boot" check:

```bash
node packages/cli/src/index.ts dev examples/hello --once --timeout 3
```

```text
MicroStudio dev — .../examples/hello/default.project.json
loaded 6 instances
hello, MicroStudio
running on the server: true
tagged a checkpoint: Checkpoint1
checkpoint position: 0, 0, 0
tagged instances: 1
util.sum: 6
[server] ServerScriptService.TS.game
```

For assertions rather than a boot check, `test` runs your Luau specs against the
project: see [Writing tests](#writing-tests) and [docs/testing.md](docs/testing.md).

`repl` inspects the DataModel in the context of a loaded project. A bare
expression echoes its value:

```bash
node packages/cli/src/index.ts repl examples/hello
```

```text
loaded .../default.project.json
MicroStudio repl — :help for commands
> workspace
Instance<Workspace> Workspace
> :player Eddie
PlayerAdded: Eddie
> game:GetService("ReplicatedStorage").TS.util
Instance<ModuleScript> util
> :tree ReplicatedStorage
ReplicatedStorage (ReplicatedStorage)
  TS (Folder)
    util (ModuleScript)
```

`:help`, `:stats`, `:tree <path>`, `:player <name>`, `:advance <seconds>` and
`:quit` are handled by the REPL; everything else is evaluated as Luau.

`repl` inspects the *loaded* tree without running the project's scripts. For a
live game, use the next section.

## A live game you can prompt into

`dev` runs until you stop it. Add `--interactive` and it also reads Luau from
stdin, against the same running world the scripts are using:

```bash
node packages/cli/src/index.ts dev examples/hello --interactive
```

```text
...startup, exactly as above...
MicroStudio live — real clock, 1 server script — :help for commands
> task.wait(0.6)
half a second later
spawned task resumed after 0.25s
> game:GetService("CollectionService"):GetTagged("checkpoint")[1].Name
Checkpoint1
> game.Workspace.Checkpoints:GetChildren()[1].ClassName
Part
> :player Eddie
welcome, Eddie
PlayerAdded: Eddie
> :quit
stopped at 0.756s — 0 error(s)
```

Note what happened on its own: the hello project's `task.wait(0.5)` and its
spawned task both completed while the prompt was sitting idle. The session is
not paused — scripts keep running, signals keep firing, and typing at the prompt
observes and changes that same world. `:player` fired `PlayerAdded` into the
live script (`welcome, Eddie`) *and* reported it in the console.

Time is wall-clock by default, so `task.wait` really waits. Pass
`--clock virtual` for a world that only moves when you ask it to:

```bash
node packages/cli/src/index.ts dev examples/hello --interactive --clock virtual
```

```text
MicroStudio live — virtual clock, 1 server script — :help for commands
> :advance 0.6
half a second later
time = 0.600
stopped at 0.600s — 0 error(s)
```

This is the difference between watching a server and stepping one: nothing runs
until `:advance`, and a test can assert on the result. Both modes share the same
runtime, so a script behaves identically under either.

## Reload on save

`--watch` restarts the session when the project changes — any `.lua`/`.luau`
file under a Rojo mount, or the project file itself:

```bash
node packages/cli/src/index.ts dev examples/hello --watch
```

```text
running — Ctrl-C to stop
watching the project's mounts — reload on save
half a second later
spawned task resumed after 0.28s
↻ reload — examples\hello\out\shared\util.luau
loaded 6 instances
hello from the reload, MicroStudio
running on the server: true
[server] ServerScriptService.TS.game
```

Combine it with `--interactive` for the full loop — edit, recompile with
`rbxtsc`, save, and both the game and your prompt come back on the new code.

Two decisions worth knowing:

- **A reload is a restart, not a patch.** MicroStudio replaces the sidecar
  rather than splicing changed instances into a live DataModel, so there is no
  duplicated instance, no stale listener and no warm `require` cache. Everything
  the scripts create is built again from scratch; mock players and anything you
  poked in by hand are gone. Per-file patching is a deliberate later step.
- **Errors do not end the session.** A syntax error is reported with its script,
  line and stack trace, and the watcher keeps running:

  ```text
  ↻ reload — examples\hello\out\shared\util.luau
  loaded 6 instances
  [server] ServerScriptService.TS.game
  Error: syntax error: ReplicatedStorage.TS.util:21: Expected identifier when parsing expression, got <eof>
  Stack:
    stack traceback:
    [C]: in function '__microstudio_require'
    MicroStudio/Prelude:174: in function 'require'
    ServerScriptService.TS.game:13: in function <ServerScriptService.TS.game:1>
  ```

  Saving a fix reloads again and the run recovers. Writes are debounced (120 ms)
  and coalesced, so a compiler rewriting its whole output directory reloads
  once.

## Using it in your own project

MicroStudio needs a Rojo project file, because that is what says where compiled
Luau lives. A typical roblox-ts `default.project.json` is enough:

```json
{
  "name": "my-game",
  "tree": {
    "$className": "DataModel",
    "ServerScriptService": { "TS": { "$path": "out/server" } },
    "ReplicatedStorage": { "TS": { "$path": "out/shared" } }
  }
}
```

Compile with roblox-ts as usual, then point MicroStudio at the project:

```bash
npx rbxtsc
npx microstudio dev . --project default.project.json   # or a checkout: node packages/cli/src/index.ts
```

Note the nested `TS` folder — MicroStudio reads it from the project file rather
than guessing, which is why non-trivial layouts work.

## Writing tests

There are two ways in, and both run Luau against a real world.

**Luau specs**, run by `microstudio test` — the vitest-like path:

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
		expect(board.entries.Eddie).toBe(0)
	end)

	it("refuses an unknown player", function()
		expect(function()
			board:score("Nobody", 1)
		end).toThrow("no entry for Nobody")
	end)
end)
```

```bash
node packages/cli/src/index.ts test examples/testing
```

```text
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

Specs are `*.spec.luau` / `*.test.luau` (and the `.lua` spellings), found by
directory walk. Each file gets its own runtime; `--isolate` rebuilds the world
before every test; `--timeout` (ms) fails a test that never finishes and
continues the run; `--json` reports the whole thing machine-readably;
`--watch` re-runs on save. `docs/testing.md` has the matchers, the hooks and the
reasoning.

**TypeScript specs**, when the code under test is roblox-ts: write
`src/scoreboard.spec.ts` next to the module and `microstudio test` compiles it
with the project's own `rbxtsc` before running it, then reports it under the
`.ts` path you edited:

```ts
import { Scoreboard } from "./scoreboard";

describe("Scoreboard", () => {
	it("counts a goal", () => {
		const board = new Scoreboard();
		board.score("Eddie", 1);
		expect(board.entries.get("Eddie")).toBe(1);
	});
});
```

The globals type themselves, so no project config is needed:
`microstudio test` writes a `tsconfig.test.json` into `.microstudio/` that
extends yours and adds the framework's declarations. `--no-compile` skips the
compile (for a project that builds in its own script), and `--tsconfig <file>`
names a config other than `tsconfig.json`. `docs/testing.md` covers what a
compiled spec can and cannot do.

**From TypeScript**, when the test itself is a TypeScript test: `luaTest` runs a
Luau snippet as one test, `runSuite` runs a whole spec from a string, and
`withRuntime` drives a runtime by hand. These come from `@microstudio/test`, which
brings the sidecar with it:

```bash
npm install --save-dev @microstudio/test     # or: microstudio, for the cli as well
```

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { luaTest, withRuntime } from "@microstudio/test";

test("checkpoints are tagged", async () => {
  await withRuntime(async (runtime) => {
    const seen = await runtime.eval(`
      local cs = game:GetService("CollectionService")
      local seen = {}
      cs:GetInstanceAddedSignal("cp"):Connect(function(part) table.insert(seen, part.Name) end)
      local part = Instance.new("Part") part.Name = "A" part.Parent = workspace
      cs:AddTag(part, "cp")
      return seen
    `);
    assert.deepEqual(seen, ["A"]);
  });
});

test("the counter starts at zero", async () => {
  await luaTest("starts at zero", "expect(Counter.new().value).toBe(0)", {
    modules: { "ReplicatedStorage.Counter": counterSource },
  });
});
```

Time is virtual by default, so `runtime.advanceTime(3600)` runs an hour of
scheduled work instantly and deterministically. `runtime.players.addMockPlayer`
fires `Players.PlayerAdded` before it resolves; in Luau, `addPlayer()` does the
same. `runtime.logs()` and `runtime.errors()` hold everything the run printed or
raised.

## Repository layout

```
.cargo/                  windows builds link the c++ runtime statically
crates/
  microstudio-types/      Roblox datatypes (Vector3, CFrame, Color3, ...)
  microstudio-datamodel/  Instance tree, signals, attributes, class table
  microstudio-services/   World, the state store, Players, RunService, CollectionService
  microstudio-luau/       Lua VM, bindings, scheduler, script loader
  microstudio-runtime/    JSON-RPC sidecar (the `microstudio-runtime` binary)
packages/
  runtime/               typed driver for the sidecar
  roblox-ts/             Rojo reading and roblox-ts output mapping
  cli/                   `dev`, `test`, `repl`
  test/                  the test framework, runtime helpers, acceptance tests
scripts/                 staging the publishable packages, the manifest rules,
                         and the dependency check a shipped binary has to pass
.github/workflows/       `test` on every push, `release` from a tag
examples/hello/          a runnable project
examples/testing/        a project with a spec, for `microstudio test`
docs/architecture.md     layering, the borrow contract, the yield protocol
docs/services.md         every simulated service, its data files, and its tier
docs/testing.md          the test framework: DSL, CLI, and how it works
docs/publishing.md       the release: binary packages and the publish-time compile step
docs/compatibility.md    what is faithful to Roblox, and every known deviation
```

## Environment

| Variable | Effect |
| --- | --- |
| `MICROSTUDIO_RUNTIME_BIN` | Use this sidecar binary. Must exist, or the driver fails loudly. |
| `MICROSTUDIO_TARGET_DIR` | Cargo target directory to search for the binary. |
| `CARGO_TARGET_DIR` | Same, for redirected cargo output. |

The driver looks for the sidecar in this order: the override above, then a source
checkout's cargo output (`<repo>/target/{release,debug}/`, and the `deps/`
spellings below those), then the **prebuilt binary package** for this platform.
A published install has no `target/`, so it uses the last of those and never
needs Rust; a machine with both uses its own build.

Binary packages are published per platform — linux x64 and arm64, macOS Intel and
Apple silicon, and Windows x64 and arm64 (`@microstudio/runtime-linux-x64`,
`@microstudio/runtime-win32-x64`, …). The *published* `@microstudio/runtime`
lists all six as optional dependencies, so an install keeps only the one that
matches its machine. Each is static against its platform's c++ runtime, so an
install never has to fetch a redistributable either, and the release reads every
binary's dependency table before it publishes it.
[docs/publishing.md](docs/publishing.md) covers the mechanism; `yarn
stage:binary stage --target <name>` builds one locally.

## Development

Dependencies are installed with [Yarn 4](https://yarnpkg.com), the release
`packageManager` in `package.json` names, run through corepack:

```bash
corepack enable      # once, so that `yarn` is the pinned release
yarn install
```

If `yarn --version` reports something other than what `packageManager` names, a
Yarn of your own is earlier on `PATH`; `corepack yarn` always uses the pinned
one. Node 25 and newer no longer ship corepack, so those installs need
`npm i -g corepack` first.

```bash
yarn check           # typecheck + cargo test + node --test
yarn test:rust       # 176 tests across the five crates
yarn test:ts         # 135 tests, including the CLI and the spec runner as child processes
yarn typecheck       # tsc --noEmit, strict
```

## Releasing

Two workflows in [.github/workflows/](.github/workflows): `test` on every push and
pull request — Rust on linux, the TypeScript layer on linux, and an `install` job
on Windows and macOS that packs the packages, `npm install`s them and runs the cli
out of `node_modules` — and `release` from a `vX.Y.Z` tag, which re-runs the tests,
waits for a human to approve the `release` environment, builds the six platform
packages (each one checked for libraries an install cannot provide, and run), and
publishes all eleven with `npm publish --provenance`.

Publishing authenticates with the `NPM_PUBLISH` repository secret. A trusted
publisher on npmjs.com takes precedence over that secret, so moving to OIDC is
incremental and worth doing before January 2027, when npm stops accepting a direct
publish from a 2FA-bypass token.
[docs/publishing.md](docs/publishing.md) has the rest, and the comment at the top
of `release.yml` lists the setup steps.

<hr />

<div align="center" id="top">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
    <br />
    <br />
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
</div>