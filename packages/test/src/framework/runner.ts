import { readFileSync } from "node:fs";

import { createRuntime, type PlaceEntry, type Runtime } from "@microstudio/runtime";

import type {
  HookFailure,
  SuiteResult,
  SuiteSummary,
  TestResult,
  TestStatus,
} from "./types.ts";

// traceback frames from this chunk are filtered out, so the name is load-bearing
export const FRAMEWORK_CHUNK = "[framework]";

const frameworkSource = readFileSync(
  new URL("../luau/framework.luau", import.meta.url),
  "utf8",
);

// a deadlock is a failed test, not a hang
export const DEFAULT_TIMEOUT_MS = 5_000;

export interface RunSuiteOptions {
  source?: string;
  // default `[string]`
  file?: string;
  // DataModel path of the spec's own instance: it is run as a script, so `script`
  // resolves and roblox-ts's runtime lib can key its import cache
  script?: string;
  place?: PlaceEntry[];
  // source by path, dotted or slashed; testable without a Rojo project
  modules?: Record<string, string>;
  isolate?: boolean;
  // 0 disables the watchdog
  timeoutMs?: number;
  runtimeFactory?: () => Promise<Runtime>;
  // where simulated services keep their json; defaults to the sidecar's ~/.microstudio
  stateDir?: string;
  // streamed as each test finishes
  onResult?: (result: TestResult) => void;
}

// the default factory: virtual clock, and the caller's state directory when it named one
function defaultFactory(stateDir: string | undefined): () => Promise<Runtime> {
  const options = stateDir === undefined ? { clock: "virtual" as const } : { clock: "virtual" as const, stateDir };
  return () => createRuntime(options);
}

export class TestTimeoutError extends Error {
  constructor(label: string, timeoutMs: number) {
    super(`${label} did not finish within ${timeoutMs}ms`);
    this.name = "TestTimeoutError";
  }
}

// Luau cannot be interrupted, so a timeout is only detectable, not stoppable
async function withTimeout<T>(
  work: Promise<T>,
  timeoutMs: number,
  label: string,
): Promise<T> {
  if (timeoutMs <= 0) {
    return work;
  }
  let timer: NodeJS.Timeout | undefined;
  const expired = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new TestTimeoutError(label, timeoutMs)), timeoutMs);
  });
  // the losing promise must not become an unhandled rejection
  work.catch(() => {});
  try {
    return await Promise.race([work, expired]);
  } finally {
    clearTimeout(timer);
  }
}

// `lib/util` and `game.ReplicatedStorage.Util` name the same instance
function modulePath(key: string): string {
  const path = key.replaceAll("/", ".");
  return path.startsWith("game.") ? path : `game.${path}`;
}

// drops the `stack traceback:` header, C frames and the runner's own chunk
export function framesOf(traceback: string): string | undefined {
  const frames = traceback
    .split("\n")
    .map((line) => line.trim())
    .filter(
      (line) =>
        line.length > 0 &&
        line !== "stack traceback:" &&
        !line.startsWith("[C]:") &&
        !line.startsWith("[runner]:") &&
        !line.startsWith("MicroStudio.repl"),
    );
  return frames.length === 0 ? undefined : frames.join("\n");
}

export function moduleEntries(modules: Record<string, string>): PlaceEntry[] {
  return Object.entries(modules)
    .map(([key, source]) => ({
      path: modulePath(key),
      className: "ModuleScript",
      source,
    }))
    .sort((a, b) => a.path.split(".").length - b.path.split(".").length);
}

interface PlanEntry {
  index: number;
  name: string;
  fullName: string;
  mode: TestStatus | "run";
  depth: number;
}

interface RunOutcome {
  status: TestStatus;
  message?: string | null | undefined;
  traceback?: string | null | undefined;
  assertions?: number;
  hookFailures?: HookFailure[];
}

// a spec's runtime + loaded spec, rebuildable after a wedged test
class SuiteEnvironment {
  runtime: Runtime;
  readonly #file: string;
  readonly #source: string | undefined;
  readonly #script: string | undefined;
  readonly #specChunk: string;
  readonly #place: PlaceEntry[];
  readonly #isolate: boolean;
  readonly #timeoutMs: number;
  readonly #factory: () => Promise<Runtime>;
  readonly #disposeRuntimes: boolean;

  constructor(
    runtime: Runtime,
    options: RunSuiteOptions & { disposeRuntimes: boolean },
  ) {
    this.runtime = runtime;
    this.#file = options.file ?? "[string]";
    this.#source = options.source;
    this.#script = options.script;
    this.#specChunk = this.#file;
    this.#place = [...(options.place ?? []), ...moduleEntries(options.modules ?? {})];
    this.#isolate = options.isolate ?? false;
    this.#timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.#factory = options.runtimeFactory ?? defaultFactory(options.stateDir);
    this.#disposeRuntimes = options.disposeRuntimes;
  }

  async prepare(): Promise<string | undefined> {
    await this.#applyPlace();
    return this.#loadSpec();
  }

  async #applyPlace(): Promise<void> {
    if (this.#place.length > 0) {
      await this.runtime.loadTree(this.#place);
    }
  }

  // registry cleared after the framework loads, before the spec registers tests
  // pcall wrapper stays on line 1, so failure line numbers still match the file
  async #loadSpec(): Promise<string | undefined> {
    await this.runtime.eval(
      `__microstudio_test_framework_chunk = ${JSON.stringify(FRAMEWORK_CHUNK)}`,
      { name: "[runner]" },
    );
    await this.runtime.eval(frameworkSource, { name: FRAMEWORK_CHUNK });
    await this.runtime.eval("return __microstudio_test_reset_registry()", {
      name: "[runner]",
    });

    // a compiled spec is run from its instance, so `script` and the runtime lib work
    if (this.#script !== undefined) {
      const errorsBefore = this.runtime.errors().length;
      try {
        await this.runtime.runScript(this.#script);
      } catch (error) {
        return error instanceof Error ? error.message : String(error);
      }
      // an error in the spec's own body is reported, not left as a silent no-tests
      const failure = this.runtime.errors().slice(errorsBefore)[0];
      if (failure !== undefined) {
        return `${failure.location}: ${failure.message}`;
      }
      return undefined;
    }

    const wrapped = [
      `local __ok, __err = pcall(function() ${this.#source ?? ""}`,
      "end)",
      "local __out = { ok = __ok }",
      "if not __ok then __out.err = tostring(__err) end",
      "return __out",
    ].join("\n");

    try {
      const loaded = (await this.runtime.eval(wrapped, {
        name: this.#specChunk,
      })) as { ok: boolean; err?: string } | null;
      if (loaded === null || loaded.ok !== true) {
        return loaded?.err ?? "the spec did not finish loading";
      }
      return undefined;
    } catch (error) {
      // a compile error never reaches the wrapper
      return error instanceof Error ? error.message : String(error);
    }
  }

  async plan(): Promise<PlanEntry[]> {
    const plan = (await this.runtime.eval("return __microstudio_test_plan()")) as {
      entries: PlanEntry[];
    } | null;
    return plan?.entries ?? [];
  }

  async #rebuild(): Promise<void> {
    await this.runtime.dispose();
    this.runtime = await this.#factory();
    const failure = await this.#loadSpec();
    if (failure !== undefined) {
      throw new Error(`could not reload the spec after a timeout: ${failure}`);
    }
  }

  async run(entry: PlanEntry): Promise<TestResult> {
    const startedAt = Date.now();
    const logsBefore = this.runtime.logs().length;
    const errorsBefore = this.runtime.errors().length;

    let outcome: RunOutcome;
    if (entry.mode !== "run") {
      // the framework already knows; a skip needs no world work
      outcome = await this.#invoke(entry);
    } else {
      if (this.#isolate) {
        await this.runtime.resetWorld();
        await this.#applyPlace();
      }
      outcome = await this.#invoke(entry);
    }

    const logs = this.runtime
      .logs()
      .slice(logsBefore)
      // an error is both a log and a notification; the error path reports it once
      .filter((log) => log.level !== "error")
      .map((log) => log.text);

    // an error in a spawned thread is a test failure; nothing else would notice
    const errors = this.runtime.errors().slice(errorsBefore);
    const first = errors[0];
    if (first !== undefined && outcome.status === "passed") {
      outcome = {
        ...outcome,
        status: "failed",
        message: `${first.location}: ${first.message}`,
        traceback: framesOf(first.traceback),
      };
    }

    const result: TestResult = {
      name: entry.fullName,
      shortName: entry.name,
      file: this.#file,
      depth: entry.depth,
      status: outcome.status,
      durationMs: Date.now() - startedAt,
      assertions: outcome.assertions ?? 0,
      logs,
    };
    if (outcome.message != null && outcome.message !== "") {
      result.message = outcome.message;
    }
    if (outcome.traceback != null && outcome.traceback !== "") {
      result.traceback = outcome.traceback;
    }
    if (outcome.hookFailures !== undefined && outcome.hookFailures.length > 0) {
      result.hookFailures = outcome.hookFailures;
    }
    return result;
  }

  async #invoke(entry: PlanEntry): Promise<RunOutcome> {
    const call = this.runtime.eval(`return __microstudio_test_run(${entry.index})`);
    try {
      const outcome = (await withTimeout(
        call,
        this.#timeoutMs,
        entry.fullName,
      )) as RunOutcome | null;
      return outcome ?? {
        status: "failed",
        message: "the test runner returned nothing",
      };
    } catch (error) {
      if (error instanceof TestTimeoutError) {
        // the vm is still busy with the abandoned test, so it is replaced
        await this.#rebuild();
        return { status: "failed", message: error.message };
      }
      await this.#rebuild().catch(() => {});
      return {
        status: "failed",
        message: error instanceof Error ? error.message : String(error),
      };
    }
  }

  async dispose(): Promise<void> {
    if (this.#disposeRuntimes) {
      await this.runtime.dispose();
    }
  }
}

export async function runSuite(options: RunSuiteOptions): Promise<SuiteResult> {
  const startedAt = Date.now();
  const file = options.file ?? "[string]";
  if (options.source === undefined && options.script === undefined) {
    throw new Error("runSuite needs a source string or a script path");
  }
  const runtime = await (options.runtimeFactory ?? defaultFactory(options.stateDir))();
  const environment = new SuiteEnvironment(runtime, {
    ...options,
    disposeRuntimes: true,
  });

  let logsBefore = runtime.logs().length;
  try {
    const loadError = await environment.prepare();
    const loadLogs = environment.runtime.logs().map((log) => log.text);
    if (loadError !== undefined) {
      return {
        file,
        tests: [],
        loadError,
        logs: loadLogs,
        durationMs: Date.now() - startedAt,
      };
    }

    const tests: TestResult[] = [];
    for (const entry of await environment.plan()) {
      const result = await environment.run(entry);
      tests.push(result);
      options.onResult?.(result);
    }

    return {
      file,
      tests,
      logs: loadLogs,
      durationMs: Date.now() - startedAt,
    };
  } finally {
    await environment.dispose();
  }
}

export interface SpecEntry {
  file: string;
  // how the spec is named in the output; defaults to file
  label?: string;
  source?: string;
  // DataModel path of the spec's instance, for a compiled spec
  script?: string;
}

// one runtime per entry, in order
export async function runSpecFiles(
  entries: readonly (string | SpecEntry)[],
  options: {
    read?: (file: string) => string;
    display?: (file: string) => string;
    onSuite?: (result: SuiteResult) => void;
    onResult?: (result: TestResult) => void;
  } & Omit<RunSuiteOptions, "source" | "file" | "script"> = {},
): Promise<SuiteResult[]> {
  const read = options.read ?? ((file: string) => readFileSync(file, "utf8"));
  const display = options.display ?? ((file: string) => file);
  const results: SuiteResult[] = [];

  for (const item of entries) {
    const entry: SpecEntry = typeof item === "string" ? { file: item } : item;
    // a compiled spec is read from the world, not from disk
    const source =
      entry.source ?? (entry.script === undefined ? read(entry.file) : undefined);
    const result = await runSuite({
      ...options,
      ...(source === undefined ? {} : { source }),
      ...(entry.script === undefined ? {} : { script: entry.script }),
      file: entry.label ?? display(entry.file),
    });
    results.push(result);
    options.onSuite?.(result);
  }

  return results;
}

export function summarize(
  results: readonly SuiteResult[],
  options: { durationMs?: number } = {},
): SuiteSummary {
  const summary: SuiteSummary = {
    files: results.length,
    loadFailures: 0,
    failedFiles: 0,
    tests: 0,
    passed: 0,
    failed: 0,
    skipped: 0,
    todo: 0,
    assertions: 0,
    durationMs: 0,
    ok: true,
  };
  let wallTime = 0;

  for (const result of results) {
    wallTime += result.durationMs;
    if (result.loadError !== undefined) {
      summary.loadFailures += 1;
      summary.ok = false;
    }
    if (
      result.loadError !== undefined ||
      result.tests.some((entry) => entry.status === "failed")
    ) {
      summary.failedFiles += 1;
    }
    for (const test of result.tests) {
      summary.tests += 1;
      summary.assertions += test.assertions;
      switch (test.status) {
        case "passed":
          summary.passed += 1;
          break;
        case "failed":
          summary.failed += 1;
          summary.ok = false;
          break;
        case "skipped":
          summary.skipped += 1;
          break;
        case "todo":
          summary.todo += 1;
          break;
      }
    }
  }

  summary.durationMs = options.durationMs ?? wallTime;
  return summary;
}

export interface LuaTestOptions
  extends Omit<RunSuiteOptions, "source" | "file"> {
  // default the test's own name
  file?: string;
}

// the snippet is the test body, so it can expect, task.wait and use the world
// a failure throws, which is what makes node:test and vitest report it
export async function luaTest(
  name: string,
  body: string,
  options: LuaTestOptions = {},
): Promise<TestResult> {
  const source = [
    `it(${JSON.stringify(name)}, function()`,
    body,
    "end)",
  ].join("\n");

  const results = await runSuite({ ...options, source });

  if (results.loadError !== undefined) {
    throw new Error(`${results.file}: ${results.loadError}`);
  }
  const test = results.tests[0];
  if (test === undefined) {
    throw new Error(`${results.file}: the test was never registered`);
  }
  if (test.status === "failed") {
    throw new Error(formatFailure(test));
  }
  return test;
}

// location, reason, then the call chain
export function formatFailure(test: TestResult): string {
  const lines: string[] = [];
  for (const log of test.logs) {
    lines.push(`  ${log}`);
  }
  lines.push(`  ${test.name}`);
  if (test.message !== undefined) {
    lines.push(`  ${test.message}`);
  }
  if (test.traceback !== undefined) {
    const frames = test.traceback.split("\n").filter((frame) => frame.length > 0);
    // one frame is the location the message already has; more means a real chain
    if (frames.length > 1) {
      for (const frame of frames) {
        lines.push(`    at ${frame}`);
      }
    }
  }
  return lines.join("\n");
}
