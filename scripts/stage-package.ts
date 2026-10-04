#!/usr/bin/env node
// compile a package into dist/npm/@microstudio/<name>, with a manifest pointing
// at it: a checkout runs .ts, node will not strip types under node_modules, so
// only what is published has to be javascript

import { cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

import {
  PACKAGE_DIRS,
  STAGING_DIR,
  flagArgs,
  publishAliases,
  publishManifest,
  publishRepository,
  publishedPackages,
  readManifest,
  releaseVersion,
  repoRoot,
  stagedDir,
  writeManifest,
} from "./manifest.ts";

const USAGE = `\
stage-package -- compile a package into something npm can publish

USAGE:
  stage <name> [--out <dir>] [--version <v>]
  stage --all  [--out <dir>] [--version <v>]
  list         [--out <dir>]

COMMANDS:
  stage  Compile packages/<name> into <out>/@microstudio/<name>, with a manifest
         whose exports, types and bin point at the compiled output. A package
         that declares publishAliases is staged again under each alias, from the
         same files. --version defaults to the root package.json.
  list   Print "<name> <dir>" for everything stage --all writes, aliases
         included, so a release publishes what was staged and nothing else.
`;

// the tsc this repo installed
function typescriptCompiler(): string {
  return createRequire(import.meta.url).resolve("typescript/bin/tsc");
}

// tsconfig for one package: repo strictness plus what emitting needs
function buildConfig(packageDir: string, outDir: string): string {
  // ts matches include globs with forward slashes, even on windows
  const posix = (path: string): string => path.replaceAll("\\", "/");
  return `${JSON.stringify(
    {
      extends: posix(join(repoRoot, "tsconfig.json")),
      compilerOptions: {
        noEmit: false,
        declaration: true,
        declarationMap: false,
        sourceMap: false,
        rewriteRelativeImportExtensions: true,
        outDir: posix(outDir),
        rootDir: posix(join(packageDir, "src")),
        // the config sits outside the repo, so name @types explicitly
        typeRoots: [posix(join(repoRoot, "node_modules", "@types"))],
      },
      include: [posix(join(packageDir, "src", "**", "*.ts"))],
      // tests stay in the repo, they spawn a cli and a sidecar
      exclude: [posix(join(packageDir, "src", "**", "*.test.ts"))],
    },
    null,
    2,
  )}\n`;
}

// compile one package, returns where it and its aliases went
function stagePackage(
  name: string,
  version: string,
  out: string,
): { staged: string; aliases: string[] } {
  const packageDir = join(repoRoot, "packages", name);
  const manifestPath = join(packageDir, "package.json");
  if (!existsSync(manifestPath)) {
    throw new Error(`No packages/${name}/package.json.`);
  }

  const manifest = readManifest(manifestPath);
  if (manifest.name === undefined) {
    throw new Error(`packages/${name}/package.json has no name.`);
  }

  const staged = stagedDir(out, manifest.name);
  rmSync(staged, { recursive: true, force: true });
  const compiledDir = join(staged, "dist");
  mkdirSync(compiledDir, { recursive: true });

  const configDir = join(out, ".staging");
  mkdirSync(configDir, { recursive: true });
  const configPath = join(configDir, `${name}.tsconfig.json`);
  writeFileSync(configPath, buildConfig(packageDir, compiledDir));
  const compile = spawnSync(
    process.execPath,
    [typescriptCompiler(), "-p", configPath],
    { cwd: repoRoot, stdio: "inherit" },
  );
  rmSync(configPath, { force: true });
  if (compile.status !== 0) {
    throw new Error(`TypeScript failed for packages/${name}.`);
  }

  // luau framework is read from disk by the runner, so keep the layout:
  // dist/framework/runner.js reads dist/luau/framework.luau
  const luau = join(packageDir, "src", "luau");
  if (existsSync(luau)) {
    cpSync(luau, join(compiledDir, "luau"), { recursive: true });
  }
  // types/ is already declarations, copied not compiled
  const types = join(packageDir, "types");
  if (existsSync(types)) {
    cpSync(types, join(staged, "types"), { recursive: true });
  }

  writeManifest(
    join(staged, "package.json"),
    publishManifest(manifest, version, publishRepository()),
  );

  const aliases = publishAliases(manifest).map((alias) => {
    const copy = stagedDir(out, alias);
    rmSync(copy, { recursive: true, force: true });
    cpSync(staged, copy, { recursive: true });
    const manifestOfCopy = readManifest(join(copy, "package.json"));
    // the same files under another name, so `npm i -g microstudio` is the cli
    writeManifest(join(copy, "package.json"), { ...manifestOfCopy, name: alias });
    return copy;
  });

  return { staged, aliases };
}

function main(argv: readonly string[]): number {
  const { command, flags, positionals, values } = flagArgs(argv);
  const out = resolve(values.get("out") ?? STAGING_DIR);

  if (command === "list") {
    for (const entry of publishedPackages(out)) {
      // forward slashes, so a bash `read -r name dir` loop can use the path on any
      // platform the release runs on
      console.log(`${entry.name} ${entry.dir.split("\\").join("/")}`);
    }
    return 0;
  }

  if (command !== "stage") {
    process.stdout.write(USAGE);
    return command === "" || command === "--help" ? 0 : 2;
  }

  const version = releaseVersion(values.get("version"));
  const names = flags.includes("all") ? [...PACKAGE_DIRS] : positionals;

  if (names.length === 0) {
    throw new Error(
      `stage needs a package name or --all. Known: ${PACKAGE_DIRS.join(", ")}.`,
    );
  }
  for (const name of names) {
    if (!(PACKAGE_DIRS as readonly string[]).includes(name)) {
      throw new Error(
        `Unknown package '${name}'. Known: ${PACKAGE_DIRS.join(", ")}.`,
      );
    }
  }

  for (const name of names) {
    const { staged, aliases } = stagePackage(name, version, out);
    console.log(`@microstudio/${name}@${version}`);
    console.log(`  staged: ${staged}`);
    for (const alias of aliases) {
      console.log(`  alias:  ${alias}`);
    }
  }
  return 0;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
