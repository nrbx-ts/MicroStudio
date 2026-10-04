// release helpers: staging dir, version, manifest rewrite rules

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PLATFORM_PACKAGE_PREFIX,
  TARGETS,
} from "../packages/runtime/src/targets.ts";

const scriptDir = dirname(fileURLToPath(import.meta.url));

export const repoRoot = resolve(scriptDir, "..");
// staged packages land here, gitignored
export const STAGING_DIR = join(repoRoot, "dist", "npm");

// source repo, the shape npm compares for provenance
export interface Repository {
  type: string;
  url: string;
  directory?: string;
}

// the parts of package.json a release touches
export interface Manifest {
  name?: string;
  version?: string;
  private?: boolean;
  type?: string;
  repository?: string | Repository;
  dependencies?: Record<string, string>;
  optionalDependencies?: Record<string, string>;
  exports?: Record<string, unknown> | string;
  types?: string;
  files?: string[];
  bin?: Record<string, string> | string;
  // other names this package also publishes under
  publishAliases?: string[];
  [key: string]: unknown;
}

export function readManifest(path: string): Manifest {
  return JSON.parse(readFileSync(path, "utf8")) as Manifest;
}

export function writeManifest(path: string, manifest: Manifest): void {
  // two spaces and a trailing newline, same as npm
  writeFileSync(path, `${JSON.stringify(manifest, null, 2)}\n`);
}

// version being released, root manifest unless given
export function releaseVersion(explicit: string | undefined): string {
  if (explicit !== undefined) {
    return explicit;
  }
  const version = readManifest(join(repoRoot, "package.json")).version;
  if (typeof version !== "string") {
    throw new Error("The root package.json has no version.");
  }
  return version;
}

// repo a package must point at, npm checks it before signing provenance
export function publishRepository(directory?: string): Repository {
  const { repository } = readManifest(join(repoRoot, "package.json"));
  if (
    repository === undefined ||
    typeof repository === "string" ||
    typeof repository.url !== "string"
  ) {
    throw new Error(
      "The root package.json needs a `repository` object with a `url`: npm checks it against the repository a release runs in, and publishes nothing with provenance when it disagrees.",
    );
  }
  const { type = "git", url } = repository;
  return directory === undefined ? { type, url } : { type, url, directory };
}

// --flag value pairs, plus positionals
export function flagArgs(argv: readonly string[]): {
  command: string;
  flags: string[];
  values: Map<string, string>;
  positionals: string[];
} {
  const [command = "", ...rest] = argv;
  const values = new Map<string, string>();
  const flags: string[] = [];
  const positionals: string[] = [];

  for (let index = 0; index < rest.length; index += 1) {
    const arg = rest[index];
    if (arg === undefined) {
      continue;
    }
    if (!arg.startsWith("--")) {
      positionals.push(arg);
      continue;
    }
    const value = rest[index + 1];
    if (value === undefined || value.startsWith("--")) {
      // --all and friends take no value
      if (arg === "--all") {
        flags.push("all");
        continue;
      }
      throw new Error(`${arg} needs a value.`);
    }
    values.set(arg.slice(2), value);
    index += 1;
  }

  return { command, flags, positionals, values };
}

// everything published lives under this scope
const PACKAGE_SCOPE = "@microstudio/";

// npm's own rules for a name, so an alias cannot escape the staging directory
const PACKAGE_NAME = /^(@[a-z0-9-~][a-z0-9-._~]*\/)?[a-z0-9-~][a-z0-9-._~]*$/;

// the packages a release stages, in the order npm should see them
export const PACKAGE_DIRS = ["runtime", "roblox-ts", "cli", "test"] as const;

// where a staged package lands: a scoped name nests one directory deeper
export function stagedDir(out: string, name: string): string {
  return resolve(out, ...name.split("/"));
}

// the other names a package also publishes under, from its own manifest.
// `microstudio` is the cli, so the tool can also be installed by its own name
export function publishAliases(manifest: Manifest): string[] {
  const declared = manifest.publishAliases;
  if (declared === undefined) {
    return [];
  }
  if (!Array.isArray(declared)) {
    throw new Error(
      `${manifest.name ?? "A package"} has a publishAliases that is not an array.`,
    );
  }

  const aliases: string[] = [];
  for (const entry of declared) {
    if (typeof entry !== "string" || !PACKAGE_NAME.test(entry)) {
      throw new Error(
        `${manifest.name ?? "A package"} has an invalid alias ${JSON.stringify(entry)}: a package name is lowercase and url-safe.`,
      );
    }
    if (entry === manifest.name) {
      throw new Error(
        `${manifest.name ?? "A package"} lists itself as an alias.`,
      );
    }
    if (aliases.includes(entry)) {
      throw new Error(
        `${manifest.name ?? "A package"} lists the alias ${entry} twice.`,
      );
    }
    aliases.push(entry);
  }
  return aliases;
}

// every name a release publishes, an alias right after the package it mirrors
export function publishedPackages(
  out: string = STAGING_DIR,
): { name: string; dir: string }[] {
  const published: { name: string; dir: string }[] = [];
  for (const packageDir of PACKAGE_DIRS) {
    const manifest = readManifest(join(repoRoot, "packages", packageDir, "package.json"));
    const name = manifest.name;
    if (typeof name !== "string") {
      throw new Error(`packages/${packageDir}/package.json has no name.`);
    }
    published.push({ name, dir: stagedDir(out, name) });
    for (const alias of publishAliases(manifest)) {
      published.push({ name: alias, dir: stagedDir(out, alias) });
    }
  }
  return published;
}

// install of this package brings a sidecar, platform deps only exist once published
const SIDECAR_PACKAGE = "@microstudio/runtime";

// pin our packages to one version: platform packages, and each other
export function pinReleaseVersions(
  manifest: Manifest,
  version: string,
): Manifest {
  // pins our entries; `add` must exist even when the manifest omits it
  const pinned = (
    given: Record<string, string> | undefined,
    add?: ReadonlyMap<string, string>,
  ): Record<string, string> | undefined => {
    if (given === undefined && add === undefined) {
      return undefined;
    }
    const entries: Record<string, string> = {};
    for (const [name, range] of Object.entries(given ?? {})) {
      entries[name] =
        add?.get(name) ?? (name.startsWith(PACKAGE_SCOPE) ? version : range);
    }
    for (const [name, value] of add ?? []) {
      entries[name] ??= value;
    }
    return entries;
  };

  const publish: Manifest = { ...manifest };

  // all five listed, though an install keeps only the one it can run
  const optional = pinned(
    manifest.optionalDependencies,
    manifest.name === SIDECAR_PACKAGE
      ? new Map(
          TARGETS.map((target) => [
            `${PLATFORM_PACKAGE_PREFIX}${target.name}`,
            version,
          ]),
        )
      : undefined,
  );
  if (optional !== undefined) {
    publish.optionalDependencies = optional;
  }

  const dependencies = pinned(manifest.dependencies);
  if (dependencies !== undefined) {
    publish.dependencies = dependencies;
  }

  return publish;
}

// rewrite a repo manifest into what npm gets: ./src/*.ts -> ./dist/*.js, files
// follow, and the repository has to match or provenance is refused
export function publishManifest(
  manifest: Manifest,
  version: string,
  repository: Repository,
): Manifest {
  const declared =
    typeof manifest.repository === "string"
      ? manifest.repository
      : manifest.repository?.url;
  if (declared !== repository.url) {
    throw new Error(
      `${manifest.name ?? "This package"} points at ${declared ?? "no repository"}, but a release publishes from ${repository.url}.`,
    );
  }

  const publish: Manifest = { ...manifest, version };

  const rewrite = (value: string, extension = ".js"): string =>
    value.startsWith("./src/")
      ? `./dist/${value.slice("./src/".length).replace(/\.ts$/, extension)}`
      : value;

  // declarations to .d.ts, everything else to .js
  if (typeof manifest.types === "string") {
    publish.types = rewrite(manifest.types, ".d.ts");
  }
  if (typeof manifest.bin === "string") {
    publish.bin = rewrite(manifest.bin);
  } else if (manifest.bin !== undefined) {
    publish.bin = Object.fromEntries(
      Object.entries(manifest.bin).map(([name, path]) => [name, rewrite(path)]),
    );
  }
  if (typeof manifest.exports === "string") {
    publish.exports = rewrite(manifest.exports);
  } else if (manifest.exports !== undefined) {
    publish.exports = Object.fromEntries(
      Object.entries(manifest.exports).map(([key, value]) => [
        key,
        typeof value === "string" ? rewrite(value) : value,
      ]),
    );
  }

  // compiled output ships, sources do not (types/ is already declarations)
  const files = manifest.files ?? ["src"];
  publish.files = files
    .filter((entry) => !entry.startsWith("!"))
    .map((entry) => (entry === "src" ? "dist" : entry));

  // a staging directive, not something an install should carry. checked here, so a
  // bad alias fails staging rather than an upload halfway through a release
  publishAliases(manifest);
  delete publish.publishAliases;

  return pinReleaseVersions(publish, version);
}
