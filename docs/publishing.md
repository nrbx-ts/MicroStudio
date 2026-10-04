# Publishing

MicroStudio ships as eleven npm packages: four that contain code, plus the cli
under a second name (`microstudio` as well as `@microstudio/cli`, so the tool can
be installed by its own name), and six that contain a prebuilt
`microstudio-runtime` sidecar — one per platform.

Nobody who writes Luau wants to install a Rust toolchain to run their tests, so
the sidecar is compiled once per platform and published as its own small
package. An install then keeps only the package that matches the machine, which
is the arrangement esbuild, swc and TypeScript's native preview all use.

| Package | `os` | `cpu` | Cargo triple | Binary |
| --- | --- | --- | --- | --- |
| `@microstudio/runtime-linux-x64` | `linux` | `x64` | `x86_64-unknown-linux-gnu` | `bin/microstudio-runtime` |
| `@microstudio/runtime-linux-arm64` | `linux` | `arm64` | `aarch64-unknown-linux-gnu` | `bin/microstudio-runtime` |
| `@microstudio/runtime-darwin-x64` | `darwin` | `x64` | `x86_64-apple-darwin` | `bin/microstudio-runtime` |
| `@microstudio/runtime-darwin-arm64` | `darwin` | `arm64` | `aarch64-apple-darwin` | `bin/microstudio-runtime` |
| `@microstudio/runtime-win32-x64` | `win32` | `x64` | `x86_64-pc-windows-msvc` | `bin/microstudio-runtime.exe` |
| `@microstudio/runtime-win32-arm64` | `win32` | `arm64` | `aarch64-pc-windows-msvc` | `bin/microstudio-runtime.exe` |

The *published* `@microstudio/runtime` lists all six as **optional**
dependencies, so an install keeps only the one whose `os`/`cpu` match this
machine: `npm install @microstudio/cli` pulls exactly one binary.

That list lives in what is published and not in the repository, because Yarn
fails on a dependency that does not exist — `YN0035 Package not found` — where
npm quietly skips a missing optional one, and none of these packages exists
until a release publishes it. Two consequences:

- `yarn install` works in a fresh checkout, and
  `packages/runtime/package.json` names no platform package at all.
- A released version is installable with Yarn only once **all six** platform
  packages exist for it, so a release publishes the binaries first and the
  `publish` job refuses to start until every one of them is up. npm tolerates
  the gap; Yarn does not.

All six are built by the release workflow, one per runner, so no step of a
release needs a machine of a particular platform.

## How the two halves find each other

```
  packages/runtime/src/targets.ts          the table: one row per platform
        │                                  name, os, cpu, triple, binary
        ├──────────────────────────────┐
        ▼                              ▼
  scripts/stage-binary.ts          src/resolve-binary.ts
  (what a release writes)          (what a run looks for)
        │                              │
        ▼                              ▼
  dist/npm/@microstudio/…           @microstudio/runtime-linux-x64/bin/…
```

Both sides are derived from `targets.ts`, and a test fails if the staged
`optionalDependencies` does not list exactly the packages that table describes.
Those are the two ways this feature can break silently — a package nothing
depends on, or a package the resolver never looks for — so both are covered
rather than commented.

At run time the search order is:

1. `MICROSTUDIO_RUNTIME_BIN`, for anything unusual.
2. The cargo output of a source checkout (`target/release`, then `target/debug`,
   honouring `MICROSTUDIO_TARGET_DIR` and `CARGO_TARGET_DIR`). A checkout that has
   built the sidecar uses its own build, which is what someone editing the Rust
   side wants.
3. The installed platform package — the only source a published install has.

If none of them exists, the error names the package to install for this machine.

## Building a platform package

`stage-binary.ts` is the whole mechanism. It writes the manifest the registry is
handed (`os`, `cpu`, `files`) and copies the binary to `bin/`, and it is the same
code in CI and on a laptop:

```bash
cargo build --release --bin microstudio-runtime
yarn stage:binary stage --target linux-x64   # → dist/npm/@microstudio/runtime-linux-x64
npm publish "dist/npm/@microstudio/runtime-linux-x64" --access public
```

The staging script is deliberately dependency-free, so a release job can call it
with plain `node scripts/stage-binary.ts stage …` without installing the
workspace first.

`--target` names a row of the table, `--binary` defaults to where a native cargo
build puts it, and `--version` defaults to the root `package.json`'s. A windows
run stages `microstudio-runtime.exe` without being told, because the table says
so.

There is no `exports` map in a platform package on purpose: the resolver reaches
into `bin/<binary>` by path, and an `exports` map would close that door.

### A binary that runs anywhere

A prebuilt binary that wants a library the machine does not have is a release
that installs and then fails to start, which is the one thing shipping a binary
is meant to avoid. Two things keep that from happening:

- **It links its c++ runtime statically.** `.cargo/config.toml` sets
  `+crt-static` for both windows targets, which takes the Visual C++
  redistributable off the list of things an install needs. Before that, the
  sidecar imported `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll` and `MSVCP140.dll`,
  so a machine without the redistributable installed fine and then would not run.
- **The release reads the dependency table.** `scripts/check-binary.ts` asks the
  platform (`readelf -d`, `otool -L`, or the PE import table) what the binary
  loads, and fails if that is anything past the c library — the three dlls above
  and their `MSVCP`/`MSVCR` relatives are the deny list on windows, and on linux
  and macOS anything outside the system libraries is refused. Linux allows the
  c++ runtime as well, because Luau is C++ and Node links `libstdc++` too, so a
  machine that can run the cli already has it. The `binaries` job runs the check
  on each staged binary, and then runs the binary itself.

`packages/test/src/binaries.test.ts` runs the same check on the sidecar a
checkout built, so a new dependency that breaks portability fails on a laptop
instead of in a release.

The linux legs build on `ubuntu-22.04`, the oldest image GitHub still hosts,
because the glibc a binary is linked against is the floor for everyone who
installs it. Alpine and other musl systems are not covered: they would need a
musl sidecar of their own, which no release publishes yet.

## Staging a release

A release is a directory of ready packages, not a checkout that has been built.
Two scripts fill `dist/npm/` (gitignored), one for each kind of package:

| Script | Writes | Does |
| --- | --- | --- |
| `scripts/stage-binary.ts` | `dist/npm/@microstudio/runtime-<platform>-<arch>` | copies the sidecar into `bin/`, writes the `os`/`cpu` manifest and a generated readme |
| `scripts/stage-package.ts` | `dist/npm/<name>` | compiles the package with `tsc`, copies its assets, readme and licence, writes the publish manifest, and stages a copy under each alias |

```bash
cargo build --release --bin microstudio-runtime
yarn stage:binary  stage --target linux-x64
yarn stage:package stage --all --version 0.2.0
node scripts/stage-package.ts list        # what a release publishes, one per line
npm publish dist/npm/@microstudio/runtime-linux-x64 --access public
npm publish dist/npm/@microstudio/runtime           --access public   # and the other three
```

`stage-package.ts` is the one that has to think: it runs `tsc` over
`packages/<name>/src` into `<staged>/dist`, copies the assets that are not
TypeScript (`src/luau/`, `types/`), copies the package's `README.md` and the
repository's `LICENSE` beside them, and rewrites the manifest so `exports`,
`types` and `bin` point at the compiled files instead of the sources. The rules
live in `scripts/manifest.ts`, which both scripts share.

`list` prints `<name> <dir>` for everything `stage --all` writes, aliases
included, one package per line with forward slashes in the path so a bash `read`
loop can use it anywhere. The release workflow reads it instead of repeating the
package names — so a package that is staged is a package that is published — and
refuses to run if it comes back empty.

The repository itself is never written to — which is what keeps the development
loop free of a build step. A checkout still runs `.ts` files directly; only a
staged copy is JavaScript.

### Two names for the cli

A package declares the other names it publishes under in its own manifest, and
staging does the rest:

```jsonc
// packages/cli/package.json
"publishAliases": ["microstudio"]
```

`stage cli` compiles once and then writes `<out>/microstudio` as a byte-for-byte
copy of `<out>/@microstudio/cli` whose manifest differs in one field: `name`.
Everything else is what the scoped package got — the same version, the same
`bin`, and the same pinned `@microstudio/*` dependencies, so an install of
either name runs the same code and pulls the same sidecar. `publishAliases` is
dropped from both published manifests; it is a staging directive, not something
an install should see.

Both names go up in the same release, one after the other, with the same
dist-tag and provenance. Adding a third name is one line in that array: the token
publishes any package the account can publish, so nothing on npmjs.com has to be
set up first. Once the secret is gone in favour of trusted publishing, a new name
does need its own trusted publisher, because that is all npm will accept.

### Versions

The repository keeps `"*"` so a version is not written down twice in every
manifest, and a staged manifest is pinned to the version being released:

- the six platform packages in `optionalDependencies`, so an install cannot
  take a sidecar older than the driver that asked for it;
- the code packages' dependencies on each other, so a published CLI cannot
  pick up a runtime whose sidecar protocol has moved on since.

`devDependencies` are left alone: nobody installing the package gets them.

## Yarn for the project, npm for the registry

Installing, staging and testing are Yarn, named by `packageManager` and run
through corepack. Publishing stays with the npm CLI, because that is the client
the registry speaks:

- `--provenance` is an npm-CLI feature, and the attestation is signed with the
  OIDC token the job asks for. `yarn npm publish` speaks the same flow on GitHub
  Actions, but it publishes the active *workspace*, and a release here is a
  directory of staged packages;
- `yarn npm pack` is not a substitute for `npm pack --dry-run` in the tests:
  the npm CLI is what builds the published tarball, so its file list is the one
  that matters;
- `yarn npm info` cannot replace `npm view` in the "refuse to overwrite"
  guards. Asked for a version that does not exist, it warns
  (`Unmet range … falling back to the latest version`) and answers with the
  latest one, exit code 0 — which would look like "already published" and block
  every release.

Every package names the repository it is published from, and `stage-package.ts`
refuses to stage one that points somewhere else. npm compares that field with
the repository the release workflow runs in and will not attach a provenance
attestation when they disagree — a release that would fail with everything
already built.

One more Yarn behaviour worth knowing on release day: Yarn 4 quarantines a
version published less than `npmMinimalAgeGate` minutes ago, which defaults to
1440 — a day. A release is therefore not installable with Yarn until it is a day
old, unless the project lowers that setting; npm installs it straight away.
Nothing here is broken by that, but "the release is up and my install cannot see
it" has this explanation.

## In CI

`.github/workflows/release.yml` is the whole release: a `vX.Y.Z` tag, or a
manual run.

```
plan ──▶ test ──▶ approve ──▶ binaries (one runner per platform, 6 in parallel) ──┐
                                                                    publish ◀─────┘
```

`plan` works out the version and the dist-tag once and hands them to every job;
`test` is the same workflow a push and a pull request run, so nothing is
published from untested code; `approve` is a protected environment, which is
where a human signs off. `binaries` then builds each platform package on a
runner native to that platform — `ubuntu-22.04`, `ubuntu-22.04-arm`,
`macos-15-intel`, `macos-15`, `windows-2022` and `windows-11-arm` — so nothing is
cross-compiled or emulated. Each leg checks the staged binary before uploading it
(what it loads, and that it starts) and then publishes itself with
`npm publish --provenance`.

`publish` refuses to start until all six platform packages exist for the version
being released, which it checks against the target table rather than a second
list. That is a Yarn requirement: the published driver names all six, and Yarn
fails to resolve one that is missing, where npm would skip it. It then stages the
code packages (aliases included), publishes what `stage-package.ts list` printed,
and finishes by checking that all eleven carry an attestation — so what ships is
what the packaging tests packed and inspected, and a release that lost its
provenance says so.

The `test` workflow also runs an `install` job on Windows and macOS, which packs
the packages, installs them into a scratch `node_modules` and runs the cli from
there. The `node` job does the same on linux, so every platform a sidecar is
published for has had an install of it run on the real thing.

Provenance is the reason this is on GitHub Actions rather than anywhere else: an
attestation is signed with an OIDC token the workflow asks for, and records the
repository, workflow and commit the tarball came from. CircleCI cannot mint one.

### The publish token

`npm publish` authenticates with the `NPM_PUBLISH` repository secret: a granular
npm token with permission to publish these packages and 2FA bypass enabled — a
token without that waits for a one-time password no runner can type, and the
release stops on the first upload. A granular token can also create a package that
does not exist yet, which is why the first release needs no manual step.

`actions/setup-node` writes the `.npmrc` that reads the value out of
`NODE_AUTH_TOKEN`, so the secret is never written to a file on the runner or
printed in a log. Both publishing jobs check it is there, and that `npm whoami`
answers with it, before they build anything — a wrong or expired token costs
seconds instead of a release run.

Trusted publishing is where this has to end up. npm gives a configured trusted
publisher precedence over `NODE_AUTH_TOKEN`, so the two coexist: add a trusted
publisher on npmjs.com to however many packages you have done (this repository,
workflow file `release.yml`, no environment), and delete the secret once all
eleven have one. Nothing in the workflow changes when you do — it already asks for
`id-token: write`, and the npm 11.5.1 the provenance check insists on is the same
version trusted publishing needs. That matters because from January 2027 npm stops
accepting a direct publish from a 2FA-bypass token: without trusted publishers,
the release would have to move to a stage-only token and `npm stage publish`, with
a person approving each staged package in the npm UI.

## Why the code packages are compiled at publish time

The code packages ship JavaScript, because Node refuses to strip types for
files under `node_modules`:

```
Error [ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING]: Stripping types is
currently unsupported for files under node_modules
```

No flag turns that off, and a workspace checkout never notices: the package
manager links workspace packages by symlink, so their real path is outside
`node_modules`. The first *published* install of `@microstudio/runtime` or
`@microstudio/cli` would fail on the first import, and the CLI's `bin` on the
first run.

The alternative — a build step between a developer and their tests — was worth
avoiding, so the compilation happens on the way out instead. Two files hold the
line, and both stage into a temporary directory the way CI does:

- `packages/test/src/packaging.test.ts` checks the staged shape (JavaScript and
  declarations, no sources, no tests, assets copied, readme and licence copied,
  manifest rewritten, versions pinned) and then imports the staged packages from a
  real `node_modules` copy, which is the case that used to fail. It ends by packing
  every package, `npm install`ing the tarballs into a scratch directory and
  running the cli from there — `--version`, a line of Luau, and `microstudio
  test` on a project — which is the one test that fails if an install does not
  work.
- `packages/test/src/binaries.test.ts` does the same for a platform package,
  down to resolving the binary out of the installed package, and checks the
  sidecar a checkout built loads nothing but the system's own libraries.
