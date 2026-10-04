import assert from "node:assert/strict";
import { exec, execFile } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  PLATFORM_PACKAGE_PREFIX,
  TARGETS,
  platformPackageName,
  resolveRuntimeBinary,
} from "@microstudio/runtime";
import {
  publishManifest,
  publishRepository,
  publishedPackages,
} from "../../../scripts/manifest.ts";

// node refuses to type-strip files under node_modules, so a published package
// is compiled; these tests are about the staged copy loading with no build step
const STAGER = fileURLToPath(
  new URL("../../../scripts/stage-package.ts", import.meta.url),
);
const BINARY_STAGER = fileURLToPath(
  new URL("../../../scripts/stage-binary.ts", import.meta.url),
);
const REPO_ROOT = fileURLToPath(new URL("../../..", import.meta.url));
const PACKAGES = ["runtime", "roblox-ts", "cli", "test"] as const;
const VERSION = "9.9.9";

interface Run {
  code: number;
  stdout: string;
  stderr: string;
}

function result(error: Error | null, stdout: string, stderr: string): Run {
  return {
    code: error === null ? 0 : ((error as { code?: number }).code ?? 1),
    stdout,
    stderr,
  };
}

// compiling four packages takes a moment
function runStager(args: readonly string[]): Promise<Run> {
  return runNode(STAGER, args);
}

function runBinaryStager(args: readonly string[]): Promise<Run> {
  return runNode(BINARY_STAGER, args);
}

function runNode(script: string, args: readonly string[]): Promise<Run> {
  return new Promise((resolvePromise) => {
    execFile(
      process.execPath,
      [script, ...args],
      { timeout: 180_000 },
      (error, stdout, stderr) => {
        resolvePromise(result(error, String(stdout), String(stderr)));
      },
    );
  });
}

function runNodeIn(args: readonly string[], cwd: string): Promise<Run> {
  return new Promise((resolvePromise) => {
    execFile(
      process.execPath,
      [...args],
      { cwd, timeout: 60_000 },
      (error, stdout, stderr) => {
        resolvePromise(result(error, String(stdout), String(stderr)));
      },
    );
  });
}

// npm is a shell script on windows, execFile cannot run it
// npm (not yarn pack) computes the file list a release actually publishes
function runNpm(command: string, cwd?: string): Promise<Run> {
  return new Promise((resolvePromise) => {
    exec(`npm ${command}`, { cwd, timeout: 120_000 }, (error, stdout, stderr) => {
      resolvePromise(result(error, String(stdout), String(stderr)));
    });
  });
}

function manifestOf(name: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(staged(name), "package.json"), "utf8")) as
    Record<string, unknown>;
}

// packs one staged package and returns the tarball's file name
async function pack(packageDir: string, into: string): Promise<string> {
  const packed = await runNpm(
    `pack "${packageDir}" --pack-destination "${into}" --json`,
  );
  assert.equal(packed.code, 0, packed.stderr);
  const entries = JSON.parse(packed.stdout) as { filename?: string }[];
  const file = entries[0]?.filename;
  assert.ok(file, `${packageDir} packed nothing`);
  return file;
}

function staged(name: string): string {
  return join(out, "@microstudio", name);
}

let dir = "";
let out = "";

before(async () => {
  dir = mkdtempSync(join(tmpdir(), "microstudio-packages-"));
  out = join(dir, "npm");
  const stagedAll = await runStager([
    "stage",
    "--all",
    "--out",
    out,
    "--version",
    VERSION,
  ]);
  assert.equal(stagedAll.code, 0, stagedAll.stderr);
});

after(() => {
  rmSync(dir, { recursive: true, force: true });
});

test("stage compiles every publishable package to JavaScript", () => {
  for (const name of PACKAGES) {
    assert.equal(existsSync(staged(name)), true, `${name} was not staged`);
    assert.equal(
      existsSync(join(staged(name), "dist", "index.js")),
      true,
      `${name} has no compiled entry point`,
    );
    // src/ is what a checkout runs, not what the registry gets
    assert.equal(existsSync(join(staged(name), "src")), false);
  }
});

test("the staged packages carry declarations and nothing else TypeScript", () => {
  for (const name of PACKAGES) {
    const files = readdirSync(staged(name), {
      recursive: true,
      encoding: "utf8",
    });
    const sources = files.filter(
      (file) => file.endsWith(".ts") && !file.endsWith(".d.ts"),
    );
    assert.deepEqual(sources, [], `${name} staged TypeScript sources`);
    assert.ok(
      files.includes(join("dist", "index.d.ts")),
      `${name} staged no declarations`,
    );
  }
});

test("the staged manifest points at the compiled output", () => {
  // exports, types and bin describe the package from outside, so all move together
  const runtime = manifestOf("runtime");
  assert.equal(runtime["name"], "@microstudio/runtime");
  assert.equal(runtime["version"], VERSION);
  assert.deepEqual(runtime["exports"], { ".": "./dist/index.js" });
  assert.equal(runtime["types"], "./dist/index.d.ts");
  assert.deepEqual(runtime["files"], ["dist"]);

  const cli = manifestOf("cli");
  assert.deepEqual(cli["bin"], { microstudio: "./dist/index.js" });

  // an uncompiled subpath (already declarations) survives the rewrite as written
  const framework = manifestOf("test");
  assert.deepEqual(framework["exports"], {
    ".": "./dist/index.js",
    "./types/globals": "./types/globals.d.ts",
  });
  assert.deepEqual(framework["files"], ["dist", "types"]);
});

test("a release pins its own packages to one version", () => {
  // five names the repository omits: yarn fails on a dependency that does not
  // exist, so staging is what writes them down
  const optional = manifestOf("runtime")["optionalDependencies"] as
    | Record<string, string>
    | undefined;
  assert.ok(optional, "the published driver must name its platform packages");
  assert.deepEqual(
    Object.keys(optional).sort(),
    [...TARGETS.map((target) => `${PLATFORM_PACKAGE_PREFIX}${target.name}`)].sort(),
  );
  for (const [name, range] of Object.entries(optional)) {
    assert.equal(range, VERSION, `${name} is not pinned to ${VERSION}`);
  }

  // only the runtime installs a sidecar; a cli and test framework should not
  for (const name of ["roblox-ts", "cli", "test"]) {
    assert.equal(
      "optionalDependencies" in manifestOf(name),
      false,
      `${name} would install a sidecar`,
    );
  }

  // the packages pin each other for the same reason
  const dependencies = manifestOf("cli")["dependencies"] as Record<string, string>;
  assert.deepEqual(Object.keys(dependencies).sort(), [
    "@microstudio/roblox-ts",
    "@microstudio/runtime",
    "@microstudio/test",
  ]);
  for (const [name, range] of Object.entries(dependencies)) {
    assert.equal(range, VERSION, `${name} is not pinned to ${VERSION}`);
  }

  // devDependencies keep the repository's "*": no installer resolves them
  const dev = manifestOf("test")["devDependencies"] as Record<string, string>;
  assert.deepEqual(dev, { "@microstudio/roblox-ts": "*" });
});

test("every package says where it was published from", () => {
  // npm compares this with the release repository and refuses the upload on a
  // mismatch; provenance cannot be generated at all without it
  const root = JSON.parse(
    readFileSync(join(REPO_ROOT, "package.json"), "utf8"),
  ) as { repository: { url: string } };
  for (const name of PACKAGES) {
    const repository = manifestOf(name)["repository"] as
      | { url?: string; directory?: string }
      | undefined;
    assert.ok(repository, `${name} declares no repository`);
    assert.equal(
      repository.url,
      root.repository.url,
      `${name} points at a different repository`,
    );
    assert.ok(repository.directory, `${name} does not say where it lives`);
  }
});

test("staging refuses a manifest that names another repository", () => {
  // caught at staging, not upload: npm refuses provenance on a mismatch
  const repository = publishRepository();
  assert.throws(
    () =>
      publishManifest(
        {
          name: "@microstudio/example",
          repository: {
            type: "git",
            url: "git+https://github.com/someone-else/microstudio.git",
          },
        },
        VERSION,
        repository,
      ),
    /someone-else/,
  );
  assert.throws(
    () => publishManifest({ name: "@microstudio/example" }, VERSION, repository),
    /no repository/,
  );

  // the named repository goes through, keeping its directory
  const staged = publishManifest(
    {
      name: "@microstudio/example",
      repository: publishRepository("packages/example"),
      files: ["src"],
    },
    VERSION,
    repository,
  );
  assert.deepEqual(staged.repository, {
    type: "git",
    url: repository.url,
    directory: "packages/example",
  });
  assert.deepEqual(staged.files, ["dist"]);
});

test("the assets that are not TypeScript survive staging", () => {
  // the runner reads the framework from disk relative to the compiled output
  const luau = join(staged("test"), "dist", "luau", "framework.luau");
  assert.equal(existsSync(luau), true);
  assert.equal(
    readFileSync(luau, "utf8"),
    readFileSync(join(REPO_ROOT, "packages", "test", "src", "luau", "framework.luau"), "utf8"),
  );
  assert.equal(
    existsSync(join(staged("test"), "types", "globals.d.ts")),
    true,
  );
});

test("npm packs the compiled output and no tests", async () => {
  for (const name of PACKAGES) {
    const packed = await runNpm(`pack --dry-run --json "${staged(name)}"`);
    assert.equal(packed.code, 0, packed.stderr);
    const entries = JSON.parse(packed.stdout) as {
      name: string;
      version: string;
      files: { path: string }[];
    }[];
    assert.equal(entries.length, 1);
    assert.equal(entries[0]?.name, `@microstudio/${name}`);
    assert.equal(entries[0]?.version, VERSION);

    const paths = (entries[0]?.files ?? []).map((file) => file.path);
    assert.ok(paths.includes("package.json"), `${name} packs no manifest`);
    assert.ok(
      paths.includes("dist/index.js"),
      `${name} packs no compiled entry point`,
    );
    for (const path of paths) {
      assert.equal(
        path.includes(".test."),
        false,
        `${name} would publish ${path}`,
      );
    }
  }
});

test("the compiled packages load from a plain node_modules", async () => {
  // install them as a package manager does: a copy under node_modules, exactly
  // the case node refuses to type-strip; sources here would fail this import
  const consumer = join(dir, "consumer");
  const modules = join(consumer, "node_modules", "@microstudio");
  mkdirSync(modules, { recursive: true });
  for (const name of PACKAGES) {
    cpSync(staged(name), join(modules, name), { recursive: true });
  }

  const target = TARGETS[0];
  assert.ok(target);
  const imported = await runNodeIn(
    [
      "--input-type=module",
      "--eval",
      // one import per package, so a missing file in any shows up
      [
        'import { TARGETS, platformPackageName } from "@microstudio/runtime";',
        'import { withRuntime } from "@microstudio/test";',
        'import * as robloxTs from "@microstudio/roblox-ts";',
        "const target = TARGETS[0];",
        "console.log(",
        '  [TARGETS.length, typeof withRuntime, Object.keys(robloxTs).length > 0, platformPackageName(target.platform, target.arch)].join(":"),',
        ");",
      ].join("\n"),
    ],
    consumer,
  );
  assert.equal(imported.code, 0, imported.stderr);
  assert.equal(
    imported.stdout.trim(),
    [
      TARGETS.length,
      "function",
      "true",
      platformPackageName(target.platform, target.arch),
    ].join(":"),
  );

  // the bin entry point is runnable: the shebang survived the compile
  const cli = join(modules, "cli", "dist", "index.js");
  assert.match(readFileSync(cli, "utf8"), /^#!\/usr\/bin\/env node\r?\n/);
  const version = await runNodeIn([cli, "--version"], consumer);
  assert.equal(version.code, 0, version.stderr);
  // the version the manifest carries, so an install cannot report another one
  assert.equal(version.stdout.trim(), `microstudio ${VERSION}`);

  // and so is the same package under its plain name, installed the way npm
  // installs an unscoped package
  cpSync(join(out, "microstudio"), join(consumer, "node_modules", "microstudio"), {
    recursive: true,
  });
  const plain = await runNodeIn(
    [join(consumer, "node_modules", "microstudio", "dist", "index.js"), "--version"],
    consumer,
  );
  assert.equal(plain.code, 0, plain.stderr);
  assert.equal(plain.stdout, version.stdout);
});

test("an install of the release runs the cli on the shipped sidecar", async () => {
  // the last step between a release and a user, and the one a release is for:
  // npm installs the tarballs, `microstudio` runs, and nothing compiled on the
  // machine. the sidecar comes from this checkout's build, which is the same
  // artifact the release publishes per platform
  const target = TARGETS.find(
    (entry) =>
      entry.platform === process.platform && entry.arch === process.arch,
  );
  assert.ok(target, `no platform package for ${process.platform}-${process.arch}`);

  const stagedBinary = await runBinaryStager([
    "stage",
    "--target",
    target.name,
    "--binary",
    resolveRuntimeBinary(),
    "--out",
    out,
    "--version",
    VERSION,
  ]);
  assert.equal(stagedBinary.code, 0, stagedBinary.stderr);

  // every staged package, packed and installed the way npm does it
  const install = join(dir, "install");
  const tarballs = join(install, "tgz");
  mkdirSync(tarballs, { recursive: true });
  const dependencies: Record<string, string> = {};
  for (const entry of publishedPackages(out)) {
    const packed = await pack(entry.dir, tarballs);
    dependencies[entry.name] = `file:tgz/${packed}`;
  }
  const platform = `${PLATFORM_PACKAGE_PREFIX}${target.name}`;
  dependencies[platform] = `file:tgz/${await pack(
    join(out, ...platform.split("/")),
    tarballs,
  )}`;

  writeFileSync(
    join(install, "package.json"),
    JSON.stringify(
      {
        name: "consumer",
        private: true,
        version: "0.0.0",
        dependencies,
      },
      null,
      2,
    ),
  );

  const installed = await runNpm(
    "install --no-audit --no-fund --loglevel error",
    install,
  );
  assert.equal(installed.code, 0, installed.stderr);

  // the command npm wires up, run from the installed tree
  const bin = join(install, "node_modules", "microstudio", "dist", "index.js");
  const version = await runNodeIn([bin, "--version"], install);
  assert.equal(version.code, 0, version.stderr);
  assert.equal(version.stdout.trim(), `microstudio ${VERSION}`);

  // and it finds the sidecar in the platform package the install pulled in
  const ran = await runNodeIn([bin, "-e", "print('installed')"], install);
  assert.equal(ran.code, 0, ran.stderr);
  assert.equal(ran.stdout.trim(), "installed");

  // the whole tool, on a project beside it: spec discovery, the Luau framework
  // it ships as an asset, and the runtime all come from the install
  cpSync(join(REPO_ROOT, "examples", "testing"), join(install, "project"), {
    recursive: true,
  });
  const specs = await runNodeIn([bin, "test"], join(install, "project"));
  assert.equal(specs.code, 0, specs.stderr);
  assert.match(specs.stdout, /Test files\s+1 passed/);
});

test("stage refuses a package it does not know", async () => {
  const unknown = await runStager(["stage", "not-a-package", "--out", out]);
  assert.equal(unknown.code, 1);
  assert.match(unknown.stderr, /Unknown package 'not-a-package'/);

  const nothing = await runStager(["stage", "--out", out]);
  assert.equal(nothing.code, 1);
  assert.match(nothing.stderr, /needs a package name or --all/);
});

test("the cli publishes under its own name as well as the scoped one", async () => {
  // the same files twice, so `npm i -g microstudio` is the same command
  const alias = join(out, "microstudio");
  assert.equal(existsSync(alias), true, "the alias was not staged");

  const canonical = manifestOf("cli");
  const mirrored = JSON.parse(
    readFileSync(join(alias, "package.json"), "utf8"),
  ) as Record<string, unknown>;
  assert.equal(mirrored["name"], "microstudio");
  assert.equal(mirrored["version"], canonical["version"]);
  assert.equal(mirrored["bin"] && JSON.stringify(mirrored["bin"]), JSON.stringify({ microstudio: "./dist/index.js" }));
  // an install of either name pulls the same internals, at this release's version
  assert.deepEqual(mirrored["dependencies"], canonical["dependencies"]);
  // a staging directive, not something npm should hand out
  assert.equal("publishAliases" in mirrored, false);
  assert.equal("publishAliases" in canonical, false);

  const files = readdirSync(alias, { recursive: true, encoding: "utf8" });
  assert.ok(files.includes(join("dist", "index.js")), "the alias has no entry point");
  assert.deepEqual(
    files.filter((file) => file.endsWith(".ts") && !file.endsWith(".d.ts")),
    [],
    "the alias staged TypeScript sources",
  );

  const packed = await runNpm(`pack --dry-run --json "${alias}"`);
  assert.equal(packed.code, 0, packed.stderr);
  const entries = JSON.parse(packed.stdout) as { name: string; version: string }[];
  assert.equal(entries[0]?.name, "microstudio");
  assert.equal(entries[0]?.version, VERSION);
});

test("the list a release publishes is what staging wrote", async () => {
  const listed = await runStager(["list", "--out", out]);
  assert.equal(listed.code, 0, listed.stderr);

  const entries = listed.stdout
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => {
      const [name = "", dir = ""] = line.trim().split(" ");
      return { name, dir };
    });
  assert.deepEqual(
    entries.map((entry) => entry.name),
    [
      "@microstudio/runtime",
      "@microstudio/roblox-ts",
      "@microstudio/cli",
      "microstudio",
      "@microstudio/test",
    ],
  );
  for (const entry of entries) {
    assert.equal(
      existsSync(join(entry.dir, "package.json")),
      true,
      `${entry.name} was listed at ${entry.dir}, which staging did not write`,
    );
  }
});

test("staging refuses an alias that is not a package name", () => {
  const repository = publishRepository();
  assert.throws(
    () =>
      publishManifest(
        {
          name: "@microstudio/example",
          repository: publishRepository("packages/example"),
          publishAliases: ["../escape"],
        },
        VERSION,
        repository,
      ),
    /invalid alias/,
  );
  assert.throws(
    () =>
      publishManifest(
        {
          name: "@microstudio/example",
          repository: publishRepository("packages/example"),
          publishAliases: ["@microstudio/example"],
        },
        VERSION,
        repository,
      ),
    /lists itself/,
  );
});
