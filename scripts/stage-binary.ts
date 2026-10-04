#!/usr/bin/env node
// build one platform package: manifest plus the sidecar in bin/
// no dependencies on purpose, a release job has to run it as it is

import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
} from "node:fs";
import { join, resolve } from "node:path";

import {
  PLATFORM_PACKAGE_PREFIX,
  TARGETS,
  findTargetByName,
} from "../packages/runtime/src/targets.ts";
import {
  STAGING_DIR,
  flagArgs,
  publishRepository,
  releaseVersion,
  repoRoot,
  writeManifest,
} from "./manifest.ts";

const USAGE = `\
stage-binary -- build the publishable platform packages

USAGE:
  stage --target <name> [--binary <path>] [--out <dir>] [--version <v>]

COMMANDS:
  stage  Assemble one platform package: its manifest, and the binary in bin/.
         --binary defaults to target/release/<binary>, which is where a native
         cargo build puts it. --version defaults to the root package.json.

TARGETS:
${TARGETS.map((target) => `${target.name.padEnd(13)} ${target.triple}`).join("\n")}
`;

function main(argv: readonly string[]): number {
  const { command, values } = flagArgs(argv);
  if (command !== "stage") {
    process.stdout.write(USAGE);
    return command === "" || command === "--help" ? 0 : 2;
  }

  const name = values.get("target");
  if (name === undefined) {
    throw new Error("stage needs --target.");
  }
  const target = findTargetByName(name);
  if (target === undefined) {
    throw new Error(
      `Unknown target '${name}'. Known targets: ${TARGETS.map((entry) => entry.name).join(", ")}.`,
    );
  }

  const version = releaseVersion(values.get("version"));
  const binary = resolve(
    values.get("binary") ?? join(repoRoot, "target", "release", target.binary),
  );
  if (!existsSync(binary)) {
    throw new Error(
      `No binary at ${binary}. Build one first (cargo build --release --bin microstudio-runtime), or pass --binary.`,
    );
  }

  const packageName = `${PLATFORM_PACKAGE_PREFIX}${target.name}`;
  const packageDir = resolve(
    values.get("out") ?? STAGING_DIR,
    ...packageName.split("/"),
  );
  mkdirSync(join(packageDir, "bin"), { recursive: true });

  // os/cpu make an install skip the four packages that are not this machine
  // no exports map: the resolver reads bin/<binary> by path
  writeManifest(join(packageDir, "package.json"), {
    name: packageName,
    version,
    description: `The MicroStudio runtime sidecar for ${target.name} (${target.triple}).`,
    license: "MIT",
    // npm checks this before signing provenance, sidecar comes from the runtime package
    repository: publishRepository("packages/runtime"),
    os: [target.platform],
    cpu: [target.arch],
    files: ["bin"],
  });

  const staged = join(packageDir, "bin", target.binary);
  copyFileSync(binary, staged);
  // npm packs the mode it finds, so keep it executable (no-op on windows)
  chmodSync(staged, 0o755);

  console.log(`${packageName}@${version}`);
  console.log(`  binary: ${binary}`);
  console.log(`  staged: ${packageDir}`);
  return 0;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
