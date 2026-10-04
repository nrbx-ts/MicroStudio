import assert from "node:assert/strict";
import { test } from "node:test";

import { asArray, asNumber, asString, createTestRuntime, withRuntime } from "./index.ts";

test("boots and reports its services", async () => {
  await withRuntime(async (runtime) => {
    const services = await runtime.listServices();
    assert.ok(services.includes("ServerScriptService"));
    assert.ok(services.includes("ReplicatedStorage"));
    // the whole API dump is available, not just the services the runtime creates up front
    assert.ok(services.includes("TweenService"));
    assert.ok(services.includes("DataStoreService"));

    // the runtime reports its own Luau pin, so this checks the linked revision
    assert.match(runtime.luauVersion, /^Luau \d+\.\d+$/);
    assert.equal(runtime.luauVersion, await runtime.eval("return _VERSION"));
    const stats = await runtime.stats();
    assert.equal(stats.clock, "virtual");
    assert.equal(stats.time, 0);
  });
});

test("creates and manipulates instances, and resolves children by name", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval(`
      local part = Instance.new("Part")
      part.Name = "TestPart"
      part.Parent = workspace
    `);

    const children = asArray<{ name: string; className: string }>(
      await runtime.eval("return workspace:GetChildren()"),
    );
    assert.equal(children.length, 1);
    assert.equal(children[0]?.name, "TestPart");
    assert.equal(children[0]?.className, "Part");

    // `workspace.TestPart` is the idiom real projects use
    assert.equal(asString(await runtime.eval("return workspace.TestPart.Name")), "TestPart");
    // workspace descendants include the part itself; the part's own are empty
    assert.equal(asNumber(await runtime.eval("return #workspace:GetDescendants()")), 1);
    assert.equal(
      asNumber(await runtime.eval("return #workspace.TestPart:GetDescendants()")),
      0,
    );

    const inspected = await runtime.inspect("game.Workspace.TestPart");
    assert.equal(inspected.className, "Part");
    assert.equal(inspected.path, "game.Workspace.TestPart");
  });
});

test("instance identity is stable, so equality works like Roblox", async () => {
  await withRuntime(async (runtime) => {
    assert.equal(
      await runtime.eval(`
        local part = Instance.new("Part")
        part.Parent = workspace
        return part == workspace.Part
      `),
      true,
    );
  });
});

test("datatypes behave: construction, arithmetic, equality, typeof", async () => {
  await withRuntime(async (runtime) => {
    assert.equal(asNumber(await runtime.eval("return (Vector3.new(1, 2, 3) + Vector3.new(1, 1, 1)).Y")), 3);
    assert.equal(asNumber(await runtime.eval("return Vector3.new(3, 4, 0).Magnitude")), 5);
    assert.equal(await runtime.eval("return Vector3.new(1, 2, 3) == Vector3.new(1, 2, 3)"), true);
    assert.equal(await runtime.eval("return Vector3.new(1, 2, 3) == Vector3.new(1, 2, 4)"), false);
    assert.equal(asString(await runtime.eval("return typeof(Vector3.new())")), "Vector3");
    assert.equal(asString(await runtime.eval("return tostring(Vector3.new(1, 2, 3))")), "1, 2, 3");
    assert.equal(asNumber(await runtime.eval("return CFrame.Angles(0, math.pi / 2, 0).LookVector.X")), -1);
    assert.equal(asString(await runtime.eval("return Color3.fromRGB(255, 0, 128):ToHex()")), "#FF0080");
  });
});

test("properties are typed and attribute-like access is validated", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval(`
      local part = Instance.new("Part")
      part.Size = Vector3.new(1, 2, 3)
      part.Parent = workspace
      part:SetAttribute("speed", 12)
      part:SetAttribute("label", "loco")
    `);
    assert.equal(asNumber(await runtime.eval("return workspace.Part.Size.Z")), 3);
    assert.equal(asNumber(await runtime.eval("return workspace.Part:GetAttribute('speed')")), 12);
    assert.equal(asString(await runtime.eval("return workspace.Part:GetAttribute('label')")), "loco");

    // a bad property name errors, it is not a silent no-op
    const errors = await runtime.eval(`
      local ok, err = pcall(function() workspace.Part.NotAProperty = 1 end)
      return tostring(err)
    `);
    assert.match(asString(errors), /not a valid member of Part/);
  });
});

test("signals connect, fire in order, and disconnect", async () => {
  await withRuntime(async (runtime) => {
    const order = await runtime.eval(`
      local folder = Instance.new("Folder")
      folder.Parent = workspace
      local seen = {}
      local first = folder.ChildAdded:Connect(function(child) table.insert(seen, "a:" .. child.Name) end)
      folder.ChildAdded:Connect(function(child) table.insert(seen, "b:" .. child.Name) end)

      local p = Instance.new("Part") p.Name = "One" p.Parent = folder
      first:Disconnect()
      local q = Instance.new("Part") q.Name = "Two" q.Parent = folder
      return seen
    `);
    // only the first listener was disconnected; the second fires for both parts
    assert.deepEqual(asArray<string>(order), ["a:One", "b:One", "b:Two"]);
  });
});

test("Once listeners fire exactly once", async () => {
  await withRuntime(async (runtime) => {
    const count = await runtime.eval(`
      local folder = Instance.new("Folder")
      folder.Parent = workspace
      local n = 0
      folder.ChildAdded:Once(function() n = n + 1 end)
      local a = Instance.new("Part") a.Parent = folder
      local b = Instance.new("Part") b.Parent = folder
      return n
    `);
    assert.equal(count, 1);
  });
});

test("signals fire synchronously, before the assignment returns", async () => {
  await withRuntime(async (runtime) => {
    const observed = await runtime.eval(`
      local folder = Instance.new("Folder")
      folder.Parent = workspace
      local fired = false
      folder.ChildAdded:Connect(function() fired = true end)
      local p = Instance.new("Part")
      p.Parent = folder
      return fired
    `);
    assert.equal(observed, true);
  });
});

test("Signal:Wait resumes the waiting thread", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval(`
      _G.__arrived = nil
      local folder = Instance.new("Folder")
      folder.Parent = workspace
      task.spawn(function()
        local child = folder.ChildAdded:Wait()
        _G.__arrived = child.Name
      end)
    `);
    assert.equal(await runtime.eval("return _G.__arrived"), null);

    await runtime.eval("local p = Instance.new('Part') p.Name = 'Arrived' p.Parent = workspace.Folder");
    assert.equal(asString(await runtime.eval("return _G.__arrived")), "Arrived");
  });
});

test("task.delay is deterministic under a virtual clock", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval(`
      _G.__log = {}
      task.delay(5, function() table.insert(_G.__log, "five") end)
      task.delay(1, function() table.insert(_G.__log, "one") end)
    `);

    assert.deepEqual(asArray(await runtime.eval("return _G.__log")), []);

    // not yet due
    await runtime.advanceTime(0.5);
    assert.deepEqual(asArray(await runtime.eval("return _G.__log")), []);

    // only the 1s timer fires; the 5s one is still pending
    await runtime.advanceTime(0.6);
    assert.deepEqual(asArray<string>(await runtime.eval("return _G.__log")), ["one"]);

    await runtime.advanceTime(10);
    assert.deepEqual(asArray<string>(await runtime.eval("return _G.__log")), ["one", "five"]);
  });
});

test("task.wait returns elapsed simulated time", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval(`
      _G.__elapsed = nil
      task.spawn(function()
        _G.__elapsed = task.wait(2)
      end)
    `);
    await runtime.advanceTime(2);
    assert.equal(asNumber(await runtime.eval("return _G.__elapsed")), 2);
  });
});

test("task.spawn runs immediately and task.defer waits for the cycle", async () => {
  await withRuntime(async (runtime) => {
    const log = await runtime.eval(`
      local log = {}
      task.spawn(function() table.insert(log, "spawn") end)
      table.insert(log, "main")
      task.defer(function() table.insert(log, "defer") end)
      return log
    `);
    // spawn is synchronous; defer runs at the end of the cycle
    assert.deepEqual(asArray<string>(log), ["spawn", "main"]);
  });
});

test("events fire listeners synchronously", async () => {
  await withRuntime(async (runtime) => {
    const result = await runtime.eval(`
      _G.__events = {}
      game:GetService("Players").PlayerAdded:Connect(function(player)
        table.insert(_G.__events, "added:" .. player.Name)
      end)
      game:GetService("Players").PlayerRemoving:Connect(function(player)
        table.insert(_G.__events, "removing:" .. player.Name)
      end)
      return true
    `);
    assert.equal(result, true);

    const player = await runtime.players.addMockPlayer("Eddie");
    assert.equal(player.className, "Player");
    assert.equal(player.path, "game.Players.Eddie");

    assert.deepEqual(asArray<string>(await runtime.eval("return _G.__events")), [
      "added:Eddie",
    ]);
    assert.equal(
      asString(await runtime.eval("return game:GetService('Players'):GetPlayers()[1].Name")),
      "Eddie",
    );
  });
});

test("mock players can spawn a character", async () => {
  await withRuntime(async (runtime) => {
    const player = await runtime.players.addMockPlayer("Eddie", { withCharacter: true });
    assert.equal(player.name, "Eddie");

    const character = await runtime.inspect("game.Workspace.Eddie");
    assert.equal(character.className, "Model");

    const humanoid = await runtime.inspect("game.Workspace.Eddie.Humanoid");
    assert.equal(humanoid.className, "Humanoid");
  });
});

test("RunService reports a server-side session", async () => {
  await withRuntime(async (runtime) => {
    assert.equal(await runtime.eval("return game:GetService('RunService'):IsServer()"), true);
    assert.equal(await runtime.eval("return game:GetService('RunService'):IsClient()"), false);
    assert.equal(await runtime.eval("return game:GetService('RunService'):IsRunning()"), true);
  });
});

test("CollectionService tags instances and fires tag signals", async () => {
  await withRuntime(async (runtime) => {
    const tagged = await runtime.eval(`
      local cs = game:GetService("CollectionService")
      local part = Instance.new("Part")
      part.Name = "Checkpoint"
      part.Parent = workspace
      local seen = {}
      cs:GetInstanceAddedSignal("cp"):Connect(function(instance) table.insert(seen, instance.Name) end)
      cs:AddTag(part, "cp")
      return { seen = seen, tagged = #cs:GetTagged("cp"), has = part:HasTag("cp") }
    `) as { seen: string[]; tagged: number; has: boolean };
    assert.deepEqual(tagged.seen, ["Checkpoint"]);
    assert.equal(tagged.tagged, 1);
    assert.equal(tagged.has, true);

    // tag methods exist on the service and the instance; different arities
    const both = await runtime.eval(`
      local cs = game:GetService("CollectionService")
      local other = Instance.new("Part") other.Name = "Other"
      other.Parent = workspace
      other:AddTag("cp")
      local viaInstance = other:GetTags()
      local viaService = cs:GetTags(other)
      cs:RemoveTag(other, "cp")
      return { instance = viaInstance[1], service = viaService[1],
               removed = other:HasTag("cp"), remaining = #cs:GetTagged("cp") }
    `) as { instance: string; service: string; removed: boolean; remaining: number };
    assert.equal(both.instance, "cp");
    assert.equal(both.service, "cp");
    assert.equal(both.removed, false);
    assert.equal(both.remaining, 1);
  });
});

test("eval can compile a line as an expression for the REPL", async () => {
  await withRuntime(async (runtime) => {
    // `statement` rejects a bare expression, which is why the REPL asks for `auto`
    const autoWorkspace = (await runtime.eval("workspace", { mode: "auto" })) as {
      __type: string;
      name: string;
      className: string;
      path: string;
    };
    assert.equal(autoWorkspace.__type, "Instance");
    assert.equal(autoWorkspace.name, "Workspace");
    assert.equal(autoWorkspace.className, "Workspace");
    assert.equal(autoWorkspace.path, "game.Workspace");

    assert.equal(await runtime.eval("1 + 1", { mode: "expression" }), 2);
    assert.equal(await runtime.eval("local x = 2 return x * 3", { mode: "auto" }), 6);
    // a statement runs but reports no value
    assert.equal(await runtime.eval("local unused = 1", { mode: "auto" }), null);
  });
});

test("empty tables cross the JSON boundary as arrays", async () => {
  await withRuntime(async (runtime) => {
    // Lua cannot tell `{}` from an empty array, so the boundary picks `[]`
    // rather than `{}` for JavaScript consumers
    assert.deepEqual(await runtime.eval("return {}"), []);
    assert.deepEqual(await runtime.eval("return {1, 2}"), [1, 2]);
    assert.deepEqual(await runtime.eval("return { a = 1 }"), { a: 1 });
  });
});

test("errors are reported with script, line and stack trace", async () => {
  await withRuntime(async (runtime) => {
    await runtime.runSource(
      "TrainController",
      ["local train = nil", "return train.Position"].join("\n"),
    );

    const errors = runtime.errors();
    assert.equal(errors.length, 1);
    const [error] = errors;
    assert.ok(error, "expected an error to be recorded");
    assert.match(error.location, /TrainController:2/);
    assert.match(error.message, /attempt to index nil/);
    assert.match(error.rendered, /^\[server\] /);
    assert.match(error.rendered, /Error: /);
  });
});

test("eval reports errors without rejecting the command", async () => {
  await withRuntime(async (runtime) => {
    await runtime.eval("local t = nil return t.Position");
    assert.equal(runtime.errors().length, 1);
    // the runtime is still usable afterwards
    assert.equal(await runtime.eval("return 1 + 1"), 2);
  });
});

test("a server script runs against the live DataModel", async () => {
  await withRuntime(async (runtime) => {
    await runtime.loadTree([
      { path: "ServerScriptService", className: "Folder" },
      {
        path: "ServerScriptService.Bootstrap",
        className: "Script",
        source: `
          local folder = Instance.new("Folder")
          folder.Name = "FromScript"
          folder.Parent = workspace
          print("bootstrap done")
        `,
      },
    ]);

    const ran = await runtime.run();
    assert.deepEqual(ran, ["ServerScriptService.Bootstrap"]);

    const folder = await runtime.inspect("game.Workspace.FromScript");
    assert.equal(folder.className, "Folder");
    assert.ok(runtime.logs().some((log) => log.text === "bootstrap done"));
  });
});

test("nested project folders land where Rojo says they should", async () => {
  await withRuntime(async (runtime) => {
    await runtime.loadTree([
      { path: "ServerScriptService.TS.server", className: "Script", source: "return 1" },
      { path: "ReplicatedStorage.TS.shared", className: "ModuleScript", source: "return { a = 1 }" },
    ]);

    assert.equal((await runtime.inspect("game.ServerScriptService.TS.server")).className, "Script");
    assert.equal(
      (await runtime.inspect("game.ReplicatedStorage.TS.shared")).className,
      "ModuleScript",
    );

    // modules are require-able; the cache is per instance
    const value = await runtime.eval(`
      local shared = game:GetService("ReplicatedStorage"):FindFirstChild("TS").shared
      return require(shared).a + require(shared).a
    `);
    assert.equal(value, 2);
  });
});

test("a script named `game` keeps its own path segment", async () => {
  await withRuntime(async (runtime) => {
    // only the leading `game` is the root; stripping every `game` segment
    // would collapse an entry named `game` onto its parent folder
    await runtime.loadTree([
      {
        path: "ServerScriptService.TS.game",
        className: "Script",
        source: "print('the game script ran')",
      },
      {
        path: "ReplicatedStorage.TS.game",
        className: "ModuleScript",
        source: "return { nested = true }",
      },
    ]);

    const script = await runtime.inspect("game.ServerScriptService.TS.game");
    assert.equal(script.className, "Script");
    assert.equal(script.path, "game.ServerScriptService.TS.game");

    assert.equal(
      await runtime.eval("return require(game.ReplicatedStorage.TS.game).nested"),
      true,
    );

    // a collapsed entry would also fail loadTree: the parent has no Source
    const ran = await runtime.run();
    assert.deepEqual(ran, ["ServerScriptService.TS.game"]);
    assert.ok(runtime.logs().some((log) => log.text === "the game script ran"));
  });
});

test("module requires are cached like Roblox", async () => {
  await withRuntime(async (runtime) => {
    await runtime.loadTree([
      {
        path: "ReplicatedStorage.Counter",
        className: "ModuleScript",
        source: "return { n = (math.random() and 0) or 0 }",
      },
    ]);
    const same = await runtime.eval(`
      local a = game:GetService("ReplicatedStorage").Counter
      local b = game:GetService("ReplicatedStorage").Counter
      return require(a) == require(b)
    `);
    assert.equal(same, true);
  });
});

test("re-running a project in a fresh runtime does not duplicate state", async () => {
  // dev --watch replaces the sidecar rather than patching it: re-running on
  // the old world would duplicate instances and leave stale listeners
  const entries = [
    {
      path: "ServerScriptService.Bootstrap",
      className: "Script",
      source: `
        local folder = Instance.new("Folder")
        folder.Name = "FromScript"
        folder.Parent = workspace
        print("booted")
      `,
    },
  ];

  const boot = async () => {
    const runtime = await createTestRuntime();
    await runtime.loadTree(entries);
    await runtime.run();
    return runtime;
  };

  const first = await boot();
  try {
    assert.equal(await first.eval("return #workspace:GetChildren()"), 1);
  } finally {
    await first.dispose();
  }

  const second = await boot();
  try {
    assert.equal(await second.eval("return #workspace:GetChildren()"), 1);
    assert.equal(await second.eval("return workspace.FromScript.ClassName"), "Folder");
    assert.deepEqual(
      second.logs().map((log) => log.text),
      ["booted"],
      "a replaced runtime carries nothing over",
    );
  } finally {
    await second.dispose();
  }
});

test("a real clock keeps running between calls", async () => {
  // dev relies on this: on wall time timers only fire when the scheduler is
  // pumped, so the session pumps on an interval while the prompt is idle
  const runtime = await createTestRuntime({ clock: "real" });
  try {
    await runtime.eval("task.delay(0.05, function() print('fired while idle') end)");
    assert.deepEqual(
      runtime.logs().map((log) => log.text),
      [],
      "nothing is due yet",
    );

    await new Promise((resolvePromise) => setTimeout(resolvePromise, 90));
    await runtime.pump();
    assert.deepEqual(
      runtime.logs().map((log) => log.text),
      ["fired while idle"],
    );

    // a prompt-driven eval still sees the same live world
    assert.equal(await runtime.eval("return 2 + 2"), 4);
  } finally {
    await runtime.dispose();
  }
});

test("the clock is virtual, so nothing waits in wall time", async () => {
  const started = Date.now();
  await withRuntime(async (runtime) => {
    await runtime.eval("task.delay(3600, function() print('an hour later') end)");
    await runtime.advanceTime(3600);
    assert.ok(runtime.logs().some((log) => log.text === "an hour later"));
  });
  assert.ok(Date.now() - started < 30_000, "an hour of simulated time must not take an hour");
});

test("dispose is idempotent and stops the sidecar", async () => {
  const runtime = await createTestRuntime();
  await runtime.dispose();
  await runtime.dispose();
  await assert.rejects(() => runtime.eval("return 1"));
});
