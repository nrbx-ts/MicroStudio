<div align="center" id="top">
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
    <br />
    <br />
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
</div>

<hr />

## @microstudio/runtime

> The TypeScript driver for a local, headless Roblox runtime.

Runs Roblox Luau on your machine — no Roblox Studio, no Roblox client — and drives
it from Node. The instances, the DataModel, the signals, the task scheduler and
every service are real, and a real Luau VM runs the code:

```
your .ts  ──▶  @microstudio/runtime  ──stdio JSON-RPC──▶  microstudio-runtime  ──▶  Luau
```

The sidecar ships prebuilt for linux, macOS and Windows, so there is no Rust
toolchain to install and nothing to compile.

## Installation

```bash
npm install @microstudio/runtime
yarn add @microstudio/runtime
pnpm add @microstudio/runtime
```

The published package lists one binary package per platform as an **optional**
dependency — `@microstudio/runtime-linux-x64`, `@microstudio/runtime-darwin-arm64`,
`@microstudio/runtime-win32-x64` and the rest — so an install keeps only the one
that matches the machine.

Node 22.18 or newer.

## Quick Start

```ts
import { createRuntime } from "@microstudio/runtime";

const runtime = await createRuntime();
try {
  const name = await runtime.eval(`
    local part = Instance.new("Part")
    part.Name = "Baseplate"
    part.Parent = workspace
    return part:GetFullName()
  `);

  console.log(name);                // Workspace.Baseplate
  console.log(runtime.luauVersion); // Luau 0.740
} finally {
  await runtime.dispose();
}
```

`createRuntime` starts the sidecar and shakes hands with it, so a protocol
mismatch fails here rather than inside your first call.

## Usage

### Driving the runtime

| Method | Returns | Does |
| --- | --- | --- |
| `eval(code, options?)` | `unknown` | runs Luau and returns its value; `mode` is `statement`, `expression` or `auto` |
| `loadTree(entries)` | `string[]` | creates `PlaceEntry[]` in the tree, parents first |
| `run()` | `string[]` | runs the server scripts already in the tree |
| `runScript(path)` | `string[]` | runs one script by its DataModel path |
| `runSource(name, source, parent?)` | `string[]` | runs a source string as a new script |
| `tree(path?, depth?)` | `InstanceJson` | the tree, or a subtree |
| `inspect(path)` | `InstanceJson` | one instance, with attributes |
| `advanceTime(seconds)` | `number` | moves the virtual clock, running everything due |
| `pump()` | `number` | runs due work without moving the clock |
| `drain()` | `number` | runs scheduled work to completion |
| `resetWorld()` | `number` | clears instances, signals, threads and the `require` cache |
| `stats()` | `StatsResult` | clock kind, time, pending work, instance and output counts |
| `listServices()` | `string[]` | every service in the API dump, not just the ones in use |
| `logs()` / `errors()` / `clearOutput()` | | what the world printed, and what it raised |
| `onOutput(fn)` / `onError(fn)` | `() => void` | streams output as it arrives; the returned function unsubscribes |
| `dispose()` | `void` | ends the sidecar process |

### Instances

```ts
await runtime.loadTree([
  { path: "ReplicatedStorage.util", className: "ModuleScript", source: "return { n = 1 }" },
  { path: "ServerScriptService.main", className: "Script", source: "print(require(game.ReplicatedStorage.util).n)" },
]);

await runtime.run();                             // runs the scripts, in load order
console.log(await runtime.tree("game.Workspace", 2));
console.log(await runtime.inspect("game.ServerScriptService.main"));
```

Changes made by running code are visible with `tree` and `inspect`, and the
identity of an instance is stable, so `part == workspace.Part` behaves the way it
does on Roblox.

### Time

The clock is virtual by default: `task.wait`, `task.delay` and `TweenService`
advance when you say so, which is what makes a run assertable.

```ts
await runtime.eval(`task.delay(5, function() print("five") end)`);
await runtime.advanceTime(5);      // the print happens here, not five real seconds later
console.log(runtime.logs().map((log) => log.text));
```

Pass `clock: "real"` and the runtime follows wall time instead, pumping the
scheduler on an interval — that is what the `dev` command uses.

```ts
const runtime = await createRuntime({ clock: "real", headless: true });
```

### Players

```ts
const player = await runtime.players.addMockPlayer("Eddie", { withCharacter: true });
console.log(player.name);          // PlayerAdded has already fired for it
```

### Services that keep state

Services with local behaviour hold real state — `DataStoreService`,
`MemoryStoreService`, `MessagingService`, `CollectionService`, `Players`,
`HttpService` (which makes real requests) and the rest. Where that state is kept
is your choice:

```ts
const runtime = await createRuntime({ stateDir: ".microstudio" });
```

Without a `stateDir` the run keeps everything in memory and writes nothing.
[The main README](https://github.com/nrbx-ts/microstudio#environment) lists every
service and where its data lands.

## Where the sidecar comes from

In order:

1. `MICROSTUDIO_RUNTIME_BIN`, for a binary you already have.
2. cargo's output in a source checkout (`target/release`, then `target/debug`,
   honouring `MICROSTUDIO_TARGET_DIR` and `CARGO_TARGET_DIR`).
3. The installed binary package for this platform — the only source a published
   install has.

`resolveRuntimeBinary()` returns the path it picked, and `installedPlatformBinary()`
answers with just the installed package's binary, or `undefined`. When none of them
exists the error names the package to install for your machine.

## Related

| Package | Is |
| --- | --- |
| [`@microstudio/test`](https://www.npmjs.com/package/@microstudio/test) | `luaTest`, `withRuntime` and the runner behind `microstudio test` |
| [`@microstudio/roblox-ts`](https://www.npmjs.com/package/@microstudio/roblox-ts) | Rojo project reading and running your project's own `rbxtsc` |
| [`microstudio`](https://www.npmjs.com/package/microstudio) | the command line: `run`, `dev`, `test`, `repl` |

## License

MIT — see [LICENSE](./LICENSE)

---

Built on [Luau](https://luau.org), [mlua](https://github.com/mlua-rs/mlua) and [roblox-ts](https://roblox-ts.com)

<hr />

<div align="center" id="top">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
    <br />
    <br />
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
</div>
