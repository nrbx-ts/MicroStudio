# Simulated services

Every service in Roblox's API dump exists in MicroStudio, so
`game:GetService("TweenService")` hands back an instance rather than erroring.
What a service *does* varies, and this page says how.

Three tiers:

1. **Real local behaviour.** The service keeps state and answers correctly:
   `Players`, `RunService`, `CollectionService`, `HttpService`,
   `DataStoreService`, `MemoryStoreService`, `MessagingService`, `TweenService`,
   `PhysicsService`, `ContentProvider`, `LogService`, `BadgeService`,
   `MarketplaceService`, `UserService`, `GroupService`, `InsertService`,
   `TeleportService`.
2. **Declared but simulated.** Every other service, and every member of the
   services above that is not listed here, still resolves: a function returns a
   fixed value for its declared return type, an event is a real signal that may
   never fire, and a property reads a zero value. The first **call** to a
   simulated member warns once, prefixed `[warn]`:

   ```
   [warn] GuiService.IsTenFootInterface is simulated and returns a fixed value
   ```

   Property reads do not warn, because properties are read in loops and the note
   would drown the output. A script's warnings go to stderr, so they cannot be
   mistaken for its output. So a game that depends on something unimplemented
   tells you instead of failing silently or quietly doing the wrong thing.
3. **Absent on purpose.** Nothing is invented that the dump does not declare, so
   a typo still reports `'Foo' is not a valid member of Bar`, and
   `Instance.new` still rejects abstract and non-creatable classes.

## Where the data lives

Services that persist anything write plain JSON, so a project can seed its own
mock data the way miniflare seeds a binding. The directory is chosen like this:

| Situation | State directory |
| --- | --- |
| `--state-dir <path>` | that path |
| `MICROSTUDIO_STATE_DIR` | that path |
| a Rojo project file or `tsconfig.json` beside the code | `<project>/.microstudio` |
| anything else | **none**: service data stays in memory |

That last row is the point: running a script at a prompt (`microstudio -e …`,
`microstudio script.luau`, stdin, `repl`) must not leave data behind, and it
cannot see a project's seeds by accident. Pass `--state-dir` to opt in — a
leading `~` means your home directory, so `--state-dir ~/.microstudio` is the
shared scratch area, and `MICROSTUDIO_STATE_DIR` does the same for a shell you
set once.

`MICROSTUDIO_PROJECT_DIR` does the same job for a sidecar started by hand: the
runtime appends `.microstudio` to it when nothing more specific is set.

```
.microstudio/
  datastores/<store name>.json   DataStoreService
  secrets.json                   HttpService:SetSecret
  badges.json                    BadgeService
  marketplace.json               MarketplaceService
  users.json                     UserService
  groups.json                    GroupService
  assets.json                    InsertService:LoadAsset
  assets/<name>.json             InsertService:LoadLocalAsset
```

Nothing else is ever written. Workspace instances, the place tree and the code
you ran are never persisted, so `.microstudio` holds service data and nothing
else.

`MemoryStoreService` and `MessagingService` are **session state** and never
touch disk, which is the lifetime Roblox gives them: a local runtime has exactly
one session, so it drops both when the process exits.

Nothing in `.microstudio` is generated for you. A missing file behaves as an
empty one, so `DataStore:GetAsync("k")` is `nil` until something writes it, and
`BadgeService:AwardBadge` reports that the badge does not exist until you seed
it.

### Seeding an example

```jsonc
// .microstudio/datastores/players.json
{ "global": { "player_1": { "coins": 50 } } }
```

```jsonc
// .microstudio/users.json
{ "1": { "Username": "Builderman", "DisplayName": "Builder", "HasVerifiedBadge": true } }
```

```jsonc
// .microstudio/assets.json
{
  "assets": {
    "12345": {
      "name": "Crate",
      "class": "Model",
      "children": [{ "name": "Body", "class": "Part", "properties": { "Anchored": true, "Size": [4, 4, 4] } }]
    }
  }
}
```

## HttpService

Real requests to the real internet, with no domain allow-list, because there is
no Roblox proxy in the way.

- `RequestAsync({Url, Method, Headers, Body})` returns Roblox's dictionary —
  `Success`, `StatusCode`, `StatusMessage`, `Headers` (lower-cased, repeated
  headers joined with `", "`) and `Body`. A non-2xx status is **not** an error
  here, matching Roblox. A transport failure raises `HttpError: ConnectFail`,
  `HttpError: DnsResolve`, `HttpError: Timeout` or `HttpError: NetFail`.
- `GetAsync(url)` and `PostAsync(url, body)` return the body and raise on a
  non-2xx status.
- A table `Body` is JSON-encoded, and `Content-Type: application/json` is added
  when the caller did not set one.
- `JSONEncode` / `JSONDecode` map Lua tables to JSON and back, `GenerateGUID`
  returns a real v4 GUID (braced unless you pass `false`), and `UrlEncode` /
  `UrlDecode` percent-encode and decode.
- `HttpEnabled` reads `true` and is writable; a request while it is `false`
  raises instead, like a game with HTTP requests turned off.
- `GetSecret(name)` reads `MICROSTUDIO_SECRET_<NAME>` from the environment
  first, then `secrets.json`. `SetSecret(name, value)` writes to that file.

Requests time out after 30 seconds. Redirects are followed.

## DataStoreService

One JSON file per store name, keyed by scope then key, rewritten atomically on
every change:

```json
{ "global": { "player_1": { "coins": 50 } }, "season2": { "player_1": {} } }
```

- `GetDataStore(name, scope?)`, `GetGlobalDataStore()` and
  `GetOrderedDataStore(name, scope?)` hand back the **same instance** for the
  same name and scope.
- `GetAsync`, `SetAsync`, `UpdateAsync`, `IncrementAsync`, `RemoveAsync` and
  `ListKeysAsync` work as they do on Roblox, including `UpdateAsync` cancelling
  a write when its transform returns `nil`, `RemoveAsync` handing back the old
  value, and a key longer than 50 characters or a value that cannot be encoded
  as JSON raising instead of being stored.
- `GetSortedAsync(ascending, pageSize, minValue?, maxValue?)` sorts by value,
  with `minValue` inclusive and `maxValue` exclusive, and returns a pages object
  whose `AdvanceToNextPageAsync()` raises once the pages run out.
- `ListDataStoresAsync(prefix?)` lists the store files that exist.

Version history (`GetVersionAsync`, `ListVersionsAsync`) and `OnUpdate` are not
modelled: a store file is the whole truth.

## MemoryStoreService

`GetSortedMap(name)` and `GetQueue(name)` keep their entries in memory, with
Roblox's API: `SetAsync(key, value, expiration?)`, `GetAsync`, `UpdateAsync`,
`RemoveAsync`, `GetRangeAsync(direction, count, lower?, upper?)` sorted by value,
and `AddAsync` / `ReadAsync` / `RemoveAsync` for queues (highest priority first,
then oldest).

Expiry is measured on the **virtual clock**, so `advanceTime` moves it and a
test can prove that an entry is gone. The default expiry is the documented 45
days for a map and 30 for a queue item. A read does not consume a queue item;
`RemoveAsync(id)` does.

## MessagingService

`SubscribeAsync(topic, callback)` returns a connection, and `PublishAsync(topic,
message)` invokes every subscriber with Roblox's payload table `{ Data, Sent }`.

This is in-process: a message published in a runtime reaches that runtime's
subscribers only, and the publisher *does* receive its own message (real Roblox
skips the publishing server). Both differences are warned about once, and they
are what make a listener testable locally. Messages are capped at 1KB and topics
at 80 characters.

## TweenService

`Create(instance, tweenInfo, properties)` returns a `Tween`. Building one changes
nothing — `Play()` runs it, `Pause()` holds it (and `Play()` again resumes),
`Cancel()` stops it (before a `Play()` it only changes the state), `Completed`
fires with the state it finished in, and `PlaybackState` reads back.
`GetValue(alpha, easingStyle, easingDirection)` computes the real easing curve
for every `Enum.EasingStyle` and direction, and `TweenInfo.new(...)` is available
as a datatype.

Values are **not** interpolated frame by frame: a tween whose `Time` is 0 lands
its properties on the first `Play()`, and a longer one lands them, and fires
`Completed`, when the virtual clock reaches `playAt + DelayTime + Time`. A
repeated or reversing tween follows the same rule per repetition, and
`RepeatCount < 0` (endless) plays once. A warning says so the first time.

A `Tween` is a runtime object with the same methods rather than a DataModel
instance, because Roblox does not let scripts create one either
(`Instance.new("Tween")` fails there too).

## PhysicsService

Collision groups are bookkept for real — `RegisterCollisionGroup`,
`UnregisterCollisionGroup`, `RenameCollisionGroup`, `IsCollisionGroupRegistered`,
`GetRegisteredCollisionGroups`, `CollisionGroupSetCollidable`,
`CollisionGroupsAreCollidable`, `SetPartCollisionGroup`,
`CollisionGroupContainsPart`, `GetMaxCollisionGroups` (32) and the deprecated
`CreateCollisionGroup` / `GetCollisionGroupId` / `GetCollisionGroupName` /
`GetCollisionGroups` / `RemoveCollisionGroup` family — including
`BasePart.CollisionGroup`, which is an ordinary string property you can read and
write directly.

Nothing collides, because there is no physics step: a group is a label and a
matrix of "may these two touch", which is what a script reads. Renaming or
unregistering a group re-points the parts that used it, by walking the tree from
`game` — a part that is not in the tree keeps whatever it was last set to.

## ContentProvider

`PreloadAsync(instances)` walks the given instances and their descendants,
collects `rbxassetid://` values, and fires
`GetAssetFetchStatusChangedSignal(assetId)` for each with
`Enum.AssetFetchStatus.Success`. Nothing is downloaded, so preloading is
instantaneous, and `GetFailedRequests()` is always empty.

## LogService

`GetLogHistory()` returns what the runtime has printed, and `ClearOutput()`
clears it. `MessageOut` exists as a signal but the runtime's own `print` does not
fire it.

## TeleportService

There is nowhere to load, so a teleport records the request
(`Teleport`, `TeleportAsync`, `TeleportToPlaceInstance`,
`TeleportToSpawnByName`, `TeleportPartyAsync`), warns once, and fires
`TeleportInitFailed` — the event Roblox fires when a teleport cannot start,
which is exactly this case — with the failure reason as its name (`"Failure"`),
because an event argument can only carry a string. `ReserveServer` raises, and
`GetTeleportSetting` / `SetTeleportSetting` keep their values for the session.

## Config-driven services

These read their data from the state directory and are useful precisely because
a test can seed them.

| File | Service | Shape |
| --- | --- | --- |
| `badges.json` | `BadgeService` | `{ "badges": { "<badgeId>": { name, description, iconImageId, enabled } }, "awards": { "<userId>": ["<badgeId>"] } }` |
| `marketplace.json` | `MarketplaceService` | `{ "products": {...}, "gamePasses": {...}, "ownership": { "<userId>": { assets: [], gamePasses: [], products: [] } } }` |
| `users.json` | `UserService` | `{ "<userId>": { Username, DisplayName, HasVerifiedBadge } }` |
| `groups.json` | `GroupService` | `{ "groups": { "<groupId>": { Name, Description, OwnerId, MemberCount, Created } }, "members": {}, "allies": {}, "enemies": {} }` |
| `assets.json` | `InsertService` | `{ "assets": { "<assetId>": { name, class, properties, children } } }` |

- **BadgeService**: `UserHasBadgeAsync`, `AwardBadge` (false when the user
  already has it, an error naming the badge when it is not seeded) and
  `GetBadgeInfoAsync`, plus the deprecated `UserHasBadge` and the `BadgeAwarded`
  / `OnBadgeAwarded` events.
- **MarketplaceService**: `GetProductInfo`, `PlayerOwnsAsset`,
  `UserOwnsGamePassAsync`, `PlayerCanMakePurchases`, and the prompt family
  (`PromptPurchase`, `PromptGamePassPurchase`, `PromptProductPurchase`) which
  records the ownership and fires the matching `...Finished` event with
  `true` — a local purchase always succeeds, which the first one warns about.
  `ProcessReceipt` (a callback property) and `GetDeveloperProductsAsync` are
  left simulated.
- **UserService**: `GetUserInfosByUserIdsAsync` (in the order you passed the
  ids, with an unknown id raising). `Players:GetNameFromUserIdAsync` and
  `Players:GetUserIdFromNameAsync` read the same file, which is where real
  Roblox exposes them.
- **GroupService**: `GetGroupInfoAsync`, `GetGroupsAsync` (with `Role` and
  `Rank`), `GetAlliesAsync` and `GetEnemiesAsync`, which return a plain array
  where Roblox returns a Pages object.
- **InsertService**: `LoadAsset(assetId)` builds the seeded tree and returns an
  unparented `Model`; `LoadLocalAsset(path)` reads
  `<state dir>/assets/<path>.json` and refuses a path that escapes that
  directory. An unknown id raises with a hint showing how to seed it.
  `CreateMeshPartAsync` raises, because there is no mesh loading.

Unknown ids always raise rather than returning a plausible-looking default, so a
missing seed is visible immediately.

## Reaching a service from Rust

`crates/microstudio-datamodel/src/api/services.txt` is generated from Roblox's
API dump by `scripts/api-dump.ts` and embedded in the binary; it is the source of
truth for which services and members exist, and `docs/architecture.md` describes
how a service module registers real behaviour on top of it.
