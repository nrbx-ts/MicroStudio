import { createInterface } from "node:readline";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { existsSync, readFileSync, statSync, watch } from "node:fs";

import {
  buildPlaceEntries,
  compileProject,
  DEFAULT_SERVICES,
  findCompiler,
  findCompiledSpec,
  findProjectFile,
  mountDirectories,
  readRojoProject,
  rbxtsIncludeFolder,
  readTsConfig,
  writeTestConfig,
} from "@microstudio/roblox-ts";
import {
  DEFAULT_INCLUDE,
  DEFAULT_TIMEOUT_MS,
  DEFAULT_TS_INCLUDE,
  discoverSpecs,
  formatSummary,
  formatSuite,
  renderJsonReport,
  runSpecFiles,
  summarize,
  type SpecEntry,
} from "@microstudio/test";
import { createRuntime, type InstanceJson, type Runtime } from "@microstudio/runtime";

import { flagBool, flagString, projectDir, type ParsedArgs } from "./args.ts";

const COLOURS = {
  reset: "\u001b[0m",
  dim: "\u001b[2m",
  red: "\u001b[31m",
  yellow: "\u001b[33m",
  cyan: "\u001b[36m",
};

// read from the manifest, not written down twice, so an installed cli reports the
// version of the package that was installed: staging pins this one to the release
const manifest = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
) as { version?: string };

export const VERSION = manifest.version ?? "0.0.0";

// a spec the project's TypeScript compiler has to turn into Luau first
const TYPESCRIPT_SPEC = /\.tsx?$/i;

function paint(text: string, colour: keyof typeof COLOURS): string {
  if (!process.stdout.isTTY) {
    return text;
  }
  const code = COLOURS[colour] ?? "";
  return `${code}${text}${COLOURS.reset}`;
}

interface MaybeInstance {
  __type?: unknown;
  name?: unknown;
  className?: unknown;
}

// true for the runtime's `{ __type: "Instance" }` shape
function isInstance(value: unknown): value is InstanceJson {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as MaybeInstance).__type === "Instance"
  );
}

export function formatValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "nil";
  }
  if (isInstance(value)) {
    if (value.className === "Player") {
      return `PlayerAdded: ${value.name ?? ""}`;
    }
    return `Instance<${value.className ?? "?"}> ${value.name ?? ""}`;
  }
  if (Array.isArray(value)) {
    if (value.length > 0 && value.every(isInstance)) {
      return value.map((instance) => instance.name ?? "").join("\n");
    }
    return JSON.stringify(value);
  }
  if (typeof value === "object") {
    return JSON.stringify(value, null, 2);
  }
  return String(value);
}

function resolveProjectFile(args: ParsedArgs): string | undefined {
  const explicit = flagString(args, "--project");
  if (explicit !== undefined) {
    return resolve(explicit);
  }
  return findProjectFile(projectDir(args));
}

// re-readable without the runtime: --watch re-reads on a mount change
interface ProjectSource {
  projectFile: string;
  services: readonly string[];
  entries(): ReturnType<typeof buildPlaceEntries>;
}

function readProject(args: ParsedArgs): ProjectSource | undefined {
  const projectFile = resolveProjectFile(args);
  const dir = projectDir(args);

  if (projectFile === undefined) {
    console.error(
      [
        `No Rojo project file found in ${dir}.`,
        "",
        "MicroStudio reads the project's existing Rojo setup to learn where the",
        "compiled Luau lives. Add a `default.project.json`, or pass",
        "`--project path/to/default.project.json`.",
      ].join("\n"),
    );
    return undefined;
  }

  const serviceList = flagString(args, "--services");
  const services =
    serviceList === undefined ? DEFAULT_SERVICES : serviceList.split(",").map((s) => s.trim());

  return { projectFile, services, entries: () => buildPlaceEntries({ projectFile, services }) };
}

// routes sidecar diagnostics through the cli's formatting
async function startRuntime(args: ParsedArgs, clock: "virtual" | "real"): Promise<Runtime> {
  const stateDir = stateDirFor(args);
  return createRuntime({
    clock,
    ...(stateDir === undefined ? {} : { stateDir }),
    onStderr: (line) => {
      if (line.trim().length > 0) {
        console.error(paint(`[runtime] ${line}`, "dim"));
      }
    },
  });
}

// a project keeps its mock data beside its source; a loose script gets ~/.microstudio
function stateDirFor(args: ParsedArgs): string | undefined {
  const explicit = flagString(args, "--state-dir");
  if (explicit !== undefined) {
    // the shell does not expand ~ for a program it starts, so do it here
    return resolve(explicit.replace(/^~(?=[\\/]|$)/, homedir()));
  }
  const project = flagString(args, "--project");
  if (project !== undefined) {
    return projectStateDir(dirname(resolve(project)));
  }

  // the first token that names a path is what the user is working on: a subcommand's
  // directory, a script, or nothing at all
  const target = [args.command, ...args.positionals]
    .filter((token) => token.length > 0)
    .map((token) => resolve(token))
    .find((path) => statSync(path, { throwIfNoEntry: false }) !== undefined);
  if (target === undefined) {
    return projectStateDir(process.cwd());
  }

  const stats = statSync(target, { throwIfNoEntry: false });
  return projectStateDir(stats?.isDirectory() === true ? target : dirname(target));
}

// the sidecar falls back to the user's home directory when a directory is not a project
function projectStateDir(dir: string): string | undefined {
  if (findProjectFile(dir) !== undefined || existsSync(join(dir, "tsconfig.json"))) {
    return join(dir, ".microstudio");
  }
  return undefined;
}

// a missing project file is not an error: specs run against an empty world
function projectEntriesIfAny(
  args: ParsedArgs,
  root: string,
): ReturnType<typeof buildPlaceEntries> | undefined {
  const explicit = flagString(args, "--project");
  const projectFile =
    explicit !== undefined
      ? resolve(explicit)
      : (findProjectFile(root) ?? findProjectFile(process.cwd()));
  if (projectFile === undefined) {
    return undefined;
  }

  const serviceList = flagString(args, "--services");
  const services =
    serviceList === undefined
      ? DEFAULT_SERVICES
      : serviceList.split(",").map((service) => service.trim());
  return buildPlaceEntries({ projectFile, services });
}

// `--module ReplicatedStorage.Counter=src/counter.luau`, comma-separated
function readModuleFlags(args: ParsedArgs): Record<string, string> {
  const raw = flagString(args, "--module");
  if (raw === undefined) {
    return {};
  }

  const modules: Record<string, string> = {};
  for (const part of raw.split(",")) {
    const separator = part.indexOf("=");
    if (separator === -1) {
      throw new Error(
        `--module expects path=file, got "${part.trim()}". ` +
          "Example: --module ReplicatedStorage.Counter=src/counter.luau",
      );
    }
    const name = part.slice(0, separator).trim();
    const file = part.slice(separator + 1).trim();
    if (name.length === 0 || file.length === 0) {
      throw new Error(`--module expects path=file, got "${part.trim()}".`);
    }
    modules[name] = readTextFile(resolve(file));
  }
  return modules;
}

const WATCHED_EXTENSIONS = /\.(lua|luau)$/i;

// node_modules and dotfiles are noise, not game code
function isIgnoredWatchPath(path: string): boolean {
  return path
    .split(/[\\/]/)
    .some((segment) => segment === "node_modules" || segment.startsWith("."));
}

// relative while inside the cwd, absolute otherwise
function displayPath(path: string): string {
  const relativePath = relative(process.cwd(), path);
  return relativePath.length === 0 || relativePath.startsWith("..") ? path : relativePath;
}

interface WatchOptions {
  // mount dirs from mountDirectories
  directories: string[];
  // the project file or a script
  files: string[];
  // called with a reason, after a debounce
  onReload(reason: string): Promise<void> | void;
}

// editors rename temp files and compilers rewrite everything at once, so one
// reload per burst; a change during a reload queues exactly one more
function watchProject(options: WatchOptions): () => void {
  const watchers: Array<{ close(): void }> = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  let queued: string | undefined;
  let reloading = false;

  const run = async (): Promise<void> => {
    if (reloading) {
      return;
    }
    reloading = true;
    try {
      while (queued !== undefined) {
        const reason = queued;
        queued = undefined;
        await options.onReload(reason);
      }
    } finally {
      reloading = false;
    }
  };

  const schedule = (reason: string): void => {
    queued = reason;
    if (timer !== undefined) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = undefined;
      void run();
    }, 120);
  };

  for (const directory of options.directories) {
    try {
      watchers.push(
        watch(directory, { recursive: true }, (_event, filename) => {
          const changed = filename === null ? directory : join(directory, String(filename));
          if (isIgnoredWatchPath(changed) || !WATCHED_EXTENSIONS.test(changed)) {
            return;
          }
          schedule(displayPath(changed));
        }),
      );
    } catch (error) {
      console.error(
        paint(
          `cannot watch ${directory}: ${error instanceof Error ? error.message : String(error)}`,
          "yellow",
        ),
      );
    }
  }

  for (const file of options.files) {
    try {
      watchers.push(watch(file, () => schedule(displayPath(file))));
    } catch {
      // an unwatchable file is not fatal
    }
  }

  return () => {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
    for (const watcher of watchers) {
      watcher.close();
    }
    watchers.length = 0;
  };
}

// a warning goes beside a script's output, not into it: the output may be piped somewhere
function attachOutput(
  runtime: Runtime,
  write: (line: string) => void,
  writeWarning: (line: string) => void = write,
): () => void {
  const offLog = runtime.onOutput((log) => {
    // an error is also a notification; printing both duplicates the block
    if (log.level === "error") {
      return;
    }
    if (log.level === "warn") {
      writeWarning(paint(`[warn] ${log.text}`, "yellow"));
      return;
    }
    write(log.text);
  });
  const offError = runtime.onError((error) => {
    write(paint(error.rendered, "red"));
  });
  return () => {
    offLog();
    offError();
  };
}

// shared by dev --interactive and repl; runtime via a getter so --watch can replace it
interface LiveConsole {
  // redraws the prompt after unsolicited output; a no-op when stdin is not a TTY
  reprompt(): void;
  // releases stdin so the process can exit
  close(): void;
  // resolves on quit or end of input
  done: Promise<void>;
}

function startConsole(current: () => Runtime, banner: string): LiveConsole {
  console.log(paint(banner, "cyan"));

  const isTty = process.stdin.isTTY === true;
  const rl = createInterface({ input: process.stdin, output: process.stdout, terminal: isTty });

  // piped input can EOF while the last line is still evaluating
  // so readline may already be closed when we prompt again
  let closed = false;
  rl.on("close", () => {
    closed = true;
  });

  const close = () => {
    if (closed) {
      return;
    }
    closed = true;
    rl.close();
  };

  const prompt = () => {
    if (closed) {
      return;
    }
    rl.setPrompt("> ");
    rl.prompt();
  };
  const reprompt = () => {
    if (closed || !isTty) {
      return;
    }
    // `true` keeps the half-typed line on screen
    rl.prompt(true);
  };

  const done = (async () => {
    prompt();
    for await (const rawLine of rl) {
      const line = rawLine.trim();
      if (line.length === 0) {
        prompt();
        continue;
      }

      if (line === ":quit" || line === ":q") {
        break;
      }
      if (line === ":help") {
        console.log(REPL_HELP);
        prompt();
        continue;
      }
      if (line === ":stats") {
        console.log(JSON.stringify(await current().stats(), null, 2));
        prompt();
        continue;
      }
      if (line.startsWith(":advance")) {
        const seconds = Number(line.split(/\s+/)[1] ?? "1");
        const time = await current().advanceTime(Number.isFinite(seconds) ? seconds : 1);
        console.log(paint(`time = ${time.toFixed(3)}`, "dim"));
        prompt();
        continue;
      }
      if (line.startsWith(":player")) {
        const name = line.split(/\s+/).slice(1).join(" ") || "Player1";
        const player = await current().players.addMockPlayer(name);
        console.log(`PlayerAdded: ${player.name}`);
        prompt();
        continue;
      }
      if (line.startsWith(":tree")) {
        const path = line.split(/\s+/)[1];
        const root = await current().tree(path);
        printTree(root);
        prompt();
        continue;
      }

      try {
        // `auto` echoes a bare expression and still accepts statements; a statement echoes nothing
        const value = await current().eval(line, { mode: "auto" });
        if (value !== null && value !== undefined) {
          console.log(formatValue(value));
        }
      } catch (error) {
        console.error(paint(error instanceof Error ? error.message : String(error), "red"));
      }
      prompt();
    }
  })().finally(close);

  return { reprompt, close, done };
}

export async function dev(args: ParsedArgs): Promise<number> {
  // live session defaults to wall time; --clock virtual only moves on :advance
  const requested = flagString(args, "--clock") === "virtual" ? "virtual" : "real";
  const source = readProject(args);
  if (source === undefined) {
    return 2;
  }
  const { projectFile } = source;

  let runtime = await startRuntime(args, requested);
  // the sidecar has the last word on the clock
  const clock = runtime.clock;

  let live: LiveConsole | undefined;
  const write = (line: string) => {
    console.log(line);
    live?.reprompt();
  };
  let detach = attachOutput(runtime, write);

  const boot = async (target: Runtime): Promise<number> => {
    const created = await target.loadTree(source.entries());
    console.log(paint(`loaded ${created.length} instances`, "dim"));
    const scripts = await target.run();
    for (const script of scripts) {
      console.log(paint(`[server] ${script}`, "dim"));
    }
    return scripts.length;
  };

  console.log(paint(`MicroStudio dev — ${projectFile}`, "cyan"));
  let stopWatching: (() => void) | undefined;
  let watchMounts: (() => void) | undefined;

  try {
    const scriptCount = await boot(runtime);

    if (flagBool(args, "--once")) {
      await runtime.advanceTime(Number(flagString(args, "--timeout") ?? "0.1"));
      const errors = runtime.errors();
      return errors.length > 0 ? 1 : 0;
    }

    // wall clock needs a pump: timers only fire when someone asks the scheduler
    const pump =
      clock === "real"
        ? setInterval(() => {
            void runtime.pump().catch(() => undefined);
          }, 16)
        : undefined;
    const stopped = new Promise<void>((resolvePromise) => {
      process.once("SIGINT", () => resolvePromise());
      process.once("SIGTERM", () => resolvePromise());
    });

    if (flagBool(args, "--interactive")) {
      // interactive session owns the stream; the getter survives a --watch reload
      detach();
      live = startConsole(
        () => runtime,
        `MicroStudio live — ${clock} clock, ${scriptCount} server script${scriptCount === 1 ? "" : "s"} — :help for commands`,
      );
      detach = attachOutput(runtime, write);
    } else {
      console.log(
        paint(
          clock === "real"
            ? "running — Ctrl-C to stop"
            : "running — time only moves with :advance; Ctrl-C to stop",
          "dim",
        ),
      );
    }

    if (flagBool(args, "--watch")) {
      // re-armed each reload: the project file can add or move mounts
      watchMounts = () => {
        stopWatching?.();
        stopWatching = watchProject({
          directories: mountDirectories({ projectFile, services: source.services }),
          files: [projectFile],
          onReload: async (changed) => {
            // a reload is a restart: re-running on the old world would duplicate
            // instances, leave stale listeners and reuse a warm require cache
            const previous = runtime;
            detach();
            await previous.dispose();
            runtime = await startRuntime(args, requested);
            detach = attachOutput(runtime, write);
            console.log(paint(`↻ reload — ${changed}`, "cyan"));
            try {
              await boot(runtime);
            } catch (error) {
              console.error(
                paint(error instanceof Error ? error.message : String(error), "red"),
              );
            }
            watchMounts?.();
          },
        });
      };
      watchMounts();
      console.log(paint("watching the project's mounts — reload on save", "dim"));
    }

    if (live === undefined) {
      await stopped;
    } else {
      await Promise.race([live.done, stopped]);
    }

    stopWatching?.();
    if (pump !== undefined) {
      clearInterval(pump);
    }

    const errors = runtime.errors().length;
    const stats = await runtime.stats();
    console.log(
      paint(`stopped at ${stats.time.toFixed(3)}s — ${errors} error(s)`, "dim"),
    );
    return errors > 0 ? 1 : 0;
  } finally {
    stopWatching?.();
    // Ctrl-C must release stdin too, or the process lingers
    live?.close();
    detach();
    await runtime.dispose();
  }
}

// the framework's globals, so a TypeScript spec needs no tsconfig of its own
const FRAMEWORK_TYPES = createRequire(import.meta.url).resolve(
  "@microstudio/test/types/globals",
);

interface CompiledSpecs {
  outDir: string;
  // ts spec -> the Luau file it compiled to
  files: Map<string, string>;
}

type CompileOutcome =
  | ({ ok: true } & CompiledSpecs)
  | { ok: false; message: string };

// nothing else turns a .ts spec into Luau, so the project's own roblox-ts runs first
function compileTypeScriptSpecs(options: {
  args: ParsedArgs;
  root: string;
  specs: readonly string[];
  compile: boolean;
}): CompileOutcome {
  const { args, root } = options;
  const tsconfigFlag = flagString(args, "--tsconfig");
  const tsconfigFile =
    tsconfigFlag === undefined ? join(root, "tsconfig.json") : resolve(tsconfigFlag);
  if (!existsSync(tsconfigFile)) {
    return {
      ok: false,
      message:
        `No tsconfig.json in ${displayPath(dirname(tsconfigFile))}. A roblox-ts ` +
        "project has one; pass --tsconfig <file> if yours is named differently.",
    };
  }

  const config = readTsConfig(tsconfigFile);
  const files = new Map<string, string>();

  if (options.compile) {
    const compiler = findCompiler(root);
    if (compiler === undefined) {
      return {
        ok: false,
        message:
          `No roblox-ts compiler installed for ${displayPath(root)}, and a ` +
          "TypeScript spec has to be compiled before it can run:\n" +
          "  npm install --save-dev roblox-ts\n" +
          "Or compile yourself and pass --no-compile.",
      };
    }

    const explicitProject = flagString(args, "--project");
    const projectFile =
      explicitProject === undefined
        ? findProjectFile(root)
        : resolve(explicitProject);
    if (projectFile === undefined) {
      return {
        ok: false,
        message:
          "roblox-ts maps the tree through Rojo, so it needs the project's " +
          "default.project.json (or --project <file>).",
      };
    }

    const includeFolder = rbxtsIncludeFolder(readRojoProject(projectFile));
    const generated = writeTestConfig({
      config,
      specs: options.specs,
      globals: FRAMEWORK_TYPES,
    });
    const result = compileProject({
      dir: root,
      compiler,
      config: generated,
      projectFile,
      includeFolder,
    });
    if (!result.ok) {
      return { ok: false, message: `roblox-ts failed:\n${result.output}` };
    }
  }

  for (const spec of options.specs) {
    const compiled = findCompiledSpec(config.outDir, spec);
    if (compiled === undefined) {
      return {
        ok: false,
        message:
          `${basename(spec)} is not in ${displayPath(config.outDir)}. ` +
          (options.compile
            ? 'A spec has to sit under the project\'s tsconfig "rootDir" to be compiled.'
            : "This run skipped the compiler (--no-compile), so it has to be built already."),
      };
    }
    files.set(spec, compiled);
  }

  return { ok: true, outDir: config.outDir, files };
}

// the compiler's output and the vendored runtime hold no hand-written specs, and
// running a compiled spec as a chunk would break its require of RuntimeLib
function generatedSpecRoots(args: ParsedArgs, root: string): string[] {
  const patterns: string[] = [];

  const tsconfigFlag = flagString(args, "--tsconfig");
  const tsconfigFile =
    tsconfigFlag === undefined ? join(root, "tsconfig.json") : resolve(tsconfigFlag);
  if (existsSync(tsconfigFile)) {
    const outDir = relative(root, readTsConfig(tsconfigFile).outDir)
      .split(sep)
      .join("/");
    if (outDir.length > 0 && !outDir.startsWith("..")) {
      patterns.push(`${outDir}/**`);
    }
  }

  const projectFlag = flagString(args, "--project");
  const projectFile =
    projectFlag === undefined ? findProjectFile(root) : resolve(projectFlag);
  if (projectFile !== undefined) {
    const include = relative(root, rbxtsIncludeFolder(readRojoProject(projectFile)))
      .split(sep)
      .join("/");
    if (include.length > 0 && !include.startsWith("..")) {
      patterns.push(`${include}/**`);
    }
  }

  return patterns;
}

export async function test(args: ParsedArgs): Promise<number> {
  const root = projectDir(args);
  const include = flagString(args, "--include")
    ?.split(",")
    .map((pattern) => pattern.trim());
  const exclude = flagString(args, "--exclude")
    ?.split(",")
    .map((pattern) => pattern.trim());
  const timeoutMs = Number(flagString(args, "--timeout") ?? DEFAULT_TIMEOUT_MS);
  const isolate = flagBool(args, "--isolate");
  const asJson = flagBool(args, "--json");
  const watchMode = flagBool(args, "--watch");
  const compile = !flagBool(args, "--no-compile");

  const runOnce = async (): Promise<number> => {
    const startedAt = Date.now();
    // one pass over the tree, then split by extension: a .ts spec is compiled first
    const excluded = {
      exclude: [...(exclude ?? []), ...generatedSpecRoots(args, root)],
    };
    const found = discoverSpecs(root, {
      include: include ?? [...DEFAULT_INCLUDE, ...DEFAULT_TS_INCLUDE],
      ...excluded,
    });
    const tsSpecs = found.filter((file) => TYPESCRIPT_SPEC.test(file));
    const luauSpecs = found.filter((file) => !TYPESCRIPT_SPEC.test(file));

    if (luauSpecs.length === 0 && tsSpecs.length === 0) {
      console.error(
        paint(
          `No spec files found in ${displayPath(root)}.\n` +
            "Specs are *.spec.luau, *.spec.lua, *.test.luau, *.test.lua or, for a\n" +
            "roblox-ts project, *.spec.ts, *.test.ts.",
          "red",
        ),
      );
      return 1;
    }

    const entries: SpecEntry[] = luauSpecs.map((file) => ({
      file,
      label: displayPath(file),
    }));

    let compiled: CompiledSpecs | undefined;
    if (tsSpecs.length > 0) {
      const outcome = compileTypeScriptSpecs({ args, root, specs: tsSpecs, compile });
      if (!outcome.ok) {
        console.error(paint(outcome.message, "red"));
        return 2;
      }
      compiled = outcome;
    }

    let place: ReturnType<typeof buildPlaceEntries>;
    let modules: Record<string, string>;
    try {
      place = projectEntriesIfAny(args, root) ?? [];
      modules = readModuleFlags(args);
    } catch (error) {
      console.error(paint(error instanceof Error ? error.message : String(error), "red"));
      return 2;
    }

    if (compiled !== undefined) {
      const byFile = new Map<string, string>();
      for (const entry of place) {
        if (entry.file !== undefined) {
          byFile.set(resolve(entry.file), entry.path);
        }
      }

      for (const spec of tsSpecs) {
        const file = compiled.files.get(spec);
        const script = file === undefined ? undefined : byFile.get(resolve(file));
        if (file === undefined || script === undefined) {
          console.error(
            paint(
              `${displayPath(file ?? spec)} is not in the project's Rojo tree, so ` +
                "the compiled spec cannot be loaded. Mount the compiler's output, " +
                'e.g. "ReplicatedStorage": { "TS": { "$path": "out" } }.',
              "red",
            ),
          );
          return 2;
        }
        // the label is the file the test author edits; the world holds the Luau
        entries.push({ file, label: displayPath(spec), script });
      }
    }

    entries.sort((left, right) =>
      (left.label ?? left.file).localeCompare(right.label ?? right.file),
    );

    const stateDir = stateDirFor(args);
    const results = await runSpecFiles(entries, {
      place,
      modules,
      isolate,
      timeoutMs,
      display: displayPath,
      read: readTextFile,
      // specs share the project's mock data, so they see what a run would see
      ...(stateDir === undefined ? {} : { stateDir }),
      onSuite: (result) => {
        if (asJson) {
          return;
        }
        for (const line of formatSuite(result, { verbose: !asJson })) {
          console.log(line);
        }
      },
    });

    const summary = summarize(results, { durationMs: Date.now() - startedAt });
    if (asJson) {
      console.log(renderJsonReport(results, summary));
    } else {
      console.log();
      for (const line of formatSummary(summary)) {
        console.log(line);
      }
    }
    return summary.ok ? 0 : 1;
  };

  if (!watchMode) {
    return runOnce();
  }

  console.log(
    paint(`watching ${displayPath(root)} — Ctrl-C to stop`, "dim"),
  );
  await runOnce();

  return await new Promise<number>((resolveWatch) => {
    let timer: NodeJS.Timeout | undefined;
    const watcher = watch(root, { recursive: true }, (_event, filename) => {
      const changed = filename?.toString() ?? "";
      if (changed.length > 0 && !/\.(lua|luau|ts|tsx)$/i.test(changed)) {
        return;
      }
      clearTimeout(timer);
      // editors and compilers write in bursts; one run per burst is enough
      timer = setTimeout(() => {
        if (process.stdout.isTTY) {
          process.stdout.write("\u001b[2J\u001b[3J\u001b[H");
        }
        void runOnce();
      }, 120);
    });

    process.on("SIGINT", () => {
      watcher.close();
      clearTimeout(timer);
      resolveWatch(0);
    });
  });
}

const REPL_HELP = `\
Commands:
  :help                 show this message
  :advance <seconds>    move the simulated clock forward
  :tree [path]          print the DataModel below a path (default: game)
  :player <name>        add a mock player
  :stats                print runtime counters
  :quit                 exit

Anything else is evaluated as Luau against the live DataModel.`;

// one prompt implementation for bare, repl and --interactive; caller supplies the world
async function withConsole(
  args: ParsedArgs,
  options: {
    clock: "virtual" | "real";
    banner: (runtime: Runtime) => string;
    // runs before the prompt, e.g. loading a project's tree
    prepare?: (runtime: Runtime) => Promise<void>;
  },
): Promise<number> {
  const runtime = await startRuntime(args, options.clock);
  let live: LiveConsole | undefined;
  let detach: (() => void) | undefined;

  try {
    if (options.prepare !== undefined) {
      await options.prepare(runtime);
    }
    live = startConsole(() => runtime, options.banner(runtime));
    detach = attachOutput(runtime, (line) => {
      console.log(line);
      live?.reprompt();
    });

    await live.done;
    return 0;
  } finally {
    live?.close();
    detach?.();
    await runtime.dispose();
  }
}

// fresh empty world, nothing read from disk; use repl to load a project
export async function prompt(args: ParsedArgs): Promise<number> {
  return withConsole(args, {
    clock: flagString(args, "--clock") === "real" ? "real" : "virtual",
    banner: (runtime) => `MicroStudio ${VERSION} — ${runtime.luauVersion} — :help for commands`,
  });
}

export async function repl(args: ParsedArgs): Promise<number> {
  const projectFile = resolveProjectFile(args);

  return withConsole(args, {
    clock: "virtual",
    banner: () => "MicroStudio repl — :help for commands",
    prepare: async (runtime) => {
      if (projectFile === undefined) {
        return;
      }
      const serviceList = flagString(args, "--services");
      const services =
        serviceList === undefined
          ? DEFAULT_SERVICES
          : serviceList.split(",").map((s) => s.trim());
      await runtime.loadTree(buildPlaceEntries({ projectFile, services }));
      console.log(paint(`loaded ${projectFile}`, "dim"));
    },
  });
}

// a chunk from a file, -e or stdin
interface ScriptTarget {
  // chunk name, so errors read as `script.luau:12`
  label: string;
  read(): string;
  // set for files; --watch re-runs them
  file?: string;
}

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) {
    chunks.push(chunk as Buffer);
  }
  return readText(Buffer.concat(chunks).toString("utf8"));
}

// Luau's lexer rejects a leading BOM; windows editors write them
function readText(text: string): string {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

function readTextFile(file: string): string {
  return readText(readFileSync(file, "utf8"));
}

// fresh world with the full Roblox API; nothing read from disk
// prints nothing on success, exits non-zero if the script errored
export async function standalone(args: ParsedArgs, command: string): Promise<number> {
  const inline = flagString(args, "-e") ?? flagString(args, "--eval");
  if (inline !== undefined) {
    return runScript(args, { label: "[eval]", read: () => inline });
  }

  const named = command.length > 0 ? command : (args.positionals[0] ?? "");
  if (named.length === 0) {
    return prompt(args);
  }

  if (named === "-") {
    const source = await readStdin();
    return runScript(args, { label: "[stdin]", read: () => source });
  }

  const file = resolve(named);
  if (!existsSync(file) || statSync(file).isDirectory()) {
    console.error(`Cannot find a script at '${named}'.`);
    return 2;
  }

  return runScript(args, {
    // the path as typed, so errors read `./x.luau:3`, not a `..\..\` chain
    label: named,
    file,
    read: () => readTextFile(file),
  });
}

async function runScript(args: ParsedArgs, target: ScriptTarget): Promise<number> {
  const requested = flagString(args, "--clock");
  const interactive = flagBool(args, "--interactive");
  // scripts run on the virtual clock and finish; --interactive or --clock real changes that
  const clock: "virtual" | "real" =
    requested === "real" || (requested !== "virtual" && interactive) ? "real" : "virtual";

  let runtime = await startRuntime(args, clock);
  let live: LiveConsole | undefined;
  const write = (line: string) => {
    console.log(line);
    live?.reprompt();
  };
  // a warning from a simulated service is diagnostic, so it does not join the script's output
  let detach = attachOutput(
    runtime,
    write,
    interactive ? write : (line: string) => console.error(line),
  );

  let stopWatching: (() => void) | undefined;
  let armWatch: (() => void) | undefined;
  let clearPump: (() => void) | undefined;
  let failed = false;

  const execute = async (): Promise<void> => {
    failed = false;
    try {
      await runtime.eval(target.read(), { mode: "statement", name: target.label });
    } catch (error) {
      // a chunk that will not compile never becomes a script error, so report it here
      failed = true;
      console.error(paint(error instanceof Error ? error.message : String(error), "red"));
      return;
    }
    // work the script scheduled is part of the program, so it finishes first
    // with a prompt attached the world keeps running instead
    if (!interactive) {
      await runtime.drain();
    }
  };

  const stopped = new Promise<void>((resolvePromise) => {
    process.once("SIGINT", () => resolvePromise());
    process.once("SIGTERM", () => resolvePromise());
  });

  try {
    await execute();

    if (flagBool(args, "--watch") && target.file !== undefined) {
      const file = target.file;
      // re-armed each re-run: the watcher must survive a replaced runtime
      armWatch = () => {
        stopWatching?.();
        stopWatching = watchProject({
          directories: [],
          files: [file],
          onReload: async (changed) => {
            // same rule as dev: a re-run starts from a fresh world
            const previous = runtime;
            detach();
            await previous.dispose();
            runtime = await startRuntime(args, clock);
            detach = attachOutput(runtime, write);
            console.log(paint(`↻ re-run — ${changed}`, "cyan"));
            try {
              await execute();
            } catch (error) {
              console.error(
                paint(error instanceof Error ? error.message : String(error), "red"),
              );
            }
            armWatch?.();
          },
        });
      };
      armWatch();
      console.log(paint(`watching ${target.label} — re-run on save`, "dim"));
    }

    if (!interactive) {
      if (armWatch !== undefined) {
        await stopped;
      }
      return failed || runtime.errors().length > 0 ? 1 : 0;
    }

    live = startConsole(
      () => runtime,
      `MicroStudio ${VERSION} — ${runtime.luauVersion} — ${clock} clock — :help for commands`,
    );

    // as in dev: on a wall clock timers only fire when the scheduler is pumped
    const pump =
      clock === "real"
        ? setInterval(() => {
            void runtime.pump().catch(() => undefined);
          }, 16)
        : undefined;
    clearPump = () => {
      if (pump !== undefined) {
        clearInterval(pump);
      }
    };

    await Promise.race([live.done, stopped]);
    stopWatching?.();

    const errors = runtime.errors().length;
    console.log(paint(`stopped — ${errors} error(s)`, "dim"));
    return errors > 0 ? 1 : 0;
  } finally {
    stopWatching?.();
    clearPump?.();
    live?.close();
    detach();
    await runtime.dispose();
  }
}

function printTree(node: InstanceJson, indent = 0): void {
  const pad = "  ".repeat(indent);
  console.log(`${pad}${node.name ?? "?"} (${node.className ?? "?"})`);
  for (const child of node.children ?? []) {
    printTree(child, indent + 1);
  }
}

export function help(): number {
  console.log(
    [
      "microstudio — a local, headless Roblox development runtime",
      "",
      "USAGE:",
      "  microstudio [file.luau]                run a script on a fresh world",
      "  microstudio -e \"<code>\"                 run one line on a fresh world",
      "  microstudio -                          run a script read from stdin",
      "  microstudio                            prompt, with no project loaded",
      "  microstudio run [file.luau] [-e <code>] [--interactive] [--watch]",
      "  microstudio dev  [dir] [--project <file>] [--services a,b]",
      "                  [--once] [--interactive] [--watch] [--clock virtual|real]",
      "                  [--state-dir <path>]",
      "  microstudio test [dir] [--project <file>] [--include <glob>]",
      "                  [--exclude <glob>] [--module path=file] [--isolate]",
      "                  [--timeout <ms>] [--json] [--watch] [--tsconfig <file>]",
      "                  [--no-compile] [--state-dir <path>]",
      "  microstudio repl [dir] [--project <file>]",
      "",
      "COMMANDS:",
      "  dev    compile nothing, run everything: read the Rojo project, load the",
      "         compiled Luau into a live DataModel, and run the server scripts.",
      "         Runs until Ctrl-C. With --interactive it keeps running *and*",
      "         prompts for Luau, so you can inspect or poke a live game.",
      "         With --watch it restarts the session when a mount changes.",
      "  test   run the Luau in *.spec.luau files and report failures. Each spec",
      "         gets its own runtime; the project's instances are loaded first,",
      "         but its scripts are not run. With --isolate the world is rebuilt",
      "         from the project before every test. Exits non-zero on failure.",
      "         A roblox-ts project can write *.spec.ts instead: those specs are",
      "         compiled by the project's own rbxtsc first, then run the same way",
      "         and reported under the .ts path. --no-compile skips that step, for",
      "         a project that compiles in a separate script. --tsconfig names the",
      "         config to compile with, defaulting to tsconfig.json.",
      "         Use dev --once instead to check that the project boots.",
      "  repl   inspect and manipulate the DataModel interactively, without",
      "         running the project's scripts.",
      "",
      "NO PROJECT NEEDED:",
      "  With no subcommand, microstudio is an interpreter for Luau with the",
      "  Roblox API available: Instance.new, game, workspace, Players, datatypes",
      "  and task all work, and nothing is read from disk. A script's own",
      "  scheduled work finishes before MicroStudio exits; pass --interactive to",
      "  keep the world running and type at it instead.",
      "",
      "ENVIRONMENT:",
      "  MICROSTUDIO_RUNTIME_BIN   path to the microstudio-runtime sidecar",
      "  MICROSTUDIO_STATE_DIR     where simulated services keep their json",
      "  MICROSTUDIO_SECRET_<NAME> a secret HttpService:GetSecret can read",
      "",
      "SIMULATED SERVICES:",
      "  Every Roblox service exists: game:GetService(\"TweenService\") works even",
      "  though nothing is tweened. A service with real local behaviour keeps its",
      "  data as plain json beside the project, so a project can seed it:",
      "    <project>/.microstudio/   beside a Rojo or roblox-ts project",
      "    --state-dir <path>        or MICROSTUDIO_STATE_DIR; ~ is your home",
      "  With neither, service data stays in memory: a script run from the command",
      "  line writes nothing, and workspace instances are never persisted at all.",
      "  A member with no local behaviour returns a fixed value and warns once.",
      "  See docs/services.md for the whole list.",
    ].join("\n"),
  );
  return 0;
}
