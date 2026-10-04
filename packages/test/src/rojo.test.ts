import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  buildPlaceEntries,
  classifyScriptFile,
  findProjectFile,
  mountDirectories,
  readRojoProject,
  scriptInstanceName,
} from "@microstudio/roblox-ts";

// mirrors the real default.project.json shape: Rojo decides where compiled Luau lives
function makeFixture(): string {
  const dir = mkdtempSync(join(tmpdir(), "microstudio-rojo-"));
  const write = (relative: string, contents: string) => {
    const full = join(dir, relative);
    mkdirSync(join(full, ".."), { recursive: true });
    writeFileSync(full, contents);
  };

  write(
    "default.project.json",
    JSON.stringify(
      {
        name: "Fixture",
        tree: {
          $className: "DataModel",
          ServerScriptService: { TS: { $path: "out/server" } },
          ReplicatedStorage: {
            rbxts_include: {
              $path: "include",
              node_modules: { "@rbxts": { $path: "node_modules/@rbxts" } },
            },
            TS: { $path: "out/shared" },
          },
        },
      },
      null,
      2,
    ),
  );

  write("out/server/game.server.luau", "print('server')\n");
  write("out/server/chassis.luau", "return {}\n");
  // compiled output carries these; they must not become instances
  write("out/server/package.json", "{}");
  write("out/server/tsconfig.json", "{}");

  write("include/RuntimeLib.luau", "return {}\n");
  write("include/Promise.luau", "return {}\n");
  write("include/node_modules/@rbxts/services/out/init.luau", "return {}\n");

  write("out/shared/shared.server.luau", "print('shared server')\n");
  write("out/shared/util.luau", "return {}\n");

  return dir;
}

test("classifies Rojo script file names", () => {
  assert.equal(classifyScriptFile("game.server.luau"), "Script");
  assert.equal(classifyScriptFile("hud.client.luau"), "LocalScript");
  assert.equal(classifyScriptFile("util.luau"), "ModuleScript");
  assert.equal(classifyScriptFile("init.luau"), "ModuleScript");
  assert.equal(classifyScriptFile("legacy.lua"), "ModuleScript");
  assert.equal(classifyScriptFile("package.json"), undefined);
  assert.equal(classifyScriptFile("icon.png"), undefined);
  assert.equal(classifyScriptFile("types.d.ts"), undefined);
});

test("derives instance names from file names", () => {
  assert.equal(scriptInstanceName("game.server.luau"), "game");
  assert.equal(scriptInstanceName("hud.client.lua"), "hud");
  assert.equal(scriptInstanceName("util.luau"), "util");
});

test("finds the default project file", () => {
  const dir = makeFixture();
  try {
    assert.equal(findProjectFile(dir), join(dir, "default.project.json"));
    assert.equal(findProjectFile(join(dir, "does-not-exist")), undefined);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("reads the Rojo tree, including the v7 `tree` wrapper", () => {
  const dir = makeFixture();
  try {
    const project = readRojoProject(join(dir, "default.project.json"));
    assert.equal(project.root.className, "DataModel");

    const names = project.root.children.map((child) => child.name).sort();
    assert.deepEqual(names, ["ReplicatedStorage", "ServerScriptService"]);

    const serverScriptService = project.root.children.find(
      (child) => child.name === "ServerScriptService",
    );
    const ts = serverScriptService?.children.find((child) => child.name === "TS");
    assert.equal(ts?.path, "out/server");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("expands mounts parent-first, with the right classes", () => {
  const dir = makeFixture();
  try {
    const entries = buildPlaceEntries({
      projectFile: join(dir, "default.project.json"),
    });
    const paths = entries.map((entry) => entry.path);

    // parents always precede children
    for (const entry of entries) {
      const segments = entry.path.split(".");
      for (let index = 1; index < segments.length; index += 1) {
        const ancestor = segments.slice(0, index).join(".");
        assert.ok(
          paths.includes(ancestor),
          `${entry.path} was emitted before its ancestor ${ancestor}`,
        );
      }
    }

    const byPath = new Map(entries.map((entry) => [entry.path, entry]));

    // the nested TS folder is the shape that would be guessed wrong
    assert.equal(byPath.get("ServerScriptService.TS")?.className, "Folder");
    assert.equal(byPath.get("ServerScriptService.TS.game")?.className, "Script");
    assert.equal(byPath.get("ServerScriptService.TS.chassis")?.className, "ModuleScript");
    assert.match(byPath.get("ServerScriptService.TS.game")?.source ?? "", /print\('server'\)/);

    // RuntimeLib must be materialised or nothing runs
    assert.equal(byPath.get("ReplicatedStorage.rbxts_include")?.className, "Folder");
    assert.equal(byPath.get("ReplicatedStorage.rbxts_include.RuntimeLib")?.className, "ModuleScript");
    assert.equal(byPath.get("ReplicatedStorage.rbxts_include.Promise")?.className, "ModuleScript");
    assert.equal(
      byPath.get("ReplicatedStorage.rbxts_include.node_modules.@rbxts.services")?.className,
      "Folder",
    );
    // an `init.luau` turns the directory into the ModuleScript itself
    assert.equal(
      byPath.get("ReplicatedStorage.rbxts_include.node_modules.@rbxts.services.out")?.className,
      "ModuleScript",
    );

    // globIgnorePaths defaults exclude package.json/tsconfig.json
    assert.equal(paths.some((path) => path.endsWith("package")), false);
    assert.equal(paths.some((path) => path.endsWith("tsconfig")), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("treats a directory with init.luau as a ModuleScript", () => {
  const dir = makeFixture();
  try {
    writeFileSync(join(dir, "out", "shared", "init.luau"), "return { init = true }\n");
    const entries = buildPlaceEntries({
      projectFile: join(dir, "default.project.json"),
    });
    const ts = entries.find((entry) => entry.path === "ReplicatedStorage.TS");
    assert.equal(ts?.className, "ModuleScript");
    assert.match(ts?.source ?? "", /init = true/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("init.server.luau and init.client.luau set the directory's class", () => {
  const dir = makeFixture();
  try {
    writeFileSync(join(dir, "out", "server", "init.server.luau"), "print('boot')\n");
    writeFileSync(join(dir, "out", "shared", "init.client.luau"), "print('ui')\n");
    const entries = buildPlaceEntries({
      projectFile: join(dir, "default.project.json"),
    });
    const byPath = new Map(entries.map((entry) => [entry.path, entry]));
    assert.equal(byPath.get("ServerScriptService.TS")?.className, "Script");
    assert.equal(byPath.get("ReplicatedStorage.TS")?.className, "LocalScript");
    // the init file is the directory's own source, so no child named `init`
    assert.equal(byPath.has("ServerScriptService.TS.init"), false);
    // siblings still land as children of the script
    assert.equal(byPath.get("ServerScriptService.TS.chassis")?.className, "ModuleScript");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("lists the directories a project reads from", () => {
  const dir = makeFixture();
  try {
    const projectFile = join(dir, "default.project.json");
    // --watch reloads on these, so they are mount targets, not the project root
    // (which would drag in node_modules and the whole source tree)
    assert.deepEqual(mountDirectories({ projectFile }), [
      join(dir, "include"),
      join(dir, "include", "node_modules", "@rbxts"),
      join(dir, "out", "server"),
      join(dir, "out", "shared"),
    ]);

    assert.deepEqual(mountDirectories({ projectFile, services: ["ServerScriptService"] }), [
      join(dir, "out", "server"),
    ]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("only descends into the requested services", () => {
  const dir = makeFixture();
  try {
    const entries = buildPlaceEntries({
      projectFile: join(dir, "default.project.json"),
      services: ["ServerScriptService"],
    });
    assert.ok(entries.every((entry) => entry.path.startsWith("ServerScriptService")));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
