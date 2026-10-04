// runs the project's own roblox-ts compiler so TypeScript specs exist as Luau

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

import { parseJsonc } from "./jsonc.ts";

export const TEST_CONFIG_NAME = "tsconfig.test.json";

export interface ProjectCompiler {
  // js entry run with the current node, so no shell or .cmd shim is involved
  entry: string;
  version?: string;
  // the directory that has node_modules/roblox-ts
  root: string;
}

export interface TsConfigInfo {
  file: string;
  dir: string;
  // absolute
  outDir: string;
  rootDir?: string;
  // absolute patterns
  include: string[];
}

function readJsonFile(file: string): Record<string, unknown> {
  const parsed: unknown = parseJsonc(readFileSync(file, "utf8"));
  return typeof parsed === "object" && parsed !== null
    ? (parsed as Record<string, unknown>)
    : {};
}

// tsc and roblox-ts both take forward slashes in a config on any platform
function slashes(path: string): string {
  return path.replaceAll("\\", "/");
}

// walks up: a spec's project may sit in a monorepo with one install at the root
export function findCompiler(startDir: string): ProjectCompiler | undefined {
  let dir = resolve(startDir);
  for (;;) {
    const manifest = join(dir, "node_modules", "roblox-ts", "package.json");
    if (existsSync(manifest)) {
      const json = readJsonFile(manifest);
      const bin = json["bin"];
      const entryPath =
        typeof bin === "string"
          ? bin
          : typeof bin === "object" && bin !== null
            ? (bin as Record<string, unknown>)["rbxtsc"]
            : undefined;
      if (typeof entryPath === "string") {
        const entry = resolve(dirname(manifest), entryPath);
        if (existsSync(entry)) {
          return {
            entry,
            root: dir,
            ...(typeof json["version"] === "string" ? { version: json["version"] } : {}),
          };
        }
      }
    }

    const parent = dirname(dir);
    if (parent === dir) {
      return undefined;
    }
    dir = parent;
  }
}

export function readTsConfig(file: string): TsConfigInfo {
  const configFile = resolve(file);
  const dir = dirname(configFile);
  const json = readJsonFile(configFile);
  const options =
    typeof json["compilerOptions"] === "object" && json["compilerOptions"] !== null
      ? (json["compilerOptions"] as Record<string, unknown>)
      : {};

  const rawInclude = json["include"];
  const patterns = Array.isArray(rawInclude)
    ? rawInclude.filter((entry): entry is string => typeof entry === "string")
    : ["**/*"];

  return {
    file: configFile,
    dir,
    outDir: resolve(dir, typeof options["outDir"] === "string" ? options["outDir"] : "out"),
    ...(typeof options["rootDir"] === "string"
      ? { rootDir: resolve(dir, options["rootDir"]) }
      : {}),
    include: patterns.map((pattern) => resolve(dir, pattern)),
  };
}

export interface TestConfigOptions {
  config: TsConfigInfo;
  // absolute spec files, so specs outside the project's include still compile
  specs: readonly string[];
  // absolute path to the framework globals, so specs need no config of their own
  globals: string;
}

// written into .microstudio so the project keeps its own tsconfig untouched
export function testConfigFile(config: TsConfigInfo): string {
  return join(config.dir, ".microstudio", TEST_CONFIG_NAME);
}

export function writeTestConfig(options: TestConfigOptions): string {
  const { config } = options;
  const file = testConfigFile(config);
  mkdirSync(dirname(file), { recursive: true });

  const document = {
    extends: slashes(config.file),
    compilerOptions: {
      // roblox-ts requires a root that resolves to <config dir>/node_modules/@rbxts,
      // and the config sits one level down, so both are named
      typeRoots: [
        slashes(join(dirname(file), "node_modules", "@rbxts")),
        slashes(join(config.dir, "node_modules", "@rbxts")),
      ],
    },
    include: [
      ...config.include.map(slashes),
      ...options.specs.map(slashes),
      slashes(options.globals),
    ],
  };

  writeFileSync(file, `${JSON.stringify(document, null, 2)}\n`);
  return file;
}

export interface CompileOptions {
  // project directory, the compiler's working directory
  dir: string;
  compiler: ProjectCompiler;
  // the generated config
  config: string;
  // roblox-ts maps the tree through Rojo, so it needs the file
  projectFile: string;
  // where RuntimeLib is copied; the Rojo project's rbxts_include mount
  includeFolder: string;
}

export interface CompileResult {
  ok: boolean;
  output: string;
}

export function compileProject(options: CompileOptions): CompileResult {
  const args = [
    "-p",
    options.config,
    "--rojo",
    options.projectFile,
    "--includePath",
    options.includeFolder,
  ];
  const result = spawnSync(process.execPath, [options.compiler.entry, ...args], {
    cwd: options.dir,
    encoding: "utf8",
  });
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
  return { ok: result.status === 0, output };
}

// longest trailing run of matching path segments, so two specs of the same name
// in different folders are told apart
function suffixScore(candidate: string, wanted: string): number {
  const left = candidate.replaceAll("\\", "/").split("/");
  const right = wanted.replaceAll("\\", "/").split("/");
  let score = 0;
  while (
    score < left.length &&
    score < right.length &&
    left[left.length - 1 - score] === right[right.length - 1 - score]
  ) {
    score += 1;
  }
  return score;
}

// rbxtsc decides the extension and the depth from the project's tsconfig
export function findCompiledSpec(outDir: string, spec: string): string | undefined {
  if (!existsSync(outDir)) {
    return undefined;
  }

  const stem = basename(spec).replace(/\.tsx?$/i, "");
  const names = new Set([`${stem}.luau`, `${stem}.lua`]);
  const wanted = slashes(resolve(spec).replace(/\.tsx?$/i, ""));

  let best: string | undefined;
  let bestScore = -1;
  let tied = false;
  for (const entry of readdirSync(outDir, { recursive: true, withFileTypes: false })) {
    const candidate = resolve(outDir, String(entry));
    if (!existsSync(candidate) || !names.has(basename(candidate))) {
      continue;
    }
    const score = suffixScore(candidate.replace(/\.(luau|lua)$/i, ""), wanted);
    if (score > bestScore) {
      best = candidate;
      bestScore = score;
      tied = false;
    } else if (score === bestScore) {
      tied = true;
    }
  }

  return tied ? undefined : best;
}
