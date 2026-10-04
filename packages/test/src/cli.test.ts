import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const CLI = fileURLToPath(new URL("../../cli/src/index.ts", import.meta.url));

interface Run {
  code: number;
  stdout: string;
  stderr: string;
}

function run(args: readonly string[], stdin?: string, env?: NodeJS.ProcessEnv): Promise<Run> {
  return new Promise((resolvePromise, reject) => {
    const child = execFile(
      process.execPath,
      [CLI, ...args],
      { timeout: 30_000, ...(env === undefined ? {} : { env }) },
      (error, stdout, stderr) => {
        // a non-zero exit is a result, not a harness failure
        const code = error === null ? 0 : ((error as { code?: number }).code ?? 1);
        resolvePromise({ code, stdout, stderr });
      },
    );
    child.on("error", reject);
    if (stdin !== undefined) {
      child.stdin?.end(stdin);
    } else {
      child.stdin?.end();
    }
  });
}

function withScript(source: string, body: (path: string) => Promise<void>): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-cli-"));
  const path = join(dir, "main.luau");
  writeFileSync(path, source);
  return body(path).finally(() => rmSync(dir, { recursive: true, force: true }));
}

test("runs a file with no project involved", async () => {
  await withScript(
    [
      "local part = Instance.new('Part')",
      "part.Name = 'Standalone'",
      "part.Parent = workspace",
      "print('created', part:GetFullName(), typeof(part.Position))",
    ].join("\n"),
    async (path) => {
      const result = await run([path]);
      assert.equal(result.stderr, "");
      assert.equal(result.stdout.trim(), "created game.Workspace.Standalone Vector3");
      assert.equal(result.code, 0);
    },
  );
});

test("finishes work the script scheduled before exiting", async () => {
  await withScript(
    [
      "task.spawn(function()",
      "  task.wait(0.25)",
      "  print('background finished')",
      "end)",
      "task.delay(0.5, function() print('delayed finished') end)",
      "print('started')",
    ].join("\n"),
    async (path) => {
      const result = await run([path]);
      assert.equal(
        result.stdout.trim(),
        ["started", "background finished", "delayed finished"].join("\n"),
      );
      assert.equal(result.code, 0);
    },
  );
});

test("reports an error against the script's own name, and exits non-zero", async () => {
  await withScript("local t = nil\nreturn t.Position", async (path) => {
    const result = await run([path]);
    assert.equal(result.code, 1);
    // a windows absolute path must survive the location splitter
    assert.match(result.stdout, /\[server\] .*main\.luau:2/);
    assert.match(result.stdout, /Error: attempt to index nil with 'Position'/);
    // exactly once: the header names the location, the message follows
    assert.equal(result.stdout.match(/attempt to index nil/g)?.length, 1);
  });
});

test("reports a compile error without a stack trace", async () => {
  await withScript("local x = \nreturn x", async (path) => {
    const result = await run([path]);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /main\.luau:2/);
    assert.match(result.stderr, /syntax error/i);
  });
});

test("runs inline code and reads a script from stdin", async () => {
  const inline = await run(["-e", "print('inline', 1 + 1)"]);
  assert.equal(inline.stdout.trim(), "inline 2");
  assert.equal(inline.code, 0);

  const piped = await run(["-"], "print('piped', #workspace:GetChildren())");
  assert.equal(piped.stdout.trim(), "piped 0");
  assert.equal(piped.code, 0);
});

test("a BOM does not stop a script from running", async () => {
  // windows editors write a BOM; Luau's lexer rejects the raw character
  await withScript("\uFEFFprint('bom tolerated')", async (path) => {
    const result = await run([path]);
    assert.equal(result.stdout.trim(), "bom tolerated");
    assert.equal(result.code, 0);
  });
});

test("prompts when given nothing to run", async () => {
  const result = await run([], "print('from the prompt')\n:quit\n");
  assert.match(result.stdout, /MicroStudio 1\.1\.0 — Luau \d+\.\d+ — :help for commands/);
  assert.match(result.stdout, /from the prompt/);
  assert.equal(result.code, 0);
});

test("a missing script is an error, not a crash", async () => {
  const result = await run(["definitely-not-a-script.luau"]);
  assert.equal(result.code, 2);
  assert.match(result.stderr, /Cannot find a script/);
  // --help still works without the sidecar
  const help = await run(["--help"]);
  assert.match(help.stdout, /NO PROJECT NEEDED/);
  assert.equal(help.code, 0);
});

// directory of specs, removed when body returns
function withSpecs(
  files: Record<string, string>,
  body: (dir: string) => Promise<void>,
): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-test-"));
  for (const [name, source] of Object.entries(files)) {
    const path = join(dir, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, source);
  }
  return body(dir).finally(() => rmSync(dir, { recursive: true, force: true }));
}

test("a project keeps its mock data beside its source", async () => {
  await withSpecs(
    {
      "default.project.json": JSON.stringify({
        name: "demo",
        tree: { $className: "DataModel" },
      }),
      "main.luau": [
        "local store = game:GetService('DataStoreService'):GetDataStore('players')",
        "store:SetAsync('alice', { coins = 12 })",
        "print(store:GetAsync('alice').coins)",
      ].join("\n"),
    },
    async (dir) => {
      const result = await run([join(dir, "main.luau")]);
      assert.equal(result.stderr, "");
      assert.equal(result.stdout.trim(), "12");
      assert.equal(result.code, 0);

      const saved = join(dir, ".microstudio", "datastores", "players.json");
      assert.ok(existsSync(saved), `${saved} was not written`);
      assert.deepEqual(JSON.parse(readFileSync(saved, "utf8")), {
        global: { alice: { coins: 12 } },
      });
    },
  );
});

test("--state-dir puts the mock data where it was asked to", async () => {
  await withScript(
    [
      "local store = game:GetService('DataStoreService'):GetDataStore('players')",
      "store:SetAsync('bob', 3)",
      "print('written')",
    ].join("\n"),
    async (path) => {
      const state = join(dirname(path), "elsewhere");
      const result = await run([path, "--state-dir", state]);
      assert.equal(result.stderr, "");
      assert.equal(result.stdout.trim(), "written");
      assert.equal(result.code, 0);
      assert.deepEqual(
        JSON.parse(readFileSync(join(state, "datastores", "players.json"), "utf8")),
        { global: { bob: 3 } },
      );
    },
  );
});

test("a simulated member says so once, on stderr", async () => {
  await withScript(
    [
      "local gui = game:GetService('GuiService')",
      "print(gui:IsTenFootInterface())",
      "print(gui:IsTenFootInterface())",
    ].join("\n"),
    async (path) => {
      const result = await run([path, "--state-dir", join(dirname(path), "state")]);
      assert.equal(result.stdout.trim(), "false\nfalse");
      // one note per member, not per call, so a loop is not a wall of warnings
      assert.equal(result.stderr.trim().split("\n").length, 1);
      assert.match(result.stderr, /GuiService\.IsTenFootInterface is simulated/);    },
  );
});

test("--state-dir ~ means the home directory", async () => {
  await withScript(
    [
      "local store = game:GetService('DataStoreService'):GetDataStore('players')",
      "store:SetAsync('alice', 5)",
      "print('written')",
    ].join("\n"),
    async (path) => {
      const home = mkdtempSync(join(tmpdir(), "microstudio-home-"));
      try {
        const result = await run([path, "--state-dir", "~/.microstudio"], undefined, {
          ...process.env,
          HOME: home,
          USERPROFILE: home,
        });
        assert.equal(result.stdout.trim(), "written");
        assert.deepEqual(
          JSON.parse(readFileSync(join(home, ".microstudio", "datastores", "players.json"), "utf8")),
          { global: { alice: 5 } },
        );
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    },
  );
});

test("a run with no project keeps service data in memory", async () => {
  await withScript(
    [
      "local store = game:GetService('DataStoreService'):GetDataStore('players')",
      "store:SetAsync('alice', 1)",
      "print('value', store:GetAsync('alice'))",
    ].join("\n"),
    async (path) => {
      // a home directory that must not be read from or written to
      const home = mkdtempSync(join(tmpdir(), "microstudio-home-"));
      try {
        const result = await run([path], undefined, {
          ...process.env,
          HOME: home,
          USERPROFILE: home,
        });
        assert.equal(result.code, 0);
        // the value came from memory, and nothing landed in the home directory
        assert.equal(result.stdout.trim(), "value 1");
        assert.equal(existsSync(join(home, ".microstudio")), false);
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    },
  );
});

test("instances are never persisted, only service data is", async () => {
  await withScript(
    [
      "local part = Instance.new('Part')",
      "part.Name = 'Standalone'",
      "part.Parent = workspace",
      "workspace:SetAttribute('built', true)",
      "print(workspace.Standalone.Name)",
    ].join("\n"),
    async (path) => {
      const state = join(dirname(path), "state");
      const result = await run([path, "--state-dir", state]);
      assert.equal(result.stdout.trim(), "Standalone");
      assert.equal(result.code, 0);
      // no service wrote anything, so the state directory was never even created
      assert.equal(existsSync(state), false);
    },
  );
});

test("test runs against the mock data beside the project", async () => {
  await withSpecs(
    {
      "default.project.json": JSON.stringify({
        name: "demo",
        tree: { $className: "DataModel" },
      }),
      ".microstudio/datastores/players.json": JSON.stringify({
        global: { alice: { coins: 12 } },
      }),
      "players.spec.luau": [
        "it('reads the seeded store', function()",
        "  local store = game:GetService('DataStoreService'):GetDataStore('players')",
        "  expect(store:GetAsync('alice').coins).toBe(12)",
        "end)",
      ].join("\n"),
    },
    async (dir) => {
      const result = await run(["test", dir]);
      assert.equal(result.code, 0, `${result.stdout}${result.stderr}`);
      assert.match(result.stdout, /Tests {2}1 passed/);
    },
  );
});

test("test runs spec files and exits zero when they pass", async () => {
  await withSpecs(
    {
      "counter.luau": "return { value = 7 }",
      "counter.spec.luau": [
        'local counter = require(game.ReplicatedStorage.counter)',
        'it("uses an injected module", function()',
        "  expect(counter.value).toBe(7)",
        "  task.wait(0.1)",
        "  expect(counter.value).toBe(7)",
        "end)",
      ].join("\n"),
    },
    async (dir) => {
      const result = await run([
        "test",
        dir,
        "--module",
        `ReplicatedStorage.counter=${join(dir, "counter.luau")}`,
      ]);

      assert.equal(result.code, 0);
      assert.match(result.stdout, /✓ .*counter\.spec\.luau \(1 test,/);
      assert.match(result.stdout, /Tests {2}1 passed \(1\)/);
      assert.match(result.stdout, /Assertions {2}2/);
    },
  );
});

test("test reports a failure, keeps going, and exits non-zero", async () => {
  await withSpecs(
    {
      "first.spec.luau": 'it("fails", function() expect(1).toBe(2) end)',
      "second.spec.luau": [
        'describe("suite", function()',
        '  it("passes", function() expect(true).toBeTruthy() end)',
        '  it.skip("is skipped", function() end)',
        "end)",
      ].join("\n"),
    },
    async (dir) => {
      const result = await run(["test", dir]);

      assert.equal(result.code, 1);
      assert.match(result.stdout, /first\.spec\.luau:1: Expected 1 to be 2/);
      assert.match(result.stdout, /⊘ is skipped/);
      assert.match(result.stdout, /Tests {2}1 passed \| 1 failed \| 1 skipped \(3\)/);
      assert.match(result.stdout, /Test files {2}1 failed \| 1 passed \(2\)/);
    },
  );
});

test("test --json reports the same facts machine-readably", async () => {
  await withSpecs(
    { "json.spec.luau": 'it("fails", function() expect(1).toBe(2) end)' },
    async (dir) => {
      const result = await run(["test", dir, "--json", "--timeout", "2000"]);
      assert.equal(result.code, 1);

      const report = JSON.parse(result.stdout) as {
        summary: { failed: number; ok: boolean; assertions: number };
        files: { tests: { status: string; message?: string }[] }[];
      };
      assert.equal(report.summary.failed, 1);
      assert.equal(report.summary.ok, false);
      assert.equal(report.summary.assertions, 1);
      assert.equal(report.files[0]?.tests[0]?.status, "failed");
      assert.match(String(report.files[0]?.tests[0]?.message), /Expected 1 to be 2/);
    },
  );
});

test("test finds nothing to run rather than passing silently", async () => {
  await withSpecs({ "module.luau": "return {}" }, async (dir) => {
    const result = await run(["test", dir]);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /No spec files found/);
  });
});

test("test --include narrows the run", async () => {
  await withSpecs(
    {
      "one.spec.luau": 'it("one", function() expect(1).toBe(1) end)',
      "two.spec.luau": 'it("two", function() expect(1).toBe(2) end)',
    },
    async (dir) => {
      const result = await run(["test", dir, "--include", "**/one.spec.luau"]);
      assert.equal(result.code, 0);
      assert.match(result.stdout, /one\.spec\.luau/);
      assert.equal(result.stdout.includes("two.spec.luau"), false);
    },
  );
});

// the shape a real roblox-ts project has: a tsconfig, a Rojo file, and the compiler
function tsProject(stub: string): Record<string, string> {
  return {
    "default.project.json": JSON.stringify({
      tree: { ReplicatedStorage: { TS: { $path: "out" } } },
    }),
    "tsconfig.json": JSON.stringify({
      compilerOptions: { outDir: "out", rootDir: "src" },
      include: ["src/**/*"],
    }),
    "src/counter.ts": "export const value = 7;\n",
    "src/counter.spec.ts": 'it("uses the counter", () => {\n\texpect(1).toBe(1);\n});\n',
    "node_modules/roblox-ts/package.json": JSON.stringify({
      name: "roblox-ts",
      version: "0.0.0-stub",
      bin: { rbxtsc: "stub.js" },
    }),
    "node_modules/roblox-ts/stub.js": stub,
  };
}

test("test compiles a TypeScript spec with the project's roblox-ts, then runs it", async () => {
  const luauSpec = [
    'it("adds up", function() expect(1 + 1).toBe(2) end)',
    'it("sees the world", function() expect(game.Workspace.ClassName).toBe("Workspace") end)',
    "",
  ].join("\n");

  await withSpecs(
    tsProject(
      [
        "const fs = require('node:fs');",
        `const spec = ${JSON.stringify(luauSpec)};`,
        "fs.mkdirSync('out', { recursive: true });",
        "fs.writeFileSync('out/counter.spec.luau', spec);",
        "",
      ].join("\n"),
    ),
    async (dir) => {
      const result = await run(["test", dir]);

      assert.equal(result.code, 0);
      // reported under the file the author edits, not the compiled Luau
      assert.match(result.stdout, /src[\\/]counter\.spec\.ts/);
      assert.match(result.stdout, /Tests {2}2 passed \(2\)/);
      // and run once: the emitted Luau is not a spec of its own
      assert.equal(result.stdout.includes("counter.spec.luau"), false);

      // the compile used a generated config, so the project's own was not touched
      assert.equal(existsSync(join(dir, ".microstudio", "tsconfig.test.json")), true);
      const own = readFileSync(join(dir, "tsconfig.json"), "utf8");
      assert.equal(own.includes("globals"), false);
    },
  );
});

test("test explains how to get a compiler when a TypeScript spec needs one", async () => {
  await withSpecs(
    {
      "tsconfig.json": JSON.stringify({
        compilerOptions: { outDir: "out" },
        include: ["src/**/*"],
      }),
      "src/counter.spec.ts": "it(\"x\", () => {});\n",
    },
    async (dir) => {
      const result = await run(["test", dir]);
      assert.equal(result.code, 2);
      assert.match(result.stderr, /npm install --save-dev roblox-ts/);
      assert.match(result.stderr, /--no-compile/);
    },
  );
});

test("--no-compile runs the output a separate build already wrote", async () => {
  await withSpecs(
    {
      ...tsProject("console.error('the compiler should not run');\nprocess.exit(1);\n"),
      "out/counter.spec.luau":
        'it("adds up", function() expect(1 + 1).toBe(2) end)\n',
    },
    async (dir) => {
      const result = await run(["test", dir, "--no-compile"]);
      assert.equal(result.code, 0);
      assert.match(result.stdout, /src[\\/]counter\.spec\.ts/);
      assert.match(result.stdout, /1 passed \(1\)/);
      assert.equal(result.stderr.includes("the compiler should not run"), false);
    },
  );
});

test("a TypeScript spec that is not mounted by Rojo says what to fix", async () => {
  await withSpecs(
    {
      ...tsProject(
        [
          "const fs = require('node:fs');",
          "fs.mkdirSync('out', { recursive: true });",
          "fs.writeFileSync('out/counter.spec.luau', 'it(\"x\", function() end)\\n');",
          "",
        ].join("\n"),
      ),
      // the project does not mount the compiler's output
      "default.project.json": JSON.stringify({ tree: { ReplicatedStorage: {} } }),
    },
    async (dir) => {
      const result = await run(["test", dir]);
      assert.equal(result.code, 2);
      assert.match(result.stderr, /not in the project's Rojo tree/);
      assert.match(result.stderr, /\$path/);
    },
  );
});
