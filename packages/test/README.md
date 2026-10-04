<div align="center" id="top">
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
    <br />
    <br />
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
</div>

<hr />

## @microstudio/test

> Test helpers for MicroStudio: run Luau against a real world, from `node:test` or vitest.

Two ways in, and both run against real instances in a real DataModel rather than a
mock of one:

- **Luau specs**, run by the [`microstudio`](https://www.npmjs.com/package/microstudio)
  command — `describe`/`it`/`expect`, hooks, per-test timeouts, one world per spec
  file. The framework those specs use lives in this package.
- **From TypeScript**, when the test itself is JavaScript: `luaTest` runs a Luau
  snippet as one test, `runSuite` runs a whole spec from a string, and
  `withRuntime` drives a runtime by hand.

In a roblox-ts project the specs can be `*.spec.ts`: the CLI compiles them with the
project's own `rbxtsc` first, then runs and reports them under the `.ts` path.

## Installation

```bash
npm install --save-dev @microstudio/test
yarn add --dev @microstudio/test
pnpm add -D @microstudio/test
```

It depends on [`@microstudio/runtime`](https://www.npmjs.com/package/@microstudio/runtime),
so the prebuilt sidecar comes with it. Node 22.18 or newer.

## Quick Start

```bash
npx microstudio test
```

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
		expect(function() board.score("Nobody", 1) end).toThrow("unknown player")
	end)

	it.todo("resets at the end of a round")
end)
```

```
✓ src/scoreboard.spec.luau (3 tests, 16ms)
    ✓ notices a player joining 1ms
    ✓ refuses an unknown player 0ms
    ☐ resets at the end of a round

 Test files  1 passed (1)
      Tests  2 passed | 1 todo (3)
 Assertions  3
   Duration  21ms
```

## Usage

### From `node:test` or vitest

```ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { asNumber, luaTest, runSuite, withRuntime } from "@microstudio/test";

// a Luau snippet as one test; a failed assertion throws, which is what the
// runner reports
test("a part lands in the workspace", () =>
  luaTest("part parented", `
    local part = Instance.new("Part")
    part.Parent = workspace
    expect(workspace.Part).toBe(part)
  `));

// a whole spec from a string, with the results in your hands
test("the suite passes", async () => {
  const result = await runSuite({
    file: "sum.spec.luau",
    source: `
      it("adds", function() expect(1 + 1).toBe(2) end)
    `,
  });
  assert.equal(result.loadError, undefined);
  assert.deepEqual(result.tests.map((t) => t.status), ["passed"]);
});

// or drive the runtime yourself
test("task.wait moves the virtual clock", () =>
  withRuntime(async (runtime) => {
    await runtime.eval("task.wait(2)");
    const stats = await runtime.stats();
    assert.equal(asNumber(stats.time), 2);
  }));
```

| Export | Does |
| --- | --- |
| `luaTest(name, body, options?)` | runs the snippet as one `it`, throwing on failure |
| `runSuite(options)` | runs a spec from `source`, a `script` path, `place` entries or `modules` |
| `runSpecFiles(entries, options?)` | runs several spec files, reporting each as it finishes |
| `withRuntime(body, options?)` | hands you a runtime and disposes it even when `body` throws |
| `createTestRuntime(options?)` | `createRuntime` with the virtual clock, for a runtime you manage |
| `asNumber` / `asString` / `asArray` | read an `eval` result, throwing when it is the wrong shape |
| `moduleEntries({ "ReplicatedStorage.util": "return {}" })` | turns sources into `PlaceEntry[]`, for tests with no Rojo project |

`options` carries the runtime options (`clock`, `headless`, `stateDir`) plus
`isolate` for a fresh world per test and `timeoutMs` for the watchdog
(`DEFAULT_TIMEOUT_MS`, 5000). A test that overruns throws `TestTimeoutError`.

### The runner behind the CLI

```ts
import {
  discoverSpecs,
  formatSummary,
  formatSuite,
  renderJsonReport,
  runSpecFiles,
  summarize,
} from "@microstudio/test";

const specs = discoverSpecs(process.cwd());          // *.spec.luau and friends
const results = await runSpecFiles(specs, {
  onResult: (result) => console.log(`${result.name}: ${result.status}`),
});

const summary = summarize(results);
for (const result of results) {
  console.log(formatSuite(result).join("\n"));
}
console.log(formatSummary(summary).join("\n"));
console.log(renderJsonReport(results, summary));      // what `--json` prints
```

| Export | Does |
| --- | --- |
| `discoverSpecs(root, { include?, exclude? })` | finds spec files; `DEFAULT_INCLUDE` is `**/*.spec.luau`, `*.spec.lua`, `*.test.luau`, `*.test.lua`, `DEFAULT_TS_INCLUDE` adds the TypeScript forms |
| `runSpecFiles(entries, options?)` | runs them in order, with `onSuite` and `onResult` callbacks |
| `summarize(results)` | totals: files, tests, assertions, failures, todo, duration |
| `formatSuite` / `formatSummary` / `formatTest` | the CLI's output, as lines |
| `failedResults(results)` | just the failures, for a custom report |
| `renderJsonReport(results, summary)` | the JSON report |
| `globToRegExp` / `matchesPattern` | the pattern matching the above uses |

### The spec DSL

| In a spec | Is |
| --- | --- |
| `describe(name, fn)`, `describe.skip`, `describe.only` | a suite, nested as deep as you like |
| `it(name, fn)`, `it.skip`, `it.only`, `it.todo` | a test |
| `beforeAll` / `afterAll` / `beforeEach` / `afterEach` | hooks, scoped to their suite |
| `expect(value, message?)` | matchers, each negatable with `.never` |
| `reset()` | empties the world, keeping the DataModel |
| `addPlayer(name)` | fires `PlayerAdded` the way a real join does |

Matchers: `toBe`, `toEqual`, `toBeCloseTo`, `toBeTruthy`, `toBeFalsy`, `toBeNil`,
`toBeType`, `toBeA`, `toHaveLength`, `toContain`, `toMatch`, `toThrow`,
`toHaveProperty`, `toBeGreaterThan`, `toBeGreaterThanOrEqual`, `toBeLessThan`,
`toBeLessThanOrEqual`.

Each spec file gets its own runtime, so one file cannot leak instances, state or a
cached `require` into another. Inside a file every test shares the world unless
`--isolate` asks for one per test.

## Related

| Package | Is |
| --- | --- |
| [`@microstudio/runtime`](https://www.npmjs.com/package/@microstudio/runtime) | the runtime these helpers drive |
| [`@microstudio/roblox-ts`](https://www.npmjs.com/package/@microstudio/roblox-ts) | compiling `*.spec.ts` with the project's compiler |
| [`microstudio`](https://www.npmjs.com/package/microstudio) | the command line that runs all of this |

## License

MIT — see [LICENSE](./LICENSE)

---

Built for [roblox-ts](https://roblox-ts.com) projects, on [Luau](https://luau.org)

<hr />

<div align="center" id="top">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20support%20NN140.UK-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04&link=https%3A%2F%2Fdonate.stripe.com%2F9B6eVdbTd4n1a6H1yXa3u04" alt="Badge">
    <img src="https://img.shields.io/badge/Stripe-Donate%20to%20Support%20NN140.UK%20(RECURRING)-1b1b1b?style=for-the-badge&labelColor=6860ff&logo=stripe&logoColor=ffffff&logoSize=auto&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05&link=https%3A%2F%2Fdonate.stripe.com%2FdRm9ATe1laLpgv5b9xa3u05" alt="Badge">
    <br />
    <br />
    <img src="https://r2.nrbx.nn140.uk/img/NRBX-Banner.png" alt="NRBX logo" width="1000"/>
</div>
