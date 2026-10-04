// prebuilt sidecar per platform; the published manifest lists them as optional deps
export interface RuntimeTarget {
  // package suffix; also the value --target takes
  readonly name: string;
  readonly platform: string;
  readonly arch: string;
  // cargo triple, used by a local build
  readonly triple: string;
  // file name inside the platform package's bin/
  readonly binary: string;
}

export const PLATFORM_PACKAGE_PREFIX = "@microstudio/runtime-";

export const TARGETS: readonly RuntimeTarget[] = [
  {
    name: "linux-x64",
    platform: "linux",
    arch: "x64",
    triple: "x86_64-unknown-linux-gnu",
    binary: "microstudio-runtime",
  },
  {
    name: "linux-arm64",
    platform: "linux",
    arch: "arm64",
    triple: "aarch64-unknown-linux-gnu",
    binary: "microstudio-runtime",
  },
  {
    name: "darwin-x64",
    platform: "darwin",
    arch: "x64",
    triple: "x86_64-apple-darwin",
    binary: "microstudio-runtime",
  },
  {
    name: "darwin-arm64",
    platform: "darwin",
    arch: "arm64",
    triple: "aarch64-apple-darwin",
    binary: "microstudio-runtime",
  },
  {
    name: "win32-x64",
    platform: "win32",
    arch: "x64",
    triple: "x86_64-pc-windows-msvc",
    binary: "microstudio-runtime.exe",
  },
  {
    name: "win32-arm64",
    platform: "win32",
    arch: "arm64",
    triple: "aarch64-pc-windows-msvc",
    binary: "microstudio-runtime.exe",
  },
];

export function targetName(platform: string, arch: string): string {
  return `${platform}-${arch}`;
}

export function findTarget(
  platform: string,
  arch: string,
): RuntimeTarget | undefined {
  const name = targetName(platform, arch);
  return TARGETS.find((target) => target.name === name);
}

export function findTargetByName(name: string): RuntimeTarget | undefined {
  return TARGETS.find((target) => target.name === name);
}

export function platformPackageName(
  platform: string,
  arch: string,
): string | undefined {
  const target = findTarget(platform, arch);
  return target === undefined
    ? undefined
    : `${PLATFORM_PACKAGE_PREFIX}${target.name}`;
}
