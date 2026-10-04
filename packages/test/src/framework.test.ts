import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  DEFAULT_TS_INCLUDE,
  discoverSpecs,
  formatSummary,
  luaTest,
  matchesPattern,
  runSuite,
  summarize,
  type RunSuiteOptions,
} from "./index.ts";
import { buildPlaceEntries, DEFAULT_SERVICES } from "@microstudio/roblox-ts";

// fails if the spec did not load
async function suiteOf(
  source: string,
  options: Omit<RunSuiteOptions, "source" | "file"> = {},
) {
  const result = await runSuite({ ...options, source, file: "spec.luau" });
  assert.equal(result.loadError, undefined, `spec failed to load: ${result.loadError}`);
  return result.tests;
}

function statuses(tests: { status: string }[]): string[] {
  return tests.map((entry) => entry.status);
}

function names(tests: { name: string; status: string }[]): string[] {
  return tests.map((entry) => `${entry.name}:${entry.status}`);
}

test("runs a passing test and counts assertions", async () => {
  const tests = await suiteOf(`
    it("adds", function()
      expect(1 + 1).toBe(2)
      expect("a" .. "b").toBe("ab")
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
  assert.equal(tests[0]?.assertions, 2);
});

test("reports a failure against the spec's own line", async () => {
  const tests = await suiteOf(
    [
      'it("fails", function()', // line 1
      "  local value = 41", // line 2
      "  expect(value + 1).toBe(43)", // line 3
      "end)",
    ].join("\n"),
  );

  assert.deepEqual(statuses(tests), ["failed"]);
  assert.match(String(tests[0]?.message), /^spec\.luau:3: Expected 42 to be 43$/);
});

test("names the failing test the way a nested suite reads", async () => {
  const tests = await suiteOf(`
    describe("Counter", function()
      describe("increment", function()
        it("wraps at the maximum", function()
          expect(1).toBe(2)
        end)
      end)
    end)
  `);

  assert.equal(tests[0]?.name, "Counter > increment > wraps at the maximum");
  assert.equal(tests[0]?.depth, 2);
});

test("supports skip, todo and only", async () => {
  const tests = await suiteOf(`
    it("runs", function() expect(true).toBeTruthy() end)
    it.skip("is skipped", function() error("never runs") end)
    it.todo("is a todo")
    describe.skip("skipped suite", function()
      it("is skipped too", function() error("never runs") end)
    end)
  `);
  assert.deepEqual(names(tests), [
    "runs:passed",
    "is skipped:skipped",
    "is a todo:todo",
    "skipped suite > is skipped too:skipped",
  ]);
});

test("only narrows the run to the marked tests", async () => {
  const tests = await suiteOf(`
    it("is left out", function() expect(false).toBeTruthy() end)
    it.only("is left in", function() expect(true).toBeTruthy() end)
  `);

  assert.deepEqual(names(tests), ["is left out:skipped", "is left in:passed"]);
});

test("runs hooks in the order a fixture needs", async () => {
  const tests = await suiteOf(`
    local order = {}
    describe("suite", function()
      beforeAll(function() table.insert(order, "beforeAll") end)
      afterAll(function()
        table.insert(order, "afterAll")
        assert(table.concat(order, ",") == "beforeAll,beforeEach,test,afterEach,beforeEach,test,afterEach,afterAll")
      end)
      beforeEach(function() table.insert(order, "beforeEach") end)
      afterEach(function() table.insert(order, "afterEach") end)

      it("first", function() table.insert(order, "test") end)
      it("second", function() table.insert(order, "test") end)
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed", "passed"]);
  // a failing afterAll would fail the second test
  assert.equal(tests[1]?.status, "passed");
});

test("a failing beforeEach fails the test without running its body", async () => {
  const tests = await suiteOf(`
    describe("suite", function()
      beforeEach(function()
        expect("fixture").toBe("built")
      end)
      it("never runs", function()
        expect(true).toBe(false)
      end)
    end)
  `);

  assert.deepEqual(statuses(tests), ["failed"]);
  assert.match(String(tests[0]?.message), /^beforeEach failed: spec\.luau:4: Expected "fixture" to be "built"$/);
  // the body's assertion never ran, so the count is the hook's
  assert.equal(tests[0]?.assertions, 1);
});

test("a throwing test keeps its location and call chain", async () => {
  const tests = await suiteOf(
    [
      'local function inner() error("boom") end', // 1
      "local function outer() inner() end", // 2
      'it("throws", function() outer() end)', // 3
    ].join("\n"),
  );

  assert.deepEqual(statuses(tests), ["failed"]);
  assert.match(String(tests[0]?.message), /^spec\.luau:1: boom$/);
  assert.match(String(tests[0]?.traceback), /spec\.luau:1 function inner/);
  assert.match(String(tests[0]?.traceback), /spec\.luau:2 function outer/);
});

test("a suite that cannot be described becomes a failing test", async () => {
  const tests = await suiteOf(`
    describe("broken", function()
      local nothing = nil
      nothing.field = 1
    end)

    it("still runs", function() expect(true).toBeTruthy() end)
  `);

  assert.deepEqual(names(tests), ["broken:failed", "still runs:passed"]);
  assert.match(String(tests[0]?.message), /attempt to index nil/);
});

test("reports a spec that does not compile as a load failure", async () => {
  const result = await runSuite({ source: "it('unclosed', function()", file: "broken.spec.luau" });
  assert.equal(result.tests.length, 0);
  assert.match(
    String(result.loadError),
    /syntax error: broken\.spec\.luau:\d+: Expected 'end' \(to close 'function' at line 1\)/,
  );
});

test("a spec that throws while loading is a load failure, not a crash", async () => {
  const result = await runSuite({ source: 'error("cannot even load")', file: "boom.spec.luau" });
  assert.equal(result.tests.length, 0);
  assert.match(String(result.loadError), /boom\.spec\.luau:1: cannot even load/);
});

test("failures do not leak into the runtime's error list", async () => {
  const result = await runSuite({
    source: 'it("fails", function() expect(1).toBe(2) end)',
    file: "spec.luau",
  });
  assert.equal(result.tests[0]?.status, "failed");
  // a failing assertion is a result, not a script error
  assert.equal(result.logs.length, 0);
});

test("an error in a thread the test spawned fails it", async () => {
  const tests = await suiteOf(`
    it("spawns a failure", function()
      task.spawn(function() error("background boom") end)
      task.wait(0.1)
      expect(true).toBeTruthy()
    end)
  `);

  assert.deepEqual(statuses(tests), ["failed"]);
  assert.match(String(tests[0]?.message), /background boom/);
  // the assertion the test did make still counts
  assert.equal(tests[0]?.assertions, 1);
});

test("captures what a test printed", async () => {
  const tests = await suiteOf(`
    it("prints", function()
      print("hello from a test")
      expect(1).toBe(2)
    end)
  `);

  assert.deepEqual(tests[0]?.logs, ["hello from a test"]);
});

test("addPlayer makes a player join and fires PlayerAdded", async () => {
  const tests = await suiteOf(`
    it("joins", function()
      local joined = nil
      game:GetService("Players").PlayerAdded:Connect(function(player)
        joined = player.Name
      end)

      local player = addPlayer("Eddie")

      expect(player).toBeA("Player")
      expect(joined).toBe("Eddie")
      expect(#game:GetService("Players"):GetPlayers()).toBe(1)
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
});

test("reset() takes the mock players with it", async () => {
  const tests = await suiteOf(`
    it("clears players", function()
      addPlayer("Eddie")
      expect(#game:GetService("Players"):GetPlayers()).toBe(1)
      reset()
      expect(#game:GetService("Players"):GetPlayers()).toBe(0)
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
});

test("a test can await a signal", async () => {
  const tests = await suiteOf(`
    it("awaits", function()
      local part = Instance.new("Part")
      task.delay(0.5, function()
        local child = Instance.new("Part")
        child.Parent = part
      end)
      local child = part.ChildAdded:Wait()
      expect(child).toBeA("BasePart")
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
});

test("reset() gives the next test an empty world", async () => {
  const tests = await suiteOf(`
    it("dirties the world", function()
      local part = Instance.new("Part")
      part.Parent = workspace
      expect(#workspace:GetChildren()).toBe(1)
      reset()
      expect(#workspace:GetChildren()).toBe(0)
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
});

test("the world survives between tests unless reset", async () => {
  const tests = await suiteOf(`
    it("creates", function()
      local part = Instance.new("Part")
      part.Parent = workspace
    end)
    it("sees it", function()
      expect(#workspace:GetChildren()).toBe(1)
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed", "passed"]);
});

test("isolate rebuilds the world before every test", async () => {
  const result = await runSuite({
    source: `
      it("creates", function()
        local part = Instance.new("Part")
        part.Parent = workspace
        expect(#workspace:GetChildren()).toBe(1)
      end)
      it("does not see it", function()
        expect(#workspace:GetChildren()).toBe(0)
      end)
    `,
    file: "isolate.spec.luau",
    isolate: true,
  });

  assert.deepEqual(statuses(result.tests), ["passed", "passed"]);
});

test("isolate reloads the modules, and clears the require cache with the world", async () => {
  const result = await runSuite({
    source: `
      it("first", function()
        local Counter = require(game.ReplicatedStorage.Counter)
        Counter.calls = (Counter.calls or 0) + 1
        expect(Counter.calls).toBe(1)
      end)
      it("second", function()
        local Counter = require(game.ReplicatedStorage.Counter)
        Counter.calls = (Counter.calls or 0) + 1
        expect(Counter.calls).toBe(1)
        expect(game.ReplicatedStorage:FindFirstChild("Counter")).toBeA("ModuleScript")
      end)
    `,
    file: "modules.spec.luau",
    isolate: true,
    modules: {
      "ReplicatedStorage.Counter": "return {}",
    },
  });

  assert.deepEqual(statuses(result.tests), ["passed", "passed"]);
});

test("modules can be injected from source without a project", async () => {
  const result = await runSuite({
    source: `
      local Counter = require(game.ReplicatedStorage.Counter)
      it("uses the module", function()
        expect(Counter.new(2).value).toBe(2)
      end)
    `,
    file: "modules.spec.luau",
    modules: {
      "ReplicatedStorage/Counter": `
        local Counter = {}
        function Counter.new(value) return { value = value } end
        return Counter
      `,
    },
  });

  assert.equal(result.loadError, undefined);
  assert.deepEqual(statuses(result.tests), ["passed"]);
});

test("a test that never finishes is failed, and the run continues", async () => {
  const result = await runSuite({
    source: `
      it("hangs", function() while true do end end)
      it("still runs", function() expect(1).toBe(2) end)
    `,
    file: "hang.spec.luau",
    timeoutMs: 400,
  });

  assert.deepEqual(statuses(result.tests), ["failed", "failed"]);
  assert.match(String(result.tests[0]?.message), /did not finish within 400ms/);
  // not a timeout: the assertion really was wrong after the runtime was replaced
  assert.match(String(result.tests[1]?.message), /Expected 1 to be 2/);
});

test("a timeout of zero disables the watchdog", async () => {
  const result = await runSuite({
    source: 'it("fast", function() expect(1).toBe(1) end)',
    file: "spec.luau",
    timeoutMs: 0,
  });
  assert.deepEqual(statuses(result.tests), ["passed"]);
});

test("matchers cover the shapes a Roblox test needs", async () => {
  const tests = await suiteOf(`
    it("matches", function()
      expect({ a = { b = 1, c = { 1, 2 } } }).toEqual({ a = { b = 1, c = { 1, 2 } } })
      expect(2).never.toBe(3)
      expect(nil).isNot.toBeTruthy()
      expect(0.1 + 0.2).toBeCloseTo(0.3)
      expect("concatenated").toMatch("cat")
      expect({ 1, 2, 3 }).toContain(2)
      expect({ 1, 2, 3 }).toHaveLength(3)
      expect("text").toBeType("string")
      expect(Instance.new("Part")).toBeA("BasePart")
      expect(function() error("nope") end).toThrow("nope")
      expect({ nested = { deep = true } }).toHaveProperty("nested")
      expect(3).toBeGreaterThan(2)
      expect(2).toBeLessThanOrEqual(2)
      expect(Instance.new("Part").Name).toBe("Part")
    end)
  `);

  assert.deepEqual(statuses(tests), ["passed"]);
});

test("a failing matcher says what it expected", async () => {
  const tests = await suiteOf(`
    it("reports the key that differs", function()
      expect({ a = { b = 1 } }).toEqual({ a = { b = 2 } })
    end)
  `);

  assert.match(String(tests[0]?.message), /Expected \{ a = \{ b = 1 \} \} to equal \{ a = \{ b = 2 \} \} \(a\.b is 1, expected 2\)/);
});

test("expect's optional message is carried into the failure", async () => {
  const tests = await suiteOf(`
    it("explains itself", function()
      expect(1, "the seed must be one").toBe(2)
    end)
  `);

  assert.match(String(tests[0]?.message), /the seed must be one: Expected 1 to be 2/);
});

test("summarize and formatSummary agree about a failing run", async () => {
  const results = [
    await runSuite({
      source: `
        it("passes", function() expect(1).toBe(1) end)
        it("fails", function() expect(1).toBe(2) end)
        it.skip("skips", function() end)
      `,
      file: "sum.spec.luau",
    }),
  ];
  const summary = summarize(results, { durationMs: 10 });

  assert.deepEqual(
    {
      files: summary.files,
      tests: summary.tests,
      passed: summary.passed,
      failed: summary.failed,
      skipped: summary.skipped,
      assertions: summary.assertions,
      ok: summary.ok,
      durationMs: summary.durationMs,
    },
    {
      files: 1,
      tests: 3,
      passed: 1,
      failed: 1,
      skipped: 1,
      assertions: 2,
      ok: false,
      durationMs: 10,
    },
  );

  const lines = formatSummary(summary, { color: false });
  assert.match(lines.join("\n"), /Tests {2}1 passed \| 1 failed \| 1 skipped \(3\)/);
  assert.match(lines.join("\n"), /Duration {2}10ms/);
});

test("summarize says so when a file could not be loaded", async () => {
  const results = [await runSuite({ source: "it(", file: "broken.spec.luau" })];
  const summary = summarize(results);
  assert.equal(summary.loadFailures, 1);
  assert.equal(summary.failedFiles, 1);
  assert.equal(summary.ok, false);
  assert.match(
    formatSummary(summary, { color: false }).join("\n"),
    /Test files {2}1 failed \(1\)/,
  );
  assert.match(formatSummary(summary, { color: false }).join("\n"), /no tests found/);
});

test("luaTest runs a snippet from a string", async () => {
  const result = await luaTest("adds two numbers", "expect(1 + 1).toBe(2)");
  assert.equal(result.status, "passed");
  assert.equal(result.assertions, 1);
});

test("luaTest gives a snippet the world it asks for", async () => {
  await luaTest(
    "requires an injected module",
    `
      local Counter = require(game.ReplicatedStorage.Counter)
      local counter = Counter.new(41)
      counter:increment()
      expect(counter.value).toBe(42)
    `,
    {
      modules: {
        "ReplicatedStorage.Counter": `
          local Counter = {}
          Counter.__index = Counter
          function Counter.new(value)
            return setmetatable({ value = value }, Counter)
          end
          function Counter:increment(by)
            self.value += by or 1
          end
          return Counter
        `,
      },
    },
  );
});

test("luaTest throws the failure so a runner can report it", async () => {
  await assert.rejects(
    () => luaTest("fails on purpose", "expect(1).toBe(2)"),
    /fails on purpose[\s\S]*Expected 1 to be 2/,
  );
});

test("luaTest reports a snippet that does not compile", async () => {
  await assert.rejects(() => luaTest("bad", "expect("), /syntax error/);
});

test("spec discovery finds the four spec names and skips noise", async () => {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-specs-"));
  const nested = join(dir, "src", "deep");
  const modules = join(dir, "node_modules", "pkg");
  mkdirSync(nested, { recursive: true });
  mkdirSync(modules, { recursive: true });

  for (const name of ["a.spec.luau", "b.spec.lua", "c.test.luau", "d.test.lua"]) {
    writeFileSync(join(nested, name), "");
  }
  writeFileSync(join(nested, "module.luau"), "");
  writeFileSync(join(modules, "e.spec.luau"), "");

  try {
    const found = discoverSpecs(dir).map((file) => file.slice(dir.length + 1).replaceAll("\\", "/"));
    assert.deepEqual(found, [
      "src/deep/a.spec.luau",
      "src/deep/b.spec.lua",
      "src/deep/c.test.luau",
      "src/deep/d.test.lua",
    ]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("glob patterns cover the forms a --include needs", () => {
  assert.equal(matchesPattern("a.spec.luau", "**/*.spec.luau"), true);
  assert.equal(matchesPattern("src/deep/a.spec.luau", "**/*.spec.luau"), true);
  assert.equal(matchesPattern("src/a.luau", "**/*.spec.luau"), false);
  assert.equal(matchesPattern("src/a.spec.luau", "src/*.spec.{luau,lua}"), true);
  assert.equal(matchesPattern("src/a.spec.lua", "src/*.spec.{luau,lua}"), true);
  assert.equal(matchesPattern("src/a.spec.txt", "src/*.spec.{luau,lua}"), false);
  assert.equal(matchesPattern("src/a.spec.luau", "**/src/**"), true);
  assert.equal(matchesPattern("other/src/a.spec.luau", "**/src/**"), true);
  assert.equal(matchesPattern("src/deep/a.spec.luau", "src/*.spec.luau"), false);
  assert.equal(matchesPattern("src/deep/a.spec.luau", "src/**/*.luau"), true);
});

test("spec discovery finds TypeScript specs, but never a declaration", () => {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-ts-specs-"));
  const src = join(dir, "src");
  mkdirSync(src, { recursive: true });
  for (const name of ["a.spec.ts", "b.test.tsx", "c.spec.d.ts", "d.d.ts"]) {
    writeFileSync(join(src, name), "");
  }

  try {
    const found = discoverSpecs(dir, { include: DEFAULT_TS_INCLUDE }).map((file) =>
      file.slice(dir.length + 1).replaceAll("\\", "/"),
    );
    assert.deepEqual(found, ["src/a.spec.ts", "src/b.test.tsx"]);
    // the Luau defaults stay Luau: a .ts spec has to be compiled first
    assert.deepEqual(discoverSpecs(dir), []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a compiled spec runs from its instance, so script resolves while it loads", async () => {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-instance-"));
  const out = join(dir, "out");
  mkdirSync(out, { recursive: true });
  writeFileSync(
    join(dir, "default.project.json"),
    JSON.stringify({ tree: { ReplicatedStorage: { TS: { $path: "out" } } } }),
  );
  writeFileSync(
    join(out, "helper.luau"),
    [
      "local helper = {}",
      "function helper.double(n) return n * 2 end",
      "function helper.owner() return script.Name end",
      "return helper",
      "",
    ].join("\n"),
  );
  // a name that is one path segment: the framework names the spec's instance after it
  const spec = [
    // load time: `script` is the spec, so a sibling resolves
    "local helper = require(script.Parent.helper)",
    'it("sees its own instance while it loads", function()',
    // bodies run later, as their own chunk, so `script` is not bound there
    "  expect(script).toBeNil()",
    "  expect(helper.double(2)).toBe(4)",
    "end)",
    "",
  ].join("\n");
  writeFileSync(join(out, "board.luau"), spec);

  try {
    const entries = buildPlaceEntries({
      projectFile: join(dir, "default.project.json"),
      services: ["ReplicatedStorage"],
    });
    const entry = entries.find((candidate) => candidate.path.endsWith(".board"));
    assert.equal(entry?.className, "ModuleScript");

    // loaded as a string there is no instance, so `script` is nil and the require fails
    const asChunk = await runSuite({
      source: spec,
      place: entries,
      file: "board.luau",
    });
    assert.notEqual(asChunk.loadError, undefined);

    const result = await runSuite({
      script: entry?.path ?? "",
      place: entries,
      file: join("out", "board.luau"),
    });
    assert.equal(result.loadError, undefined);
    assert.deepEqual(statuses(result.tests), ["passed"]);
    assert.equal(result.tests[0]?.assertions, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
