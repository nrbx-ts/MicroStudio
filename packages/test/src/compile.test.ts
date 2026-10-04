import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

import {
  compileProject,
  findCompiler,
  findCompiledSpec,
  parseJsonc,
  readTsConfig,
  testConfigFile,
  writeTestConfig,
} from "@microstudio/roblox-ts";

function makeProject(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-compile-"));
  for (const [relative, contents] of Object.entries(files)) {
    const full = join(dir, relative);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, contents);
  }
  return dir;
}

// stands in for the real compiler: records its arguments and writes the output
const STUB = [
  "const fs = require('node:fs');",
  "const path = require('node:path');",
  "fs.writeFileSync('stub-argv.json', JSON.stringify(process.argv.slice(2)));",
  "if (process.env.STUB_FAIL === '1') {",
  "  console.error('stub: compile failed');",
  "  process.exit(1);",
  "}",
  "fs.mkdirSync('out', { recursive: true });",
  "fs.writeFileSync(path.join('out', 'counter.spec.luau'), 'return true\\n');",
  "",
].join("\n");

function withProject(body: (dir: string) => void | Promise<void>): Promise<void> {
  const dir = makeProject({
    "tsconfig.json": JSON.stringify({
      compilerOptions: { outDir: "out", rootDir: "src" },
      include: ["src/**/*"],
    }),
    "default.project.json": JSON.stringify({ tree: {} }),
    "src/counter.spec.ts": "describe('counter', () => {});\n",
    "node_modules/roblox-ts/package.json": JSON.stringify({
      name: "roblox-ts",
      version: "0.0.0-stub",
      bin: { rbxtsc: "stub.js" },
    }),
    "node_modules/roblox-ts/stub.js": STUB,
  });

  return Promise.resolve(body(dir)).finally(() => {
    rmSync(dir, { recursive: true, force: true });
  });
}

test("json with comments and trailing commas reads like JSON", () => {
  const parsed = parseJsonc(
    [
      "{",
      "  // the best config",
      '  "include": ["src/**/*",], /* trailing comma above */',
      '  "url": "https://example.com/*",',
      '  "empty": {},',
      "}",
    ].join("\n"),
  ) as Record<string, unknown>;

  assert.deepEqual(parsed["include"], ["src/**/*"]);
  // a comment marker inside a string is not a comment
  assert.equal(parsed["url"], "https://example.com/*");
  assert.deepEqual(parsed["empty"], {});
});

test("a tsconfig reports its output, sources and include patterns", () => {
  withProject((dir) => {
    const config = readTsConfig(join(dir, "tsconfig.json"));
    assert.equal(config.dir, dir);
    assert.equal(config.outDir, join(dir, "out"));
    assert.equal(config.rootDir, join(dir, "src"));
    assert.deepEqual(config.include, [join(dir, "src", "**", "*")]);
  });
});

test("a tsconfig without output or include falls back to what roblox-ts uses", () => {
  const dir = makeProject({ "tsconfig.json": "{ /* nothing useful */ }" });
  try {
    const config = readTsConfig(join(dir, "tsconfig.json"));
    assert.equal(config.outDir, join(dir, "out"));
    assert.equal(config.rootDir, undefined);
    assert.deepEqual(config.include, [join(dir, "**", "*")]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the test config extends the project and adds the globals and the specs", () => {
  withProject((dir) => {
    const config = readTsConfig(join(dir, "tsconfig.json"));
    const globals = join(dir, "globals.d.ts");
    const file = writeTestConfig({
      config,
      specs: [join(dir, "src", "counter.spec.ts")],
      globals,
    });

    assert.equal(file, testConfigFile(config));
    assert.equal(file, join(dir, ".microstudio", "tsconfig.test.json"));

    const written = JSON.parse(readFileSync(file, "utf8")) as {
      extends: string;
      compilerOptions: { typeRoots: string[] };
      include: string[];
    };
    assert.equal(written.extends, config.file.replaceAll("\\", "/"));
    assert.equal(written.include.includes(globals.replaceAll("\\", "/")), true);
    assert.equal(
      written.include.includes(join(dir, "src", "counter.spec.ts").replaceAll("\\", "/")),
      true,
    );
    // roblox-ts demands a root below the config, so the real one is named too
    assert.equal(
      written.compilerOptions.typeRoots.includes(
        join(dir, "node_modules", "@rbxts").replaceAll("\\", "/"),
      ),
      true,
    );
  });
});

test("the project's compiler is found by walking up the tree", () => {
  withProject((dir) => {
    const compiler = findCompiler(join(dir, "src"));
    assert.equal(compiler?.entry, join(dir, "node_modules", "roblox-ts", "stub.js"));
    assert.equal(compiler?.version, "0.0.0-stub");
    assert.equal(compiler?.root, dir);
  });
});

test("a project without roblox-ts has no compiler", () => {
  const dir = makeProject({ "src/counter.spec.ts": "" });
  try {
    assert.equal(findCompiler(join(dir, "src")), undefined);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the compiler is run with the generated config, the rojo file and the include folder", async () => {
  await withProject((dir) => {
    const compiler = findCompiler(dir);
    assert.notEqual(compiler, undefined);

    const result = compileProject({
      dir,
      compiler: compiler!,
      config: join(dir, ".microstudio", "tsconfig.test.json"),
      projectFile: join(dir, "default.project.json"),
      includeFolder: join(dir, "include"),
    });

    assert.equal(result.ok, true);
    const argv = JSON.parse(readFileSync(join(dir, "stub-argv.json"), "utf8")) as string[];
    assert.deepEqual(argv, [
      "-p",
      join(dir, ".microstudio", "tsconfig.test.json"),
      "--rojo",
      join(dir, "default.project.json"),
      "--includePath",
      join(dir, "include"),
    ]);
  });
});

test("a failing compile reports what the compiler said", async () => {
  await withProject((dir) => {
    const previous = process.env["STUB_FAIL"];
    process.env["STUB_FAIL"] = "1";
    try {
      const result = compileProject({
        dir,
        compiler: findCompiler(dir)!,
        config: join(dir, "tsconfig.test.json"),
        projectFile: join(dir, "default.project.json"),
        includeFolder: join(dir, "include"),
      });
      assert.equal(result.ok, false);
      assert.match(result.output, /stub: compile failed/);
    } finally {
      if (previous === undefined) {
        delete process.env["STUB_FAIL"];
      } else {
        process.env["STUB_FAIL"] = previous;
      }
    }
  });
});

test("a compiled spec is found by name, in either extension", () => {
  const dir = makeProject({
    "out/counter.spec.luau": "return true\n",
    "out/other.test.lua": "return true\n",
  });
  try {
    assert.equal(
      findCompiledSpec(join(dir, "out"), join(dir, "src", "counter.spec.ts")),
      join(dir, "out", "counter.spec.luau"),
    );
    assert.equal(
      findCompiledSpec(join(dir, "out"), join(dir, "src", "other.test.ts")),
      join(dir, "out", "other.test.lua"),
    );
    assert.equal(
      findCompiledSpec(join(dir, "out"), join(dir, "src", "absent.spec.ts")),
      undefined,
    );
    assert.equal(
      findCompiledSpec(join(dir, "missing"), join(dir, "src", "a.spec.ts")),
      undefined,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("two specs of the same name are told apart by their folders", () => {
  const dir = makeProject({
    "out/a/board.spec.luau": "return true\n",
    "out/b/board.spec.luau": "return true\n",
  });
  try {
    assert.equal(
      findCompiledSpec(join(dir, "out"), join(dir, "a", "board.spec.ts")),
      join(dir, "out", "a", "board.spec.luau"),
    );
    assert.equal(
      findCompiledSpec(join(dir, "out"), join(dir, "b", "board.spec.ts")),
      join(dir, "out", "b", "board.spec.luau"),
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
