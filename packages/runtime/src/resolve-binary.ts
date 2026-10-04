import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PLATFORM_PACKAGE_PREFIX,
  findTarget,
  platformPackageName,
} from "./targets.ts";

const here = dirname(fileURLToPath(import.meta.url));

// repo root, from this file's location (packages/runtime/src)
export const repoRoot = join(here, "..", "..", "..");

const binaryName =
  process.platform === "win32" ? "microstudio-runtime.exe" : "microstudio-runtime";
// cargo's deps/ spells crate names with underscores
const depsBinaryName =
  process.platform === "win32" ? "microstudio_runtime.exe" : "microstudio_runtime";

export interface ResolveOptions {
  preferRelease?: boolean;
  // searched in order, before the defaults
  extraCandidates?: string[];
  // where to resolve the platform package from; a test passes elsewhere
  from?: string | URL;
}

export interface InstalledBinaryOptions {
  platform?: string;
  arch?: string;
  from?: string | URL;
}

// package name and bin/ path must agree with scripts/stage-binary.ts
export function installedPlatformBinary(
  options: InstalledBinaryOptions = {},
): string | undefined {
  const target = findTarget(
    options.platform ?? process.platform,
    options.arch ?? process.arch,
  );
  if (target === undefined) {
    return undefined;
  }

  try {
    // deep path: platform package must not have an exports map
    const resolved = createRequire(options.from ?? import.meta.url).resolve(
      `${PLATFORM_PACKAGE_PREFIX}${target.name}/bin/${target.binary}`,
    );
    return existsSync(resolved) ? resolved : undefined;
  } catch {
    // optional dep: not installed is normal
    return undefined;
  }
}

function installHint(platform: string, arch: string): string {
  const name = platformPackageName(platform, arch);
  return name === undefined
    ? `  There is no prebuilt binary for ${platform}-${arch} yet.`
    : `  add ${name} to your dependencies`;
}

// order: env override, cargo output, then the platform package
export function resolveRuntimeBinary(options: ResolveOptions = {}): string {
  const override = process.env["MICROSTUDIO_RUNTIME_BIN"];
  if (override) {
    if (!existsSync(override)) {
      throw new Error(
        `MICROSTUDIO_RUNTIME_BIN points at '${override}', which does not exist.`,
      );
    }
    return override;
  }

  const targetDirs = [
    process.env["MICROSTUDIO_TARGET_DIR"],
    process.env["CARGO_TARGET_DIR"],
    join(repoRoot, "target"),
  ].filter((dir): dir is string => typeof dir === "string" && dir.length > 0);

  const profiles =
    options.preferRelease === false ? ["debug", "release"] : ["release", "debug"];

  const candidates: string[] = [...(options.extraCandidates ?? [])];
  for (const targetDir of targetDirs) {
    for (const profile of profiles) {
      candidates.push(join(targetDir, profile, binaryName));
      // some overlay cargo setups only expose the artifact under deps/
      candidates.push(join(targetDir, profile, "deps", binaryName));
      candidates.push(join(targetDir, profile, "deps", depsBinaryName));
    }
  }

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  const installed = installedPlatformBinary(
    options.from === undefined ? {} : { from: options.from },
  );
  if (installed !== undefined) {
    return installed;
  }

  throw new Error(
    [
      `Could not find the MicroStudio runtime binary (${binaryName}).`,
      "Searched:",
      ...candidates.map((candidate) => `  ${candidate}`),
      "",
      "and this platform's prebuilt package, which is not installed. Install it, or:",
      installHint(process.platform, process.arch),
      "",
      "  cargo build --release --bin microstudio-runtime   # build it here",
      "  MICROSTUDIO_RUNTIME_BIN=...                       # use one you already have",
    ].join("\n"),
  );
}
