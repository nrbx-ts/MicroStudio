// directory walk + tiny glob matcher: no runtime dependencies

import { readdirSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

export const DEFAULT_INCLUDE = [
  "**/*.spec.luau",
  "**/*.spec.lua",
  "**/*.test.luau",
  "**/*.test.lua",
] as const;

// TypeScript specs, compiled by the project's roblox-ts before they run
export const DEFAULT_TS_INCLUDE = [
  "**/*.spec.ts",
  "**/*.test.ts",
  "**/*.spec.tsx",
  "**/*.test.tsx",
] as const;

export const DEFAULT_EXCLUDE = [
  "**/node_modules/**",
  "**/.git/**",
  "**/.microstudio/**",
  "**/target/**",
  "**/dist/**",
] as const;

export interface DiscoverOptions {
  include?: readonly string[];
  exclude?: readonly string[];
}

// only `{a,b}` alternatives are expanded; the rest is left alone
function expandBraces(pattern: string): string[] {
  const open = pattern.indexOf("{");
  if (open === -1) {
    return [pattern];
  }
  const close = pattern.indexOf("}", open);
  if (close === -1) {
    return [pattern];
  }
  const head = pattern.slice(0, open);
  const tail = pattern.slice(close + 1);
  const body = pattern.slice(open + 1, close);
  return body
    .split(",")
    .flatMap((part) => expandBraces(`${head}${part}${tail}`));
}

function escapeRegExp(text: string): string {
  return text.replace(/[.+^${}()|[\]\\]/g, "\\$&");
}

// anchored regex over `/`-separated paths
export function globToRegExp(pattern: string): RegExp {
  const normalised = pattern.replaceAll("\\", "/");
  let out = "^";

  for (let index = 0; index < normalised.length; index += 1) {
    const char = normalised[index];
    if (char === "*") {
      if (normalised[index + 1] === "*") {
        // `**/` spans any number of directories, including none
        if (normalised[index + 2] === "/") {
          out += "(?:.*/)?";
          index += 2;
        } else {
          out += ".*";
          index += 1;
        }
      } else {
        out += "[^/]*";
      }
    } else if (char === "?") {
      out += "[^/]";
    } else {
      out += escapeRegExp(char ?? "");
    }
  }

  return new RegExp(`${out}$`);
}

export function matchesPattern(path: string, pattern: string): boolean {
  const normalised = path.replaceAll("\\", "/");
  return expandBraces(pattern).some((expanded) =>
    globToRegExp(expanded).test(normalised),
  );
}

function matchesAny(path: string, patterns: readonly string[]): boolean {
  return patterns.some((pattern) => matchesPattern(path, pattern));
}

// absolute paths, sorted so a run is reproducible
export function discoverSpecs(
  root: string,
  options: DiscoverOptions = {},
): string[] {
  const start = resolve(root);
  const include = options.include ?? DEFAULT_INCLUDE;
  const exclude = options.exclude ?? DEFAULT_EXCLUDE;
  const found: string[] = [];

  const walk = (dir: string): void => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }

    for (const entry of entries) {
      const full = join(dir, entry.name);
      const rel = relative(start, full).split(sep).join("/");
      if (matchesAny(rel, exclude)) {
        continue;
      }
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile() && matchesAny(rel, include)) {
        found.push(isAbsolute(full) ? full : resolve(full));
      }
    }
  };

  walk(start);
  return found.sort();
}
