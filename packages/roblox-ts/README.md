<div align="center" id="top">
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
    <br />
    <br />
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
</div>

<hr />

## @microstudio/roblox-ts

> Reads your existing roblox-ts and Rojo setup, and runs the project's own compiler.

MicroStudio never asks you to change how your project builds. This package reads
the Rojo project file to learn where the compiled Luau lives, flattens that into
the entries a
[runtime](https://www.npmjs.com/package/@microstudio/runtime) can load, and — when
a spec needs compiling — runs the `rbxtsc` **your project already pins**, so the
compiler version is never a second opinion.

```
default.project.json  ──▶  buildPlaceEntries()  ──▶  PlaceEntry[]  ──▶  runtime.loadTree()
        │                                                                    ▲
        └── out/server/*.luau  ────────── your own rbxtsc ────────────────────┘
```

## Installation

```bash
npm install @microstudio/roblox-ts
yarn add @microstudio/roblox-ts
pnpm add @microstudio/roblox-ts
```

It depends on `@microstudio/runtime`, which brings the prebuilt sidecar.

## Quick Start

```ts
import { createRuntime } from "@microstudio/runtime";
import { buildPlaceEntries, findProjectFile } from "@microstudio/roblox-ts";

const projectFile = findProjectFile(process.cwd());
if (projectFile === undefined) {
  throw new Error("no default.project.json here");
}

const runtime = await createRuntime();
try {
  await runtime.loadTree(buildPlaceEntries({ projectFile }));
  await runtime.run();                 // your compiled server scripts, running
} finally {
  await runtime.dispose();
}
```

## Usage

### Reading a project

| Function | Returns | Does |
| --- | --- | --- |
| `findProjectFile(dir)` | `string \| undefined` | `default.project.json`, then any other `*.project.json` |
| `readRojoProject(file)` | `RojoProject` | parses a project file, v7 `tree` or the older top-level form |
| `buildPlaceEntries(options)` | `PlaceEntry[]` | flattens the project into `{ path, className, source }`, parents first |
| `mountDirectories(options)` | `string[]` | every directory the project mounts, for a watcher |
| `rbxtsIncludeFolder(project)` | `string` | where `RuntimeLib` belongs (`rbxts_include`, or `include`) |
| `readText(file)` | `string` | reads a file, dropping a byte-order mark |
| `parseJsonc(text)` | `unknown` | JSON with comments and trailing commas, the way Rojo writes it |

`buildPlaceEntries` takes `{ projectFile, services? }`. `services` defaults to
`DEFAULT_SERVICES` — `ServerScriptService`, `ReplicatedStorage`, `ServerStorage`,
which is the server-side set a headless runtime needs. `ALL_SERVICES` adds
`Workspace`, `Players`, `StarterGui`, `StarterPlayer` and `CollectionService` for
the cases that want them.

### What a project may contain

`$path`, `$className`, `$properties`, `$ignoreUnknownInstances` and
`globIgnorePaths` are read, and script classes come from the file name the way
Rojo derives them:

| File | Becomes |
| --- | --- |
| `foo.luau`, `foo.lua` | `ModuleScript` named `foo` |
| `foo.server.luau` | `Script` named `foo` |
| `foo.client.luau` | `LocalScript` named `foo` |
| `init.luau` | its directory becomes a `ModuleScript` |
| `init.server.luau` | its directory becomes a `Script` |

`classifyScriptFile(name)`, `scriptInstanceName(name)`, `isInitFileName(name)` and
`initFileClass(name)` expose those rules on their own, for a loader of your own.

### Running the project's compiler

Nothing here compiles TypeScript on its own. When something has to be compiled,
these find and drive the project's compiler:

```ts
import {
  compileProject,
  findCompiler,
  readTsConfig,
  rbxtsIncludeFolder,
  readRojoProject,
  writeTestConfig,
} from "@microstudio/roblox-ts";

const compiler = findCompiler(process.cwd());   // walks up to node_modules/roblox-ts
if (compiler === undefined) {
  throw new Error("this project has no roblox-ts installed");
}

const config = readTsConfig("tsconfig.json");
const project = readRojoProject("default.project.json");

// specs get their own config, so the project's tsconfig is left alone:
// it extends yours and adds the framework's type declarations
const testConfig = writeTestConfig({
  config,
  specs: [resolve("src/scoreboard.spec.ts")],
  globals: resolve("node_modules/@microstudio/test/types/globals.d.ts"),
});

const result = compileProject({
  dir: process.cwd(),
  compiler,
  config: testConfig,
  projectFile: project.file,
  includeFolder: rbxtsIncludeFolder(project),
});

if (!result.ok) {
  console.error(result.output);
}
```

`writeTestConfig` writes into `.microstudio/` (the same directory the runtime keeps
service state in) and returns the file it wrote. `findCompiledSpec(outDir, spec)`
maps a `.ts` spec to the `.luau` the compiler produced, and `TEST_CONFIG_NAME` is
that generated config's name.

## Related

| Package | Is |
| --- | --- |
| [`@microstudio/runtime`](https://www.npmjs.com/package/@microstudio/runtime) | the sidecar driver these entries load into |
| [`@microstudio/test`](https://www.npmjs.com/package/@microstudio/test) | the runner that compiles specs with the above |
| [`microstudio`](https://www.npmjs.com/package/microstudio) | the command line: `run`, `dev`, `test`, `repl` |

## License

MIT — see [LICENSE](./LICENSE)

---

Built for [roblox-ts](https://roblox-ts.com) projects that use [Rojo](https://rojo.space)

<hr />

<div align="center" id="top">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
    <br />
    <br />
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
</div>
