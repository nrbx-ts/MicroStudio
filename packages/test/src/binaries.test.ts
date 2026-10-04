import assert from "node:assert/strict";
import { exec, execFile } from "node:child_process";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  PLATFORM_PACKAGE_PREFIX,
  TARGETS,
  installedPlatformBinary,
  platformPackageName,
  resolveRuntimeBinary,
} from "@microstudio/runtime";
import {
  readDependencies,
  unexpectedDependencies,
} from "../../../scripts/check-binary.ts";

// the target table, staging script and resolver must agree; tests cover the seams
const STAGER = fileURLToPath(
  new URL("../../../scripts/stage-binary.ts", import.meta.url),
);
const RUNTIME_MANIFEST = new URL(
  "../../runtime/package.json",
  import.meta.url,
);
const ROOT_MANIFEST = new URL("../../../package.json", import.meta.url);

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

function runStager(args: readonly string[]): Promise<Run> {
  return new Promise((resolvePromise) => {
    execFile(
      process.execPath,
      [STAGER, ...args],
      { timeout: 60_000 },
      (error, stdout, stderr) => {
        resolvePromise(result(error, String(stdout), String(stderr)));
      },
    );
  });
}

// npm is a shell script on windows, execFile cannot run it
function runNpm(command: string): Promise<Run> {
  return new Promise((resolvePromise) => {
    // a shell is needed (see above); paths come from mkdtempSync, so double
    // quotes are enough, and execFile with an args array would warn
    exec(
      `npm ${command}`,
      { timeout: 60_000 },
      (error, stdout, stderr) => {
        resolvePromise(result(error, String(stdout), String(stderr)));
      },
    );
  });
}

function withTempDir(body: (dir: string) => Promise<void> | void): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-binaries-"));
  return Promise.resolve(body(dir)).finally(() =>
    rmSync(dir, { recursive: true, force: true }),
  );
}

// stand-in sidecar; the stager only copies bytes
function standInBinary(dir: string, name: string): string {
  const path = join(dir, name);
  writeFileSync(path, "#!/bin/sh\necho stand-in\n");
  chmodSync(path, 0o755);
  return path;
}

function rootVersion(): string {
  const manifest = JSON.parse(readFileSync(ROOT_MANIFEST, "utf8")) as {
    version: string;
  };
  return manifest.version;
}

// the repository npm compares a package against
function rootRepository(): string {
  const manifest = JSON.parse(readFileSync(ROOT_MANIFEST, "utf8")) as {
    repository: { url: string };
  };
  return manifest.repository.url;
}

const hostTarget = TARGETS.find(
  (target) =>
    target.platform === process.platform && target.arch === process.arch,
);

test("the sidecar this checkout built needs nothing installed", () => {
  // a prebuilt binary that wants a library the machine lacks installs and then
  // fails to start, so its dependency table is part of what a release promises.
  // this is the check the binaries job runs, on the binary it is about to publish
  const binary = resolveRuntimeBinary();
  const dependencies = readDependencies(binary);
  assert.ok(
    dependencies.length > 0,
    `${binary} reports no dependencies, which means it was not read`,
  );
  assert.deepEqual(
    unexpectedDependencies(dependencies),
    [],
    `${binary} loads something a machine would have to install`,
  );
});

test("each platform's rules name what it cannot assume", () => {
  // windows: the visual c++ redistributable is not part of windows
  assert.deepEqual(
    unexpectedDependencies(
      [
        "KERNEL32.dll",
        "api-ms-win-crt-runtime-l1-1-0.dll",
        "VCRUNTIME140.dll",
        "MSVCP140.dll",
      ],
      "win32",
    ),
    ["VCRUNTIME140.dll", "MSVCP140.dll"],
  );

  // linux: the c library, what gcc brings, and the c++ runtime luau needs
  assert.deepEqual(
    unexpectedDependencies(
      [
        "libc.so.6",
        "libm.so.6",
        "libgcc_s.so.1",
        "libpthread.so.0",
        "libstdc++.so.6",
        "libssl.so.3",
      ],
      "linux",
    ),
    ["libssl.so.3"],
  );

  // macOS: whatever the system ships, by path
  assert.deepEqual(
    unexpectedDependencies(
      ["/usr/lib/libSystem.B.dylib", "/opt/homebrew/lib/libssl.3.dylib"],
      "darwin",
    ),
    ["/opt/homebrew/lib/libssl.3.dylib"],
  );
});

test("the target table describes one package per platform", () => {
  assert.ok(TARGETS.length >= 3);
  const names = new Set<string>();
  for (const target of TARGETS) {
    assert.equal(
      target.name,
      `${target.platform}-${target.arch}`,
      `${target.name} must be named after Node's own identifiers`,
    );
    assert.equal(names.has(target.name), false, `${target.name} is a duplicate`);
    names.add(target.name);
    // cargo triple: at least arch-vendor-os, sometimes with an ABI
    assert.match(target.triple, /^[a-z0-9_]+(-[a-z0-9_]+){2,}$/);
    // npm checks `os`/`cpu` against process.platform/process.arch
    assert.equal(target.binary.endsWith(".exe"), target.platform === "win32");
  }
});

test("a platform package is named after the platform", () => {  assert.equal(
    platformPackageName("win32", "x64"),
    "@microstudio/runtime-win32-x64",
  );
  assert.equal(
    platformPackageName("linux", "arm64"),
    "@microstudio/runtime-linux-arm64",
  );
  assert.equal(
    platformPackageName("darwin", "arm64"),
    "@microstudio/runtime-darwin-arm64",
  );
  // an unbuilt platform is not an error, just nothing to install
  assert.equal(platformPackageName("sunos", "x64"), undefined);
  assert.equal(platformPackageName("linux", "ia32"), undefined);
});

test("the repository manifest leaves the platform packages to a release", () => {
  // yarn fails on a missing dependency where npm skips a missing optional, so
  // naming the platform packages here would break yarn install; staging adds them
  const manifest = JSON.parse(readFileSync(RUNTIME_MANIFEST, "utf8")) as {
    optionalDependencies?: Record<string, string>;
  };
  assert.deepEqual(manifest.optionalDependencies ?? {}, {});
});

test("stage writes a package npm can publish", async () => {
  const target = hostTarget ?? TARGETS[0];
  assert.ok(target);
  await withTempDir(async (dir) => {
    const binary = standInBinary(dir, target.binary);
    const out = join(dir, "npm");
    const result = await runStager([
      "stage",
      "--target",
      target.name,
      "--binary",
      binary,
      "--out",
      out,
      "--version",
      "1.2.3",
    ]);

    assert.equal(result.code, 0, result.stderr);
    const packageDir = join(out, ...`${PLATFORM_PACKAGE_PREFIX}${target.name}`.split("/"));

    const manifest = JSON.parse(
      readFileSync(join(packageDir, "package.json"), "utf8"),
    ) as Record<string, unknown>;
    assert.equal(manifest["name"], `${PLATFORM_PACKAGE_PREFIX}${target.name}`);
    assert.equal(manifest["version"], "1.2.3");
    assert.deepEqual(manifest["os"], [target.platform]);
    assert.deepEqual(manifest["cpu"], [target.arch]);
    assert.deepEqual(manifest["files"], ["bin"]);
    // a deep import must keep working, so exports is not narrowed
    assert.equal("exports" in manifest, false);
    // npm checks this against the repository a release runs in
    assert.equal(
      (manifest["repository"] as { url: string }).url,
      rootRepository(),
    );

    const staged = join(packageDir, "bin", target.binary);
    assert.equal(readFileSync(staged, "utf8"), readFileSync(binary, "utf8"));
    if (process.platform !== "win32") {
      assert.equal(statSync(staged).mode & 0o777, 0o755, "the binary must be executable");
    }
  });
});

test("stage defaults the version to the one being released", async () => {
  const target = hostTarget ?? TARGETS[0];
  assert.ok(target);
  await withTempDir(async (dir) => {
    const result = await runStager([
      "stage",
      "--target",
      target.name,
      "--binary",
      standInBinary(dir, target.binary),
      "--out",
      join(dir, "npm"),
    ]);

    assert.equal(result.code, 0, result.stderr);
    assert.match(result.stdout, /^@microstudio\/runtime-/m);
    const manifest = JSON.parse(
      readFileSync(
        join(dir, "npm", "@microstudio", `runtime-${target.name}`, "package.json"),
        "utf8",
      ),
    ) as { version: string };
    assert.equal(manifest.version, rootVersion());
  });
});

test("stage refuses a target it cannot build", async () => {
  await withTempDir(async (dir) => {
    const result = await runStager([
      "stage",
      "--target",
      "plan9-x64",
      "--binary",
      standInBinary(dir, "microstudio-runtime"),
      "--out",
      join(dir, "npm"),
    ]);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /Unknown target 'plan9-x64'/);
  });
});

test("stage refuses a missing binary rather than shipping nothing", async () => {
  await withTempDir(async (dir) => {
    const result = await runStager([
      "stage",
      "--target",
      "linux-x64",
      "--binary",
      join(dir, "not-built"),
      "--out",
      join(dir, "npm"),
    ]);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /No binary at/);
  });
});

test("a staged package survives install and is found by the resolver", async () => {
  const target = hostTarget ?? TARGETS[0];
  assert.ok(target);
  const packageName = `${PLATFORM_PACKAGE_PREFIX}${target.name}`;
  await withTempDir(async (dir) => {
    // stage it the way a release does
    const out = join(dir, "npm");
    const staged = await runStager([
      "stage",
      "--target",
      target.name,
      "--binary",
      standInBinary(dir, target.binary),
      "--out",
      out,
      "--version",
      "1.2.3",
    ]);
    assert.equal(staged.code, 0, staged.stderr);
    const packageDir = join(out, ...packageName.split("/"));

    // install it the way npm does, under a consumer's node_modules
    const installed = join(dir, "node_modules", "@microstudio", `runtime-${target.name}`);
    mkdirSync(join(dir, "node_modules", "@microstudio"), { recursive: true });
    cpSync(packageDir, installed, { recursive: true });

    // the resolver finds it from the consumer's own file. macOS turns /var into
    // /private/var and node's resolver returns the real path, so ask the
    // filesystem rather than comparing the string mkdtemp handed us
    const resolved = installedPlatformBinary({
      platform: target.platform,
      arch: target.arch,
      from: join(dir, "index.js"),
    });
    assert.equal(
      resolved === undefined ? undefined : realpathSync(resolved),
      realpathSync(join(installed, "bin", target.binary)),
    );

    // npm packs exactly the binary and the manifest
    const packed = await runNpm(`pack --dry-run --json "${packageDir}"`);
    assert.equal(packed.code, 0, packed.stderr);
    const entries = JSON.parse(packed.stdout) as {
      name: string;
      version: string;
      files: { path: string }[];
    }[];
    assert.equal(entries.length, 1);
    assert.equal(entries[0]?.name, packageName);
    assert.equal(entries[0]?.version, "1.2.3");
    assert.deepEqual(
      entries[0]?.files.map((file) => file.path).sort(),
      [
        "package.json",
        "README.md",
        "LICENSE",
        join("bin", target.binary).replaceAll("\\", "/"),
      ].sort(),
    );
  });
});

test("the resolver says nothing when the package is not installed", async () => {
  await withTempDir((dir) => {
    assert.equal(
      installedPlatformBinary({ from: join(dir, "index.js") }),
      undefined,
    );
    // a missing file inside an installed package is not a binary
    const target = hostTarget ?? TARGETS[0];
    assert.ok(target);
    mkdirSync(
      join(dir, "node_modules", "@microstudio", `runtime-${target.name}`),
      { recursive: true },
    );
    assert.equal(
      installedPlatformBinary({ from: join(dir, "index.js") }),
      undefined,
    );
  });
});

test("the resolver has no package for a platform we do not build", () => {
  assert.equal(
    installedPlatformBinary({ platform: "sunos", arch: "x64" }),
    undefined,
  );
});

test("the staging script explains itself", async () => {
  const result = await runStager([]);
  assert.equal(result.code, 0);
  assert.match(result.stdout, /USAGE:/);
  for (const target of TARGETS) {
    assert.match(result.stdout, new RegExp(target.name));
    assert.match(result.stdout, new RegExp(target.triple));
  }
  assert.equal(existsSync(STAGER), true);
});
