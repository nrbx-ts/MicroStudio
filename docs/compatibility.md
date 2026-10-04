# Compatibility

Two things are documented here: the **exact pinned versions** MicroStudio runs
against, and **every known deviation** from Roblox. The second list is the one
to read before porting a real project; it is written to be boringly honest
rather than reassuring.

## Pinned versions

| Component | Version | How it is pinned |
| --- | --- | --- |
| Luau | **0.740** | `mlua` 0.12.2 → `mlua-sys` 0.13.0 → `luau0-src` 0.22.0+luau740, built from source |
| Lua binding layer | mlua 0.12.2 | `[workspace.dependencies]` in `Cargo.toml`, features `luau`, `vendored`, `serialize`, `macros` |
| Luau runtime | none required | `vendored` compiles Luau from source; no system Lua is used or needed |
| Rust | 1.88 MSRV | `rust-version` in `Cargo.toml`; edition 2021 |
| Node.js | 22.18+ | `engines`; Node strips TypeScript natively, so there is no build step |
| JSON-RPC protocol | v1 | `PROTOCOL_VERSION` in `crates/microstudio-runtime/src/protocol.rs` |

The Luau revision is not a claim in a manifest — it is read back from the
running VM, because mlua's build script sets `_VERSION` from the compiled
source:

```console
$ node -e "import('@microstudio/runtime').then(async (m) => {
    const rt = await m.createRuntime();
    console.log(rt.luauVersion);
    await rt.dispose();
  })"
Luau 0.740
```

`hello` reports the same string, so a driver can refuse to run against a
different Luau than it was tested with. The protocol handshake already does this
for `PROTOCOL_VERSION`.

Roblox itself does not publish which Luau commit a given client ships, so
"matches Roblox" cannot be asserted by version number; it is asserted by the
tests in this repository instead.

## What is faithful

These are the semantics a port relies on, and each has tests behind it:

- **Signals fire synchronously and in connection order.** A `ChildAdded`
  listener runs before the `Parent =` assignment returns. `Once` disconnects
  itself before invoking. `Disconnect` takes effect from the next fire.
- **Instance identity is `==`-stable.** `workspace.Part == workspace.Part`
  holds, because instances are cached per id.
- **`require` caches per ModuleScript**, so two `require`s of the same module
  return the same table.
- **`task.wait` returns the elapsed time**, and `task.spawn` / `defer` / `delay`
  / `cancel` follow Roblox's ordering rules. The legacy globals `wait`,
  `spawn`, `delay` exist and forward to `task.*`.
- **Errors carry `[server] Location:line`, a message and a stack traceback**,
  in Roblox's shape.
- **`tostring` formats** match Roblox: `Vector3` as `1, 2, 3`, `Color3` with
  three decimals (`1, 0.502, 0`), `UDim`/`UDim2` as `{1, 2}`, `CFrame` as all
  twelve components, and `BrickColor` as its palette name, which is what Roblox
  prints. `print` renders datatypes through `__tostring` rather than showing
  `userdata`, and a datatype returned from `eval` crosses the JSON boundary in
  the same form.
- **`Instance.new` rejects abstract and non-creatable classes**, and members
  that do not exist on a class produce Roblox's "not a valid member of X".
- **`GetService` is only valid on `game`.**
- **Only enabled `Script`s under `ServerScriptService` are run**; a `Disabled`
  script is skipped, and `LocalScript`s are never executed.
- **Unknown `BrickColor` numbers keep their number** instead of being lost.

## Deviations

### 1. Datatype components are `f64`, not `f32`

`Vector3`, `CFrame` and friends store `f64` components. Roblox computes in
`f32`, so long chains of arithmetic can diverge in the last decimal places.

*Impact:* comparisons against literal expected values are usually fine; bitwise
comparisons of accumulated transforms are not. This is deliberate — the simpler
maths was worth more than bit-exactness for a local runtime — but it is a real
difference.

### 2. An empty Lua table crosses the JSON boundary as `[]`, not `{}`

Lua cannot distinguish `{}` from an empty array. MicroStudio picks the array
shape, because `[]` is far more useful to a JavaScript caller than `{}` and a
round-trip is then lossless in the direction people actually use.

*Impact:* a genuine empty *record* comes back as `[]`. Non-empty tables are
unambiguous and unaffected.

### 2b. datatypes cross the JSON boundary as strings

`runtime.eval("return Vector3.new(1, 2, 3)")` gives `"1, 2, 3"`, not a
structured object with `x`/`y`/`z`. It is the same rendering Roblox's `tostring`
produces, so it reads correctly at a prompt and prints correctly in a script,
but a JavaScript caller that wants the components has to parse it (or read
`.X`/`.Y`/`.Z` in Luau, which is usually what you wanted anyway). Instances are
the exception: they cross as `{__type: "Instance", id, name, className, path}`.

### 3. `BrickColor` is a subset

Roblox ships several hundred palette entries and the list is not in the public
API dump. MicroStudio carries 39 hand-checked entries. An unknown number is
preserved by `:Number()` but renders as a neutral grey, and `:Name()` returns
`"Unknown"`.

*Impact:* `BrickColor.new("Alder")` errors; `BrickColor.fromColor3` snaps to the
nearest *known* entry.

### 4. `Enum` item values are synthesised

`Enum.<Type>.<Item>` resolves dynamically: the first time an item name is seen
it is created with the next sequential `Value`, starting at 0.

*Impact:* `Enum.Material.Concrete.Name` and `tostring` are correct, but
`Enum.Material.Concrete.Value` is **not** Roblox's number, and the numbers
depend on access order. Do not persist or compare enum values numerically.

### 5. No `Changed` / `GetPropertyChangedSignal`

Only `AttributeChanged` exists as a property-level signal. `Instance.Changed`,
`:GetPropertyChangedSignal()` and `:GetPropertyChangedSignal("Source")` are not
implemented.

*Impact:* the common `part.Changed:Connect(...)` idiom fails as an invalid
member. Attribute-based change detection works.

### 6. Frame events are connectable but never fire

`RunService.Heartbeat`, `Stepped`, `RenderStepped`, `PreSimulation` and
`PostSimulation` exist as events, so connecting to them does not error, but
**nothing fires them**. There is no frame loop: scheduled work runs when
`advanceTime` (virtual clock) or `pump` (real clock) drives the scheduler.

*Impact:* a loop written as `RunService.Heartbeat:Connect(function(dt) ... end)`
silently never runs. Use `task.spawn` + `task.wait` instead, which works under
both clocks.

### 7. Server only, and simulated services

MicroStudio runs `Script` instances under `ServerScriptService`. `LocalScript`s
are parsed and placed in the tree but never executed, and there is no client, so
`RunService:IsClient()` is false and `RunService:IsServer()` is true.
`--headless` reports `IsStudio() == false`; the default reports it as true.

**Every service in the API dump exists** — `game:GetService("TweenService")`
returns an instance rather than erroring — so a port no longer trips over a
missing service. What a service *does* varies, in three tiers:

- **Real local behaviour.** `Players`, `RunService`, `CollectionService`,
  `HttpService` (real requests, real JSON, real GUIDs, secrets from the
  environment), `DataStoreService` (a JSON file per store), `MemoryStoreService`
  (session state on the virtual clock), `MessagingService` (in-process pub/sub),
  `TweenService`, `PhysicsService`, `ContentProvider`, `LogService`,
  `BadgeService`, `MarketplaceService`, `UserService`, `GroupService`,
  `InsertService` (models seeded from JSON) and `TeleportService` (which records
  the request because there is nowhere to load).
- **Declared but simulated.** Every member the dump declares resolves: a
  function returns a fixed value for its declared type and most properties read
  a zero value, with **one warning per member** on first use so a game leaning
  on something unimplemented is not quietly wrong. `BasePart.Massless` on a
  service is nonsense, but `TeleportService:ReserveServer` returning `nil` is
  the kind of thing you want to be told about.
- **Absent on purpose.** Nothing is invented that the dump does not declare, so
  a typo still reports "is not a valid member of X".

Services with state keep it as plain JSON under the state directory
(`<project>/.microstudio` when a project file or `tsconfig.json` sits beside the
code, `--state-dir` or `MICROSTUDIO_STATE_DIR` to choose your own). A run with no
project keeps that data **in memory only**, so a script at a prompt leaves
nothing behind. A user can seed the directory the way miniflare seeds a binding.
See [services.md](./services.md).

*Impact:* a service call that used to error now returns a fixed value and warns.
Behaviour that depends on Roblox's servers (asset delivery, the real data store,
cross-server messaging, teleports) is emulated locally, so it is reproducible
but not remote.

### 8. No physics, replication or streaming

No `RemoteEvent` / `RemoteFunction`, no `BasePart:GetTouchingParts`, no
`RunService` simulation stepping, no character controller, no network ownership,
no `StreamingEnabled`. Parts hold geometry and properties; they do not move or
collide. `Humanoid` exists as a class with no behaviour, and a mock player's
character is parented, not simulated.

### 9. `$ignoreUnknownInstances` is parsed but inert

The Rojo key is read and retained. It has no effect, because a MicroStudio run
starts from an empty DataModel — there are no pre-existing instances that need
preserving.

### 10. `advanceTime` has a step budget

`task.wait` loops that reschedule themselves forever would hang a single
`advanceTime(1000)` call, so the scheduler gives up after 500 000 steps
(`DEFAULT_STEP_BUDGET`), sets a flag and emits a warning naming the likely
cause.

*Impact:* pathological loops stop instead of hanging; a legitimate run never
comes close.

### 11. Smaller gaps worth naming

- `Instance:Clone()` copies the subtree's properties, attributes and source, but
  clones are not inserted into any service-processing pipeline (there is none).
- A destroyed instance's id is reused for nothing and any access errors, as
  Roblox does, but `Destroy` does not run `Debris`-style cleanup.
- `wait()` is `task.wait()`; there is no legacy `elapsedTime` difference
  (`tick()`, `time()` and `elapsedTime()` all derive from the same clock, with
  `tick()` adding the fixed epoch).
- `require` takes an `Instance` (or a dotted path string). Requiring by asset id
  is not supported.
- Only the attribute value types in `microstudio_datamodel::AttributeValue` can
  be stored: the numeric/string datatypes, `BrickColor`, `Instance` and tables
  of those. Arbitrary userdata is rejected.
- `print` separates arguments with a space rather than a tab.
- **Extras that Roblox does not have.** `reset()`, `addPlayer(name)` and the
  `__microstudio_*` bindings exist for the test framework: `addPlayer` is the
  Luau spelling of `Players:AddMockPlayer`, and nothing in a normal script needs
  either. `docs/testing.md` covers them.

## Reporting a difference

If a script behaves differently here than in Studio, the useful report is the
smallest Luau chunk that shows it, plus what Studio prints and what MicroStudio
prints. Signals, scheduling and datatype formatting are the areas where a
difference is most likely to be a bug in this repository rather than a
documented deviation.
