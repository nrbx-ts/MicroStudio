import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import type { PlaceEntry } from "@microstudio/runtime";

// Rojo project reader: maps compiled Luau onto DataModel paths

export interface RojoNode {
  name: string;
  className: string | null;
  // relative to the containing project file or mount
  path: string | null;
  // Rojo typed form, e.g. { "Bool": true }
  properties: Record<string, unknown>;
  // parsed but has no effect yet
  ignoreUnknownInstances: boolean;
  globIgnorePaths: string[];
  children: RojoNode[];
}

export interface RojoProject {
  file: string;
  // children are the top-level services
  root: RojoNode;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// strips UTF-8 BOM: Luau lexer and JSON.parse reject it
export function readText(file: string): string {
  const text = readFileSync(file, "utf8");
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

export function readRojoProject(file: string): RojoProject {
  const json: unknown = JSON.parse(readText(file));

  // v7 wraps the tree in a "tree" key; older files put it at top level
  const rootSource =
    isRecord(json) && isRecord(json["tree"]) ? json["tree"] : json;

  return { file: resolve(file), root: nodeFromJson("game", rootSource) };
}

function nodeFromJson(name: string, json: unknown): RojoNode {
  const record = isRecord(json) ? json : {};
  const children: RojoNode[] = [];
  for (const [key, value] of Object.entries(record)) {
    if (key.startsWith("$") || key === "globIgnorePaths") {
      continue;
    }
    children.push(nodeFromJson(key, value));
  }

  const globIgnorePaths = Array.isArray(record["globIgnorePaths"])
    ? record["globIgnorePaths"].map((pattern) => String(pattern))
    : [];

  return {
    name,
    className: typeof record["$className"] === "string" ? record["$className"] : null,
    path: typeof record["$path"] === "string" ? record["$path"] : null,
    properties: isRecord(record["$properties"]) ? record["$properties"] : {},
    ignoreUnknownInstances: record["$ignoreUnknownInstances"] === true,
    globIgnorePaths,
    children,
  };
}

export const DEFAULT_SERVICES = [
  "ServerScriptService",
  "ReplicatedStorage",
  "ServerStorage",
] as const;

export const ALL_SERVICES = [
  "Workspace",
  "Players",
  "ReplicatedStorage",
  "ServerStorage",
  "ServerScriptService",
  "CollectionService",
  "StarterPlayer",
  "StarterGui",
  "Lighting",
] as const;

// Rojo's default ignores; roblox-ts output contains both
const BUILTIN_IGNORES = ["package.json", "tsconfig.json"];

export function findProjectFile(dir: string): string | undefined {
  const preferred = join(dir, "default.project.json");
  try {
    if (statSync(preferred).isFile()) {
      return preferred;
    }
  } catch {
  }

  let candidates: string[];
  try {
    candidates = readdirSync(dir)
      .filter((name) => name.endsWith(".project.json"))
      .sort();
  } catch {
    return undefined;
  }
  const first = candidates[0];
  return first === undefined ? undefined : join(dir, first);
}

// maps a Rojo script filename to a class; non-scripts return undefined
export function classifyScriptFile(fileName: string): string | undefined {
  const lower = fileName.toLowerCase();
  if (lower === "init.luau" || lower === "init.lua") {
    return "ModuleScript";
  }
  if (lower.endsWith(".server.luau") || lower.endsWith(".server.lua")) {
    return "Script";
  }
  if (lower.endsWith(".client.luau") || lower.endsWith(".client.lua")) {
    return "LocalScript";
  }
  if (lower.endsWith(".luau") || lower.endsWith(".lua")) {
    return "ModuleScript";
  }
  return undefined;
}

export function scriptInstanceName(fileName: string): string {
  const lower = fileName.toLowerCase();
  for (const suffix of [".server.luau", ".server.lua", ".client.luau", ".client.lua", ".luau", ".lua"]) {
    if (lower.endsWith(suffix)) {
      return fileName.slice(0, fileName.length - suffix.length);
    }
  }
  return fileName;
}

export function isInitFileName(fileName: string): boolean {
  return initFileClass(fileName) !== undefined;
}

// init file turns its directory into that script class, not a Folder
export function initFileClass(fileName: string): string | undefined {
  switch (fileName.toLowerCase()) {
    case "init.luau":
    case "init.lua":
      return "ModuleScript";
    case "init.server.luau":
    case "init.server.lua":
      return "Script";
    case "init.client.luau":
    case "init.client.lua":
      return "LocalScript";
    default:
      return undefined;
  }
}

function isIgnored(name: string, extra: string[]): boolean {
  if (BUILTIN_IGNORES.includes(name)) {
    return true;
  }
  for (const pattern of extra) {
    const bare = pattern.replace(/^\*\*\//, "");
    if (bare === name) {
      return true;
    }
  }
  return false;
}

function findInitFile(dir: string): { file: string; className: string } | undefined {
  for (const candidate of [
    "init.luau",
    "init.lua",
    "init.server.luau",
    "init.server.lua",
    "init.client.luau",
    "init.client.lua",
  ]) {
    const full = join(dir, candidate);
    try {
      if (statSync(full).isFile()) {
        const className = initFileClass(candidate);
        if (className !== undefined) {
          return { file: full, className };
        }
      }
    } catch {
    }
  }
  return undefined;
}

// directories a watcher should watch; project file changes are the caller's job
export function mountDirectories(options: BuildPlaceOptions): string[] {
  const project = readRojoProject(options.projectFile);
  const projectDir = dirname(project.file);
  const services = options.services ?? DEFAULT_SERVICES;

  const dirs = new Set<string>();
  const visit = (node: RojoNode, inheritedDir: string): void => {
    const dir = node.path === null ? inheritedDir : resolve(inheritedDir, node.path);
    if (node.path !== null) {
      dirs.add(dir);
    }
    for (const child of node.children) {
      visit(child, dir);
    }
  };

  for (const service of services) {
    const node = project.root.children.find((child) => child.name === service);
    if (node !== undefined) {
      visit(node, projectDir);
    }
  }

  return [...dirs].sort();
}

export interface BuildPlaceOptions {
  projectFile: string;
  // defaults to the server-side set
  services?: readonly string[];
}

// flattens a Rojo project into parent-first loadTree entries
export function buildPlaceEntries(options: BuildPlaceOptions): PlaceEntry[] {
  const project = readRojoProject(options.projectFile);
  const projectDir = dirname(project.file);
  const services = options.services ?? DEFAULT_SERVICES;

  const out: PlaceEntry[] = [];
  const seen = new Set<string>();
  const rootNode = project.root;

  const emit = (entry: PlaceEntry): void => {
    if (seen.has(entry.path)) {
      return;
    }
    seen.add(entry.path);
    out.push(entry);
  };

  const walk = (dotted: string, node: RojoNode, inheritedDir: string): void => {
    const directoryBacked = node.path !== null;
    const dir = directoryBacked ? resolve(inheritedDir, node.path as string) : inheritedDir;

    let className = node.className;
    let source: string | undefined;
    let file: string | undefined;

    if (directoryBacked) {
      const init = findInitFile(dir);
      if (init !== undefined) {
        className = className ?? init.className;
        source = readText(init.file);
        file = init.file;
      } else {
        className = className ?? "Folder";
      }
    } else {
      className = className ?? "Folder";
    }

    emit({
      path: dotted,
      className,
      ...(source === undefined ? {} : { source }),
      ...(file === undefined ? {} : { file }),
      ...(Object.keys(node.properties).length === 0
        ? {}
        : { properties: node.properties }),
    });

    if (directoryBacked) {
      let entries: string[];
      try {
        entries = readdirSync(dir).sort();
      } catch {
        return;
      }

      for (const name of entries) {
        if (name.startsWith(".") || isIgnored(name, node.globIgnorePaths)) {
          continue;
        }
        const full = join(dir, name);
        let isDirectory = false;
        try {
          isDirectory = statSync(full).isDirectory();
        } catch {
          continue;
        }

        if (isDirectory) {
          walk(`${dotted}.${name}`, syntheticFolder(name, name), dir);
          continue;
        }

        const fileClass = classifyScriptFile(name);
        if (fileClass === undefined) {
          continue;
        }
        // init file already emitted as the directory's source
        if (isInitFileName(name)) {
          continue;
        }
        emit({
          path: `${dotted}.${scriptInstanceName(name)}`,
          className: fileClass,
          source: readText(full),
          file: full,
        });
      }
      return;
    }

    for (const child of node.children) {
      walk(`${dotted}.${child.name}`, child, dir);
    }
  };

  for (const service of services) {
    const node = rootNode.children.find((child) => child.name === service);
    if (node === undefined) {
      continue;
    }
    walk(service, node, projectDir);
  }

  return out;
}

// where roblox-ts copies RuntimeLib; a compiled spec requires it
export function rbxtsIncludeFolder(project: RojoProject): string {
  const projectDir = dirname(project.file);
  const found = findIncludeFolder(project.root, projectDir);
  return found ?? join(projectDir, "include");
}

function hasRuntimeLib(dir: string): boolean {
  try {
    return readdirSync(dir).some((name) => name.toLowerCase().startsWith("runtimelib"));
  } catch {
    return false;
  }
}

function findIncludeFolder(node: RojoNode, inheritedDir: string): string | undefined {
  const dir = node.path === null ? inheritedDir : resolve(inheritedDir, node.path);
  if (node.path !== null && (node.name === "rbxts_include" || hasRuntimeLib(dir))) {
    return dir;
  }
  for (const child of node.children) {
    const found = findIncludeFolder(child, dir);
    if (found !== undefined) {
      return found;
    }
  }
  return undefined;
}

function syntheticFolder(name: string, path: string): RojoNode {
  return {
    name,
    className: null,
    path,
    properties: {},
    ignoreUnknownInstances: false,
    globIgnorePaths: [],
    children: [],
  };
}
